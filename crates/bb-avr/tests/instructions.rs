//! Instruction-level tests: flags and cycle counts for every opcode family (datasheet / AVR
//! instruction set manual values), on hand-assembled programs.

use bb_avr::{Avr, AvrConfig, Bootloader, FaultKind, Mcu, StopReason};

// ------------------------------------------------------------------ tiny assembler

fn rd(d: u8) -> u16 {
    ((d as u16) & 0x1F) << 4
}
fn rr(r: u8) -> u16 {
    (((r as u16) & 0x10) << 5) | ((r as u16) & 0x0F)
}
fn two(base: u16, d: u8, r: u8) -> u16 {
    base | rd(d) | rr(r)
}
fn imm(base: u16, d: u8, k: u8) -> u16 {
    assert!(d >= 16);
    base | (((k as u16) & 0xF0) << 4) | (((d - 16) as u16) << 4) | ((k as u16) & 0x0F)
}
fn add(d: u8, r: u8) -> u16 {
    two(0x0C00, d, r)
}
fn adc(d: u8, r: u8) -> u16 {
    two(0x1C00, d, r)
}
fn sub(d: u8, r: u8) -> u16 {
    two(0x1800, d, r)
}
fn sbc(d: u8, r: u8) -> u16 {
    two(0x0800, d, r)
}
fn cp(d: u8, r: u8) -> u16 {
    two(0x1400, d, r)
}
fn cpc(d: u8, r: u8) -> u16 {
    two(0x0400, d, r)
}
fn cpse(d: u8, r: u8) -> u16 {
    two(0x1000, d, r)
}
fn and(d: u8, r: u8) -> u16 {
    two(0x2000, d, r)
}
fn or(d: u8, r: u8) -> u16 {
    two(0x2800, d, r)
}
fn eor(d: u8, r: u8) -> u16 {
    two(0x2400, d, r)
}
fn mov(d: u8, r: u8) -> u16 {
    two(0x2C00, d, r)
}
fn mul(d: u8, r: u8) -> u16 {
    two(0x9C00, d, r)
}
fn ldi(d: u8, k: u8) -> u16 {
    imm(0xE000, d, k)
}
fn cpi(d: u8, k: u8) -> u16 {
    imm(0x3000, d, k)
}
fn subi(d: u8, k: u8) -> u16 {
    imm(0x5000, d, k)
}
fn sbci(d: u8, k: u8) -> u16 {
    imm(0x4000, d, k)
}
fn ori(d: u8, k: u8) -> u16 {
    imm(0x6000, d, k)
}
fn andi(d: u8, k: u8) -> u16 {
    imm(0x7000, d, k)
}
fn one(base: u16, d: u8) -> u16 {
    base | rd(d)
}
fn adiw(d: u8, k: u8) -> u16 {
    0x9600 | (((k as u16) & 0x30) << 2) | ((((d - 24) / 2) as u16) << 4) | ((k as u16) & 0xF)
}
fn sbiw(d: u8, k: u8) -> u16 {
    adiw(d, k) | 0x0100
}
fn muls(d: u8, r: u8) -> u16 {
    0x0200 | (((d - 16) as u16) << 4) | ((r - 16) as u16)
}
fn mulsu_like(base: u16, d: u8, r: u8) -> u16 {
    base | (((d - 16) as u16) << 4) | ((r - 16) as u16)
}
fn movw(d: u8, r: u8) -> u16 {
    0x0100 | (((d / 2) as u16) << 4) | ((r / 2) as u16)
}
fn rjmp(k: i16) -> u16 {
    0xC000 | (k as u16 & 0xFFF)
}
fn rcall(k: i16) -> u16 {
    0xD000 | (k as u16 & 0xFFF)
}
fn jmp(k: u32) -> [u16; 2] {
    [
        0x940C | ((((k >> 16) & 0x3E) as u16) << 3) | ((k >> 16) & 1) as u16,
        k as u16,
    ]
}
fn call(k: u32) -> [u16; 2] {
    let [a, b] = jmp(k);
    [a | 0x0002, b]
}
fn brbs(s: u8, k: i8) -> u16 {
    0xF000 | (((k as u16) & 0x7F) << 3) | s as u16
}
fn brbc(s: u8, k: i8) -> u16 {
    0xF400 | (((k as u16) & 0x7F) << 3) | s as u16
}
fn sbrc(r: u8, b: u8) -> u16 {
    0xFC00 | rd(r) | b as u16
}
fn sbrs(r: u8, b: u8) -> u16 {
    0xFE00 | rd(r) | b as u16
}
fn bst(r: u8, b: u8) -> u16 {
    0xFA00 | rd(r) | b as u16
}
fn bld(r: u8, b: u8) -> u16 {
    0xF800 | rd(r) | b as u16
}
fn in_(d: u8, a: u8) -> u16 {
    0xB000 | (((a as u16) & 0x30) << 5) | rd(d) | ((a as u16) & 0xF)
}
fn out(a: u8, r: u8) -> u16 {
    0xB800 | (((a as u16) & 0x30) << 5) | rd(r) | ((a as u16) & 0xF)
}
fn sbi(a: u8, b: u8) -> u16 {
    0x9A00 | ((a as u16) << 3) | b as u16
}
fn cbi(a: u8, b: u8) -> u16 {
    0x9800 | ((a as u16) << 3) | b as u16
}
fn sbic(a: u8, b: u8) -> u16 {
    0x9900 | ((a as u16) << 3) | b as u16
}
fn sbis(a: u8, b: u8) -> u16 {
    0x9B00 | ((a as u16) << 3) | b as u16
}
fn lds(d: u8, k: u16) -> [u16; 2] {
    [0x9000 | rd(d), k]
}
fn sts(k: u16, r: u8) -> [u16; 2] {
    [0x9200 | rd(r), k]
}
fn q_bits(q: u8) -> u16 {
    let q = q as u16;
    ((q & 0x20) << 8) | ((q & 0x18) << 7) | (q & 7)
}
fn ldd_y(d: u8, q: u8) -> u16 {
    0x8008 | rd(d) | q_bits(q)
}
fn ldd_z(d: u8, q: u8) -> u16 {
    0x8000 | rd(d) | q_bits(q)
}
fn std_y(q: u8, r: u8) -> u16 {
    0x8208 | rd(r) | q_bits(q)
}
fn std_z(q: u8, r: u8) -> u16 {
    0x8200 | rd(r) | q_bits(q)
}
fn push(r: u8) -> u16 {
    0x920F | rd(r)
}
fn pop(d: u8) -> u16 {
    0x900F | rd(d)
}
const NOP: u16 = 0x0000;
const RET: u16 = 0x9508;
const RETI: u16 = 0x9518;
const ICALL: u16 = 0x9509;
const IJMP: u16 = 0x9409;
const EICALL: u16 = 0x9519;
const EIJMP: u16 = 0x9419;
const SLEEP: u16 = 0x9588;
const BREAK: u16 = 0x9598;
const WDR: u16 = 0x95A8;
const LPM0: u16 = 0x95C8;
const ELPM0: u16 = 0x95D8;
const SPM: u16 = 0x95E8;
const SEI: u16 = 0x9478;
const CLI: u16 = 0x94F8;
fn bset(s: u8) -> u16 {
    0x9408 | ((s as u16) << 4)
}
fn bclr(s: u8) -> u16 {
    0x9488 | ((s as u16) << 4)
}

