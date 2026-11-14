//! Board catalogue: Arduino UNO R3, Nano and Mega 2560 — FQBN, MCU, pin mapping (Arduino pin ↔
//! port/bit), analog channels, PWM pins with their timer/channel and frequency, interrupts,
//! bus pins, LED_BUILTIN and header layouts. Tables follow the arduino:avr `pins_arduino.h`
//! variants (`standard`, `eightanaloginputs`, `mega`).

use bb_avr::{Mcu, Pin};
use serde::Serialize;

/// PWM capability of a pin under the Arduino core's default timer setup.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Pwm {
    pub timer: u8,
    /// 'A', 'B' or 'C'.
    pub channel: char,
    /// analogWrite() frequency with the core's prescalers (976.5625 Hz fast PWM on Timer 0,
    /// 490.196 Hz phase-correct elsewhere).
    pub freq_hz: f64,
}

/// One Arduino pin number.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BoardPin {
    /// Arduino pin number (D0 = 0, A0 = 14 on UNO, 54 on Mega).
    pub number: u8,
    /// Silk-screen style name: "D13", "A0".
    pub name: String,
    /// MCU port pin (`None` never happens for these boards; A6/A7 of the Nano map to the
    /// analog-only ADC6/ADC7 inputs, modelled as PC6/PC7).
    #[serde(skip)]
    pub pin: Option<Pin>,
    /// Port pin as text ("PB5").
    pub port_pin: String,
    /// ADC channel if the pin is an analog input.
    pub adc: Option<u8>,
    /// Digital I/O available (false for the Nano's A6/A7).
    pub digital: bool,
    pub pwm: Option<Pwm>,
    /// External interrupt number for attachInterrupt().
    pub interrupt: Option<u8>,
    /// Alternate functions: "RX0", "TX0", "SDA", "SCL", "MOSI", "MISO", "SCK", "SS", "LED".
    pub functions: Vec<&'static str>,
}

/// A header (row of female sockets) on the board, for drawing and wiring.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Header {
    pub name: &'static str,
    /// Labels in physical order; Arduino pins are "D0", "A3", …; others "5V", "GND", "AREF", …
    pub labels: Vec<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Board {
    /// Short id: "uno", "nano", "mega".
    pub id: &'static str,
    /// Display name: "UNO R3".
    pub name: &'static str,
    pub fqbn: &'static str,
    #[serde(skip)]
    pub mcu: Mcu,
    pub mcu_name: &'static str,
    pub f_cpu: u64,
    /// Flash available to sketches (bootloader excluded) and SRAM, as arduino-cli reports.
    pub max_program: usize,
    pub max_data: usize,
    pub pins: Vec<BoardPin>,
    pub led_builtin: u8,
    pub sda: u8,
    pub scl: u8,
    pub ss: u8,
    pub mosi: u8,
    pub miso: u8,
    pub sck: u8,
    /// Arduino pin numbers of the hardware serial ports: (rx, tx) per USART.
    pub serial: Vec<(u8, u8)>,
    pub headers: Vec<Header>,
}

const FAST: f64 = 16e6 / 64.0 / 256.0;
const PHASE: f64 = 16e6 / 64.0 / 510.0;

fn pwm(timer: u8, channel: char) -> Option<Pwm> {
    Some(Pwm {
        timer,
        channel,
        freq_hz: if timer == 0 { FAST } else { PHASE },
    })
}

fn letter_to_index(l: char) -> u8 {
    b"ABCDEFGHJKL"
        .iter()
        .position(|&c| c as char == l)
        .unwrap_or(0) as u8
}

