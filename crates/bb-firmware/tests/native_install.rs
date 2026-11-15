//! `MCU-4` on Apple Silicon without Rosetta: the installer brings native avr-gcc (Homebrew
//! osx-cross) and builds Arduino ctags from source, automatically, with progress and clear
//! errors. Exercised offline with a fake `brew` and a fake ctags source archive; the real
//! download + build runs with `--ignored`.

use bb_firmware::{
    FirmwareError, InstallProgress, NativeInstallOptions, NativeOverrides, Timeouts, Toolchain,
    ToolchainSource, build_ctags,
};
use std::io::Write;
use std::path::{Path, PathBuf};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("bb-native-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn script(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A Toolchain whose arduino:avr core is installed but whose bundled tools can't run.
fn broken_toolchain(dir: &Path) -> Toolchain {
    Toolchain {
        arduino_cli: dir.join("bin/arduino-cli"),
        cli_version: "1.3.1".into(),
        source: ToolchainSource::AppSupport,
        config_file: None,
        data_dir: dir.join("data"),
        user_dir: dir.join("sketchbook"),
        downloads_dir: dir.join("staging"),
        avr_core_version: Some("1.8.8".into()),
        avr_gcc_version: None,
        overrides: NativeOverrides::default(),
        timeouts: Timeouts::default(),
    }
}

/// A gzip'd tarball of a tiny "ctags" source tree with a configure script and Makefile.
fn fake_ctags_source(dir: &Path) -> PathBuf {
    let src = dir.join("ctags-src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("configure"),
        "printf 'ctags: main.c general.h\\n\\tcc $(CFLAGS) -o ctags main.c\\n' > Makefile\n",
    )
    .unwrap();
    // The real source defines __unused__, which breaks against the macOS SDK; the installer
    // must rename it.
    std::fs::write(
        src.join("general.h"),
        "#define __unused__ __attribute__((unused))\n",
    )
    .unwrap();
    std::fs::write(
        src.join("main.c"),
        "#include <stdio.h>\n#include \"general.h\"\nint main(int argc, char** argv) { int x __unused__ = 0; (void)argv; if (argc > 1) printf(\"Exuberant Ctags 5.8 (fake)\\n\"); return 0; }\n",
    )
    .unwrap();
    let tgz = dir.join("ctags.tar.gz");
    let f = std::fs::File::create(&tgz).unwrap();
    let mut b = tar::Builder::new(flate2::write::GzEncoder::new(
        f,
        flate2::Compression::fast(),
    ));
    b.append_dir_all("ctags-5.8-arduino11", &src).unwrap();
    b.into_inner().unwrap().finish().unwrap().flush().unwrap();
    tgz
}

#[test]
fn installs_native_gcc_with_brew_and_builds_ctags() {
    let dir = tmp("ok");
    let prefix = dir.join("brew-prefix");
    // Fake Homebrew: `brew install osx-cross/avr/avr-gcc@8` creates the two kegs.
    script(
        &dir.join("bin/brew"),
        &format!(
            r#"[ "$1 $2" = "install osx-cross/avr/avr-gcc@8" ] || exit 1
echo "==> Installing avr-gcc@8 from osx-cross/avr"
mkdir -p "{p}/opt/avr-gcc@8/bin" "{p}/opt/avr-binutils/bin"
printf '#!/bin/sh\necho "avr-gcc (Homebrew AVR GCC 8.5.0_5) 8.5.0"\n' > "{p}/opt/avr-gcc@8/bin/avr-gcc"
printf '#!/bin/sh\nexit 0\n' > "{p}/opt/avr-binutils/bin/avr-objcopy"
chmod +x "{p}/opt/avr-gcc@8/bin/avr-gcc" "{p}/opt/avr-binutils/bin/avr-objcopy"
echo "==> Summary: avr-gcc@8 8.5.0_5""#,
            p = prefix.display()
        ),
    );
    let tgz = fake_ctags_source(&dir);
    let mut tc = broken_toolchain(&dir);
    assert!(tc.needs_native_tools() && !tc.ready());
    let opts = NativeInstallOptions {
        brew: Some(dir.join("bin/brew")),
        brew_prefixes: vec![prefix.clone()],
        ctags_url: format!("file://{}", tgz.display()),
        app_dir: dir.join("toolchain"),
    };
    let mut steps = vec![];
    let mut logs = vec![];
    tc.install_native_tools_with(&opts, &mut |p| match p {
        InstallProgress::Step(s) => steps.push(s),
        InstallProgress::Log(l) => logs.push(l),
        _ => {}
    })
    .unwrap();
    assert_eq!(
        steps,
        vec![
            "Installing avr-gcc 8 for Apple Silicon (Homebrew osx-cross/avr)".to_string(),
            "Downloading Arduino ctags source".to_string(),
            "Building Arduino ctags for this Mac".to_string(),
            "Native toolchain ready (avr-gcc 8.5.0)".to_string(),
        ]
    );
    assert!(logs.iter().any(|l| l.contains("Installing avr-gcc@8")));
    assert!(tc.ready() && !tc.needs_native_tools());
    assert_eq!(tc.compiler_label(), "avr-gcc 8.5.0");
    let bin = tc.overrides.compiler_bin.clone().unwrap();
    assert_eq!(bin, dir.join("toolchain/native/avr-gcc/bin"));
    assert!(
        bin.join("avr-objcopy").exists(),
        "merged bin dir links binutils"
    );
    assert_eq!(
        tc.overrides.ctags_dir.clone().unwrap(),
        dir.join("toolchain/native/ctags")
    );
    let args = tc.build_property_args();
    assert!(args.contains(&format!("compiler.path={}/", bin.display())));
    // Second run: everything already present, no brew call.
    let mut tc2 = broken_toolchain(&dir);
    let opts2 = NativeInstallOptions { brew: None, ..opts };
    let mut steps2 = vec![];
    tc2.install_native_tools_with(&opts2, &mut |p| {
        if let InstallProgress::Step(s) = p {
            steps2.push(s)
        }
    })
    .unwrap_or_else(|e| panic!("{e}"));
    assert!(steps2[0].starts_with("Using native avr-gcc in "));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn clear_error_without_rosetta_or_homebrew() {
    let dir = tmp("nobrew");
    let mut tc = broken_toolchain(&dir);
    let opts = NativeInstallOptions {
        brew: None,
        brew_prefixes: vec![dir.join("none")],
        ctags_url: "file:///nonexistent".into(),
        app_dir: dir.join("toolchain"),
    };
    let err = tc
        .install_native_tools_with(&opts, &mut |_| {})
        .unwrap_err();
    let FirmwareError::NativeTools(msg) = err else {
        panic!("{err:?}")
    };
    assert!(
        msg.contains("Rosetta")
            && msg.contains("Homebrew")
            && msg.contains("softwareupdate --install-rosetta"),
        "{msg}"
    );
    // A brew that fails is reported with its last line.
    script(
        &dir.join("bin/brew"),
        "echo 'Error: No available formula' >&2; exit 1",
    );
    let opts = NativeInstallOptions {
        brew: Some(dir.join("bin/brew")),
        ..opts
    };
    let err = tc
        .install_native_tools_with(&opts, &mut |_| {})
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Homebrew could not install avr-gcc@8: Error: No available formula"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Downloads and builds the real Arduino ctags (network).
#[test]
#[ignore]
fn builds_real_arduino_ctags() {
    let dir = tmp("real");
    let bin = build_ctags(
        bb_firmware::CTAGS_SOURCE_URL,
        &dir.join("ctags"),
        &mut |_| {},
    )
    .unwrap();
    let out = std::process::Command::new(&bin)
        .arg("--version")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("Exuberant Ctags"));
    let _ = std::fs::remove_dir_all(&dir);
}
