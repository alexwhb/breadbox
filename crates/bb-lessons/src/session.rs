//! The state machine behind the Lessons screen (LES-2 model): the current step, the choice
//! ("Pick R1"), the optional prediction ("Do the math myself" → check, hints, "Show me the
//! working"), the measurements table and its checkpoint, quizzes, Back/Next, and "Open in
//! free play" (LES-5).
//!
//! The session owns the lesson's circuit (a `bb_circuit::edit::Editor`); the bench that runs
//! it lives with the caller (the app's `SimHandle`, or [`crate::runner::Runner`] headless),
//! which applies [`Edit`]s and feeds meter readings back.

use crate::format::{Lesson, Step};
use crate::text::{self, Vars};
use crate::{LessonError, LoadedLesson};
use bb_circuit::edit::Editor;
use bb_circuit::project::Mode;
use bb_circuit::{Library, Project, PropValue};
use serde::{Deserialize, Serialize};

/// One row of "Your measurements".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Row {
    /// Option label (`220Ω`).
    pub label: String,
    pub value: f64,
    /// `13.6 mA`, or `—` without a prediction.
    pub predicted: String,
    /// `13.4 mA`.
    pub meter: String,
}

/// Per-step state.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StepState {
    pub choice: Option<usize>,
    /// "Do the math myself" is on.
    pub math: bool,
    pub guess: Option<f64>,
    /// Some(true) = right, Some(false) = wrong.
    pub verdict: Option<bool>,
    pub hint: Option<String>,
    pub show_working: bool,
    pub rows: Vec<Row>,
    pub quiz: Option<usize>,
    /// The code step's firmware was loaded and runs.
    pub firmware: bool,
    /// Last evaluation passed.
    pub passed: bool,
}

/// A circuit change the bench must apply.
#[derive(Debug, Clone, PartialEq)]
pub enum Edit {
    /// A live property change (`Command::SetPartParam`).
    SetProp {
        part: String,
        prop: String,
        value: f64,
    },
    /// The project changed structurally (`Command::UpdateProject`).
    Project(Box<Project>),
}

/// The result of checking a prediction.
#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    pub correct: bool,
    /// `Nailed it. The meter agrees: 13.4 mA.` (the meter's reading) or the hint.
    pub text: String,
    pub expected: f64,
}

/// A running lesson.
#[derive(Debug, Clone)]
pub struct Session {
    pub lesson: Lesson,
    pub loaded: LoadedLesson,
    pub step: usize,
    pub states: Vec<StepState>,
    pub editor: Editor,
    lib: std::sync::Arc<Library>,
}

impl Session {
    pub fn new(l: &LoadedLesson, lib: std::sync::Arc<Library>) -> Result<Session, LessonError> {
        let project = l.start_project()?;
        let n = l.lesson.steps.len();
        let mut s = Session {
            lesson: l.lesson.clone(),
            loaded: l.clone(),
            step: 0,
            states: vec![StepState::default(); n],
            editor: Editor::new(project),
            lib,
        };
        s.enter_step();
        Ok(s)
    }

    pub fn library(&self) -> &std::sync::Arc<Library> {
        &self.lib
    }

    pub fn project(&self) -> &Project {
        &self.editor.project
    }

    pub fn current(&self) -> &Step {
        &self.lesson.steps[self.step]
    }

    pub fn state(&self) -> &StepState {
        &self.states[self.step]
    }

    fn state_mut(&mut self) -> &mut StepState {
        &mut self.states[self.step]
    }

    /// `Step 3 of 5`.
    pub fn label(&self) -> String {
        format!("Step {} of {}", self.step + 1, self.lesson.steps.len())
    }

    /// `Lesson 3 of 8` within its track (`count` = lessons in the track).
    pub fn lesson_label(&self, count: usize) -> String {
        format!("Lesson {} of {count}", self.lesson.order)
    }

    fn enter_step(&mut self) {
        let step = self.current().clone();
        let st = &mut self.states[self.step];
        if let Some(p) = &step.predict
            && st.choice.is_none()
            && st.guess.is_none()
        {
            st.math = !p.optional || !p.toggle_label.is_empty();
        }
        if let Some(c) = &step.choice
            && st.choice.is_none()
        {
            st.choice = Some(c.default.min(c.options.len().saturating_sub(1)));
        }
    }

