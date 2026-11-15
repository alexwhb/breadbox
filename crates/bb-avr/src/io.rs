//! I/O register map: which peripheral owns each address, dispatch of reads/writes, and the
//! register names shown in the debugger's I/O view.

use crate::avr::Avr;
use crate::variant::{PORT_LETTERS, Variant, port_base, reg};
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TReg {
    A,
    B,
    C,
    CntL,
    CntH,
    IcrL,
    IcrH,
    OcrL(u8),
    OcrH(u8),
    Cnt8,
    Ocr8(u8),
    Tifr,
    Timsk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UReg {
    A,
    B,
    C,
    BrrL,
    BrrH,
    Udr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IoReg {
    Plain,
    Unmapped,
    Pin(u8),
    Ddr(u8),
    Port(u8),
    Sreg,
    Spl,
    Sph,
    Rampz,
    Eind,
    Timer(u8, TReg),
    Gtccr,
    Assr,
    Usart(u8, UReg),
    Adcl,
    Adch,
    Adcsra,
    Adcsrb,
    Admux,
    Didr(u8),
    Acsr,
    Spcr,
    Spsr,
    Spdr,
    Twbr,
    Twsr,
    Twar,
    Twdr,
    Twcr,
    Twamr,
    Eecr,
    Eedr,
    Eearl,
    Eearh,
    Wdtcsr,
    Mcusr,
    Mcucr,
    Smcr,
    Clkpr,
    Prr(u8),
    Spmcsr,
    Eicra,
    Eicrb,
    Eimsk,
    Eifr,
    Pcicr,
    Pcifr,
    Pcmsk(u8),
}

pub(crate) fn build_io_map(v: &Variant) -> Vec<IoReg> {
    let mut m = vec![IoReg::Unmapped; v.sram_start];
    let mut set = |a: u16, r: IoReg| {
        if (a as usize) < m.len() {
            m[a as usize] = r;
        }
    };
    for &p in v.ports {
        let b = port_base(p);
        set(b, IoReg::Pin(p));
        set(b + 1, IoReg::Ddr(p));
        set(b + 2, IoReg::Port(p));
    }
    for t in v.timers {
        let n = t.n;
        set(t.tifr, IoReg::Timer(n, TReg::Tifr));
        set(t.timsk, IoReg::Timer(n, TReg::Timsk));
        set(t.base, IoReg::Timer(n, TReg::A));
        set(t.base + 1, IoReg::Timer(n, TReg::B));
        if t.wide {
            set(t.base + 2, IoReg::Timer(n, TReg::C));
            set(t.base + 4, IoReg::Timer(n, TReg::CntL));
            set(t.base + 5, IoReg::Timer(n, TReg::CntH));
            set(t.base + 6, IoReg::Timer(n, TReg::IcrL));
            set(t.base + 7, IoReg::Timer(n, TReg::IcrH));
            for ch in 0..t.channels as u8 {
                set(t.base + 8 + 2 * ch as u16, IoReg::Timer(n, TReg::OcrL(ch)));
                set(t.base + 9 + 2 * ch as u16, IoReg::Timer(n, TReg::OcrH(ch)));
            }
        } else {
            set(t.base + 2, IoReg::Timer(n, TReg::Cnt8));
            set(t.base + 3, IoReg::Timer(n, TReg::Ocr8(0)));
            set(t.base + 4, IoReg::Timer(n, TReg::Ocr8(1)));
        }
    }
    for u in v.usarts {
        let n = u.n;
        set(u.base, IoReg::Usart(n, UReg::A));
        set(u.base + 1, IoReg::Usart(n, UReg::B));
        set(u.base + 2, IoReg::Usart(n, UReg::C));
        set(u.base + 4, IoReg::Usart(n, UReg::BrrL));
        set(u.base + 5, IoReg::Usart(n, UReg::BrrH));
        set(u.base + 6, IoReg::Usart(n, UReg::Udr));
    }
    use reg::*;
    set(PCIFR, IoReg::Pcifr);
    set(EIFR, IoReg::Eifr);
    set(EIMSK, IoReg::Eimsk);
    set(GPIOR0, IoReg::Plain);
    set(EECR, IoReg::Eecr);
    set(EEDR, IoReg::Eedr);
    set(EEARL, IoReg::Eearl);
    set(EEARH, IoReg::Eearh);
    set(GTCCR, IoReg::Gtccr);
    set(GPIOR1, IoReg::Plain);
    set(GPIOR2, IoReg::Plain);
    set(SPCR, IoReg::Spcr);
    set(SPSR, IoReg::Spsr);
    set(SPDR, IoReg::Spdr);
    set(ACSR, IoReg::Acsr);
    set(SMCR, IoReg::Smcr);
    set(MCUSR, IoReg::Mcusr);
    set(MCUCR, IoReg::Mcucr);
    set(SPMCSR, IoReg::Spmcsr);
    if v.ext {
        set(RAMPZ, IoReg::Rampz);
        set(EIND, IoReg::Eind);
        set(OCDR, IoReg::Plain);
        set(PRR1, IoReg::Prr(1));
        set(EICRB, IoReg::Eicrb);
        set(DIDR2, IoReg::Didr(2));
        set(0x74, IoReg::Plain); // XMCRA
        set(0x75, IoReg::Plain); // XMCRB
    }
    set(SPL, IoReg::Spl);
    set(SPH, IoReg::Sph);
    set(SREG, IoReg::Sreg);
    set(WDTCSR, IoReg::Wdtcsr);
    set(CLKPR, IoReg::Clkpr);
    set(PRR0, IoReg::Prr(0));
    set(OSCCAL, IoReg::Plain);
    set(PCICR, IoReg::Pcicr);
    set(EICRA, IoReg::Eicra);
    set(PCMSK0, IoReg::Pcmsk(0));
    set(PCMSK1, IoReg::Pcmsk(1));
    set(PCMSK2, IoReg::Pcmsk(2));
    set(ADCL, IoReg::Adcl);
    set(ADCH, IoReg::Adch);
    set(ADCSRA, IoReg::Adcsra);
    set(ADCSRB, IoReg::Adcsrb);
    set(ADMUX, IoReg::Admux);
    set(DIDR0, IoReg::Didr(0));
    set(DIDR1, IoReg::Didr(1));
    set(ASSR, IoReg::Assr);
    set(TWBR, IoReg::Twbr);
    set(TWSR, IoReg::Twsr);
    set(TWAR, IoReg::Twar);
    set(TWDR, IoReg::Twdr);
    set(TWCR, IoReg::Twcr);
    set(TWAMR, IoReg::Twamr);
    m
}

fn reg_name(v: &Variant, a: u16, r: IoReg) -> Option<String> {
    let l = |p: u8| PORT_LETTERS[p as usize] as char;
    let t8 = |n: u8| {
        v.timers
            .iter()
            .find(|t| t.n == n)
            .map(|t| t.wide)
            .unwrap_or(false)
    };
    let prr_name = |i: u8| {
        if v.ext {
            format!("PRR{i}")
        } else {
            "PRR".to_string()
        }
    };
    let usart_suffix = |n: u8| format!("{n}");
    Some(match r {
        IoReg::Unmapped => return None,
        IoReg::Plain => match a {
            reg::GPIOR0 => "GPIOR0".into(),
            reg::GPIOR1 => "GPIOR1".into(),
            reg::GPIOR2 => "GPIOR2".into(),
            reg::OSCCAL => "OSCCAL".into(),
            reg::OCDR => "OCDR".into(),
            0x74 => "XMCRA".into(),
            0x75 => "XMCRB".into(),
            _ => format!("IO_{a:02X}"),
        },
        IoReg::Pin(p) => format!("PIN{}", l(p)),
        IoReg::Ddr(p) => format!("DDR{}", l(p)),
        IoReg::Port(p) => format!("PORT{}", l(p)),
        IoReg::Sreg => "SREG".into(),
        IoReg::Spl => "SPL".into(),
        IoReg::Sph => "SPH".into(),
        IoReg::Rampz => "RAMPZ".into(),
        IoReg::Eind => "EIND".into(),
        IoReg::Timer(n, t) => {
            let _ = t8(n);
            match t {
                TReg::A => format!("TCCR{n}A"),
                TReg::B => format!("TCCR{n}B"),
                TReg::C => format!("TCCR{n}C"),
                TReg::CntL => format!("TCNT{n}L"),
                TReg::CntH => format!("TCNT{n}H"),
                TReg::IcrL => format!("ICR{n}L"),
                TReg::IcrH => format!("ICR{n}H"),
                TReg::OcrL(c) => format!("OCR{n}{}L", (b'A' + c) as char),
                TReg::OcrH(c) => format!("OCR{n}{}H", (b'A' + c) as char),
                TReg::Cnt8 => format!("TCNT{n}"),
                TReg::Ocr8(c) => format!("OCR{n}{}", (b'A' + c) as char),
                TReg::Tifr => format!("TIFR{n}"),
                TReg::Timsk => format!("TIMSK{n}"),
            }
        }
        IoReg::Gtccr => "GTCCR".into(),
        IoReg::Assr => "ASSR".into(),
        IoReg::Usart(n, u) => {
            let s = usart_suffix(n);
            match u {
                UReg::A => format!("UCSR{s}A"),
                UReg::B => format!("UCSR{s}B"),
                UReg::C => format!("UCSR{s}C"),
                UReg::BrrL => format!("UBRR{s}L"),
                UReg::BrrH => format!("UBRR{s}H"),
                UReg::Udr => format!("UDR{s}"),
            }
        }
        IoReg::Adcl => "ADCL".into(),
        IoReg::Adch => "ADCH".into(),
        IoReg::Adcsra => "ADCSRA".into(),
        IoReg::Adcsrb => "ADCSRB".into(),
        IoReg::Admux => "ADMUX".into(),
        IoReg::Didr(i) => format!("DIDR{i}"),
        IoReg::Acsr => "ACSR".into(),
        IoReg::Spcr => "SPCR".into(),
        IoReg::Spsr => "SPSR".into(),
        IoReg::Spdr => "SPDR".into(),
        IoReg::Twbr => "TWBR".into(),
        IoReg::Twsr => "TWSR".into(),
        IoReg::Twar => "TWAR".into(),
        IoReg::Twdr => "TWDR".into(),
        IoReg::Twcr => "TWCR".into(),
        IoReg::Twamr => "TWAMR".into(),
        IoReg::Eecr => "EECR".into(),
        IoReg::Eedr => "EEDR".into(),
        IoReg::Eearl => "EEARL".into(),
        IoReg::Eearh => "EEARH".into(),
        IoReg::Wdtcsr => "WDTCSR".into(),
        IoReg::Mcusr => "MCUSR".into(),
        IoReg::Mcucr => "MCUCR".into(),
        IoReg::Smcr => "SMCR".into(),
        IoReg::Clkpr => "CLKPR".into(),
        IoReg::Prr(i) => prr_name(i),
        IoReg::Spmcsr => "SPMCSR".into(),
        IoReg::Eicra => "EICRA".into(),
        IoReg::Eicrb => "EICRB".into(),
        IoReg::Eimsk => "EIMSK".into(),
        IoReg::Eifr => "EIFR".into(),
        IoReg::Pcicr => "PCICR".into(),
        IoReg::Pcifr => "PCIFR".into(),
        IoReg::Pcmsk(i) => format!("PCMSK{i}"),
    })
}

fn names_for(v: &'static Variant) -> Vec<(&'static str, u16)> {
    let map = build_io_map(v);
    map.iter()
        .enumerate()
        .filter_map(|(a, r)| {
            reg_name(v, a as u16, *r).map(|n| (&*Box::leak(n.into_boxed_str()), a as u16))
        })
        .collect()
}

/// A named I/O register of an AVR part, for the debugger's I/O register view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct IoRegister {
    /// Datasheet name: `"PORTB"`, `"TCCR1A"`, `"UDR0"`.
    pub name: &'static str,
    /// Data-space address (`PORTB` = `0x25`; extended I/O such as `PORTH` = `0x102`).
    pub addr: u16,
    /// Peripheral it belongs to: `"Port B"`, `"Timer/Counter1"`, `"USART0"`, `"ADC"`, `"SPI"`,
    /// `"TWI (I2C)"`, `"EEPROM"`, `"External interrupts"`, `"Watchdog"`, `"Analog comparator"`,
    /// `"CPU"`, `"External memory"`.
    pub peripheral: &'static str,
}

const PORT_GROUPS: [&str; 11] = [
    "Port A", "Port B", "Port C", "Port D", "Port E", "Port F", "Port G", "Port H", "Port J",
    "Port K", "Port L",
];
const TIMER_GROUPS: [&str; 6] = [
    "Timer/Counter0",
    "Timer/Counter1",
    "Timer/Counter2",
    "Timer/Counter3",
    "Timer/Counter4",
    "Timer/Counter5",
];
const USART_GROUPS: [&str; 4] = ["USART0", "USART1", "USART2", "USART3"];

fn peripheral_of(a: u16, r: IoReg) -> &'static str {
    match r {
        IoReg::Pin(p) | IoReg::Ddr(p) | IoReg::Port(p) => PORT_GROUPS[p as usize % 11],
        IoReg::Timer(n, _) => TIMER_GROUPS[n as usize % 6],
        IoReg::Assr => "Timer/Counter2",
        IoReg::Gtccr => "Timers",
        IoReg::Usart(n, _) => USART_GROUPS[n as usize % 4],
        IoReg::Adcl
        | IoReg::Adch
        | IoReg::Adcsra
        | IoReg::Adcsrb
        | IoReg::Admux
        | IoReg::Didr(_) => "ADC",
        IoReg::Acsr => "Analog comparator",
        IoReg::Spcr | IoReg::Spsr | IoReg::Spdr => "SPI",
        IoReg::Twbr | IoReg::Twsr | IoReg::Twar | IoReg::Twdr | IoReg::Twcr | IoReg::Twamr => {
            "TWI (I2C)"
        }
        IoReg::Eecr | IoReg::Eedr | IoReg::Eearl | IoReg::Eearh => "EEPROM",
        IoReg::Wdtcsr => "Watchdog",
        IoReg::Eicra
        | IoReg::Eicrb
        | IoReg::Eimsk
        | IoReg::Eifr
        | IoReg::Pcicr
        | IoReg::Pcifr
        | IoReg::Pcmsk(_) => "External interrupts",
        IoReg::Plain if a == 0x74 || a == 0x75 => "External memory",
        IoReg::Plain
        | IoReg::Unmapped
        | IoReg::Sreg
        | IoReg::Spl
        | IoReg::Sph
        | IoReg::Rampz
        | IoReg::Eind
        | IoReg::Mcusr
        | IoReg::Mcucr
        | IoReg::Smcr
        | IoReg::Clkpr
        | IoReg::Prr(_)
        | IoReg::Spmcsr => "CPU",
    }
}

