//! Lightweight project access for the app shell: reads `project.json` as plain JSON (name,
//! mode, description, MCUs + their sketch folders, radio modules). The full document model is
//! `bb-circuit`'s; screens that edit circuits use that.

use crate::bench::BenchTarget;
use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Playground (breadboard bench) or Pro (SPICE workbench).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash, Default)]
pub enum ProjectMode {
    #[default]
    Playground,
    Pro,
}

impl ProjectMode {
    pub fn label(self) -> &'static str {
        match self {
            ProjectMode::Playground => "Playground",
            ProjectMode::Pro => "Pro",
        }
    }
    fn from_json(s: &str) -> Self {
        if s.eq_ignore_ascii_case("pro") {
            ProjectMode::Pro
        } else {
            ProjectMode::Playground
        }
    }
    fn json(self) -> &'static str {
        match self {
            ProjectMode::Playground => "playground",
            ProjectMode::Pro => "pro",
        }
    }
}

/// An MCU part with its firmware folder.
#[derive(Debug, Clone, PartialEq)]
pub struct McuRef {
    /// `U1`.
    pub reference: String,
    /// Part id (`uno`, `esp32_devkit`, `pico`).
    pub part: String,
    pub sketch_dir: Option<PathBuf>,
    /// Prebuilt image referenced by the project (`../fixtures/garden-node.uno.hex`).
    pub image: Option<PathBuf>,
}

/// Board facts for a part id.
pub struct BoardKind {
    pub name: &'static str,
    pub mcu: &'static str,
    pub fqbn: Option<&'static str>,
    pub emulation: &'static str,
    /// Suffix of prebuilt fixtures (`blink.uno.elf`).
    pub fixture_tag: &'static str,
}

pub fn board_kind(part: &str) -> Option<BoardKind> {
    let k = |name, mcu, fqbn, emulation, fixture_tag| {
        Some(BoardKind {
            name,
            mcu,
            fqbn,
            emulation,
            fixture_tag,
        })
    };
    match part {
        "uno" => k(
            "UNO R3",
            "ATmega328P",
            Some("arduino:avr:uno"),
            "cycle-accurate AVR core",
            "uno",
        ),
        "nano" => k(
            "Nano",
            "ATmega328P",
            Some("arduino:avr:nano"),
            "cycle-accurate AVR core",
            "nano",
        ),
        "mega" | "mega2560" => k(
            "Mega 2560",
            "ATmega2560",
            Some("arduino:avr:mega"),
            "cycle-accurate AVR core",
            "mega",
        ),
        "esp32" | "esp32_devkit" => k("ESP32 DevKit v1", "ESP32", None, "API-level", "esp32"),
        "pico" => k("Raspberry Pi Pico", "RP2040", None, "API-level", "pico"),
        _ => None,
    }
}

/// Radio module part ids the bench knows.
pub const RADIO_PARTS: &[&str] = &["esp01", "hm10", "hc05", "rylr896"];

/// An open project.
#[derive(Debug, Clone)]
pub struct Project {
    pub dir: PathBuf,
    pub name: String,
    pub mode: ProjectMode,
    pub description: String,
    /// Bundled examples open read-only (Save As copies them).
    pub read_only: bool,
    pub mcus: Vec<McuRef>,
    /// `(ref, part)` of radio modules.
    pub radios: Vec<(String, String)>,
    pub json: serde_json::Value,
}

impl Project {
    /// Loads `<dir>/project.json`.
    pub fn load(dir: &Path) -> Result<Project> {
        let file = dir.join("project.json");
        let text = std::fs::read_to_string(&file).with_context(|| {
            format!(
                "{} is not a Breadbox project (no project.json)",
                dir.display()
            )
        })?;
        let json: serde_json::Value = serde_json::from_str(&text)
            .with_context(|| format!("{}: invalid JSON", file.display()))?;
        Ok(Project::from_value(dir, json))
    }

