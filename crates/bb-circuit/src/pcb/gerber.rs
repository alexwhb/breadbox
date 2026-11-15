//! Gerber RS-274X + Excellon export (`PRO-7`, `PRJ-5` Gerbers) and a small Gerber /
//! Excellon reader used to validate what was written (apertures defined before use,
//! coordinates inside the board, flash counts).
//!
//! Files (KiCad naming): `<name>-F_Cu.gbr`, `-B_Cu.gbr`, `-F_Mask.gbr`, `-B_Mask.gbr`,
//! `-F_SilkS.gbr`, `-Edge_Cuts.gbr`, `<name>.drl`. Units mm, format 4.6, absolute, Y up with the
//! board's bottom-left corner at the origin.

use super::footprint::PadShape;
use super::{Board, Layer, Placed, WorldPad};
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// Solder-mask opening around pads (mm).
pub const MASK_EXPANSION: f64 = 0.05;
/// Silkscreen line width (mm).
pub const SILK_WIDTH: f64 = 0.15;
/// Height of reference designators on the silkscreen (mm).
pub const REF_HEIGHT: f64 = 1.0;

/// Where a part's reference designator goes: top-left of its courtyard (where the PCB screen
/// shows it), board coordinates (mm, y down).
pub fn reference_anchor(pl: &Placed) -> (f64, f64) {
    let bb = pl.bounds();
    (bb.0 + 0.4, bb.1 + 0.2)
}

