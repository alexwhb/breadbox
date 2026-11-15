//! App-wide firmware toolchain state (`MCU-4`): background detection at start-up, the
//! one-click install flow with progress, and the library list.

use crate::bench::Repaint;
use crate::jobs::{Job, ProgressState};
use bb_firmware::{FirmwareError, InstallProgress, InstalledLibrary, Toolchain};
use std::sync::Arc;

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
        };
        if detect {
            s.redetect();
        }
        s
    }

    pub fn redetect(&mut self) {
        self.state = ToolchainState::Detecting;
        self.detect = Some(Job::spawn("toolchain-detect", self.repaint.clone(), |_| {
            Toolchain::detect()
        }));
    }

    /// Advances background jobs; call once per frame.
    pub fn poll(&mut self) {
        if let Some(j) = self.detect.as_mut()
            && let Some(r) = j.poll()
        {
            self.detect = None;
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
            ToolchainState::Detecting => "detecting toolchain…".into(),
            ToolchainState::Ready(t) => t.compiler_label(),
            ToolchainState::Missing { .. } => "toolchain not installed".into(),
            ToolchainState::Installing => "installing toolchain…".into(),
            ToolchainState::Failed(_) => "toolchain install failed".into(),
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
        self.install = Some(Job::spawn("toolchain-install", self.repaint.clone(), move |p| {
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
        }));
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
