//! Instruction execution. Cycle counts follow the AVR instruction set manual for the AVRe+
//! core (ATmega328P) and the 3-byte-PC variant (ATmega2560: +1 on CALL/RCALL/ICALL/RET/RETI).
//! Side effects are applied at the end of the instruction (cycle counter already advanced).

use crate::avr::{Avr, FLAG_C, FLAG_H, FLAG_I, FLAG_N, FLAG_S, FLAG_T, FLAG_V, FLAG_Z};
use crate::debug::CallFrame;
use crate::decode::{Op, is_two_word};
use crate::{CallKind, FaultKind, StopReason};

#[inline(always)]
fn nz_s(mut s: u8, res: u8) -> u8 {
    if res == 0 {
        s |= FLAG_Z;
    }
    if res & 0x80 != 0 {
        s |= FLAG_N;
    }
    // S = N ^ V
    if ((s >> 2) ^ (s >> 3)) & 1 != 0 {
        s |= FLAG_S;
    }
    s
}

impl Avr {
    #[inline(always)]
    fn r(&self, i: u8) -> u8 {
        self.regs[(i & 31) as usize]
    }
    #[inline(always)]
    fn set_r(&mut self, i: u8, v: u8) {
        self.regs[(i & 31) as usize] = v;
    }
    #[inline(always)]
    fn r16(&self, i: u8) -> u16 {
        let i = (i & 30) as usize;
        u16::from_le_bytes([self.regs[i], self.regs[i + 1]])
    }
    #[inline(always)]
    fn set_r16(&mut self, i: u8, v: u16) {
        let i = (i & 30) as usize;
        let [lo, hi] = v.to_le_bytes();
        self.regs[i] = lo;
        self.regs[i + 1] = hi;
    }

    // ---------------------------------------------------------------- data memory

    #[inline(always)]
    pub(crate) fn ld(&mut self, addr: u16) -> u8 {
        if !self.dbg.watches.is_empty() {
            self.watch_check(addr, false);
        }
        let a = addr as usize;
        if a >= self.sram_start {
            if a < self.data.len() { self.data[a] } else { 0 }
        } else if a < 32 {
            self.regs[a]
        } else {
            self.io_read(a)
        }
    }

    #[inline(always)]
    pub(crate) fn st(&mut self, addr: u16, v: u8) {
        if !self.dbg.watches.is_empty() {
            self.watch_check(addr, true);
        }
        let a = addr as usize;
        if a >= self.sram_start {
            if a < self.data.len() {
                self.data[a] = v;
            }
        } else if a < 32 {
            self.regs[a] = v;
        } else {
            self.io_write(a, v);
        }
    }

    #[inline(always)]
    pub(crate) fn push(&mut self, v: u8) {
        let sp = self.sp;
        if (sp as usize) < self.sram_start {
            self.stack_fault(sp);
        }
        self.st(sp, v);
        self.sp = sp.wrapping_sub(1);
    }

    #[inline(always)]
    pub(crate) fn pop(&mut self) -> u8 {
        self.sp = self.sp.wrapping_add(1);
        let sp = self.sp;
        self.ld(sp)
    }

    #[cold]
    fn stack_fault(&mut self, sp: u16) {
        if !self.fault_reported_sp && self.cfg.stop_on.faults {
            self.fault_reported_sp = true;
            let pc = self.pc.wrapping_sub(1) * 2;
            self.request_stop(StopReason::Fault {
                kind: FaultKind::StackOverflow { sp },
                pc,
            });
        }
    }

    pub(crate) fn push_pc(&mut self, pc: u32) {
        self.push(pc as u8);
        self.push((pc >> 8) as u8);
        if self.v.pc22 {
            self.push((pc >> 16) as u8);
        }
    }

    pub(crate) fn pop_pc(&mut self) -> u32 {
        let mut v = 0u32;
        if self.v.pc22 {
            v = (self.pop() as u32) << 16;
        }
        v |= (self.pop() as u32) << 8;
        v |= self.pop() as u32;
        v & self.pc_mask
    }