    /// Variables of the current step (choice value + label, guess, meter, answer, step vars).
    pub fn vars(&self, meter: Option<f64>) -> Vars {
        let step = self.current();
        let st = self.state();
        let mut v = Vars::default();
        for (k, x) in &step.vars {
            v.set(k, *x);
        }
        if let (Some(c), Some(i)) = (&step.choice, st.choice)
            && let Some(o) = c.options.get(i)
        {
            v.set(&c.var, o.value);
            v.set_text("choice", &o.label);
        }
        if let Some(g) = st.guess {
            v.set("guess", g);
        }
        if let Some(m) = meter {
            v.set("meter", m);
        }
        if let Some(p) = &step.predict
            && let Ok(a) = text::eval(&p.answer, &v)
        {
            v.set("answer", a);
            // the design's working uses amps and mA
            v.set("mA", a);
            v.set("amps", a / 1000.0);
        }
        v
    }

    /// Renders a template of the current step.
    pub fn render(&self, template: &str, meter: Option<f64>) -> String {
        text::render(template, &self.vars(meter))
    }

    /// Picks option `index` of the step's choice; returns the bench edit.
    pub fn choose(&mut self, index: usize) -> Result<Edit, LessonError> {
        let step = self.current().clone();
        let c = step.choice.as_ref().ok_or_else(|| LessonError::Invalid {
            lesson: self.lesson.id.clone(),
            msg: format!("step `{}` has no choice", step.title),
        })?;
        let o = c.options.get(index).ok_or_else(|| LessonError::Invalid {
            lesson: self.lesson.id.clone(),
            msg: format!("no option {index}"),
        })?;
        // "None": a jumper in place of the part (the smallest value it allows)
        let value = if o.bypass {
            let lib = self.lib.clone();
            self.editor
                .project
                .part(&c.part)
                .and_then(|p| lib.get(&p.part))
                .and_then(|d| d.prop(&c.prop))
                .and_then(|pd| match &pd.kind {
                    bb_circuit::part::PropKind::Number { min, .. } => *min,
                    _ => None,
                })
                .unwrap_or(0.01)
                .max(o.value)
        } else {
            o.value
        };
        let lib = self.lib.clone();
        self.editor
            .set_prop(&lib, &c.part, &c.prop, PropValue::Num(value))
            .map_err(|e| LessonError::Bench(e.to_string()))?;
        let st = self.state_mut();
        st.choice = Some(index);
        st.verdict = None;
        st.guess = None;
        st.hint = None;
        st.show_working = false;
        Ok(Edit::SetProp {
            part: c.part.clone(),
            prop: c.prop.clone(),
            value,
        })
    }

    /// The chosen option is the "None" (bypass) one.
    pub fn bypassed(&self) -> bool {
        let step = self.current();
        match (&step.choice, self.state().choice) {
            (Some(c), Some(i)) => c.options.get(i).is_some_and(|o| o.bypass),
            _ => false,
        }
    }

    /// "Do the math myself".
    pub fn set_math(&mut self, on: bool) {
        let st = self.state_mut();
        st.math = on;
        st.verdict = None;
        st.guess = None;
        st.hint = None;
    }

    /// The meter "waits for your prediction" (math mode on, nothing right yet).
    pub fn meter_hidden(&self) -> bool {
        let st = self.state();
        self.current().predict.is_some()
            && st.math
            && st.verdict != Some(true)
            && !st.show_working
            && !self.bypassed()
    }

    /// Checks a prediction (`meter` = the instrument's reading, in the prediction's unit).
    pub fn predict(&mut self, guess: f64, meter: Option<f64>) -> Verdict {
        let step = self.current().clone();
        self.state_mut().guess = Some(guess);
        let vars = self.vars(meter);
        let Some(p) = &step.predict else {
            return Verdict {
                correct: false,
                text: String::new(),
                expected: f64::NAN,
            };
        };
        let expected = vars.get("answer").unwrap_or(f64::NAN);
        // `{meter}` quotes the instrument's reading; without one (no bench) the answer stands in
        let vars = if meter.is_none() && expected.is_finite() {
            let mut v = vars;
            v.set("meter", expected);
            v
        } else {
            vars
        };
        let correct = guess.is_finite()
            && expected.is_finite()
            && ((guess - expected).abs() <= p.tolerance * expected.abs().max(1e-12));
        let text = if correct {
            text::render(&p.correct_text, &vars)
        } else {
            p.hints
                .iter()
                .find(|h| text::eval(&h.when, &vars).is_ok_and(|x| x != 0.0))
                .map(|h| text::render(&h.text, &vars))
                .unwrap_or_else(|| text::render(&p.default_hint, &vars))
        };
        let st = self.state_mut();
        st.verdict = Some(correct);
        st.hint = (!correct).then(|| text.clone());
        Verdict {
            correct,
            text,
            expected,
        }
    }

