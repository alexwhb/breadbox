//! # bb-lessons — guided lessons (LES-1, LES-3, LES-4, model side of LES-5/LES-6)
//!
//! Lessons are TOML files (`lessons/<track>/<lesson>.toml`, `docs/LESSON-FORMAT.md`) with a
//! start circuit (a real bb-circuit project), steps of eight kinds (explain, build, predict,
//! measure, experiment, break, quiz, code), checkers evaluated against the circuit and the
//! running bench, hints and worked solutions.
//!
//! * [`Catalog`] loads tracks from `lessons/` and lesson packs from plugins;
//! * [`session::Session`] is the state machine behind the Lessons screen (choices, the
//!   "Do the math myself" prediction with hints and working, the measurements table,
//!   quizzes, step navigation, "Open in free play");
//! * [`check`] evaluates goals against `bb_circuit` (connectivity, parts, properties) and
//!   `bb_sim` (readings, LEDs, damage, serial output, pin frequency/duty, MQTT, SPICE);
//! * [`runner::Runner`] couples a session with a headless bench and replays worked solutions —
//!   the scripted solver that proves every lesson can be completed;
//! * [`progress::Progress`] persists per-user progress;
//! * [`editor::Draft`] is the "Make your own lesson" model: capture the bench, add steps and
//!   checkers by picking parts/nets/instruments, save a lesson pack plugin.

pub mod apply;
pub mod check;
pub mod code;
pub mod diagnose;
pub mod editor;
pub mod format;
pub mod guide;
pub mod names;
pub mod progress;
pub mod runner;
pub mod session;
pub mod text;

pub use format::{Action, Check, Lesson, Step, TrackDef};
pub use progress::Progress;
pub use runner::Runner;
pub use session::Session;

use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq)]
pub enum LessonError {
    #[error("{}: {msg}", .path.display())]
    Io { path: PathBuf, msg: String },
    #[error("{lesson}: {msg}")]
    Format { lesson: String, msg: String },
    #[error("{lesson}: {msg}")]
    Invalid { lesson: String, msg: String },
    #[error("{0}")]
    Bench(String),
    #[error("no lesson `{0}`")]
    NotFound(String),
}

pub(crate) fn io(path: &Path, e: impl std::fmt::Display) -> LessonError {
    LessonError::Io {
        path: path.to_path_buf(),
        msg: e.to_string(),
    }
}

/// A lesson with the folder it came from (bench files and sketches are relative to it).
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedLesson {
    pub lesson: Lesson,
    pub dir: PathBuf,
    /// The file it was read from.
    pub file: PathBuf,
    /// Lesson pack plugin id, for lessons from plugins.
    pub pack: Option<String>,
}

impl LoadedLesson {
    /// Parses a lesson file.
    pub fn load(file: &Path) -> Result<LoadedLesson, LessonError> {
        let text = std::fs::read_to_string(file).map_err(|e| io(file, e))?;
        let lesson = parse(&text, &file.display().to_string())?;
        Ok(LoadedLesson {
            lesson,
            dir: file.parent().map(Path::to_path_buf).unwrap_or_default(),
            file: file.to_path_buf(),
            pack: None,
        })
    }

    /// The start circuit (LES-4): the lesson's bench file or an example project.
    pub fn start_project(&self) -> Result<bb_circuit::Project, LessonError> {
        let b = &self.lesson.bench;
        let mut p = if let Some(f) = &b.project {
            let path = self.dir.join(f);
            let text = std::fs::read_to_string(&path).map_err(|e| io(&path, e))?;
            serde_json::from_str::<bb_circuit::Project>(&text).map_err(|e| LessonError::Format {
                lesson: self.lesson.id.clone(),
                msg: format!("{f}: {e}"),
            })?
        } else if let Some(ex) = &b.example {
            let dir = examples_dir().join(ex);
            bb_circuit::Project::load(&dir).map_err(|e| LessonError::Bench(format!("{ex}: {e}")))?
        } else {
            return Err(LessonError::Invalid {
                lesson: self.lesson.id.clone(),
                msg: "no start circuit (`bench.project` or `bench.example`)".into(),
            });
        };
        p.name = self.lesson.title.clone();
        Ok(p)
    }

