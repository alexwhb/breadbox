//! Source-level debugger over a [`bb_avr::Avr`] and [`DebugInfo`].
//!
//! This is a *policy layer*: it never owns the run loop. Whoever drives the core (the co-sim
//! scheduler, a test, the firmware screen) calls `avr.run(..)` as usual and hands every stop to
//! [`Debugger::on_stop`], which answers [`Decision::Continue`] (e.g. a breakpoint whose hit
//! condition is not met yet, or a step that has not reached a new line) or
//! [`Decision::Pause`]. Stepping arms [`bb_avr::StepFilter`]s on the core, so a step costs no
//! more than normal execution.

use crate::FirmwareError;
use crate::debuginfo::{DebugInfo, FrameRegs, SourceLocation, VarValue};
use bb_avr::{Avr, AvrIo, CallKind, FaultKind, StepFilter, StopReason};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;

/// When a breakpoint actually pauses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
pub enum HitCondition {
    #[default]
    Always,
    /// Pause on exactly the n-th hit.
    Equals(u64),
    /// Pause on every hit from the n-th on.
    AtLeast(u64),
    /// Pause on every n-th hit.
    Every(u64),
}

impl HitCondition {
    pub fn matches(self, hits: u64) -> bool {
        match self {
            HitCondition::Always => true,
            HitCondition::Equals(n) => hits == n,
            HitCondition::AtLeast(n) => hits >= n,
            HitCondition::Every(n) => n > 0 && hits.is_multiple_of(n),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SourceBreakpoint {
    pub id: u32,
    /// File as resolved in the debug info.
    pub file: PathBuf,
    /// Line the user asked for.
    pub requested_line: u32,
    /// Line actually used (next line with code).
    pub line: u32,
    /// Byte addresses (one per function containing the line).
    pub addrs: Vec<u32>,
    pub condition: HitCondition,
    pub hits: u64,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum PauseReason {
    Breakpoint { id: u32, hits: u64 },
    Step,
    User,
    BreakInstruction,
    Fault(String),
    Watchpoint { addr: u16, write: bool },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PauseInfo {
    pub reason: PauseReason,
    pub pc: u32,
    pub location: Option<SourceLocation>,
    pub function: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum Decision {
    Continue,
    Pause(PauseInfo),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StackFrame {
    pub index: usize,
    pub function: Option<String>,
    /// Current pc for frame 0, the call instruction for callers.
    pub pc: u32,
    pub location: Option<SourceLocation>,
    pub interrupt: Option<u8>,
    pub cfa: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StepKind {
    Over,
    Into,
    Out,
    Instruction,
}

#[derive(Debug, Clone)]
struct StepState {
    kind: StepKind,
    depth: usize,
    function: Option<(u32, u32)>,
    awaiting_return: bool,
}

/// AVR call-saved registers: r2–r17, r28, r29.
const CALL_SAVED: u32 = 0x3003_FFFC;

pub struct Debugger {
    info: Arc<DebugInfo>,
    bps: Vec<SourceBreakpoint>,
    next_id: u32,
    step: Option<StepState>,
}

impl Debugger {
    pub fn new(info: Arc<DebugInfo>) -> Debugger {
        Debugger {
            info,
            bps: Vec::new(),
            next_id: 1,
            step: None,
        }
    }

    pub fn info(&self) -> &DebugInfo {
        &self.info
    }

    // ------------------------------------------------------------------ breakpoints

    /// Sets a breakpoint on `file:line` (file may be a tab name like `"sketch.ino"`).
    pub fn add_breakpoint(
        &mut self,
        avr: &mut Avr,
        file: &str,
        line: u32,
        condition: HitCondition,
    ) -> Result<SourceBreakpoint, FirmwareError> {
        let (actual, addrs) = self
            .info
            .line_addresses(file, line)
            .ok_or_else(|| FirmwareError::NoCode(format!("{file}:{line}")))?;
        let resolved = self
            .info
            .location(addrs[0])
            .map(|l| l.file)
            .unwrap_or_else(|| PathBuf::from(file));
        for &a in &addrs {
            avr.set_breakpoint(a);
        }
        let bp = SourceBreakpoint {
            id: self.next_id,
            file: resolved,
            requested_line: line,
            line: actual,
            addrs,
            condition,
            hits: 0,
            enabled: true,
        };
        self.next_id += 1;
        self.bps.push(bp.clone());
        Ok(bp)
    }

    pub fn remove_breakpoint(&mut self, avr: &mut Avr, id: u32) {
        self.bps.retain(|b| b.id != id);
        self.sync(avr);
    }

    pub fn set_enabled(&mut self, avr: &mut Avr, id: u32, enabled: bool) {
        if let Some(b) = self.bps.iter_mut().find(|b| b.id == id) {
            b.enabled = enabled;
        }
        self.sync(avr);
    }

    pub fn set_condition(&mut self, id: u32, condition: HitCondition) {
        if let Some(b) = self.bps.iter_mut().find(|b| b.id == id) {
            b.condition = condition;
        }
    }

    pub fn breakpoints(&self) -> &[SourceBreakpoint] {
        &self.bps
    }

    /// Re-installs the enabled breakpoints on `avr` (e.g. a fresh instance of the same build).
    pub fn sync(&self, avr: &mut Avr) {
        avr.clear_all_breakpoints();
        for b in self.bps.iter().filter(|b| b.enabled) {
            for &a in &b.addrs {
                avr.set_breakpoint(a);
            }
        }
    }

    /// Switches to new debug info after a re-flash, re-resolving breakpoints by file:line
    /// (hit counts are kept). Breakpoints whose line has no code any more are dropped.
    pub fn rebind(&mut self, info: Arc<DebugInfo>, avr: &mut Avr) {
        self.info = info;
        self.step = None;
        let old = std::mem::take(&mut self.bps);
        for mut b in old {
            let file = b.file.to_string_lossy().into_owned();
            if let Some((line, addrs)) = self.info.line_addresses(&file, b.requested_line) {
                b.line = line;
                b.addrs = addrs;
                self.bps.push(b);
            }
        }
        self.sync(avr);
    }

    // ------------------------------------------------------------------ stops

    fn pause_info(&self, avr: &Avr, reason: PauseReason) -> PauseInfo {
        let pc = avr.pc();
        PauseInfo {
            reason,
            pc,
            location: self.info.location(pc),
            function: self.info.function_name(pc),
        }
    }

    /// Decides what a stop means for the debugger.
    pub fn on_stop(&mut self, avr: &mut Avr, reason: &StopReason) -> Decision {
        match *reason {
            StopReason::Breakpoint { pc } => {
                let mut pause = None;
                for b in self
                    .bps
                    .iter_mut()
                    .filter(|b| b.enabled && b.addrs.contains(&pc))
                {
                    b.hits += 1;
                    if pause.is_none() && b.condition.matches(b.hits) {
                        pause = Some((b.id, b.hits));
                    }
                }
                match pause {
                    Some((id, hits)) => {
                        self.cancel_step(avr);
                        Decision::Pause(self.pause_info(avr, PauseReason::Breakpoint { id, hits }))
                    }
                    None => Decision::Continue,
                }
            }
            StopReason::StepDone => self.step_done(avr),
            StopReason::Paused => {
                self.cancel_step(avr);
                Decision::Pause(self.pause_info(avr, PauseReason::User))
            }
            StopReason::Break { .. } => {
                self.cancel_step(avr);
                Decision::Pause(self.pause_info(avr, PauseReason::BreakInstruction))
            }
            StopReason::Fault { kind, .. } => {
                self.cancel_step(avr);
                let msg = match kind {
                    FaultKind::InvalidOpcode(op) => format!("invalid opcode 0x{op:04X}"),
                    FaultKind::StackOverflow { sp } => format!("stack overflow (SP=0x{sp:04X})"),
                };
                Decision::Pause(self.pause_info(avr, PauseReason::Fault(msg)))
            }
            StopReason::Watchpoint { addr, write, .. } => {
                self.cancel_step(avr);
                Decision::Pause(self.pause_info(avr, PauseReason::Watchpoint { addr, write }))
            }
            _ => Decision::Continue,
        }
    }

    fn cancel_step(&mut self, avr: &mut Avr) {
        if self.step.take().is_some() {
            avr.set_step_filter(None);
        }
    }

    fn step_done(&mut self, avr: &mut Avr) -> Decision {
        let Some(mut st) = self.step.take() else {
            return Decision::Pause(self.pause_info(avr, PauseReason::Step));
        };
        let pc = avr.pc();
        let depth = avr.call_stack().len();
        match st.kind {
            StepKind::Instruction | StepKind::Out => {
                Decision::Pause(self.pause_info(avr, PauseReason::Step))
            }
            StepKind::Over | StepKind::Into => {
                let has_lines =
                    self.info.location(pc).is_some() && self.info.function_at(pc).is_some();
                if !has_lines {
                    if depth > 0 && (depth > st.depth || st.awaiting_return) {
                        // Entered code without debug info: run until it returns.
                        avr.set_step_filter(Some(StepFilter::Return { depth }));
                        st.awaiting_return = true;
                        self.step = Some(st);
                        return Decision::Continue;
                    }
                    return Decision::Pause(self.pause_info(avr, PauseReason::Step));
                }
                let func = self.info.function_at(pc).map(|f| (f.low_pc, f.high_pc));
                let same_fn = func == st.function && depth == st.depth;
                if (same_fn || st.awaiting_return) && !self.info.is_statement_start(pc) {
                    // Mid-line (back-edge into a condition, or back from code without line
                    // info): finish this line.
                    let ranges = self.info.line_ranges(pc);
                    if !ranges.is_empty() {
                        st.awaiting_return = false;
                        st.function = func;
                        st.depth = depth;
                        avr.set_step_filter(Some(self.range_filter(st.kind, ranges, depth)));
                        self.step = Some(st);
                        return Decision::Continue;
                    }
                }
                Decision::Pause(self.pause_info(avr, PauseReason::Step))
            }
        }
    }

    fn range_filter(&self, kind: StepKind, ranges: Vec<(u32, u32)>, depth: usize) -> StepFilter {
        StepFilter::OutOfRanges {
            ranges,
            max_depth: (kind == StepKind::Over).then_some(depth),
            ignore_interrupts_from: Some(depth),
        }
    }

    fn start_step(&mut self, avr: &mut Avr, kind: StepKind) {
        let pc = avr.pc();
        let depth = avr.call_stack().len();
        let function = self.info.function_at(pc).map(|f| (f.low_pc, f.high_pc));
        let filter = match kind {
            StepKind::Instruction => StepFilter::Instructions(1),
            StepKind::Out if depth > 0 => StepFilter::Return { depth },
            StepKind::Out => StepFilter::Instructions(1),
            StepKind::Over | StepKind::Into => {
                let ranges = self.info.line_ranges(pc);
                if ranges.is_empty() {
                    // No line info here: behave like step out (or a single instruction).
                    if depth > 0 {
                        StepFilter::Return { depth }
                    } else {
                        StepFilter::Instructions(1)
                    }
                } else {
                    self.range_filter(kind, ranges, depth)
                }
            }
        };
        avr.set_step_filter(Some(filter));
        self.step = Some(StepState {
            kind,
            depth,
            function,
            awaiting_return: false,
        });
    }

    /// Step to the next source line in this function (calls run to completion).
    pub fn step_over(&mut self, avr: &mut Avr) {
        self.start_step(avr, StepKind::Over);
    }
    /// Step to the next source line, entering calls.
    pub fn step_into(&mut self, avr: &mut Avr) {
        self.start_step(avr, StepKind::Into);
    }
    /// Run until the current function returns.
    pub fn step_out(&mut self, avr: &mut Avr) {
        self.start_step(avr, StepKind::Out);
    }
    /// One machine instruction.
    pub fn step_instruction(&mut self, avr: &mut Avr) {
        self.start_step(avr, StepKind::Instruction);
    }
    /// Pause at the next instruction boundary.
    pub fn pause(&mut self, avr: &mut Avr) {
        avr.request_pause();
    }
    pub fn is_stepping(&self) -> bool {
        self.step.is_some()
    }

    /// Convenience driver: runs `avr` (for at most `max_cycles`) until the debugger pauses.
    /// Pin changes and other non-debug stops are ignored. ADC sample requests must be answered
    /// by `io` (or pushed voltages).
    pub fn run_until_pause(
        &mut self,
        avr: &mut Avr,
        io: &mut dyn AvrIo,
        max_cycles: u64,
    ) -> Option<PauseInfo> {
        let until = avr.cycle() + max_cycles;
        loop {
            let r = avr.run(until, io);
            avr.take_pin_events();
            if let Decision::Pause(p) = self.on_stop(avr, &r) {
                return Some(p);
            }
            match r {
                StopReason::ReachedCycle => return None,
                StopReason::AdcSampleNeeded { .. } => return None,
                _ => {}
            }
        }
    }

    // ------------------------------------------------------------------ inspection

    /// Current source location.
    pub fn location(&self, avr: &Avr) -> Option<SourceLocation> {
        self.info.location(avr.pc())
    }

    fn frames(&self, avr: &Avr) -> Vec<(StackFrame, FrameRegs)> {
        let shadow = avr.call_stack();
        let n = shadow.len();
        let pcb = self.info.pc_bytes;
        let mut out = Vec::new();
        let mut regs = FrameRegs::from_avr(avr);
        for i in 0..=n {
            let pc = if i == 0 {
                avr.pc()
            } else {
                shadow[n - i].call_pc
            };
            let cfa = if i < n {
                Some(shadow[n - 1 - i].sp as u32 + pcb)
            } else {
                None
            };
            regs.pc = pc;
            regs.cfa = cfa.or_else(|| self.info.cfi_at(pc, &regs).map(|c| c.0));
            let interrupt = if i < n {
                match shadow[n - 1 - i].kind {
                    CallKind::Interrupt { vector } => Some(vector),
                    CallKind::Call => None,
                }
            } else {
                None
            };
            out.push((
                StackFrame {
                    index: i,
                    function: self.info.function_name(pc),
                    pc,
                    location: self.info.location(pc),
                    interrupt,
                    cfa: regs.cfa,
                },
                regs.clone(),
            ));
            if i < n {
                // Caller registers: CFI-restored call-saved registers, the rest unknown
                // (interrupt frames preserve everything their prologue saved).
                let mut caller = regs.clone();
                let was_isr = interrupt.is_some();
                if let Some(c) = self.info.unwind(avr, &regs) {
                    caller.regs = c.regs;
                    caller.known = c.known | (regs.known & CALL_SAVED);
                } else {
                    caller.known = regs.known & CALL_SAVED;
                }
                if was_isr {
                    caller.known = regs.known;
                }
                caller.sp = cfa.unwrap_or(regs.sp);
                regs = caller;
            }
        }
        out
    }

    /// Call stack, innermost first (shadow call stack + DWARF CFI for frame registers).
    pub fn call_stack(&self, avr: &Avr) -> Vec<StackFrame> {
        self.frames(avr).into_iter().map(|(f, _)| f).collect()
    }

    /// Parameters and locals in scope in frame `frame` (0 = innermost).
    pub fn locals(&self, avr: &Avr, frame: usize) -> Vec<VarValue> {
        let frames = self.frames(avr);
        let Some((_, regs)) = frames.get(frame) else {
            return vec![];
        };
        let pc = regs.pc;
        let Some(f) = self.info.function_at(pc) else {
            return vec![];
        };
        let fb = self.info.frame_base(f, avr, regs);
        f.vars
            .iter()
            .filter(|v| v.scope.is_empty() || v.scope.iter().any(|(a, b)| pc >= *a && pc < *b))
            .map(|v| self.info.read_variable(avr, v, regs, fb))
            .collect()
    }

    /// All globals with a static location.
    pub fn globals(&self, avr: &Avr) -> Vec<VarValue> {
        let regs = FrameRegs::from_avr(avr);
        self.info
            .globals
            .iter()
            .map(|g| self.info.read_variable(avr, g, &regs, None))
            .collect()
    }

    /// Looks a name up as a local of `frame`, then as a global.
    pub fn read(&self, avr: &Avr, name: &str, frame: usize) -> Option<VarValue> {
        if let Some(v) = self.locals(avr, frame).into_iter().find(|v| v.name == name) {
            return Some(v);
        }
        let regs = FrameRegs::from_avr(avr);
        self.info
            .global(name)
            .map(|g| self.info.read_variable(avr, g, &regs, None))
    }
}
