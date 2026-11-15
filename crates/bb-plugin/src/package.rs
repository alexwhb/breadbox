//! `.bbplugin` packages: a zip of a plugin folder (PLUG-1). [`inspect`] reads the manifest for
//! the install prompt without extracting; [`unpack`] extracts safely (no absolute paths, no
//! `..`, size and file-count limits, `plugin.json` at the root or inside a single top folder);
//! [`pack`] builds one from a folder (lesson editor, SDK template).

use crate::manifest::Manifest;
use crate::{PluginError, io};
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

/// Package limits.
pub const MAX_FILES: usize = 4_000;
pub const MAX_BYTES: u64 = 128 << 20;

fn perr(e: impl std::fmt::Display) -> PluginError {
    PluginError::Package(e.to_string())
}

/// Entries of a package: (safe relative path, is_dir), and the prefix (top folder) to strip.
fn index(
    z: &mut zip::ZipArchive<Cursor<&[u8]>>,
) -> Result<(Vec<(PathBuf, bool)>, PathBuf), PluginError> {
    if z.len() > MAX_FILES {
        return Err(perr(format!("too many files ({} > {MAX_FILES})", z.len())));
    }
    let mut entries = Vec::new();
    let mut total = 0u64;
    for i in 0..z.len() {
        let f = z.by_index(i).map_err(perr)?;
        let name = f.name().map(|c| c.to_string()).unwrap_or_default();
        let path = f
            .enclosed_name()
            .ok_or_else(|| perr(format!("unsafe path `{name}` in package")))?;
        total = total.saturating_add(f.size());
        if total > MAX_BYTES {
            return Err(perr("package is larger than 128 MB unpacked"));
        }
        entries.push((path, f.is_dir()));
    }
    // plugin.json at the root, or inside exactly one top-level folder
    let prefix = if entries.iter().any(|(p, _)| p == Path::new("plugin.json")) {
        PathBuf::new()
    } else {
        let candidates: Vec<&PathBuf> = entries
            .iter()
            .map(|(p, _)| p)
            .filter(|p| {
                p.file_name().is_some_and(|n| n == "plugin.json") && p.components().count() == 2
            })
            .collect();
        match candidates.as_slice() {
            [one] => one.parent().map(Path::to_path_buf).unwrap_or_default(),
            [] => return Err(perr("no plugin.json in the package")),
            _ => return Err(perr("several plugin.json files in the package")),
        }
    };
    Ok((entries, prefix))
}

/// Reads the manifest of a package (for the permission prompt).
pub fn inspect(bytes: &[u8]) -> Result<Manifest, PluginError> {
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).map_err(perr)?;
    let (_, prefix) = index(&mut z)?;
    let name = prefix.join("plugin.json");
    let name = name.to_string_lossy().replace('\\', "/");
    let mut f = z.by_name(&name).map_err(perr)?;
    let mut text = String::new();
    f.read_to_string(&mut text).map_err(perr)?;
    let m = Manifest::parse(&text)?;
    m.validate(None)?;
    Ok(m)
}

/// Extracts a package into `dest` (created; must not exist or be empty) and validates it there.
pub fn unpack(bytes: &[u8], dest: &Path) -> Result<Manifest, PluginError> {
    let mut z = zip::ZipArchive::new(Cursor::new(bytes)).map_err(perr)?;
    let (entries, prefix) = index(&mut z)?;
    std::fs::create_dir_all(dest).map_err(|e| io(dest, e))?;
    for (i, (path, is_dir)) in entries.iter().enumerate() {
        let Ok(rel) = path.strip_prefix(&prefix) else {
            continue; // files outside the plugin folder are ignored
        };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let out = dest.join(rel);
        if *is_dir {
            std::fs::create_dir_all(&out).map_err(|e| io(&out, e))?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
        }
        let mut f = z.by_index(i).map_err(perr)?;
        let mut w = std::fs::File::create(&out).map_err(|e| io(&out, e))?;
        std::io::copy(&mut (&mut f).take(MAX_BYTES), &mut w).map_err(|e| io(&out, e))?;
    }
    Manifest::load(dest)
}

fn skip(name: &str) -> bool {
    name.starts_with('.') || name == "target" || name == "Cargo.lock"
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), PluginError> {
    let mut items: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| io(dir, e))?
        .filter_map(|e| e.ok())
        .collect();
    items.sort_by_key(|e| e.file_name());
    for e in items {
        let name = e.file_name().to_string_lossy().to_string();
        if skip(&name) {
            continue;
        }
        let p = e.path();
        if p.is_dir() {
            walk(root, &p, out)?;
        } else if let Ok(rel) = p.strip_prefix(root) {
            out.push(rel.to_path_buf());
        }
    }
    Ok(())
}

/// Zips a plugin folder (validated first) into `.bbplugin` bytes.
pub fn pack(dir: &Path) -> Result<Vec<u8>, PluginError> {
    Manifest::load(dir)?;
    let mut files = Vec::new();
    walk(dir, dir, &mut files)?;
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for rel in files {
        let name = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join("/");
        let data = std::fs::read(dir.join(&rel)).map_err(|e| io(&dir.join(&rel), e))?;
        w.start_file(name, opts).map_err(perr)?;
        w.write_all(&data).map_err(perr)?;
    }
    Ok(w.finish().map_err(perr)?.into_inner())
}
