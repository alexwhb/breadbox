//! Firmware loaders: Intel HEX and AVR ELF executables (program headers, LMA placement, so
//! `.data` initialisers land after `.text` exactly like avr-objcopy produces them).

use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum LoadError {
    #[error("Intel HEX line {line}: {msg}")]
    Hex { line: usize, msg: String },
    #[error("ELF: {0}")]
    Elf(String),
    #[error("image is {size} bytes but the memory holds {max}")]
    TooLarge { size: usize, max: usize },
    #[error("image contains no program data")]
    Empty,
}

/// A contiguous block of bytes at a (flash or EEPROM) byte address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub addr: u32,
    pub data: Vec<u8>,
}

/// A parsed firmware image ready to flash.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FirmwareImage {
    pub flash: Vec<Segment>,
    pub eeprom: Vec<Segment>,
    /// Entry point (byte address); 0 for HEX files.
    pub entry: u32,
}

impl FirmwareImage {
    /// Highest flash byte address used + 1.
    pub fn flash_end(&self) -> usize {
        self.flash
            .iter()
            .map(|s| s.addr as usize + s.data.len())
            .max()
            .unwrap_or(0)
    }

    /// Total number of flash bytes in the image.
    pub fn flash_bytes(&self) -> usize {
        self.flash.iter().map(|s| s.data.len()).sum()
    }

    /// Parses Intel HEX (record types 00, 01, 02, 03, 04, 05).
    pub fn from_hex(text: &str) -> Result<FirmwareImage, LoadError> {
        let mut segs: Vec<Segment> = Vec::new();
        let mut base: u32 = 0;
        let mut saw_eof = false;
        for (ln, raw) in text.lines().enumerate() {
            let line = ln + 1;
            let l = raw.trim();
            if l.is_empty() {
                continue;
            }
            let err = |msg: &str| LoadError::Hex {
                line,
                msg: msg.to_string(),
            };
            let body = l.strip_prefix(':').ok_or_else(|| err("missing ':'"))?;
            if body.len() < 10 || body.len() % 2 != 0 {
                return Err(err("bad record length"));
            }
            let bytes: Vec<u8> = (0..body.len() / 2)
                .map(|i| u8::from_str_radix(&body[2 * i..2 * i + 2], 16))
                .collect::<Result<_, _>>()
                .map_err(|_| err("invalid hex digit"))?;
            let n = bytes[0] as usize;
            if bytes.len() != n + 5 {
                return Err(err("byte count mismatch"));
            }
            let sum = bytes.iter().fold(0u8, |a, b| a.wrapping_add(*b));
            if sum != 0 {
                return Err(err("checksum mismatch"));
            }
            let addr = u16::from_be_bytes([bytes[1], bytes[2]]) as u32;
            let data = &bytes[4..4 + n];
            match bytes[3] {
                0x00 => {
                    let a = base + addr;
                    match segs.last_mut() {
                        Some(s) if s.addr + s.data.len() as u32 == a => {
                            s.data.extend_from_slice(data)
                        }
                        _ => segs.push(Segment {
                            addr: a,
                            data: data.to_vec(),
                        }),
                    }
                }
                0x01 => {
                    saw_eof = true;
                    break;
                }
                0x02 => {
                    if n != 2 {
                        return Err(err("bad type 02 record"));
                    }
                    base = (u16::from_be_bytes([data[0], data[1]]) as u32) << 4;
                }
                0x04 => {
                    if n != 2 {
                        return Err(err("bad type 04 record"));
                    }
                    base = (u16::from_be_bytes([data[0], data[1]]) as u32) << 16;
                }
                0x03 | 0x05 => {}
                t => return Err(err(&format!("unknown record type {t:02X}"))),
            }
        }
        if !saw_eof && segs.is_empty() {
            return Err(LoadError::Empty);
        }
        segs.sort_by_key(|s| s.addr);
        Ok(FirmwareImage {
            flash: segs,
            eeprom: Vec::new(),
            entry: 0,
        })
    }

