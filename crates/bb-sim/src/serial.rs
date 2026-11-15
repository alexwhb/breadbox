//! Serial monitor streams (WB-11): bytes from an MCU's USART0 assembled into timestamped
//! lines, interleaved with system lines (resets) and radio annotations (RF-8).

use bb_core::SimTime;
use serde::{Deserialize, Serialize};

/// Colour class of a line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineKind {
    /// Bytes printed by the firmware.
    Data,
    /// Simulator / system (grey).
    System,
    /// Radio / network annotation (cyan).
    Net,
    /// Warning (amber).
    Warn,
}

/// One serial-monitor line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SerialLine {
    /// Time of the line's first byte.
    pub time: SimTime,
    pub text: String,
    pub kind: LineKind,
    /// Which MCU (designator).
    pub mcu: String,
}

/// Line assembly for one MCU.
#[derive(Debug, Clone)]
pub struct SerialMonitor {
    pub mcu: String,
    pub lines: Vec<SerialLine>,
    partial: Vec<u8>,
    partial_start: SimTime,
    /// Lines handed out by [`SerialMonitor::take_new`].
    taken: usize,
    pub(crate) seen_resets: u64,
    /// Keep at most this many lines (oldest dropped).
    pub max_lines: usize,
    pub bytes: u64,
    /// Show radio annotations (RF-8 toggle).
    pub annotations: bool,
}

impl SerialMonitor {
    pub fn new(mcu: &str) -> SerialMonitor {
        SerialMonitor {
            mcu: mcu.into(),
            lines: Vec::new(),
            partial: Vec::new(),
            partial_start: SimTime::ZERO,
            taken: 0,
            seen_resets: 1,
            max_lines: 20_000,
            bytes: 0,
            annotations: true,
        }
    }

    fn push(&mut self, time: SimTime, text: String, kind: LineKind) {
        self.lines.push(SerialLine {
            time,
            text,
            kind,
            mcu: self.mcu.clone(),
        });
        if self.lines.len() > self.max_lines {
            let drop = self.lines.len() - self.max_lines;
            self.lines.drain(..drop);
            self.taken = self.taken.saturating_sub(drop);
        }
    }

    /// A byte from the firmware.
    pub fn byte(&mut self, t: SimTime, b: u8) {
        self.bytes += 1;
        if self.partial.is_empty() {
            self.partial_start = t;
        }
        match b {
            b'\n' => {
                let text = String::from_utf8_lossy(&self.partial)
                    .trim_end_matches('\r')
                    .to_string();
                self.partial.clear();
                let start = self.partial_start;
                self.push(start, text, LineKind::Data);
            }
            _ => {
                self.partial.push(b);
                if self.partial.len() > 4096 {
                    let text = String::from_utf8_lossy(&self.partial).to_string();
                    self.partial.clear();
                    let start = self.partial_start;
                    self.push(start, text, LineKind::Data);
                }
            }
        }
    }

    pub fn system(&mut self, t: SimTime, text: &str) {
        self.push(t, text.into(), LineKind::System);
    }
    pub fn warn(&mut self, t: SimTime, text: &str) {
        self.push(t, text.into(), LineKind::Warn);
    }
    /// `wifi: AT+CWJAP="SIM-AP" → OK …`
    pub fn annotation(&mut self, t: SimTime, channel: &str, text: &str) {
        if self.annotations {
            self.push(t, format!("{channel}: {text}"), LineKind::Net);
        }
    }

    /// Text received but not yet terminated by a newline.
    pub fn pending(&self) -> String {
        String::from_utf8_lossy(&self.partial).to_string()
    }

    /// Lines added since the last call.
    pub fn take_new(&mut self) -> Vec<SerialLine> {
        let out = self.lines[self.taken..].to_vec();
        self.taken = self.lines.len();
        out
    }

    /// Firmware lines only.
    pub fn data_lines(&self) -> impl Iterator<Item = &SerialLine> {
        self.lines.iter().filter(|l| l.kind == LineKind::Data)
    }

    pub fn clear(&mut self) {
        self.lines.clear();
        self.taken = 0;
        self.partial.clear();
    }
}

/// Formats a time like the serial monitor (`00:10.500`).
pub fn stamp(t: SimTime) -> String {
    t.to_string()
}
