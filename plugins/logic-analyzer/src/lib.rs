//! 16-channel logic analyzer with protocol decoders (PLUG-5): UART, I²C, SPI, 1-Wire.
//!
//! Inputs are high-impedance digital probes (no circuit load modelled: 1 MΩ class); every
//! edge is timestamped exactly by the bench (`bb_watch` → `on_edge`). Decoders run on the
//! recorded edges; their output shows as annotations on the screen and as decoded text.
//!
//! Decoders are configured with the part's `decoders` property, e.g.
//! `uart:D0@9600, i2c:D2/D3, spi:D4/D5/D6/D7, onewire:D8`
//! (UART `ch@baud`; I²C `SDA/SCL`; SPI `SCK/MOSI/MISO/CS`; 1-Wire `DQ`).

use bb_plugin_sdk::prelude::*;

const N: usize = 16;
const MAX_EDGES: usize = 400_000;

#[derive(Clone, Debug)]
enum Decoder {
    Uart {
        ch: usize,
        baud: f64,
    },
    I2c {
        sda: usize,
        scl: usize,
    },
    Spi {
        sck: usize,
        mosi: usize,
        miso: Option<usize>,
        cs: Option<usize>,
    },
    OneWire {
        dq: usize,
    },
}

/// One decoded item.
#[derive(Clone, Debug)]
struct Frame {
    t: f64,
    proto: &'static str,
    text: String,
    byte: Option<u8>,
}

struct La {
    pins: Vec<usize>,
    /// Channel → pin index.
    level: [bool; N],
    /// (time, channel, level).
    edges: Vec<(f64, u8, bool)>,
    decoders: Vec<Decoder>,
    /// Per decoder: frames decoded so far and the edge index it reached.
    frames: Vec<Vec<Frame>>,
    cursor: Vec<usize>,
    /// Per decoder: every channel's level just before `cursor`.
    lv: Vec<[bool; N]>,
    /// Decoder states.
    uart_state: Vec<UartState>,
    i2c_state: Vec<I2cState>,
    spi_state: Vec<SpiState>,
    ow_state: Vec<OwState>,
    window: f64,
    threshold: f64,
}

#[derive(Clone, Default)]
struct UartState {
    busy_until: f64,
}

#[derive(Clone, Default)]
struct I2cState {
    active: bool,
    bits: Vec<bool>,
    first: bool,
    read: bool,
}

#[derive(Clone, Default)]
struct SpiState {
    mosi: u16,
    miso: u16,
    n: u8,
}

#[derive(Clone, Default)]
struct OwState {
    fall: Option<f64>,
    bits: Vec<bool>,
}

fn parse_ch(s: &str) -> Option<usize> {
    let s = s.trim();
    let s = s
        .strip_prefix('D')
        .or_else(|| s.strip_prefix('d'))
        .unwrap_or(s);
    s.parse::<usize>().ok().filter(|c| *c < N)
}

fn parse_decoders(spec: &str) -> Vec<Decoder> {
    let mut v = Vec::new();
    for item in spec.split([',', ';']) {
        let item = item.trim();
        let Some((kind, args)) = item.split_once(':') else {
            continue;
        };
        let parts: Vec<&str> = args.split('/').collect();
        match kind.trim().to_ascii_lowercase().as_str() {
            "uart" => {
                let (ch, baud) = match args.split_once('@') {
                    Some((c, b)) => (parse_ch(c), b.trim().parse().unwrap_or(9600.0)),
                    None => (parse_ch(args), 9600.0),
                };
                if let Some(ch) = ch {
                    v.push(Decoder::Uart { ch, baud });
                }
            }
            "i2c" => {
                if let (Some(sda), Some(scl)) = (
                    parts.first().and_then(|x| parse_ch(x)),
                    parts.get(1).and_then(|x| parse_ch(x)),
                ) {
                    v.push(Decoder::I2c { sda, scl });
                }
            }
            "spi" => {
                if let (Some(sck), Some(mosi)) = (
                    parts.first().and_then(|x| parse_ch(x)),
                    parts.get(1).and_then(|x| parse_ch(x)),
                ) {
                    v.push(Decoder::Spi {
                        sck,
                        mosi,
                        miso: parts.get(2).and_then(|x| parse_ch(x)),
                        cs: parts.get(3).and_then(|x| parse_ch(x)),
                    });
                }
            }
            "onewire" | "1wire" | "1-wire" => {
                if let Some(dq) = parse_ch(args) {
                    v.push(Decoder::OneWire { dq });
                }
            }
            _ => {}
        }
    }
    v
}

