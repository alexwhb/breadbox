//! Breadboard hole naming, geometry and internal connectivity.
//!
//! Holes are the breadboard part's pins: terminal strips `a1`…`j63` (a–e and f–j of one
//! column are connected), and four power rails `t+N`, `t-N` (top) and `b+N`, `b-N` (bottom),
//! each rail fully connected (or split in the middle with the `split_rails` property).
//!
//! Geometry (0.1" units, y down): top rails at y = 1 (`+`, red) and 2 (`-`); rows a–e at
//! y = 4…8; the centre channel; rows f–j at y = 11…15; bottom rails at y = 17 (`+`) and
//! 18 (`-`, blue). Column `c` sits at x = c. Rail holes come in groups of five with a one-pitch
//! gap between groups, centred on the board.

use crate::part::{Lead, PinDef, PinRole};

pub const ROWS: [char; 10] = ['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j'];

/// One rail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rail {
    TopPlus,
    TopMinus,
    BottomPlus,
    BottomMinus,
}

impl Rail {
    pub const ALL: [Rail; 4] = [
        Rail::TopPlus,
        Rail::TopMinus,
        Rail::BottomPlus,
        Rail::BottomMinus,
    ];
    pub fn prefix(self) -> &'static str {
        match self {
            Rail::TopPlus => "t+",
            Rail::TopMinus => "t-",
            Rail::BottomPlus => "b+",
            Rail::BottomMinus => "b-",
        }
    }
    pub fn y(self) -> f64 {
        match self {
            Rail::TopPlus => 1.0,
            Rail::TopMinus => 2.0,
            Rail::BottomPlus => 17.0,
            Rail::BottomMinus => 18.0,
        }
    }
    pub fn is_plus(self) -> bool {
        matches!(self, Rail::TopPlus | Rail::BottomPlus)
    }
}

/// Y of a terminal-strip row letter.
pub fn row_y(row: char) -> Option<f64> {
    let i = ROWS.iter().position(|&r| r == row)? as f64;
    Some(if i < 5.0 { 4.0 + i } else { 6.0 + i })
}

/// X of rail hole `k` (1-based).
pub fn rail_x(columns: u32, rail_holes: u32, k: u32) -> f64 {
    let groups = rail_holes.div_ceil(5);
    let span = (rail_holes - 1) + (groups - 1);
    let start = 1 + (columns.saturating_sub(1).saturating_sub(span)) / 2;
    let i = k - 1;
    (start + i + i / 5) as f64
}

/// Body size of a breadboard.
pub fn size(columns: u32) -> (f64, f64) {
    (columns as f64 + 1.0, 19.0)
}

/// All holes as pins + leads.
pub fn holes(columns: u32, rail_holes: u32) -> (Vec<PinDef>, Vec<Lead>) {
    let mut pins = Vec::with_capacity((columns * 10 + rail_holes * 4) as usize);
    let mut leads = Vec::with_capacity(pins.capacity());
    for c in 1..=columns {
        for r in ROWS {
            let id = format!("{r}{c}");
            leads.push(Lead {
                pin: id.clone(),
                x: c as f64,
                y: row_y(r).unwrap_or(0.0),
            });
            pins.push(PinDef::new(&id, PinRole::Passive));
        }
    }
    for rail in Rail::ALL {
        for k in 1..=rail_holes {
            let id = format!("{}{k}", rail.prefix());
            leads.push(Lead {
                pin: id.clone(),
                x: rail_x(columns, rail_holes, k),
                y: rail.y(),
            });
            pins.push(PinDef::new(&id, PinRole::Passive));
        }
    }
    (pins, leads)
}

/// Parses a hole name: `Strip(column, top_half)` or `Rail(rail, index)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hole {
    Strip { row: char, column: u32 },
    Rail { rail: Rail, index: u32 },
}

pub fn parse_hole(name: &str) -> Option<Hole> {
    for rail in Rail::ALL {
        if let Some(n) = name.strip_prefix(rail.prefix()) {
            return n.parse().ok().map(|index| Hole::Rail { rail, index });
        }
    }
    let mut ch = name.chars();
    let row = ch.next()?;
    if !ROWS.contains(&row) {
        return None;
    }
    ch.as_str()
        .parse()
        .ok()
        .map(|column| Hole::Strip { row, column })
}

/// Key of the internal strip a hole belongs to (holes with equal keys are connected).
pub fn strip_key(hole: &Hole, rail_holes: u32, split_rails: bool) -> String {
    match *hole {
        Hole::Strip { row, column } => {
            let half = if ROWS.iter().position(|&r| r == row).unwrap_or(0) < 5 {
                "top"
            } else {
                "bot"
            };
            format!("col{column}{half}")
        }
        Hole::Rail { rail, index } => {
            let part = if split_rails && index > rail_holes / 2 {
                "R"
            } else {
                "L"
            };
            format!("rail{}{part}", rail.prefix())
        }
    }
}

/// Human description of a strip for hover text ("column 12, rows a–e", "top + rail").
pub fn describe(hole: &Hole) -> String {
    match *hole {
        Hole::Strip { row, column } => {
            let half = if ROWS.iter().position(|&r| r == row).unwrap_or(0) < 5 {
                "a–e"
            } else {
                "f–j"
            };
            format!("column {column}, rows {half}")
        }
        Hole::Rail { rail, .. } => match rail {
            Rail::TopPlus => "top + rail".into(),
            Rail::TopMinus => "top − rail".into(),
            Rail::BottomPlus => "bottom + rail".into(),
            Rail::BottomMinus => "bottom − rail".into(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hole_counts_and_names() {
        let (p, l) = holes(63, 50);
        assert_eq!(p.len(), 830);
        assert_eq!(l.len(), 830);
        let (p4, _) = holes(30, 25);
        assert_eq!(p4.len(), 400);
        assert_eq!(
            parse_hole("a1"),
            Some(Hole::Strip {
                row: 'a',
                column: 1
            })
        );
        assert_eq!(
            parse_hole("j63"),
            Some(Hole::Strip {
                row: 'j',
                column: 63
            })
        );
        assert_eq!(
            parse_hole("t+7"),
            Some(Hole::Rail {
                rail: Rail::TopPlus,
                index: 7
            })
        );
        assert_eq!(parse_hole("k1"), None);
        // rail holes stay on the board and groups of five have gaps
        assert_eq!(rail_x(63, 50, 1), 3.0);
        assert_eq!(rail_x(63, 50, 5), 7.0);
        assert_eq!(rail_x(63, 50, 6), 9.0);
        assert_eq!(rail_x(63, 50, 50), 61.0);
        assert!(rail_x(30, 25, 25) <= 30.0);
    }

    #[test]
    fn strips_connect_the_right_holes() {
        let k = |n: &str| strip_key(&parse_hole(n).unwrap(), 50, false);
        assert_eq!(k("a5"), k("e5"));
        assert_ne!(k("e5"), k("f5"));
        assert_eq!(k("f5"), k("j5"));
        assert_ne!(k("a5"), k("a6"));
        assert_eq!(k("t+1"), k("t+50"));
        assert_ne!(k("t+1"), k("t-1"));
        assert_ne!(k("t+1"), k("b+1"));
        let ks = |n: &str| strip_key(&parse_hole(n).unwrap(), 50, true);
        assert_ne!(ks("t+1"), ks("t+50"));
        assert_eq!(ks("t+26"), ks("t+50"));
    }
}
