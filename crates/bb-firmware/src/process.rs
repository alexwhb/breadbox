//! Subprocesses with a time budget and a cancel switch (`MCU-4`). Everything `bb-firmware`
//! starts — arduino-cli, the avr-gcc / ctags probes, Homebrew, the ctags build, the host C++
//! compiler — goes through [`run`], so nothing can block a caller forever: on timeout or
//! cancel the child and everything it started (its process group) is killed and the caller
//! gets [`FirmwareError::Timeout`] / [`FirmwareError::Cancelled`].

use crate::{FirmwareError, platform};
use std::io::{BufRead, BufReader, Read};
use std::process::{Command, ExitStatus, Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

/// A shared cancel switch (the app's job cancel flag, a test's handle).
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        CancelToken::default()
    }
    /// Wraps an existing flag (e.g. a background job's cancel flag).
    pub fn from_flag(flag: Arc<AtomicBool>) -> Self {
        CancelToken(flag)
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

impl PartialEq for CancelToken {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// `"20 s"`, `"1.5 s"`, `"300 ms"` — for error messages.
pub fn fmt_limit(d: &Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        format!("{ms} ms")
    } else if ms.is_multiple_of(1000) {
        format!("{} s", ms / 1000)
    } else {
        format!("{:.1} s", d.as_secs_f64())
    }
}

/// Short name of a command for messages: `arduino-cli core list`, `avr-gcc --version`,
/// `brew install` (no config paths, no sketch paths).
pub fn describe(cmd: &Command) -> String {
    let prog = std::path::Path::new(cmd.get_program())
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| cmd.get_program().to_string_lossy().into_owned());
    let mut words = vec![prog];
    let mut args = cmd.get_args().map(|a| a.to_string_lossy().into_owned());
    let mut taken = 0;
    while let Some(a) = args.next() {
        if a == "--config-file" {
            args.next();
            continue;
        }
        if a.starts_with('-') {
            if taken == 0 {
                words.push(a);
            }
            break;
        }
        if a.contains('/') || taken == 2 {
            break;
        }
        words.push(a);
        taken += 1;
    }
    words.join(" ")
}

enum Msg {
    Out(Vec<u8>),
    OutEnd,
    Err(Vec<u8>),
    ErrEnd,
}

/// Runs `cmd` to completion within `timeout`, streaming each stdout line to `on_line`.
///
/// stdin is closed. The child runs in its own process group; on timeout / cancel the group is
/// killed. A child that exits while something it started still holds its pipes gets two
/// seconds, then the leftovers are killed too.
pub fn run(
    mut cmd: Command,
    timeout: Duration,
    cancel: Option<&CancelToken>,
    on_line: &mut dyn FnMut(&str),
) -> Result<Output, FirmwareError> {
    let what = describe(&cmd);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    platform::isolate(&mut cmd);
    let mut child = cmd
        .spawn()
        .map_err(|e| FirmwareError::Io(format!("{what}: {e}")))?;
    let pid = child.id();
    let (tx, rx) = mpsc::channel::<Msg>();
    match child.stdout.take() {
        Some(out) => {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let mut r = BufReader::new(out);
                loop {
                    let mut line = Vec::new();
                    match r.read_until(b'\n', &mut line) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {
                            if tx.send(Msg::Out(line)).is_err() {
                                return;
                            }
                        }
                    }
                }
                let _ = tx.send(Msg::OutEnd);
            });
        }
        None => {
            let _ = tx.send(Msg::OutEnd);
        }
    }
    match child.stderr.take() {
        Some(mut err) => {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let mut buf = vec![0u8; 8192];
                loop {
                    match err.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if tx.send(Msg::Err(buf[..n].to_vec())).is_err() {
                                return;
                            }
                        }
                    }
                }
                let _ = tx.send(Msg::ErrEnd);
            });
        }
        None => {
            let _ = tx.send(Msg::ErrEnd);
        }
    }
    drop(tx);
    let deadline = Instant::now() + timeout;
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let (mut out_open, mut err_open) = (true, true);
    let mut status: Option<ExitStatus> = None;
    let mut exited_at: Option<Instant> = None;
    loop {
        let wait = deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(25));
        if out_open || err_open {
            match rx.recv_timeout(wait) {
                Ok(Msg::Out(l)) => {
                    on_line(String::from_utf8_lossy(&l).trim_end_matches(['\n', '\r']));
                    stdout.extend_from_slice(&l);
                }
                Ok(Msg::Err(b)) => stderr.extend_from_slice(&b),
                Ok(Msg::OutEnd) => out_open = false,
                Ok(Msg::ErrEnd) => err_open = false,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    out_open = false;
                    err_open = false;
                }
            }
        } else {
            std::thread::sleep(wait.min(Duration::from_millis(5)));
        }
        if status.is_none()
            && let Ok(Some(s)) = child.try_wait()
        {
            status = Some(s);
            exited_at = Some(Instant::now());
        }
        if let (Some(s), Some(at)) = (status, exited_at) {
            if !out_open && !err_open {
                return Ok(Output {
                    status: s,
                    stdout,
                    stderr,
                });
            }
            if at.elapsed() > Duration::from_secs(2) {
                platform::kill_group(pid);
                return Ok(Output {
                    status: s,
                    stdout,
                    stderr,
                });
            }
        }
        if cancel.is_some_and(|c| c.is_cancelled()) {
            platform::kill_tree(&mut child);
            let _ = child.wait();
            return Err(FirmwareError::Cancelled(what));
        }
        if Instant::now() >= deadline {
            platform::kill_tree(&mut child);
            let _ = child.wait();
            return Err(FirmwareError::Timeout {
                what,
                limit: timeout,
            });
        }
    }
}