const C: u8 = 0x01;
const Z: u8 = 0x02;
const N: u8 = 0x04;
const V: u8 = 0x08;
const S: u8 = 0x10;
const H: u8 = 0x20;
const T: u8 = 0x40;
const I: u8 = 0x80;

fn cpu_for(mcu: Mcu, prog: &[u16]) -> Avr {
    let mut cfg = AvrConfig::new(mcu);
    cfg.bootloader = Bootloader::None;
    let mut a = Avr::new(cfg);
    a.load_program_words(prog);
    a
}

fn cpu(prog: &[u16]) -> Avr {
    cpu_for(Mcu::Atmega328p, prog)
}

/// Executes one instruction, returning the cycles it took.
fn step(a: &mut Avr) -> u64 {
    let c0 = a.cycle();
    let r = a.step_instruction(&mut ());
    assert!(matches!(r, StopReason::StepDone), "unexpected {r:?}");
    a.cycle() - c0
}

fn flags(a: &Avr, mask: u8) -> u8 {
    a.sreg() & mask
}

const ALL: u8 = C | Z | N | V | S | H;

// ------------------------------------------------------------------ arithmetic

#[test]
fn add_adc_16bit_carry_chain() {
    // 0x00FF + 0x0001 = 0x0100
    let mut a = cpu(&[add(24, 22), adc(25, 23)]);
    a.set_reg(24, 0xFF);
    a.set_reg(25, 0x00);
    a.set_reg(22, 0x01);
    a.set_reg(23, 0x00);
    assert_eq!(step(&mut a), 1);
    assert_eq!(a.reg(24), 0x00);
    assert_eq!(flags(&a, ALL), C | Z | H);
    assert_eq!(step(&mut a), 1);
    assert_eq!(a.reg(25), 0x01);
    assert_eq!(flags(&a, ALL), 0);
}

#[test]
fn add_overflow_and_half_carry() {
    let mut a = cpu(&[add(16, 17)]);
    a.set_reg(16, 0x7F);
    a.set_reg(17, 0x01);
    step(&mut a);
    assert_eq!(a.reg(16), 0x80);
    // V=1, N=1 → S=0
    assert_eq!(flags(&a, ALL), V | N | H);
    let mut a = cpu(&[add(16, 17)]);
    a.set_reg(16, 0x80);
    a.set_reg(17, 0x80);
    step(&mut a);
    assert_eq!(a.reg(16), 0x00);
    assert_eq!(flags(&a, ALL), C | Z | V | S);
}

#[test]
fn adc_uses_carry_in() {
    let mut a = cpu(&[bset(0), adc(16, 17)]);
    a.set_reg(16, 0x0F);
    a.set_reg(17, 0x00);
    step(&mut a);
    step(&mut a);
    assert_eq!(a.reg(16), 0x10);
    assert_eq!(flags(&a, ALL), H);
}

#[test]
fn sub_sbc_16bit_borrow_chain() {
    // 0x0100 - 0x0001 = 0x00FF
    let mut a = cpu(&[sub(24, 22), sbc(25, 23)]);
    a.set_reg(24, 0x00);
    a.set_reg(25, 0x01);
    a.set_reg(22, 0x01);
    a.set_reg(23, 0x00);
    step(&mut a);
    assert_eq!(a.reg(24), 0xFF);
    assert_eq!(flags(&a, ALL), C | H | N | S);
    step(&mut a);
    assert_eq!(a.reg(25), 0x00);
    // SBC never sets Z: previous Z was 0, result 0 → Z stays 0.
    assert_eq!(flags(&a, ALL), 0);
}

