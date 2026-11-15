//! Share / export (`PRJ-5`): the `.breadbox.zip` bundle (and opening one), ngspice `.cir`,
//! BOM CSV, schematic SVG / PNG. The bench PNG is a screenshot of the canvas
//! (`widgets::plot::export_png`).

use crate::circuit::CircuitDoc;
use anyhow::{Context as _, Result, bail};
use bb_circuit::{Library, Project};
use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// Zips the project folder (sketches, analyses) with the in-memory `project.json`.
pub fn bundle_zip(doc: &CircuitDoc, out: &Path) -> Result<()> {
    use zip::write::SimpleFileOptions;
    let file = std::fs::File::create(out).with_context(|| out.display().to_string())?;
    let mut z = zip::ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let root = format!(
        "{}.breadbox",
        doc.project().name.replace(['/', '\\', ':'], "-")
    );
    z.start_file(format!("{root}/project.json"), opts)?;
    z.write_all(doc.project().to_json().as_bytes())?;
    let mut files = Vec::new();
    collect(&doc.dir, &doc.dir, &mut files)?;
    for rel in files {
        if rel == Path::new("project.json") {
            continue;
        }
        let name = format!("{root}/{}", rel.to_string_lossy().replace('\\', "/"));
        z.start_file(name, opts)?;
        z.write_all(&std::fs::read(doc.dir.join(&rel))?)?;
    }
    // examples reference prebuilt images outside their folder: bundle those too
    for p in &doc.project().parts {
        if let Some(img) = p.firmware.as_ref().and_then(|f| f.image.as_ref())
            && img.starts_with("..")
        {
            let src = doc.dir.join(img);
            for path in [src.clone(), src.with_extension("elf")] {
                if path.exists()
                    && let Some(fname) = path.file_name()
                {
                    let name = format!("{root}/firmware/prebuilt/{}", fname.to_string_lossy());
                    z.start_file(name, opts)?;
                    z.write_all(&std::fs::read(&path)?)?;
                }
            }
        }
    }
    z.finish()?;
    Ok(())
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        let path = e.path();
        let name = e.file_name();
        let n = name.to_string_lossy();
        if n.starts_with('.') || n == "target" {
            continue;
        }
        if path.is_dir() {
            collect(root, &path, out)?;
        } else if let Ok(rel) = path.strip_prefix(root) {
            out.push(rel.to_path_buf());
        }
    }
    Ok(())
}

/// File extensions treated as SPICE netlists by Open / Import (`PRJ-4`).
pub const CIR_EXTENSIONS: [&str; 5] = ["cir", "net", "sp", "spice", "ckt"];

/// Imports an ngspice / LTspice-style netlist as a new Pro project `parent/<name>.breadbox`
/// (a fresh name if it exists): parts with an auto-placed schematic, analysis cards from the
/// netlist's dot-cards (Pro → Analysis shows the netlist synced from that schematic), and the
/// original file kept as `imported/<name>.cir`. Returns the folder.
pub fn import_cir_project(cir: &Path, parent: &Path, lib: &Library) -> Result<PathBuf> {
    let text = std::fs::read_to_string(cir).with_context(|| cir.display().to_string())?;
    let stem = cir
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "imported".into());
    let base: String = stem
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let base = if base.is_empty() {
        "imported".to_string()
    } else {
        base
    };
    let project = bb_circuit::spice_io::import_cir(&text, &stem, lib)
        .map_err(|e| anyhow::anyhow!("{}: {e}", cir.display()))?;
    let mut dest = parent.join(format!("{base}.breadbox"));
    let mut n = 2;
    while dest.exists() {
        dest = parent.join(format!("{base}-{n}.breadbox"));
        n += 1;
    }
    project
        .save(&dest)
        .map_err(|e| anyhow::anyhow!("{}: {e}", dest.display()))?;
    std::fs::create_dir_all(dest.join("analyses"))?;
    let kept = dest.join("imported");
    std::fs::create_dir_all(&kept)?;
    std::fs::write(kept.join(format!("{base}.cir")), &text)?;
    Ok(dest)
}

