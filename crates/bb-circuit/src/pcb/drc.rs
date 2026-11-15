//! PCB design-rule check (`PRO-7`): copper clearance between different nets, shorts (one
//! copper group touching two nets), minimum trace width, minimum via drill, copper too close
//! to the board edge, and unrouted nets.

use super::{Board, Copper, ItemKind};
use serde::{Deserialize, Serialize};

/// What a finding is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DrcKind {
    Clearance,
    Short,
    Width,
    Drill,
    Edge,
    Unrouted,
}

/// One DRC finding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DrcIssue {
    pub kind: DrcKind,
    pub message: String,
    /// Where to look (mm).
    pub at: (f64, f64),
}

fn item_name(cu: &Copper, k: &ItemKind) -> String {
    match k {
        ItemKind::Pad(i) => {
            let p = &cu.pads[*i];
            format!("{}.{}", p.reference, p.pin)
        }
        ItemKind::Trace(id, _) => format!("trace {id}"),
        ItemKind::Via(id) => format!("via {id}"),
    }
}

/// Runs every rule.
pub fn check(board: &Board, cu: &Copper) -> Vec<DrcIssue> {
    let r = board.rules;
    let mut out = Vec::new();
    let nets = cu.group_nets();
    // shorts
    let mut shorted: Vec<usize> = Vec::new();
    for (g, ns) in &nets {
        if ns.len() > 1 {
            shorted.push(*g);
            let at = cu
                .items
                .iter()
                .enumerate()
                .find(|(i, _)| cu.group[*i] == *g)
                .map(|(_, it)| it.shape.a)
                .unwrap_or_default();
            out.push(DrcIssue {
                kind: DrcKind::Short,
                message: format!("Short between nets {}", ns.join(" and ")),
                at,
            });
        }
    }
    // clearance between different copper groups on a shared layer
    let n = cu.items.len();
    for i in 0..n {
        for j in i + 1..n {
            let (a, b) = (&cu.items[i], &cu.items[j]);
            if cu.group[i] == cu.group[j] {
                continue;
            }
            let shared = (a.layers[0] && b.layers[0]) || (a.layers[1] && b.layers[1]);
            if !shared {
                continue;
            }
            // two pads of one footprint that are both unconnected (NC pins) don't matter
            let gap = a.shape.gap(&b.shape);
            if gap < r.clearance - 1e-6 {
                let at = (
                    (a.shape.a.0 + b.shape.a.0) / 2.0,
                    (a.shape.a.1 + b.shape.a.1) / 2.0,
                );
                // pads of the same part are spaced by the part itself
                if let (ItemKind::Pad(x), ItemKind::Pad(y)) = (&a.kind, &b.kind)
                    && cu.pads[*x].part == cu.pads[*y].part
                {
                    continue;
                }
                out.push(DrcIssue {
                    kind: DrcKind::Clearance,
                    message: format!(
                        "Clearance {:.2} mm < {:.2} mm between {} and {}",
                        gap.max(0.0),
                        r.clearance,
                        item_name(cu, &a.kind),
                        item_name(cu, &b.kind)
                    ),
                    at,
                });
            }
        }
    }
    // widths, drills
    for t in &board.traces {
        if t.width < r.min_width - 1e-9 {
            out.push(DrcIssue {
                kind: DrcKind::Width,
                message: format!(
                    "Trace {} is {:.2} mm wide (minimum {:.2} mm)",
                    t.id, t.width, r.min_width
                ),
                at: t.points.first().copied().unwrap_or_default(),
            });
        }
    }
    for v in &board.vias {
        if v.drill < r.min_drill - 1e-9 {
            out.push(DrcIssue {
                kind: DrcKind::Drill,
                message: format!(
                    "Via {} drill {:.2} mm (minimum {:.2} mm)",
                    v.id, v.drill, r.min_drill
                ),
                at: (v.x, v.y),
            });
        }
    }
    // board edge
    if let Some((x0, y0, x1, y1)) = board.outline {
        for it in &cu.items {
            let s = &it.shape;
            let lo_x = s.a.0.min(s.b.0) - s.r;
            let hi_x = s.a.0.max(s.b.0) + s.r;
            let lo_y = s.a.1.min(s.b.1) - s.r;
            let hi_y = s.a.1.max(s.b.1) + s.r;
            let e = r.edge_clearance;
            if lo_x < x0 + e || hi_x > x1 - e || lo_y < y0 + e || hi_y > y1 - e {
                out.push(DrcIssue {
                    kind: DrcKind::Edge,
                    message: format!("{} is too close to the board edge", item_name(cu, &it.kind)),
                    at: s.a,
                });
            }
        }
    }
    // unrouted
    for (net, a, b) in cu.ratsnest() {
        let (pa, pb) = (&cu.pads[a], &cu.pads[b]);
        out.push(DrcIssue {
            kind: DrcKind::Unrouted,
            message: format!(
                "Unrouted net {net}: {}.{} – {}.{}",
                pa.reference, pa.pin, pb.reference, pb.pin
            ),
            at: (pa.x, pa.y),
        });
    }
    out
}