#[test]
fn sbc_z_flag_propagates_through_chain() {
    // 0x0000 - 0x0000: SUB sets Z, SBC keeps it.
    let mut a = cpu(&[sub(24, 22), sbc(25, 23)]);
    step(&mut a);
    assert_eq!(flags(&a, Z), Z);
    step(&mut a);
    assert_eq!(flags(&a, Z), Z);
    // 0x0100 - 0x0100 via CP/CPC
    let mut a = cpu(&[cp(24, 22), cpc(25, 23)]);
    a.set_reg(25, 1);
    a.set_reg(23, 1);
    step(&mut a);
    assert_eq!(flags(&a, Z), Z);
    step(&mut a);
    assert_eq!(flags(&a, Z | C), Z);
    // 0x0100 vs 0x0000: low equal, high differs → Z cleared by CPC.
    let mut a = cpu(&[cp(24, 22), cpc(25, 23)]);
    a.set_reg(25, 1);
    step(&mut a);
    step(&mut a);
    assert_eq!(flags(&a, Z | C), 0);
}

#[test]
fn subi_sbci_cpi_overflow() {
    let mut a = cpu(&[subi(16, 1), cpi(17, 0x10), sbci(18, 0), ldi(19, 0x5A)]);
    a.set_reg(16, 0x80);
    a.set_reg(17, 0x0F);
    a.set_reg(18, 0x00);
    step(&mut a);
    assert_eq!(a.reg(16), 0x7F);
    assert_eq!(flags(&a, ALL), V | S | H);
    step(&mut a);
    assert_eq!(a.reg(17), 0x0F, "CPI does not write");
    assert_eq!(flags(&a, C | N | Z), C | N);
    step(&mut a);
    assert_eq!(a.reg(18), 0xFF, "SBCI subtracts the carry");
    step(&mut a);
    assert_eq!(a.reg(19), 0x5A);
}

#[test]
fn logic_ops_clear_v() {
    let mut a = cpu(&[
        bset(3),
        and(16, 17),
        or(18, 19),
        eor(20, 20),
        andi(21, 0x0F),
        ori(22, 0x80),
    ]);
    a.set_reg(16, 0xF0);
    a.set_reg(17, 0x0F);
    a.set_reg(18, 0x80);
    a.set_reg(19, 0x01);
    a.set_reg(20, 0x55);
    a.set_reg(21, 0xF0);
    a.set_reg(22, 0x00);
    step(&mut a);
    step(&mut a);
    assert_eq!(a.reg(16), 0);
    assert_eq!(flags(&a, Z | V | N | S), Z);
    step(&mut a);
    assert_eq!(a.reg(18), 0x81);
    assert_eq!(flags(&a, Z | V | N | S), N | S);
    step(&mut a);
    assert_eq!(a.reg(20), 0);
    assert_eq!(flags(&a, Z), Z);
    step(&mut a);
    assert_eq!(a.reg(21), 0);
    step(&mut a);
    assert_eq!(a.reg(22), 0x80);
    assert_eq!(flags(&a, N | S | V), N | S);
}

#[test]
fn com_neg() {
    let mut a = cpu(&[
        one(0x9400, 16),
        one(0x9401, 17),
        one(0x9401, 18),
        one(0x9401, 19),
    ]);
    a.set_reg(16, 0xFF);
    a.set_reg(17, 0x80);
    a.set_reg(18, 0x00);
    a.set_reg(19, 0x01);
    step(&mut a);
    assert_eq!(a.reg(16), 0x00);
    assert_eq!(flags(&a, ALL & !H), C | Z);
    step(&mut a);
    assert_eq!(a.reg(17), 0x80);
    assert_eq!(flags(&a, C | V | N | Z), C | V | N);
    step(&mut a);
    assert_eq!(a.reg(18), 0x00);
    assert_eq!(flags(&a, C | Z), Z);
    step(&mut a);
    assert_eq!(a.reg(19), 0xFF);
    assert_eq!(flags(&a, C | H | N), C | H | N);
}

#[test]
fn inc_dec_keep_carry() {
    let mut a = cpu(&[bset(0), one(0x9403, 16), one(0x940A, 17), one(0x940A, 18)]);
    a.set_reg(16, 0x7F);
    a.set_reg(17, 0x80);
    a.set_reg(18, 0x01);
    step(&mut a);
    step(&mut a);
    assert_eq!(a.reg(16), 0x80);
    assert_eq!(flags(&a, ALL), C | V | N);
    step(&mut a);
    assert_eq!(a.reg(17), 0x7F);
    assert_eq!(flags(&a, ALL), C | V | S);
    step(&mut a);
    assert_eq!(a.reg(18), 0x00);
    assert_eq!(flags(&a, ALL), C | Z);
}

#[test]
fn shifts_and_swap() {
    let mut a = cpu(&[
        one(0x9405, 16),
        one(0x9406, 17),
        bset(0),
        one(0x9407, 18),
        one(0x9402, 19),
    ]);
    a.set_reg(16, 0x81);
    a.set_reg(17, 0x01);
    a.set_reg(18, 0x02);
    a.set_reg(19, 0xA5);
    step(&mut a);
    assert_eq!(a.reg(16), 0xC0);
    // C=1, N=1 → V = N^C = 0, S = N^V = 1
    assert_eq!(flags(&a, C | N | V | S | Z), C | N | S);
    step(&mut a);
    assert_eq!(a.reg(17), 0x00);
    // C=1, N=0 → V=1, S=1, Z=1
    assert_eq!(flags(&a, C | N | V | S | Z), C | V | S | Z);
    step(&mut a);
    step(&mut a);
    assert_eq!(a.reg(18), 0x81);
    assert_eq!(flags(&a, C | N | V | S), N | V);
    step(&mut a);
    assert_eq!(a.reg(19), 0x5A);
}

