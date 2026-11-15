//! `MCU-4`: toolchain detection and every arduino-cli call must settle — never hang — and
//! arduino-cli always runs with Breadbox's private settings (never `~/Documents/Arduino`,
//! whose macOS privacy prompt made a Finder-launched app's detection hang for 15+ minutes).
//!
//! Exercised with a fake `arduino-cli` (a shell script) that answers some commands and sleeps
//! "forever" on others, the way the real one blocked in `open()` behind a privacy prompt.

use bb_firmware::{
    CancelToken, CompileOptions, DetectOptions, FirmwareError, Timeouts, Toolchain, ToolchainSource,
};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("bb-cli-timeouts-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A fake arduino-cli. `hang` lists commands that never answer: `"version"`, `"core_list"`,
/// `"lib_search"`, `"lib_install"`, `"compile"`. Every call is logged to `<dir>/calls.log` as
/// `args|ARDUINO_DIRECTORIES_USER|ARDUINO_CONFIG_FILE`.
fn fake_cli(dir: &Path, hang: &[&str]) -> PathBuf {
    let bin = dir.join("fake-bin/arduino-cli");
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    let script = format!(
        r#"#!/bin/sh
printf '%s|%s|%s\n' "$*" "$ARDUINO_DIRECTORIES_USER" "${{ARDUINO_CONFIG_FILE-unset}}" >> "{log}"
if [ "$1" = "--config-file" ]; then shift 2; fi
case " {hang} " in *" $1 "*|*" $1_$2 "*) sleep 100000; exit 9 ;; esac
case "$1" in
  version) echo '{{"VersionString":"9.9.9-fake"}}' ;;
  core) echo '{{"platforms":[{{"id":"arduino:avr","installed_version":"1.8.6"}}]}}' ;;
  lib)
    case "$2" in
      list) echo '{{"installed_libraries":[]}}' ;;
      search) echo '{{"libraries":[{{"name":"Servo","latest":{{"version":"1.2.2"}}}}]}}' ;;
      *) echo "Installed $3" ;;
    esac ;;
  *) echo '{{"success":false,"error":"fake compiler"}}'; exit 1 ;;
esac
"#,
        log = dir.join("calls.log").display(),
        hang = hang.join(" ")
    );
    std::fs::write(&bin, script).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

fn opts(dir: &Path, cli: Option<PathBuf>, budget: Duration) -> DetectOptions {
    DetectOptions {
        explicit: cli,
        search_path: false,
        search_homebrew: false,
        app_dir: dir.join("toolchain"),
        shared_data_dirs: vec![],
        timeouts: Timeouts::all(budget),
    }
}

fn calls(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join("calls.log"))
        .unwrap_or_default()
        .lines()
        .map(|l| l.to_string())
        .collect()
}

fn expect_timeout<T: std::fmt::Debug>(r: Result<T, FirmwareError>, what: &str) {
    match r {
        Err(FirmwareError::Timeout { what: w, .. }) => assert_eq!(w, what),
        other => panic!("expected `{what}` to time out, got {other:?}"),
    }
}

#[test]
fn an_arduino_cli_that_never_answers_times_out_instead_of_hanging_detection() {
    let dir = tmp("never");
    let cli = fake_cli(&dir, &["version"]);
    let t0 = Instant::now();
    let r = Toolchain::detect_with(&opts(&dir, Some(cli), Duration::from_millis(800)));
    let dt = t0.elapsed();
    expect_timeout(r, "arduino-cli version");
    assert!(dt < Duration::from_secs(6), "detection took {dt:?}");
    let e = FirmwareError::Timeout {
        what: "arduino-cli version".into(),
        limit: Duration::from_millis(800),
    };
    assert_eq!(
        e.to_string(),
        "`arduino-cli version` did not finish within 800 ms"
    );
}

/// The observed hang: `version` answers, `core list` blocks (it scans the sketchbook).
#[test]
fn a_core_list_stuck_behind_a_privacy_prompt_times_out() {
    let dir = tmp("core-list");
    let cli = fake_cli(&dir, &["core_list"]);
    let t0 = Instant::now();
    let r = Toolchain::detect_with(&opts(&dir, Some(cli), Duration::from_millis(800)));
    expect_timeout(r, "arduino-cli core list");
    assert!(t0.elapsed() < Duration::from_secs(6));
}

#[test]
fn a_stuck_candidate_does_not_hide_breadboxs_own_install() {
    let dir = tmp("fallback");
    let stuck = fake_cli(&dir, &["version"]);
    // Breadbox's own install in <app_dir>/bin answers
    let own = dir.join("toolchain/bin/arduino-cli");
    std::fs::create_dir_all(own.parent().unwrap()).unwrap();
    std::fs::copy(fake_cli(&tmp("fallback-own"), &[]), &own).unwrap();
    let tc = Toolchain::detect_with(&opts(&dir, Some(stuck), Duration::from_millis(800)))
        .expect("the working install is found after the stuck one times out");
    assert_eq!(tc.source, ToolchainSource::AppSupport);
    assert_eq!(tc.cli_version, "9.9.9-fake");
    assert_eq!(tc.avr_core_version.as_deref(), Some("1.8.6"));
}