    /// Re-reads name, MCUs and radios from the edited circuit (boards added on the bench).
    pub fn refresh_from(&mut self, circuit: &bb_circuit::Project) {
        if let Ok(json) = serde_json::to_value(circuit) {
            let read_only = self.read_only;
            let mcus_before = self.mcus.clone();
            *self = Project::from_value(&self.dir.clone(), json);
            self.read_only = read_only;
            // keep prebuilt images that pointed outside the folder
            for m in self.mcus.iter_mut() {
                if let Some(b) = mcus_before.iter().find(|b| b.reference == m.reference)
                    && m.image.is_none()
                {
                    m.image = b.image.clone();
                }
            }
        }
    }

    /// Builds the view from `project.json` contents.
    pub fn from_value(dir: &Path, json: serde_json::Value) -> Project {
        let name = json["name"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| folder_name(dir));
        let mode = ProjectMode::from_json(json["mode"].as_str().unwrap_or("playground"));
        let description = json["description"].as_str().unwrap_or("").to_string();
        let mut mcus = Vec::new();
        let mut radios = Vec::new();
        if let Some(parts) = json["parts"].as_array() {
            for p in parts {
                let part = p["part"].as_str().unwrap_or("");
                let reference = p["ref"].as_str().unwrap_or("").to_string();
                if board_kind(part).is_some() {
                    let fw = &p["firmware"];
                    let sketch_dir = fw["sketch"]
                        .as_str()
                        .map(|s| dir.join(s))
                        .or_else(|| find_sketch(&dir.join("firmware").join(&reference)));
                    let image = fw["image"].as_str().map(|s| dir.join(s));
                    mcus.push(McuRef {
                        reference,
                        part: part.to_string(),
                        sketch_dir,
                        image,
                    });
                } else if RADIO_PARTS.contains(&part) {
                    radios.push((reference, part.to_string()));
                }
            }
        }
        Project {
            dir: dir.to_path_buf(),
            name,
            mode,
            description,
            read_only: false,
            mcus,
            radios,
            json,
        }
    }

    /// Creates `<parent>/<name>.breadbox/` with a minimal `project.json` (+ an UNO with a
    /// blank sketch for Playground projects).
    pub fn create(parent: &Path, name: &str, mode: ProjectMode) -> Result<Project> {
        let clean: String = name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        if clean.is_empty() {
            bail!("project name is empty");
        }
        let dir = parent.join(format!("{clean}.breadbox"));
        if dir.exists() {
            bail!("{} already exists", dir.display());
        }
        std::fs::create_dir_all(&dir)?;
        let parts = if mode == ProjectMode::Playground {
            let sketch = dir.join("firmware/U1").join(&clean);
            std::fs::create_dir_all(&sketch)?;
            std::fs::write(
                sketch.join(format!("{clean}.ino")),
                "void setup() {\n  Serial.begin(115200);\n  pinMode(LED_BUILTIN, OUTPUT);\n}\n\nvoid loop() {\n  digitalWrite(LED_BUILTIN, HIGH);\n  delay(500);\n  digitalWrite(LED_BUILTIN, LOW);\n  delay(500);\n}\n",
            )?;
            std::fs::write(
                sketch.join("sketch.yaml"),
                "default_fqbn: arduino:avr:uno\n",
            )?;
            serde_json::json!([{
                "id": "u1", "ref": "U1", "part": "uno",
                "bench": {"x": 0.0, "y": 4.0},
                "firmware": {"sketch": format!("firmware/U1/{clean}")}
            }])
        } else {
            std::fs::create_dir_all(dir.join("analyses"))?;
            std::fs::write(
                dir.join("analyses").join(format!("{clean}.cir")),
                starter_netlist(name),
            )?;
            serde_json::json!([])
        };
        let json = serde_json::json!({
            "format": "breadbox/1",
            "name": name,
            "mode": mode.json(),
            "description": "",
            "parts": parts,
            "wires": [],
        });
        std::fs::write(
            dir.join("project.json"),
            serde_json::to_string_pretty(&json)?,
        )?;
        Project::load(&dir)
    }

