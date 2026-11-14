//! arduino-cli integration: detection, compilation, diagnostics, size report, libraries,
//! installer. Tests that need the toolchain print a message and pass when it is absent.

use bb_avr::{Avr, Mcu, StopOn};
use bb_firmware::{CompileOptions, DebugInfo, Severity, Toolchain, ToolchainSource};
use std::path::{Path, PathBuf};

fn toolchain() -> Option<Toolchain> {
    match Toolchain::detect() {
        Ok(tc) if tc.ready() => Some(tc),
        Ok(tc) => {
            eprintln!(
                "SKIP: arduino-cli found but not ready (core {:?}, gcc {:?})",
                tc.avr_core_version, tc.avr_gcc_version
            );
            None
        }
        Err(e) => {
            eprintln!("SKIP: {e}");
            None
        }
    }
}

fn sketch(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir()
        .join(format!("bbfw-tests-{}", std::process::id()))
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (f, text) in files {
        std::fs::write(dir.join(f), text).unwrap();
    }
    dir
}

fn run_serial(elf: &[u8], mcu: Mcu, millis: u64) -> String {
    let mut avr = Avr::with_mcu(mcu);
    avr.set_stop_on(StopOn {
        pin_change: false,
        ..StopOn::default()
    });
    avr.load_elf(elf).unwrap();
    let r = avr.run(millis * 16_000, &mut ());
    assert_eq!(r, bb_avr::StopReason::ReachedCycle);
    avr.take_usart_tx(0)
        .iter()
        .map(|b| b.data as u8 as char)
        .collect()
}

/// What `arduino-cli compile` prints in text mode for the same sketch/board.
fn cli_size_lines(tc: &Toolchain, dir: &Path, fqbn: &str) -> Vec<String> {
    let out = tc
        .command()
        .args(["compile", "--fqbn", fqbn])
        .args(tc.build_property_args())
        .arg("--build-path")
        .arg(dir.join("../cli-text-build"))
        .arg(dir)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| l.starts_with("Sketch uses") || l.starts_with("Global variables"))
        .map(|l| l.to_string())
        .collect()
}

#[test]
fn detect_reports_cli_core_and_compiler() {
    let Some(tc) = toolchain() else { return };
    assert!(tc.arduino_cli.is_file());
    assert!(!tc.cli_version.is_empty());
    assert!(tc.avr_core_version.is_some());
    assert!(
        tc.compiler_label().starts_with("avr-gcc "),
        "{}",
        tc.compiler_label()
    );
    println!(
        "arduino-cli {} ({:?}) · arduino:avr {} · {} · overrides {:?}",
        tc.cli_version,
        tc.source,
        tc.avr_core_version.as_deref().unwrap_or("-"),
        tc.compiler_label(),
        tc.overrides
    );
}

#[test]
fn compiles_multi_tab_sketch_with_size_report_and_runs_it() {
    let Some(tc) = toolchain() else { return };
    let dir = sketch(
        "multitab",
        &[
            (
                "multitab.ino",
                "#include \"util.h\"\n#include \"mathx.h\"\n\nvoid setup() {\n  Serial.begin(115200);\n  Serial.print(\"twice=\");\n  Serial.print(twice(5));\n  Serial.print(\" add=\");\n  Serial.print(add(3, 4));\n  Serial.print(\" triple=\");\n  Serial.println(triple(3));\n}\n\nvoid loop() {}\n",
            ),
            (
                "helper.ino",
                "// second tab: no prototype needed thanks to arduino-cli\nint twice(int v) {\n  return v * 2;\n}\n",
            ),
            (
                "util.h",
                "#pragma once\ninline int add(int a, int b) { return a + b; }\n",
            ),
            ("mathx.h", "#pragma once\nint triple(int v);\n"),
            (
                "mathx.cpp",
                "#include \"mathx.h\"\nint triple(int v) { return 3 * v; }\n",
            ),
        ],
    );
    let out = tc
        .compile(&dir, &CompileOptions::new("arduino:avr:uno"))
        .unwrap();
    assert!(out.success, "{:#?}\n{}", out.diagnostics, out.log);
    assert!(
        out.diagnostics
            .iter()
            .all(|d| d.severity != Severity::Error)
    );
    let size = out.size.unwrap();
    assert_eq!(size.program_max, 32_256);
    assert_eq!(size.data_max, 2_048);
    // Our size report reproduces arduino-cli's text exactly.
    assert_eq!(
        size.arduino_lines(),
        cli_size_lines(&tc, &dir, "arduino:avr:uno")
    );
    assert!(
        out.banner
            .starts_with("Compiling sketch for UNO R3 (avr-gcc "),
        "{}",
        out.banner
    );
    assert!(out.banner.ends_with("-Os -mmcu=atmega328p)…"));
    let lines = out.output_lines();
    assert!(
        lines
            .iter()
            .any(|(_, l)| l.starts_with("Sketch uses ") && l.contains(" bytes ("))
    );
    assert!(out.hex.as_ref().unwrap().starts_with(':'));
    assert_eq!(
        run_serial(out.elf.as_ref().unwrap(), Mcu::Atmega328p, 20),
        "twice=10 add=7 triple=9\r\n"
    );
    out.image().unwrap();
}

