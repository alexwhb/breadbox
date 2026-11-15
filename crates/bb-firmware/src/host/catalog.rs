//! Board catalogue for the API-level boards (`PART-1`, `MCU-6`): ESP32 DevKit v1 (DOIT,
//! 30 pins) and Raspberry Pi Pico — GPIO map, ADC channels and resolution, PWM channels,
//! touch/DAC pins, buses, onboard LED and header layouts.

use crate::boards::Header;
use serde::Serialize;

/// Which API-level core a sketch is built for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum HostKind {
    Esp32DevKit,
    Pico,
}

impl HostKind {
    pub fn id(self) -> &'static str {
        match self {
            HostKind::Esp32DevKit => "esp32",
            HostKind::Pico => "pico",
        }
    }
    pub fn by_id(id: &str) -> Option<HostKind> {
        match id.to_ascii_lowercase().as_str() {
            "esp32" | "esp32dev" | "esp32-devkit" | "breadbox:host:esp32" => Some(HostKind::Esp32DevKit),
            "pico" | "rp2040" | "breadbox:host:pico" => Some(HostKind::Pico),
            _ => None,
        }
    }
    /// Preprocessor defines for the host compiler.
    pub fn defines(self) -> &'static [&'static str] {
        match self {
            HostKind::Esp32DevKit => &["-DBB_HOST_ESP32", "-DESP32", "-DARDUINO_ESP32_DEV", "-DARDUINO_ARCH_ESP32", "-DARDUINO_BOARD=\"ESP32_DEV\""],
            HostKind::Pico => &["-DBB_HOST_PICO", "-DARDUINO_ARCH_RP2040", "-DARDUINO_RASPBERRY_PI_PICO", "-DARDUINO_BOARD=\"RASPBERRY_PI_PICO\""],
        }
    }
    pub fn spec(self) -> HostBoardSpec {
        match self {
            HostKind::Esp32DevKit => esp32(),
            HostKind::Pico => pico(),
        }
    }
}

/// One GPIO.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HostPin {
    pub gpio: u8,
    /// Silk-screen label ("D4", "VP", "TX2", "GP15").
    pub name: String,
    /// (ADC unit, channel).
    pub adc: Option<(u8, u8)>,
    pub input_only: bool,
    pub touch: Option<u8>,
    pub dac: Option<u8>,
    pub pwm: bool,
    /// "SDA", "SCL", "MOSI", "MISO", "SCK", "SS", "TX0", "RX0", "TX2", "RX2", "LED", "strap".
    pub functions: Vec<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HostBoardSpec {
    pub kind: HostKind,
    pub id: &'static str,
    pub name: &'static str,
    /// Pseudo-FQBN used in project files.
    pub fqbn: &'static str,
    pub chip: &'static str,
    /// Shown next to the board in the UI ("API-level" vs "cycle-accurate AVR core").
    pub emulation: &'static str,
    pub logic_v: f64,
    pub adc_bits: u8,
    pub adc_default_bits: u8,
    pub adc_full_scale_v: f64,
    pub pwm_channels: u8,
    pub pwm_default_bits: u8,
    pub pwm_default_freq: u32,
    pub led_builtin: u8,
    pub sda: u8,
    pub scl: u8,
    pub mosi: u8,
    pub miso: u8,
    pub sck: u8,
    pub ss: u8,
    /// (rx, tx) per UART.
    pub serial: Vec<(u8, u8)>,
    pub wifi: bool,
    pub ble: bool,
    pub pins: Vec<HostPin>,
    pub headers: Vec<Header>,
}

