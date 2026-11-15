//! Plot widgets on `egui_plot`, styled like the design's SVG plots (`#1F1F1F` grid, 10 px
//! mono tick labels, unit on the top tick): multi-trace line plots with a secondary Y axis
//! (V and mA on one plot), log axes, draggable cursors with Δt / f / duty readouts, `.step`
//! families, histograms with threshold markers, Bode magnitude + phase. Export: CSV and SVG
//! (pure functions) and PNG (a cropped viewport screenshot).

use crate::theme::{font, pal};
use eframe::egui::{self, Color32, Id, Rect, Stroke, Ui, pos2, vec2};
use egui_plot::{AxisHints, GridInput, GridMark, HPlacement, Plot, PlotPoints};

/// Axis scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scale {
    #[default]
    Linear,
    /// Plotted as log10(value); ticks at decades.
    Log10,
}

/// Axis description.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AxisSpec {
    /// Unit appended to the top/right-most tick (`V`, `mA`, `ms`).
    pub unit: String,
    pub scale: Scale,
    /// Fixed range (data units), else auto.
    pub range: Option<(f64, f64)>,
    /// Multiply values for display (e.g. 1e3 to show A as mA).
    pub display_scale: f64,
}

impl AxisSpec {
    pub fn linear(unit: &str) -> Self {
        AxisSpec {
            unit: unit.into(),
            display_scale: 1.0,
            ..Default::default()
        }
    }
    pub fn log(unit: &str) -> Self {
        AxisSpec {
            unit: unit.into(),
            scale: Scale::Log10,
            display_scale: 1.0,
            ..Default::default()
        }
    }
    pub fn range(mut self, lo: f64, hi: f64) -> Self {
        self.range = Some((lo, hi));
        self
    }
    pub fn display(mut self, k: f64) -> Self {
        self.display_scale = k;
        self
    }
    fn fwd(&self, v: f64) -> f64 {
        match self.scale {
            Scale::Linear => v,
            Scale::Log10 => v.max(1e-300).log10(),
        }
    }
    fn inv(&self, v: f64) -> f64 {
        match self.scale {
            Scale::Linear => v,
            Scale::Log10 => 10f64.powf(v),
        }
    }
}

/// Which Y axis a trace uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum YAxis {
    #[default]
    Left,
    Right,
}

/// One trace.
#[derive(Debug, Clone, PartialEq)]
pub struct Trace {
    pub name: String,
    pub color: Color32,
    pub width: f32,
    /// Data-space points (x, y).
    pub points: Vec<[f64; 2]>,
    pub axis: YAxis,
}

impl Trace {
    pub fn new(name: impl Into<String>, color: Color32, points: Vec<[f64; 2]>) -> Self {
        Trace {
            name: name.into(),
            color,
            width: 2.0,
            points,
            axis: YAxis::Left,
        }
    }
    pub fn right(mut self) -> Self {
        self.axis = YAxis::Right;
        self
    }
    pub fn width(mut self, w: f32) -> Self {
        self.width = w;
        self
    }
}

/// Overlay markers.
#[derive(Debug, Clone, PartialEq)]
pub enum Marker {
    /// Dashed vertical line with a label near the top (thresholds).
    VLine {
        x: f64,
        label: String,
        color: Color32,
    },
    /// Highlighted point with a label (`op: 180 lx → 2.60 V`).
    Point {
        x: f64,
        y: f64,
        label: String,
        color: Color32,
    },
}

/// Two cursors on a time plot (data x).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Cursors {
    pub a: f64,
    pub b: f64,
    pub enabled: bool,
}

impl Cursors {
    pub fn delta(&self) -> f64 {
        (self.b - self.a).abs()
    }
}

/// Everything a line plot needs.
pub struct LinePlot<'a> {
    pub id: &'a str,
    pub traces: &'a [Trace],
    pub x: AxisSpec,
    pub y: AxisSpec,
    pub y2: Option<AxisSpec>,
    pub height: f32,
    pub markers: Vec<Marker>,
    pub cursors: Option<&'a mut Cursors>,
}

/// Result of showing a plot.
pub struct PlotOutput {
    pub rect: Rect,
    pub response: egui::Response,
}

fn data_bounds(traces: &[Trace], axis: YAxis) -> Option<(f64, f64)> {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for t in traces.iter().filter(|t| t.axis == axis) {
        for p in &t.points {
            if p[1].is_finite() {
                lo = lo.min(p[1]);
                hi = hi.max(p[1]);
            }
        }
    }
    (lo.is_finite() && hi.is_finite()).then_some((lo, hi))
}

/// "Nice" axis range covering `lo..hi` (0 included when close).
pub fn nice_range(lo: f64, hi: f64) -> (f64, f64) {
    let (a, b, _) = nice_range_step(lo, hi);
    (a, b)
}