    /// Copies the project folder to `dest` (Save As for read-only examples). Prebuilt images
    /// the circuit references from outside the folder come along (`firmware/prebuilt/`).
    pub fn save_as(&self, dest: &Path) -> Result<Project> {
        if dest.exists() {
            bail!("{} already exists", dest.display());
        }
        copy_dir(&self.dir, dest)?;
        if let Ok(mut circuit) = bb_circuit::Project::load(dest)
            && localize_firmware(&mut circuit, &self.dir, dest)?
        {
            circuit.save(dest)?;
        }
        let mut p = Project::load(dest)?;
        // project-relative prebuilt images that pointed outside the folder stay valid
        for (m, orig) in p.mcus.iter_mut().zip(&self.mcus) {
            if m.image.as_ref().is_some_and(|i| !i.exists()) {
                m.image = orig.image.clone();
            }
        }
        Ok(p)
    }

    /// Bench targets (MCUs that take firmware).
    pub fn bench_targets(&self) -> Vec<BenchTarget> {
        self.mcus
            .iter()
            .filter_map(|m| {
                let k = board_kind(&m.part)?;
                Some(BenchTarget {
                    id: m.reference.clone(),
                    label: format!("{} · {} ({})", m.reference, k.name, k.mcu),
                    board: k.name.to_string(),
                    fqbn: k.fqbn.map(str::to_string),
                    emulation: k.emulation.to_string(),
                    sketch_dir: m.sketch_dir.clone(),
                })
            })
            .collect()
    }

    /// Prebuilt firmware for an MCU when no toolchain is installed: the project's `image`, or
    /// `examples/fixtures/<sketch>.<board>.elf`.
    pub fn prebuilt_firmware(&self, mcu: &McuRef, fixtures: Option<&Path>) -> Option<PathBuf> {
        let tag = board_kind(&mcu.part)?.fixture_tag;
        let sketch = mcu.sketch_dir.as_ref().map(|d| folder_name(d))?;
        if let Some(dir) = fixtures {
            let elf = dir.join(format!("{sketch}.{tag}.elf"));
            if elf.exists() {
                return Some(elf);
            }
        }
        if let Some(img) = &mcu.image {
            let elf = img.with_extension("elf");
            if elf.exists() {
                return Some(elf);
            }
            if img.exists() {
                return Some(img.clone());
            }
        }
        None
    }
}

fn folder_name(p: &Path) -> String {
    p.file_name()
        .map(|s| {
            s.to_string_lossy()
                .trim_end_matches(".breadbox")
                .to_string()
        })
        .unwrap_or_else(|| "project".into())
}

/// First sub-folder of `dir` containing an `.ino`.
fn find_sketch(dir: &Path) -> Option<PathBuf> {
    let mut entries: Vec<_> = std::fs::read_dir(dir).ok()?.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    entries
        .into_iter()
        .map(|e| e.path())
        .find(|p| p.is_dir() && sketch_files(p).iter().any(|f| f.ends_with(".ino")))
}

/// Source files of a sketch folder: the main `.ino` first, then other tabs alphabetically.
pub fn sketch_files(dir: &Path) -> Vec<String> {
    let main = format!("{}.ino", folder_name(dir));
    let mut files: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().is_file())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| {
                    let l = n.to_ascii_lowercase();
                    [".ino", ".h", ".cpp", ".c", ".hpp"]
                        .iter()
                        .any(|x| l.ends_with(x))
                })
                .collect()
        })
        .unwrap_or_default();
    files.sort_by(|a, b| {
        (a != &main, !a.ends_with(".ino"), a.to_lowercase()).cmp(&(
            b != &main,
            !b.ends_with(".ino"),
            b.to_lowercase(),
        ))
    });
    files
}