#[test]
fn adiw_sbiw_flags_and_cycles() {
    let mut a = cpu(&[adiw(24, 1), adiw(26, 1), sbiw(28, 1), sbiw(30, 1)]);
    a.set_reg(24, 0xFF);
    a.set_reg(25, 0xFF);
    a.set_reg(26, 0xFF);
    a.set_reg(27, 0x7F);
    a.set_reg(28, 0x00);
    a.set_reg(29, 0x00);
    a.set_reg(30, 0x00);
    a.set_reg(31, 0x80);
    assert_eq!(step(&mut a), 2);
    assert_eq!(a.reg_pair(24), 0);
    assert_eq!(flags(&a, C | Z | N | V | S), C | Z);
    assert_eq!(step(&mut a), 2);
    assert_eq!(a.reg_pair(26), 0x8000);
    assert_eq!(flags(&a, C | Z | N | V | S), N | V);
    assert_eq!(step(&mut a), 2);
    assert_eq!(a.reg_pair(28), 0xFFFF);
    assert_eq!(flags(&a, C | Z | N | V | S), C | N | S);
    assert_eq!(step(&mut a), 2);
    assert_eq!(a.reg_pair(30), 0x7FFF);
    assert_eq!(flags(&a, C | Z | N | V | S), V | S);
}

#[test]
fn multiply_family() {
    let mut a = cpu(&[
        mul(16, 17),
        muls(18, 19),
        mulsu_like(0x0300, 20, 21),
        mulsu_like(0x0308, 22, 23),
        mulsu_like(0x0380, 16, 16),
        mulsu_like(0x0388, 16, 17),
    ]);
    a.set_reg(16, 0xFF);
    a.set_reg(17, 0xFF);
    assert_eq!(step(&mut a), 2);
    assert_eq!(a.reg_pair(0), 0xFE01);
    assert_eq!(flags(&a, C | Z), C);
    // MULS -128 * -128 = 16384
    a.set_reg(18, 0x80);
    a.set_reg(19, 0x80);
    assert_eq!(step(&mut a), 2);
    assert_eq!(a.reg_pair(0), 0x4000);
    assert_eq!(flags(&a, C | Z), 0);
    // MULSU -1 * 255 = -255
    a.set_reg(20, 0xFF);
    a.set_reg(21, 0xFF);
    assert_eq!(step(&mut a), 2);
    assert_eq!(a.reg_pair(0), 0xFF01);
    assert_eq!(flags(&a, C | Z), C);
    // FMUL 1.0 * 1.0 (1.7 unsigned: 0x80) → 1.0 in 1.15 (0x8000), C = bit 15 of raw product = 0
    a.set_reg(22, 0x80);
    a.set_reg(23, 0x80);
    assert_eq!(step(&mut a), 2);
    assert_eq!(a.reg_pair(0), 0x8000);
    assert_eq!(flags(&a, C | Z), 0);
    // FMULS -1.0 * -1.0 → 0x8000 (the classic overflow), C = 0
    a.set_reg(16, 0x80);
    assert_eq!(step(&mut a), 2);
    assert_eq!(a.reg_pair(0), 0x8000);
    assert_eq!(flags(&a, C | Z), 0);
    // FMULSU -1.0 * 1.0 → raw -16384 = 0xC000 (C=1), shifted 0x8000
    a.set_reg(16, 0x80);
    a.set_reg(17, 0x80);
    assert_eq!(step(&mut a), 2);
    assert_eq!(a.reg_pair(0), 0x8000);
    assert_eq!(flags(&a, C | Z), C);
    // MUL result zero sets Z
    let mut b = cpu(&[mul(2, 3)]);
    b.set_reg(2, 0);
    b.set_reg(3, 9);
    step(&mut b);
    assert_eq!(flags(&b, Z | C), Z);
}

#[test]
fn movw_mov_ldi() {
    let mut a = cpu(&[movw(2, 24), mov(5, 25), ldi(31, 0xFF)]);
    a.set_reg(24, 0x34);
    a.set_reg(25, 0x12);
    assert_eq!(step(&mut a), 1);
    assert_eq!(a.reg_pair(2), 0x1234);
    step(&mut a);
    assert_eq!(a.reg(5), 0x12);
    step(&mut a);
    assert_eq!(a.reg(31), 0xFF);
}

#[test]
fn bit_ops_and_sreg_instructions() {
    let mut a = cpu(&[bst(16, 3), bld(17, 7), bset(6), bclr(6), bset(1), bclr(1)]);
    a.set_reg(16, 0x08);
    step(&mut a);
    assert_eq!(flags(&a, T), T);
    step(&mut a);
    assert_eq!(a.reg(17), 0x80);
    step(&mut a);
    assert_eq!(flags(&a, T), T);
    step(&mut a);
    assert_eq!(flags(&a, T), 0);
    step(&mut a);
    assert_eq!(flags(&a, Z), Z);
    step(&mut a);
    assert_eq!(flags(&a, Z), 0);
}

// ------------------------------------------------------------------ control flow

#[test]
fn branch_cycles() {
    // brne +1 taken (Z=0) → 2 cycles; breq not taken → 1
    let mut a = cpu(&[brbc(1, 1), NOP, brbs(1, 5), NOP]);
    assert_eq!(step(&mut a), 2);
    assert_eq!(a.pc(), 4);
    assert_eq!(step(&mut a), 1);
    assert_eq!(a.pc(), 6);
    // backwards branch
    let mut b = cpu(&[NOP, brbc(0, -2)]);
    step(&mut b);
    assert_eq!(step(&mut b), 2);
    assert_eq!(b.pc(), 0);
}