/// One glyph of the silkscreen stroke font: polylines on a 4 × 6 grid (x right, y down).
fn glyph(c: char) -> &'static [&'static [(f64, f64)]] {
    match c.to_ascii_uppercase() {
        '0' => &[
            &[
                (1.0, 0.0),
                (3.0, 0.0),
                (4.0, 1.0),
                (4.0, 5.0),
                (3.0, 6.0),
                (1.0, 6.0),
                (0.0, 5.0),
                (0.0, 1.0),
                (1.0, 0.0),
            ],
            &[(0.0, 5.0), (4.0, 1.0)],
        ],
        '1' => &[
            &[(1.0, 1.0), (2.0, 0.0), (2.0, 6.0)],
            &[(1.0, 6.0), (3.0, 6.0)],
        ],
        '2' => &[&[
            (0.0, 1.0),
            (1.0, 0.0),
            (3.0, 0.0),
            (4.0, 1.0),
            (4.0, 2.0),
            (0.0, 6.0),
            (4.0, 6.0),
        ]],
        '3' => &[&[
            (0.0, 0.0),
            (4.0, 0.0),
            (2.0, 2.5),
            (3.0, 2.5),
            (4.0, 3.5),
            (4.0, 5.0),
            (3.0, 6.0),
            (1.0, 6.0),
            (0.0, 5.0),
        ]],
        '4' => &[&[(3.0, 6.0), (3.0, 0.0), (0.0, 4.0), (4.0, 4.0)]],
        '5' => &[&[
            (4.0, 0.0),
            (0.0, 0.0),
            (0.0, 2.5),
            (3.0, 2.5),
            (4.0, 3.5),
            (4.0, 5.0),
            (3.0, 6.0),
            (0.0, 6.0),
        ]],
        '6' => &[&[
            (3.0, 0.0),
            (1.0, 0.0),
            (0.0, 1.0),
            (0.0, 5.0),
            (1.0, 6.0),
            (3.0, 6.0),
            (4.0, 5.0),
            (4.0, 3.5),
            (3.0, 2.5),
            (0.0, 2.5),
        ]],
        '7' => &[&[(0.0, 0.0), (4.0, 0.0), (1.5, 6.0)]],
        '8' => &[
            &[
                (1.0, 0.0),
                (3.0, 0.0),
                (4.0, 1.0),
                (4.0, 2.0),
                (3.0, 3.0),
                (1.0, 3.0),
                (0.0, 4.0),
                (0.0, 5.0),
                (1.0, 6.0),
                (3.0, 6.0),
                (4.0, 5.0),
                (4.0, 4.0),
                (3.0, 3.0),
            ],
            &[(1.0, 3.0), (0.0, 2.0), (0.0, 1.0), (1.0, 0.0)],
        ],
        '9' => &[&[
            (4.0, 3.5),
            (1.0, 3.5),
            (0.0, 2.5),
            (0.0, 1.0),
            (1.0, 0.0),
            (3.0, 0.0),
            (4.0, 1.0),
            (4.0, 5.0),
            (3.0, 6.0),
            (1.0, 6.0),
        ]],
        'A' => &[
            &[(0.0, 6.0), (0.0, 2.0), (2.0, 0.0), (4.0, 2.0), (4.0, 6.0)],
            &[(0.0, 3.5), (4.0, 3.5)],
        ],
        'B' => &[&[
            (0.0, 3.0),
            (3.0, 3.0),
            (4.0, 4.0),
            (4.0, 5.0),
            (3.0, 6.0),
            (0.0, 6.0),
            (0.0, 0.0),
            (3.0, 0.0),
            (4.0, 1.0),
            (4.0, 2.0),
            (3.0, 3.0),
        ]],
        'C' => &[&[
            (4.0, 1.0),
            (3.0, 0.0),
            (1.0, 0.0),
            (0.0, 1.0),
            (0.0, 5.0),
            (1.0, 6.0),
            (3.0, 6.0),
            (4.0, 5.0),
        ]],
        'D' => &[&[
            (0.0, 0.0),
            (0.0, 6.0),
            (2.5, 6.0),
            (4.0, 4.5),
            (4.0, 1.5),
            (2.5, 0.0),
            (0.0, 0.0),
        ]],
        'E' => &[
            &[(4.0, 0.0), (0.0, 0.0), (0.0, 6.0), (4.0, 6.0)],
            &[(0.0, 3.0), (3.0, 3.0)],
        ],
        'F' => &[
            &[(4.0, 0.0), (0.0, 0.0), (0.0, 6.0)],
            &[(0.0, 3.0), (3.0, 3.0)],
        ],
        'G' => &[&[
            (4.0, 1.0),
            (3.0, 0.0),
            (1.0, 0.0),
            (0.0, 1.0),
            (0.0, 5.0),
            (1.0, 6.0),
            (3.0, 6.0),
            (4.0, 5.0),
            (4.0, 3.5),
            (2.0, 3.5),
        ]],
        'H' => &[
            &[(0.0, 0.0), (0.0, 6.0)],
            &[(4.0, 0.0), (4.0, 6.0)],
            &[(0.0, 3.0), (4.0, 3.0)],
        ],
        'I' => &[
            &[(1.0, 0.0), (3.0, 0.0)],
            &[(2.0, 0.0), (2.0, 6.0)],
            &[(1.0, 6.0), (3.0, 6.0)],
        ],
        'J' => &[&[(4.0, 0.0), (4.0, 5.0), (3.0, 6.0), (1.0, 6.0), (0.0, 5.0)]],
        'K' => &[
            &[(0.0, 0.0), (0.0, 6.0)],
            &[(4.0, 0.0), (0.0, 4.0)],
            &[(1.5, 2.5), (4.0, 6.0)],
        ],
        'L' => &[&[(0.0, 0.0), (0.0, 6.0), (4.0, 6.0)]],
        'M' => &[&[(0.0, 6.0), (0.0, 0.0), (2.0, 3.0), (4.0, 0.0), (4.0, 6.0)]],
        'N' => &[&[(0.0, 6.0), (0.0, 0.0), (4.0, 6.0), (4.0, 0.0)]],
        'O' => &[&[
            (1.0, 0.0),
            (3.0, 0.0),
            (4.0, 1.0),
            (4.0, 5.0),
            (3.0, 6.0),
            (1.0, 6.0),
            (0.0, 5.0),
            (0.0, 1.0),
            (1.0, 0.0),
        ]],
        'P' => &[&[
            (0.0, 6.0),
            (0.0, 0.0),
            (3.0, 0.0),
            (4.0, 1.0),
            (4.0, 2.0),
            (3.0, 3.0),
            (0.0, 3.0),
        ]],
        'Q' => &[
            &[
                (1.0, 0.0),
                (3.0, 0.0),
                (4.0, 1.0),
                (4.0, 5.0),
                (3.0, 6.0),
                (1.0, 6.0),
                (0.0, 5.0),
                (0.0, 1.0),
                (1.0, 0.0),
            ],
            &[(2.5, 4.5), (4.0, 6.0)],
        ],
        'R' => &[
            &[
                (0.0, 6.0),
                (0.0, 0.0),
                (3.0, 0.0),
                (4.0, 1.0),
                (4.0, 2.0),
                (3.0, 3.0),
                (0.0, 3.0),
            ],
            &[(2.0, 3.0), (4.0, 6.0)],
        ],
        'S' => &[&[
            (4.0, 1.0),
            (3.0, 0.0),
            (1.0, 0.0),
            (0.0, 1.0),
            (0.0, 2.0),
            (1.0, 3.0),
            (3.0, 3.0),
            (4.0, 4.0),
            (4.0, 5.0),
            (3.0, 6.0),
            (1.0, 6.0),
            (0.0, 5.0),
        ]],
        'T' => &[&[(0.0, 0.0), (4.0, 0.0)], &[(2.0, 0.0), (2.0, 6.0)]],
        'U' => &[&[
            (0.0, 0.0),
            (0.0, 5.0),
            (1.0, 6.0),
            (3.0, 6.0),
            (4.0, 5.0),
            (4.0, 0.0),
        ]],
        'V' => &[&[(0.0, 0.0), (2.0, 6.0), (4.0, 0.0)]],
        'W' => &[&[(0.0, 0.0), (1.0, 6.0), (2.0, 3.0), (3.0, 6.0), (4.0, 0.0)]],
        'X' => &[&[(0.0, 0.0), (4.0, 6.0)], &[(4.0, 0.0), (0.0, 6.0)]],
        'Y' => &[
            &[(0.0, 0.0), (2.0, 3.0), (4.0, 0.0)],
            &[(2.0, 3.0), (2.0, 6.0)],
        ],
        'Z' => &[&[(0.0, 0.0), (4.0, 0.0), (0.0, 6.0), (4.0, 6.0)]],
        '-' => &[&[(0.5, 3.0), (3.5, 3.0)]],
        '+' => &[&[(0.5, 3.0), (3.5, 3.0)], &[(2.0, 1.5), (2.0, 4.5)]],
        '.' => &[&[(2.0, 5.6), (2.0, 6.0)]],
        '_' => &[&[(0.0, 6.0), (4.0, 6.0)]],
        '/' => &[&[(0.0, 6.0), (4.0, 0.0)]],
        ' ' => &[],
        // anything else: a question mark
        _ => &[
            &[
                (0.0, 1.0),
                (1.0, 0.0),
                (3.0, 0.0),
                (4.0, 1.0),
                (4.0, 2.0),
                (2.0, 3.5),
                (2.0, 4.5),
            ],
            &[(2.0, 5.6), (2.0, 6.0)],
        ],
    }
}