    /// "Show me the working" lines for the current choice.
    pub fn working(&self) -> Vec<String> {
        let vars = self.vars(None);
        self.current()
            .predict
            .as_ref()
            .map(|p| p.working.iter().map(|w| text::render(w, &vars)).collect())
            .unwrap_or_default()
    }

    pub fn toggle_working(&mut self) -> bool {
        let st = self.state_mut();
        st.show_working = !st.show_working;
        st.show_working
    }

    /// Records the meter reading for the current choice into the table. With "Do the math
    /// myself" on, only after a right prediction (the row then shows it). Returns whether a
    /// row was recorded.
    pub fn record(&mut self, meter: f64) -> bool {
        let step = self.current().clone();
        let st = self.state().clone();
        if self.bypassed() || st.choice.is_none() {
            return false;
        }
        let math = step.predict.is_some() && st.math;
        if math && st.verdict != Some(true) {
            return false;
        }
        let unit = step
            .measure
            .as_ref()
            .map(|m| m.unit.clone())
            .or_else(|| step.predict.as_ref().map(|p| p.unit.clone()))
            .unwrap_or_default();
        let (label, value) = match (&step.choice, st.choice) {
            (Some(c), Some(i)) => (c.options[i].label.clone(), c.options[i].value),
            _ => (String::new(), 0.0),
        };
        let predicted = match (math, st.guess) {
            (true, Some(g)) => format!("{} {unit}", text::smart(g)).trim().to_string(),
            _ => "—".into(),
        };
        let row = Row {
            label,
            value,
            predicted,
            meter: format!("{} {unit}", text::smart(meter)).trim().to_string(),
        };
        let rows = &mut self.state_mut().rows;
        rows.retain(|r| r.value != row.value);
        rows.push(row);
        true
    }

    /// Checkpoint text and whether it passed (`Checkpoint passed` / `Try 2 more values`).
    pub fn checkpoint(&self) -> (bool, String) {
        let Some(t) = &self.current().table else {
            return (true, String::new());
        };
        let n = self.state().rows.len();
        if n >= t.min_rows {
            (true, t.passed.clone())
        } else {
            let left = t.min_rows - n;
            let mut v = Vars::default();
            v.set("n", left as f64);
            v.set_text("s", if left == 1 { "" } else { "s" });
            v.set_text("n", &left.to_string());
            (false, text::render(&t.remaining, &v))
        }
    }

    pub fn answer_quiz(&mut self, index: usize) -> bool {
        let ok = self
            .current()
            .quiz
            .as_ref()
            .is_some_and(|q| q.answer == index);
        self.state_mut().quiz = Some(index);
        ok
    }

    pub fn set_firmware_loaded(&mut self, on: bool) {
        self.state_mut().firmware = on;
    }

    pub fn mark(&mut self, passed: bool) {
        self.state_mut().passed = passed;
    }

    /// Next step (returns false at the end).
    pub fn next_step(&mut self) -> bool {
        if self.step + 1 < self.lesson.steps.len() {
            self.step += 1;
            self.enter_step();
            true
        } else {
            false
        }
    }

    pub fn prev_step(&mut self) -> bool {
        if self.step > 0 {
            self.step -= 1;
            true
        } else {
            false
        }
    }

    pub fn is_last(&self) -> bool {
        self.step + 1 == self.lesson.steps.len()
    }

    /// Next button label (`Next: break it on purpose`, `Finish lesson`).
    pub fn next_label(&self) -> String {
        let s = self.current();
        if !s.next_label.is_empty() {
            return s.next_label.clone();
        }
        if self.is_last() {
            "Finish lesson".into()
        } else {
            format!(
                "Next: {}",
                self.lesson.steps[self.step + 1].title.to_lowercase()
            )
        }
    }

    /// "Open in free play" (LES-5): the lesson bench as an editable Playground project.
    pub fn free_play(&self) -> Project {
        let mut p = self.editor.project.clone();
        p.mode = Mode::Playground;
        p.name = format!("{} (free play)", self.lesson.title);
        p
    }
}
