#![allow(dead_code)]
//! Helpers: temporary folders and throwaway plugins built from WAT.

use std::path::{Path, PathBuf};

pub struct Tmp(pub PathBuf);

impl Tmp {
    pub fn new(name: &str) -> Tmp {
        let p = std::env::temp_dir().join(format!(
            "bb-plugin-test-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        Tmp(p)
    }
    pub fn dir(&self, name: &str) -> PathBuf {
        let d = self.0.join(name);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    /// A plugin folder: plugin.json (+ `extra` manifest fields), model.wasm from `wat`,
    /// parts.json.
    pub fn plugin(
        &self,
        id: &str,
        kinds: &str,
        perms: &str,
        wat: &str,
        parts: Option<&str>,
        extra: &str,
    ) -> PathBuf {
        let d = self.dir(id);
        let wasm = wat::parse_str(wat).unwrap();
        std::fs::write(d.join("model.wasm"), wasm).unwrap();
        let parts_field = match parts {
            Some(p) => {
                std::fs::write(d.join("parts.json"), p).unwrap();
                r#""parts": "parts.json","#
            }
            None => "",
        };
        let manifest = format!(
            r#"{{"id": "{id}", "version": "1.0.0", "kind": {kinds}, "summary": "test",
                {extra}
                "provides": {{ {parts_field} "model": "model.wasm" }},
                "permissions": {perms}}}"#
        );
        std::fs::write(d.join("plugin.json"), manifest).unwrap();
        d
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The repository root.
pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}