/// `text` in the silkscreen stroke font: polylines in board coordinates (mm, y down) with the
/// top-left of the first glyph at `(x, y)` and capitals `h` mm tall.
pub fn silk_text(text: &str, x: f64, y: f64, h: f64) -> Vec<Vec<(f64, f64)>> {
    let u = h / 6.0;
    let mut out = Vec::new();
    for (i, c) in text.chars().enumerate() {
        let x0 = x + i as f64 * 5.0 * u;
        for stroke in glyph(c) {
            out.push(
                stroke
                    .iter()
                    .map(|(gx, gy)| (x0 + gx * u, y + gy * u))
                    .collect(),
            );
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
enum Ap {
    Circle(f64),
    Rect(f64, f64),
    Obround(f64, f64),
}

impl Ap {
    fn key(&self) -> String {
        match self {
            Ap::Circle(d) => format!("C,{d:.6}"),
            Ap::Rect(w, h) => format!("R,{w:.6}X{h:.6}"),
            Ap::Obround(w, h) => format!("O,{w:.6}X{h:.6}"),
        }
    }
}

struct Writer {
    origin: (f64, f64),
    height: f64,
    aps: Vec<Ap>,
    body: String,
    current: Option<usize>,
}

impl Writer {
    fn new(outline: (f64, f64, f64, f64)) -> Writer {
        Writer {
            origin: (outline.0, outline.3),
            height: outline.3 - outline.1,
            aps: Vec::new(),
            body: String::new(),
            current: None,
        }
    }
    fn coord(&self, x: f64, y: f64) -> String {
        let gx = ((x - self.origin.0) * 1e6).round() as i64;
        let gy = ((self.origin.1 - y) * 1e6).round() as i64;
        format!("X{gx}Y{gy}")
    }
    fn select(&mut self, ap: Ap) {
        let i = match self.aps.iter().position(|a| a.key() == ap.key()) {
            Some(i) => i,
            None => {
                self.aps.push(ap);
                self.aps.len() - 1
            }
        };
        if self.current != Some(i) {
            let _ = writeln!(self.body, "D{}*", 10 + i);
            self.current = Some(i);
        }
    }
    fn flash(&mut self, ap: Ap, x: f64, y: f64) {
        self.select(ap);
        let c = self.coord(x, y);
        let _ = writeln!(self.body, "{c}D03*");
    }
    fn polyline(&mut self, width: f64, pts: &[(f64, f64)]) {
        if pts.len() < 2 {
            if let Some(p) = pts.first() {
                self.flash(Ap::Circle(width), p.0, p.1);
            }
            return;
        }
        self.select(Ap::Circle(width));
        let c = self.coord(pts[0].0, pts[0].1);
        let _ = writeln!(self.body, "{c}D02*");
        for p in &pts[1..] {
            let c = self.coord(p.0, p.1);
            let _ = writeln!(self.body, "{c}D01*");
        }
    }
    fn finish(self, function: &str) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "G04 Breadbox PCB export*");
        let _ = writeln!(
            s,
            "%TF.GenerationSoftware,Breadbox,{}*%",
            env!("CARGO_PKG_VERSION")
        );
        let _ = writeln!(s, "%TF.FileFunction,{function}*%");
        let _ = writeln!(s, "%FSLAX46Y46*%");
        let _ = writeln!(s, "%MOMM*%");
        let _ = writeln!(s, "%LPD*%");
        for (i, a) in self.aps.iter().enumerate() {
            let _ = writeln!(s, "%ADD{}{}*%", 10 + i, a.key());
        }
        s.push_str(&self.body);
        let _ = writeln!(s, "M02*");
        let _ = self.height;
        s
    }
}

fn pad_ap(p: &WorldPad, grow: f64) -> Ap {
    let (w, h) = (p.w + grow * 2.0, p.h + grow * 2.0);
    match p.shape {
        PadShape::Round => Ap::Circle(w.max(h)),
        PadShape::Rect => Ap::Rect(w, h),
        PadShape::Oval => {
            if (w - h).abs() < 1e-9 {
                Ap::Circle(w)
            } else {
                Ap::Obround(w, h)
            }
        }
    }
}

/// Writes every layer. `name` is the file stem.
pub fn export(name: &str, board: &Board, parts: &[Placed]) -> Vec<(String, String)> {
    let outline = board.outline.unwrap_or((0.0, 0.0, 100.0, 80.0));
    let pads: Vec<WorldPad> = parts.iter().flat_map(|p| p.pads()).collect();
    let mut files = Vec::new();
    for (layer, suffix, func) in [
        (Layer::Top, "F_Cu", "Copper,L1,Top"),
        (Layer::Bottom, "B_Cu", "Copper,L2,Bot"),
    ] {
        let mut w = Writer::new(outline);
        for p in &pads {
            if p.layers()[layer.index()] {
                w.flash(pad_ap(p, 0.0), p.x, p.y);
            }
        }
        for v in &board.vias {
            w.flash(Ap::Circle(v.diameter), v.x, v.y);
        }
        for t in board.traces.iter().filter(|t| t.layer == layer) {
            w.polyline(t.width, &t.points);
        }
        files.push((format!("{name}-{suffix}.gbr"), w.finish(func)));
    }
    for (layer, suffix, func) in [
        (Layer::Top, "F_Mask", "Soldermask,Top"),
        (Layer::Bottom, "B_Mask", "Soldermask,Bot"),
    ] {
        let mut w = Writer::new(outline);
        for p in &pads {
            if p.layers()[layer.index()] {
                w.flash(pad_ap(p, MASK_EXPANSION), p.x, p.y);
            }
        }
        files.push((format!("{name}-{suffix}.gbr"), w.finish(func)));
    }
    {
        let mut w = Writer::new(outline);
        for pl in parts {
            for (a, b) in pl.silk() {
                w.polyline(SILK_WIDTH, &[a, b]);
            }
            // the reference designator, where the PCB screen shows it
            let (x, y) = reference_anchor(pl);
            for stroke in silk_text(&pl.reference, x, y, REF_HEIGHT) {
                w.polyline(SILK_WIDTH, &stroke);
            }
        }
        files.push((format!("{name}-F_SilkS.gbr"), w.finish("Legend,Top")));
    }
    {
        let mut w = Writer::new(outline);
        let (x0, y0, x1, y1) = outline;
        w.polyline(0.1, &[(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)]);
        files.push((format!("{name}-Edge_Cuts.gbr"), w.finish("Profile,NP")));
    }
    files.push((format!("{name}.drl"), excellon(board, &pads, outline)));
    files
}

/// Excellon drill file (plated holes: THT pads and vias).
pub fn excellon(board: &Board, pads: &[WorldPad], outline: (f64, f64, f64, f64)) -> String {
    let mut tools: BTreeMap<String, Vec<(f64, f64)>> = BTreeMap::new();
    for p in pads {
        if let Some(d) = p.drill {
            tools.entry(format!("{d:.3}")).or_default().push((p.x, p.y));
        }
    }
    for v in &board.vias {
        tools
            .entry(format!("{:.3}", v.drill))
            .or_default()
            .push((v.x, v.y));
    }
    let mut s = String::new();
    let _ = writeln!(s, "M48");
    let _ = writeln!(s, "; Breadbox PCB export");
    let _ = writeln!(s, "METRIC,TZ");
    for (i, d) in tools.keys().enumerate() {
        let _ = writeln!(s, "T{}C{}", i + 1, d);
    }
    let _ = writeln!(s, "%");
    let _ = writeln!(s, "G90");
    let _ = writeln!(s, "G05");
    for (i, holes) in tools.values().enumerate() {
        let _ = writeln!(s, "T{}", i + 1);
        for (x, y) in holes {
            let _ = writeln!(s, "X{:.3}Y{:.3}", x - outline.0, outline.3 - y);
        }
    }
    let _ = writeln!(s, "M30");
    s
}

/// A drawn segment: (D-code, from, to) in mm.
pub type Draw = (u32, (f64, f64), (f64, f64));

/// What a Gerber file contains (validation / tests).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Parsed {
    /// D-code → aperture template (`C`, `R`, `O`) and its size parameters.
    pub apertures: BTreeMap<u32, (String, Vec<f64>)>,
    /// (D-code, x, y) in mm.
    pub flashes: Vec<(u32, f64, f64)>,
    /// (D-code, from, to) in mm.
    pub draws: Vec<Draw>,
    pub function: String,
}

