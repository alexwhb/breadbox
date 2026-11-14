//! Cycle-accurate AVR (ATmega328P / ATmega2560) microcontroller emulator with debugger support.
//!
//! The emulator runs real compiled firmware (Intel HEX or ELF from avr-gcc / arduino-cli).
//! It is built for co-simulation (`docs/ARCHITECTURE.md` §5):
//!
//! * [`Avr::run`] advances the core to a target cycle but **stops early** on externally visible
//!   events (pin drive changes, ADC sample requests, breakpoints, sleep, resets, faults) so an
//!   outer scheduler can keep an analog solver exact.
//! * Pin state is reported *logically* ([`PinState`]: input with/without pull-up, or output
//!   high/low); converting that into an electrical drive is the co-simulator's job.
//! * Inputs are *pushed*: [`Avr::set_pin_input`] / [`Avr::schedule_pin_input`] for digital line
//!   levels (edges drive INTn/PCINT/USART RX/SPI/TWI/input capture) and
//!   [`Avr::set_pin_voltage`] for the analog comparator. ADC samples are *pulled* at the exact
//!   sample-and-hold instant through [`AvrIo::adc_sample`].
//! * Time is counted in oscillator cycles (`f_cpu`, default 16 MHz); [`Avr::time`] converts to
//!   [`bb_core::SimTime`] exactly.
//!
//! Peripherals are event driven (no per-cycle ticking): timers, USARTs, ADC, SPI, TWI,
//! EEPROM, watchdog compute the cycle of their next observable event and the core runs straight
//! to it. Flash is pre-decoded, which keeps a busy 16 MHz sketch well above real time.

mod adc;
mod avr;
mod debug;
mod decode;
mod exec;
mod gpio;
mod io;
mod loader;
mod misc;
mod spi;
mod timer;
mod twi;
mod usart;
mod variant;

pub use avr::Avr;
pub use debug::{CallFrame, CallKind, StepFilter, Watchpoint};
pub use decode::{Op, decode, is_two_word};
pub use loader::{FirmwareImage, LoadError, Segment};
pub use variant::{Irq, Mcu};

use std::fmt;

/// A GPIO pin: `port` is the index into `"ABCDEFGHJKL"` (A=0, B=1, …, H=7, J=8, K=9, L=10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Pin {
    pub port: u8,
    pub bit: u8,
}

impl Pin {
    /// `Pin::new('B', 5)` is PB5. Panics on an invalid letter (programmer error).
    pub const fn new(letter: char, bit: u8) -> Pin {
        let port = match letter {
            'A' => 0,
            'B' => 1,
            'C' => 2,
            'D' => 3,
            'E' => 4,
            'F' => 5,
            'G' => 6,
            'H' => 7,
            'J' => 8,
            'K' => 9,
            'L' => 10,
            _ => panic!("invalid port letter"),
        };
        assert!(bit < 8);
        Pin { port, bit }
    }
    /// Port letter, e.g. `'B'`.
    pub fn letter(self) -> char {
        variant::PORT_LETTERS[self.port as usize % 11] as char
    }
    /// Parses `"PB5"` / `"B5"`.
    pub fn parse(s: &str) -> Option<Pin> {
        let s = s.trim();
        let s = s.strip_prefix('P').unwrap_or(s);
        let mut ch = s.chars();
        let letter = ch.next()?.to_ascii_uppercase();
        let bit: u8 = ch.as_str().parse().ok()?;
        let port = variant::PORT_LETTERS
            .iter()
            .position(|&c| c as char == letter)? as u8;
        (bit < 8).then_some(Pin { port, bit })
    }
    pub(crate) fn index(self) -> usize {
        self.port as usize * 8 + self.bit as usize
    }
}

impl fmt::Display for Pin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "P{}{}", self.letter(), self.bit)
    }
}

/// Logical drive state of a pin (what the MCU does to it, not the resulting voltage).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PinState {
    /// High impedance input, optionally with the internal pull-up (≈ 35 kΩ) enabled.
    Input { pullup: bool },
    /// Push-pull output.
    Output { high: bool },
}