    /// Parses a 32-bit little-endian AVR ELF executable.
    pub fn from_elf(b: &[u8]) -> Result<FirmwareImage, LoadError> {
        let e = |m: &str| LoadError::Elf(m.to_string());
        if b.len() < 52 || &b[0..4] != b"\x7fELF" {
            return Err(e("not an ELF file"));
        }
        if b[4] != 1 || b[5] != 1 {
            return Err(e("not a 32-bit little-endian ELF"));
        }
        let u16le = |o: usize| -> Result<u16, LoadError> {
            b.get(o..o + 2)
                .map(|s| u16::from_le_bytes([s[0], s[1]]))
                .ok_or_else(|| e("truncated"))
        };
        let u32le = |o: usize| -> Result<u32, LoadError> {
            b.get(o..o + 4)
                .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
                .ok_or_else(|| e("truncated"))
        };
        let machine = u16le(18)?;
        if machine != 83 {
            return Err(LoadError::Elf(format!("machine {machine} is not AVR (83)")));
        }
        let entry = u32le(24)?;
        let phoff = u32le(28)? as usize;
        let phentsize = u16le(42)? as usize;
        let phnum = u16le(44)? as usize;
        let mut img = FirmwareImage {
            entry,
            ..Default::default()
        };
        for i in 0..phnum {
            let o = phoff + i * phentsize;
            let ptype = u32le(o)?;
            if ptype != 1 {
                continue;
            }
            let offset = u32le(o + 4)? as usize;
            let paddr = u32le(o + 12)?;
            let filesz = u32le(o + 16)? as usize;
            if filesz == 0 {
                continue;
            }
            let data = b
                .get(offset..offset + filesz)
                .ok_or_else(|| e("segment outside file"))?
                .to_vec();
            if paddr < 0x80_0000 {
                img.flash.push(Segment { addr: paddr, data });
            } else if (0x81_0000..0x82_0000).contains(&paddr) {
                img.eeprom.push(Segment {
                    addr: paddr - 0x81_0000,
                    data,
                });
            }
        }
        if img.flash.is_empty() {
            return Err(LoadError::Empty);
        }
        img.flash.sort_by_key(|s| s.addr);
        Ok(img)
    }

    /// Serialises the flash part as Intel HEX (16 bytes per record, type 04 above 64 KiB).
    pub fn to_hex(&self) -> String {
        let mut out = String::new();
        let mut cur_hi: u32 = 0;
        let rec = |out: &mut String, addr: u16, ty: u8, data: &[u8]| {
            let mut sum: u8 = data.len() as u8;
            sum = sum
                .wrapping_add((addr >> 8) as u8)
                .wrapping_add(addr as u8)
                .wrapping_add(ty);
            out.push_str(&format!(":{:02X}{:04X}{:02X}", data.len(), addr, ty));
            for d in data {
                sum = sum.wrapping_add(*d);
                out.push_str(&format!("{d:02X}"));
            }
            out.push_str(&format!("{:02X}\n", sum.wrapping_neg()));
        };
        for s in &self.flash {
            for (i, chunk) in s.data.chunks(16).enumerate() {
                let a = s.addr + (i * 16) as u32;
                if a >> 16 != cur_hi {
                    cur_hi = a >> 16;
                    rec(&mut out, 0, 4, &(cur_hi as u16).to_be_bytes());
                }
                rec(&mut out, a as u16, 0, chunk);
            }
        }
        rec(&mut out, 0, 1, &[]);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip() {
        let img = FirmwareImage {
            flash: vec![
                Segment {
                    addr: 0,
                    data: (0..40u8).collect(),
                },
                Segment {
                    addr: 0x1_0000,
                    data: vec![1, 2, 3],
                },
            ],
            eeprom: vec![],
            entry: 0,
        };
        let txt = img.to_hex();
        let back = FirmwareImage::from_hex(&txt).unwrap();
        assert_eq!(back.flash, img.flash);
    }

    #[test]
    fn hex_errors() {
        assert!(matches!(
            FirmwareImage::from_hex(":0100000001FF\n"),
            Err(LoadError::Hex { line: 1, .. })
        ));
        assert!(matches!(
            FirmwareImage::from_hex("hello"),
            Err(LoadError::Hex { .. })
        ));
        assert_eq!(
            FirmwareImage::from_elf(b"nope"),
            Err(LoadError::Elf("not an ELF file".into()))
        );
    }
}
