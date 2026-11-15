//! `PLAT-1`: OS-specific code lives only in `platform` modules — `breadbox::platform`,
//! `bb_firmware::platform` and `bb_core::paths`. A grep over every Rust source in the
//! workspace (crates, tests, plugins) keeps it that way.

use std::path::{Path, PathBuf};

/// The modules allowed to contain OS-specific code.
const PLATFORM_MODULES: &[&str] = &[
    "crates/breadbox/src/platform.rs",
    "crates/bb-firmware/src/platform.rs",
    "crates/bb-core/src/paths.rs",
];

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if p.is_dir() {
            if name == "target" || name.starts_with('.') || name == "node_modules" {
                continue;
            }
            rust_files(&p, out);
        } else if name.ends_with(".rs") {
            out.push(p);
        }
    }
}

/// Code (not comments) on a line: cut at the first `//` outside a string literal.
fn code(line: &str) -> &str {
    let mut in_str = false;
    let b = line.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\\' if in_str => i += 1,
            b'"' => in_str = !in_str,
            b'/' if !in_str && b.get(i + 1) == Some(&b'/') => return &line[..i],
            _ => {}
        }
        i += 1;
    }
    line
}

#[test]
fn os_specific_code_lives_only_in_platform_modules() {
    let root = workspace();
    let mut files = Vec::new();
    for d in ["crates", "tests", "plugins"] {
        rust_files(&root.join(d), &mut files);
    }
    assert!(files.len() > 100, "found only {} files", files.len());
    // `target_os` anywhere; the other OS markers in bb-firmware (the crate this rule was
    // missing in: toolchain paths, permissions, process groups)
    let everywhere = ["target_os"];
    let firmware_only = [
        "cfg(unix)",
        "cfg(windows)",
        "cfg(not(unix))",
        "consts::OS",
        "\"/opt/homebrew",
        "\"/usr/local",
        "std::os::unix",
        "std::os::windows",
    ];
    let mut offenders = Vec::new();
    for f in &files {
        let rel = f
            .strip_prefix(&root)
            .unwrap_or(f)
            .to_string_lossy()
            .replace('\\', "/");
        if PLATFORM_MODULES.contains(&rel.as_str()) || rel == "crates/bb-firmware/tests/platform.rs"
        {
            continue;
        }
        let firmware_src = rel.starts_with("crates/bb-firmware/src/");
        let Ok(text) = std::fs::read_to_string(f) else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            let c = code(line);
            let hit = everywhere.iter().find(|m| c.contains(*m)).or_else(|| {
                firmware_src
                    .then(|| firmware_only.iter().find(|m| c.contains(*m)))
                    .flatten()
            });
            if let Some(m) = hit {
                offenders.push(format!("{rel}:{}: `{m}`: {}", i + 1, line.trim()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "OS-specific code outside the platform modules (PLAT-1):\n{}",
        offenders.join("\n")
    );
}

#[test]
fn the_rule_catches_a_violation() {
    let line = "    if cfg!(target_os = \"macos\") {";
    assert_eq!(code(line), line);
    assert_eq!(code("    // cfg!(target_os) in a comment"), "    ");
    assert_eq!(
        code("let u = \"https://x\"; // target_os"),
        "let u = \"https://x\"; "
    );
}