    #[inline(always)]
    fn flash_byte_at(&self, byte_addr: u32) -> u8 {
        let w = self.flash[((byte_addr >> 1) & self.pc_mask) as usize];
        if byte_addr & 1 == 0 {
            w as u8
        } else {
            (w >> 8) as u8
        }
    }

    // ---------------------------------------------------------------- flags

    #[inline(always)]
    fn flags_add(&mut self, a: u8, b: u8, res: u8) {
        let c = (a & b) | (b & !res) | (!res & a);
        let v = (a & b & !res) | (!a & !b & res);
        let mut s = self.sreg & !(FLAG_H | FLAG_S | FLAG_V | FLAG_N | FLAG_Z | FLAG_C);
        if c & 0x08 != 0 {
            s |= FLAG_H;
        }
        if c & 0x80 != 0 {
            s |= FLAG_C;
        }
        if v & 0x80 != 0 {
            s |= FLAG_V;
        }
        self.sreg = nz_s(s, res);
    }

    /// SUB/SBC/CP/CPC/SUBI/SBCI/CPI flags. `keep_z`: Z is only cleared, never set (SBC/CPC).
    #[inline(always)]
    fn flags_sub(&mut self, a: u8, b: u8, res: u8, keep_z: bool) {
        let c = (!a & b) | (b & res) | (res & !a);
        let v = (a & !b & !res) | (!a & b & res);
        let old_z = self.sreg & FLAG_Z;
        let mut s = self.sreg & !(FLAG_H | FLAG_S | FLAG_V | FLAG_N | FLAG_Z | FLAG_C);
        if c & 0x08 != 0 {
            s |= FLAG_H;
        }
        if c & 0x80 != 0 {
            s |= FLAG_C;
        }
        if v & 0x80 != 0 {
            s |= FLAG_V;
        }
        s = nz_s(s, res);
        if keep_z {
            s = (s & !FLAG_Z) | if res == 0 { old_z } else { 0 };
        }
        self.sreg = s;
    }

    #[inline(always)]
    fn flags_logic(&mut self, res: u8) {
        let s = self.sreg & !(FLAG_S | FLAG_V | FLAG_N | FLAG_Z);
        self.sreg = nz_s(s, res);
    }

    /// Flags for ASR/LSR/ROR: C = old bit 0, V = N ^ C.
    #[inline(always)]
    fn flags_shift(&mut self, old: u8, res: u8) {
        let mut s = self.sreg & !(FLAG_S | FLAG_V | FLAG_N | FLAG_Z | FLAG_C);
        if old & 1 != 0 {
            s |= FLAG_C;
        }
        if res == 0 {
            s |= FLAG_Z;
        }
        let n = res & 0x80 != 0;
        if n {
            s |= FLAG_N;
        }
        let v = n ^ (old & 1 != 0);
        if v {
            s |= FLAG_V;
        }
        if n ^ v {
            s |= FLAG_S;
        }
        self.sreg = s;
    }

    #[inline(always)]
    fn flags_mul(&mut self, res: u16, carry_bit: bool) {
        let mut s = self.sreg & !(FLAG_Z | FLAG_C);
        if carry_bit {
            s |= FLAG_C;
        }
        if res == 0 {
            s |= FLAG_Z;
        }
        self.sreg = s;
    }

    #[inline(always)]
    fn skip_next(&mut self) {
        let words = if is_two_word(self.flash[(self.pc & self.pc_mask) as usize]) {
            2
        } else {
            1
        };
        self.pc = (self.pc + words) & self.pc_mask;
        self.cycle += words as u64;
    }

    #[inline(always)]
    fn branch(&mut self, k: i32) {
        self.pc = (self.pc as i32 + k) as u32 & self.pc_mask;
    }

    fn do_call(&mut self, target: u32, ret: u32, call_pc: u32) {
        let sp_before = self.sp;
        self.push_pc(ret);
        if self.cfg.shadow_stack {
            self.dbg.push_frame(CallFrame {
                kind: CallKind::Call,
                call_pc: call_pc * 2,
                target: target * 2,
                return_pc: ret * 2,
                sp: sp_before.wrapping_sub(if self.v.pc22 { 3 } else { 2 }),
            });
        }
        self.pc = target & self.pc_mask;
    }

