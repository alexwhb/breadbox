//! AVR instruction decoding. Flash is pre-decoded into one [`Op`] per word so the hot loop is a
//! single indexed load plus a jump table.

use std::fmt;

/// A decoded instruction. Register operands are register numbers (0–31); `k` fields are
/// already sign-extended / combined. Two-word instructions carry their second word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Nop,
    Movw {
        d: u8,
        r: u8,
    },
    Muls {
        d: u8,
        r: u8,
    },
    Mulsu {
        d: u8,
        r: u8,
    },
    Fmul {
        d: u8,
        r: u8,
    },
    Fmuls {
        d: u8,
        r: u8,
    },
    Fmulsu {
        d: u8,
        r: u8,
    },
    Cpc {
        d: u8,
        r: u8,
    },
    Sbc {
        d: u8,
        r: u8,
    },
    Add {
        d: u8,
        r: u8,
    },
    Cpse {
        d: u8,
        r: u8,
    },
    Cp {
        d: u8,
        r: u8,
    },
    Sub {
        d: u8,
        r: u8,
    },
    Adc {
        d: u8,
        r: u8,
    },
    And {
        d: u8,
        r: u8,
    },
    Eor {
        d: u8,
        r: u8,
    },
    Or {
        d: u8,
        r: u8,
    },
    Mov {
        d: u8,
        r: u8,
    },
    Cpi {
        d: u8,
        k: u8,
    },
    Sbci {
        d: u8,
        k: u8,
    },
    Subi {
        d: u8,
        k: u8,
    },
    Ori {
        d: u8,
        k: u8,
    },
    Andi {
        d: u8,
        k: u8,
    },
    LddY {
        d: u8,
        q: u8,
    },
    LddZ {
        d: u8,
        q: u8,
    },
    StdY {
        r: u8,
        q: u8,
    },
    StdZ {
        r: u8,
        q: u8,
    },
    Lds {
        d: u8,
        k: u16,
    },
    Sts {
        r: u8,
        k: u16,
    },
    LdZInc {
        d: u8,
    },
    LdZDec {
        d: u8,
    },
    LdYInc {
        d: u8,
    },
    LdYDec {
        d: u8,
    },
    LdX {
        d: u8,
    },
    LdXInc {
        d: u8,
    },
    LdXDec {
        d: u8,
    },
    StZInc {
        r: u8,
    },
    StZDec {
        r: u8,
    },
    StYInc {
        r: u8,
    },
    StYDec {
        r: u8,
    },
    StX {
        r: u8,
    },
    StXInc {
        r: u8,
    },
    StXDec {
        r: u8,
    },
    Lpm {
        d: u8,
    },
    LpmInc {
        d: u8,
    },
    Elpm {
        d: u8,
    },
    ElpmInc {
        d: u8,
    },
    Lpm0,
    Elpm0,
    Pop {
        d: u8,
    },
    Push {
        r: u8,
    },
    Com {
        d: u8,
    },
    Neg {
        d: u8,
    },
    Swap {
        d: u8,
    },
    Inc {
        d: u8,
    },
    Asr {
        d: u8,
    },
    Lsr {
        d: u8,
    },
    Ror {
        d: u8,
    },
    Dec {
        d: u8,
    },
    Bset {
        s: u8,
    },
    Bclr {
        s: u8,
    },
    Ret,
    Reti,
    Sleep,
    Break,
    Wdr,
    Spm,
    Ijmp,
    Eijmp,
    Icall,
    Eicall,
    /// `k` is a word address.
    Jmp {
        k: u32,
    },
    Call {
        k: u32,
    },
    Adiw {
        d: u8,
        k: u8,
    },
    Sbiw {
        d: u8,
        k: u8,
    },
    Cbi {
        a: u8,
        b: u8,
    },
    Sbic {
        a: u8,
        b: u8,
    },
    Sbi {
        a: u8,
        b: u8,
    },
    Sbis {
        a: u8,
        b: u8,
    },
    Mul {
        d: u8,
        r: u8,
    },
    /// `a` is the I/O address (0–63), i.e. data address − 0x20.
    In {
        d: u8,
        a: u8,
    },
    Out {
        r: u8,
        a: u8,
    },
    /// Relative word offset.
    Rjmp {
        k: i16,
    },
    Rcall {
        k: i16,
    },
    Ldi {
        d: u8,
        k: u8,
    },
    Brbs {
        s: u8,
        k: i8,
    },
    Brbc {
        s: u8,
        k: i8,
    },
    Bld {
        d: u8,
        b: u8,
    },
    Bst {
        d: u8,
        b: u8,
    },
    Sbrc {
        r: u8,
        b: u8,
    },
    Sbrs {
        r: u8,
        b: u8,
    },
    /// Undefined / unsupported on this part.
    Invalid {
        opcode: u16,
    },
}

