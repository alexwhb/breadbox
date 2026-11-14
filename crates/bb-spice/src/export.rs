//! Result export (CSV, ASCII rawfile) and rawfile import.
//!
//! The ASCII rawfile format is ngspice's (`Title/Date/Plotname/Flags/No. Variables/No. Points/
//! Variables/Values`); several plots may be concatenated in one file. [`read_raw`] is used by the
//! reference tests to load ngspice golden data.

use crate::matrix::C64;
use crate::results::{Data, Plot, PlotKind, Quantity, Results, Vector};
use std::fmt::Write;

/// How complex vectors are written to CSV.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ComplexCsv {
    /// `mag(x)` and `phase(x)` (degrees) columns.
    #[default]
    MagPhase,
    /// `db(x)` and `phase(x)` columns.
    DbPhase,
    /// `re(x)` and `im(x)` columns.
    ReIm,
}

fn csv_escape(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// One plot as CSV (header row of vector names, one row per point).
pub fn plot_to_csv(p: &Plot, complex: ComplexCsv) -> String {
    let mut o = String::new();
    let mut cols: Vec<String> = Vec::new();
    for v in &p.vectors {
        if v.is_complex() && !(p.has_scale && std::ptr::eq(v, &p.vectors[0])) {
            let (a, b) = match complex {
                ComplexCsv::MagPhase => ("mag", "phase"),
                ComplexCsv::DbPhase => ("db", "phase"),
                ComplexCsv::ReIm => ("re", "im"),
            };
            cols.push(csv_escape(&format!("{a}({})", v.name)));
            cols.push(csv_escape(&format!("{b}({})", v.name)));
        } else {
            cols.push(csv_escape(&v.name));
        }
    }
    let _ = writeln!(o, "{}", cols.join(","));
    for i in 0..p.len() {
        let mut row = Vec::with_capacity(cols.len());
        for (k, v) in p.vectors.iter().enumerate() {
            if v.is_complex() && !(p.has_scale && k == 0) {
                let c = v.complex_at(i);
                let (a, b) = match complex {
                    ComplexCsv::MagPhase => (c.abs(), c.arg().to_degrees()),
                    ComplexCsv::DbPhase => (20.0 * c.abs().log10(), c.arg().to_degrees()),
                    ComplexCsv::ReIm => (c.re, c.im),
                };
                row.push(format!("{a:.9e}"));
                row.push(format!("{b:.9e}"));
            } else {
                row.push(format!("{:.9e}", v.real_at(i)));
            }
        }
        let _ = writeln!(o, "{}", row.join(","));
    }
    o
}

/// One plot in ngspice ASCII rawfile format.
pub fn plot_to_raw(p: &Plot, title: &str) -> String {
    let mut o = String::new();
    let complex = p.vectors.iter().any(|v| v.is_complex());
    let _ = writeln!(o, "Title: {title}");
    let _ = writeln!(o, "Date: (breadbox)");
    let _ = writeln!(o, "Plotname: {}", p.kind.plot_name());
    let _ = writeln!(o, "Flags: {}", if complex { "complex" } else { "real" });
    let _ = writeln!(o, "No. Variables: {}", p.vectors.len());
    let _ = writeln!(o, "No. Points: {}", p.len());
    let _ = writeln!(o, "Variables:");
    for (i, v) in p.vectors.iter().enumerate() {
        let _ = writeln!(o, "\t{i}\t{}\t{}", v.name, v.quantity.raw_type());
    }
    let _ = writeln!(o, "Values:");
    for i in 0..p.len() {
        for (k, v) in p.vectors.iter().enumerate() {
            let val = if complex {
                let c = v.complex_at(i);
                format!("{:.15e},{:.15e}", c.re, c.im)
            } else {
                format!("{:.15e}", v.real_at(i))
            };
            if k == 0 {
                let _ = writeln!(o, "{i}\t\t{val}");
            } else {
                let _ = writeln!(o, "\t{val}");
            }
        }
    }
    o
}

/// All plots of all runs (plus Monte Carlo runs) as one ASCII rawfile.
pub fn results_to_raw(r: &Results) -> String {
    let mut o = String::new();
    for run in &r.runs {
        for p in &run.plots {
            o.push_str(&plot_to_raw(p, &r.title));
        }
    }
    if let Some(mc) = &r.mc {
        for run in &mc.runs {
            for p in &run.plots {
                o.push_str(&plot_to_raw(p, &format!("{} ({})", r.title, run.label)));
            }
        }
    }
    o
}

/// Error reading a rawfile.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("rawfile line {line}: {msg}")]
pub struct RawError {
    pub line: usize,
    pub msg: String,
}

/// Reads an ASCII rawfile (one or more plots).
pub fn read_raw(text: &str) -> Result<Vec<Plot>, RawError> {
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    let mut plots = Vec::new();
    let err = |line: usize, msg: &str| RawError {
        line: line + 1,
        msg: msg.to_string(),
    };
    while i < lines.len() {
        // header
        let mut plotname = String::new();
        let mut title = String::new();
        let mut complex = false;
        let mut nvars = 0usize;
        let mut npts = 0usize;
        while i < lines.len() && !lines[i].starts_with("Variables:") {
            let l = lines[i];
            if let Some(v) = l.strip_prefix("Plotname:") {
                plotname = v.trim().to_string();
            } else if let Some(v) = l.strip_prefix("Title:") {
                title = v.trim().to_string();
            } else if let Some(v) = l.strip_prefix("Flags:") {
                complex = v.contains("complex");
            } else if let Some(v) = l.strip_prefix("No. Variables:") {
                nvars = v.trim().parse().map_err(|_| err(i, "bad variable count"))?;
            } else if let Some(v) = l.strip_prefix("No. Points:") {
                npts = v.trim().parse().map_err(|_| err(i, "bad point count"))?;
            }
            i += 1;
        }
        if i >= lines.len() {
            break;
        }
        i += 1; // Variables:
        let mut names = Vec::new();
        let mut quants = Vec::new();
        for _ in 0..nvars {
            let l = lines
                .get(i)
                .ok_or_else(|| err(i, "truncated variable list"))?;
            let parts: Vec<&str> = l.split_whitespace().collect();
            if parts.len() < 3 {
                return Err(err(i, "bad variable line"));
            }
            names.push(parts[1].to_string());
            quants.push(Quantity::from_raw_type(parts[2]));
            i += 1;
        }
        if !lines.get(i).is_some_and(|l| l.starts_with("Values:")) {
            return Err(err(i, "expected `Values:`"));
        }
        i += 1;
        let mut re: Vec<Vec<f64>> = vec![Vec::with_capacity(npts); nvars];
        let mut im: Vec<Vec<f64>> = vec![Vec::with_capacity(npts); nvars];
        // values: tokens; each point = index + nvars values
        let mut tokens: Vec<&str> = Vec::with_capacity(npts * (nvars + 1));
        let need = npts * (nvars + 1);
        while tokens.len() < need && i < lines.len() {
            tokens.extend(lines[i].split_whitespace());
            i += 1;
        }
        if tokens.len() < need {
            return Err(err(i, "truncated values"));
        }
        for p in 0..npts {
            let base = p * (nvars + 1);
            for k in 0..nvars {
                let t = tokens[base + 1 + k];
                if complex {
                    let (a, b) = t
                        .split_once(',')
                        .ok_or_else(|| err(i, "expected complex value"))?;
                    re[k].push(a.parse().map_err(|_| err(i, "bad number"))?);
                    im[k].push(b.parse().map_err(|_| err(i, "bad number"))?);
                } else {
                    re[k].push(t.parse().map_err(|_| err(i, "bad number"))?);
                }
            }
        }
        let kind = PlotKind::from_plot_name(&plotname).unwrap_or(PlotKind::Op);
        let mut plot = Plot::new(kind, title);
        let has_scale = !matches!(kind, PlotKind::Op | PlotKind::Tf | PlotKind::NoiseTotal);
        for k in 0..nvars {
            let v = if complex {
                Vector {
                    name: names[k].clone(),
                    quantity: quants[k],
                    data: Data::Complex(
                        re[k]
                            .iter()
                            .zip(&im[k])
                            .map(|(a, b)| C64::new(*a, *b))
                            .collect(),
                    ),
                }
            } else {
                Vector {
                    name: names[k].clone(),
                    quantity: quants[k],
                    data: Data::Real(std::mem::take(&mut re[k])),
                }
            };
            if k == 0 && has_scale {
                plot.push_scale(v);
            } else {
                plot.push(v);
            }
        }
        plots.push(plot);
    }
    Ok(plots)
}

/// `name = value` lines printed by ngspice for `.meas` results.
pub fn read_meas_log(text: &str) -> Vec<(String, f64)> {
    let mut out = Vec::new();
    for l in text.lines() {
        let Some((a, b)) = l.split_once('=') else {
            continue;
        };
        let name = a.trim();
        if name.is_empty()
            || name.contains(' ')
            || !name.chars().next().unwrap().is_ascii_alphabetic()
        {
            continue;
        }
        if let Some(v) = b
            .split_whitespace()
            .next()
            .and_then(|t| t.parse::<f64>().ok())
        {
            out.push((name.to_ascii_lowercase(), v));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_round_trip() {
        let mut p = Plot::new(PlotKind::Tran, "t");
        p.push_scale(Vector::real("time", Quantity::Time, vec![0.0, 1e-3, 2e-3]));
        p.push(Vector::real(
            "v(out)",
            Quantity::Voltage,
            vec![0.0, 0.5, 0.75],
        ));
        let mut q = Plot::new(PlotKind::Ac, "t");
        q.push_scale(Vector::complex(
            "frequency",
            Quantity::Frequency,
            vec![C64::new(1.0, 0.0), C64::new(10.0, 0.0)],
        ));
        q.push(Vector::complex(
            "v(out)",
            Quantity::Voltage,
            vec![C64::new(1.0, -0.1), C64::new(0.5, -0.5)],
        ));
        let text = plot_to_raw(&p, "t") + &plot_to_raw(&q, "t");
        let back = read_raw(&text).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].real("v(out)").unwrap(), vec![0.0, 0.5, 0.75]);
        assert_eq!(
            back[1].vector("v(out)").unwrap().complex_at(1),
            C64::new(0.5, -0.5)
        );
        assert_eq!(back[1].kind, PlotKind::Ac);
    }

    #[test]
    fn csv_columns() {
        let mut q = Plot::new(PlotKind::Ac, "t");
        q.push_scale(Vector::complex(
            "frequency",
            Quantity::Frequency,
            vec![C64::new(1.0, 0.0)],
        ));
        q.push(Vector::complex(
            "v(out)",
            Quantity::Voltage,
            vec![C64::new(0.0, 1.0)],
        ));
        let csv = plot_to_csv(&q, ComplexCsv::DbPhase);
        let mut lines = csv.lines();
        assert_eq!(lines.next().unwrap(), "frequency,db(v(out)),phase(v(out))");
        let row: Vec<f64> = lines
            .next()
            .unwrap()
            .split(',')
            .map(|s| s.parse().unwrap())
            .collect();
        assert!((row[2] - 90.0).abs() < 1e-9 && row[1].abs() < 1e-9);
    }

    #[test]
    fn meas_log_lines() {
        let m = read_meas_log(
            "vavg                =  6.41760e-01 from=  1.00000e-03 to=  2.00960e-03\ntcross              =   2.79930e-03\nNo. of Data Rows : 511\n",
        );
        assert_eq!(
            m,
            vec![
                ("vavg".to_string(), 0.64176),
                ("tcross".to_string(), 2.7993e-3)
            ]
        );
    }
}