/// Parses the subset of RS-274X Breadbox writes, checking that every operation uses a defined
/// aperture and the file ends with `M02`.
pub fn parse(text: &str) -> Result<Parsed, String> {
    let mut out = Parsed::default();
    let mut fmt_ok = false;
    let mut mm = false;
    let mut current: Option<u32> = None;
    let mut pos = (0.0, 0.0);
    let mut ended = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("G04") {
            continue;
        }
        if let Some(rest) = line.strip_prefix('%') {
            let cmd = rest.trim_end_matches('%').trim_end_matches('*');
            if cmd == "FSLAX46Y46" {
                fmt_ok = true;
            } else if cmd == "MOMM" {
                mm = true;
            } else if let Some(f) = cmd.strip_prefix("TF.FileFunction,") {
                out.function = f.to_string();
            } else if let Some(def) = cmd.strip_prefix("ADD") {
                let n: String = def.chars().take_while(|c| c.is_ascii_digit()).collect();
                let code: u32 = n.parse().map_err(|_| format!("bad aperture `{cmd}`"))?;
                let rest = &def[n.len()..];
                let (t, params) = rest
                    .split_once(',')
                    .ok_or_else(|| format!("bad aperture `{cmd}`"))?;
                let ps: Result<Vec<f64>, _> = params.split('X').map(|x| x.parse::<f64>()).collect();
                let ps = ps.map_err(|_| format!("bad aperture size `{cmd}`"))?;
                if code < 10 || ps.iter().any(|v| *v <= 0.0) {
                    return Err(format!("invalid aperture `{cmd}`"));
                }
                out.apertures.insert(code, (t.to_string(), ps));
            }
            continue;
        }
        let stmt = line.trim_end_matches('*');
        if stmt == "M02" {
            ended = true;
            continue;
        }
        if let Some(d) = stmt.strip_prefix('D')
            && let Ok(code) = d.parse::<u32>()
            && code >= 10
        {
            if !out.apertures.contains_key(&code) {
                return Err(format!("D{code} used before it is defined"));
            }
            current = Some(code);
            continue;
        }
        if stmt.starts_with('X') {
            let (xy, op) = stmt.split_at(stmt.rfind('D').ok_or("operation without D code")?);
            let yi = xy.find('Y').ok_or("coordinate without Y")?;
            let x: i64 = xy[1..yi]
                .parse()
                .map_err(|_| format!("bad X in `{stmt}`"))?;
            let y: i64 = xy[yi + 1..]
                .parse()
                .map_err(|_| format!("bad Y in `{stmt}`"))?;
            let p = (x as f64 / 1e6, y as f64 / 1e6);
            let ap = current.ok_or("drawing without an aperture")?;
            match op {
                "D01" => out.draws.push((ap, pos, p)),
                "D02" => {}
                "D03" => out.flashes.push((ap, p.0, p.1)),
                other => return Err(format!("unknown operation {other}")),
            }
            pos = p;
            continue;
        }
        if stmt.starts_with("%TF") || stmt.starts_with("G01") || stmt.starts_with("G75") {
            continue;
        }
        return Err(format!("unexpected `{line}`"));
    }
    if !fmt_ok || !mm {
        return Err("missing %FSLAX46Y46*% / %MOMM*% header".into());
    }
    if !ended {
        return Err("missing M02".into());
    }
    Ok(out)
}