impl Op {
    /// Number of flash words occupied by the instruction.
    pub fn words(&self) -> u32 {
        match self {
            Op::Jmp { .. } | Op::Call { .. } | Op::Lds { .. } | Op::Sts { .. } => 2,
            _ => 1,
        }
    }
}

/// True if `w` is the first word of a 2-word instruction (JMP, CALL, LDS, STS).
#[inline]
pub fn is_two_word(w: u16) -> bool {
    (w & 0xFE0E) == 0x940C
        || (w & 0xFE0E) == 0x940E
        || (w & 0xFE0F) == 0x9000
        || (w & 0xFE0F) == 0x9200
}

/// Decodes the instruction word `w` (followed by `w2`). `ext` enables ELPM/EIJMP/EICALL
/// (parts with more than 128 KiB of flash).
pub fn decode(w: u16, w2: u16, ext: bool) -> Op {
    let d5 = ((w >> 4) & 0x1F) as u8;
    let r5 = (((w >> 5) & 0x10) | (w & 0x0F)) as u8;
    let d4 = 16 + ((w >> 4) & 0x0F) as u8;
    let k8 = (((w >> 4) & 0xF0) | (w & 0x0F)) as u8;
    let inv = Op::Invalid { opcode: w };
    match w >> 12 {
        0x0 => match (w >> 10) & 3 {
            0 => match (w >> 8) & 3 {
                0 => {
                    if w == 0 {
                        Op::Nop
                    } else {
                        inv
                    }
                }
                1 => Op::Movw {
                    d: (((w >> 4) & 0xF) * 2) as u8,
                    r: ((w & 0xF) * 2) as u8,
                },
                2 => Op::Muls {
                    d: d4,
                    r: 16 + (w & 0xF) as u8,
                },
                _ => {
                    let d = 16 + ((w >> 4) & 7) as u8;
                    let r = 16 + (w & 7) as u8;
                    match ((w >> 7) & 1, (w >> 3) & 1) {
                        (0, 0) => Op::Mulsu { d, r },
                        (0, _) => Op::Fmul { d, r },
                        (_, 0) => Op::Fmuls { d, r },
                        _ => Op::Fmulsu { d, r },
                    }
                }
            },
            1 => Op::Cpc { d: d5, r: r5 },
            2 => Op::Sbc { d: d5, r: r5 },
            _ => Op::Add { d: d5, r: r5 },
        },
        0x1 => match (w >> 10) & 3 {
            0 => Op::Cpse { d: d5, r: r5 },
            1 => Op::Cp { d: d5, r: r5 },
            2 => Op::Sub { d: d5, r: r5 },
            _ => Op::Adc { d: d5, r: r5 },
        },
        0x2 => match (w >> 10) & 3 {
            0 => Op::And { d: d5, r: r5 },
            1 => Op::Eor { d: d5, r: r5 },
            2 => Op::Or { d: d5, r: r5 },
            _ => Op::Mov { d: d5, r: r5 },
        },
        0x3 => Op::Cpi { d: d4, k: k8 },
        0x4 => Op::Sbci { d: d4, k: k8 },
        0x5 => Op::Subi { d: d4, k: k8 },
        0x6 => Op::Ori { d: d4, k: k8 },
        0x7 => Op::Andi { d: d4, k: k8 },
        0x8 | 0xA => {
            let q = (((w >> 8) & 0x20) | ((w >> 7) & 0x18) | (w & 7)) as u8;
            let y = w & 0x8 != 0;
            let store = w & 0x200 != 0;
            match (store, y) {
                (false, false) => Op::LddZ { d: d5, q },
                (false, true) => Op::LddY { d: d5, q },
                (true, false) => Op::StdZ { r: d5, q },
                (true, true) => Op::StdY { r: d5, q },
            }
        }
        0x9 => decode_9(w, w2, ext, d5, r5),
        0xB => {
            let a = (((w >> 5) & 0x30) | (w & 0xF)) as u8;
            if w & 0x800 != 0 {
                Op::Out { r: d5, a }
            } else {
                Op::In { d: d5, a }
            }
        }
        0xC => Op::Rjmp {
            k: sext(w & 0xFFF, 12),
        },
        0xD => Op::Rcall {
            k: sext(w & 0xFFF, 12),
        },
        0xE => Op::Ldi { d: d4, k: k8 },
        _ => {
            if w & 0x800 == 0 {
                let k = sext((w >> 3) & 0x7F, 7) as i8;
                let s = (w & 7) as u8;
                if w & 0x400 != 0 {
                    Op::Brbc { s, k }
                } else {
                    Op::Brbs { s, k }
                }
            } else {
                if w & 0x8 != 0 {
                    return inv;
                }
                let b = (w & 7) as u8;
                match (w >> 9) & 3 {
                    0 => Op::Bld { d: d5, b },
                    1 => Op::Bst { d: d5, b },
                    2 => Op::Sbrc { r: d5, b },
                    _ => Op::Sbrs { r: d5, b },
                }
            }
        }
    }
}

