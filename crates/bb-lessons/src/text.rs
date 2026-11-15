//! Expressions and text templates for lesson steps.
//!
//! Expressions use the SPICE expression language (`3.0 / R * 1000`, `abs(guess - x) / x <=
//! 0.05`, `if(…)`), variables are case-insensitive. Templates replace `{name}` or
//! `{name:spec}`: spec `s` = smart (≥ 10 → 1 decimal, ≥ 1 → 2, else 3 — the design's
//! formatting), `.N` = N decimals, `.Nt` = N decimals with trailing zeros trimmed, `i` =
//! integer, `g` = shortest; text variables (`{choice}`) are inserted as is.

use std::collections::BTreeMap;

/// Variables of a step: numbers and texts.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Vars {
    pub num: BTreeMap<String, f64>,
    pub text: BTreeMap<String, String>,
}

impl Vars {
    pub fn set(&mut self, k: &str, v: f64) {
        self.num.insert(k.to_ascii_lowercase(), v);
    }
    pub fn set_text(&mut self, k: &str, v: &str) {
        self.text.insert(k.to_ascii_lowercase(), v.into());
    }
    pub fn get(&self, k: &str) -> Option<f64> {
        self.num.get(&k.to_ascii_lowercase()).copied()
    }
}

/// Evaluates an expression with the variables.
pub fn eval(expr: &str, vars: &Vars) -> Result<f64, String> {
    let e = bb_spice::expr::Expr::parse(expr).map_err(|e| e.to_string())?;
    e.eval_with(&|name: &str| vars.get(name))
}

/// The design's number formatting: ≥ 10 → 1 decimal, ≥ 1 → 2, else 3.
pub fn smart(v: f64) -> String {
    let a = v.abs();
    if a >= 10.0 {
        format!("{v:.1}")
    } else if a >= 1.0 {
        format!("{v:.2}")
    } else {
        format!("{v:.3}")
    }
}

fn trimmed(v: f64, n: usize) -> String {
    let s = format!("{v:.n$}");
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

fn fmt_num(v: f64, spec: &str) -> String {
    match spec {
        "" | "s" => smart(v),
        "i" => format!("{}", v.round() as i64),
        "g" => {
            if v == v.trunc() && v.abs() < 1e12 {
                format!("{}", v as i64)
            } else {
                format!("{v}")
            }
        }
        s if s.starts_with('.') => {
            let (n, trim) = match s.strip_suffix('t') {
                Some(n) => (n, true),
                None => (s, false),
            };
            let n: usize = n[1..].parse().unwrap_or(3);
            if trim {
                trimmed(v, n)
            } else {
                format!("{v:.n$}")
            }
        }
        _ => smart(v),
    }
}

/// Fills `{name}` / `{name:spec}` placeholders; unknown names stay as they are.
pub fn render(template: &str, vars: &Vars) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let Some(j) = after.find('}') else {
            out.push_str(&rest[i..]);
            return out;
        };
        let inner = &after[..j];
        let (name, spec) = inner.split_once(':').unwrap_or((inner, ""));
        let key = name.trim().to_ascii_lowercase();
        if let Some(t) = vars.text.get(&key) {
            out.push_str(t);
        } else if let Some(v) = vars.num.get(&key) {
            out.push_str(&fmt_num(*v, spec.trim()));
        } else if let Ok(v) = eval(name, vars) {
            out.push_str(&fmt_num(v, spec.trim()));
        } else {
            out.push('{');
            out.push_str(inner);
            out.push('}');
        }
        rest = &after[j + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_and_formats() {
        let mut v = Vars::default();
        v.set("R", 220.0);
        v.set_text("choice", "220Ω");
        v.set("amps", 3.0 / 220.0);
        v.set("mA", 3000.0 / 220.0);
        assert_eq!(
            render("What current do you expect with {choice}?", &v),
            "What current do you expect with 220Ω?"
        );
        assert_eq!(render("I = 3.0 V ÷ {R:g} Ω", &v), "I = 3.0 V ÷ 220 Ω");
        assert_eq!(
            render("I = {amps:.5t} A = {mA} mA", &v),
            "I = 0.01364 A = 13.6 mA"
        );
        assert_eq!(render("{3/R*1000:s} mA", &v), "13.6 mA");
        assert_eq!(render("{nope}", &v), "{nope}");
        assert!((eval("3.0 / r * 1000", &v).unwrap() - 13.636).abs() < 1e-3);
        assert_eq!(smart(0.3), "0.300");
        assert_eq!(smart(3.0), "3.00");
        assert_eq!(smart(30.0), "30.0");
    }
}