#[test]
fn diagnostics_map_back_to_tabs() {
    let Some(tc) = toolchain() else { return };
    let dir = sketch(
        "broken",
        &[
            (
                "broken.ino",
                "void setup() {\n  pinMode(13, OUTPUT);\n}\n\nvoid loop() {\n  digitalWrite(13, HIGH);\n  missing_function(1);\n}\n",
            ),
            (
                "second.ino",
                "int helper(int x) {\n  int unused_local = 4;\n  return x + undefined_var;\n}\n",
            ),
        ],
    );
    let mut opts = CompileOptions::new("arduino:avr:uno");
    opts.warnings = bb_firmware::Warnings::All;
    let out = tc.compile(&dir, &opts).unwrap();
    assert!(!out.success);
    assert!(out.elf.is_none());
    let errs: Vec<_> = out
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    let main = errs
        .iter()
        .find(|d| d.tab.as_deref() == Some("broken.ino"))
        .expect("error in broken.ino");
    assert_eq!(main.line, Some(7));
    assert_eq!(main.column, Some(3));
    assert!(
        main.message.contains("missing_function"),
        "{}",
        main.message
    );
    let second = errs
        .iter()
        .find(|d| d.tab.as_deref() == Some("second.ino"))
        .expect("error in second tab");
    assert_eq!(second.line, Some(3));
    assert!(second.message.contains("undefined_var"));
    let warn = out
        .diagnostics
        .iter()
        .find(|d| d.severity == Severity::Warning && d.tab.as_deref() == Some("second.ino"));
    assert_eq!(
        warn.and_then(|w| w.line),
        Some(2),
        "unused variable warning"
    );
    let lines = out.output_lines();
    assert!(
        lines
            .iter()
            .any(|(_, l)| l.starts_with("broken.ino:7:3: error:")),
        "{lines:#?}"
    );
}

#[test]
fn build_cache_makes_reverify_fast() {
    let Some(tc) = toolchain() else { return };
    let dir = sketch(
        "cached",
        &[(
            "cached.ino",
            "void setup() { pinMode(13, OUTPUT); }\nvoid loop() { digitalWrite(13, millis() & 512); }\n",
        )],
    );
    let opts = CompileOptions::new("arduino:avr:uno");
    let first = tc.compile(&dir, &opts).unwrap();
    let second = tc.compile(&dir, &opts).unwrap();
    assert!(first.success && second.success);
    assert_eq!(first.build_dir, second.build_dir);
    assert_eq!(first.elf, second.elf);
    println!(
        "verify: first {:?}, cached {:?}",
        first.duration, second.duration
    );
    assert!(
        second.duration <= first.duration,
        "cached re-verify should not be slower"
    );
}

#[test]
fn debug_build_keeps_locals_visible() {
    let Some(tc) = toolchain() else { return };
    let dir = sketch(
        "dbgbuild",
        &[(
            "dbgbuild.ino",
            "volatile int g;\nint f(int a) {\n  int b = a * 2;\n  return b + 1;\n}\nvoid setup() {}\nvoid loop() {\n  int x = g;\n  g = f(x);\n}\n",
        )],
    );
    let out = tc
        .compile(&dir, &CompileOptions::new("arduino:avr:uno").debug(true))
        .unwrap();
    assert!(out.success, "{}", out.log);
    assert!(out.banner.contains("-Og"));
    let info = DebugInfo::from_elf(out.elf.as_ref().unwrap()).unwrap();
    let f = info.function_by_name("f").expect("f kept (no LTO)");
    let names: Vec<&str> = f.vars.iter().map(|v| v.name.as_str()).collect();
    assert!(names.contains(&"a") && names.contains(&"b"), "{names:?}");
    assert!(info.line_addresses("dbgbuild.ino", 9).is_some());
}