impl HostBoardSpec {
    pub fn all() -> Vec<HostBoardSpec> {
        vec![esp32(), pico()]
    }
    pub fn pin(&self, gpio: u8) -> Option<&HostPin> {
        self.pins.iter().find(|p| p.gpio == gpio)
    }
    /// By label ("D4", "GP15", "VP", "LED_BUILTIN", "A0" → first ADC pin of the core's A0).
    pub fn pin_by_name(&self, name: &str) -> Option<&HostPin> {
        let n = name.trim().to_ascii_uppercase();
        if n == "LED_BUILTIN" {
            return self.pin(self.led_builtin);
        }
        if let Some(a) = n.strip_prefix('A').and_then(|x| x.parse::<u8>().ok()) {
            let gpio = match self.kind {
                HostKind::Pico => 26 + a,
                HostKind::Esp32DevKit => match a {
                    0 => 36,
                    3 => 39,
                    4 => 32,
                    5 => 33,
                    6 => 34,
                    7 => 35,
                    _ => return None,
                },
            };
            return self.pin(gpio);
        }
        self.pins.iter().find(|p| p.name.eq_ignore_ascii_case(&n) || format!("GPIO{}", p.gpio) == n)
    }
    pub fn analog_pins(&self) -> Vec<&HostPin> {
        self.pins.iter().filter(|p| p.adc.is_some()).collect()
    }
}