/// Makes a circuit saved into `dest` self-contained: prebuilt firmware images it references
/// from outside `dest` (examples point at `../fixtures/…`, relative to `src`) are copied into
/// `dest/firmware/prebuilt/` (with their `.elf` / `.hex` sibling) and re-pointed. Returns
/// whether anything changed.
pub fn localize_firmware(
    circuit: &mut bb_circuit::Project,
    src: &Path,
    dest: &Path,
) -> Result<bool> {
    let mut changed = false;
    for part in circuit.parts.iter_mut() {
        let Some(fw) = part.firmware.as_mut() else {
            continue;
        };
        let Some(img) = fw.image.clone() else {
            continue;
        };
        let rel = Path::new(&img);
        let inside = rel.is_relative()
            && !rel
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir));
        if inside && dest.join(rel).exists() {
            continue;
        }
        let from = if rel.is_absolute() {
            rel.to_path_buf()
        } else {
            src.join(rel)
        };
        let Some(fname) = from.file_name().map(|f| f.to_owned()) else {
            continue;
        };
        if !from.exists() {
            continue;
        }
        let dir = dest.join("firmware/prebuilt");
        std::fs::create_dir_all(&dir)?;
        for ext in ["hex", "elf"] {
            let sib = from.with_extension(ext);
            if sib.exists() {
                std::fs::copy(&sib, dir.join(sib.file_name().unwrap_or_default()))?;
            }
        }
        std::fs::copy(&from, dir.join(&fname))?;
        fw.image = Some(format!("firmware/prebuilt/{}", fname.to_string_lossy()));
        changed = true;
    }
    Ok(changed)
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let name = e.file_name();
        if name == ".breadbox-cache" {
            continue;
        }
        let src = e.path();
        let dst = to.join(&name);
        if src.is_dir() {
            copy_dir(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

/// An entry of the examples gallery.
#[derive(Debug, Clone, PartialEq)]
pub struct ExampleInfo {
    pub dir: PathBuf,
    pub name: String,
    pub description: String,
    pub mode: ProjectMode,
    /// Number of MCUs with firmware.
    pub boards: Vec<String>,
}

/// Scans `<dir>/*/project.json` (name + description only).
pub fn scan_examples(dir: &Path) -> Vec<ExampleInfo> {
    let mut v: Vec<ExampleInfo> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| {
                    let d = e.path();
                    let p = Project::load(&d).ok()?;
                    Some(ExampleInfo {
                        boards: p
                            .mcus
                            .iter()
                            .filter_map(|m| board_kind(&m.part).map(|k| k.name.to_string()))
                            .collect(),
                        dir: d,
                        name: p.name,
                        description: p.description,
                        mode: p.mode,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    // the design's hero example first, then alphabetical
    v.sort_by_key(|e| {
        (
            e.name != "garden-node",
            e.mode == ProjectMode::Pro,
            e.name.clone(),
        )
    });
    v
}

/// Where bundled resources (examples, fixtures, plugins, lessons) live.
#[derive(Debug, Clone, Default)]
pub struct Resources {
    pub root: Option<PathBuf>,
}

impl Resources {
    /// `$BREADBOX_RESOURCES`, `Breadbox.app/Contents/Resources`, or the source checkout in
    /// development builds (`bb_core::paths::resources_root`; a bundled app never reads the
    /// repository).
    pub fn locate() -> Resources {
        Resources {
            root: bb_core::paths::resources_root(),
        }
    }

    /// `<root>/<rel>` if it exists (`docs/PLUGIN-SDK.md`, `licenses`, …).
    pub fn file(&self, rel: &str) -> Option<PathBuf> {
        self.root
            .as_ref()
            .map(|r| r.join(rel))
            .filter(|p| p.exists())
    }

    pub fn examples(&self) -> Option<PathBuf> {
        self.root
            .as_ref()
            .map(|r| r.join("examples"))
            .filter(|p| p.is_dir())
    }

    pub fn fixtures(&self) -> Option<PathBuf> {
        self.examples()
            .map(|e| e.join("fixtures"))
            .filter(|p| p.is_dir())
    }

    /// True if `dir` is inside the bundled examples (opened read-only).
    pub fn is_example(&self, dir: &Path) -> bool {
        match (self.examples(), dir.canonicalize()) {
            (Some(e), Ok(d)) => e.canonicalize().map(|e| d.starts_with(e)).unwrap_or(false),
            _ => false,
        }
    }
}

/// A recent-projects entry (persisted).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecentProject {
    pub path: PathBuf,
    pub name: String,
    pub mode: ProjectMode,
    /// Unix seconds.
    pub opened: u64,
}

/// Adds/moves `p` to the front of the recent list (max 12).
pub fn touch_recent(list: &mut Vec<RecentProject>, p: &Project) {
    list.retain(|r| r.path != p.dir);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    list.insert(
        0,
        RecentProject {
            path: p.dir.clone(),
            name: p.name.clone(),
            mode: p.mode,
            opened: now,
        },
    );
    list.truncate(12);
}

/// Default folder for new projects: `~/Documents/Breadbox`.
pub fn default_projects_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("BREADBOX_PROJECTS_DIR") {
        return PathBuf::from(d);
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Documents/Breadbox")
}

/// The netlist a new Pro project starts with (`analyses/<name>.cir`, autosaved): a comment
/// that says what to do, an RC low-pass to start from, and `.op`.
pub fn starter_netlist(name: &str) -> String {
    format!(
        "* {name} — type a SPICE netlist; it is saved with the project.\n\
         * Analyses on the right add / switch dot-cards (.tran, .dc, .ac, .mc, .noise, .op).\n\
         *\n\
         * For example, an RC low-pass (fc = 1 / (2 pi R C) = 1 kHz):\n\
         * V1 in 0 DC 0 AC 1\n\
         * R1 in out 1k\n\
         * C1 out 0 159n\n\
         .op\n\
         .end\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn examples_are_found_with_garden_node_first() {
        let r = Resources::locate();
        let ex = scan_examples(&r.examples().expect("examples dir"));
        assert!(ex.len() >= 8, "{ex:?}");
        assert_eq!(ex[0].name, "garden-node");
        let blink = ex.iter().find(|e| e.name == "blink").unwrap();
        assert_eq!(blink.boards, vec!["UNO R3".to_string()]);
        assert!(ex.iter().any(|e| e.mode == ProjectMode::Pro));
    }

    #[test]
    fn garden_node_has_an_uno_and_an_esp01() {
        let r = Resources::locate();
        let p = Project::load(&r.examples().unwrap().join("garden-node")).unwrap();
        assert_eq!(p.mcus.len(), 1);
        assert_eq!(p.radios, vec![("U2".to_string(), "esp01".to_string())]);
        let t = p.bench_targets();
        assert_eq!(t[0].label, "U1 · UNO R3 (ATmega328P)");
        let fw = p
            .prebuilt_firmware(&p.mcus[0], r.fixtures().as_deref())
            .unwrap();
        assert!(fw.ends_with("garden-node.uno.elf"));
        let files = sketch_files(p.mcus[0].sketch_dir.as_ref().unwrap());
        assert_eq!(files[0], "garden-node.ino");
        assert!(files.contains(&"wifi_at.h".to_string()));
    }

    #[test]
    fn create_and_save_as() {
        let tmp = std::env::temp_dir().join(format!("bb-proj-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let p = Project::create(&tmp, "my bench", ProjectMode::Playground).unwrap();
        assert_eq!(p.name, "my bench");
        assert_eq!(p.mcus.len(), 1);
        assert!(
            p.mcus[0]
                .sketch_dir
                .as_ref()
                .unwrap()
                .join("my-bench.ino")
                .exists()
        );
        let q = p.save_as(&tmp.join("copy.breadbox")).unwrap();
        assert_eq!(q.mcus.len(), 1);
        let pro = Project::create(&tmp, "filter", ProjectMode::Pro).unwrap();
        assert_eq!(pro.mode, ProjectMode::Pro);
        let mut recent = Vec::new();
        touch_recent(&mut recent, &p);
        touch_recent(&mut recent, &pro);
        touch_recent(&mut recent, &p);
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].name, "my bench");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
