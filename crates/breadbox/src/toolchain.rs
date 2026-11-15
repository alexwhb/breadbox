//! App-wide firmware toolchain state (`MCU-4`): background detection at start-up, the
//! one-click install flow with progress, and the library list.
//!
//! Detection always settles: every arduino-cli call has a time budget
//! (`bb_firmware::Timeouts`), so a stuck arduino-cli ends in [`ToolchainState::Unresponsive`]
//! with a message and a Retry, never an endless "detecting…".

use crate::bench::Repaint;
use crate::jobs::{Job, ProgressState};
use bb_firmware::{FirmwareError, InstallProgress, InstalledLibrary, Toolchain};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// After this long, "detecting…" says how long it has been waiting.
const SLOW_DETECT: Duration = Duration::from_secs(3);

#[derive(Clone)]
pub enum ToolchainState {
    Detecting,
    Ready(Arc<Toolchain>),
    /// Not usable yet: nothing found, or arduino-cli without the AVR core / a runnable compiler.
    Missing {
        found: Option<Arc<Toolchain>>,
        reason: String,
    },
    Installing,
    Failed(String),
    /// arduino-cli did not answer in its time budget (it was stopped): the message says what
    /// timed out and what to do.
    Unresponsive(String),
    /// Detection switched off (tests that must not depend on the machine).
    Disabled,
}

pub struct ToolchainService {
    pub state: ToolchainState,
    detect: Option<Job<Result<Toolchain, FirmwareError>>>,
    install: Option<Job<Result<Toolchain, FirmwareError>>>,
    libs: Option<Job<Result<Vec<InstalledLibrary>, FirmwareError>>>,
    pub libraries: Vec<InstalledLibrary>,
    repaint: Repaint,
    detect_started: Option<Instant>,
}

impl ToolchainService {
    pub fn new(repaint: Repaint, detect: bool) -> ToolchainService {
        let mut s = ToolchainService {
            state: if detect {
                ToolchainState::Detecting
            } else {
                ToolchainState::Disabled
            },
            detect: None,
            install: None,
            libs: None,
            libraries: Vec::new(),
            repaint,
            detect_started: None,
        };
        if detect {
            s.redetect();
        }
        s
    }

    pub fn redetect(&mut self) {
        self.state = ToolchainState::Detecting;
        self.detect_started = Some(Instant::now());
        self.detect = Some(Job::spawn("toolchain-detect", self.repaint.clone(), |_| {
            Toolchain::detect()
        }));
    }

    /// How long the running detection has taken (`None` when not detecting).
    pub fn detecting_for(&self) -> Option<Duration> {
        match self.state {
            ToolchainState::Detecting => self.detect_started.map(|t| t.elapsed()),
            _ => None,
        }
    }

    /// One line for "detecting": plain at first, then with the seconds waited so far
    /// (`detecting toolchain… 7 s`).
    pub fn detecting_label(&self) -> String {
        match self.detecting_for() {
            Some(d) if d >= SLOW_DETECT => format!("detecting toolchain… {} s", d.as_secs()),
            _ => "detecting toolchain…".into(),
        }
    }

    /// Advances background jobs; call once per frame.
    pub fn poll(&mut self) {
        if let Some(j) = self.detect.as_mut()
            && let Some(r) = j.poll()
        {
            self.detect = None;
            self.detect_started = None;
            self.state = classify(r);
            self.refresh_libraries();
        }
        if let Some(j) = self.install.as_mut()
            && let Some(r) = j.poll()
        {
            self.install = None;
            self.state = match r {
                Ok(tc) if tc.ready() => ToolchainState::Ready(Arc::new(tc)),
                Ok(tc) => ToolchainState::Missing {
                    reason: missing_reason(&tc),
                    found: Some(Arc::new(tc)),
                },
                Err(e) => ToolchainState::Failed(e.to_string()),
            };
            self.refresh_libraries();
        }
        if let Some(j) = self.libs.as_mut()
            && let Some(r) = j.poll()
        {
            self.libs = None;
            if let Ok(l) = r {
                self.libraries = l;
            }
        }
    }

    pub fn ready(&self) -> Option<Arc<Toolchain>> {
        match &self.state {
            ToolchainState::Ready(t) => Some(t.clone()),
            _ => None,
        }
    }

    /// Any detected arduino-cli (ready or not) — for library search.
    pub fn any(&self) -> Option<Arc<Toolchain>> {
        match &self.state {
            ToolchainState::Ready(t) => Some(t.clone()),
            ToolchainState::Missing { found, .. } => found.clone(),
            _ => None,
        }
    }

