//! Firmware toolchain integration for Breadbox.
//!
//! * [`Toolchain`] — locate or install `arduino-cli` + the `arduino:avr` core (with native
//!   avr-gcc/ctags overrides on Rosetta-less Apple Silicon), manage libraries.
//! * [`Toolchain::compile`] — build a sketch folder into ELF/HEX with a size report that
//!   reproduces arduino-cli's text and diagnostics mapped back to the user's tabs.
//! * [`DebugInfo`] — DWARF line table (both directions), functions, globals/locals of basic
//!   types with their locations, symbols, CFI.
//! * [`Debugger`] — breakpoints with hit conditions, stepping by source line, call stack and
//!   variables over a [`bb_avr::Avr`], usable while an outer scheduler drives the core.
//! * [`Board`] — UNO R3 / Nano / Mega 2560 pin mapping.
//! * [`host`] — ESP32 DevKit / Raspberry Pi Pico API-level emulation (`MCU-6`): host-compiled
//!   sketches running in lock-step with simulated time ([`host::HostBoard`]).

pub mod boards;
pub mod compile;
pub mod debugger;
pub mod debuginfo;
pub mod host;
pub mod toolchain;

pub use boards::{Board, BoardPin, Header, Pwm};
pub use compile::{
    BuildOutput, CompileOptions, Diagnostic, InoMap, OutputKind, Severity, SizeReport, Warnings,
    banner, thousands,
};
pub use debugger::{
    Debugger, Decision, HitCondition, PauseInfo, PauseReason, SourceBreakpoint, StackFrame,
};
pub use debuginfo::{
    DebugInfo, FrameRegs, Function, SourceLocation, TypeInfo, Value, VarValue, Variable,
};
pub use toolchain::{
    CTAGS_SOURCE_URL, DetectOptions, InstallProgress, InstalledLibrary, LibraryInfo, NativeInstallOptions, NativeOverrides,
    Toolchain, ToolchainSource, build_ctags,
};

use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq)]
pub enum FirmwareError {
    #[error(
        "arduino-cli was not found (install it from the firmware screen, or `brew install arduino-cli`)"
    )]
    ToolchainMissing,
    #[error("arduino-cli: {0}")]
    Cli(String),
    #[error("download failed: {0}")]
    Download(String),
    #[error("I/O: {0}")]
    Io(String),
    #[error("ELF: {0}")]
    Elf(String),
    #[error("DWARF: {0}")]
    Dwarf(String),
    #[error("the build produced no firmware")]
    NoOutput,
    #[error("no code at {0}")]
    NoCode(String),
    #[error("{0}")]
    NativeTools(String),
}
