//! Per-user lesson progress (LES-4): which steps and lessons are done, where to resume, and the
//! measurements a student recorded — `~/Library/Application Support/Breadbox/lessons/
//! progress.json`, written atomically.

use crate::session::Row;
use crate::{LessonError, io};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LessonProgress {
    pub steps_done: BTreeSet<usize>,
    /// Step to resume at.
    pub current: usize,
    pub done: bool,
    /// Measurement rows by step.
    #[serde(default)]
    pub measurements: BTreeMap<usize, Vec<Row>>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Progress {
    pub lessons: BTreeMap<String, LessonProgress>,
    #[serde(skip)]
    path: Option<PathBuf>,
}

impl Progress {
    pub fn default_path() -> PathBuf {
        if let Ok(d) = std::env::var("BREADBOX_PROGRESS") {
            return PathBuf::from(d);
        }
        bb_core::paths::app_support().join("lessons/progress.json")
    }

    /// Loads (or starts empty when the file doesn't exist yet).
    pub fn load(path: &Path) -> Result<Progress, LessonError> {
        let mut p = match std::fs::read_to_string(path) {
            Ok(t) => serde_json::from_str(&t).map_err(|e| io(path, e))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Progress::default(),
            Err(e) => return Err(io(path, e)),
        };
        p.path = Some(path.to_path_buf());
        Ok(p)
    }

    pub fn save(&self) -> Result<(), LessonError> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(|e| io(d, e))?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self).unwrap_or_default())
            .map_err(|e| io(&tmp, e))?;
        std::fs::rename(&tmp, path).map_err(|e| io(path, e))
    }

    pub fn lesson(&self, id: &str) -> Option<&LessonProgress> {
        self.lessons.get(id)
    }

    pub fn step_done(&mut self, lesson: &str, step: usize) {
        let l = self.lessons.entry(lesson.into()).or_default();
        l.steps_done.insert(step);
        l.current = l.current.max(step + 1);
    }

    pub fn set_current(&mut self, lesson: &str, step: usize) {
        self.lessons.entry(lesson.into()).or_default().current = step;
    }

    pub fn record(&mut self, lesson: &str, step: usize, rows: Vec<Row>) {
        self.lessons
            .entry(lesson.into())
            .or_default()
            .measurements
            .insert(step, rows);
    }

    pub fn complete(&mut self, lesson: &str) {
        let l = self.lessons.entry(lesson.into()).or_default();
        l.done = true;
    }

    pub fn is_done(&self, lesson: &str) -> bool {
        self.lessons.get(lesson).is_some_and(|l| l.done)
    }

    /// Step to resume a lesson at.
    pub fn resume(&self, lesson: &str) -> usize {
        self.lessons.get(lesson).map(|l| l.current).unwrap_or(0)
    }

    /// Lessons done among `ids` (track progress checkmarks).
    pub fn done_count<'a>(&self, ids: impl IntoIterator<Item = &'a str>) -> usize {
        ids.into_iter().filter(|i| self.is_done(i)).count()
    }

    pub fn reset(&mut self, lesson: &str) {
        self.lessons.remove(lesson);
    }
}
