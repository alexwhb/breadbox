//! Static per-MCU descriptions: memory sizes, I/O register map, interrupt vectors and the
//! pins each peripheral uses. Everything else in the crate is table-driven from these.

use crate::Pin;

/// Supported AVR parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mcu {
    /// ATmega328P (Arduino UNO R3, Nano): 32 KiB flash, 2 KiB SRAM, 1 KiB EEPROM.
    Atmega328p,
    /// ATmega2560 (Arduino Mega 2560): 256 KiB flash, 8 KiB SRAM, 4 KiB EEPROM, 3-byte PC.
    Atmega2560,
}

impl Mcu {
    /// Datasheet part name, e.g. `"ATmega328P"`.
    pub fn name(self) -> &'static str {
        self.variant().name
    }
    /// The `-mmcu=` name used by avr-gcc.
    pub fn gcc_name(self) -> &'static str {
        match self {
            Mcu::Atmega328p => "atmega328p",
            Mcu::Atmega2560 => "atmega2560",
        }
    }
    /// Parses an avr-gcc `-mmcu` name or datasheet name (case-insensitive).
    pub fn from_name(s: &str) -> Option<Mcu> {
        match s.to_ascii_lowercase().as_str() {
            "atmega328p" | "m328p" => Some(Mcu::Atmega328p),
            "atmega2560" | "m2560" => Some(Mcu::Atmega2560),
            _ => None,
        }
    }
    pub fn flash_size(self) -> usize {
        self.variant().flash_bytes
    }
    pub fn sram_size(self) -> usize {
        self.variant().ramend + 1 - self.variant().sram_start
    }
    pub fn eeprom_size(self) -> usize {
        self.variant().eeprom_bytes
    }
    /// First SRAM data address (0x100 on the 328P, 0x200 on the 2560).
    pub fn sram_start(self) -> usize {
        self.variant().sram_start
    }
    pub fn ramend(self) -> usize {
        self.variant().ramend
    }
    /// Device signature bytes.
    pub fn signature(self) -> [u8; 3] {
        self.variant().signature
    }
    pub(crate) fn variant(self) -> &'static Variant {
        match self {
            Mcu::Atmega328p => &M328P,
            Mcu::Atmega2560 => &M2560,
        }
    }
}

/// An interrupt source. The vector table of each variant lists these in priority order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Irq {
    Reset,
    Int(u8),
    Pcint(u8),
    Wdt,
    TimerCompA(u8),
    TimerCompB(u8),
    TimerCompC(u8),
    TimerOvf(u8),
    TimerCapt(u8),
    Spi,
    UsartRx(u8),
    UsartUdre(u8),
    UsartTx(u8),
    Adc,
    EeReady,
    AnaComp,
    Twi,
    SpmReady,
}

#[derive(Debug)]
pub(crate) struct TimerDesc {
    pub n: u8,
    pub wide: bool,
    /// Address of TCCRnA. 8-bit: A, B, TCNT, OCRA, OCRB follow. 16-bit: A, B, C, -, TCNT L/H,
    /// ICR L/H, OCRA L/H, OCRB L/H, OCRC L/H.
    pub base: u16,
    pub tifr: u16,
    pub timsk: u16,
    pub oc: [Option<Pin>; 3],
    pub icp: Option<Pin>,
    pub tclk: Option<Pin>,
    pub channels: usize,
    /// Timer2: own prescaler table, asynchronous clock option, PSRASY.
    pub async_timer: bool,
    /// (PRR register address, bit)
    pub prr: (u16, u8),
}

#[derive(Debug)]
pub(crate) struct UsartDesc {
    pub n: u8,
    pub base: u16,
    pub rx: Pin,
    pub tx: Pin,
}

