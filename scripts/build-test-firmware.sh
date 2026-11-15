#!/usr/bin/env bash
# Compiles every test sketch with arduino-cli and stores .hex + .elf fixtures next to the tests,
# so `cargo test` runs without the toolchain.
#
#   crates/<crate>/tests/firmware/<board>/<sketch>/<sketch>.ino
#     → crates/<crate>/tests/fixtures/<sketch>.<board>.hex / .elf
#
# <board> is uno | nano | mega (`host/` sketches are skipped: ESP32 / Pico tests compile them
# at run time). An optional `build.flags` file in a sketch folder adds
# arduino-cli compile flags; an optional `build.props` file adds one `--build-property` per line
# (e.g. `compiler.cpp.extra_flags=-Og -fno-lto` for debugger fixtures).
#
# On Apple Silicon without Rosetta the Arduino-shipped avr-gcc 7.3.0 and ctags (x86_64) can't
# run; the script then uses native replacements (see bb-firmware's Toolchain::native_overrides):
#   avr-gcc : $BREADBOX_AVR_GCC_BIN, ~/Library/Application Support/Breadbox/toolchain/native/avr-gcc/bin,
#             or Homebrew osx-cross avr-gcc@N + avr-binutils
#   ctags   : $BREADBOX_CTAGS_DIR or ~/Library/Application Support/Breadbox/toolchain/native/ctags
#
# Usage: scripts/build-test-firmware.sh [sketch-name-filter]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FILTER="${1:-}"

if ! command -v arduino-cli >/dev/null 2>&1; then
    echo "arduino-cli not found (brew install arduino-cli; arduino-cli core install arduino:avr)" >&2
    exit 1
fi

DATA_DIR="$(arduino-cli config get directories.data 2>/dev/null || echo "$HOME/Library/Arduino15")"
BUNDLED_GCC="$(ls -d "$DATA_DIR"/packages/arduino/tools/avr-gcc/*/bin 2>/dev/null | tail -1 || true)"
APP_TC="$HOME/Library/Application Support/Breadbox/toolchain/native"

PROPS=()
if [ -n "$BUNDLED_GCC" ] && "$BUNDLED_GCC/avr-gcc" --version >/dev/null 2>&1; then
    echo "using bundled avr-gcc: $BUNDLED_GCC"
else
    GCC_BIN="${BREADBOX_AVR_GCC_BIN:-}"
    if [ -z "$GCC_BIN" ] && [ -x "$APP_TC/avr-gcc/bin/avr-gcc" ]; then
        GCC_BIN="$APP_TC/avr-gcc/bin"
    fi
    if [ -z "$GCC_BIN" ]; then
        for v in 8 9 10 11 12 13 14 15; do
            if [ -x "/opt/homebrew/opt/avr-gcc@$v/bin/avr-gcc" ] && [ -x /opt/homebrew/opt/avr-binutils/bin/avr-objcopy ]; then
                GCC_BIN="$APP_TC/avr-gcc/bin"
                mkdir -p "$GCC_BIN"
                for f in /opt/homebrew/opt/avr-gcc@$v/bin/avr-* /opt/homebrew/opt/avr-binutils/bin/avr-*; do
                    ln -sf "$f" "$GCC_BIN/$(basename "$f")"
                done
                break
            fi
        done
    fi
    if [ -z "$GCC_BIN" ]; then
        echo "bundled avr-gcc cannot run and no native avr-gcc found (brew install osx-cross/avr/avr-gcc@8)" >&2
        exit 1
    fi
    echo "using native avr-gcc: $GCC_BIN ($("$GCC_BIN/avr-gcc" --version | head -1))"
    PROPS+=(--build-property "compiler.path=$GCC_BIN/")
fi

BUNDLED_CTAGS="$(ls -d "$DATA_DIR"/packages/builtin/tools/ctags/* 2>/dev/null | tail -1 || true)"
if [ -n "$BUNDLED_CTAGS" ] && "$BUNDLED_CTAGS/ctags" --version >/dev/null 2>&1; then
    :