#[test]
fn jumps_calls_returns_328p() {
    let j = jmp(0x40);
    let c = call(0x50);
    let mut prog = vec![rjmp(2), NOP, NOP, j[0], j[1]];
    prog.resize(0x40, NOP);
    prog.push(c[0]);
    prog.push(c[1]);
    prog.resize(0x50, NOP);
    prog.push(rcall(1));
    prog.push(RET);
    prog.push(RET);
    let mut a = cpu(&prog);
    assert_eq!(step(&mut a), 2, "rjmp");
    assert_eq!(a.pc(), 6);
    assert_eq!(step(&mut a), 3, "jmp");
    assert_eq!(a.pc(), 0x80);
    let sp0 = a.sp();
    assert_eq!(step(&mut a), 4, "call");
    assert_eq!(a.pc(), 0xA0);
    assert_eq!(a.sp(), sp0 - 2);
    // Return address is stored big-endian (high byte at the lower address).
    assert_eq!(a.peek(sp0), 0x42);
    assert_eq!(a.peek(sp0 - 1), 0x00);
    assert_eq!(a.call_stack().len(), 1);
    assert_eq!(step(&mut a), 3, "rcall");
    assert_eq!(a.pc(), 0xA4);
    assert_eq!(step(&mut a), 4, "ret");
    assert_eq!(a.pc(), 0xA2);
    assert_eq!(step(&mut a), 4, "ret");
    assert_eq!(a.pc(), 0x84);
    assert_eq!(a.sp(), sp0);
    assert!(a.call_stack().is_empty());
}

#[test]
fn indirect_jumps_and_calls() {
    let mut prog = vec![ICALL, IJMP];
    prog.resize(0x20, NOP);
    prog.push(RET);
    let mut a = cpu(&prog);
    a.set_reg(30, 0x20);
    a.set_reg(31, 0);
    assert_eq!(step(&mut a), 3, "icall");
    assert_eq!(a.pc(), 0x40);
    assert_eq!(step(&mut a), 4);
    assert_eq!(a.pc(), 2);
    assert_eq!(step(&mut a), 2, "ijmp");
    assert_eq!(a.pc(), 0x40);
}

#[test]
fn mega2560_three_byte_pc() {
    // CALL to 0x10000 words (byte 0x20000), EICALL/EIJMP via EIND, RET with 3-byte PC.
    let c = call(0x1_0000);
    let mut a = cpu_for(Mcu::Atmega2560, &[c[0], c[1], EICALL, EIJMP, NOP]);
    let mut far = vec![NOP; 0x1_0000 + 4];
    far[0] = c[0];
    far[1] = c[1];
    far[2] = EICALL;
    far[3] = EIJMP;
    far[0x1_0000] = RET;
    far[0x1_0001] = RET;
    a.load_program_words(&far);
    let sp0 = a.sp();
    assert_eq!(step(&mut a), 5, "call on 2560");
    assert_eq!(a.pc(), 0x2_0000);
    assert_eq!(a.sp(), sp0 - 3);
    assert_eq!(a.peek(sp0 - 2), 0x00, "PC bits 21:16");
    assert_eq!(a.peek(sp0 - 1), 0x00);
    assert_eq!(a.peek(sp0), 0x02);
    assert_eq!(step(&mut a), 5, "ret on 2560");
    assert_eq!(a.pc(), 4);
    a.poke(0x5C, 0x01); // EIND
    a.set_reg(30, 0x01);
    a.set_reg(31, 0x00);
    assert_eq!(step(&mut a), 4, "eicall");
    assert_eq!(a.pc(), 0x2_0002);
    step(&mut a);
    assert_eq!(a.pc(), 6);
    assert_eq!(step(&mut a), 2, "eijmp");
    assert_eq!(a.pc(), 0x2_0002);
}

#[test]
fn rcall_icall_cycles_on_2560() {
    let mut prog = vec![rcall(1), NOP, RET, ICALL];
    prog.resize(0x10, NOP);
    let mut a = cpu_for(Mcu::Atmega2560, &prog);
    assert_eq!(step(&mut a), 4, "rcall 22-bit");
    assert_eq!(step(&mut a), 5, "ret 22-bit");
    a.set_reg(30, 2);
    a.set_pc(6);
    assert_eq!(step(&mut a), 4, "icall 22-bit");
}

#[test]
fn skips_over_one_and_two_word_instructions() {
    let l = lds(0, 0x100);
    // CPSE equal, next is 2-word → 3 cycles
    let mut a = cpu(&[cpse(1, 2), l[0], l[1], NOP]);
    assert_eq!(step(&mut a), 3);
    assert_eq!(a.pc(), 6);
    // CPSE equal, next 1-word → 2 cycles
    let mut a = cpu(&[cpse(1, 2), NOP, NOP]);
    assert_eq!(step(&mut a), 2);
    assert_eq!(a.pc(), 4);
    // not equal → 1 cycle
    let mut a = cpu(&[cpse(1, 2), NOP]);
    a.set_reg(1, 5);
    assert_eq!(step(&mut a), 1);
    assert_eq!(a.pc(), 2);
    // SBRS skipping a JMP / SBRC skipping a CALL / STS
    let j = jmp(0x100);
    let mut a = cpu(&[sbrs(16, 0), j[0], j[1], NOP]);
    a.set_reg(16, 1);
    assert_eq!(step(&mut a), 3);
    assert_eq!(a.pc(), 6);
    let c = call(0x100);
    let mut a = cpu(&[sbrc(16, 1), c[0], c[1]]);
    assert_eq!(step(&mut a), 3);
    let s = sts(0x100, 0);
    let mut a = cpu(&[sbrc(16, 1), s[0], s[1]]);
    assert_eq!(step(&mut a), 3);
    // SBIC / SBIS on GPIOR0 (I/O 0x1E)
    let mut a = cpu(&[sbis(0x1E, 3), l[0], l[1], sbic(0x1E, 3), NOP, NOP]);
    a.poke(0x3E, 0x08);
    assert_eq!(step(&mut a), 3);
    assert_eq!(a.pc(), 6);
    assert_eq!(step(&mut a), 1);
    assert_eq!(a.pc(), 8);
}

// ------------------------------------------------------------------ memory

