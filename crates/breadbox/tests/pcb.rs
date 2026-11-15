//! Pro → PCB GUI end-to-end (`TEST-5`, `PRO-7`): blink's footprints are auto-placed, the DRC
//! flags its unrouted nets, traces routed with the Route tool (pad → corner → pad, on both
//! layers, a via) clear them, and the exported Gerbers + drill file parse back with every pad
//! and hole inside the board. Saves `docs/screenshots/pcb.png`.

mod common;

use bb_circuit::pcb::{self, Board, Copper};
use breadbox::BreadboxApp;
use breadbox::router::{ProTab, Route};
use breadbox::screens::pro::pcb::{PcbView, view_id};
use common::*;
use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};

type H = Harness<'static, BreadboxApp>;

fn view(h: &H) -> PcbView {
    h.state()
        .ctx
        .egui
        .data(|d| d.get_temp::<PcbView>(view_id()))
        .expect("pcb view")
}

fn copper(h: &mut H) -> Copper {
    let doc = h.state_mut().ctx.circuit.as_mut().unwrap();
    let conn = doc.connectivity();
    let board = Board::load(doc.project());
    Copper::build(doc.project(), &doc.lib, &board, &conn)
}

fn click(h: &mut H, p: egui::Pos2) {
    h.hover_at(p);
    h.run_steps(1);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
        h.run_steps(1);
    }
    h.run_steps(1);
}

fn drc_text(h: &H) -> String {
    h.query_all_by_label_contains("DRC ")
        .filter_map(|n| {
            let a = n.accesskit_node();
            a.label().or_else(|| a.value())
        })
        .next()
        .unwrap_or_default()
}

#[test]
fn blink_pcb_place_route_drc_and_gerbers() {
    let mut h = harness();
    h.get_by_label("Open example blink").click();
    h.run_steps(4);
    h.state_mut().ctx.navigate(Route::Pro(ProTab::Pcb));
    h.run_steps(5);
    assert!(has_label(&h, "PCB canvas"));
    h.get_by_label("Auto-place").click();
    h.run_steps(4);
    {
        let doc = h.state().ctx.circuit.as_ref().unwrap();
        let placed = pcb::placed_parts(doc.project(), &doc.lib, false);
        assert!(placed.len() >= 3, "UNO, R1, D1 on the board");
        assert!(doc.project().pcb.outline.is_some());
    }
    assert!(drc_text(&h).contains("unrouted"), "{}", drc_text(&h));
    // route every ratsnest line with the Route tool: pad → down into the free middle of the
    // board → across → up to the other pad, each net on its own lane (the first on the top
    // copper, the others on the bottom, so lanes never cross on one layer)
    h.get_by_label("Route").click();
    h.run_steps(2);
    let lines = copper(&mut h).ratsnest();
    assert!(!lines.is_empty());
    let (_, oy0, _, oy1) = Board::load(h.state().ctx.circuit.as_ref().unwrap().project())
        .outline
        .unwrap();
    for (k, (_, a, b)) in lines.iter().enumerate() {
        if k >= 1 {
            h.get_by_label("Bottom copper").click();
        } else {
            h.get_by_label("Top copper").click();
        }
        h.run_steps(1);
        let cu = copper(&mut h);
        let (pa, pb) = (cu.pads[*a].clone(), cu.pads[*b].clone());
        let lane = (oy0 + oy1) / 2.0 - 6.0 + 4.0 * k as f64;
        let v = view(&h);
        click(&mut h, v.to_screen(pa.x, pa.y));
        click(&mut h, v.to_screen(pa.x, lane));
        click(&mut h, v.to_screen(pb.x, lane));
        click(&mut h, v.to_screen(pb.x, pb.y));
    }
    h.run_steps(3);
    assert!(
        copper(&mut h).unrouted().is_empty(),
        "{:?}",
        copper(&mut h).unrouted()
    );
    assert!(!drc_text(&h).contains("unrouted"), "{}", drc_text(&h));
    assert_eq!(drc_text(&h), "DRC clean");
    {
        let doc = h.state().ctx.circuit.as_ref().unwrap();
        let b = Board::load(doc.project());
        assert!(b.traces.len() >= lines.len());
        assert!(b.traces.iter().any(|t| t.layer == pcb::Layer::Bottom));
    }
    // a via
    h.get_by_label("Via").click();
    h.run_steps(1);
    let (x0, y0, ..) = Board::load(h.state().ctx.circuit.as_ref().unwrap().project())
        .outline
        .unwrap();
    let v = view(&h);
    click(&mut h, v.to_screen(x0 + 1.5, y0 + 1.5));
    h.get_by_label("Select").click();
    h.run_steps(2);
    assert_eq!(
        Board::load(h.state().ctx.circuit.as_ref().unwrap().project())
            .vias
            .len(),
        1
    );
    h.state_mut().ctx.toasts.clear();
    screenshot(&mut h, "pcb");
    // Gerbers
    let out = breadbox::plugins::temp_dir("gerbers");
    // SAFETY: only this test reads it
    unsafe { std::env::set_var("BREADBOX_EXPORT_DIR", &out) };
    h.get_by_label("Export Gerbers…").click();
    h.run_steps(3);
    let dir = out.join("gerbers");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    files.sort();
    assert_eq!(files.len(), 7, "{files:?}");
    let board = Board::load(h.state().ctx.circuit.as_ref().unwrap().project());
    let (x0, y0, x1, y1) = board.outline.unwrap();
    let cu = copper(&mut h);
    for f in &files {
        let text = std::fs::read_to_string(f).unwrap();
        if f.extension().is_some_and(|e| e == "drl") {
            let holes = pcb::gerber::parse_excellon(&text).unwrap();
            let tht = cu.pads.iter().filter(|p| p.drill.is_some()).count();
            assert_eq!(holes.len(), tht + 1, "pads + via");
            continue;
        }
        let g = pcb::gerber::parse(&text).unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        for (_, x, y) in &g.flashes {
            assert!(*x >= -0.01 && *x <= x1 - x0 + 0.01 && *y >= -0.01 && *y <= y1 - y0 + 0.01);
        }
        let name = f.file_name().unwrap().to_string_lossy().to_string();
        if name.ends_with("F_Cu.gbr") {
            assert_eq!(g.flashes.len(), cu.pads.len() + 1, "every pad + the via");
            assert!(!g.draws.is_empty(), "top traces");
        }
    }
}
