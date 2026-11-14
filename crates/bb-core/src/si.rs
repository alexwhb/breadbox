//! SI-prefixed formatting and SPICE-style value parsing.

use thiserror::Error;

#[derive(Debug, Error, PartialEq)]
#[error("cannot parse value `{0}`")]
pub struct ParseError(pub String);

const PREFIXES: [(f64, &str); 11] = [
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
    (1e-18, "a"),
];

/// Formats `value` with an SI prefix and 4 significant digits, trimming zeros:
/// `format(1.882, "V") == "1.882 V"`, `format(0.0065, "A") == "6.5 mA"`,
/// `format(4700.0, "Ω") == "4.7 kΩ"`.
pub fn format(value: f64, unit: &str) -> String {
    format_sig(value, unit, 4)
}

/// Like [`format`] with an explicit number of significant digits.
pub fn format_sig(value: f64, unit: &str, sig: usize) -> String {
    if value == 0.0 || !value.is_finite() {
        let v = if value.is_nan() {
            "NaN".to_string()
        } else if value.is_infinite() {
            if value > 0.0 {
                "∞".into()
            } else {
                "-∞".into()
            }
        } else {
            "0".into()
        };
        return if unit.is_empty() {
            v
        } else {
            format!("{v} {unit}")
        };
    }
    let mag = value.abs();
    let (scale, prefix) = PREFIXES
        .iter()
        .copied()
        .find(|(s, _)| mag >= *s * 0.999_95)
        .unwrap_or((1e-18, "a"));
    let scaled = value / scale;
    let int_digits = (scaled.abs().log10().floor() as i32 + 1).max(1) as usize;
    let decimals = sig.saturating_sub(int_digits);
    let mut s = format!("{scaled:.decimals$}");
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    let u = format!("{prefix}{unit}");
    if u.is_empty() { s } else { format!("{s} {u}") }
}

/// Parses SPICE-style numbers: `4k7`, `10meg`, `2.2u`, `100n`, `1e-3`, `5V`, `220Ω`, `3mil`.
/// Case-insensitive like SPICE (`M`/`m` = milli, `meg` = mega); trailing unit letters ignored.
pub fn parse(s: &str) -> Result<f64, ParseError> {
    let t = s.trim();
    let err = || ParseError(s.to_string());
    if t.is_empty() {
        return Err(err());
    }
    let lower = t.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    // Longest numeric prefix (digits, sign, dot, exponent).
    let mut end = 0;
    let mut seen_digit = false;
    while end < bytes.len() {
        let c = bytes[end] as char;
        if c.is_ascii_digit() {
            seen_digit = true;
            end += 1;
        } else if c == '.'
            || ((c == '+' || c == '-') && (end == 0 || bytes[end - 1] == b'e'))
            || (c == 'e' && seen_digit && end + 1 < bytes.len() && {
                let n = bytes[end + 1] as char;
                n.is_ascii_digit()
                    || ((n == '+' || n == '-')
                        && end + 2 < bytes.len()
                        && (bytes[end + 2] as char).is_ascii_digit())
            })
        {
            end += 1;
        } else {
            break;
        }
    }
    if !seen_digit {
        return Err(err());
    }
    let mut base: f64 = lower[..end].parse().map_err(|_| err())?;
    let rest = &lower[end..];
    let rest_orig = &t[end..];
    // "4k7" style: suffix in the middle of digits.
    let (mult, consumed) = if rest.starts_with("meg") {
        (1e6, 3)
    } else if rest.starts_with("mil") {
        (25.4e-6, 3)
    } else if rest_orig.starts_with('µ') {
        (1e-6, 'µ'.len_utf8())
    } else {
        match rest.chars().next() {
            Some('t') => (1e12, 1),
            Some('g') => (1e9, 1),
            Some('k') => (1e3, 1),
            Some('m') => (1e-3, 1),
            Some('u') => (1e-6, 1),
            Some('n') => (1e-9, 1),
            Some('p') => (1e-12, 1),
            Some('f') => (1e-15, 1),
            Some('a') => (1e-18, 1),
            _ => (1.0, 0),
        }
    };
    let after = &rest[consumed..];
    if consumed > 0
        && !after.is_empty()
        && after.chars().all(|c| c.is_ascii_digit())
        && !lower[..end].contains('.')
    {
        // 4k7 → 4.7k
        base = format!("{}.{}", &lower[..end], after)
            .parse()
            .map_err(|_| err())?;
    }
    Ok(base * mult)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_like_the_design() {
        assert_eq!(format(1.882, "V"), "1.882 V");
        assert_eq!(format(0.0065, "A"), "6.5 mA");
        assert_eq!(format(4700.0, "Ω"), "4.7 kΩ");
        assert_eq!(format(0.0082, "W"), "8.2 mW");
        assert_eq!(format(10e6, "Ω"), "10 MΩ");
        assert_eq!(format(100e-12, "F"), "100 pF");
        assert_eq!(format(-0.0004, "V"), "-400 µV");
        assert_eq!(format(0.0, "V"), "0 V");
    }

    #[test]
    fn parses_spice_values() {
        let close = |a: f64, b: f64| (a - b).abs() <= 1e-12 * b.abs().max(1.0);
        assert!(close(parse("4k7").unwrap(), 4700.0));
        assert!(close(parse("10meg").unwrap(), 10e6));
        assert!(close(parse("10Meg").unwrap(), 10e6));
        assert!(close(parse("2.2u").unwrap(), 2.2e-6));
        assert!(close(parse("2.2µF").unwrap(), 2.2e-6));
        assert!(close(parse("100n").unwrap(), 100e-9));
        assert!(close(parse("1e-3").unwrap(), 1e-3));
        assert!(close(parse("5V").unwrap(), 5.0));
        assert!(close(parse("220").unwrap(), 220.0));
        assert!(close(parse("1M").unwrap(), 1e-3));
        assert!(close(parse("-1.5e3").unwrap(), -1500.0));
        assert!(parse("abc").is_err());
        assert!(parse("").is_err());
    }
}