#[test]
fn loads_and_stores_all_modes() {
    let s1 = sts(0x0123, 16);
    let l1 = lds(17, 0x0123);
    let mut a = cpu(&[
        s1[0],
        s1[1],
        l1[0],
        l1[1],
        0x920D | rd(18), // st X+, r18
        0x920E | rd(19), // st -X, r19
        0x900C | rd(20), // ld r20, X
        std_y(5, 21),
        ldd_y(22, 5),
        0x9209 | rd(23), // st Y+, r23
        0x900A | rd(24), // ld r24, -Y
        std_z(63, 25),
        ldd_z(26, 63),
        0x9201 | rd(16), // st Z+, r16
        0x9002 | rd(27), // ld r27, -Z  (clobbers X high; fine)
    ]);
    a.set_reg(16, 0xAB);
    a.set_reg(18, 0x11);
    a.set_reg(19, 0x22);
    a.set_reg(21, 0x33);
    a.set_reg(23, 0x44);
    a.set_reg(25, 0x55);
    a.set_reg(26, 0x00);
    a.set_reg(27, 0x02); // X = 0x200
    a.set_reg(28, 0x00);
    a.set_reg(29, 0x03); // Y = 0x300
    a.set_reg(30, 0x00);
    a.set_reg(31, 0x04); // Z = 0x400
    assert_eq!(step(&mut a), 2, "sts");
    assert_eq!(a.peek(0x123), 0xAB);
    assert_eq!(step(&mut a), 2, "lds");
    assert_eq!(a.reg(17), 0xAB);
    assert_eq!(step(&mut a), 2, "st X+");
    assert_eq!(a.peek(0x200), 0x11);
    assert_eq!(a.reg_pair(26), 0x201);
    assert_eq!(step(&mut a), 2, "st -X");
    assert_eq!(a.reg_pair(26), 0x200);
    assert_eq!(a.peek(0x200), 0x22);
    assert_eq!(step(&mut a), 2, "ld X");
    assert_eq!(a.reg(20), 0x22);
    assert_eq!(step(&mut a), 2, "std Y+5");
    assert_eq!(a.peek(0x305), 0x33);
    assert_eq!(step(&mut a), 2, "ldd Y+5");
    assert_eq!(a.reg(22), 0x33);
    assert_eq!(step(&mut a), 2, "st Y+");
    assert_eq!(a.reg_pair(28), 0x301);
    assert_eq!(step(&mut a), 2, "ld -Y");
    assert_eq!(a.reg(24), 0x44);
    assert_eq!(a.reg_pair(28), 0x300);
    assert_eq!(step(&mut a), 2, "std Z+63");
    assert_eq!(a.peek(0x43F), 0x55);
    assert_eq!(step(&mut a), 2, "ldd Z+63");
    assert_eq!(a.reg(26), 0x55);
    assert_eq!(step(&mut a), 2, "st Z+");
    assert_eq!(a.peek(0x400), 0xAB);
    assert_eq!(step(&mut a), 2, "ld -Z");
    assert_eq!(a.reg(27), 0xAB);
}

#[test]
fn registers_are_memory_mapped() {
    let l = lds(16, 0x0005);
    let s = sts(0x0007, 17);
    let mut a = cpu(&[l[0], l[1], s[0], s[1]]);
    a.set_reg(5, 0x77);
    a.set_reg(17, 0x99);
    step(&mut a);
    assert_eq!(a.reg(16), 0x77);
    step(&mut a);
    assert_eq!(a.reg(7), 0x99);
}

#[test]
fn push_pop_and_stack_pointer_io() {
    let mut a = cpu(&[
        push(16),
        push(17),
        pop(18),
        pop(19),
        in_(20, 0x3D),
        in_(21, 0x3E),
        out(0x3D, 22),
    ]);
    a.set_reg(16, 0x12);
    a.set_reg(17, 0x34);
    a.set_reg(22, 0x80);
    assert_eq!(a.sp(), 0x08FF);
    assert_eq!(step(&mut a), 2);
    assert_eq!(a.peek(0x08FF), 0x12);
    assert_eq!(a.sp(), 0x08FE);
    step(&mut a);
    assert_eq!(step(&mut a), 2);
    assert_eq!(a.reg(18), 0x34);
    step(&mut a);
    assert_eq!(a.reg(19), 0x12);
    assert_eq!(a.sp(), 0x08FF);
    assert_eq!(step(&mut a), 1, "in");
    assert_eq!(a.reg(20), 0xFF);
    step(&mut a);
    assert_eq!(a.reg(21), 0x08);
    assert_eq!(step(&mut a), 1, "out");
    assert_eq!(a.sp(), 0x0880);
}

#[test]
fn lpm_variants() {
    let mut prog = vec![LPM0, 0x9004 | rd(16), 0x9005 | rd(17), 0x9005 | rd(18)];
    prog.resize(0x40, 0);
    prog[0x20] = 0xBBAA;
    prog[0x21] = 0xDDCC;
    let mut a = cpu(&prog);
    a.set_reg(30, 0x40);
    a.set_reg(31, 0x00);
    assert_eq!(step(&mut a), 3);
    assert_eq!(a.reg(0), 0xAA);
    assert_eq!(step(&mut a), 3);
    assert_eq!(a.reg(16), 0xAA);
    assert_eq!(step(&mut a), 3);
    assert_eq!(a.reg(17), 0xAA);
    assert_eq!(a.reg_pair(30), 0x41);
    step(&mut a);
    assert_eq!(a.reg(18), 0xBB);
}

