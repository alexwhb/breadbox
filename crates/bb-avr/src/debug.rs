//! Debugger primitives: breakpoints (one flag per flash word), data watchpoints, a shadow call
//! stack maintained by CALL/RET/interrupts, and step filters evaluated after each instruction.

use crate::StopReason;
use crate::avr::Avr;

/// A data watchpoint on `len` bytes starting at data address `addr`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Watchpoint {
    pub addr: u16,
    pub len: u16,
    pub read: bool,
    pub write: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallKind {
    Call,
    Interrupt { vector: u8 },
}

/// One frame of the shadow call stack. Addresses are byte addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallFrame {
    pub kind: CallKind,
    /// Address of the CALL instruction (or the interrupted instruction).
    pub call_pc: u32,
    /// Entry address of the callee / interrupt vector.
    pub target: u32,
    /// Where execution resumes on return.
    pub return_pc: u32,
    /// SP right after the return address was pushed.
    pub sp: u16,
}

/// Condition that makes [`Avr::run`] stop with [`StopReason::StepDone`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepFilter {
    /// After this many instructions (interrupt entries count as one).
    Instructions(u64),
    /// When PC is outside every `[start, end)` byte range, the shadow stack is no deeper than
    /// `max_depth` (if set), and — if `ignore_interrupts_from` is set — no interrupt frame was
    /// entered at or above that depth.
    OutOfRanges {
        ranges: Vec<(u32, u32)>,
        max_depth: Option<usize>,
        ignore_interrupts_from: Option<usize>,
    },
    /// When the shadow stack becomes shallower than `depth` (step out).
    Return { depth: usize },
}

#[derive(Debug, Clone)]
pub(crate) struct Debug {
    pub bp: Vec<bool>,
    pub bp_count: usize,
    pub bp_skip: Option<u32>,
    pub watches: Vec<Watchpoint>,
    pub shadow: Vec<CallFrame>,
    pub step: Option<StepFilter>,
    pub pause_requested: bool,
}

const MAX_SHADOW: usize = 4096;

impl Debug {
    pub fn new(words: usize) -> Debug {
        Debug {
            bp: vec![false; words],
            bp_count: 0,
            bp_skip: None,
            watches: Vec::new(),
            shadow: Vec::new(),
            step: None,
            pause_requested: false,
        }
    }

    #[inline]
    pub fn active(&self) -> bool {
        self.bp_count > 0 || self.step.is_some()
    }

    #[inline]
    pub fn push_frame(&mut self, f: CallFrame) {
        if self.shadow.len() >= MAX_SHADOW {
            self.shadow.remove(0);
        }
        self.shadow.push(f);
    }

    /// After a return, SP is above every frame that has been unwound.
    #[inline]
    pub fn pop_frames(&mut self, sp: u16) {
        while let Some(top) = self.shadow.last() {
            if top.sp < sp {
                self.shadow.pop();
            } else {
                break;
            }
        }
    }
}

impl Avr {
    #[cold]
    pub(crate) fn watch_check(&mut self, addr: u16, write: bool) {
        let hit = self.dbg.watches.iter().any(|w| {
            addr >= w.addr
                && addr < w.addr.saturating_add(w.len.max(1))
                && if write { w.write } else { w.read }
        });
        if hit {
            let pc = self.pc.wrapping_sub(1) * 2;
            self.request_stop(StopReason::Watchpoint { addr, write, pc });
        }
    }

    pub(crate) fn step_filter_hit(&mut self) -> bool {
        let pc = self.pc * 2;
        let Debug { step, shadow, .. } = &mut self.dbg;
        let depth = shadow.len();
        let hit = match step {
            None => false,
            Some(StepFilter::Instructions(n)) => {
                *n = n.saturating_sub(1);
                *n == 0
            }
            Some(StepFilter::OutOfRanges {
                ranges,
                max_depth,
                ignore_interrupts_from,
            }) => {
                if ranges.iter().any(|&(a, b)| pc >= a && pc < b) {
                    false
                } else if max_depth.is_some_and(|m| depth > m) {
                    false
                } else if let Some(base) = *ignore_interrupts_from {
                    !shadow[base.min(depth)..]
                        .iter()
                        .any(|f| matches!(f.kind, crate::CallKind::Interrupt { .. }))
                } else {
                    true
                }
            }
            Some(StepFilter::Return { depth: d }) => depth < *d,
        };
        if hit {
            self.dbg.step = None;
        }
        hit
    }
}
