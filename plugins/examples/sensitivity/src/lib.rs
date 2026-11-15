//! Analysis example: DC sensitivity of a node to every resistor. Runs the netlist's
//! operating point, then once more per resistor scaled by +1 %, and reports
//! ∂V/∂R·R (volts per 1 % change), sorted by influence.

use bb_plugin_sdk::prelude::*;

struct Sensitivity;

fn op_voltage(ctx: &Ctx, netlist: &str, node: &str) -> Result<f64, String> {
    let r = ctx.spice_run(netlist)?;
    let name = format!("v({})", node.to_ascii_lowercase());
    for p in r["plots"].as_array().cloned().unwrap_or_default() {
        if p["kind"] != "op" {
            continue;
        }
        for v in p["vectors"].as_array().cloned().unwrap_or_default() {
            if v["name"].as_str().map(|n| n.to_ascii_lowercase()) == Some(name.clone()) {
                return v["values"][0]
                    .as_f64()
                    .ok_or_else(|| "no value".to_string());
            }
        }
    }
    Err(format!("no node {node} in the operating point"))
}

/// The netlist with `.op` as its only analysis.
fn with_op(netlist: &str) -> String {
    let mut out = Vec::new();
    for l in netlist.lines() {
        let t = l.trim_start().to_ascii_lowercase();
        if t.starts_with(".tran")
            || t.starts_with(".ac")
            || t.starts_with(".dc")
            || t.starts_with(".op")
            || t.starts_with(".end")
            || t.starts_with(".step")
            || t.starts_with(".mc")
            || t.starts_with(".noise")
        {
            continue;
        }
        out.push(l.to_string());
    }
    out.push(".op".into());
    out.push(".end".into());
    out.join("\n")
}

impl Analysis for Sensitivity {
    fn init(_cfg: &Config) -> Result<Self, String> {
        Ok(Sensitivity)
    }
    fn run(&mut self, ctx: &Ctx, input: &AnalysisInput) -> Result<Value, String> {
        let node = input
            .options
            .get("node")
            .and_then(|n| n.as_str())
            .unwrap_or("out")
            .to_string();
        let base_nl = with_op(&input.netlist);
        let v0 = op_voltage(ctx, &base_nl, &node)?;
        let mut rows: Vec<(String, f64, f64)> = Vec::new();
        for line in base_nl.lines() {
            let mut toks = line.split_whitespace();
            let Some(name) = toks.next() else { continue };
            if !name.to_ascii_lowercase().starts_with('r') {
                continue;
            }
            let (Some(_a), Some(_b), Some(val)) = (toks.next(), toks.next(), toks.next()) else {
                continue;
            };
            let scaled = format!("{{({})*1.01}}", val.trim_matches(|c| c == '{' || c == '}'));
            let nl: String = base_nl
                .lines()
                .map(|l| {
                    if l.split_whitespace().next() == Some(name) {
                        l.replacen(val, &scaled, 1)
                    } else {
                        l.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            let v1 = op_voltage(ctx, &nl, &node)?;
            rows.push((
                name.to_string(),
                val.to_string().parse().unwrap_or(f64::NAN),
                v1 - v0,
            ));
        }
        rows.sort_by(|a, b| b.2.abs().total_cmp(&a.2.abs()));
        let mut table = vec![vec![
            "Resistor".to_string(),
            "Value".into(),
            format!("ΔV({node}) per +1 %"),
        ]];
        for (n, v, d) in &rows {
            table.push(vec![
                n.clone(),
                format!("{v}"),
                format!("{:+.3} mV", d * 1e3),
            ]);
        }
        Ok(json!({
            "summary": format!("Sensitivity of V({node}) = {v0:.4} V · {} resistors", rows.len()),
            "table": table,
            "plots": [{
                "title": format!("ΔV({node}) per +1 %"),
                "x_label": "resistor #",
                "x": (0..rows.len()).map(|i| i as f64).collect::<Vec<_>>(),
                "series": [{"name": "ΔV", "unit": "V", "values": rows.iter().map(|r| r.2).collect::<Vec<_>>()}],
            }],
            "text": "",
        }))
    }
}

export_analysis!(Sensitivity);