/// Every named I/O register of `mcu` with its data-space address and peripheral, in address
/// order (built once per part).
pub fn io_registers(mcu: crate::Mcu) -> &'static [IoRegister] {
    static M328: OnceLock<Vec<IoRegister>> = OnceLock::new();
    static M2560: OnceLock<Vec<IoRegister>> = OnceLock::new();
    let cell = match mcu {
        crate::Mcu::Atmega328p => &M328,
        crate::Mcu::Atmega2560 => &M2560,
    };
    cell.get_or_init(|| {
        let v = mcu.variant();
        let map = build_io_map(v);
        io_register_names(v)
            .into_iter()
            .map(|(name, addr)| IoRegister {
                name,
                addr,
                peripheral: peripheral_of(
                    addr,
                    map.get(addr as usize).copied().unwrap_or(IoReg::Unmapped),
                ),
            })
            .collect()
    })
    .as_slice()
}

/// `(name, data address)` of every mapped I/O register, in address order.
pub(crate) fn io_register_names(v: &'static Variant) -> Vec<(&'static str, u16)> {
    static M328: OnceLock<Vec<(&'static str, u16)>> = OnceLock::new();
    static M2560: OnceLock<Vec<(&'static str, u16)>> = OnceLock::new();
    let cell = match v.mcu {
        crate::Mcu::Atmega328p => &M328,
        crate::Mcu::Atmega2560 => &M2560,
    };
    cell.get_or_init(|| names_for(v)).clone()
}

impl Avr {
    /// I/O read with side effects (UDR pops, ADCL locks, TCNT latches TEMP, …).
    pub(crate) fn io_read(&mut self, a: usize) -> u8 {
        let r = self.io_map.get(a).copied().unwrap_or(IoReg::Unmapped);
        match r {
            IoReg::Plain => self.data[a],
            IoReg::Unmapped => 0,
            IoReg::Pin(p) => self.pin_reg(p as usize),
            IoReg::Ddr(p) => self.ports[p as usize].ddr,
            IoReg::Port(p) => self.ports[p as usize].port,
            IoReg::Sreg => self.sreg,
            IoReg::Spl => self.sp as u8,
            IoReg::Sph => (self.sp >> 8) as u8,
            IoReg::Rampz => self.rampz,
            IoReg::Eind => self.eind,
            IoReg::Timer(n, t) => self.timer_read(n as usize, t),
            IoReg::Usart(n, u) => self.usart_read(n as usize, u),
            IoReg::Adcl => self.adc_read_l(),
            IoReg::Adch => self.adc_read_h(),
            IoReg::Spdr => self.spi_read_dr(),
            IoReg::Spsr => self.spi_read_sr(),
            IoReg::Twdr | IoReg::Twcr | IoReg::Twsr => self.twi_read(r),
            _ => self.io_peek(a),
        }
    }

    /// I/O value without side effects.
    pub(crate) fn io_peek(&self, a: usize) -> u8 {
        let r = self.io_map.get(a).copied().unwrap_or(IoReg::Unmapped);
        match r {
            IoReg::Plain => self.data[a],
            IoReg::Unmapped => 0,
            IoReg::Pin(p) => self.pin_reg(p as usize),
            IoReg::Ddr(p) => self.ports[p as usize].ddr,
            IoReg::Port(p) => self.ports[p as usize].port,
            IoReg::Sreg => self.sreg,
            IoReg::Spl => self.sp as u8,
            IoReg::Sph => (self.sp >> 8) as u8,
            IoReg::Rampz => self.rampz,
            IoReg::Eind => self.eind,
            IoReg::Timer(n, t) => self.timer_peek(n as usize, t),
            IoReg::Gtccr => self.sys.gtccr,
            IoReg::Assr => self.sys.assr,
            IoReg::Usart(n, u) => self.usart_peek(n as usize, u),
            IoReg::Adcl => self.adc_peek_l(),
            IoReg::Adch => self.adc_peek_h(),
            IoReg::Adcsra => self.adc_peek_sra(),
            IoReg::Adcsrb => self.adc.adcsrb,
            IoReg::Admux => self.adc.admux,
            IoReg::Didr(i) => self.adc.didr[i as usize & 3],
            IoReg::Acsr => self.acomp_peek(),
            IoReg::Spcr => self.spi.spcr,
            IoReg::Spsr => self.spi_peek_sr(),
            IoReg::Spdr => self.spi.rx_buf,
            IoReg::Twbr | IoReg::Twsr | IoReg::Twar | IoReg::Twdr | IoReg::Twcr | IoReg::Twamr => {
                self.twi_peek(r)
            }
            IoReg::Eecr => self.ee_peek_cr(),
            IoReg::Eedr => self.ee.eedr,
            IoReg::Eearl => self.ee.eear as u8,
            IoReg::Eearh => (self.ee.eear >> 8) as u8,
            IoReg::Wdtcsr => self.wdt_peek(),
            IoReg::Mcusr => self.sys.mcusr,
            IoReg::Mcucr => self.sys.mcucr,
            IoReg::Smcr => self.sys.smcr,
            IoReg::Clkpr => self.sys.clkpr,
            IoReg::Prr(i) => self.sys.prr[i as usize & 1],
            IoReg::Spmcsr => self.sys.spmcsr,
            IoReg::Eicra => self.exint.eicr[0],
            IoReg::Eicrb => self.exint.eicr[1],
            IoReg::Eimsk => self.exint.eimsk,
            IoReg::Eifr => self.exint.eifr,
            IoReg::Pcicr => self.exint.pcicr,
            IoReg::Pcifr => self.exint.pcifr,
            IoReg::Pcmsk(i) => self.exint.pcmsk[i as usize & 3],
        }
    }

    pub(crate) fn io_write(&mut self, a: usize, v: u8) {
        let r = self.io_map.get(a).copied().unwrap_or(IoReg::Unmapped);
        let now = self.cycle;
        match r {
            IoReg::Plain => self.data[a] = v,
            IoReg::Unmapped => {}
            IoReg::Pin(p) => {
                // Writing 1 to PINxn toggles PORTxn.
                if v != 0 {
                    let new = self.ports[p as usize].port ^ v;
                    self.gpio_write_port(p as usize, new, now);
                }
            }
            IoReg::Ddr(p) => self.gpio_write_ddr(p as usize, v, now),
            IoReg::Port(p) => self.gpio_write_port(p as usize, v, now),
            IoReg::Sreg => self.write_sreg(v),
            IoReg::Spl => self.sp = (self.sp & 0xFF00) | v as u16,
            IoReg::Sph => self.sp = (self.sp & 0x00FF) | ((v as u16) << 8),
            IoReg::Rampz => self.rampz = v,
            IoReg::Eind => self.eind = v,
            IoReg::Timer(n, t) => self.timer_write(n as usize, t, v),
            IoReg::Gtccr => self.gtccr_write(v),
            IoReg::Assr => self.sys.assr = v & 0x60,
            IoReg::Usart(n, u) => self.usart_write(n as usize, u, v),
            IoReg::Adcl | IoReg::Adch => {}
            IoReg::Adcsra => self.adc_write_sra(v),
            IoReg::Adcsrb => self.adc_write_srb(v),
            IoReg::Admux => self.adc_write_mux(v),
            IoReg::Didr(i) => self.didr_write(i, v),
            IoReg::Acsr => self.acomp_write(v),
            IoReg::Spcr => self.spi_write_cr(v),
            IoReg::Spsr => self.spi_write_sr(v),
            IoReg::Spdr => self.spi_write_dr(v),
            IoReg::Twbr | IoReg::Twsr | IoReg::Twar | IoReg::Twdr | IoReg::Twcr | IoReg::Twamr => {
                self.twi_write(r, v)
            }
            IoReg::Eecr => self.ee_write_cr(v),
            IoReg::Eedr => self.ee.eedr = v,
            IoReg::Eearl => self.ee_write_ar((self.ee.eear & 0xFF00) | v as u16),
            IoReg::Eearh => self.ee_write_ar((self.ee.eear & 0x00FF) | ((v as u16) << 8)),
            IoReg::Wdtcsr => self.wdt_write(v),
            IoReg::Mcusr => self.mcusr_write(v),
            IoReg::Mcucr => self.mcucr_write(v),
            IoReg::Smcr => self.sys.smcr = v & 0x0F,
            IoReg::Clkpr => self.clkpr_write(v),
            IoReg::Prr(i) => self.prr_write(i as usize, v),
            IoReg::Spmcsr => self.spmcsr_write(v),
            IoReg::Eicra => self.eicr_write(0, v),
            IoReg::Eicrb => self.eicr_write(1, v),
            IoReg::Eimsk => self.eimsk_write(v),
            IoReg::Eifr => self.eifr_write(v),
            IoReg::Pcicr => self.pcicr_write(v),
            IoReg::Pcifr => self.pcifr_write(v),
            IoReg::Pcmsk(i) => self.exint.pcmsk[i as usize & 3] = v,
        }
    }

    /// SBI/CBI: bit-only access (flag registers clear only the addressed bit, PINx toggles only
    /// that bit).
    pub(crate) fn io_write_bit(&mut self, a: usize, b: u8, val: bool) {
        let r = self.io_map.get(a).copied().unwrap_or(IoReg::Unmapped);
        let mask = 1u8 << b;
        match r {
            IoReg::Pin(_) => {
                if val {
                    self.io_write(a, mask);
                }
            }
            IoReg::Timer(_, TReg::Tifr) | IoReg::Eifr | IoReg::Pcifr => {
                if val {
                    self.io_write(a, mask);
                }
            }
            _ => {
                let cur = self.io_peek(a);
                let nv = if val { cur | mask } else { cur & !mask };
                // Write-one-to-clear flags inside otherwise normal registers must not be
                // re-written as 1 by the read-modify-write.
                let w1c = match r {
                    IoReg::Adcsra => 0x10,
                    IoReg::Usart(_, UReg::A) => 0x40,
                    IoReg::Acsr => 0x10,
                    IoReg::Wdtcsr => 0x80,
                    IoReg::Twcr => 0x80,
                    _ => 0,
                } & !mask;
                self.io_write(a, nv & !w1c);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{Mcu, io_registers};

    fn find(mcu: Mcu, name: &str) -> (u16, &'static str) {
        let r = io_registers(mcu)
            .iter()
            .find(|r| r.name == name)
            .unwrap_or_else(|| panic!("{name} missing on {mcu:?}"));
        (r.addr, r.peripheral)
    }

    #[test]
    fn atmega328p_register_table() {
        let m = Mcu::Atmega328p;
        assert_eq!(find(m, "PINB"), (0x23, "Port B"));
        assert_eq!(find(m, "DDRB"), (0x24, "Port B"));
        assert_eq!(find(m, "PORTB"), (0x25, "Port B"));
        assert_eq!(find(m, "PORTD"), (0x2B, "Port D"));
        assert_eq!(find(m, "TCCR0A"), (0x44, "Timer/Counter0"));
        assert_eq!(find(m, "TCNT0"), (0x46, "Timer/Counter0"));
        assert_eq!(find(m, "TIMSK0"), (0x6E, "Timer/Counter0"));
        assert_eq!(find(m, "TCCR1A"), (0x80, "Timer/Counter1"));
        assert_eq!(find(m, "OCR1AL"), (0x88, "Timer/Counter1"));
        assert_eq!(find(m, "UCSR0A"), (0xC0, "USART0"));
        assert_eq!(find(m, "UDR0"), (0xC6, "USART0"));
        assert_eq!(find(m, "ADMUX"), (0x7C, "ADC"));
        assert_eq!(find(m, "SPCR"), (0x4C, "SPI"));
        assert_eq!(find(m, "TWCR"), (0xBC, "TWI (I2C)"));
        assert_eq!(find(m, "EECR"), (0x3F, "EEPROM"));
        assert_eq!(find(m, "WDTCSR"), (0x60, "Watchdog"));
        assert_eq!(find(m, "EIMSK"), (0x3D, "External interrupts"));
        assert_eq!(find(m, "SPL"), (0x5D, "CPU"));
        assert_eq!(find(m, "SREG"), (0x5F, "CPU"));
        assert_eq!(find(m, "PRR"), (0x64, "CPU"));
        // no Mega-only registers
        let t = io_registers(m);
        assert!(
            !t.iter()
                .any(|r| r.name == "PORTH" || r.name == "UDR1" || r.name == "EIND")
        );
        // address order, every register inside the I/O space, no duplicates
        assert!(t.windows(2).all(|w| w[0].addr < w[1].addr));
        assert!(t.iter().all(|r| (0x20..0x100).contains(&r.addr)));
        assert!(t.len() > 70, "{} registers", t.len());
    }

    #[test]
    fn atmega2560_register_table() {
        let m = Mcu::Atmega2560;
        assert_eq!(find(m, "PORTA"), (0x22, "Port A"));
        assert_eq!(find(m, "PORTB"), (0x25, "Port B"));
        assert_eq!(find(m, "PORTG"), (0x34, "Port G"));
        assert_eq!(find(m, "PINH"), (0x100, "Port H"));
        assert_eq!(find(m, "PORTH"), (0x102, "Port H"));
        assert_eq!(find(m, "PORTL"), (0x10B, "Port L"));
        assert_eq!(find(m, "TCCR4A"), (0xA0, "Timer/Counter4"));
        assert_eq!(find(m, "TCCR5A"), (0x120, "Timer/Counter5"));
        assert_eq!(find(m, "UDR1"), (0xCE, "USART1"));
        assert_eq!(find(m, "UCSR3A"), (0x130, "USART3"));
        assert_eq!(find(m, "EIND"), (0x5C, "CPU"));
        assert_eq!(find(m, "RAMPZ"), (0x5B, "CPU"));
        assert_eq!(find(m, "PRR1"), (0x65, "CPU"));
        assert_eq!(find(m, "XMCRA"), (0x74, "External memory"));
        let t = io_registers(m);
        assert!(t.windows(2).all(|w| w[0].addr < w[1].addr));
        assert!(t.iter().all(|r| (0x20..0x200).contains(&r.addr)));
        assert!(t.len() > 150, "{} registers", t.len());
    }

    #[test]
    fn table_matches_live_values() {
        let mut avr = crate::Avr::with_mcu(Mcu::Atmega328p);
        avr.poke(0x24, 0x20); // DDRB: PB5 output
        avr.poke(0x25, 0x20); // PORTB: PB5 high
        let live = avr.io_registers();
        assert_eq!(live.len(), io_registers(Mcu::Atmega328p).len());
        let portb = live.iter().find(|r| r.0 == "PORTB").unwrap();
        assert_eq!((portb.1, portb.2), (0x25, 0x20));
    }

    #[test]
    fn flash_generation_changes_with_program_memory() {
        let mut a = crate::Avr::with_mcu(Mcu::Atmega328p);
        let b = crate::Avr::with_mcu(Mcu::Atmega328p);
        let g0 = a.flash_generation();
        assert_ne!(g0, b.flash_generation(), "unique per instance");
        a.load_program_words(&[0x0000, 0xCFFF]);
        assert_ne!(a.flash_generation(), g0);
        let g1 = a.flash_generation();
        a.set_pc(0);
        assert_eq!(a.flash_generation(), g1, "running code does not change it");
    }
}