    /// Firmware images for the start circuit's MCUs: the example's or the bench file's
    /// `firmware.image` paths, plus `bench.firmware` overrides (part id → lesson-relative file).
    pub fn images(
        &self,
        project: &bb_circuit::Project,
    ) -> Result<std::collections::HashMap<String, bb_avr::FirmwareImage>, LessonError> {
        let b = &self.lesson.bench;
        let mut out = match &b.example {
            Some(ex) if b.project.is_none() => {
                bb_sim::firmware::project_images(project, examples_dir().join(ex))
            }
            _ => bb_sim::firmware::project_images(project, &self.dir),
        };
        for (part, path) in &b.firmware {
            let p = self.resolve(path);
            let img = bb_sim::firmware::load_image(&p)
                .map_err(|e| LessonError::Bench(format!("{}: {e}", p.display())))?;
            out.insert(part.clone(), img);
        }
        Ok(out)
    }

    /// Builds the sketches of API-level boards (ESP32, Pico) in the start circuit with the host
    /// C++ compiler (cached; the first build takes a few seconds). Part id → executable, for
    /// `BenchOptions::host_firmware`. Empty when the circuit has no such board.
    pub fn host_firmware(
        &self,
        project: &bb_circuit::Project,
        lib: &bb_circuit::Library,
    ) -> Result<std::collections::HashMap<String, PathBuf>, LessonError> {
        let needs = project.parts.iter().any(|p| {
            p.firmware.is_some()
                && lib
                    .get(&p.part)
                    .and_then(|d| d.model.mcu.as_ref())
                    .is_some_and(|m| m.core == "host")
        });
        if !needs {
            return Ok(Default::default());
        }
        let b = &self.lesson.bench;
        let dir = match &b.example {
            Some(ex) if b.project.is_none() => examples_dir().join(ex),
            _ => self.dir.clone(),
        };
        bb_sim::firmware::compile_host_project(project, &dir, lib)
            .map_err(|e| LessonError::Bench(e.to_string()))
    }

    /// Resolves a lesson-relative path (or `examples/…` from the repository / app resources).
    pub fn resolve(&self, rel: &str) -> PathBuf {
        let local = self.dir.join(rel);
        if local.exists() {
            return local;
        }
        if let Some(r) = rel.strip_prefix("examples/") {
            return examples_dir().join(r);
        }
        local
    }
}

/// `examples/`: `$BREADBOX_EXAMPLES`, else the app resources (`Breadbox.app/Contents/Resources`
/// or the repository, see [`bb_core::paths`]).
pub fn examples_dir() -> PathBuf {
    if let Ok(d) = std::env::var("BREADBOX_EXAMPLES") {
        return PathBuf::from(d);
    }
    bb_core::paths::resource_path("examples")
}

/// `lessons/`: `$BREADBOX_LESSONS`, else the app resources.
pub fn lessons_dir() -> PathBuf {
    if let Ok(d) = std::env::var("BREADBOX_LESSONS") {
        return PathBuf::from(d);
    }
    bb_core::paths::resource_path("lessons")
}

/// Parses lesson TOML.
pub fn parse(text: &str, name: &str) -> Result<Lesson, LessonError> {
    toml::from_str::<Lesson>(text).map_err(|e| LessonError::Format {
        lesson: name.into(),
        msg: e.to_string(),
    })
}

/// A track with its lessons (in order).
#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub def: TrackDef,
    pub lessons: Vec<LoadedLesson>,
}

/// Every track: built-in ones from `lessons/` plus lesson packs.
#[derive(Debug, Clone, Default)]
pub struct Catalog {
    pub tracks: Vec<Track>,
    /// Files that failed to load.
    pub errors: Vec<LessonError>,
}

impl Catalog {
    /// Loads `dir/<track>/track.toml` + `*.toml` lessons.
    pub fn load_dir(dir: &Path) -> Catalog {
        let mut c = Catalog::default();
        c.add_dir(dir, None);
        c
    }

