//! API-level emulation of the ESP32 DevKit and Raspberry Pi Pico (`MCU-6`): sketches are
//! compiled natively against Breadbox's Arduino-compatible host core (`hostcore/`) and run as
//! child processes in lock-step with simulated time over a pipe protocol.
//!
//! * [`compile_host`] — Arduino-style preprocessing + clang++ build (cached).
//! * [`HostBoard`] — spawn + `run(until, io)`; pushed inputs, timestamped output events.
//! * [`HostBoardSpec`] — pin maps, ADC/PWM details, headers.
//!
//! Radio access (Wi-Fi, sockets, BLE) goes through [`HostIo::service`] as opaque byte requests;
//! `bb_radio::RadioSpace::host_service` serves them (see `docs/ARCHITECTURE.md` §5b).

pub mod board;
pub mod catalog;
pub mod compile;
pub mod protocol;

pub use board::{
    HostBoard, HostConfig, HostEvent, HostIo, HostPinState, HostStop, I2cStatus, Pull,
};
pub use catalog::{HostBoardSpec, HostKind, HostPin};
pub use compile::{HostBuild, compile_host, cxx, find_prototypes, hostcore_dir, preprocess};