fn printable(b: u8) -> String {
    if (0x20..0x7f).contains(&b) {
        format!("'{}'", b as char)
    } else {
        match b {
            b'\r' => "CR".into(),
            b'\n' => "LF".into(),
            _ => format!("0x{b:02X}"),
        }
    }
}

impl La {
    /// Level of channel `ch` at time `t` (from the edge list).
    fn level_at(&self, ch: usize, t: f64) -> bool {
        let mut lvl = true; // idle high for UART/I²C lines until the first edge says otherwise
        let mut seen = false;
        for &(et, c, l) in self.edges.iter().rev() {
            if c as usize == ch && et <= t {
                return l;
            }
            if c as usize == ch {
                lvl = !l;
                seen = true;
            }
        }
        if seen { lvl } else { self.level[ch] }
    }

    fn decode(&mut self, now: f64) {
        for di in 0..self.decoders.len() {
            let d = self.decoders[di].clone();
            match d {
                Decoder::Uart { ch, baud } => self.decode_uart(di, ch, baud, now),
                Decoder::I2c { sda, scl } => self.decode_i2c(di, sda, scl),
                Decoder::Spi {
                    sck,
                    mosi,
                    miso,
                    cs,
                } => self.decode_spi(di, sck, mosi, miso, cs),
                Decoder::OneWire { dq } => self.decode_ow(di, dq),
            }
        }
    }

    fn decode_uart(&mut self, di: usize, ch: usize, baud: f64, now: f64) {
        let bit = 1.0 / baud;
        let mut i = self.cursor[di];
        while i < self.edges.len() {
            let (t, c, l) = self.edges[i];
            if c as usize != ch || l || t < self.uart_state[di].busy_until {
                i += 1;
                continue;
            }
            // falling edge = start bit; sample mid-bit by scanning forward
            let stop_t = t + 9.5 * bit;
            if stop_t > now {
                break; // frame not complete yet
            }
            let edges = &self.edges;
            let sample = |tt: f64| {
                let mut lvl = false;
                for &(t2, c2, l2) in &edges[i + 1..] {
                    if t2 > tt {
                        break;
                    }
                    if c2 as usize == ch {
                        lvl = l2;
                    }
                }
                lvl
            };
            let mut byte = 0u8;
            for b in 0..8 {
                if sample(t + (1.5 + b as f64) * bit) {
                    byte |= 1 << b;
                }
            }
            let framing_ok = sample(stop_t);
            self.frames[di].push(Frame {
                t,
                proto: "UART",
                text: if framing_ok {
                    printable(byte)
                } else {
                    format!("{} (framing error)", printable(byte))
                },
                byte: Some(byte),
            });
            self.uart_state[di].busy_until = t + 9.75 * bit;
            i += 1;
        }
        self.cursor[di] = i;
    }

    fn decode_i2c(&mut self, di: usize, sda: usize, scl: usize) {
        let mut i = self.cursor[di];
        while i < self.edges.len() {
            let (t, c, l) = self.edges[i];
            let c = c as usize;
            let scl_high = self.lv[di][scl];
            if c == sda && scl_high {
                let st = &mut self.i2c_state[di];
                if !l {
                    st.active = true;
                    st.bits.clear();
                    st.first = true;
                    self.frames[di].push(Frame {
                        t,
                        proto: "I2C",
                        text: "START".into(),
                        byte: None,
                    });
                } else if st.active {
                    st.active = false;
                    self.frames[di].push(Frame {
                        t,
                        proto: "I2C",
                        text: "STOP".into(),
                        byte: None,
                    });
                }
            } else if c == scl && l && self.i2c_state[di].active {
                let bit = self.lv[di][sda];
                let st = &mut self.i2c_state[di];
                st.bits.push(bit);
                if st.bits.len() == 9 {
                    let mut byte = 0u8;
                    for b in &st.bits[..8] {
                        byte = (byte << 1) | *b as u8;
                    }
                    let ack = !st.bits[8];
                    let text = if st.first {
                        st.read = byte & 1 == 1;
                        format!(
                            "addr 0x{:02X} {} {}",
                            byte >> 1,
                            if st.read { "R" } else { "W" },
                            if ack { "ACK" } else { "NACK" }
                        )
                    } else {
                        format!("0x{byte:02X} {}", if ack { "ACK" } else { "NACK" })
                    };
                    st.first = false;
                    st.bits.clear();
                    self.frames[di].push(Frame {
                        t,
                        proto: "I2C",
                        text,
                        byte: Some(byte),
                    });
                }
            }
            self.lv[di][c] = l;
            i += 1;
        }
        self.cursor[di] = i;
    }