/// `DRC clean` / `DRC 2 unrouted · 1 clearance`.
pub fn summary(issues: &[DrcIssue]) -> String {
    if issues.is_empty() {
        return "DRC clean".into();
    }
    let mut parts = Vec::new();
    for (k, name) in [
        (DrcKind::Unrouted, "unrouted"),
        (DrcKind::Short, "short"),
        (DrcKind::Clearance, "clearance"),
        (DrcKind::Width, "width"),
        (DrcKind::Drill, "drill"),
        (DrcKind::Edge, "edge"),
    ] {
        let n = issues.iter().filter(|i| i.kind == k).count();
        if n > 0 {
            parts.push(format!("{n} {name}"));
        }
    }
    format!("DRC {}", parts.join(" · "))
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::*;
    use crate::connectivity::Connectivity;

    #[test]
    fn unrouted_until_routed_then_width_and_clearance_rules() {
        let (mut p, lib) = super::super::tests::blink();
        auto_place(&mut p, &lib, true);
        let conn = Connectivity::compute(&p, &lib);
        let mut board = Board::load(&p);
        let cu = Copper::build(&p, &lib, &board, &conn);
        let issues = check(&board, &cu);
        assert!(
            issues.iter().any(|i| i.kind == DrcKind::Unrouted),
            "{issues:?}"
        );
        assert!(summary(&issues).contains("unrouted"));
        // a too-thin trace far from everything
        let (x0, y0, x1, _) = board.outline.unwrap();
        board.traces.push(Trace {
            id: "t1".into(),
            layer: Layer::Top,
            width: 0.1,
            points: vec![(x0 + 2.0, y0 + 1.0), ((x0 + x1) / 2.0, y0 + 1.0)],
        });
        let cu = Copper::build(&p, &lib, &board, &conn);
        let issues = check(&board, &cu);
        assert!(issues.iter().any(|i| i.kind == DrcKind::Width));
        // a trace dragged straight through another net's pad → short / clearance
        let pads = &cu.pads;
        let (a, b) = (0..pads.len())
            .flat_map(|i| (0..pads.len()).map(move |j| (i, j)))
            .find(|(i, j)| {
                cu.pad_net[*i].is_some()
                    && cu.pad_net[*j].is_some()
                    && cu.pad_net[*i] != cu.pad_net[*j]
            })
            .unwrap();
        board.traces.push(Trace {
            id: "t2".into(),
            layer: Layer::Top,
            width: 0.3,
            points: vec![(pads[a].x, pads[a].y), (pads[b].x, pads[b].y)],
        });
        let cu = Copper::build(&p, &lib, &board, &conn);
        let issues = check(&board, &cu);
        assert!(
            issues.iter().any(|i| i.kind == DrcKind::Short),
            "{issues:?}"
        );
    }
}