#[allow(clippy::too_many_arguments)]
fn bp(
    number: u8,
    name: &str,
    port: char,
    bit: u8,
    adc: Option<u8>,
    pwm: Option<Pwm>,
    interrupt: Option<u8>,
    functions: &[&'static str],
) -> BoardPin {
    BoardPin {
        number,
        name: name.to_string(),
        pin: Some(Pin {
            port: letter_to_index(port),
            bit,
        }),
        port_pin: format!("P{port}{bit}"),
        adc,
        digital: true,
        pwm,
        interrupt,
        functions: functions.to_vec(),
    }
}

fn atmega328_pins(eight_analog: bool) -> Vec<BoardPin> {
    let mut v = vec![
        bp(0, "D0", 'D', 0, None, None, None, &["RX0"]),
        bp(1, "D1", 'D', 1, None, None, None, &["TX0"]),
        bp(2, "D2", 'D', 2, None, None, Some(0), &[]),
        bp(3, "D3", 'D', 3, None, pwm(2, 'B'), Some(1), &[]),
        bp(4, "D4", 'D', 4, None, None, None, &[]),
        bp(5, "D5", 'D', 5, None, pwm(0, 'B'), None, &[]),
        bp(6, "D6", 'D', 6, None, pwm(0, 'A'), None, &["AIN0"]),
        bp(7, "D7", 'D', 7, None, None, None, &["AIN1"]),
        bp(8, "D8", 'B', 0, None, None, None, &["ICP1"]),
        bp(9, "D9", 'B', 1, None, pwm(1, 'A'), None, &[]),
        bp(10, "D10", 'B', 2, None, pwm(1, 'B'), None, &["SS"]),
        bp(11, "D11", 'B', 3, None, pwm(2, 'A'), None, &["MOSI"]),
        bp(12, "D12", 'B', 4, None, None, None, &["MISO"]),
        bp(13, "D13", 'B', 5, None, None, None, &["SCK", "LED"]),
    ];
    for i in 0..6u8 {
        let f: &[&'static str] = match i {
            4 => &["SDA"],
            5 => &["SCL"],
            _ => &[],
        };
        v.push(bp(14 + i, &format!("A{i}"), 'C', i, Some(i), None, None, f));
    }
    if eight_analog {
        for i in 6..8u8 {
            let mut p = bp(14 + i, &format!("A{i}"), 'C', i, Some(i), None, None, &[]);
            p.port_pin = format!("ADC{i}");
            p.digital = false;
            v.push(p);
        }
    }
    v
}

fn uno() -> Board {
    Board {
        id: "uno",
        name: "UNO R3",
        fqbn: "arduino:avr:uno",
        mcu: Mcu::Atmega328p,
        mcu_name: "ATmega328P",
        f_cpu: 16_000_000,
        max_program: 32_256,
        max_data: 2_048,
        pins: atmega328_pins(false),
        led_builtin: 13,
        sda: 18,
        scl: 19,
        ss: 10,
        mosi: 11,
        miso: 12,
        sck: 13,
        serial: vec![(0, 1)],
        headers: vec![
            Header {
                name: "POWER",
                labels: vec!["NC", "IOREF", "RESET", "3.3V", "5V", "GND", "GND", "VIN"],
            },
            Header {
                name: "ANALOG IN",
                labels: vec!["A0", "A1", "A2", "A3", "A4", "A5"],
            },
            Header {
                name: "DIGITAL (PWM~) 0-7",
                labels: vec!["D7", "D6", "D5", "D4", "D3", "D2", "D1", "D0"],
            },
            Header {
                name: "DIGITAL (PWM~) 8-13",
                labels: vec![
                    "SCL", "SDA", "AREF", "GND", "D13", "D12", "D11", "D10", "D9", "D8",
                ],
            },
            Header {
                name: "ICSP",
                labels: vec!["MISO", "5V", "SCK", "MOSI", "RESET", "GND"],
            },
        ],
    }
}

fn nano() -> Board {
    Board {
        id: "nano",
        name: "Nano",
        fqbn: "arduino:avr:nano:cpu=atmega328",
        mcu: Mcu::Atmega328p,
        mcu_name: "ATmega328P",
        f_cpu: 16_000_000,
        max_program: 30_720,
        max_data: 2_048,
        pins: atmega328_pins(true),
        led_builtin: 13,
        sda: 18,
        scl: 19,
        ss: 10,
        mosi: 11,
        miso: 12,
        sck: 13,
        serial: vec![(0, 1)],
        headers: vec![
            Header {
                name: "J2 (left)",
                labels: vec![
                    "D13", "3V3", "AREF", "A0", "A1", "A2", "A3", "A4", "A5", "A6", "A7", "5V",
                    "RESET", "GND", "VIN",
                ],
            },
            Header {
                name: "J1 (right)",
                labels: vec![
                    "D12", "D11", "D10", "D9", "D8", "D7", "D6", "D5", "D4", "D3", "D2", "GND",
                    "RESET", "D0", "D1",
                ],
            },
        ],
    }
}

fn mega() -> Board {
    // (number, port, bit, timer/channel, interrupt, functions) from variants/mega/pins_arduino.h
    #[allow(clippy::type_complexity)]
    let digital: [(
        u8,
        char,
        u8,
        Option<(u8, char)>,
        Option<u8>,
        &[&'static str],
    ); 54] = [
        (0, 'E', 0, None, None, &["RX0"]),
        (1, 'E', 1, None, None, &["TX0"]),
        (2, 'E', 4, Some((3, 'B')), Some(0), &[]),
        (3, 'E', 5, Some((3, 'C')), Some(1), &[]),
        (4, 'G', 5, Some((0, 'B')), None, &[]),
        (5, 'E', 3, Some((3, 'A')), None, &["AIN1"]),
        (6, 'H', 3, Some((4, 'A')), None, &[]),
        (7, 'H', 4, Some((4, 'B')), None, &[]),
        (8, 'H', 5, Some((4, 'C')), None, &[]),
        (9, 'H', 6, Some((2, 'B')), None, &[]),
        (10, 'B', 4, Some((2, 'A')), None, &[]),
        (11, 'B', 5, Some((1, 'A')), None, &[]),
        (12, 'B', 6, Some((1, 'B')), None, &[]),
        (13, 'B', 7, Some((0, 'A')), None, &["LED"]),
        (14, 'J', 1, None, None, &["TX3"]),
        (15, 'J', 0, None, None, &["RX3"]),
        (16, 'H', 1, None, None, &["TX2"]),
        (17, 'H', 0, None, None, &["RX2"]),
        (18, 'D', 3, None, Some(5), &["TX1"]),
        (19, 'D', 2, None, Some(4), &["RX1"]),
        (20, 'D', 1, None, Some(3), &["SDA"]),
        (21, 'D', 0, None, Some(2), &["SCL"]),
        (22, 'A', 0, None, None, &[]),
        (23, 'A', 1, None, None, &[]),
        (24, 'A', 2, None, None, &[]),
        (25, 'A', 3, None, None, &[]),
        (26, 'A', 4, None, None, &[]),
        (27, 'A', 5, None, None, &[]),
        (28, 'A', 6, None, None, &[]),
        (29, 'A', 7, None, None, &[]),
        (30, 'C', 7, None, None, &[]),
        (31, 'C', 6, None, None, &[]),
        (32, 'C', 5, None, None, &[]),
        (33, 'C', 4, None, None, &[]),
        (34, 'C', 3, None, None, &[]),
        (35, 'C', 2, None, None, &[]),
        (36, 'C', 1, None, None, &[]),
        (37, 'C', 0, None, None, &[]),
        (38, 'D', 7, None, None, &[]),
        (39, 'G', 2, None, None, &[]),
        (40, 'G', 1, None, None, &[]),
        (41, 'G', 0, None, None, &[]),
        (42, 'L', 7, None, None, &[]),
        (43, 'L', 6, None, None, &[]),
        (44, 'L', 5, Some((5, 'C')), None, &[]),
        (45, 'L', 4, Some((5, 'B')), None, &[]),
        (46, 'L', 3, Some((5, 'A')), None, &[]),
        (47, 'L', 2, None, None, &[]),
        (48, 'L', 1, None, None, &[]),
        (49, 'L', 0, None, None, &[]),
        (50, 'B', 3, None, None, &["MISO"]),
        (51, 'B', 2, None, None, &["MOSI"]),
        (52, 'B', 1, None, None, &["SCK"]),
        (53, 'B', 0, None, None, &["SS"]),
    ];
    let mut pins: Vec<BoardPin> = digital
        .iter()
        .map(|&(n, port, bit, t, irq, f)| {
            bp(
                n,
                &format!("D{n}"),
                port,
                bit,
                None,
                t.and_then(|(tm, ch)| pwm(tm, ch)),
                irq,
                f,
            )
        })
        .collect();
    for i in 0..16u8 {
        let (port, bit) = if i < 8 { ('F', i) } else { ('K', i - 8) };
        pins.push(bp(
            54 + i,
            &format!("A{i}"),
            port,
            bit,
            Some(i),
            None,
            None,
            &[],
        ));
    }
    Board {
        id: "mega",
        name: "Mega 2560",
        fqbn: "arduino:avr:mega:cpu=atmega2560",
        mcu: Mcu::Atmega2560,
        mcu_name: "ATmega2560",
        f_cpu: 16_000_000,
        max_program: 253_952,
        max_data: 8_192,
        pins,
        led_builtin: 13,
        sda: 20,
        scl: 21,
        ss: 53,
        mosi: 51,
        miso: 50,
        sck: 52,
        serial: vec![(0, 1), (19, 18), (17, 16), (15, 14)],
        headers: vec![
            Header {
                name: "POWER",
                labels: vec!["NC", "IOREF", "RESET", "3.3V", "5V", "GND", "GND", "VIN"],
            },
            Header {
                name: "ANALOG IN 0-7",
                labels: vec!["A0", "A1", "A2", "A3", "A4", "A5", "A6", "A7"],
            },
            Header {
                name: "ANALOG IN 8-15",
                labels: vec!["A8", "A9", "A10", "A11", "A12", "A13", "A14", "A15"],
            },
            Header {
                name: "PWM 0-7",
                labels: vec!["D7", "D6", "D5", "D4", "D3", "D2", "D1", "D0"],
            },
            Header {
                name: "PWM 8-13",
                labels: vec![
                    "SCL", "SDA", "AREF", "GND", "D13", "D12", "D11", "D10", "D9", "D8",
                ],
            },
            Header {
                name: "COMMUNICATION",
                labels: vec!["D14", "D15", "D16", "D17", "D18", "D19", "D20", "D21"],
            },
            Header {
                name: "DIGITAL 22-53",
                labels: vec![
                    "5V", "5V", "D22", "D23", "D24", "D25", "D26", "D27", "D28", "D29", "D30",
                    "D31", "D32", "D33", "D34", "D35", "D36", "D37", "D38", "D39", "D40", "D41",
                    "D42", "D43", "D44", "D45", "D46", "D47", "D48", "D49", "D50", "D51", "D52",
                    "D53", "GND", "GND",
                ],
            },
            Header {
                name: "ICSP",
                labels: vec!["MISO", "5V", "SCK", "MOSI", "RESET", "GND"],
            },
        ],
    }
}

impl Board {
    /// Every supported AVR board.
    pub fn all() -> Vec<Board> {
        vec![uno(), nano(), mega()]
    }
    /// Looks up a board by id ("uno") or display name ("UNO R3"), case-insensitively.
    pub fn by_id(id: &str) -> Option<Board> {
        Board::all()
            .into_iter()
            .find(|b| b.id.eq_ignore_ascii_case(id) || b.name.eq_ignore_ascii_case(id))
    }
    /// Looks up a board by FQBN; options after the board id (`:cpu=…`) are ignored.
    pub fn by_fqbn(fqbn: &str) -> Option<Board> {
        let base: Vec<&str> = fqbn.split(':').take(3).collect();
        Board::all().into_iter().find(|b| {
            let bb: Vec<&str> = b.fqbn.split(':').take(3).collect();
            bb == base
        })
    }
    /// Pin by Arduino number.
    pub fn pin(&self, number: u8) -> Option<&BoardPin> {
        self.pins.iter().find(|p| p.number == number)
    }
    /// Pin by name ("D13", "A0", "13", "LED_BUILTIN", "SDA", …).
    pub fn pin_by_name(&self, name: &str) -> Option<&BoardPin> {
        let n = name.trim().to_ascii_uppercase();
        match n.as_str() {
            "LED_BUILTIN" => return self.pin(self.led_builtin),
            "SDA" => return self.pin(self.sda),
            "SCL" => return self.pin(self.scl),
            "SS" => return self.pin(self.ss),
            "MOSI" => return self.pin(self.mosi),
            "MISO" => return self.pin(self.miso),
            "SCK" => return self.pin(self.sck),
            _ => {}
        }
        if let Ok(num) = n.parse::<u8>() {
            return self.pin(num);
        }
        self.pins.iter().find(|p| p.name == n)
    }
    /// Arduino pin number driven by an MCU port pin.
    pub fn arduino_pin_of(&self, pin: Pin) -> Option<&BoardPin> {
        self.pins.iter().find(|p| p.pin == Some(pin) && p.digital)
    }
    /// Analog input pins in channel order.
    pub fn analog_pins(&self) -> Vec<&BoardPin> {
        self.pins.iter().filter(|p| p.adc.is_some()).collect()
    }
    /// PWM-capable pins.
    pub fn pwm_pins(&self) -> Vec<&BoardPin> {
        self.pins.iter().filter(|p| p.pwm.is_some()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uno_mapping() {
        let b = Board::by_id("uno").unwrap();
        assert_eq!(b.pin(13).unwrap().pin, Some(Pin::new('B', 5)));
        assert_eq!(b.pin_by_name("A0").unwrap().pin, Some(Pin::new('C', 0)));
        assert_eq!(b.pin_by_name("LED_BUILTIN").unwrap().number, 13);
        let pwm: Vec<u8> = b.pwm_pins().iter().map(|p| p.number).collect();
        assert_eq!(pwm, vec![3, 5, 6, 9, 10, 11]);
        assert!((b.pin(5).unwrap().pwm.unwrap().freq_hz - 976.5625).abs() < 1e-9);
        assert_eq!(b.pin(9).unwrap().pwm.unwrap().timer, 1);
        assert_eq!(
            b.arduino_pin_of(Pin::new('D', 2)).unwrap().interrupt,
            Some(0)
        );
        assert_eq!(Board::by_fqbn("arduino:avr:uno").unwrap().id, "uno");
        assert_eq!(b.analog_pins().len(), 6);
    }

    #[test]
    fn nano_and_mega() {
        let n = Board::by_fqbn("arduino:avr:nano:cpu=atmega328old").unwrap();
        assert_eq!(n.analog_pins().len(), 8);
        assert!(!n.pin_by_name("A7").unwrap().digital);
        let m = Board::by_id("mega").unwrap();
        assert_eq!(m.mcu, Mcu::Atmega2560);
        assert_eq!(m.pins.len(), 70);
        assert_eq!(m.pin(13).unwrap().pin, Some(Pin::new('B', 7)));
        assert_eq!(m.pin_by_name("A15").unwrap().pin, Some(Pin::new('K', 7)));
        let pwm: Vec<u8> = m.pwm_pins().iter().map(|p| p.number).collect();
        assert_eq!(
            pwm,
            vec![2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 44, 45, 46]
        );
        assert_eq!(m.pin(21).unwrap().interrupt, Some(2));
        assert_eq!(m.serial[1], (19, 18));
    }
}