    fn decode_spi(
        &mut self,
        di: usize,
        sck: usize,
        mosi: usize,
        miso: Option<usize>,
        cs: Option<usize>,
    ) {
        let mut i = self.cursor[di];
        while i < self.edges.len() {
            let (t, c, l) = self.edges[i];
            let c = c as usize;
            if Some(c) == cs && l {
                self.spi_state[di] = SpiState::default();
            }
            let selected = cs.map(|p| !self.lv[di][p]).unwrap_or(true);
            if c == sck && l && selected {
                let mo = self.lv[di][mosi];
                let mi = miso.map(|p| self.lv[di][p]).unwrap_or(false);
                let st = &mut self.spi_state[di];
                st.mosi = (st.mosi << 1) | mo as u16;
                st.miso = (st.miso << 1) | mi as u16;
                st.n += 1;
                if st.n == 8 {
                    let (o, ii) = (st.mosi as u8, st.miso as u8);
                    *st = SpiState::default();
                    self.frames[di].push(Frame {
                        t,
                        proto: "SPI",
                        text: if miso.is_some() {
                            format!("MOSI 0x{o:02X} · MISO 0x{ii:02X}")
                        } else {
                            format!("MOSI 0x{o:02X}")
                        },
                        byte: Some(o),
                    });
                }
            }
            self.lv[di][c] = l;
            i += 1;
        }
        self.cursor[di] = i;
    }

    fn decode_ow(&mut self, di: usize, dq: usize) {
        let mut i = self.cursor[di];
        while i < self.edges.len() {
            let (t, c, l) = self.edges[i];
            if c as usize == dq {
                if !l {
                    self.ow_state[di].fall = Some(t);
                } else if let Some(f) = self.ow_state[di].fall.take() {
                    let w = t - f;
                    if w > 400e-6 {
                        self.ow_state[di].bits.clear();
                        self.frames[di].push(Frame {
                            t: f,
                            proto: "1-Wire",
                            text: "RESET".into(),
                            byte: None,
                        });
                    } else if w > 50e-6 && w < 300e-6 {
                        // presence pulse from a device is reported as part of the reset
                        if let Some(last) = self.frames[di].last_mut()
                            && last.text == "RESET"
                        {
                            last.text = "RESET · presence".into();
                        } else {
                            self.ow_state[di].bits.push(false);
                        }
                    } else {
                        // write-1 / read-1: short low; write-0: long low (≥ 15 µs)
                        self.ow_state[di].bits.push(w < 15e-6);
                    }
                    if self.ow_state[di].bits.len() == 8 {
                        let mut byte = 0u8;
                        for (k, b) in self.ow_state[di].bits.iter().enumerate() {
                            if *b {
                                byte |= 1 << k;
                            }
                        }
                        self.ow_state[di].bits.clear();
                        self.frames[di].push(Frame {
                            t: f,
                            proto: "1-Wire",
                            text: format!("0x{byte:02X}"),
                            byte: Some(byte),
                        });
                    }
                }
            }
            i += 1;
        }
        self.cursor[di] = i;
    }

