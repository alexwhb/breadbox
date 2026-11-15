//! Analysis plugins (PLUG-7): netlist in → results out. The Pro screen hands the plugin the
//! circuit's netlist and the card's options; the plugin may call the solver
//! (`bb_spice_run`) as often as it likes and returns an [`AnalysisOutput`] with `bb_result`.

use crate::PluginError;
use crate::sandbox::{CallCtx, Compiled, Sandbox};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

/// One vector of a solver result (`v(out)` real values, or magnitude + phase for AC).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VectorOut {
    pub name: String,
    pub values: Vec<f64>,
    /// AC phase in degrees (complex vectors only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase_deg: Option<Vec<f64>>,
}

/// One plot of a solver result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlotOut {
    /// `op`, `dc`, `tran`, `ac`, `noise`, `tf`.
    pub kind: String,
    /// Run label (`.step` values), empty otherwise.
    pub run: String,
    pub vectors: Vec<VectorOut>,
}

/// What `bb_spice_run` returns (JSON in the inbox).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SpiceOut {
    pub plots: Vec<PlotOut>,
    /// `.meas` results: (name, value).
    pub measures: Vec<(String, f64)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

const SOLVER_TIMEOUT: Duration = Duration::from_secs(20);

/// Runs a netlist with bb-spice (every analysis in it), with a time limit.
pub fn run_netlist(text: &str) -> SpiceOut {
    let nl = match bb_spice::Netlist::parse(text) {
        Ok(n) => n,
        Err(e) => {
            return SpiceOut {
                error: Some(e.to_string()),
                ..Default::default()
            };
        }
    };
    let cancel = bb_spice::CancelToken::new();
    let c2 = cancel.clone();
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let d2 = done.clone();
    std::thread::spawn(move || {
        let t0 = std::time::Instant::now();
        while t0.elapsed() < SOLVER_TIMEOUT {
            if d2.load(std::sync::atomic::Ordering::Relaxed) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        c2.cancel();
    });
    let r = nl.run_with(&bb_spice::RunOptions {
        cancel: Some(cancel),
        ..Default::default()
    });
    done.store(true, std::sync::atomic::Ordering::Relaxed);
    match r {
        Ok(res) => {
            let mut out = SpiceOut::default();
            for run in &res.runs {
                for p in &run.plots {
                    let kind = format!("{:?}", p.kind).to_ascii_lowercase();
                    let vectors = p
                        .vectors
                        .iter()
                        .map(|v| {
                            if v.is_complex() {
                                let n = v.len();
                                let c: Vec<_> = (0..n).map(|i| v.complex_at(i)).collect();
                                VectorOut {
                                    name: v.name.clone(),
                                    values: c.iter().map(|x| x.abs()).collect(),
                                    phase_deg: Some(
                                        c.iter().map(|x| x.arg().to_degrees()).collect(),
                                    ),
                                }
                            } else {
                                VectorOut {
                                    name: v.name.clone(),
                                    values: v.reals(),
                                    phase_deg: None,
                                }
                            }
                        })
                        .collect();
                    out.plots.push(PlotOut {
                        kind,
                        run: run.label.clone(),
                        vectors,
                    });
                }
                for m in &run.measures {
                    if let Ok(v) = m.value {
                        out.measures.push((m.name.clone(), v));
                    }
                }
            }
            out
        }
        Err(e) => SpiceOut {
            error: Some(e.to_string()),
            ..Default::default()
        },
    }
}

/// A series of an analysis plot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Series {
    pub name: String,
    #[serde(default)]
    pub unit: String,
    pub values: Vec<f64>,
}

/// A plot the Pro screen draws.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnalysisPlot {
    pub title: String,
    pub x_label: String,
    #[serde(default)]
    pub x_log: bool,
    pub x: Vec<f64>,
    pub series: Vec<Series>,
}

/// What an analysis plugin returns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AnalysisOutput {
    /// Card header (`Sensitivity · 4 parts`).
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub plots: Vec<AnalysisPlot>,
    /// Table rows; the first row is the header.
    #[serde(default)]
    pub table: Vec<Vec<String>>,
    #[serde(default)]
    pub text: String,
}

/// Runs an analysis plugin once on a netlist.
pub fn run(
    c: &Arc<Compiled>,
    netlist: &str,
    options: &serde_json::Value,
) -> Result<AnalysisOutput, PluginError> {
    let m = &c.manifest;
    let mut sb = Sandbox::new(c, 0xa11a, None)?;
    sb.init(&serde_json::json!({
        "plugin": m.id,
        "version": m.version,
        "role": "analysis",
        "specs": m.specs,
        "permissions": m.permissions,
    }))?;
    let input = serde_json::json!({ "netlist": netlist, "options": options });
    let bytes = serde_json::to_vec(&input).unwrap_or_default();
    sb.out().result = None;
    let code = sb
        .call_bytes(CallCtx::None, "bb_analysis_run", &bytes)?
        .ok_or_else(|| PluginError::Abi {
            plugin: m.id.clone(),
            msg: "does not export bb_analysis_run".into(),
        })?;
    if code != 0 {
        let msg = sb
            .out()
            .error
            .take()
            .unwrap_or_else(|| format!("bb_analysis_run returned {code}"));
        return Err(PluginError::Content {
            plugin: m.id.clone(),
            msg,
        });
    }
    let out = sb.out().result.take().unwrap_or_default();
    serde_json::from_slice(&out).map_err(|e| PluginError::Abi {
        plugin: m.id.clone(),
        msg: format!("analysis result is not valid: {e}"),
    })
}