#[test]
fn elpm_uses_rampz_on_2560() {
    let mut prog = vec![NOP; 0x1_0010];
    prog[0] = ELPM0;
    prog[1] = 0x9006 | rd(16);
    prog[2] = 0x9007 | rd(17);
    prog[3] = 0x9007 | rd(18);
    prog[0x8000] = 0x2211; // byte 0x10000
    prog[0x7FFF] = 0xEEFF;
    let mut a = cpu_for(Mcu::Atmega2560, &prog);
    a.poke(0x5B, 1); // RAMPZ
    a.set_reg(30, 0);
    a.set_reg(31, 0);
    assert_eq!(step(&mut a), 3);
    assert_eq!(a.reg(0), 0x11);
    step(&mut a);
    assert_eq!(a.reg(16), 0x11);
    // ELPM Z+ carries from Z into RAMPZ: start at 0x0FFFF
    a.poke(0x5B, 0);
    a.set_reg(30, 0xFF);
    a.set_reg(31, 0xFF);
    step(&mut a);
    assert_eq!(a.reg(17), 0xEE);
    assert_eq!(a.rampz(), 1);
    assert_eq!(a.reg_pair(30), 0);
    step(&mut a);
    assert_eq!(a.reg(18), 0x11);
    // ELPM is undefined on the 328P.
    let mut b = cpu(&[ELPM0]);
    let r = b.step_instruction(&mut ());
    assert_eq!(
        r,
        StopReason::Fault {
            kind: FaultKind::InvalidOpcode(ELPM0),
            pc: 0
        }
    );
}

#[test]
fn spm_buffer_erase_write_and_signature() {
    // Program at 0x7000 (boot section): fill a word, erase page 1 (0x80), write it, read back.
    let base = 0x7000 / 2;
    let mut prog = vec![NOP; base + 32];
    let spmcsr = 0x37; // I/O address of SPMCSR
    let code = [
        ldi(20, 0x01), // SPMEN
        out(spmcsr, 20),
        SPM,           // fill buffer word at Z (offset 2) with r1:r0
        ldi(20, 0x03), // PGERS|SPMEN
        out(spmcsr, 20),
        SPM,
        ldi(20, 0x05), // PGWRT|SPMEN
        out(spmcsr, 20),
        SPM,
        ldi(20, 0x11), // RWWSRE|SPMEN
        out(spmcsr, 20),
        SPM,
        ldi(20, 0x21), // SIGRD|SPMEN
        out(spmcsr, 20),
        0x9004 | rd(21), // lpm r21, Z (Z=0x82 → page offset; signature uses Z&0x1F = 2)
    ];
    prog[base..base + code.len()].copy_from_slice(&code);
    prog[0x40] = 0x1234; // byte 0x80: the page we will erase
    prog[0x42] = 0x5678;
    let mut a = cpu(&prog);
    a.set_pc(0x7000);
    a.set_reg(0, 0xCD);
    a.set_reg(1, 0xAB);
    a.set_reg(30, 0x82);
    a.set_reg(31, 0x00);
    for _ in 0..3 {
        step(&mut a);
    }
    for _ in 0..3 {
        step(&mut a);
    }
    // Erase takes ~4 ms with the CPU halted.
    let c0 = a.cycle();
    a.run(c0 + 64_000, &mut ());
    assert_eq!(a.flash_word(0x80), 0xFFFF);
    assert_eq!(a.flash_word(0x84), 0xFFFF);
    for _ in 0..3 {
        step(&mut a);
    }
    let c1 = a.cycle();
    a.run(c1 + 64_000, &mut ());
    assert_eq!(a.flash_word(0x82), 0xABCD);
    assert_eq!(a.flash_word(0x80), 0xFFFF);
    assert_eq!(a.flash_word(0x84), 0xFFFF);
    // The new word is executable/decoded.
    assert_eq!(
        a.instruction_at(0x82),
        bb_avr::decode(0xABCD, 0xFFFF, false)
    );
    for _ in 0..3 {
        step(&mut a);
    }
    for _ in 0..3 {
        step(&mut a);
    }
    assert_eq!(a.reg(21), 0x95, "signature byte 1 of ATmega328P");
}

#[test]
fn io_bit_instructions() {
    // SBI/CBI 2 cycles; SBI on PINB toggles PORTB; SBI on TIFR0 clears only that flag.
    let mut a = cpu(&[
        sbi(0x04, 5),
        sbi(0x05, 5),
        sbi(0x03, 5),
        cbi(0x04, 5),
        out(0x15, 16),
        sbi(0x15, 0),
    ]);
    assert_eq!(step(&mut a), 2);
    assert_eq!(a.peek(0x24), 0x20);
    step(&mut a);
    assert_eq!(a.peek(0x25), 0x20);
    step(&mut a);
    assert_eq!(a.peek(0x25), 0x00, "PIN write toggles PORT");
    assert_eq!(step(&mut a), 2);
    assert_eq!(a.peek(0x24), 0x00);
}

// ------------------------------------------------------------------ system instructions

#[test]
fn sei_delays_interrupt_by_one_instruction_and_latency_is_4() {
    // Timer0 overflow pending with I=0: flag set and TOIE0 enabled; vector 16 at 0x20.
    let mut prog = vec![NOP; 0x40];
    prog[0] = SEI;
    prog[1] = ldi(16, 0x42);
    prog[2] = NOP;
    prog[0x20] = RETI;
    let mut a = cpu(&prog);
    a.poke(0x6E, 0x01); // TIMSK0.TOIE0
    a.poke(0x35, 0x00);
    // Set TOV0 by writing TCNT0 = 0xFF and running the timer at /1 for a couple of cycles.
    a.poke(0x46, 0xFF);
    a.poke(0x45, 0x01);
    let c0 = a.cycle();
    assert_eq!(step(&mut a), 1, "sei");
    // The instruction after SEI always executes.
    assert_eq!(step(&mut a), 1, "ldi after sei");
    assert_eq!(a.reg(16), 0x42);
    // Next step is the interrupt entry: 4 cycles, PC → vector 16 (byte 0x40), I cleared.
    let c = step(&mut a);
    assert_eq!(c, 4);
    assert_eq!(a.pc(), 0x40);
    assert_eq!(a.sreg() & I, 0);
    assert_eq!(a.peek(0x35) & 1, 0, "TOV0 cleared by vector execution");
    // RETI: 4 cycles, I set, returns.
    assert_eq!(step(&mut a), 4);
    assert_eq!(a.pc(), 4);
    assert_eq!(a.sreg() & I, I);
    let _ = c0;
}

