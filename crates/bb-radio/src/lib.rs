//! Virtual RF space for Breadbox (`RF-1`…`RF-8`): pure state machines over bytes and
//! [`bb_core::SimTime`] — no threads, no wall clock (except the optional LAN bridge's I/O
//! thread), deterministic for a given seed.
//!
//! * [`space::RadioSpace`] — the co-simulator's single entry point: Wi-Fi network, BLE air +
//!   virtual phone, LoRa medium + gateway, and every radio module on the bench.
//! * [`net::Network`] — APs, stations, DHCP/DNS/TCP/UDP/ICMP, the MQTT broker, HTTP/NTP/echo
//!   services, fault injection, [`log::PacketLog`] (pcap-ng/CSV export).
//! * [`espat::EspAt`] — ESP-01 with ESP-AT firmware; [`ble::Hm10`], [`ble::Hc05`];
//!   [`lora::Rylr896`].
//! * [`uart`] — `UartDecoder` / `UartEncoder` to wire modules to MCU pins; [`space::UartWire`].
//! * [`mqtt`] — MQTT 3.1.1 codec, broker, client; [`bridge::LanBridge`] mirrors the broker to a
//!   real one on the LAN.
//! * [`host`] — Wi-Fi/socket/BLE services for host-emulated boards (ESP32).

pub mod at;
pub mod ble;
pub mod bridge;
pub mod espat;
pub mod geo;
pub mod host;
pub mod log;
pub mod lora;
pub mod mqtt;
pub mod net;
pub mod rng;
pub mod space;
pub mod uart;
pub mod wire;

pub use space::{Annotation, DeviceId, ModulePin, RadioCard, RadioSpace, UartWire};
