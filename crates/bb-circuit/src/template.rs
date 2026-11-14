//! The SPICE template language used by part definitions.
//!
//! A template line is SPICE text with `$(…)` placeholders:
//!
//! | placeholder | expands to |
//! |---|---|
//! | `$(a)` | the SPICE node of pin `a`'s net (or, if there is no such pin, property `a`) |
//! | `$(prop:key)` | property value (numbers in SPICE notation, choices as text) |
//! | `$(el:R)`, `$(el:R:esr)` | element name: the designator if it starts with the letter (`R1`), else letter + designator (`RLDR1`); `:esr` appends `_esr` |
//! | `$(n:mid)` | part-local node `<ref>_mid` |
//! | `$(ext:lux)` | `ext("<REF>.lux")` — a value the simulator updates (world, knobs, damage) |
//! | `$(world:key)` | current world value (Pro export freezes e.g. `vwc=42`) |
//! | `$(ref)` | the designator |
//!
//! A line starting with `?key=value:` (or `?key!=value:`) is only used when the property
//! matches (RGB LED common anode vs cathode).

use crate::part::PropValue;

/// What a template needs from its surroundings.
pub struct TemplateCtx<'a> {
    pub reference: &'a str,
    pub pin_node: &'a dyn Fn(&str) -> Option<String>,
    pub prop: &'a dyn Fn(&str) -> Option<PropValue>,
    pub world: &'a dyn Fn(&str) -> Option<f64>,
}

/// Element name for a part's element of SPICE type `letter`.
pub fn element_name(letter: char, reference: &str, suffix: Option<&str>) -> String {
    let r = sanitize(reference);
    let l = letter.to_ascii_uppercase();
    let base = if r.to_ascii_uppercase().starts_with(l) {
        r
    } else {
        format!("{l}{r}")
    };
    match suffix {
        Some(s) if !s.is_empty() => format!("{base}_{}", sanitize(s)),
        _ => base,
    }
}

/// Expands an `el:` spec (`"R"`, `"R:esr"`) for a designator.
pub fn element_spec_name(spec: &str, reference: &str) -> String {
    let mut it = spec.splitn(2, ':');
    let letter = it.next().and_then(|s| s.chars().next()).unwrap_or('X');
    element_name(letter, reference, it.next())
}

/// Characters SPICE accepts in names: keep alphanumerics and `_`.
pub fn sanitize(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
        .collect();
    if out.is_empty() {
        out.push('x');
    }
    out
}

/// Bare placeholder names (no `kind:` prefix) in a line, e.g. pin references.
pub fn placeholders(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(i) = rest.find("$(") {
        let after = &rest[i + 2..];
        let Some(j) = after.find(')') else { break };
        let inner = &after[..j];
        if !inner.contains(':') {
            out.push(inner.trim().to_string());
        }
        rest = &after[j + 1..];
    }
    out
}

/// Element-name specs (`R`, `R:esr`) used in a line via `$(el:…)`.
pub fn element_specs(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(i) = rest.find("$(el:") {
        let after = &rest[i + 5..];
        let Some(j) = after.find(')') else { break };
        out.push(after[..j].trim().to_string());
        rest = &after[j + 1..];
    }
    out
}

/// Checks a `?key=value:` condition; returns the remaining line if it applies.
fn condition<'l>(line: &'l str, ctx: &TemplateCtx) -> Result<Option<&'l str>, String> {
    let Some(rest) = line.strip_prefix('?') else {
        return Ok(Some(line));
    };
    let colon = rest
        .find(':')
        .ok_or_else(|| format!("conditional template line without `:`: {line}"))?;
    let cond = &rest[..colon];
    let body = &rest[colon + 1..];
    let (key, want, neg) = if let Some((k, v)) = cond.split_once("!=") {
        (k, v, true)
    } else if let Some((k, v)) = cond.split_once('=') {
        (k, v, false)
    } else {
        return Err(format!("bad template condition `{cond}`"));
    };
    let have = (ctx.prop)(key.trim()).map(|v| v.as_text()).unwrap_or_default();
    let eq = have.eq_ignore_ascii_case(want.trim());
    Ok(if eq != neg { Some(body) } else { None })
}