fn decode_9(w: u16, w2: u16, ext: bool, d5: u8, r5: u8) -> Op {
    let inv = Op::Invalid { opcode: w };
    match (w >> 9) & 7 {
        0 => match w & 0xF {
            0x0 => Op::Lds { d: d5, k: w2 },
            0x1 => Op::LdZInc { d: d5 },
            0x2 => Op::LdZDec { d: d5 },
            0x4 => Op::Lpm { d: d5 },
            0x5 => Op::LpmInc { d: d5 },
            0x6 if ext => Op::Elpm { d: d5 },
            0x7 if ext => Op::ElpmInc { d: d5 },
            0x9 => Op::LdYInc { d: d5 },
            0xA => Op::LdYDec { d: d5 },
            0xC => Op::LdX { d: d5 },
            0xD => Op::LdXInc { d: d5 },
            0xE => Op::LdXDec { d: d5 },
            0xF => Op::Pop { d: d5 },
            _ => inv,
        },
        1 => match w & 0xF {
            0x0 => Op::Sts { r: d5, k: w2 },
            0x1 => Op::StZInc { r: d5 },
            0x2 => Op::StZDec { r: d5 },
            0x9 => Op::StYInc { r: d5 },
            0xA => Op::StYDec { r: d5 },
            0xC => Op::StX { r: d5 },
            0xD => Op::StXInc { r: d5 },
            0xE => Op::StXDec { r: d5 },
            0xF => Op::Push { r: d5 },
            _ => inv,
        },
        2 => match w & 0xF {
            0x0 => Op::Com { d: d5 },
            0x1 => Op::Neg { d: d5 },
            0x2 => Op::Swap { d: d5 },
            0x3 => Op::Inc { d: d5 },
            0x5 => Op::Asr { d: d5 },
            0x6 => Op::Lsr { d: d5 },
            0x7 => Op::Ror { d: d5 },
            0x8 => {
                if w & 0x100 == 0 {
                    let s = ((w >> 4) & 7) as u8;
                    if w & 0x80 == 0 {
                        Op::Bset { s }
                    } else {
                        Op::Bclr { s }
                    }
                } else {
                    match (w >> 4) & 0xF {
                        0x0 => Op::Ret,
                        0x1 => Op::Reti,
                        0x8 => Op::Sleep,
                        0x9 => Op::Break,
                        0xA => Op::Wdr,
                        0xC => Op::Lpm0,
                        0xD if ext => Op::Elpm0,
                        0xE => Op::Spm,
                        _ => inv,
                    }
                }
            }
            0x9 => match w {
                0x9409 => Op::Ijmp,
                0x9419 if ext => Op::Eijmp,
                0x9509 => Op::Icall,
                0x9519 if ext => Op::Eicall,
                _ => inv,
            },
            0xA => Op::Dec { d: d5 },
            0xC | 0xD => Op::Jmp {
                k: jmp_target(w, w2),
            },
            0xE | 0xF => Op::Call {
                k: jmp_target(w, w2),
            },
            _ => inv,
        },
        3 => {
            let d = 24 + ((w >> 4) & 3) as u8 * 2;
            let k = (((w >> 2) & 0x30) | (w & 0xF)) as u8;
            if w & 0x100 == 0 {
                Op::Adiw { d, k }
            } else {
                Op::Sbiw { d, k }
            }
        }
        4 | 5 => {
            let a = ((w >> 3) & 0x1F) as u8;
            let b = (w & 7) as u8;
            match (w >> 8) & 3 {
                0 => Op::Cbi { a, b },
                1 => Op::Sbic { a, b },
                2 => Op::Sbi { a, b },
                _ => Op::Sbis { a, b },
            }
        }
        _ => Op::Mul { d: d5, r: r5 },
    }
}