    fn publish(&self, ctx: &Ctx) {
        let now = ctx.now();
        let t0 = (now - self.window).max(0.0);
        let mut screen = ScreenState::default();
        let x = |t: f64| ((t - t0) / self.window * 10.0).clamp(0.0, 10.0) as f32;
        for ch in 0..N {
            let mut pts = Vec::new();
            let mut lvl = self.level_at(ch, t0);
            let row = ch as f32;
            let y = |h: bool| row + if h { 0.7 } else { 0.0 };
            pts.push([0.0, y(lvl)]);
            for &(t, c, l) in self.edges.iter().filter(|e| e.1 as usize == ch && e.0 > t0) {
                pts.push([x(t), y(lvl)]);
                pts.push([x(t), y(l)]);
                lvl = l;
                let _ = c;
            }
            pts.push([10.0, y(lvl)]);
            if pts.len() > 2 || ch < 4 {
                screen.traces.push(Trace {
                    channel: format!("D{ch}"),
                    points: pts,
                    digital: true,
                });
            }
        }
        let mut readouts = Vec::new();
        for (di, d) in self.decoders.iter().enumerate() {
            let fr = &self.frames[di];
            for f in fr.iter().filter(|f| f.t >= t0).rev().take(64) {
                screen.markers.push(Marker {
                    x: x(f.t),
                    y: 0.0,
                    text: f.text.clone(),
                    channel: f.proto.into(),
                });
            }
            let label = match d {
                Decoder::Uart { ch, baud } => format!("UART D{ch} @ {baud}"),
                Decoder::I2c { sda, scl } => format!("I2C D{sda}/D{scl}"),
                Decoder::Spi { sck, mosi, .. } => format!("SPI D{sck}/D{mosi}"),
                Decoder::OneWire { dq } => format!("1-Wire D{dq}"),
            };
            let text: String = match d {
                Decoder::Uart { .. } => {
                    let bytes: Vec<u8> = fr.iter().filter_map(|f| f.byte).collect();
                    let s = String::from_utf8_lossy(&bytes).into_owned();
                    let n = s.chars().count();
                    s.chars().skip(n.saturating_sub(512)).collect()
                }
                _ => fr
                    .iter()
                    .rev()
                    .take(24)
                    .rev()
                    .map(|f| f.text.clone())
                    .collect::<Vec<_>>()
                    .join(" · "),
            };
            readouts.push((label, text));
        }
        let p = PanelState {
            screens: [("screen".to_string(), screen)].into_iter().collect(),
            readouts,
            ..Default::default()
        };
        ctx.set_panel(&p);
        let total: usize = self.frames.iter().map(|f| f.len()).sum();
        ctx.set_readout(&format!("{} edges · {} decoded", self.edges.len(), total));
        ctx.set_readings(&[
            ("edges", self.edges.len() as f64),
            ("decoded", total as f64),
            ("window", self.window),
        ]);
    }
}

impl Part for La {
    fn init(cfg: &Config) -> Result<Self, String> {
        let mut pins = Vec::new();
        for c in 0..N {
            pins.push(
                cfg.pin(&format!("D{c}"))
                    .ok_or_else(|| format!("no D{c} pin"))?,
            );
        }
        let part = cfg.part.clone().unwrap_or_default();
        let spec = part
            .props
            .get("decoders")
            .and_then(|v| {
                v.as_str()
                    .map(String::from)
                    .or_else(|| v.get("Text").and_then(|t| t.as_str()).map(String::from))
            })
            .unwrap_or_default();
        let decoders = parse_decoders(&spec);
        let n = decoders.len();
        Ok(La {
            pins,
            level: [false; N],
            edges: Vec::new(),
            frames: vec![Vec::new(); n],
            cursor: vec![0; n],
            lv: vec![[true; N]; n],
            uart_state: vec![UartState::default(); n],
            i2c_state: vec![I2cState::default(); n],
            spi_state: vec![SpiState::default(); n],
            ow_state: vec![OwState::default(); n],
            decoders,
            window: part.prop("window").unwrap_or(0.02),
            threshold: part.prop("threshold").unwrap_or(1.4),
        })
    }

    fn on_start(&mut self, ctx: &Ctx) {
        let th = self.threshold;
        for (c, &p) in self.pins.iter().enumerate() {
            self.level[c] = ctx.pin_voltage(p) > th;
            ctx.watch(p, th - 0.2, th + 0.2);
        }
        ctx.tick_every(1.0 / 30.0);
        self.publish(ctx);
    }

    fn on_edge(&mut self, ctx: &Ctx, pin: usize, high: bool) {
        if let Some(c) = self.pins.iter().position(|p| *p == pin) {
            self.level[c] = high;
            self.edges.push((ctx.now(), c as u8, high));
            if self.edges.len() > MAX_EDGES {
                let cut = MAX_EDGES / 2;
                self.edges.drain(..cut);
                for k in self.cursor.iter_mut() {
                    *k = k.saturating_sub(cut);
                }
            }
        }
    }

    fn on_tick(&mut self, ctx: &Ctx) {
        self.decode(ctx.now());
        self.publish(ctx);
    }

    fn on_input(&mut self, ctx: &Ctx, input: &Input) {
        if let Input::Value { key, value } = input {
            match key.as_str() {
                "window" => self.window = value.clamp(1e-6, 10.0),
                "clear" if *value > 0.5 => {
                    self.edges.clear();
                    for f in self.frames.iter_mut() {
                        f.clear();
                    }
                    for k in self.cursor.iter_mut() {
                        *k = 0;
                    }
                }
                _ => {}
            }
            self.publish(ctx);
        }
    }

    fn set_param(&mut self, ctx: &Ctx, key: &str, value: f64) -> bool {
        match key {
            "window" => {
                self.window = value.clamp(1e-6, 10.0);
                self.publish(ctx);
                true
            }
            _ => false,
        }
    }
}

export_part!(La);