#[derive(Debug)]
pub(crate) struct Variant {
    pub mcu: Mcu,
    pub name: &'static str,
    pub signature: [u8; 3],
    pub flash_bytes: usize,
    pub sram_start: usize,
    pub ramend: usize,
    pub eeprom_bytes: usize,
    pub page_bytes: usize,
    /// Byte address where the (largest) boot section starts — NRWW boundary for SPM.
    pub nrww_start: usize,
    /// Three-byte program counter (2560): +1 cycle on calls/returns/interrupts.
    pub pc22: bool,
    /// ELPM / EIJMP / EICALL / RAMPZ / EIND present.
    pub ext: bool,
    pub vectors: &'static [Irq],
    /// Port letters present (index into "ABCDEFGHJKL").
    pub ports: &'static [u8],
    pub timers: &'static [TimerDesc],
    pub usarts: &'static [UsartDesc],
    /// INTn pins in order.
    pub ext_ints: &'static [Pin],
    /// INTn with asynchronous edge detection (can wake from power-down on edges).
    pub ext_int_async_edges: u8,
    /// PCINT groups: [group][bit] → pin.
    pub pcint: [[Option<Pin>; 8]; 3],
    /// ADC channel pins (ADC0..).
    pub adc_pins: &'static [Pin],
    pub spi_ss: Pin,
    pub spi_sck: Pin,
    pub spi_mosi: Pin,
    pub spi_miso: Pin,
    pub twi_sda: Pin,
    pub twi_scl: Pin,
    pub ain0: Pin,
    pub ain1: Pin,
    pub has_temp_sensor: bool,
    /// Fuse bytes as shipped on the Arduino board: (low, high, extended, lock).
    pub fuses: [u8; 4],
}

const fn p(port: u8, bit: u8) -> Pin {
    Pin { port, bit }
}
const A: u8 = 0;
const B: u8 = 1;
const C: u8 = 2;
const D: u8 = 3;
const E: u8 = 4;
const F: u8 = 5;
const G: u8 = 6;
const H: u8 = 7;
const J: u8 = 8;
const K: u8 = 9;
const L: u8 = 10;

/// Data-space address of PINx for a port index; DDRx = +1, PORTx = +2.
pub(crate) const fn port_base(port: u8) -> u16 {
    match port {
        0 => 0x20,
        1 => 0x23,
        2 => 0x26,
        3 => 0x29,
        4 => 0x2C,
        5 => 0x2F,
        6 => 0x32,
        7 => 0x100,
        8 => 0x103,
        9 => 0x106,
        _ => 0x109,
    }
}

pub(crate) const PORT_LETTERS: &[u8; 11] = b"ABCDEFGHJKL";

// Common register addresses (identical on both parts).
pub(crate) mod reg {
    pub const PCIFR: u16 = 0x3B;
    pub const EIFR: u16 = 0x3C;
    pub const EIMSK: u16 = 0x3D;
    pub const GPIOR0: u16 = 0x3E;
    pub const EECR: u16 = 0x3F;
    pub const EEDR: u16 = 0x40;
    pub const EEARL: u16 = 0x41;
    pub const EEARH: u16 = 0x42;
    pub const GTCCR: u16 = 0x43;
    pub const GPIOR1: u16 = 0x4A;
    pub const GPIOR2: u16 = 0x4B;
    pub const SPCR: u16 = 0x4C;
    pub const SPSR: u16 = 0x4D;
    pub const SPDR: u16 = 0x4E;
    pub const ACSR: u16 = 0x50;
    pub const OCDR: u16 = 0x51;
    pub const SMCR: u16 = 0x53;
    pub const MCUSR: u16 = 0x54;
    pub const MCUCR: u16 = 0x55;
    pub const SPMCSR: u16 = 0x57;
    pub const RAMPZ: u16 = 0x5B;
    pub const EIND: u16 = 0x5C;
    pub const SPL: u16 = 0x5D;
    pub const SPH: u16 = 0x5E;
    pub const SREG: u16 = 0x5F;
    pub const WDTCSR: u16 = 0x60;
    pub const CLKPR: u16 = 0x61;
    pub const PRR0: u16 = 0x64;
    pub const PRR1: u16 = 0x65;
    pub const OSCCAL: u16 = 0x66;
    pub const PCICR: u16 = 0x68;
    pub const EICRA: u16 = 0x69;
    pub const EICRB: u16 = 0x6A;
    pub const PCMSK0: u16 = 0x6B;
    pub const PCMSK1: u16 = 0x6C;
    pub const PCMSK2: u16 = 0x6D;
    pub const ADCL: u16 = 0x78;
    pub const ADCH: u16 = 0x79;
    pub const ADCSRA: u16 = 0x7A;
    pub const ADCSRB: u16 = 0x7B;
    pub const ADMUX: u16 = 0x7C;
    pub const DIDR2: u16 = 0x7D;
    pub const DIDR0: u16 = 0x7E;
    pub const DIDR1: u16 = 0x7F;
    pub const ASSR: u16 = 0xB6;
    pub const TWBR: u16 = 0xB8;
    pub const TWSR: u16 = 0xB9;
    pub const TWAR: u16 = 0xBA;
    pub const TWDR: u16 = 0xBB;
    pub const TWCR: u16 = 0xBC;
    pub const TWAMR: u16 = 0xBD;
}