impl Default for PinState {
    fn default() -> Self {
        PinState::Input { pullup: false }
    }
}

/// A change of a pin's drive, stamped with the exact cycle it happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PinEvent {
    pub cycle: u64,
    pub pin: Pin,
    pub state: PinState,
}

/// Which hardware block currently controls a pin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PinOwner {
    #[default]
    Gpio,
    OutputCompare {
        timer: u8,
        channel: u8,
    },
    UsartTx(u8),
    UsartRx(u8),
    Spi,
    Twi,
}

/// Full logical state of one pin, for the co-simulator and the debugger's I/O view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PinInfo {
    /// DDRx bit as written by firmware.
    pub ddr: bool,
    /// PORTx bit as written by firmware.
    pub port: bool,
    /// Internal pull-up active (input, PORTx=1, PUD=0).
    pub pullup: bool,
    /// Peripheral overriding the GPIO function, if any.
    pub owner: PinOwner,
    /// Effective drive.
    pub state: PinState,
    /// Logic level the input buffer currently sees.
    pub level: bool,
    /// Externally supplied level (see [`Avr::set_pin_input`]); `None` = nothing connected.
    pub external: Option<bool>,
}

/// One byte that left a USART (fast-path serial monitor), stamped with the cycle its stop bit
/// ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsartByte {
    pub cycle: u64,
    pub data: u16,
}

/// Why [`Avr::run`] returned.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StopReason {
    /// The requested cycle was reached (the core may overshoot by up to one instruction).
    ReachedCycle,
    /// One or more pins changed drive; see [`Avr::take_pin_events`].
    PinChange,
    /// The ADC reached its sample-and-hold instant (`cycle`) for `pin` and [`AvrIo::adc_sample`]
    /// returned `None`. Bring the analog side to `cycle`, call [`Avr::supply_adc_sample`] and run
    /// again. Running without supplying returns this again.
    AdcSampleNeeded { pin: Pin, cycle: u64 },
    /// Execution reached a breakpoint at byte address `pc` (not yet executed).
    Breakpoint { pc: u32 },
    /// A data watchpoint triggered on `addr` (the access has happened).
    Watchpoint { addr: u16, write: bool, pc: u32 },
    /// A `BREAK` instruction executed at `pc`.
    Break { pc: u32 },
    /// The active [`StepFilter`] was satisfied.
    StepDone,
    /// The core entered a sleep mode (only if [`StopOn::sleep`]).
    Sleep { mode: SleepMode },
    /// The core was reset (only if [`StopOn::reset`]).
    Reset { cause: ResetCause },
    /// [`Avr::request_pause`] was called.
    Paused,
    /// Something went wrong in the firmware (the core keeps running if resumed).
    Fault { kind: FaultKind, pc: u32 },
}

/// Firmware faults the emulator can detect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultKind {
    /// Undefined opcode (e.g. execution ran into erased flash, `0xFFFF`). Executed as a NOP.
    InvalidOpcode(u16),
    /// The stack pointer dropped below the start of SRAM (stack overflow into I/O space).
    StackOverflow { sp: u16 },
}

/// Reset sources (MCUSR bits).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResetCause {
    PowerOn,
    External,
    BrownOut,
    Watchdog,
}

impl ResetCause {
    pub(crate) fn mcusr_bit(self) -> u8 {
        match self {
            ResetCause::PowerOn => 0x01,
            ResetCause::External => 0x02,
            ResetCause::BrownOut => 0x04,
            ResetCause::Watchdog => 0x08,
        }
    }
}

/// Sleep modes (SMCR SM2:0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SleepMode {
    Idle,
    AdcNoiseReduction,
    PowerDown,
    PowerSave,
    Standby,
    ExtendedStandby,
}

/// Which events make [`Avr::run`] return early.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StopOn {
    pub pin_change: bool,
    pub sleep: bool,
    pub reset: bool,
    /// Stop on `BREAK` (otherwise it is a NOP, like hardware without OCD).
    pub break_instruction: bool,
    /// Stop on undefined opcodes / stack overflow.
    pub faults: bool,
}