/// [`nice_range`] plus the tick step that divides it evenly.
pub fn nice_range_step(lo: f64, hi: f64) -> (f64, f64, f64) {
    if !(lo.is_finite() && hi.is_finite()) {
        return (0.0, 1.0, 0.5);
    }
    let (mut lo, mut hi) = (lo, hi);
    if (hi - lo).abs() < 1e-15 {
        let d = if hi.abs() > 0.0 { hi.abs() * 0.1 } else { 1.0 };
        lo -= d;
        hi += d;
    }
    if lo > 0.0 && lo < (hi - lo) * 0.5 {
        lo = 0.0;
    }
    // values that only graze below/above zero (diode leakage, ringing) don't widen the axis
    if lo < 0.0 && -lo < (hi - lo) * 0.03 {
        lo = 0.0;
    }
    if hi > 0.0 && hi < (hi - lo) * 0.03 {
        hi = 0.0;
    }
    let span = hi - lo;
    // pick the step (1/2/2.5/5 × 10^k, 2–6 intervals) that wastes the least range
    let base = 10f64.powf((span / 4.0).log10().floor());
    let mut best = (f64::INFINITY, lo, hi, 1.0);
    for k in [0.1, 0.2, 0.25, 0.5, 1.0, 2.0, 2.5, 5.0, 10.0, 20.0] {
        let step = base * k;
        let (a, b) = ((lo / step).floor() * step, (hi / step).ceil() * step);
        let n = ((b - a) / step).round();
        if !(2.0..=6.0).contains(&n) {
            continue;
        }
        let waste = (b - a) - span;
        if waste < best.0 - 1e-12 {
            best = (waste, a, b, step);
        }
    }
    if best.0.is_finite() {
        // prefer fewer, design-like ticks (0 / 2.5 / 5) over many when the range is the same
        let (a, b, mut step) = (best.1, best.2, best.3);
        while ((b - a) / step).round() > 4.0 {
            let s2 = step * 2.5;
            if (((b - a) / s2) - ((b - a) / s2).round()).abs() < 1e-6 {
                step = s2;
            } else if (((b - a) / (step * 2.0)) - ((b - a) / (step * 2.0)).round()).abs() < 1e-6 {
                step *= 2.0;
            } else {
                break;
            }
        }
        return (a, b, step);
    }
    let step = nice_step(span / 4.0);
    ((lo / step).floor() * step, (hi / step).ceil() * step, step)
}

fn nice_step(raw: f64) -> f64 {
    if raw <= 0.0 || !raw.is_finite() {
        return 1.0;
    }
    let p = 10f64.powf(raw.log10().floor());
    let m = raw / p;
    let n = if m <= 1.0 + 1e-9 {
        1.0
    } else if m <= 2.0 + 1e-9 {
        2.0
    } else if m <= 2.5 + 1e-9 {
        2.5
    } else if m <= 5.0 {
        5.0
    } else {
        10.0
    };
    n * p
}