#[test]
fn libraries_list_search_and_arduinojson7() {
    let Some(tc) = toolchain() else { return };
    let installed = tc.lib_list().unwrap();
    assert!(
        installed.iter().any(|l| l.name == "Servo"),
        "Servo installed by build-test-firmware setup"
    );
    assert!(
        installed.iter().any(|l| l.name == "Wire"),
        "platform-bundled libraries are listed"
    );
    match tc.lib_search("ArduinoJson") {
        Ok(found) => assert!(
            found
                .iter()
                .any(|l| l.name == "ArduinoJson" && l.version.starts_with('7')),
            "{found:?}"
        ),
        Err(e) => eprintln!("lib search unavailable (offline?): {e}"),
    }
    if !installed.iter().any(|l| l.name == "ArduinoJson") {
        eprintln!("SKIP: ArduinoJson not installed");
        return;
    }
    let dir = sketch(
        "jsonsketch",
        &[(
            "jsonsketch.ino",
            "#include <ArduinoJson.h>\nvoid setup() {\n  Serial.begin(115200);\n  JsonDocument doc;\n  doc[\"moisture\"] = 42;\n  doc[\"ok\"] = true;\n  serializeJson(doc, Serial);\n  Serial.println();\n}\nvoid loop() {}\n",
        )],
    );
    let out = tc
        .compile(&dir, &CompileOptions::new("arduino:avr:uno"))
        .unwrap();
    assert!(out.success, "{:#?}", out.diagnostics);
    assert_eq!(
        run_serial(out.elf.as_ref().unwrap(), Mcu::Atmega328p, 30),
        "{\"moisture\":42,\"ok\":true}\r\n"
    );
}

#[test]
fn compiles_for_mega_2560() {
    let Some(tc) = toolchain() else { return };
    let dir = sketch(
        "megasketch",
        &[(
            "megasketch.ino",
            "void setup() { Serial2.begin(9600); Serial.begin(9600); Serial.println(F(\"mega\")); }\nvoid loop() {}\n",
        )],
    );
    let out = tc
        .compile(
            &dir,
            &CompileOptions::new("arduino:avr:mega:cpu=atmega2560"),
        )
        .unwrap();
    assert!(out.success);
    let size = out.size.unwrap();
    assert_eq!(size.program_max, 253_952);
    assert_eq!(size.data_max, 8_192);
    assert!(out.banner.contains("Mega 2560") && out.banner.contains("atmega2560"));
    assert_eq!(
        run_serial(out.elf.as_ref().unwrap(), Mcu::Atmega2560, 30),
        "mega\r\n"
    );
}

#[test]
fn install_cli_from_archive_into_app_dir() {
    let Some(tc) = toolchain() else { return };
    // Package the detected binary like a release archive and install it from file://.
    let root = std::env::temp_dir().join(format!("bbfw-install-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let archive = root.join("arduino-cli.tar.gz");
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
        std::fs::File::create(&archive).unwrap(),
        flate2::Compression::fast(),
    ));
    builder
        .append_path_with_name(&tc.arduino_cli, "arduino-cli")
        .unwrap();
    builder.into_inner().unwrap().finish().unwrap();
    let app = root.join("toolchain");
    let mut events = Vec::new();
    let fresh = Toolchain::install_cli(&app, &format!("file://{}", archive.display()), &mut |e| {
        events.push(e)
    })
    .unwrap();
    assert_eq!(fresh.source, ToolchainSource::AppSupport);
    assert_eq!(
        fresh.config_file.as_deref(),
        Some(app.join("arduino-cli.yaml").as_path())
    );
    assert_eq!(fresh.data_dir, app.join("data"));
    assert!(
        fresh.avr_core_version.is_none(),
        "private data dir has no cores yet"
    );
    assert!(!fresh.ready());
    assert!(
        events
            .iter()
            .any(|e| matches!(e, bb_firmware::InstallProgress::Download { .. }))
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Full one-click install (downloads arduino-cli + the AVR core, ~100 MB). Run manually:
/// `cargo test -p bb-firmware --test toolchain -- --ignored full_install`.
#[test]
#[ignore]
fn full_install_downloads_cli_and_core() {
    let dir = std::env::temp_dir().join(format!("bbfw-full-install-{}", std::process::id()));
    let tc = Toolchain::install(&dir, &mut |e| println!("{e:?}")).unwrap();
    assert!(tc.avr_core_version.is_some());
}