/// Unpacks a `.breadbox.zip` into `parent/<name>.breadbox` (a fresh name if it exists) and
/// returns the project folder. Prebuilt images bundled from an example are re-pointed.
pub fn import_bundle(zip_path: &Path, parent: &Path) -> Result<PathBuf> {
    let file = std::fs::File::open(zip_path).with_context(|| zip_path.display().to_string())?;
    let mut a = zip::ZipArchive::new(file).context("not a zip archive")?;
    let tmp = parent.join(format!(".import-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    a.extract(&tmp).context("extract")?;
    // the archive has one top folder with project.json
    let top = std::fs::read_dir(&tmp)?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.join("project.json").exists())
        .or_else(|| tmp.join("project.json").exists().then(|| tmp.clone()));
    let Some(top) = top else {
        let _ = std::fs::remove_dir_all(&tmp);
        bail!("{} has no project.json", zip_path.display());
    };
    let base = top
        .file_name()
        .map(|s| {
            s.to_string_lossy()
                .trim_end_matches(".breadbox")
                .to_string()
        })
        .filter(|s| !s.starts_with(".import"))
        .unwrap_or_else(|| "imported".into());
    let mut dest = parent.join(format!("{base}.breadbox"));
    let mut n = 2;
    while dest.exists() {
        dest = parent.join(format!("{base}-{n}.breadbox"));
        n += 1;
    }
    std::fs::rename(&top, &dest)?;
    let _ = std::fs::remove_dir_all(&tmp);
    // re-point images that were bundled under firmware/prebuilt
    if let Ok(mut p) = Project::load(&dest) {
        let mut changed = false;
        for part in p.parts.iter_mut() {
            if let Some(fw) = part.firmware.as_mut()
                && let Some(img) = fw.image.clone()
                && !dest.join(&img).exists()
                && let Some(fname) = Path::new(&img).file_name()
            {
                let local = Path::new("firmware/prebuilt").join(fname);
                if dest.join(&local).exists() {
                    fw.image = Some(local.to_string_lossy().into_owned());
                    changed = true;
                }
            }
        }
        if changed {
            p.save(&dest)?;
        }
    }
    Ok(dest)
}

/// ngspice-compatible netlist of the circuit.
pub fn cir(project: &Project, lib: &Library) -> Result<String> {
    bb_circuit::spice_io::export_cir(project, lib).map_err(|e| anyhow::anyhow!("{e}"))
}

/// Bill of materials: one row per part kind + value, designators merged.
pub fn bom_csv(project: &Project, lib: &Library) -> String {
    let mut rows: BTreeMap<(String, String), (Vec<String>, String, String)> = BTreeMap::new();
    for p in &project.parts {
        let Some(def) = lib.get(&p.part) else {
            continue;
        };
        if def.bench.hidden || def.id == "jumper_wire" {
            continue;
        }
        let value = def
            .props
            .iter()
            .filter(|pd| !matches!(pd.key.as_str(), "position" | "pot" | "on" | "charge"))
            .take(2)
            .filter_map(|pd| {
                Project::prop(def, p, &pd.key).map(|v| match v.as_f64() {
                    Some(x) if !pd.unit.is_empty() => bb_core::si::format(x, &pd.unit),
                    _ => v.as_text(),
                })
            })
            .collect::<Vec<_>>()
            .join(" ");
        let e = rows
            .entry((def.name.clone(), value.clone()))
            .or_insert_with(|| (Vec::new(), def.part_number.clone(), def.description.clone()));
        e.0.push(p.reference.clone());
    }
    let q = |s: &str| {
        if s.contains([',', '"', '\n']) {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s.to_string()
        }
    };
    let mut out = String::from("References,Qty,Part,Value,Part number,Description\n");
    let wires = project.wires.len();
    for ((name, value), (refs, pn, desc)) in rows {
        out.push_str(&format!(
            "{},{},{},{},{},{}\n",
            q(&refs.join(" ")),
            refs.len(),
            q(&name),
            q(&value),
            q(&pn),
            q(&desc)
        ));
    }
    if wires > 0 {
        out.push_str(&format!(",{wires},Jumper wire,,,Dupont male-male\n"));
    }
    out
}

/// The bundled fonts (Inter, JetBrains Mono, Space Grotesk) as a resvg font database, built
/// once. SVG text (schematic exports, plugin part art) renders with these and nothing else —
/// never with whatever fonts the Mac happens to have.
pub fn svg_fonts() -> std::sync::Arc<resvg::usvg::fontdb::Database> {
    static DB: std::sync::OnceLock<std::sync::Arc<resvg::usvg::fontdb::Database>> =
        std::sync::OnceLock::new();
    DB.get_or_init(|| {
        let mut db = resvg::usvg::fontdb::Database::new();
        for f in [
            crate::theme::INTER_REGULAR,
            crate::theme::INTER_MEDIUM_TTF,
            crate::theme::INTER_SEMIBOLD_TTF,
            crate::theme::MONO_REGULAR,
            crate::theme::MONO_MEDIUM_TTF,
            crate::theme::GROTESK_VARIABLE,
        ] {
            db.load_font_data(f.to_vec());
        }
        db.set_sans_serif_family("Inter");
        db.set_monospace_family("JetBrains Mono");
        std::sync::Arc::new(db)
    })
    .clone()
}

/// usvg options over [`svg_fonts`] with `default_family` for text without a usable family.
pub fn svg_options(default_family: &str) -> resvg::usvg::Options<'static> {
    resvg::usvg::Options {
        fontdb: svg_fonts(),
        font_family: default_family.into(),
        ..Default::default()
    }
}