#[test]
fn every_call_uses_breadboxs_private_settings_never_documents() {
    let dir = tmp("private");
    let cli = fake_cli(&dir, &[]);
    let o = opts(&dir, Some(cli), Duration::from_secs(20));
    let tc = Toolchain::detect_with(&o).unwrap();
    let app = &o.app_dir;
    assert_eq!(tc.user_dir, app.join("sketchbook"));
    assert_eq!(tc.downloads_dir, app.join("staging"));
    // no shared Arduino folder with arduino:avr → Breadbox's own data folder
    assert_eq!(tc.data_dir, app.join("data"));
    let cfg = tc.config_file.clone().expect("a private config");
    assert_eq!(cfg, app.join("arduino-cli-system.yaml"));
    let text = std::fs::read_to_string(&cfg).unwrap();
    assert!(
        text.contains(&format!("user: \"{}\"", app.join("sketchbook").display())),
        "{text}"
    );
    assert!(
        text.lines()
            .filter(|l| !l.starts_with('#'))
            .all(|l| !l.contains("Documents")),
        "{text}"
    );
    assert!(text.contains("enable_notification: false"));
    tc.lib_list().unwrap();
    assert_eq!(tc.lib_search("Servo").unwrap()[0].name, "Servo");
    let log = calls(&dir);
    assert!(log.len() >= 4, "{log:?}");
    for l in &log {
        let f: Vec<&str> = l.split('|').collect();
        assert!(
            f[0].starts_with(&format!("--config-file {}", cfg.display())),
            "call without the private config: {l}"
        );
        assert_eq!(f[1], app.join("sketchbook").to_string_lossy(), "{l}");
        assert_eq!(f[2], "unset", "an inherited ARDUINO_CONFIG_FILE leaks: {l}");
    }
    // the environment overrides beat anything a user's shell exported
    let cmd = tc.command();
    let envs: Vec<(String, Option<String>)> = cmd
        .get_envs()
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.map(|v| v.to_string_lossy().into_owned()),
            )
        })
        .collect();
    assert!(envs.contains(&("ARDUINO_CONFIG_FILE".into(), None)));
    assert!(envs.contains(&(
        "ARDUINO_DIRECTORIES_USER".into(),
        Some(app.join("sketchbook").to_string_lossy().into_owned())
    )));
}

#[test]
fn a_shared_arduino_folder_is_reused_only_when_it_holds_arduino_avr() {
    let dir = tmp("shared");
    let cli = fake_cli(&dir, &[]);
    let shared = dir.join("Arduino15");
    std::fs::create_dir_all(&shared).unwrap();
    let mut o = opts(&dir, Some(cli), Duration::from_secs(20));
    o.shared_data_dirs = vec![shared.clone()];
    // empty: not reused
    assert_eq!(
        Toolchain::detect_with(&o).unwrap().data_dir,
        o.app_dir.join("data")
    );
    std::fs::create_dir_all(shared.join("packages/arduino/hardware/avr/1.8.6")).unwrap();
    let tc = Toolchain::detect_with(&o).unwrap();
    assert_eq!(tc.data_dir, shared);
    // … but the sketchbook stays Breadbox's own
    assert_eq!(tc.user_dir, o.app_dir.join("sketchbook"));
    let text = std::fs::read_to_string(tc.config_file.unwrap()).unwrap();
    assert!(text.contains(&format!("data: \"{}\"", shared.display())));
}

#[test]
fn library_search_and_install_time_out() {
    let dir = tmp("libs");
    let cli = fake_cli(&dir, &["lib_search", "lib_install"]);
    let tc = Toolchain::detect_with(&opts(&dir, Some(cli), Duration::from_millis(800))).unwrap();
    let t0 = Instant::now();
    expect_timeout(tc.lib_search("ArduinoJson"), "arduino-cli lib search");
    expect_timeout(
        tc.lib_install("ArduinoJson", None, &mut |_| {}),
        "arduino-cli lib install",
    );
    assert!(t0.elapsed() < Duration::from_secs(8));
}

#[test]
fn a_stuck_compile_times_out_and_can_be_cancelled() {
    let dir = tmp("compile");
    let cli = fake_cli(&dir, &["compile"]);
    let tc = Toolchain::detect_with(&opts(&dir, Some(cli), Duration::from_secs(20))).unwrap();
    let sketch = dir.join("blink");
    std::fs::create_dir_all(&sketch).unwrap();
    std::fs::write(sketch.join("blink.ino"), "void setup(){}\nvoid loop(){}\n").unwrap();
    let base = CompileOptions {
        build_dir: Some(dir.join("build")),
        ..CompileOptions::new("arduino:avr:uno")
    };
    let t0 = Instant::now();
    expect_timeout(
        tc.compile(&sketch, &base.clone().timeout(Duration::from_millis(800))),
        "arduino-cli compile",
    );
    assert!(t0.elapsed() < Duration::from_secs(6));
    let token = CancelToken::new();
    let t2 = token.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        t2.cancel();
    });
    let t0 = Instant::now();
    match tc.compile(&sketch, &base.cancel(token)) {
        Err(FirmwareError::Cancelled(w)) => assert_eq!(w, "arduino-cli compile"),
        other => panic!("expected a cancelled compile, got {other:?}"),
    }
    assert!(t0.elapsed() < Duration::from_secs(6));
}