    /// The built-in tracks.
    pub fn builtin() -> Catalog {
        Catalog::load_dir(&lessons_dir())
    }

    /// Adds the tracks of a folder (a lesson pack's `lessons/` when `pack` is set).
    pub fn add_dir(&mut self, dir: &Path, pack: Option<&str>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            self.errors.push(io(dir, "cannot read"));
            return;
        };
        let mut dirs: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.join("track.toml").exists())
            .collect();
        dirs.sort();
        for d in dirs {
            let tf = d.join("track.toml");
            let def: TrackDef = match std::fs::read_to_string(&tf)
                .map_err(|e| io(&tf, e))
                .and_then(|t| {
                    toml::from_str(&t).map_err(|e| LessonError::Format {
                        lesson: tf.display().to_string(),
                        msg: e.to_string(),
                    })
                }) {
                Ok(d) => d,
                Err(e) => {
                    self.errors.push(e);
                    continue;
                }
            };
            let mut files: Vec<PathBuf> = std::fs::read_dir(&d)
                .map(|rd| {
                    rd.flatten()
                        .map(|e| e.path())
                        .filter(|p| {
                            p.extension().is_some_and(|x| x == "toml")
                                && p.file_name().is_some_and(|n| n != "track.toml")
                        })
                        .collect()
                })
                .unwrap_or_default();
            files.sort();
            let mut lessons = Vec::new();
            for f in files {
                match LoadedLesson::load(&f) {
                    Ok(mut l) => {
                        l.pack = pack.map(String::from);
                        lessons.push(l);
                    }
                    Err(e) => self.errors.push(e),
                }
            }
            lessons.sort_by_key(|l| l.lesson.order);
            match self.tracks.iter_mut().find(|t| t.def.id == def.id) {
                Some(t) => {
                    t.lessons.extend(lessons);
                    t.lessons.sort_by_key(|l| l.lesson.order);
                }
                None => self.tracks.push(Track { def, lessons }),
            }
        }
        self.tracks.sort_by_key(|t| t.def.order);
    }

    /// Adds lesson packs from plugins (`PluginHost::lesson_dirs`).
    pub fn add_packs(&mut self, dirs: &[(String, PathBuf)]) {
        for (id, d) in dirs {
            self.add_dir(d, Some(id));
        }
    }

    pub fn lesson(&self, id: &str) -> Option<&LoadedLesson> {
        self.tracks
            .iter()
            .flat_map(|t| t.lessons.iter())
            .find(|l| l.lesson.id == id)
    }

    pub fn track(&self, id: &str) -> Option<&Track> {
        self.tracks.iter().find(|t| t.def.id == id)
    }

    /// Every lesson in track order.
    pub fn all(&self) -> impl Iterator<Item = &LoadedLesson> {
        self.tracks.iter().flat_map(|t| t.lessons.iter())
    }
}

/// Validates a lesson against the format rules and the parts library: step kinds, the
/// contents each kind needs, part ids used by checks/choices exist in the start circuit or are
/// added by a solution, expressions parse, the start circuit loads.
pub fn validate(l: &LoadedLesson, lib: &bb_circuit::Library) -> Vec<String> {
    validate_lesson(&l.lesson, l.start_project(), lib)
}

