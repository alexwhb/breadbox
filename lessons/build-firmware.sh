#!/usr/bin/env bash
# Compiles the lessons' AVR sketches (lessons/<track>/sketches/<name>/ with a sketch.yaml naming
# an arduino:avr board) into lessons/<track>/firmware/<name>.uno.hex, so lessons and their tests
# run without the toolchain. Host-board sketches (breadbox:host:*) are built at run time.
#
# Usage: lessons/build-firmware.sh [sketch-name-filter]
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

BUILD_ROOT="$ROOT/target/lesson-firmware-build"
mkdir -p "$BUILD_ROOT"
count=0
for dir in "$ROOT"/lessons/*/sketches/*/; do
    dir="${dir%/}"
    name="$(basename "$dir")"
    track="$(dirname "$(dirname "$dir")")"
    [ -f "$dir/sketch.yaml" ] || continue
    fqbn="$(sed -n 's/^default_fqbn: *//p' "$dir/sketch.yaml")"
    case "$fqbn" in
        arduino:avr:uno) board=uno ;;
        arduino:avr:nano*) board=nano ;;
        *) continue ;;
    esac
    if [ -n "$FILTER" ] && [[ "$name" != *"$FILTER"* ]]; then
        continue
    fi
    bp="$BUILD_ROOT/$(basename "$track")-$name"
    echo "== $(basename "$track"): $name ($fqbn)"
    arduino-cli compile --fqbn "$fqbn" --build-path "$bp" "${PROPS[@]}" "$dir" | grep -E "^(Sketch uses|Global variables)" || true
    mkdir -p "$track/firmware"
    cp "$bp/$name.ino.hex" "$track/firmware/$name.$board.hex"
    count=$((count + 1))
done
echo "built $count lesson sketch(es)"