/// Rasterises an SVG (the schematic export) to PNG with the bundled fonts.
pub fn svg_to_png(svg: &str, scale: f32) -> Result<Vec<u8>> {
    let opt = svg_options("JetBrains Mono");
    let tree = resvg::usvg::Tree::from_str(svg, &opt).context("svg")?;
    let size = tree.size();
    let w = (size.width() * scale).ceil().max(1.0) as u32;
    let h = (size.height() * scale).ceil().max(1.0) as u32;
    let mut pix = resvg::tiny_skia::Pixmap::new(w, h).context("pixmap")?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pix.as_mut(),
    );
    pix.encode_png().context("png")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples")
            .join(name)
    }

    #[test]
    fn bundle_round_trips_through_zip() {
        let doc = CircuitDoc::load(&example("garden-node"), true).unwrap();
        let tmp = std::env::temp_dir().join(format!("bb-bundle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let z = tmp.join("garden-node.breadbox.zip");
        bundle_zip(&doc, &z).unwrap();
        let dir = import_bundle(&z, &tmp).unwrap();
        assert!(dir.ends_with("garden-node.breadbox"));
        let p = Project::load(&dir).unwrap();
        assert_eq!(p.parts.len(), doc.project().parts.len());
        assert!(dir.join("firmware/U1/garden-node/garden-node.ino").exists());
        let img = p
            .part("u1")
            .unwrap()
            .firmware
            .as_ref()
            .unwrap()
            .image
            .clone()
            .unwrap();
        assert!(
            dir.join(&img).exists(),
            "prebuilt image travels with the bundle: {img}"
        );
        // a second import gets a fresh name
        let again = import_bundle(&z, &tmp).unwrap();
        assert!(again.ends_with("garden-node-2.breadbox"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn bom_merges_designators() {
        let doc = CircuitDoc::load(&example("garden-node"), true).unwrap();
        let csv = bom_csv(doc.project(), &doc.lib);
        assert!(csv.starts_with("References,Qty,Part"));
        assert!(csv.lines().any(|l| l.starts_with("R1,1,Resistor")), "{csv}");
        assert!(csv.contains("Jumper wire"));
        let cir = cir(doc.project(), &doc.lib).unwrap();
        assert!(cir.to_ascii_lowercase().contains(".tran"));
    }

    #[test]
    fn svg_rasterises() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20" viewBox="0 0 40 20"><rect width="40" height="20" fill="#F6F5F0"/><text x="2" y="14" font-family="JetBrains Mono" font-size="10">R1</text></svg>"##;
        let png = svg_to_png(svg, 2.0).unwrap();
        assert_eq!(&png[1..4], b"PNG");
    }
}