static M328P_VECTORS: [Irq; 26] = [
    Irq::Reset,
    Irq::Int(0),
    Irq::Int(1),
    Irq::Pcint(0),
    Irq::Pcint(1),
    Irq::Pcint(2),
    Irq::Wdt,
    Irq::TimerCompA(2),
    Irq::TimerCompB(2),
    Irq::TimerOvf(2),
    Irq::TimerCapt(1),
    Irq::TimerCompA(1),
    Irq::TimerCompB(1),
    Irq::TimerOvf(1),
    Irq::TimerCompA(0),
    Irq::TimerCompB(0),
    Irq::TimerOvf(0),
    Irq::Spi,
    Irq::UsartRx(0),
    Irq::UsartUdre(0),
    Irq::UsartTx(0),
    Irq::Adc,
    Irq::EeReady,
    Irq::AnaComp,
    Irq::Twi,
    Irq::SpmReady,
];

static M328P_TIMERS: [TimerDesc; 3] = [
    TimerDesc {
        n: 0,
        wide: false,
        base: 0x44,
        tifr: 0x35,
        timsk: 0x6E,
        oc: [Some(p(D, 6)), Some(p(D, 5)), None],
        icp: None,
        tclk: Some(p(D, 4)),
        channels: 2,
        async_timer: false,
        prr: (reg::PRR0, 5),
    },
    TimerDesc {
        n: 1,
        wide: true,
        base: 0x80,
        tifr: 0x36,
        timsk: 0x6F,
        oc: [Some(p(B, 1)), Some(p(B, 2)), None],
        icp: Some(p(B, 0)),
        tclk: Some(p(D, 5)),
        channels: 2,
        async_timer: false,
        prr: (reg::PRR0, 3),
    },
    TimerDesc {
        n: 2,
        wide: false,
        base: 0xB0,
        tifr: 0x37,
        timsk: 0x70,
        oc: [Some(p(B, 3)), Some(p(D, 3)), None],
        icp: None,
        tclk: None,
        channels: 2,
        async_timer: true,
        prr: (reg::PRR0, 6),
    },
];

static M328P_USARTS: [UsartDesc; 1] = [UsartDesc {
    n: 0,
    base: 0xC0,
    rx: p(D, 0),
    tx: p(D, 1),
}];

static M328P_ADC: [Pin; 8] = [
    p(C, 0),
    p(C, 1),
    p(C, 2),
    p(C, 3),
    p(C, 4),
    p(C, 5),
    p(C, 6),
    p(C, 7),
];

pub(crate) static M328P: Variant = Variant {
    mcu: Mcu::Atmega328p,
    name: "ATmega328P",
    signature: [0x1E, 0x95, 0x0F],
    flash_bytes: 32 * 1024,
    sram_start: 0x100,
    ramend: 0x8FF,
    eeprom_bytes: 1024,
    page_bytes: 128,
    nrww_start: 0x7000,
    pc22: false,
    ext: false,
    vectors: &M328P_VECTORS,
    ports: &[B, C, D],
    timers: &M328P_TIMERS,
    usarts: &M328P_USARTS,
    ext_ints: &[p(D, 2), p(D, 3)],
    ext_int_async_edges: 0,
    pcint: [
        [
            Some(p(B, 0)),
            Some(p(B, 1)),
            Some(p(B, 2)),
            Some(p(B, 3)),
            Some(p(B, 4)),
            Some(p(B, 5)),
            Some(p(B, 6)),
            Some(p(B, 7)),
        ],
        [
            Some(p(C, 0)),
            Some(p(C, 1)),
            Some(p(C, 2)),
            Some(p(C, 3)),
            Some(p(C, 4)),
            Some(p(C, 5)),
            Some(p(C, 6)),
            None,
        ],
        [
            Some(p(D, 0)),
            Some(p(D, 1)),
            Some(p(D, 2)),
            Some(p(D, 3)),
            Some(p(D, 4)),
            Some(p(D, 5)),
            Some(p(D, 6)),
            Some(p(D, 7)),
        ],
    ],
    // ADC6/ADC7 are analog-only pins on the TQFP package (Nano A6/A7); modelled as port C bits 6/7
    // which have no digital function there.
    adc_pins: &M328P_ADC,
    spi_ss: p(B, 2),
    spi_sck: p(B, 5),
    spi_mosi: p(B, 3),
    spi_miso: p(B, 4),
    twi_sda: p(C, 4),
    twi_scl: p(C, 5),
    ain0: p(D, 6),
    ain1: p(D, 7),
    has_temp_sensor: true,
    fuses: [0xFF, 0xDE, 0xFD, 0xCF],
};