/// Compact SI formatting for tick labels: `2 ms`, `10k`, `2.5`, `490 Hz`.
pub fn fmt_si(v: f64, unit: &str) -> String {
    if v == 0.0 {
        return "0".into();
    }
    let a = v.abs();
    let (k, p) = [
        (1e12, "T"),
        (1e9, "G"),
        (1e6, "M"),
        (1e3, "k"),
        (1.0, ""),
        (1e-3, "m"),
        (1e-6, "µ"),
        (1e-9, "n"),
        (1e-12, "p"),
        (1e-15, "f"),
    ]
    .into_iter()
    .find(|(k, _)| a >= *k * 0.999_999)
    .unwrap_or((1e-15, "f"));
    let x = v / k;
    let s = if (x - x.round()).abs() < 1e-9 {
        format!("{}", x.round() as i64)
    } else if (x * 10.0 - (x * 10.0).round()).abs() < 1e-6 {
        format!("{x:.1}")
    } else {
        format!("{x:.2}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    };
    if unit.is_empty() {
        format!("{s}{p}")
    } else if p.is_empty() {
        format!("{s} {unit}")
    } else {
        format!("{s} {p}{unit}")
    }
}

/// Like [`fmt_si`] with fixed significant digits for readouts: `2.041 ms`, `490.0 Hz`.
pub fn fmt_readout(v: f64, unit: &str, decimals: usize) -> String {
    if v.abs() < 1e-15 || !v.is_finite() {
        return format!("{:.*} {unit}", decimals, 0.0);
    }
    let a = v.abs();
    let (k, p) = [
        (1e9, "G"),
        (1e6, "M"),
        (1e3, "k"),
        (1.0, ""),
        (1e-3, "m"),
        (1e-6, "µ"),
        (1e-9, "n"),
        (1e-12, "p"),
    ]
    .into_iter()
    .find(|(k, _)| a >= *k * 0.999_999)
    .unwrap_or((1e-12, "p"));
    format!("{:.*} {p}{unit}", decimals, v / k)
}

fn tick_font() -> egui::FontId {
    font::mono(10.0)
}

/// Tick text: SI prefix on the unit for physical units (`2 ms`, `5 V`), on the number for
/// others (`10k lx`).
fn tick_text(v: f64, unit: &str) -> String {
    if unit.is_empty() {
        return fmt_si(v, "");
    }
    if ["s", "V", "A", "W", "Hz"].contains(&unit) {
        fmt_si(v, unit)
    } else {
        format!("{} {unit}", fmt_si(v, ""))
    }
}

/// Grid marks at a "nice" step giving at most `max` intervals over the visible range.
fn nice_marks(input: GridInput, max: f64) -> Vec<GridMark> {
    let (lo, hi) = input.bounds;
    let span = (hi - lo).abs().max(1e-300);
    let mut step = nice_step(span / max * 0.97);
    while span / step > max + 0.3 {
        step = nice_step(step * 1.01);
    }
    fixed_marks(lo, hi, step)
}

/// Marks at multiples of `step` inside `lo..=hi`.
fn fixed_marks(lo: f64, hi: f64, step: f64) -> Vec<GridMark> {
    let mut v = Vec::new();
    let mut x = (lo / step).ceil() * step;
    while x <= hi + step * 1e-9 {
        v.push(GridMark {
            value: if x.abs() < step * 1e-9 { 0.0 } else { x },
            step_size: step,
        });
        x += step;
    }
    v
}

fn decade_spacer(input: GridInput) -> Vec<GridMark> {
    let (lo, hi) = input.bounds;
    let mut v = Vec::new();
    let mut k = lo.floor() as i64;
    while (k as f64) <= hi {
        if (k as f64) >= lo - 1e-9 {
            v.push(GridMark {
                value: k as f64,
                step_size: 1.0,
            });
        }
        for m in 2..10 {
            let x = k as f64 + (m as f64).log10();
            if x >= lo && x <= hi {
                v.push(GridMark {
                    value: x,
                    step_size: 0.1,
                });
            }
        }
        k += 1;
    }
    v
}

impl<'a> LinePlot<'a> {
    pub fn new(id: &'a str, traces: &'a [Trace]) -> Self {
        LinePlot {
            id,
            traces,
            x: AxisSpec::linear(""),
            y: AxisSpec::linear(""),
            y2: None,
            height: 200.0,
            markers: Vec::new(),
            cursors: None,
        }
    }

    pub fn show(self, ui: &mut Ui) -> PlotOutput {
        let p = pal(ui);
        let LinePlot {
            id,
            traces,
            x,
            y,
            y2,
            height,
            markers,
            cursors,
        } = self;
        // ranges
        let (y_lo, y_hi, y_step) = match y.range {
            Some((a, b)) => (a, b, nice_range_step(a, b).2),
            None => data_bounds(traces, YAxis::Left)
                .map(|(a, b)| nice_range_step(a, b))
                .unwrap_or((0.0, 1.0, 0.5)),
        };
        let (y_lo, y_hi) = (y.fwd(y_lo), y.fwd(y_hi));
        let y2_range = y2.as_ref().map(|ax| {
            let (a, b) = ax
                .range
                .or_else(|| data_bounds(traces, YAxis::Right).map(|(a, b)| nice_range(a, b)))
                .unwrap_or((0.0, 1.0));
            (a, b)
        });
        let map_right = move |v: f64| -> f64 {
            match y2_range {
                Some((a, b)) if (b - a).abs() > 0.0 => y_lo + (v - a) / (b - a) * (y_hi - y_lo),
                _ => v,
            }
        };
        let x_bounds = {
            let mut lo = f64::INFINITY;
            let mut hi = f64::NEG_INFINITY;
            for t in traces {
                for pt in &t.points {
                    lo = lo.min(pt[0]);
                    hi = hi.max(pt[0]);
                }
            }
            x.range
                .unwrap_or(if lo.is_finite() { (lo, hi) } else { (0.0, 1.0) })
        };
        let (x_lo, x_hi) = (x.fwd(x_bounds.0), x.fwd(x_bounds.1));

        let xs = x.clone();
        let ys = y.clone();
        let y2s = y2.clone();
        let x_fmt = move |m: GridMark, r: &std::ops::RangeInclusive<f64>| -> String {
            if xs.scale == Scale::Log10 && m.step_size < 0.5 {
                return String::new();
            }
            let v = xs.inv(m.value) * xs.display_scale;
            let last = m.value + m.step_size > *r.end() - 1e-9;
            if last {
                tick_text(v, &xs.unit)
            } else {
                fmt_si(v, "")
            }
        };
        let y_fmt = move |m: GridMark, r: &std::ops::RangeInclusive<f64>| -> String {
            if ys.scale == Scale::Log10 && m.step_size < 0.5 {
                return String::new();
            }
            let v = ys.inv(m.value) * ys.display_scale;
            let last = m.value + m.step_size > *r.end() - 1e-9;
            if last {
                tick_text(v, &ys.unit)
            } else {
                fmt_si(v, "")
            }
        };
        let y2_fmt = move |m: GridMark, r: &std::ops::RangeInclusive<f64>| -> String {
            let (Some(ax), Some((a, b))) = (&y2s, y2_range) else {
                return String::new();
            };
            let span = (y_hi - y_lo).max(1e-300);
            let v = a + (m.value - y_lo) / span * (b - a);
            let last = m.value + m.step_size > *r.end() - 1e-9;
            let first = m.value - m.step_size < *r.start() + 1e-9;
            if last {
                tick_text(v * ax.display_scale, &ax.unit)
            } else if first {
                fmt_si(v * ax.display_scale, "")
            } else {
                String::new()
            }
        };

        let mut y_axes = vec![
            AxisHints::new_y()
                .formatter(y_fmt)
                .tick_label_font(tick_font())
                .tick_label_color(p.text_muted)
                .min_thickness(34.0),
        ];
        if y2.is_some() {
            y_axes.push(
                AxisHints::new_y()
                    .formatter(y2_fmt)
                    .placement(HPlacement::Right)
                    .tick_label_font(tick_font())
                    .tick_label_color(p.info)
                    .min_thickness(34.0),
            );
        }
        let x_axes = vec![
            AxisHints::new_x()
                .formatter(x_fmt)
                .tick_label_font(tick_font())
                .tick_label_color(p.text_muted),
        ];

        // cursor drag handling (uses last frame's transform)
        let mem_id = Id::unique(("plot-transform", id));
        let prev: Option<egui_plot::PlotTransform> = ui.data(|d| d.get_temp(mem_id));
        let mut dragging_cursor: Option<u8> = ui.data(|d| d.get_temp(mem_id.with("drag")));
        let mut cursors = cursors;
        let pointer = ui.input(|i| i.pointer.interact_pos());
        if let (Some(c), Some(t), Some(pt)) = (cursors.as_deref_mut(), prev.as_ref(), pointer)
            && c.enabled
        {
            let pressed = ui.input(|i| i.pointer.primary_pressed());
            if pressed && t.frame().contains(pt) {
                let xa = t.position_from_point_x(x.fwd(c.a));
                let xb = t.position_from_point_x(x.fwd(c.b));
                if (pt.x - xa).abs() < 6.0 {
                    dragging_cursor = Some(0);
                } else if (pt.x - xb).abs() < 6.0 {
                    dragging_cursor = Some(1);
                }
            }
            if let Some(k) = dragging_cursor {
                if ui.input(|i| i.pointer.primary_down()) {
                    let v = x.inv(t.value_from_position(pt).x);
                    if k == 0 {
                        c.a = v;
                    } else {
                        c.b = v;
                    }
                } else {
                    dragging_cursor = None;
                }
            }
        }
        ui.data_mut(|d| d.insert_temp(mem_id.with("drag"), dragging_cursor));

        let traces_owned: Vec<(String, Color32, f32, Vec<[f64; 2]>)> = traces
            .iter()
            .map(|t| {
                let pts = t
                    .points
                    .iter()
                    .filter(|p| p[0].is_finite() && p[1].is_finite())
                    .map(|pt| {
                        let yy = match t.axis {
                            YAxis::Left => y.fwd(pt[1]),
                            YAxis::Right => map_right(pt[1]),
                        };
                        [x.fwd(pt[0]), yy]
                    })
                    .collect();
                (t.name.clone(), t.color, t.width, pts)
            })
            .collect();

        let mut plot = Plot::new(id)
            .height(height)
            .show_background(false)
            .grid_color(p.grid)
            .custom_x_axes(x_axes)
            .custom_y_axes(y_axes)
            .allow_drag(dragging_cursor.is_none())
            .allow_scroll(false)
            .allow_zoom(true)
            .allow_boxed_zoom(true)
            .allow_double_click_reset(true)
            .show_crosshair(false)
            .auto_bounds(egui::Vec2b::new(false, false))
            .default_x_bounds(x_lo - (x_hi - x_lo) * 0.005, x_hi + (x_hi - x_lo) * 0.005)
            .default_y_bounds(y_lo - (y_hi - y_lo) * 0.02, y_hi + (y_hi - y_lo) * 0.02)
            .label_formatter({
                let xs = x.clone();
                let ys = y.clone();
                move |hp: &egui_plot::HoverPosition<'_>| {
                    let pt = match hp {
                        egui_plot::HoverPosition::NearDataPoint { position, .. } => *position,
                        egui_plot::HoverPosition::Elsewhere { position } => *position,
                    };
                    Some(format!(
                        "{}\n{}",
                        fmt_readout(xs.inv(pt.x) * xs.display_scale, &xs.unit, 3),
                        fmt_readout(ys.inv(pt.y) * ys.display_scale, &ys.unit, 3)
                    ))
                }
            });
        plot = if x.scale == Scale::Log10 {
            plot.x_grid_spacer(decade_spacer)
        } else {
            plot.x_grid_spacer(|g| nice_marks(g, 5.0))
        };
        plot = if y.scale == Scale::Log10 {
            plot.y_grid_spacer(decade_spacer)
        } else {
            plot.y_grid_spacer(move |g| {
                let span = (g.bounds.1 - g.bounds.0).abs();
                let n = span / y_step;
                if (1.5..=8.0).contains(&n) {
                    fixed_marks(g.bounds.0, g.bounds.1, y_step)
                } else {
                    nice_marks(g, 4.0)
                }
            })
        };
        let resp = plot.show(ui, |pui| {
            for (name, color, w, pts) in traces_owned {
                pui.line(
                    egui_plot::Line::new(name, PlotPoints::new(pts))
                        .color(color)
                        .width(w),
                );
            }
        });
        let t = resp.transform;
        ui.data_mut(|d| d.insert_temp(mem_id, t));
        let frame = *t.frame();
        let painter = ui.painter().with_clip_rect(frame.expand(2.0));
        // markers
        for m in &markers {
            match m {
                Marker::VLine {
                    x: mx,
                    label,
                    color,
                } => {
                    let px = t.position_from_point_x(x.fwd(*mx));
                    let pts = [pos2(px, frame.top() + 6.0), pos2(px, frame.bottom())];
                    painter.extend(egui::Shape::dashed_line(
                        &pts,
                        Stroke::new(1.0, *color),
                        3.0,
                        3.0,
                    ));
                    painter.text(
                        pos2(px - 4.0, frame.top() + 12.0),
                        egui::Align2::RIGHT_CENTER,
                        label,
                        tick_font(),
                        *color,
                    );
                }
                Marker::Point {
                    x: mx,
                    y: my,
                    label,
                    color,
                } => {
                    let pos =
                        t.position_from_point(&egui_plot::PlotPoint::new(x.fwd(*mx), y.fwd(*my)));
                    let pts = [pos2(pos.x, frame.top()), pos2(pos.x, frame.bottom())];
                    painter.extend(egui::Shape::dashed_line(
                        &pts,
                        Stroke::new(1.0, color.gamma_multiply(0.6)),
                        3.0,
                        3.0,
                    ));
                    painter.circle(pos, 4.5, p.bg_app, Stroke::new(2.0, *color));
                    painter.text(
                        pos + vec2(8.0, 14.0),
                        egui::Align2::LEFT_CENTER,
                        label,
                        tick_font(),
                        p.accent_soft,
                    );
                }
            }
        }
        // cursors
        if let Some(c) = cursors
            && c.enabled
        {
            let xa = t.position_from_point_x(x.fwd(c.a));
            let xb = t.position_from_point_x(x.fwd(c.b));
            for px in [xa, xb] {
                let pts = [pos2(px, frame.top() - 6.0), pos2(px, frame.bottom() + 6.0)];
                painter.extend(egui::Shape::dashed_line(
                    &pts,
                    Stroke::new(1.0, p.text),
                    3.0,
                    3.0,
                ));
            }
            let label = format!("Δ {}", fmt_readout(c.delta() * x.display_scale, &x.unit, 3));
            let g = painter.layout_no_wrap(label, tick_font(), p.text);
            let cx = (xa + xb) / 2.0;
            let r = Rect::from_center_size(pos2(cx, frame.top() + 9.0), g.size() + vec2(14.0, 6.0));
            painter.rect(
                r,
                3.0,
                p.bg_raised,
                Stroke::new(1.0, p.border_strong),
                egui::StrokeKind::Inside,
            );
            painter.galley(r.center() - g.size() / 2.0, g, p.text);
        }
        let label = format!("{id} plot");
        resp.response
            .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Figure, true, &label));
        PlotOutput {
            rect: frame,
            response: resp.response,
        }
    }
}