/// [`validate`] for a lesson and its (loaded) start circuit.
pub fn validate_lesson(
    lesson: &Lesson,
    start: Result<bb_circuit::Project, LessonError>,
    lib: &bb_circuit::Library,
) -> Vec<String> {
    let mut issues = Vec::new();
    if lesson.steps.is_empty() {
        issues.push("no steps".into());
    }
    let project = match start {
        Ok(p) => Some(p),
        Err(e) => {
            issues.push(format!("start circuit: {e}"));
            None
        }
    };
    let mut known: Vec<String> = project
        .as_ref()
        .map(|p| p.parts.iter().map(|x| x.id.clone()).collect())
        .unwrap_or_default();
    if let Some(p) = &project {
        for part in &p.parts {
            if lib.get(&part.part).is_none() {
                issues.push(format!("start circuit uses unknown part `{}`", part.part));
            }
        }
    }
    for (i, s) in lesson.steps.iter().enumerate() {
        let at = format!("step {} ({})", i + 1, s.title);
        if !format::STEP_KINDS.contains(&s.kind.as_str()) {
            issues.push(format!("{at}: unknown kind `{}`", s.kind));
        }
        let needs = |ok: bool, what: &str, issues: &mut Vec<String>| {
            if !ok {
                issues.push(format!("{at}: a `{}` step needs {what}", s.kind));
            }
        };
        match s.kind.as_str() {
            "build" => needs(!s.checks.is_empty(), "checks", &mut issues),
            "predict" => needs(s.predict.is_some(), "`predict`", &mut issues),
            "measure" => needs(
                s.measure.is_some() || !s.checks.is_empty(),
                "`measure` or checks",
                &mut issues,
            ),
            "experiment" => needs(
                s.choice.is_some() || !s.checks.is_empty(),
                "a `choice` or checks",
                &mut issues,
            ),
            "break" => needs(
                s.checks.iter().any(|c| matches!(c, Check::Damaged { .. })),
                "a `damaged` check",
                &mut issues,
            ),
            "quiz" => needs(s.quiz.is_some(), "`quiz`", &mut issues),
            "code" => needs(s.code.is_some(), "`code`", &mut issues),
            _ => {}
        }
        if let Some(q) = &s.quiz
            && q.answer >= q.options.len()
        {
            issues.push(format!("{at}: quiz answer out of range"));
        }
        if let Some(p) = &s.predict {
            if bb_spice::expr::Expr::parse(&p.answer).is_err() {
                issues.push(format!("{at}: answer `{}` does not parse", p.answer));
            }
            for h in &p.hints {
                if bb_spice::expr::Expr::parse(&h.when).is_err() {
                    issues.push(format!("{at}: hint condition `{}` does not parse", h.when));
                }
            }
        }
        // parts the solution adds become known
        for a in &s.solution {
            if let Action::AddPart { reference, def, .. } = a {
                known.push(reference.to_ascii_lowercase());
                if lib.get(def).is_none() {
                    issues.push(format!("{at}: solution adds unknown part `{def}`"));
                }
            }
        }
        let has = |part: &str| known.iter().any(|k| k.eq_ignore_ascii_case(part));
        let pin_part = |s: &str| s.split('.').next().unwrap_or("").to_string();
        let mut refs: Vec<String> = Vec::new();
        if let Some(c) = &s.choice {
            refs.push(c.part.clone());
        }
        for c in &s.checks {
            match c {
                Check::Connected { a, b } | Check::NotConnected { a, b } => {
                    refs.push(pin_part(a));
                    refs.push(pin_part(b));
                }
                Check::Prop { part, .. }
                | Check::Current { part, .. }
                | Check::Reading { part, .. }
                | Check::LedLit { part, .. }
                | Check::LedOff { part }
                | Check::Damaged { part }
                | Check::Healthy { part } => refs.push(part.clone()),
                Check::SerialContains { mcu, .. }
                | Check::PinFrequency { mcu, .. }
                | Check::PinDuty { mcu, .. }
                | Check::Firmware { mcu } => refs.push(mcu.clone()),
                Check::Voltage { a, b, .. } => {
                    refs.push(pin_part(a));
                    if let Some(b) = b {
                        refs.push(pin_part(b));
                    }
                }
                Check::Spice { analysis, .. } if !analysis.trim_start().starts_with('.') => {
                    issues.push(format!("{at}: spice analysis `{analysis}` is not a card"));
                }
                _ => {}
            }
        }
        for r in refs {
            if !r.is_empty() && !has(&r) {
                issues.push(format!("{at}: part `{r}` is not on the bench"));
            }
        }
        // steps that ask the student to *do* something need a worked solution (reading a
        // meter or a SPICE result needs none)
        if s.solution.is_empty()
            && matches!(
                s.kind.as_str(),
                "build" | "code" | "experiment" | "break" | "predict" | "quiz"
            )
        {
            issues.push(format!("{at}: no worked solution"));
        }
    }
    issues
}