static M2560_VECTORS: [Irq; 57] = [
    Irq::Reset,
    Irq::Int(0),
    Irq::Int(1),
    Irq::Int(2),
    Irq::Int(3),
    Irq::Int(4),
    Irq::Int(5),
    Irq::Int(6),
    Irq::Int(7),
    Irq::Pcint(0),
    Irq::Pcint(1),
    Irq::Pcint(2),
    Irq::Wdt,
    Irq::TimerCompA(2),
    Irq::TimerCompB(2),
    Irq::TimerOvf(2),
    Irq::TimerCapt(1),
    Irq::TimerCompA(1),
    Irq::TimerCompB(1),
    Irq::TimerCompC(1),
    Irq::TimerOvf(1),
    Irq::TimerCompA(0),
    Irq::TimerCompB(0),
    Irq::TimerOvf(0),
    Irq::Spi,
    Irq::UsartRx(0),
    Irq::UsartUdre(0),
    Irq::UsartTx(0),
    Irq::AnaComp,
    Irq::Adc,
    Irq::EeReady,
    Irq::TimerCapt(3),
    Irq::TimerCompA(3),
    Irq::TimerCompB(3),
    Irq::TimerCompC(3),
    Irq::TimerOvf(3),
    Irq::UsartRx(1),
    Irq::UsartUdre(1),
    Irq::UsartTx(1),
    Irq::Twi,
    Irq::SpmReady,
    Irq::TimerCapt(4),
    Irq::TimerCompA(4),
    Irq::TimerCompB(4),
    Irq::TimerCompC(4),
    Irq::TimerOvf(4),
    Irq::TimerCapt(5),
    Irq::TimerCompA(5),
    Irq::TimerCompB(5),
    Irq::TimerCompC(5),
    Irq::TimerOvf(5),
    Irq::UsartRx(2),
    Irq::UsartUdre(2),
    Irq::UsartTx(2),
    Irq::UsartRx(3),
    Irq::UsartUdre(3),
    Irq::UsartTx(3),
];

static M2560_TIMERS: [TimerDesc; 6] = [
    TimerDesc {
        n: 0,
        wide: false,
        base: 0x44,
        tifr: 0x35,
        timsk: 0x6E,
        oc: [Some(p(B, 7)), Some(p(G, 5)), None],
        icp: None,
        tclk: Some(p(D, 7)),
        channels: 2,
        async_timer: false,
        prr: (reg::PRR0, 5),
    },
    TimerDesc {
        n: 1,
        wide: true,
        base: 0x80,
        tifr: 0x36,
        timsk: 0x6F,
        oc: [Some(p(B, 5)), Some(p(B, 6)), Some(p(B, 7))],
        icp: Some(p(D, 4)),
        tclk: Some(p(D, 6)),
        channels: 3,
        async_timer: false,
        prr: (reg::PRR0, 3),
    },
    TimerDesc {
        n: 2,
        wide: false,
        base: 0xB0,
        tifr: 0x37,
        timsk: 0x70,
        oc: [Some(p(B, 4)), Some(p(H, 6)), None],
        icp: None,
        tclk: None,
        channels: 2,
        async_timer: true,
        prr: (reg::PRR0, 6),
    },
    TimerDesc {
        n: 3,
        wide: true,
        base: 0x90,
        tifr: 0x38,
        timsk: 0x71,
        oc: [Some(p(E, 3)), Some(p(E, 4)), Some(p(E, 5))],
        icp: Some(p(E, 7)),
        tclk: Some(p(E, 6)),
        channels: 3,
        async_timer: false,
        prr: (reg::PRR1, 3),
    },
    TimerDesc {
        n: 4,
        wide: true,
        base: 0xA0,
        tifr: 0x39,
        timsk: 0x72,
        oc: [Some(p(H, 3)), Some(p(H, 4)), Some(p(H, 5))],
        icp: Some(p(L, 0)),
        tclk: Some(p(H, 7)),
        channels: 3,
        async_timer: false,
        prr: (reg::PRR1, 4),
    },
    TimerDesc {
        n: 5,
        wide: true,
        base: 0x120,
        tifr: 0x3A,
        timsk: 0x73,
        oc: [Some(p(L, 3)), Some(p(L, 4)), Some(p(L, 5))],
        icp: Some(p(L, 1)),
        tclk: Some(p(L, 2)),
        channels: 3,
        async_timer: false,
        prr: (reg::PRR1, 5),
    },
];

