//! "Make your own lesson" (LES-6, model side): a [`Draft`] captures the bench as the start
//! circuit, holds the steps, offers checkers for what the author picks on the bench (a part, two
//! pins, an instrument) and saves the lesson as a lesson-pack plugin folder that the plugin
//! registry installs and the [`crate::Catalog`] loads (`PluginHost::lesson_dirs`).

use crate::format::{BenchSpec, Check, Lesson, Step, TrackDef};
use crate::{LessonError, LoadedLesson, io};
use bb_circuit::connectivity::Connectivity;
use bb_circuit::project::Mode;
use bb_circuit::{Library, Project, PropValue};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A checker the editor offers for a picked thing.
#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    /// `R1 is 220 Ω`, `D1 is lit`, `dmm1 reads 13.6 mA (±5 %)`.
    pub label: String,
    pub check: Check,
}

/// What a lesson pack is called.
#[derive(Debug, Clone, PartialEq)]
pub struct PackInfo {
    /// Plugin id (`me.my-lessons`).
    pub id: String,
    pub name: String,
    pub version: String,
    pub author: String,
    pub summary: String,
}

/// A lesson being written.
#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    pub track: TrackDef,
    pub lesson: Lesson,
    /// The start circuit.
    pub project: Project,
}

fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

fn pct(v: f64, p: f64) -> (f64, f64) {
    let d = (v.abs() * p).max(1e-9);
    (v - d, v + d)
}

impl Draft {
    /// A new lesson in `track` with an empty bench.
    pub fn new(title: &str, track: TrackDef) -> Draft {
        let id = slug(title);
        Draft {
            lesson: Lesson {
                id: id.clone(),
                track: track.id.clone(),
                order: 1,
                title: title.into(),
                summary: String::new(),
                minutes: 10,
                bench: BenchSpec {
                    project: Some(format!("{id}.bench.json")),
                    ..BenchSpec::default()
                },
                steps: Vec::new(),
            },
            track,
            project: Project::new(title, Mode::Playground),
        }
    }

    /// Edits an existing lesson.
    pub fn open(l: &LoadedLesson, track: TrackDef) -> Result<Draft, LessonError> {
        let project = l.start_project()?;
        let mut lesson = l.lesson.clone();
        lesson.bench.project = Some(format!("{}.bench.json", lesson.id));
        lesson.bench.example = None;
        Ok(Draft {
            track,
            lesson,
            project,
        })
    }

    /// "Use this bench as the start circuit".
    pub fn capture_bench(&mut self, project: &Project) {
        let mut p = project.clone();
        p.name = self.lesson.title.clone();
        self.project = p;
    }

    pub fn add_step(&mut self, kind: &str, title: &str) -> usize {
        self.lesson.steps.push(Step {
            kind: kind.into(),
            title: title.into(),
            ..Step::default()
        });
        self.lesson.steps.len() - 1
    }

    pub fn remove_step(&mut self, i: usize) {
        if i < self.lesson.steps.len() {
            self.lesson.steps.remove(i);
        }
    }

    pub fn move_step(&mut self, from: usize, to: usize) {
        if from < self.lesson.steps.len() && to < self.lesson.steps.len() {
            let s = self.lesson.steps.remove(from);
            self.lesson.steps.insert(to, s);
        }
    }

    pub fn step_mut(&mut self, i: usize) -> Option<&mut Step> {
        self.lesson.steps.get_mut(i)
    }

    pub fn add_check(&mut self, step: usize, check: Check) {
        if let Some(s) = self.lesson.steps.get_mut(step) {
            s.checks.push(check);
        }
    }

    /// Checkers for a picked part: it exists (with its values), each numeric property, LEDs lit /
    /// healthy, MCUs running firmware; `readings` (the bench's current readings of the part —
    /// instruments, sensors) each become a ±5 % reading check.
    pub fn suggest_for_part(
        &self,
        lib: &Library,
        part: &str,
        readings: &[(String, f64)],
    ) -> Vec<Suggestion> {
        let mut out = Vec::new();
        let Some(inst) = self.project.part(part) else {
            return out;
        };
        let Some(def) = lib.get(&inst.part) else {
            return out;
        };
        let r = inst.reference.clone();
        let mut props = BTreeMap::new();
        for pd in &def.props {
            if let Some(PropValue::Num(v)) = Project::prop(def, inst, &pd.key) {
                props.insert(pd.key.clone(), v);
                let (min, max) = pct(v, 0.01);
                out.push(Suggestion {
                    label: format!(
                        "{r} {} is {}",
                        pd.label.to_lowercase(),
                        bb_core::si::format(v, &pd.unit)
                    ),
                    check: Check::Prop {
                        part: inst.id.clone(),
                        prop: pd.key.clone(),
                        min,
                        max,
                    },
                });
            }
        }
        out.insert(
            0,
            Suggestion {
                label: format!("A {} is on the bench", def.name),
                check: Check::Part {
                    def: def.id.clone(),
                    id: Some(inst.id.clone()),
                    props,
                    count: 1,
                },
            },
        );
        let is_led = def.ref_prefix == "D"
            && (def.id.to_ascii_lowercase().contains("led") || def.name.contains("LED"));
        if is_led {
            out.push(Suggestion {
                label: format!("{r} is lit"),
                check: Check::LedLit {
                    part: inst.id.clone(),
                    min_ma: 1.0,
                },
            });
            out.push(Suggestion {
                label: format!("{r} is dark"),
                check: Check::LedOff {
                    part: inst.id.clone(),
                },
            });
            out.push(Suggestion {
                label: format!("{r} burns out"),
                check: Check::Damaged {
                    part: inst.id.clone(),
                },
            });
        }
        if !def.ratings.is_empty() {
            out.push(Suggestion {
                label: format!("{r} survives"),
                check: Check::Healthy {
                    part: inst.id.clone(),
                },
            });
        }
        if def.is_mcu() {
            out.push(Suggestion {
                label: format!("{r} runs the step's sketch"),
                check: Check::Firmware {
                    mcu: inst.id.clone(),
                },
            });
        }
        for (k, v) in readings {
            if !v.is_finite() {
                continue;
            }
            let (min, max) = pct(*v, 0.05);
            out.push(Suggestion {
                label: format!(
                    "{} {k} reads {} (±5 %)",
                    inst.id,
                    bb_core::si::format(*v, "")
                ),
                check: Check::Reading {
                    part: inst.id.clone(),
                    key: k.clone(),
                    min,
                    max,
                },
            });
        }
        out
    }