else
    CTAGS_DIR="${BREADBOX_CTAGS_DIR:-$APP_TC/ctags}"
    if [ ! -x "$CTAGS_DIR/ctags" ]; then
        echo "building Arduino ctags natively into $CTAGS_DIR"
        TMP="$(mktemp -d)"
        git clone --depth 1 https://github.com/arduino/ctags.git "$TMP/ctags" >/dev/null 2>&1
        (cd "$TMP/ctags" && autoreconf -fi >/dev/null 2>&1 && \
            ./configure CFLAGS="-O2 -Wno-implicit-function-declaration -Wno-int-conversion -Wno-incompatible-pointer-types -Wno-deprecated-non-prototype" >/dev/null && \
            make -j8 >/dev/null 2>&1)
        mkdir -p "$CTAGS_DIR"
        cp "$TMP/ctags/ctags" "$CTAGS_DIR/ctags"
        rm -rf "$TMP"
    fi
    PROPS+=(--build-property "runtime.tools.ctags.path=$CTAGS_DIR")
fi

BUILD_ROOT="$ROOT/target/test-firmware-build"
mkdir -p "$BUILD_ROOT"
count=0
for crate in bb-avr bb-firmware bb-radio bb-sim; do
    FW="$ROOT/crates/$crate/tests/firmware"
    OUT="$ROOT/crates/$crate/tests/fixtures"
    [ -d "$FW" ] || continue
    mkdir -p "$OUT"
    for dir in "$FW"/*/*/; do
        dir="${dir%/}"
        name="$(basename "$dir")"
        board="$(basename "$(dirname "$dir")")"
        if [ -n "$FILTER" ] && [[ "$name" != *"$FILTER"* ]]; then
            continue
        fi
        case "$board" in
            uno) fqbn="arduino:avr:uno" ;;
            nano) fqbn="arduino:avr:nano:cpu=atmega328" ;;
            mega) fqbn="arduino:avr:mega:cpu=atmega2560" ;;
            host) continue ;; # ESP32 / Pico sketches: host-compiled by the tests at run time
            *) echo "unknown board folder $board" >&2; exit 1 ;;
        esac
        flags=()
        if [ -f "$dir/build.flags" ]; then
            read -r -a flags < "$dir/build.flags"
        fi
        if [ -f "$dir/build.props" ]; then
            while IFS= read -r prop || [ -n "$prop" ]; do
                [ -n "$prop" ] && flags+=(--build-property "$prop")
            done < "$dir/build.props"
        fi
        bp="$BUILD_ROOT/$crate-$board-$name"
        echo "== $crate: $name ($fqbn) ${flags[*]:-}"
        arduino-cli compile --fqbn "$fqbn" --build-path "$bp" "${PROPS[@]}" ${flags[@]+"${flags[@]}"} "$dir" | grep -E "^(Sketch uses|Global variables)" || true
        cp "$bp/$name.ino.hex" "$OUT/$name.$board.hex"
        cp "$bp/$name.ino.elf" "$OUT/$name.$board.elf"
        count=$((count + 1))
    done
done
# Example projects: examples/<project>/firmware/<MCU ref>/<sketch>/ with a sketch.yaml naming the
# board (`default_fqbn: arduino:avr:uno`). Host-emulated boards (breadbox:host:*) are compiled by
# bb-firmware's HostBoard at run time, not here. Fixtures → examples/fixtures/<sketch>.<board>.*
EX_OUT="$ROOT/examples/fixtures"
for dir in "$ROOT"/examples/*/firmware/*/*/; do
    dir="${dir%/}"
    [ -f "$dir/sketch.yaml" ] || continue
    name="$(basename "$dir")"
    if [ -n "$FILTER" ] && [[ "$name" != *"$FILTER"* ]]; then
        continue
    fi
    fqbn="$(sed -n 's/^default_fqbn: *//p' "$dir/sketch.yaml" | tr -d '[:space:]')"
    case "$fqbn" in
        arduino:avr:uno) board=uno ;;
        arduino:avr:nano*) board=nano ;;
        arduino:avr:mega*) board=mega ;;
        *) continue ;;
    esac
    mkdir -p "$EX_OUT"
    bp="$BUILD_ROOT/examples-$board-$name"
    echo "== example: $name ($fqbn)"
    arduino-cli compile --fqbn "$fqbn" --build-path "$bp" "${PROPS[@]}" "$dir" | grep -E "^(Sketch uses|Global variables)" || true
    cp "$bp/$name.ino.hex" "$EX_OUT/$name.$board.hex"
    cp "$bp/$name.ino.elf" "$EX_OUT/$name.$board.elf"
    count=$((count + 1))
done
echo "built $count sketches"