impl Default for StopOn {
    fn default() -> Self {
        StopOn {
            pin_change: true,
            sleep: false,
            reset: false,
            break_instruction: true,
            faults: true,
        }
    }
}

/// What the (not emulated) bootloader does between reset and the sketch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Bootloader {
    /// Behave like the Arduino bootloaders (Optiboot / stk500v2) on their fast path: MCUSR is
    /// copied to r2 and cleared, and the watchdog is disabled before the sketch starts. This is
    /// what a real UNO/Mega does after a watchdog reset.
    #[default]
    ArduinoHandoff,
    /// Bare chip: the sketch starts at 0 with MCUSR and the watchdog as the hardware leaves them.
    None,
}

/// Emulator configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct AvrConfig {
    pub mcu: Mcu,
    /// Oscillator frequency in Hz (default 16 MHz).
    pub f_cpu: u64,
    pub stop_on: StopOn,
    pub bootloader: Bootloader,
    /// Supply voltage (Vcc = AVcc) at start.
    pub vcc: f64,
    /// Brown-out detector trip level (`None` = BOD disabled). Arduino boards ship 2.7 V.
    pub bod_level: Option<f64>,
    /// Oscillator start-up time after power-down/power-save, in cycles (16K CK for a crystal
    /// with CKSEL=1111 SUT=11 as on Arduino boards).
    pub startup_cycles: u64,
    /// Delay after a power-on/brown-out reset is released before the core starts (65 ms + 16K
    /// CK on Arduino fuses). Not applied to the initial power-up at cycle 0.
    pub reset_delay_cycles: u64,
    /// Gaussian ADC noise (standard deviation, in LSB). 0 = ideal quantization.
    pub adc_noise_lsb: f64,
    /// Seed for every random source (ADC noise). Same seed → identical run.
    pub seed: u64,
    /// Keep a shadow call stack (exact call stack for the debugger at ~zero cost).
    pub shadow_stack: bool,
}

impl AvrConfig {
    pub fn new(mcu: Mcu) -> Self {
        AvrConfig {
            mcu,
            f_cpu: 16_000_000,
            stop_on: StopOn::default(),
            bootloader: Bootloader::ArduinoHandoff,
            vcc: 5.0,
            bod_level: Some(2.7),
            startup_cycles: 16 * 1024,
            reset_delay_cycles: 16 * 1024 + 14 + 65 * 16_000,
            adc_noise_lsb: 0.0,
            seed: 0,
            shadow_stack: true,
        }
    }
}

/// Analog/bus callbacks the emulator makes while running.
///
/// Digital inputs are pushed with [`Avr::set_pin_input`] instead (the core must know every edge
/// to drive interrupts), so this trait is small. `()` implements it with all defaults.
pub trait AvrIo {
    /// Voltage (vs. GND) on `pin` at the ADC sample-and-hold instant `cycle`.
    ///
    /// Return `None` to fall back to the last [`Avr::set_pin_voltage`] value for that pin; if
    /// none was ever set, [`Avr::run`] stops with [`StopReason::AdcSampleNeeded`].
    fn adc_sample(&mut self, pin: Pin, cycle: u64) -> Option<f64> {
        let _ = (pin, cycle);
        None
    }
    /// SPI master fast path, used only when the SPI pin waveform is disabled
    /// ([`Avr::set_spi_waveform`]): exchange one byte with the selected slave. Default: bus idle
    /// (MISO high → `0xFF`).
    fn spi_exchange(&mut self, mosi: u8, cycle: u64) -> u8 {
        let _ = (mosi, cycle);
        0xFF
    }
}

impl AvrIo for () {}

/// Errors from loading or flashing firmware.
pub use loader::LoadError as Error;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_names() {
        assert_eq!(Pin::new('B', 5).to_string(), "PB5");
        assert_eq!(Pin::parse("PL3"), Some(Pin::new('L', 3)));
        assert_eq!(Pin::parse("c0"), Some(Pin::new('C', 0)));
        assert_eq!(Pin::parse("PI0"), None);
        assert_eq!(Pin::parse("PB8"), None);
    }
}
