//! Background jobs (compile, toolchain install, library install, SPICE runs): a thread with
//! shared progress + a result slot. The worker wakes the UI through the repaint callback, so
//! an idle UI does not poll.

use crate::bench::Repaint;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Progress shared between a job and the UI.
#[derive(Debug, Clone, Default)]
pub struct ProgressState {
    /// 0–1, `None` = indeterminate.
    pub fraction: Option<f32>,
    /// Current step (`"Installing arduino:avr"`).
    pub step: String,
    /// Recent log lines (bounded).
    pub log: Vec<String>,
}

/// Handle a job uses to report progress.
#[derive(Clone)]
pub struct Progress {
    state: Arc<Mutex<ProgressState>>,
    cancel: Arc<AtomicBool>,
    repaint: Repaint,
}

impl Progress {
    pub fn set_fraction(&self, f: Option<f32>) {
        self.update(|s| s.fraction = f);
    }
    pub fn step(&self, step: impl Into<String>) {
        let step = step.into();
        self.update(|s| {
            s.log.push(step.clone());
            s.step = step;
        });
    }
    pub fn log(&self, line: impl Into<String>) {
        let line = line.into();
        self.update(|s| {
            s.log.push(line);
            let n = s.log.len();
            if n > 400 {
                s.log.drain(..n - 400);
            }
        });
    }
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }
    fn update(&self, f: impl FnOnce(&mut ProgressState)) {
        if let Ok(mut s) = self.state.lock() {
            f(&mut s);
        }
        (self.repaint)();
    }
}

/// A running (or finished) background job producing `T`.
pub struct Job<T> {
    result: Arc<Mutex<Option<T>>>,
    progress: Arc<Mutex<ProgressState>>,
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
    taken: bool,
    started: std::time::Instant,
}

impl<T: Send + 'static> Job<T> {
    pub fn spawn(
        name: &str,
        repaint: Repaint,
        f: impl FnOnce(&Progress) -> T + Send + 'static,
    ) -> Job<T> {
        let result = Arc::new(Mutex::new(None));
        let progress = Arc::new(Mutex::new(ProgressState::default()));
        let cancel = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let p = Progress {
            state: progress.clone(),
            cancel: cancel.clone(),
            repaint: repaint.clone(),
        };
        let (r2, d2) = (result.clone(), done.clone());
        let spawn = std::thread::Builder::new()
            .name(format!("job-{name}"))
            .spawn(move || {
                let out = f(&p);
                if let Ok(mut r) = r2.lock() {
                    *r = Some(out);
                }
                d2.store(true, Ordering::Release);
                (repaint)();
            });
        if let Err(e) = spawn {
            log::error!("could not start job {name}: {e}");
            done.store(true, Ordering::Release);
        }
        Job {
            result,
            progress,
            cancel,
            done,
            taken: false,
            started: std::time::Instant::now(),
        }
    }

    /// Takes the result once the job finished.
    pub fn poll(&mut self) -> Option<T> {
        if self.taken || !self.done.load(Ordering::Acquire) {
            return None;
        }
        let r = self.result.lock().ok()?.take();
        if r.is_some() {
            self.taken = true;
        }
        r
    }

    pub fn is_running(&self) -> bool {
        !self.done.load(Ordering::Acquire)
    }

    pub fn progress(&self) -> ProgressState {
        self.progress.lock().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn elapsed(&self) -> std::time::Duration {
        self.started.elapsed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn job_reports_progress_and_result() {
        let mut j = Job::spawn("t", Arc::new(|| {}), |p| {
            p.step("working");
            p.set_fraction(Some(0.5));
            41 + 1
        });
        let t0 = std::time::Instant::now();
        let r = loop {
            if let Some(r) = j.poll() {
                break r;
            }
            assert!(t0.elapsed().as_secs() < 5);
            std::thread::sleep(std::time::Duration::from_millis(1));
        };
        assert_eq!(r, 42);
        assert_eq!(j.progress().step, "working");
        assert!(j.poll().is_none(), "result is taken once");
        assert!(!j.is_running());
    }
}