/// Histogram bins.
#[derive(Debug, Clone, PartialEq)]
pub struct Histogram {
    /// `(lo, hi, count)`.
    pub bins: Vec<(f64, f64, usize)>,
    pub unit: String,
    pub threshold: Option<(f64, String)>,
}

/// Histogram plot (Monte Carlo spread) with an optional threshold marker.
pub fn histogram_plot(ui: &mut Ui, id: &str, h: &Histogram, height: f32) -> PlotOutput {
    let p = pal(ui);
    let bars: Vec<egui_plot::Bar> = h
        .bins
        .iter()
        .map(|(lo, hi, n)| {
            egui_plot::Bar::new((lo + hi) / 2.0, *n as f64)
                .width((hi - lo) * 0.86)
                .fill(p.accent)
                .stroke(Stroke::NONE)
        })
        .collect();
    let max = h.bins.iter().map(|b| b.2).max().unwrap_or(1) as f64;
    let (mut lo, mut hi) = (
        h.bins.first().map(|b| b.0).unwrap_or(0.0),
        h.bins.last().map(|b| b.1).unwrap_or(1.0),
    );
    if let Some((t, _)) = &h.threshold {
        let pad = (hi - lo) * 0.08;
        lo = lo.min(t - pad);
        hi = hi.max(t + pad);
    }
    let unit = h.unit.clone();
    let x_fmt = move |m: GridMark, r: &std::ops::RangeInclusive<f64>| {
        let last = m.value + m.step_size > *r.end() - 1e-9;
        if last {
            format!("{:.2} {unit}", m.value)
        } else {
            format!("{:.2}", m.value)
        }
    };
    let resp = Plot::new(id)
        .height(height)
        .show_background(false)
        .grid_color(p.grid)
        .show_grid([false, false])
        .show_axes([true, false])
        .custom_x_axes(vec![
            AxisHints::new_x()
                .formatter(x_fmt)
                .tick_label_font(tick_font())
                .tick_label_color(p.text_muted),
        ])
        .allow_drag(false)
        .allow_scroll(false)
        .allow_zoom(false)
        .allow_boxed_zoom(false)
        .auto_bounds(egui::Vec2b::new(false, false))
        .x_grid_spacer(|g| nice_marks(g, 3.0))
        .default_x_bounds(lo - (hi - lo) * 0.04, hi + (hi - lo) * 0.04)
        .default_y_bounds(0.0, max * 1.15)
        .show(ui, |pui| {
            pui.bar_chart(egui_plot::BarChart::new("runs", bars).color(p.accent));
        });
    let t = resp.transform;
    let frame = *t.frame();
    let painter = ui.painter().with_clip_rect(frame.expand(2.0));
    painter.line_segment(
        [frame.left_bottom(), frame.right_bottom()],
        Stroke::new(1.0, p.grid),
    );
    if let Some((x, label)) = &h.threshold {
        let px = t.position_from_point_x(*x);
        let pts = [pos2(px, frame.top() + 8.0), pos2(px, frame.bottom())];
        painter.extend(egui::Shape::dashed_line(
            &pts,
            Stroke::new(1.0, p.info),
            3.0,
            3.0,
        ));
        painter.text(
            pos2(px - 4.0, frame.top() + 14.0),
            egui::Align2::RIGHT_CENTER,
            label,
            tick_font(),
            p.info,
        );
    }
    let l = format!("{id} histogram");
    resp.response
        .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Figure, true, &l));
    PlotOutput {
        rect: frame,
        response: resp.response,
    }
}