/// Expands one template line; `Ok(None)` when a condition excludes it.
pub fn expand(line: &str, ctx: &TemplateCtx) -> Result<Option<String>, String> {
    let Some(body) = condition(line, ctx)? else {
        return Ok(None);
    };
    let mut out = String::with_capacity(body.len() + 16);
    let mut rest = body;
    while let Some(i) = rest.find("$(") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        let j = after
            .find(')')
            .ok_or_else(|| format!("unterminated `$(` in template: {line}"))?;
        let inner = after[..j].trim();
        out.push_str(&expand_one(inner, ctx).map_err(|e| format!("{e} (in `{line}`)"))?);
        rest = &after[j + 1..];
    }
    out.push_str(rest);
    Ok(Some(out))
}

fn expand_one(inner: &str, ctx: &TemplateCtx) -> Result<String, String> {
    if inner == "ref" {
        return Ok(ctx.reference.to_string());
    }
    if inner == "gnd" {
        return Ok("0".into());
    }
    if let Some((kind, arg)) = inner.split_once(':') {
        return match kind {
            "el" => Ok(element_spec_name(arg, ctx.reference)),
            "n" => Ok(format!("{}_{}", sanitize(ctx.reference), sanitize(arg)).to_ascii_lowercase()),
            "ext" => Ok(format!("ext(\"{}.{}\")", ctx.reference, arg)),
            "prop" => (ctx.prop)(arg)
                .map(|v| fmt_value(&v))
                .ok_or_else(|| format!("unknown property `{arg}`")),
            "pin" => (ctx.pin_node)(arg).ok_or_else(|| format!("unknown pin `{arg}`")),
            "world" => Ok(bb_spice::expr::fmt_g((ctx.world)(arg).unwrap_or(0.0))),
            _ => Err(format!("unknown placeholder kind `{kind}`")),
        };
    }
    if let Some(n) = (ctx.pin_node)(inner) {
        return Ok(n);
    }
    if let Some(v) = (ctx.prop)(inner) {
        return Ok(fmt_value(&v));
    }
    Err(format!("`{inner}` is neither a pin nor a property"))
}

fn fmt_value(v: &PropValue) -> String {
    match v {
        PropValue::Num(x) => bb_spice::expr::fmt_g(*x),
        PropValue::Bool(b) => (*b as u8).to_string(),
        PropValue::Text(t) => t.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_pins_props_elements_and_ext() {
        let pin = |p: &str| match p {
            "1" => Some("vcc".to_string()),
            "2" => Some("a1".to_string()),
            _ => None,
        };
        let prop = |k: &str| match k {
            "r1lux" => Some(PropValue::Num(350e3)),
            "gamma" => Some(PropValue::Num(0.7)),
            "common" => Some(PropValue::Text("anode".into())),
            _ => None,
        };
        let world = |_: &str| Some(42.0);
        let ctx = TemplateCtx {
            reference: "LDR1",
            pin_node: &pin,
            prop: &prop,
            world: &world,
        };
        let s = expand(
            "$(el:R) $(1) $(2) {$(r1lux)*pwr(max($(ext:lux),0.01),-$(gamma))}",
            &ctx,
        )
        .unwrap()
        .unwrap();
        assert_eq!(s, "RLDR1 vcc a1 {350000*pwr(max(ext(\"LDR1.lux\"),0.01),-0.7)}");
        assert_eq!(
            expand("$(el:R:esr) $(n:mid) $(gnd) 1", &ctx).unwrap().unwrap(),
            "RLDR1_esr ldr1_mid 0 1"
        );
        assert_eq!(expand("?common=cathode: D1 a b x", &ctx).unwrap(), None);
        assert_eq!(
            expand("?common=anode: D1 a b x", &ctx).unwrap().as_deref(),
            Some(" D1 a b x")
        );
        assert_eq!(expand("x=$(world:vwc)", &ctx).unwrap().unwrap(), "x=42");
        assert!(expand("$(nope)", &ctx).is_err());
        assert_eq!(element_name('R', "R1", None), "R1");
        assert_eq!(element_name('X', "J1", None), "XJ1");
        assert_eq!(placeholders("$(el:R) $(1) $(n:x) $(2)"), vec!["1", "2"]);
        assert_eq!(element_specs("$(el:R) $(el:V:sense) x"), vec!["R", "V:sense"]);
    }
}