    /// Toolchain box label (`avr-gcc 7.3.0`).
    pub fn compiler_label(&self) -> String {
        match &self.state {
            ToolchainState::Detecting => self.detecting_label(),
            ToolchainState::Ready(t) => t.compiler_label(),
            ToolchainState::Missing { .. } => "toolchain not installed".into(),
            ToolchainState::Installing => "installing toolchain…".into(),
            ToolchainState::Failed(_) => "toolchain install failed".into(),
            ToolchainState::Unresponsive(_) => "arduino-cli: no answer".into(),
            ToolchainState::Disabled => "toolchain off".into(),
        }
    }

    pub fn cli_label(&self) -> Option<String> {
        self.any().map(|t| format!("arduino-cli {}", t.cli_version))
    }

    pub fn is_installing(&self) -> bool {
        self.install.is_some()
    }

    pub fn install_progress(&self) -> Option<ProgressState> {
        self.install.as_ref().map(|j| j.progress())
    }

    /// One-click install: arduino-cli + arduino:avr (+ native compiler on Rosetta-less Macs).
    pub fn start_install(&mut self) {
        if self.install.is_some() {
            return;
        }
        let found = self.any();
        self.state = ToolchainState::Installing;
        self.install = Some(Job::spawn(
            "toolchain-install",
            self.repaint.clone(),
            move |p| {
                let mut cb = |ev: InstallProgress| match ev {
                    InstallProgress::Download { bytes, total } => {
                        p.set_fraction(total.map(|t| bytes as f32 / t.max(1) as f32));
                    }
                    InstallProgress::Step(s) => {
                        p.set_fraction(None);
                        p.step(s);
                    }
                    InstallProgress::Log(l) => p.log(l),
                    InstallProgress::Done => p.set_fraction(Some(1.0)),
                };
                match found {
                    Some(tc) => {
                        let mut tc = (*tc).clone();
                        tc.install_core(&mut cb)?;
                        if tc.needs_native_tools() {
                            let opts = bb_firmware::NativeInstallOptions {
                                app_dir: Toolchain::app_dir(),
                                ..Default::default()
                            };
                            tc.install_native_tools_with(&opts, &mut cb)?;
                        }
                        cb(InstallProgress::Done);
                        Ok(tc)
                    }
                    None => Toolchain::install(&Toolchain::app_dir(), &mut cb),
                }
            },
        ));
    }

    pub fn refresh_libraries(&mut self) {
        let Some(tc) = self.any() else {
            return;
        };
        self.libs = Some(Job::spawn("lib-list", self.repaint.clone(), move |_| {
            tc.lib_list()
        }));
    }

    /// Installed version of a library (`ArduinoJson` → `7.2.1`).
    pub fn library_version(&self, name: &str) -> Option<&str> {
        self.libraries
            .iter()
            .find(|l| l.name.eq_ignore_ascii_case(name))
            .map(|l| l.version.as_str())
    }

    /// Version of a user-installed (Library Manager) library; core-bundled ones return `None`.
    pub fn user_library_version(&self, name: &str) -> Option<&str> {
        self.libraries
            .iter()
            .find(|l| l.name.eq_ignore_ascii_case(name) && l.location == "user")
            .map(|l| l.version.as_str())
    }
}

fn classify(r: Result<Toolchain, FirmwareError>) -> ToolchainState {
    match r {
        Ok(tc) if tc.ready() => ToolchainState::Ready(Arc::new(tc)),
        Ok(tc) => ToolchainState::Missing {
            reason: missing_reason(&tc),
            found: Some(Arc::new(tc)),
        },
        Err(FirmwareError::ToolchainMissing) => ToolchainState::Missing {
            found: None,
            reason: "arduino-cli is not installed".into(),
        },
        Err(e @ FirmwareError::Timeout { .. }) => ToolchainState::Unresponsive(format!(
            "{e} and was stopped. Breadbox gives arduino-cli its own settings folder \
             (never ~/Documents), so it is probably stuck or very slow. Retry, or install \
             Breadbox's own copy."
        )),
        Err(e) => ToolchainState::Missing {
            found: None,
            reason: e.to_string(),
        },
    }
}

fn missing_reason(tc: &Toolchain) -> String {
    if tc.avr_core_version.is_none() {
        "the arduino:avr core is not installed".into()
    } else if tc.avr_gcc_version.is_none() {
        "avr-gcc cannot run on this Mac (native compiler not installed)".into()
    } else {
        "ctags cannot run on this Mac (native ctags not installed)".into()
    }
}