    /// Checkers for two picked pins (`part.pin`): connected or not (as they are now) and, with a
    /// measured voltage between them, a ±5 % voltage check.
    pub fn suggest_for_pins(
        &self,
        lib: &Library,
        a: &str,
        b: &str,
        volts: Option<f64>,
    ) -> Vec<Suggestion> {
        let conn = Connectivity::compute(&self.project, lib);
        let net = |s: &str| {
            let (p, n) = s.split_once('.').unwrap_or((s, ""));
            conn.net_of(p, n)
        };
        let joined = matches!((net(a), net(b)), (Some(x), Some(y)) if x == y);
        let mut out = vec![if joined {
            Suggestion {
                label: format!("{a} is wired to {b}"),
                check: Check::Connected {
                    a: a.into(),
                    b: b.into(),
                },
            }
        } else {
            Suggestion {
                label: format!("{a} is not wired to {b}"),
                check: Check::NotConnected {
                    a: a.into(),
                    b: b.into(),
                },
            }
        }];
        if let Some(v) = volts.filter(|v| v.is_finite()) {
            let (min, max) = pct(v, 0.05);
            out.push(Suggestion {
                label: format!("{a} is {} above {b} (±5 %)", bb_core::si::format(v, "V")),
                check: Check::Voltage {
                    a: a.into(),
                    b: Some(b.into()),
                    min,
                    max,
                },
            });
        }
        out
    }

    /// Problems that would stop the lesson from loading or being completed.
    pub fn validate(&self, lib: &Library) -> Vec<String> {
        crate::validate_lesson(&self.lesson, Ok(self.project.clone()), lib)
    }

    /// The lesson file.
    pub fn to_toml(&self) -> Result<String, LessonError> {
        toml::to_string(&self.lesson).map_err(|e| LessonError::Format {
            lesson: self.lesson.id.clone(),
            msg: e.to_string(),
        })
    }

    /// Writes the lesson into a lesson-pack plugin folder (`plugin.json`,
    /// `lessons/<track>/track.toml`, `<lesson>.toml`, `<lesson>.bench.json`), keeping the pack's
    /// other lessons. Returns the lesson file.
    pub fn save_pack(&self, dir: &Path, pack: &PackInfo) -> Result<PathBuf, LessonError> {
        let tdir = dir.join("lessons").join(&self.track.id);
        std::fs::create_dir_all(&tdir).map_err(|e| io(&tdir, e))?;
        let write = |p: PathBuf, text: String| -> Result<(), LessonError> {
            std::fs::write(&p, text).map_err(|e| io(&p, e))
        };
        let mut lesson = self.lesson.clone();
        lesson.track = self.track.id.clone();
        let bench = format!("{}.bench.json", lesson.id);
        lesson.bench.project = Some(bench.clone());
        lesson.bench.example = None;
        let track_toml = toml::to_string(&self.track).map_err(|e| LessonError::Format {
            lesson: self.track.id.clone(),
            msg: e.to_string(),
        })?;
        write(tdir.join("track.toml"), track_toml)?;
        let project = &self.project;
        write(
            tdir.join(&bench),
            serde_json::to_string_pretty(project).unwrap_or_default(),
        )?;
        let file = tdir.join(format!("{:02}-{}.toml", lesson.order, lesson.id));
        let text = toml::to_string(&lesson).map_err(|e| LessonError::Format {
            lesson: lesson.id.clone(),
            msg: e.to_string(),
        })?;
        write(file.clone(), text)?;
        let manifest = serde_json::json!({
            "id": pack.id,
            "version": pack.version,
            "name": pack.name,
            "kind": ["lesson-pack"],
            "summary": pack.summary,
            "author": pack.author,
            "provides": { "lessons": "lessons" },
            "permissions": []
        });
        let mpath = dir.join("plugin.json");
        write(
            mpath.clone(),
            serde_json::to_string_pretty(&manifest).unwrap_or_default(),
        )?;
        bb_plugin::manifest::Manifest::load(dir).map_err(|e| io(&mpath, e))?;
        Ok(file)
    }

    /// Saves and zips the pack (`.bbplugin` bytes) for sharing.
    pub fn package(&self, dir: &Path, pack: &PackInfo) -> Result<Vec<u8>, LessonError> {
        self.save_pack(dir, pack)?;
        bb_plugin::package::pack(dir).map_err(|e| io(dir, e))
    }
}