/// Bode plot: magnitude (dB) over phase (°), linked log-frequency axes.
pub fn bode_plot(ui: &mut Ui, id: &str, mag: &[Trace], phase: &[Trace], height: f32) -> PlotOutput {
    let a = LinePlot {
        id: &format!("{id}-mag"),
        traces: mag,
        x: AxisSpec::log("Hz"),
        y: AxisSpec::linear("dB"),
        y2: None,
        height: height * 0.58,
        markers: vec![],
        cursors: None,
    }
    .show(ui);
    let b = LinePlot {
        id: &format!("{id}-phase"),
        traces: phase,
        x: AxisSpec::log("Hz"),
        y: AxisSpec::linear("°"),
        y2: None,
        height: height * 0.42,
        markers: vec![],
        cursors: None,
    }
    .show(ui);
    PlotOutput {
        rect: a.rect.union(b.rect),
        response: a.response.union(b.response),
    }
}

// ------------------------------------------------------------------------------------------
// Export

/// CSV of traces: one x column per trace group sharing x, else long format.
pub fn traces_to_csv(x_name: &str, traces: &[Trace]) -> String {
    let mut out = String::new();
    let same_x = traces.windows(2).all(|w| {
        w[0].points.len() == w[1].points.len()
            && w[0]
                .points
                .iter()
                .zip(&w[1].points)
                .all(|(a, b)| a[0] == b[0])
    });
    if same_x && !traces.is_empty() {
        out.push_str(x_name);
        for t in traces {
            out.push(',');
            out.push_str(&csv_field(&t.name));
        }
        out.push('\n');
        for i in 0..traces[0].points.len() {
            out.push_str(&format!("{:e}", traces[0].points[i][0]));
            for t in traces {
                out.push_str(&format!(",{:e}", t.points[i][1]));
            }
            out.push('\n');
        }
    } else {
        out.push_str(&format!("trace,{x_name},value\n"));
        for t in traces {
            for pt in &t.points {
                out.push_str(&format!("{},{:e},{:e}\n", csv_field(&t.name), pt[0], pt[1]));
            }
        }
    }
    out
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Standalone SVG of a line plot (vector export, `PRO-6`).
pub fn traces_to_svg(
    title: &str,
    traces: &[Trace],
    x: &AxisSpec,
    y: &AxisSpec,
    w: f32,
    h: f32,
) -> String {
    let (ml, mr, mt, mb) = (56.0f32, 20.0f32, 30.0f32, 30.0f32);
    let pw = w - ml - mr;
    let ph = h - mt - mb;
    let mut xr = (f64::INFINITY, f64::NEG_INFINITY);
    let mut yr = (f64::INFINITY, f64::NEG_INFINITY);
    for t in traces {
        for p in &t.points {
            xr = (xr.0.min(x.fwd(p[0])), xr.1.max(x.fwd(p[0])));
            yr = (yr.0.min(p[1]), yr.1.max(p[1]));
        }
    }
    if !xr.0.is_finite() {
        xr = (0.0, 1.0);
        yr = (0.0, 1.0);
    }
    let yr = nice_range(yr.0, yr.1);
    let sx = |v: f64| ml as f64 + (x.fwd(v) - xr.0) / (xr.1 - xr.0).max(1e-300) * pw as f64;
    let sy = |v: f64| mt as f64 + ph as f64 - (v - yr.0) / (yr.1 - yr.0).max(1e-300) * ph as f64;
    let hex = |c: Color32| format!("#{:02X}{:02X}{:02X}", c.r(), c.g(), c.b());
    let mut s = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {w} {h}\" width=\"{w}\" height=\"{h}\">\n<rect width=\"{w}\" height=\"{h}\" fill=\"#0A0A0A\"/>\n<text x=\"{ml}\" y=\"18\" font-family=\"Space Grotesk, Inter, sans-serif\" font-size=\"14\" fill=\"#EAEAEA\">{}</text>\n<g stroke=\"#1F1F1F\">",
        xml(title)
    );
    for k in 0..=4 {
        let yy = mt + ph * k as f32 / 4.0;
        s.push_str(&format!("<path d=\"M{ml} {yy}H{}\"/>", ml + pw));
        let xx = ml + pw * k as f32 / 4.0;
        s.push_str(&format!("<path d=\"M{xx} {mt}V{}\"/>", mt + ph));
    }
    s.push_str(
        "</g>\n<g font-family=\"JetBrains Mono, monospace\" font-size=\"10\" fill=\"#A3A3A3\">",
    );
    for k in 0..=4 {
        let v = yr.0 + (yr.1 - yr.0) * k as f64 / 4.0;
        let yy = sy(v);
        s.push_str(&format!(
            "<text x=\"{}\" y=\"{:.1}\" text-anchor=\"end\">{}</text>",
            ml - 6.0,
            yy + 3.0,
            xml(&fmt_si(
                v * y.display_scale,
                if k == 4 { &y.unit } else { "" }
            ))
        ));
        let xv = x.inv(xr.0 + (xr.1 - xr.0) * k as f64 / 4.0);
        s.push_str(&format!(
            "<text x=\"{:.1}\" y=\"{}\" text-anchor=\"middle\">{}</text>",
            sx(xv),
            h - 10.0,
            xml(&fmt_si(
                xv * x.display_scale,
                if k == 4 { &x.unit } else { "" }
            ))
        ));
    }
    s.push_str("</g>\n");
    for t in traces {
        let mut d = String::new();
        for (i, p) in t.points.iter().enumerate() {
            d.push_str(&format!(
                "{}{:.2} {:.2}",
                if i == 0 { "M" } else { "L" },
                sx(p[0]),
                sy(p[1])
            ));
        }
        s.push_str(&format!(
            "<path d=\"{d}\" fill=\"none\" stroke=\"{}\" stroke-width=\"{}\"><title>{}</title></path>\n",
            hex(t.color),
            t.width,
            xml(&t.name)
        ));
    }
    s.push_str("</svg>\n");
    s
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Crops a screenshot to `rect` (points) and encodes PNG.
pub fn crop_png(
    image: &egui::ColorImage,
    rect: Rect,
    pixels_per_point: f32,
) -> Result<Vec<u8>, String> {
    let [iw, ih] = image.size;
    let x0 = ((rect.left() * pixels_per_point).floor().max(0.0) as usize).min(iw);
    let y0 = ((rect.top() * pixels_per_point).floor().max(0.0) as usize).min(ih);
    let x1 = ((rect.right() * pixels_per_point).ceil() as usize).min(iw);
    let y1 = ((rect.bottom() * pixels_per_point).ceil() as usize).min(ih);
    if x1 <= x0 || y1 <= y0 {
        return Err("empty plot area".into());
    }
    let mut buf = image::RgbaImage::new((x1 - x0) as u32, (y1 - y0) as u32);
    for y in y0..y1 {
        for x in x0..x1 {
            let c = image.pixels[y * iw + x];
            buf.put_pixel(
                (x - x0) as u32,
                (y - y0) as u32,
                image::Rgba(c.to_srgba_unmultiplied()),
            );
        }
    }
    let mut out = std::io::Cursor::new(Vec::new());
    buf.write_to(&mut out, image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

/// Result slot of a PNG export started with [`export_png`].
pub type PngResult = std::sync::Arc<std::sync::Mutex<Option<Result<std::path::PathBuf, String>>>>;

/// Requests a viewport screenshot and writes the `rect` crop as PNG to `path` when the pixels
/// arrive (one or two frames later). Poll the returned slot.
pub fn export_png(ctx: &egui::Context, rect: Rect, path: std::path::PathBuf) -> PngResult {
    let slot: PngResult = Default::default();
    let s2 = slot.clone();
    let ppp = ctx.pixels_per_point();
    let c2 = ctx.clone();
    ctx.request_screenshot(move |img| {
        let r = crop_png(&img, rect, ppp)
            .and_then(|png| std::fs::write(&path, png).map_err(|e| e.to_string()))
            .map(|_| path.clone());
        if let Ok(mut g) = s2.lock() {
            *g = Some(r);
        }
        c2.request_repaint();
    });
    ctx.request_repaint();
    slot
}

/// Picks evenly spaced cursor positions on a periodic waveform: the first two rising edges
/// through the mid level after the start. Returns `(t_rise1, t_rise2, duty)`.
pub fn auto_cursors(points: &[[f64; 2]]) -> Option<(f64, f64, f64)> {
    if points.len() < 4 {
        return None;
    }
    let (lo, hi) = points
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| {
            (a.min(p[1]), b.max(p[1]))
        });
    if hi - lo < 1e-9 {
        return None;
    }
    let mid = (lo + hi) / 2.0;
    let cross = |i: usize, rising: bool| -> Option<f64> {
        let (a, b) = (points[i], points[i + 1]);
        let ok = if rising {
            a[1] < mid && b[1] >= mid
        } else {
            a[1] >= mid && b[1] < mid
        };
        ok.then(|| a[0] + (mid - a[1]) / (b[1] - a[1]) * (b[0] - a[0]))
    };
    let mut rises = Vec::new();
    let mut falls = Vec::new();
    for i in 0..points.len() - 1 {
        if let Some(t) = cross(i, true) {
            rises.push(t);
        }
        if let Some(t) = cross(i, false) {
            falls.push(t);
        }
    }
    // skip a rise at t = 0 (start-up), use the next full period
    let r: Vec<f64> = rises
        .into_iter()
        .filter(|t| *t > points[0][0] + 1e-12)
        .collect();
    let (t1, t2) = if r.len() >= 3 {
        (r[1], r[2])
    } else {
        (*r.first()?, *r.get(1)?)
    };
    let fall = falls.iter().copied().find(|f| *f > t1 && *f < t2)?;
    Some((t1, t2, (fall - t1) / (t2 - t1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn si_formatting_for_ticks_and_readouts() {
        assert_eq!(fmt_si(0.002, "s"), "2 ms");
        assert_eq!(fmt_si(10_000.0, ""), "10k");
        assert_eq!(fmt_si(2.5, ""), "2.5");
        assert_eq!(fmt_si(5.0, "V"), "5 V");
        assert_eq!(fmt_si(0.015, "A"), "15 mA");
        assert_eq!(fmt_readout(0.002041, "s", 3), "2.041 ms");
        assert_eq!(fmt_readout(490.0, "Hz", 1), "490.0 Hz");
    }

    #[test]
    fn nice_ranges() {
        assert_eq!(nice_range(0.0, 4.98), (0.0, 5.0));
        assert_eq!(nice_range(0.3, 4.6), (0.0, 5.0));
        assert_eq!(nice_range_step(-0.001, 4.97), (0.0, 5.0, 2.5));
        let (a, b) = nice_range(2.31, 2.87);
        assert!(a <= 2.31 && b >= 2.87 && b - a < 1.0);
    }

    #[test]
    fn cursors_measure_a_490_hz_pwm() {
        // 490 Hz, 47.8 % duty, 10 ms, 1 µs samples
        let period = 1.0 / 490.0;
        let pts: Vec<[f64; 2]> = (0..10_000)
            .map(|i| {
                let t = i as f64 * 1e-6;
                let ph = (t % period) / period;
                [t, if ph < 0.478 { 5.0 } else { 0.0 }]
            })
            .collect();
        let (a, b, duty) = auto_cursors(&pts).unwrap();
        assert!(((b - a) - period).abs() < 2e-6, "{}", b - a);
        assert!((duty - 0.478).abs() < 0.002, "{duty}");
    }

    #[test]
    fn csv_and_svg_export() {
        let t1 = Trace::new("v(a1)", Color32::RED, vec![[1.0, 0.1], [10.0, 0.5]]);
        let t2 = Trace::new("v(b)", Color32::BLUE, vec![[1.0, 0.2], [10.0, 0.6]]);
        let csv = traces_to_csv("lux", &[t1.clone(), t2]);
        assert!(csv.starts_with("lux,v(a1),v(b)\n1e0,1e-1,2e-1\n"), "{csv}");
        let svg = traces_to_svg(
            ".dc — V(A1) vs light",
            &[t1],
            &AxisSpec::log("lx"),
            &AxisSpec::linear("V"),
            440.0,
            210.0,
        );
        assert!(
            svg.starts_with("<svg")
                && svg.contains("<path d=\"M")
                && svg.contains("V(A1) vs light")
        );
    }

    #[test]
    fn png_crop() {
        let mut img = egui::ColorImage::new([20, 10], vec![Color32::BLACK; 200]);
        img.pixels[5 * 20 + 5] = Color32::WHITE;
        let png = crop_png(
            &img,
            Rect::from_min_max(pos2(2.0, 2.0), pos2(8.0, 8.0)),
            1.0,
        )
        .unwrap();
        let back = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(back.dimensions(), (6, 6));
        assert_eq!(back.get_pixel(3, 3).0, [255, 255, 255, 255]);
    }
}
