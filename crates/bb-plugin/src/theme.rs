//! Theme plugins (PLUG-7): token overrides on top of the dark or light token set
//! (`docs/DESIGN.md`). The app applies [`Theme::tokens`] over its base tokens.

use crate::PluginError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Theme {
    pub name: String,
    /// `dark` or `light`: the token set the overrides start from.
    #[serde(default = "default_base")]
    pub base: String,
    /// Token name → `#RRGGBB` / `#RRGGBBAA`.
    pub tokens: BTreeMap<String, String>,
}

fn default_base() -> String {
    "dark".into()
}

/// `#RRGGBB` / `#RRGGBBAA` → RGBA.
pub fn parse_color(s: &str) -> Option<[u8; 4]> {
    let h = s.strip_prefix('#')?;
    if !(h.len() == 6 || h.len() == 8) || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let b = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    Some([b(0)?, b(2)?, b(4)?, if h.len() == 8 { b(6)? } else { 255 }])
}

impl Theme {
    pub fn parse(plugin: &str, text: &str) -> Result<Theme, PluginError> {
        let err = |msg: String| PluginError::Content {
            plugin: plugin.into(),
            msg: format!("theme: {msg}"),
        };
        let t: Theme = serde_json::from_str(text).map_err(|e| err(e.to_string()))?;
        if t.base != "dark" && t.base != "light" {
            return Err(err(format!("base must be dark or light, not `{}`", t.base)));
        }
        for (k, v) in &t.tokens {
            if parse_color(v).is_none() {
                return Err(err(format!(
                    "token `{k}`: `{v}` is not #RRGGBB or #RRGGBBAA"
                )));
            }
        }
        Ok(t)
    }
    /// Resolved colours.
    pub fn colors(&self) -> BTreeMap<String, [u8; 4]> {
        self.tokens
            .iter()
            .filter_map(|(k, v)| Some((k.clone(), parse_color(v)?)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn colors_and_validation() {
        assert_eq!(parse_color("#F7C77A"), Some([0xF7, 0xC7, 0x7A, 255]));
        assert_eq!(parse_color("#00000080"), Some([0, 0, 0, 0x80]));
        assert!(parse_color("red").is_none());
        let t = Theme::parse(
            "x.y",
            r##"{"name":"Solarized","base":"light","tokens":{"bg":"#FDF6E3"}}"##,
        )
        .unwrap();
        assert_eq!(t.colors()["bg"], [0xFD, 0xF6, 0xE3, 255]);
        assert!(Theme::parse("x.y", r#"{"name":"x","tokens":{"bg":"blue"}}"#).is_err());
    }
}