/// Runs `cmd` with a budget and returns its stdout when it exits successfully (tool probes:
/// `avr-gcc --version`, `ctags --version`). Anything else — missing, wrong CPU type, crash,
/// timeout — is `None`.
pub fn stdout_if_ok(cmd: Command, timeout: Duration) -> Option<String> {
    let out = run(cmd, timeout, None, &mut |_| {}).ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str) -> Command {
        let mut c = Command::new("sh");
        c.arg("-c").arg(script);
        c
    }

    #[test]
    fn describe_drops_config_and_paths() {
        let mut c = Command::new("/tools/bin/arduino-cli");
        c.args(["--config-file", "/x/arduino-cli.yaml", "core", "list"])
            .args(["--format", "json"]);
        assert_eq!(describe(&c), "arduino-cli core list");
        let mut c = Command::new("/a/b/avr-gcc");
        c.arg("--version");
        assert_eq!(describe(&c), "avr-gcc --version");
        let mut c = Command::new("arduino-cli");
        c.args(["compile", "--fqbn", "arduino:avr:uno", "/tmp/sketch"]);
        assert_eq!(describe(&c), "arduino-cli compile");
    }

    #[test]
    fn output_and_lines_are_collected() {
        let mut lines = Vec::new();
        let out = run(
            sh("echo one; echo two; echo oops >&2; exit 3"),
            Duration::from_secs(20),
            None,
            &mut |l| lines.push(l.to_string()),
        )
        .unwrap();
        assert_eq!(lines, ["one", "two"]);
        assert_eq!(String::from_utf8_lossy(&out.stdout), "one\ntwo\n");
        assert_eq!(String::from_utf8_lossy(&out.stderr), "oops\n");
        assert_eq!(out.status.code(), Some(3));
    }

    /// `sh` starting `sleep`: killing only `sh` would leave `sleep` holding the pipes.
    #[test]
    fn a_hung_child_and_its_children_are_killed_at_the_deadline() {
        let t0 = Instant::now();
        let r = run(
            sh("echo started; sleep 600; echo never"),
            Duration::from_millis(700),
            None,
            &mut |_| {},
        );
        let dt = t0.elapsed();
        match r {
            Err(FirmwareError::Timeout { what, limit }) => {
                assert_eq!(what, "sh -c");
                assert_eq!(limit, Duration::from_millis(700));
            }
            other => panic!("expected a timeout, got {other:?}"),
        }
        assert!(dt < Duration::from_secs(5), "took {dt:?}");
        let e = FirmwareError::Timeout {
            what: "arduino-cli core list".into(),
            limit: Duration::from_secs(20),
        };
        assert_eq!(
            e.to_string(),
            "`arduino-cli core list` did not finish within 20 s"
        );
    }

    #[test]
    fn cancel_stops_a_running_child() {
        let token = CancelToken::new();
        let t2 = token.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            t2.cancel();
        });
        let t0 = Instant::now();
        let r = run(
            sh("sleep 600"),
            Duration::from_secs(60),
            Some(&token),
            &mut |_| {},
        );
        assert!(matches!(r, Err(FirmwareError::Cancelled(_))), "{r:?}");
        assert!(t0.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_background_grandchild_does_not_hold_the_caller() {
        let t0 = Instant::now();
        let out = run(
            sh("sleep 600 & echo done"),
            Duration::from_secs(30),
            None,
            &mut |_| {},
        )
        .unwrap();
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout), "done\n");
        assert!(t0.elapsed() < Duration::from_secs(8), "{:?}", t0.elapsed());
    }

    #[test]
    fn missing_program_is_an_io_error() {
        let r = stdout_if_ok(Command::new("/nonexistent/avr-gcc"), Duration::from_secs(5));
        assert!(r.is_none());
    }

    #[test]
    fn limits_format_readably() {
        assert_eq!(fmt_limit(&Duration::from_secs(20)), "20 s");
        assert_eq!(fmt_limit(&Duration::from_millis(1500)), "1.5 s");
        assert_eq!(fmt_limit(&Duration::from_millis(300)), "300 ms");
    }
}