#[test]
fn interrupt_latency_is_5_on_2560() {
    let mut prog = vec![NOP; 0x80];
    prog[0] = SEI;
    prog[1] = NOP;
    let mut a = cpu_for(Mcu::Atmega2560, &prog);
    a.poke(0x6E, 0x01);
    a.poke(0x46, 0xFF);
    a.poke(0x45, 0x01);
    step(&mut a);
    step(&mut a);
    assert_eq!(step(&mut a), 5);
    assert_eq!(a.pc(), 23 * 4, "TIMER0_OVF is vector 23 on the 2560");
}

#[test]
fn cli_blocks_and_out_sreg_enables_immediately() {
    let mut prog = vec![NOP; 0x40];
    prog[0] = CLI;
    prog[1] = NOP;
    prog[2] = out(0x3F, 16); // SREG = 0x80
    prog[3] = NOP;
    let mut a = cpu(&prog);
    a.poke(0x6E, 0x01);
    a.poke(0x46, 0xFF);
    a.poke(0x45, 0x01);
    a.set_reg(16, 0x80);
    step(&mut a);
    step(&mut a);
    assert_eq!(a.pc(), 4, "no interrupt while I=0");
    step(&mut a);
    // Writing I through SREG has no one-instruction delay.
    assert_eq!(step(&mut a), 4);
    assert_eq!(a.pc(), 0x40);
}

#[test]
fn sleep_wdr_break() {
    let mut prog = vec![NOP; 0x40];
    prog[0] = ldi(16, 0x01); // SMCR: idle + SE
    prog[1] = out(0x33, 16);
    prog[2] = SEI;
    prog[3] = SLEEP;
    prog[4] = BREAK;
    prog[5] = WDR;
    prog[0x20] = RETI;
    let mut a = cpu(&prog);
    a.poke(0x6E, 0x01); // TOIE0
    a.poke(0x45, 0x01); // timer0 /1 → overflow every 256 cycles
    step(&mut a);
    step(&mut a);
    step(&mut a);
    assert_eq!(step(&mut a), 1, "sleep");
    assert_eq!(a.sleep_mode(), Some(bb_avr::SleepMode::Idle));
    // Wake on overflow: idle has no start-up delay, +4 wake cycles +4 response, ISR, RETI,
    // then BREAK stops.
    let r = a.run(10_000, &mut ());
    assert_eq!(r, StopReason::Break { pc: 8 });
    assert!(a.cycle() > 256, "slept until the overflow");
    assert_eq!(a.sleep_mode(), None);
    // WDR restarts the watchdog counter.
    let before = a.cycle();
    assert_eq!(step(&mut a), 1);
    assert!(a.cycle() > before);
}

#[test]
fn invalid_opcode_faults() {
    let mut a = cpu(&[0xFFFF]);
    let r = a.run(100, &mut ());
    assert_eq!(
        r,
        StopReason::Fault {
            kind: FaultKind::InvalidOpcode(0xFFFF),
            pc: 0
        }
    );
}

#[test]
fn stack_overflow_faults() {
    let mut a = cpu(&[push(0), rjmp(-2)]);
    let r = a.run(100_000, &mut ());
    assert!(
        matches!(
            r,
            StopReason::Fault {
                kind: FaultKind::StackOverflow { .. },
                ..
            }
        ),
        "{r:?}"
    );
    assert!((a.sp() as usize) < 0x100);
}

#[test]
fn breakpoints_stop_before_execution_and_resume() {
    let mut a = cpu(&[NOP, NOP, ldi(16, 1), rjmp(-4)]);
    a.set_breakpoint(4);
    let r = a.run(1_000, &mut ());
    assert_eq!(r, StopReason::Breakpoint { pc: 4 });
    assert_eq!(a.reg(16), 0);
    let r = a.run(1_000, &mut ());
    assert_eq!(r, StopReason::Breakpoint { pc: 4 });
    assert_eq!(a.reg(16), 1, "resumed past the breakpoint once");
    a.clear_breakpoint(4);
    assert_eq!(a.run(1_000, &mut ()), StopReason::ReachedCycle);
}

#[test]
fn disassembly() {
    let a = cpu(&[ldi(16, 0x10), out(0x05, 16), rjmp(-1)]);
    assert_eq!(a.instruction_at(0).to_string(), "ldi r16, 0x10");
    assert_eq!(a.instruction_at(2).to_string(), "out 0x05, r16");
    assert_eq!(a.instruction_at(4).to_string(), "rjmp .-2");
}

#[test]
fn step_instruction_on_a_sleeping_core_is_bounded() {
    // Power-down with interrupts disabled: nothing can wake it.
    let mut a = cpu(&[ldi(16, 0x05), out(0x33, 16), SLEEP, NOP]);
    for _ in 0..3 {
        step(&mut a);
    }
    assert_eq!(a.sleep_mode(), Some(bb_avr::SleepMode::PowerDown));
    let c0 = a.cycle();
    assert_eq!(a.step_instruction(&mut ()), StopReason::StepDone);
    assert!(a.cycle() <= c0 + 16_000_000, "advanced at most one second");
    assert_eq!(a.sleep_mode(), Some(bb_avr::SleepMode::PowerDown));
}