fn hp(gpio: u8, name: &str, functions: &[&'static str]) -> HostPin {
    HostPin { gpio, name: name.into(), adc: None, input_only: false, touch: None, dac: None, pwm: true, functions: functions.to_vec() }
}

fn esp32() -> HostBoardSpec {
    let adc1 = [(36, 0), (39, 3), (32, 4), (33, 5), (34, 6), (35, 7)];
    let adc2 = [(4, 0), (0, 1), (2, 2), (15, 3), (13, 4), (12, 5), (14, 6), (27, 7), (25, 8), (26, 9)];
    let touch = [(4, 0), (0, 1), (2, 2), (15, 3), (13, 4), (12, 5), (14, 6), (27, 7), (33, 8), (32, 9)];
    let mut pins = vec![
        hp(36, "VP", &[]),
        hp(39, "VN", &[]),
        hp(34, "D34", &[]),
        hp(35, "D35", &[]),
        hp(32, "D32", &[]),
        hp(33, "D33", &[]),
        hp(25, "D25", &[]),
        hp(26, "D26", &[]),
        hp(27, "D27", &[]),
        hp(14, "D14", &[]),
        hp(12, "D12", &["strap"]),
        hp(13, "D13", &[]),
        hp(23, "D23", &["MOSI"]),
        hp(22, "D22", &["SCL"]),
        hp(1, "TX0", &["TX0"]),
        hp(3, "RX0", &["RX0"]),
        hp(21, "D21", &["SDA"]),
        hp(19, "D19", &["MISO"]),
        hp(18, "D18", &["SCK"]),
        hp(5, "D5", &["SS", "strap"]),
        hp(17, "TX2", &["TX2"]),
        hp(16, "RX2", &["RX2"]),
        hp(4, "D4", &[]),
        hp(2, "D2", &["LED", "strap"]),
        hp(15, "D15", &["strap"]),
    ];
    for p in &mut pins {
        p.adc = adc1.iter().find(|a| a.0 == p.gpio).map(|a| (1, a.1)).or_else(|| adc2.iter().find(|a| a.0 == p.gpio).map(|a| (2, a.1)));
        p.touch = touch.iter().find(|t| t.0 == p.gpio).map(|t| t.1);
        p.dac = match p.gpio {
            25 => Some(1),
            26 => Some(2),
            _ => None,
        };
        if matches!(p.gpio, 34 | 35 | 36 | 39) {
            p.input_only = true;
            p.pwm = false;
        }
    }
    HostBoardSpec {
        kind: HostKind::Esp32DevKit,
        id: "esp32",
        name: "ESP32 DevKit v1",
        fqbn: "breadbox:host:esp32",
        chip: "ESP32-WROOM-32 (ESP32-D0WDQ6, 240 MHz dual core)",
        emulation: "API-level",
        logic_v: 3.3,
        adc_bits: 12,
        adc_default_bits: 12,
        adc_full_scale_v: 3.3,
        pwm_channels: 16,
        pwm_default_bits: 8,
        pwm_default_freq: 1000,
        led_builtin: 2,
        sda: 21,
        scl: 22,
        mosi: 23,
        miso: 19,
        sck: 18,
        ss: 5,
        serial: vec![(3, 1), (9, 10), (16, 17)],
        wifi: true,
        ble: true,
        pins,
        headers: vec![
            Header { name: "J2", labels: vec!["EN", "VP", "VN", "D34", "D35", "D32", "D33", "D25", "D26", "D27", "D14", "D12", "D13", "GND", "VIN"] },
            Header { name: "J3", labels: vec!["D23", "D22", "TX0", "RX0", "D21", "D19", "D18", "D5", "TX2", "RX2", "D4", "D2", "D15", "GND", "3V3"] },
        ],
    }
}

fn pico() -> HostBoardSpec {
    let mut pins: Vec<HostPin> = (0..=22).map(|g| hp(g, &format!("GP{g}"), &[])).collect();
    pins.push(hp(25, "GP25", &["LED"]));
    for (g, ch) in [(26u8, 0u8), (27, 1), (28, 2)] {
        let mut p = hp(g, &format!("GP{g}"), &[]);
        p.adc = Some((0, ch));
        pins.push(p);
    }
    let mut vsys = hp(29, "VSYS/3", &[]);
    vsys.adc = Some((0, 3));
    vsys.input_only = true;
    vsys.pwm = false;
    pins.push(vsys);
    let mut set = |g: u8, f: &'static str| {
        if let Some(p) = pins.iter_mut().find(|p| p.gpio == g) {
            p.functions.push(f);
        }
    };
    for (g, f) in [(0, "TX0"), (1, "RX0"), (4, "SDA"), (5, "SCL"), (16, "MISO"), (17, "SS"), (18, "SCK"), (19, "MOSI"), (8, "TX1"), (9, "RX1")] {
        set(g, f);
    }
    HostBoardSpec {
        kind: HostKind::Pico,
        id: "pico",
        name: "Raspberry Pi Pico",
        fqbn: "breadbox:host:pico",
        chip: "RP2040 (dual Cortex-M0+, 133 MHz)",
        emulation: "API-level",
        logic_v: 3.3,
        adc_bits: 12,
        adc_default_bits: 12,
        adc_full_scale_v: 3.3,
        pwm_channels: 16,
        pwm_default_bits: 8,
        pwm_default_freq: 1000,
        led_builtin: 25,
        sda: 4,
        scl: 5,
        mosi: 19,
        miso: 16,
        sck: 18,
        ss: 17,
        serial: vec![(1, 0), (9, 8)],
        wifi: false,
        ble: false,
        pins,
        headers: vec![
            Header {
                name: "left",
                labels: vec![
                    "GP0", "GP1", "GND", "GP2", "GP3", "GP4", "GP5", "GND", "GP6", "GP7", "GP8", "GP9", "GND", "GP10", "GP11", "GP12", "GP13", "GND", "GP14", "GP15",
                ],
            },
            Header {
                name: "right",
                labels: vec![
                    "VBUS", "VSYS", "GND", "3V3_EN", "3V3", "ADC_VREF", "GP28", "AGND", "GP27", "GP26", "RUN", "GP22", "GND", "GP21", "GP20", "GP19", "GP18", "GND", "GP17", "GP16",
                ],
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn esp32_and_pico_maps() {
        let e = HostKind::Esp32DevKit.spec();
        assert_eq!(e.pin_by_name("LED_BUILTIN").unwrap().gpio, 2);
        assert_eq!(e.pin_by_name("A0").unwrap().name, "VP");
        assert_eq!(e.pin(36).unwrap().adc, Some((1, 0)));
        assert!(e.pin(34).unwrap().input_only && !e.pin(34).unwrap().pwm);
        assert_eq!(e.pin(25).unwrap().dac, Some(1));
        assert_eq!(e.pin(4).unwrap().touch, Some(0));
        assert_eq!(e.analog_pins().len(), 15);
        assert_eq!(e.headers.iter().map(|h| h.labels.len()).sum::<usize>(), 30);
        let p = HostKind::Pico.spec();
        assert_eq!(p.led_builtin, 25);
        assert_eq!(p.pin_by_name("A2").unwrap().gpio, 28);
        assert_eq!(p.pin_by_name("GP15").unwrap().gpio, 15);
        assert_eq!(p.adc_default_bits, 12);
        assert_eq!(p.headers.iter().map(|h| h.labels.len()).sum::<usize>(), 40);
        assert_eq!(HostKind::by_id("breadbox:host:pico"), Some(HostKind::Pico));
    }
}