static M2560_USARTS: [UsartDesc; 4] = [
    UsartDesc {
        n: 0,
        base: 0xC0,
        rx: p(E, 0),
        tx: p(E, 1),
    },
    UsartDesc {
        n: 1,
        base: 0xC8,
        rx: p(D, 2),
        tx: p(D, 3),
    },
    UsartDesc {
        n: 2,
        base: 0xD0,
        rx: p(H, 0),
        tx: p(H, 1),
    },
    UsartDesc {
        n: 3,
        base: 0x130,
        rx: p(J, 0),
        tx: p(J, 1),
    },
];

static M2560_ADC: [Pin; 16] = [
    p(F, 0),
    p(F, 1),
    p(F, 2),
    p(F, 3),
    p(F, 4),
    p(F, 5),
    p(F, 6),
    p(F, 7),
    p(K, 0),
    p(K, 1),
    p(K, 2),
    p(K, 3),
    p(K, 4),
    p(K, 5),
    p(K, 6),
    p(K, 7),
];

pub(crate) static M2560: Variant = Variant {
    mcu: Mcu::Atmega2560,
    name: "ATmega2560",
    signature: [0x1E, 0x98, 0x01],
    flash_bytes: 256 * 1024,
    sram_start: 0x200,
    ramend: 0x21FF,
    eeprom_bytes: 4096,
    page_bytes: 256,
    nrww_start: 0x3E000,
    pc22: true,
    ext: true,
    vectors: &M2560_VECTORS,
    ports: &[A, B, C, D, E, F, G, H, J, K, L],
    timers: &M2560_TIMERS,
    usarts: &M2560_USARTS,
    ext_ints: &[
        p(D, 0),
        p(D, 1),
        p(D, 2),
        p(D, 3),
        p(E, 4),
        p(E, 5),
        p(E, 6),
        p(E, 7),
    ],
    ext_int_async_edges: 0x0F,
    pcint: [
        [
            Some(p(B, 0)),
            Some(p(B, 1)),
            Some(p(B, 2)),
            Some(p(B, 3)),
            Some(p(B, 4)),
            Some(p(B, 5)),
            Some(p(B, 6)),
            Some(p(B, 7)),
        ],
        [
            Some(p(E, 0)),
            Some(p(J, 0)),
            Some(p(J, 1)),
            Some(p(J, 2)),
            Some(p(J, 3)),
            Some(p(J, 4)),
            Some(p(J, 5)),
            Some(p(J, 6)),
        ],
        [
            Some(p(K, 0)),
            Some(p(K, 1)),
            Some(p(K, 2)),
            Some(p(K, 3)),
            Some(p(K, 4)),
            Some(p(K, 5)),
            Some(p(K, 6)),
            Some(p(K, 7)),
        ],
    ],
    adc_pins: &M2560_ADC,
    spi_ss: p(B, 0),
    spi_sck: p(B, 1),
    spi_mosi: p(B, 2),
    spi_miso: p(B, 3),
    twi_sda: p(D, 1),
    twi_scl: p(D, 0),
    ain0: p(E, 2),
    ain1: p(E, 3),
    has_temp_sensor: false,
    fuses: [0xFF, 0xD8, 0xFD, 0xCF],
};

impl Variant {
    pub fn vector_of(&self, irq: Irq) -> Option<u8> {
        self.vectors.iter().position(|v| *v == irq).map(|i| i as u8)
    }
    pub fn flash_words(&self) -> usize {
        self.flash_bytes / 2
    }
}