/// Parses an Excellon drill file: (diameter mm, x, y) per hole.
pub fn parse_excellon(text: &str) -> Result<Vec<(f64, f64, f64)>, String> {
    let mut tools: BTreeMap<u32, f64> = BTreeMap::new();
    let mut tool = None;
    let mut holes = Vec::new();
    let mut header = true;
    let mut ended = false;
    for l in text.lines().map(str::trim) {
        if l.is_empty() || l.starts_with(';') {
            continue;
        }
        if header {
            if l == "%" {
                header = false;
            } else if let Some(t) = l.strip_prefix('T') {
                let (n, d) = t.split_once('C').ok_or_else(|| format!("bad tool `{l}`"))?;
                tools.insert(
                    n.parse().map_err(|_| format!("bad tool `{l}`"))?,
                    d.parse().map_err(|_| format!("bad tool `{l}`"))?,
                );
            }
            continue;
        }
        if l == "M30" {
            ended = true;
        } else if let Some(t) = l.strip_prefix('T') {
            let n: u32 = t.parse().map_err(|_| format!("bad tool select `{l}`"))?;
            if !tools.contains_key(&n) {
                return Err(format!("tool T{n} not defined"));
            }
            tool = Some(n);
        } else if let Some(xy) = l.strip_prefix('X') {
            let (x, y) = xy
                .split_once('Y')
                .ok_or_else(|| format!("bad hole `{l}`"))?;
            let d = tools[&tool.ok_or("hole before a tool")?];
            holes.push((
                d,
                x.parse().map_err(|_| format!("bad X `{l}`"))?,
                y.parse().map_err(|_| format!("bad Y `{l}`"))?,
            ));
        }
    }
    if !ended {
        return Err("missing M30".into());
    }
    Ok(holes)
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::*;

    #[test]
    fn the_silkscreen_font_covers_designators() {
        for c in "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-+._/".chars() {
            let s = silk_text(&c.to_string(), 0.0, 0.0, 6.0);
            assert!(!s.is_empty(), "{c}");
            for p in s.iter().flatten() {
                assert!(
                    (0.0..=4.0).contains(&p.0) && (0.0..=6.0).contains(&p.1),
                    "{c}: {p:?}"
                );
            }
            if c != 'O' && c != '?' {
                assert_ne!(glyph(c), glyph('?'), "{c} falls back to ?");
            }
        }
        // glyphs advance 5 units; lowercase draws as capitals
        let r12 = silk_text("r12", 10.0, 20.0, 1.2);
        let max_x = r12.iter().flatten().map(|p| p.0).fold(0.0, f64::max);
        assert!((max_x - (10.0 + 14.0 * 0.2)).abs() < 1e-9, "{max_x}");
        assert_eq!(silk_text("R", 0.0, 0.0, 6.0), silk_text("r", 0.0, 0.0, 6.0));
    }

    #[test]
    fn blink_gerbers_parse_back_with_every_pad_and_hole() {
        let (mut p, lib) = super::super::tests::blink();
        auto_place(&mut p, &lib, true);
        let mut board = Board::load(&p);
        let parts = placed_parts(&p, &lib, false);
        let pads: Vec<WorldPad> = parts.iter().flat_map(|x| x.pads()).collect();
        board.traces.push(Trace {
            id: "t1".into(),
            layer: Layer::Bottom,
            width: 0.4,
            points: vec![(pads[0].x, pads[0].y), (pads[0].x, pads[0].y + 2.0)],
        });
        board.vias.push(Via {
            id: "v1".into(),
            x: pads[0].x,
            y: pads[0].y + 2.0,
            drill: 0.4,
            diameter: 0.8,
        });
        let files = export("blink", &board, &parts);
        let names: Vec<&str> = files.iter().map(|f| f.0.as_str()).collect();
        assert_eq!(
            names,
            [
                "blink-F_Cu.gbr",
                "blink-B_Cu.gbr",
                "blink-F_Mask.gbr",
                "blink-B_Mask.gbr",
                "blink-F_SilkS.gbr",
                "blink-Edge_Cuts.gbr",
                "blink.drl"
            ]
        );
        let (x0, y0, x1, y1) = board.outline.unwrap();
        let (w, h) = (x1 - x0, y1 - y0);
        let tht = pads.iter().filter(|p| p.drill.is_some()).count();
        for (name, text) in &files[..6] {
            let g = parse(text).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(!g.apertures.is_empty(), "{name}");
            for (_, x, y) in &g.flashes {
                assert!(
                    (-0.01..=w + 0.01).contains(x) && (-0.01..=h + 0.01).contains(y),
                    "{name}: {x},{y}"
                );
            }
            if name.ends_with("B_Cu.gbr") {
                assert_eq!(g.flashes.len(), tht + 1, "THT pads + via on the bottom");
                assert_eq!(g.draws.len(), 1);
                assert!(
                    g.apertures
                        .values()
                        .any(|(t, s)| t == "C" && (s[0] - 0.4).abs() < 1e-6)
                );
            }
            if name.ends_with("Edge_Cuts.gbr") {
                assert_eq!(g.draws.len(), 4);
                assert_eq!(g.function, "Profile,NP");
            }
        }
        // the silkscreen carries every reference designator next to its part's outline
        let silk = parse(&files[4].1).unwrap();
        let outlines: usize = parts.iter().map(|p| p.silk().len()).sum();
        let to_gerber = |x: f64, y: f64| (x - x0, y1 - y);
        for pl in &parts {
            let strokes = silk_text(&pl.reference, 0.0, 0.0, REF_HEIGHT);
            let segs: usize = strokes.iter().map(|s| s.len() - 1).sum();
            assert!(segs > 0, "{}", pl.reference);
            let (ax, ay) = reference_anchor(pl);
            let w = pl.reference.len() as f64 * REF_HEIGHT * 5.0 / 6.0;
            let (gx0, gy1) = to_gerber(ax, ay);
            let (gx1, gy0) = to_gerber(ax + w, ay + REF_HEIGHT);
            let inside = silk
                .draws
                .iter()
                .filter(|(_, a, b)| {
                    [a, b].iter().all(|p| {
                        (gx0 - 1e-6..=gx1 + 1e-6).contains(&p.0)
                            && (gy0 - 1e-6..=gy1 + 1e-6).contains(&p.1)
                    })
                })
                .count();
            assert!(
                inside >= segs,
                "{}: {inside} of {segs} strokes",
                pl.reference
            );
        }
        assert!(
            silk.draws.len() > outlines,
            "references drawn besides the outlines"
        );
        let holes = parse_excellon(&files[6].1).unwrap();
        assert_eq!(holes.len(), tht + 1);
        assert!(holes.iter().any(|h| (h.0 - 0.4).abs() < 1e-9));
        // a hand-broken file is rejected
        assert!(parse("%FSLAX46Y46*%\n%MOMM*%\nD11*\nM02*\n").is_err());
        assert!(parse("%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,0.25*%\nD10*\nX1Y1D01*\n").is_err());
    }
}