    fn do_ret(&mut self) {
        self.pc = self.pop_pc();
        if self.cfg.shadow_stack {
            self.dbg.pop_frames(self.sp);
        }
    }

    // ---------------------------------------------------------------- execute

    /// Executes one instruction at PC.
    #[inline(always)]
    pub(crate) fn step(&mut self) {
        let pc = self.pc;
        let op = self.ops[(pc & self.pc_mask) as usize];
        self.pc = (pc + 1) & self.pc_mask;
        self.instret += 1;
        let pc22 = self.v.pc22 as u64;
        match op {
            Op::Nop => self.cycle += 1,
            Op::Movw { d, r } => {
                self.cycle += 1;
                let v = self.r16(r);
                self.set_r16(d, v);
            }
            Op::Mul { d, r } => {
                self.cycle += 2;
                let res = self.r(d) as u16 * self.r(r) as u16;
                self.set_r16(0, res);
                self.flags_mul(res, res & 0x8000 != 0);
            }
            Op::Muls { d, r } => {
                self.cycle += 2;
                let res = (self.r(d) as i8 as i16).wrapping_mul(self.r(r) as i8 as i16) as u16;
                self.set_r16(0, res);
                self.flags_mul(res, res & 0x8000 != 0);
            }
            Op::Mulsu { d, r } => {
                self.cycle += 2;
                let res = (self.r(d) as i8 as i32 * self.r(r) as i32) as u16;
                self.set_r16(0, res);
                self.flags_mul(res, res & 0x8000 != 0);
            }
            Op::Fmul { d, r } => {
                self.cycle += 2;
                let p = self.r(d) as u16 * self.r(r) as u16;
                let res = p << 1;
                self.set_r16(0, res);
                self.flags_mul(res, p & 0x8000 != 0);
            }
            Op::Fmuls { d, r } => {
                self.cycle += 2;
                let p = (self.r(d) as i8 as i16).wrapping_mul(self.r(r) as i8 as i16) as u16;
                let res = p << 1;
                self.set_r16(0, res);
                self.flags_mul(res, p & 0x8000 != 0);
            }
            Op::Fmulsu { d, r } => {
                self.cycle += 2;
                let p = (self.r(d) as i8 as i32 * self.r(r) as i32) as u16;
                let res = p << 1;
                self.set_r16(0, res);
                self.flags_mul(res, p & 0x8000 != 0);
            }
            Op::Add { d, r } => {
                self.cycle += 1;
                let (a, b) = (self.r(d), self.r(r));
                let res = a.wrapping_add(b);
                self.set_r(d, res);
                self.flags_add(a, b, res);
            }
            Op::Adc { d, r } => {
                self.cycle += 1;
                let (a, b) = (self.r(d), self.r(r));
                let res = a.wrapping_add(b).wrapping_add(self.sreg & FLAG_C);
                self.set_r(d, res);
                self.flags_add(a, b, res);
            }
            Op::Sub { d, r } => {
                self.cycle += 1;
                let (a, b) = (self.r(d), self.r(r));
                let res = a.wrapping_sub(b);
                self.set_r(d, res);
                self.flags_sub(a, b, res, false);
            }
            Op::Sbc { d, r } => {
                self.cycle += 1;
                let (a, b) = (self.r(d), self.r(r));
                let res = a.wrapping_sub(b).wrapping_sub(self.sreg & FLAG_C);
                self.set_r(d, res);
                self.flags_sub(a, b, res, true);
            }
            Op::Cp { d, r } => {
                self.cycle += 1;
                let (a, b) = (self.r(d), self.r(r));
                self.flags_sub(a, b, a.wrapping_sub(b), false);
            }
            Op::Cpc { d, r } => {
                self.cycle += 1;
                let (a, b) = (self.r(d), self.r(r));
                let res = a.wrapping_sub(b).wrapping_sub(self.sreg & FLAG_C);
                self.flags_sub(a, b, res, true);
            }
            Op::Cpse { d, r } => {
                self.cycle += 1;
                if self.r(d) == self.r(r) {
                    self.skip_next();
                }
            }
            Op::And { d, r } => {
                self.cycle += 1;
                let res = self.r(d) & self.r(r);
                self.set_r(d, res);
                self.flags_logic(res);
            }
            Op::Or { d, r } => {
                self.cycle += 1;
                let res = self.r(d) | self.r(r);
                self.set_r(d, res);
                self.flags_logic(res);
            }
            Op::Eor { d, r } => {
                self.cycle += 1;
                let res = self.r(d) ^ self.r(r);
                self.set_r(d, res);
                self.flags_logic(res);
            }
            Op::Mov { d, r } => {
                self.cycle += 1;
                let v = self.r(r);
                self.set_r(d, v);
            }
            Op::Cpi { d, k } => {
                self.cycle += 1;
                let a = self.r(d);
                self.flags_sub(a, k, a.wrapping_sub(k), false);
            }
            Op::Subi { d, k } => {
                self.cycle += 1;
                let a = self.r(d);
                let res = a.wrapping_sub(k);
                self.set_r(d, res);
                self.flags_sub(a, k, res, false);
            }
            Op::Sbci { d, k } => {
                self.cycle += 1;
                let a = self.r(d);
                let res = a.wrapping_sub(k).wrapping_sub(self.sreg & FLAG_C);
                self.set_r(d, res);
                self.flags_sub(a, k, res, true);
            }
            Op::Ori { d, k } => {
                self.cycle += 1;
                let res = self.r(d) | k;
                self.set_r(d, res);
                self.flags_logic(res);
            }
            Op::Andi { d, k } => {
                self.cycle += 1;
                let res = self.r(d) & k;
                self.set_r(d, res);
                self.flags_logic(res);
            }
            Op::Ldi { d, k } => {
                self.cycle += 1;
                self.set_r(d, k);
            }
            Op::Com { d } => {
                self.cycle += 1;
                let res = !self.r(d);
                self.set_r(d, res);
                let s = (self.sreg & !(FLAG_S | FLAG_V | FLAG_N | FLAG_Z)) | FLAG_C;
                self.sreg = nz_s(s, res);
            }
            Op::Neg { d } => {
                self.cycle += 1;
                let a = self.r(d);
                let res = 0u8.wrapping_sub(a);
                self.set_r(d, res);
                self.flags_sub(0, a, res, false);
            }
            Op::Swap { d } => {
                self.cycle += 1;
                let v = self.r(d);
                self.set_r(d, v.rotate_left(4));
            }
            Op::Inc { d } => {
                self.cycle += 1;
                let res = self.r(d).wrapping_add(1);
                self.set_r(d, res);
                let mut s = self.sreg & !(FLAG_S | FLAG_V | FLAG_N | FLAG_Z);
                if res == 0x80 {
                    s |= FLAG_V;
                }
                self.sreg = nz_s(s, res);
            }
            Op::Dec { d } => {
                self.cycle += 1;
                let res = self.r(d).wrapping_sub(1);
                self.set_r(d, res);
                let mut s = self.sreg & !(FLAG_S | FLAG_V | FLAG_N | FLAG_Z);
                if res == 0x7F {
                    s |= FLAG_V;
                }
                self.sreg = nz_s(s, res);
            }
            Op::Asr { d } => {
                self.cycle += 1;
                let a = self.r(d);
                let res = (a >> 1) | (a & 0x80);
                self.set_r(d, res);
                self.flags_shift(a, res);
            }
            Op::Lsr { d } => {
                self.cycle += 1;
                let a = self.r(d);
                let res = a >> 1;
                self.set_r(d, res);
                self.flags_shift(a, res);
            }
            Op::Ror { d } => {
                self.cycle += 1;
                let a = self.r(d);
                let res = (a >> 1) | ((self.sreg & FLAG_C) << 7);
                self.set_r(d, res);
                self.flags_shift(a, res);
            }
            Op::Adiw { d, k } => {
                self.cycle += 2;
                let a = self.r16(d);
                let res = a.wrapping_add(k as u16);
                self.set_r16(d, res);
                let mut s = self.sreg & !(FLAG_S | FLAG_V | FLAG_N | FLAG_Z | FLAG_C);
                let ah = a & 0x8000 != 0;
                let rh = res & 0x8000 != 0;
                if !ah && rh {
                    s |= FLAG_V;
                }
                if ah && !rh {
                    s |= FLAG_C;
                }
                if rh {
                    s |= FLAG_N;
                }
                if res == 0 {
                    s |= FLAG_Z;
                }
                if rh ^ (!ah && rh) {
                    s |= FLAG_S;
                }
                self.sreg = s;
            }
            Op::Sbiw { d, k } => {
                self.cycle += 2;
                let a = self.r16(d);
                let res = a.wrapping_sub(k as u16);
                self.set_r16(d, res);
                let mut s = self.sreg & !(FLAG_S | FLAG_V | FLAG_N | FLAG_Z | FLAG_C);
                let ah = a & 0x8000 != 0;
                let rh = res & 0x8000 != 0;
                let v = ah && !rh;
                if v {
                    s |= FLAG_V;
                }
                if rh && !ah {
                    s |= FLAG_C;
                }
                if rh {
                    s |= FLAG_N;
                }
                if res == 0 {
                    s |= FLAG_Z;
                }
                if rh ^ v {
                    s |= FLAG_S;
                }
                self.sreg = s;
            }
            Op::Bset { s } => {
                self.cycle += 1;
                self.sreg |= 1 << s;
                if s == 7 {
                    // SEI: the next instruction always executes before any interrupt.
                    self.irq_skip_one = true;
                    self.loop_limit = 0;
                }
            }
            Op::Bclr { s } => {
                self.cycle += 1;
                self.sreg &= !(1 << s);
            }
            Op::Bst { d, b } => {
                self.cycle += 1;
                if self.r(d) & (1 << b) != 0 {
                    self.sreg |= FLAG_T;
                } else {
                    self.sreg &= !FLAG_T;
                }
            }
            Op::Bld { d, b } => {
                self.cycle += 1;
                let v = self.r(d);
                let v = if self.sreg & FLAG_T != 0 {
                    v | (1 << b)
                } else {
                    v & !(1 << b)
                };
                self.set_r(d, v);
            }
            Op::Sbrc { r, b } => {
                self.cycle += 1;
                if self.r(r) & (1 << b) == 0 {
                    self.skip_next();
                }
            }
            Op::Sbrs { r, b } => {
                self.cycle += 1;
                if self.r(r) & (1 << b) != 0 {
                    self.skip_next();
                }
            }
            Op::Brbs { s, k } => {
                if self.sreg & (1 << s) != 0 {
                    self.cycle += 2;
                    self.branch(k as i32);
                } else {
                    self.cycle += 1;
                }
            }
            Op::Brbc { s, k } => {
                if self.sreg & (1 << s) == 0 {
                    self.cycle += 2;
                    self.branch(k as i32);
                } else {
                    self.cycle += 1;
                }
            }
            Op::Rjmp { k } => {
                self.cycle += 2;
                self.branch(k as i32);
            }
            Op::Jmp { k } => {
                self.cycle += 3;
                self.pc = k & self.pc_mask;
            }
            Op::Ijmp => {
                self.cycle += 2;
                self.pc = self.r16(30) as u32;
            }
            Op::Eijmp => {
                self.cycle += 2;
                self.pc = (((self.eind as u32) << 16) | self.r16(30) as u32) & self.pc_mask;
            }
            Op::Rcall { k } => {
                self.cycle += 3 + pc22;
                let ret = self.pc;
                let target = (ret as i32 + k as i32) as u32 & self.pc_mask;
                self.do_call(target, ret, pc);
            }
            Op::Call { k } => {
                self.cycle += 4 + pc22;
                let ret = (pc + 2) & self.pc_mask;
                self.do_call(k, ret, pc);
            }
            Op::Icall => {
                self.cycle += 3 + pc22;
                let ret = self.pc;
                let target = self.r16(30) as u32;
                self.do_call(target, ret, pc);
            }
            Op::Eicall => {
                self.cycle += 4;
                let ret = self.pc;
                let target = ((self.eind as u32) << 16) | self.r16(30) as u32;
                self.do_call(target, ret, pc);
            }
            Op::Ret => {
                self.cycle += 4 + pc22;
                self.do_ret();
            }
            Op::Reti => {
                self.cycle += 4 + pc22;
                self.do_ret();
                self.sreg |= FLAG_I;
                // One instruction of the main program executes before the next interrupt.
                self.irq_skip_one = true;
                self.loop_limit = 0;
            }
            Op::In { d, a } => {
                self.cycle += 1;
                let v = self.ld(a as u16 + 0x20);
                self.set_r(d, v);
            }
            Op::Out { r, a } => {
                self.cycle += 1;
                let v = self.r(r);
                self.st(a as u16 + 0x20, v);
            }
            Op::Sbi { a, b } => {
                self.cycle += 2;
                self.io_write_bit(a as usize + 0x20, b, true);
            }
            Op::Cbi { a, b } => {
                self.cycle += 2;
                self.io_write_bit(a as usize + 0x20, b, false);
            }
            Op::Sbic { a, b } => {
                self.cycle += 1;
                if self.ld(a as u16 + 0x20) & (1 << b) == 0 {
                    self.skip_next();
                }
            }
            Op::Sbis { a, b } => {
                self.cycle += 1;
                if self.ld(a as u16 + 0x20) & (1 << b) != 0 {
                    self.skip_next();
                }
            }
            Op::Lds { d, k } => {
                self.cycle += 2;
                self.pc = (self.pc + 1) & self.pc_mask;
                let v = self.ld(k);
                self.set_r(d, v);
            }
            Op::Sts { r, k } => {
                self.cycle += 2;
                self.pc = (self.pc + 1) & self.pc_mask;
                let v = self.r(r);
                self.st(k, v);
            }
            Op::LdX { d } => {
                self.cycle += 2;
                let v = self.ld(self.r16(26));
                self.set_r(d, v);
            }
            Op::LdXInc { d } => {
                self.cycle += 2;
                let x = self.r16(26);
                self.set_r16(26, x.wrapping_add(1));
                let v = self.ld(x);
                self.set_r(d, v);
            }
            Op::LdXDec { d } => {
                self.cycle += 2;
                let x = self.r16(26).wrapping_sub(1);
                self.set_r16(26, x);
                let v = self.ld(x);
                self.set_r(d, v);
            }
            Op::LdYInc { d } => {
                self.cycle += 2;
                let y = self.r16(28);
                self.set_r16(28, y.wrapping_add(1));
                let v = self.ld(y);
                self.set_r(d, v);
            }
            Op::LdYDec { d } => {
                self.cycle += 2;
                let y = self.r16(28).wrapping_sub(1);
                self.set_r16(28, y);
                let v = self.ld(y);
                self.set_r(d, v);
            }
            Op::LddY { d, q } => {
                self.cycle += 2;
                let v = self.ld(self.r16(28).wrapping_add(q as u16));
                self.set_r(d, v);
            }
            Op::LdZInc { d } => {
                self.cycle += 2;
                let z = self.r16(30);
                self.set_r16(30, z.wrapping_add(1));
                let v = self.ld(z);
                self.set_r(d, v);
            }
            Op::LdZDec { d } => {
                self.cycle += 2;
                let z = self.r16(30).wrapping_sub(1);
                self.set_r16(30, z);
                let v = self.ld(z);
                self.set_r(d, v);
            }
            Op::LddZ { d, q } => {
                self.cycle += 2;
                let v = self.ld(self.r16(30).wrapping_add(q as u16));
                self.set_r(d, v);
            }
            Op::StX { r } => {
                self.cycle += 2;
                let v = self.r(r);
                self.st(self.r16(26), v);
            }
            Op::StXInc { r } => {
                self.cycle += 2;
                let v = self.r(r);
                let x = self.r16(26);
                self.set_r16(26, x.wrapping_add(1));
                self.st(x, v);
            }
            Op::StXDec { r } => {
                self.cycle += 2;
                let v = self.r(r);
                let x = self.r16(26).wrapping_sub(1);
                self.set_r16(26, x);
                self.st(x, v);
            }
            Op::StYInc { r } => {
                self.cycle += 2;
                let v = self.r(r);
                let y = self.r16(28);
                self.set_r16(28, y.wrapping_add(1));
                self.st(y, v);
            }
            Op::StYDec { r } => {
                self.cycle += 2;
                let v = self.r(r);
                let y = self.r16(28).wrapping_sub(1);
                self.set_r16(28, y);
                self.st(y, v);
            }
            Op::StdY { r, q } => {
                self.cycle += 2;
                let v = self.r(r);
                self.st(self.r16(28).wrapping_add(q as u16), v);
            }
            Op::StZInc { r } => {
                self.cycle += 2;
                let v = self.r(r);
                let z = self.r16(30);
                self.set_r16(30, z.wrapping_add(1));
                self.st(z, v);
            }
            Op::StZDec { r } => {
                self.cycle += 2;
                let v = self.r(r);
                let z = self.r16(30).wrapping_sub(1);
                self.set_r16(30, z);
                self.st(z, v);
            }
            Op::StdZ { r, q } => {
                self.cycle += 2;
                let v = self.r(r);
                self.st(self.r16(30).wrapping_add(q as u16), v);
            }
            Op::Push { r } => {
                self.cycle += 2;
                let v = self.r(r);
                self.push(v);
            }
            Op::Pop { d } => {
                self.cycle += 2;
                let v = self.pop();
                self.set_r(d, v);
            }
            Op::Lpm0 => {
                self.cycle += 3;
                let v = self.lpm_read(self.r16(30) as u32);
                self.regs[0] = v;
            }
            Op::Lpm { d } => {
                self.cycle += 3;
                let v = self.lpm_read(self.r16(30) as u32);
                self.set_r(d, v);
            }
            Op::LpmInc { d } => {
                self.cycle += 3;
                let z = self.r16(30);
                let v = self.lpm_read(z as u32);
                self.set_r16(30, z.wrapping_add(1));
                self.set_r(d, v);
            }
            Op::Elpm0 => {
                self.cycle += 3;
                let a = ((self.rampz as u32) << 16) | self.r16(30) as u32;
                let v = self.flash_byte_at(a);
                self.regs[0] = v;
            }
            Op::Elpm { d } => {
                self.cycle += 3;
                let a = ((self.rampz as u32) << 16) | self.r16(30) as u32;
                let v = self.flash_byte_at(a);
                self.set_r(d, v);
            }
            Op::ElpmInc { d } => {
                self.cycle += 3;
                let a = ((self.rampz as u32) << 16) | self.r16(30) as u32;
                let v = self.flash_byte_at(a);
                let n = a.wrapping_add(1);
                self.set_r16(30, n as u16);
                self.rampz = (n >> 16) as u8;
                self.set_r(d, v);
            }
            Op::Spm => {
                self.cycle += 1;
                self.spm_execute();
            }
            Op::Sleep => {
                self.cycle += 1;
                if self.sys.smcr & 1 != 0 {
                    self.enter_sleep();
                }
            }
            Op::Wdr => {
                self.cycle += 1;
                self.wdt_reset_counter();
            }
            Op::Break => {
                self.cycle += 1;
                if self.cfg.stop_on.break_instruction {
                    self.request_stop(StopReason::Break { pc: pc * 2 });
                }
            }
            Op::Invalid { opcode } => {
                self.cycle += 1;
                if self.cfg.stop_on.faults {
                    self.request_stop(StopReason::Fault {
                        kind: FaultKind::InvalidOpcode(opcode),
                        pc: pc * 2,
                    });
                }
            }
        }
    }

    /// LPM read: flash byte, or signature / fuse bytes when SPMCSR selects them.
    fn lpm_read(&mut self, z: u32) -> u8 {
        if let Some(v) = self.spm_lpm_override(z) {
            return v;
        }
        self.flash_byte_at(z)
    }

    /// Writes SREG through the I/O path (so enabling I with a pending interrupt is noticed).
    pub(crate) fn write_sreg(&mut self, v: u8) {
        let was = self.sreg & FLAG_I;
        self.sreg = v;
        if was == 0 && v & FLAG_I != 0 && self.pending != 0 {
            self.loop_limit = 0;
        }
    }
}