fn jmp_target(w: u16, w2: u16) -> u32 {
    let hi = ((((w >> 4) & 0x1F) << 1) | (w & 1)) as u32;
    (hi << 16) | w2 as u32
}

fn sext(v: u16, bits: u32) -> i16 {
    let shift = 16 - bits;
    ((v << shift) as i16) >> shift
}

const SREG_SET: [&str; 8] = ["sec", "sez", "sen", "sev", "ses", "seh", "set", "sei"];
const SREG_CLR: [&str; 8] = ["clc", "clz", "cln", "clv", "cls", "clh", "clt", "cli"];
const BR_SET: [&str; 8] = [
    "brcs", "breq", "brmi", "brvs", "brlt", "brhs", "brts", "brie",
];
const BR_CLR: [&str; 8] = [
    "brcc", "brne", "brpl", "brvc", "brge", "brhc", "brtc", "brid",
];

/// Disassembly in avr-objdump style (relative targets are shown as `.+N` byte offsets).
impl fmt::Display for Op {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use Op::*;
        match *self {
            Nop => write!(f, "nop"),
            Movw { d, r } => write!(f, "movw r{d}, r{r}"),
            Muls { d, r } => write!(f, "muls r{d}, r{r}"),
            Mulsu { d, r } => write!(f, "mulsu r{d}, r{r}"),
            Fmul { d, r } => write!(f, "fmul r{d}, r{r}"),
            Fmuls { d, r } => write!(f, "fmuls r{d}, r{r}"),
            Fmulsu { d, r } => write!(f, "fmulsu r{d}, r{r}"),
            Cpc { d, r } => write!(f, "cpc r{d}, r{r}"),
            Sbc { d, r } => write!(f, "sbc r{d}, r{r}"),
            Add { d, r } if d == r => write!(f, "lsl r{d}"),
            Add { d, r } => write!(f, "add r{d}, r{r}"),
            Cpse { d, r } => write!(f, "cpse r{d}, r{r}"),
            Cp { d, r } => write!(f, "cp r{d}, r{r}"),
            Sub { d, r } => write!(f, "sub r{d}, r{r}"),
            Adc { d, r } if d == r => write!(f, "rol r{d}"),
            Adc { d, r } => write!(f, "adc r{d}, r{r}"),
            And { d, r } if d == r => write!(f, "tst r{d}"),
            And { d, r } => write!(f, "and r{d}, r{r}"),
            Eor { d, r } if d == r => write!(f, "clr r{d}"),
            Eor { d, r } => write!(f, "eor r{d}, r{r}"),
            Or { d, r } => write!(f, "or r{d}, r{r}"),
            Mov { d, r } => write!(f, "mov r{d}, r{r}"),
            Cpi { d, k } => write!(f, "cpi r{d}, 0x{k:02X}"),
            Sbci { d, k } => write!(f, "sbci r{d}, 0x{k:02X}"),
            Subi { d, k } => write!(f, "subi r{d}, 0x{k:02X}"),
            Ori { d, k } => write!(f, "ori r{d}, 0x{k:02X}"),
            Andi { d, k } => write!(f, "andi r{d}, 0x{k:02X}"),
            LddY { d, q: 0 } => write!(f, "ld r{d}, Y"),
            LddY { d, q } => write!(f, "ldd r{d}, Y+{q}"),
            LddZ { d, q: 0 } => write!(f, "ld r{d}, Z"),
            LddZ { d, q } => write!(f, "ldd r{d}, Z+{q}"),
            StdY { r, q: 0 } => write!(f, "st Y, r{r}"),
            StdY { r, q } => write!(f, "std Y+{q}, r{r}"),
            StdZ { r, q: 0 } => write!(f, "st Z, r{r}"),
            StdZ { r, q } => write!(f, "std Z+{q}, r{r}"),
            Lds { d, k } => write!(f, "lds r{d}, 0x{k:04X}"),
            Sts { r, k } => write!(f, "sts 0x{k:04X}, r{r}"),
            LdZInc { d } => write!(f, "ld r{d}, Z+"),
            LdZDec { d } => write!(f, "ld r{d}, -Z"),
            LdYInc { d } => write!(f, "ld r{d}, Y+"),
            LdYDec { d } => write!(f, "ld r{d}, -Y"),
            LdX { d } => write!(f, "ld r{d}, X"),
            LdXInc { d } => write!(f, "ld r{d}, X+"),
            LdXDec { d } => write!(f, "ld r{d}, -X"),
            StZInc { r } => write!(f, "st Z+, r{r}"),
            StZDec { r } => write!(f, "st -Z, r{r}"),
            StYInc { r } => write!(f, "st Y+, r{r}"),
            StYDec { r } => write!(f, "st -Y, r{r}"),
            StX { r } => write!(f, "st X, r{r}"),
            StXInc { r } => write!(f, "st X+, r{r}"),
            StXDec { r } => write!(f, "st -X, r{r}"),
            Lpm { d } => write!(f, "lpm r{d}, Z"),
            LpmInc { d } => write!(f, "lpm r{d}, Z+"),
            Elpm { d } => write!(f, "elpm r{d}, Z"),
            ElpmInc { d } => write!(f, "elpm r{d}, Z+"),
            Lpm0 => write!(f, "lpm"),
            Elpm0 => write!(f, "elpm"),
            Pop { d } => write!(f, "pop r{d}"),
            Push { r } => write!(f, "push r{r}"),
            Com { d } => write!(f, "com r{d}"),
            Neg { d } => write!(f, "neg r{d}"),
            Swap { d } => write!(f, "swap r{d}"),
            Inc { d } => write!(f, "inc r{d}"),
            Asr { d } => write!(f, "asr r{d}"),
            Lsr { d } => write!(f, "lsr r{d}"),
            Ror { d } => write!(f, "ror r{d}"),
            Dec { d } => write!(f, "dec r{d}"),
            Bset { s } => write!(f, "{}", SREG_SET[s as usize & 7]),
            Bclr { s } => write!(f, "{}", SREG_CLR[s as usize & 7]),
            Ret => write!(f, "ret"),
            Reti => write!(f, "reti"),
            Sleep => write!(f, "sleep"),
            Break => write!(f, "break"),
            Wdr => write!(f, "wdr"),
            Spm => write!(f, "spm"),
            Ijmp => write!(f, "ijmp"),
            Eijmp => write!(f, "eijmp"),
            Icall => write!(f, "icall"),
            Eicall => write!(f, "eicall"),
            Jmp { k } => write!(f, "jmp 0x{:X}", k * 2),
            Call { k } => write!(f, "call 0x{:X}", k * 2),
            Adiw { d, k } => write!(f, "adiw r{d}, 0x{k:02X}"),
            Sbiw { d, k } => write!(f, "sbiw r{d}, 0x{k:02X}"),
            Cbi { a, b } => write!(f, "cbi 0x{a:02X}, {b}"),
            Sbic { a, b } => write!(f, "sbic 0x{a:02X}, {b}"),
            Sbi { a, b } => write!(f, "sbi 0x{a:02X}, {b}"),
            Sbis { a, b } => write!(f, "sbis 0x{a:02X}, {b}"),
            Mul { d, r } => write!(f, "mul r{d}, r{r}"),
            In { d, a } => write!(f, "in r{d}, 0x{a:02X}"),
            Out { r, a } => write!(f, "out 0x{a:02X}, r{r}"),
            Rjmp { k } => write!(f, "rjmp .{:+}", k as i32 * 2),
            Rcall { k } => write!(f, "rcall .{:+}", k as i32 * 2),
            Ldi { d, k } if k == 0xFF => write!(f, "ser r{d}"),
            Ldi { d, k } => write!(f, "ldi r{d}, 0x{k:02X}"),
            Brbs { s, k } => write!(f, "{} .{:+}", BR_SET[s as usize & 7], k as i32 * 2),
            Brbc { s, k } => write!(f, "{} .{:+}", BR_CLR[s as usize & 7], k as i32 * 2),
            Bld { d, b } => write!(f, "bld r{d}, {b}"),
            Bst { d, b } => write!(f, "bst r{d}, {b}"),
            Sbrc { r, b } => write!(f, "sbrc r{r}, {b}"),
            Sbrs { r, b } => write!(f, "sbrs r{r}, {b}"),
            Invalid { opcode } => write!(f, ".word 0x{opcode:04X}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_common_ops() {
        assert_eq!(decode(0x0000, 0, false), Op::Nop);
        assert_eq!(decode(0x0C01, 0, false), Op::Add { d: 0, r: 1 });
        assert_eq!(decode(0xE0F5, 0, false), Op::Ldi { d: 31, k: 5 });
        assert_eq!(decode(0x940C, 0x0034, false), Op::Jmp { k: 0x34 });
        assert_eq!(decode(0x940E, 0x1234, false), Op::Call { k: 0x1234 });
        assert_eq!(decode(0x95F8, 0, false), Op::Invalid { opcode: 0x95F8 });
        assert_eq!(decode(0x9508, 0, false), Op::Ret);
        assert_eq!(decode(0x9478, 0, false), Op::Bset { s: 7 });
        assert_eq!(decode(0x94F8, 0, false), Op::Bclr { s: 7 });
        assert_eq!(decode(0x9A25, 0, false), Op::Sbi { a: 4, b: 5 });
        assert_eq!(decode(0xCFFF, 0, false), Op::Rjmp { k: -1 });
        assert_eq!(decode(0xF7F1, 0, false), Op::Brbc { s: 1, k: -2 });
        assert_eq!(decode(0x9006, 0, false), Op::Invalid { opcode: 0x9006 });
        assert_eq!(decode(0x9006, 0, true), Op::Elpm { d: 0 });
        assert_eq!(decode(0x8180, 0, false), Op::LddZ { d: 24, q: 0 });
        assert_eq!(decode(0xA18D, 0, false), Op::LddY { d: 24, q: 37 });
        assert_eq!(decode(0x9631, 0, false), Op::Adiw { d: 26, k: 1 });
        assert_eq!(decode(0x0110, 0, false), Op::Movw { d: 2, r: 0 });
        assert_eq!(decode(0xFFFF, 0, false), Op::Invalid { opcode: 0xFFFF });
        assert!(is_two_word(0x9100));
        assert!(is_two_word(0x93F0));
        assert!(is_two_word(0x940C));
        assert!(!is_two_word(0x9409));
    }

    #[test]
    fn disassembles() {
        assert_eq!(decode(0xCFFF, 0, false).to_string(), "rjmp .-2");
        assert_eq!(decode(0x940C, 0x0034, false).to_string(), "jmp 0x68");
        assert_eq!(decode(0x2411, 0, false).to_string(), "clr r1");
    }
}
