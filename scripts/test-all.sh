#!/usr/bin/env bash
# scripts/test-all.sh — the single green-light command (SPEC TEST-7).
#
#   fmt        cargo fmt --check (workspace + plugins)
#   clippy     cargo clippy --workspace --all-targets -D warnings
#   test       cargo test --workspace: unit, SPICE reference vs ngspice golden data (TEST-2),
#              AVR instruction/peripheral/real-firmware tests (TEST-3), headless e2e scenarios
#              in tests/e2e (TEST-4), GUI tests with egui_kittest (TEST-5); a test running
#              longer than 60 s fails the step
#   plugins    the wasm plugin workspace: clippy -D warnings for wasm32, committed .wasm files
#              rebuild byte-identically (scripts/build-plugins.sh --check)
#   perf       release perf gates (TEST-6): MCU-3 ≥ 3× real time, SIM-4 garden-node ≥ 1×
#              (engine, and in the app with the GUI drawing), SPICE-5 1 000-node .op < 50 ms,
#              Workbench frame time, PLAT-4 start-up (first frame) < 2 s
#   bundle     macOS only: scripts/bundle-macos.sh --check — builds dist/Breadbox.app (PLAT-3)
#              and runs scripts/check-bundle.sh: signature, Info.plist (incl. the .breadbox
#              document package), resources, no machine paths, sandboxed smoke runs with the
#              checkout denied, start-up via `open`, documents handed over by Finder
#
# Graceful without optional tools: ngspice (golden regeneration / .cir checks), arduino-cli
# (fresh compiles; prebuilt fixtures are used), a C++ compiler (ESP32 / Pico host boards), a
# display (the smoke run falls back to drawing without a window; BREADBOX_HEADLESS=1 forces it).
#
#   scripts/test-all.sh              everything
#   scripts/test-all.sh --quick      skip the release perf gates and the bundle
#   SKIP=perf,plugins,bundle scripts/test-all.sh
set -uo pipefail
cd "$(dirname "$0")/.."
ROOT="$PWD"
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target/test-all}"
case "$CARGO_TARGET_DIR" in /*) ;; *) CARGO_TARGET_DIR="$ROOT/$CARGO_TARGET_DIR" ;; esac
LOGS="$CARGO_TARGET_DIR/test-all-logs"
mkdir -p "$LOGS"
SKIP="${SKIP:-}"
[ "${1:-}" = "--quick" ] && SKIP="$SKIP,perf,bundle"

if [ -t 1 ]; then G=$'\e[32m'; R=$'\e[31m'; Y=$'\e[33m'; B=$'\e[1m'; N=$'\e[0m'; else G= R= Y= B= N=; fi

STEPS=()     # "name|status|seconds|note"
PERF=()      # "gate|value|limit|status"
FAILED=0

skipped() { [[ ",$SKIP," == *",$1,"* ]]; }

# run_step <name> <log> <command…>
run_step() {
  local name="$1" log="$LOGS/$2.log"; shift 2
  printf '%s── %s%s\n' "$B" "$name" "$N"
  local t0=$SECONDS
  "$@" >"$log" 2>&1
  local rc=$? dt=$((SECONDS - t0))
  if [ $rc -eq 0 ]; then
    printf '   %sok%s (%ss)\n' "$G" "$N" "$dt"
    STEPS+=("$name|PASS|$dt|")
  else
    printf '   %sFAILED%s (%ss) — log: %s\n' "$R" "$N" "$dt" "$log"
    tail -n 25 "$log" | sed 's/^/   │ /'
    STEPS+=("$name|FAIL|$dt|see $log")
    FAILED=1
  fi
  return $rc
}

skip_step() { printf '%s── %s%s\n   %sskipped%s: %s\n' "$B" "$1" "$N" "$Y" "$N" "$2"; STEPS+=("$1|SKIP|0|$2"); }

# perf_gate <gate> <value> <op> <limit> <unit>
perf_gate() {
  local gate="$1" v="$2" op="$3" lim="$4" unit="$5" st
  if [ -z "$v" ]; then st=FAIL; v="?";
  elif awk -v v="$v" -v l="$lim" -v op="$op" 'BEGIN { exit !((op == ">=") ? (v >= l) : (v < l)) }'; then st=PASS;
  else st=FAIL; fi
  [ "$st" = FAIL ] && FAILED=1
  PERF+=("$gate|$v $unit|$op $lim $unit|$st")
}

# ------------------------------------------------------------------ environment
echo "${B}Breadbox test-all${N}  ($(date '+%Y-%m-%d %H:%M'), target dir $CARGO_TARGET_DIR)"
echo "  rustc        $(rustc --version 2>/dev/null)"
NGSPICE="$(command -v ngspice || true)"
echo "  ngspice      ${NGSPICE:-absent (golden data is checked in; .cir checks run bb-spice only)}"
CLI="$(command -v arduino-cli || ls "$HOME/Library/Application Support/Breadbox/toolchain/bin/arduino-cli" 2>/dev/null || true)"
echo "  arduino-cli  ${CLI:-absent (prebuilt firmware fixtures are used)}"
CXX="${BREADBOX_CXX:-$(command -v clang++ || command -v c++ || true)}"
echo "  C++          ${CXX:-absent (ESP32 / Pico host-board tests skip)}"
echo

# ------------------------------------------------------------------ fmt
if skipped fmt; then skip_step "fmt" "SKIP"; else
  run_step "fmt (workspace + plugins)" fmt bash -c 'cargo fmt --all --check && (cd plugins && cargo fmt --all --check)'
fi

# ------------------------------------------------------------------ clippy
if skipped clippy; then skip_step "clippy" "SKIP"; else
  run_step "clippy --workspace --all-targets -D warnings" clippy \
    cargo clippy --workspace --all-targets -- -D warnings
fi

# ------------------------------------------------------------------ tests
if skipped test; then skip_step "test" "SKIP"; else
  if run_step "cargo test --workspace (unit · reference · AVR firmware · e2e · GUI)" test \
      cargo test --workspace --no-fail-fast; then
    if grep -q "has been running for over 60 seconds" "$LOGS/test.log"; then
      echo "   ${R}slow tests${N}:"; grep "has been running for over 60 seconds" "$LOGS/test.log" | sed 's/^/   │ /'
      STEPS[${#STEPS[@]}-1]="cargo test --workspace|FAIL|0|a test ran longer than 60 s"
      FAILED=1
    fi
  fi
  passed=$(grep -E "^test result: " "$LOGS/test.log" | awk '{s+=$4} END {print s+0}')
  failed=$(grep -E "^test result: " "$LOGS/test.log" | awk '{s+=$6} END {print s+0}')
  ignored=$(grep -E "^test result: " "$LOGS/test.log" | awk '{s+=$8} END {print s+0}')
  e2e=$(awk '/Running tests\/[a-z_]+\.rs .*breadbox-e2e/{f=1} f && /^test result: /{s+=$4; f=0} END {print s+0}' "$LOGS/test.log")
  echo "   $passed passed, $failed failed, $ignored ignored (perf gates run below); e2e scenarios: $e2e"
fi

# ------------------------------------------------------------------ plugins (wasm)
if skipped plugins; then skip_step "plugins" "SKIP"; else
  run_step "plugins: clippy wasm32 -D warnings" plugins-clippy bash -c \
    'cd plugins && CARGO_TARGET_DIR=target cargo clippy --workspace --release --target wasm32-unknown-unknown -- -D warnings'
  run_step "plugins: committed .wasm rebuild byte-identically" plugins-wasm scripts/build-plugins.sh --check
fi

# ------------------------------------------------------------------ perf (release)
if skipped perf; then skip_step "perf (release)" "SKIP / --quick"; else
  run_step "perf: MCU-3 · SIM-4 · SPICE-5 (release, ignored tests)" perf \
    cargo test --release --workspace --test perf -- --ignored --nocapture --test-threads=1
  P="$LOGS/perf.log"
  # the slower of the two runs (logging only / stopping on every pin change like the co-sim)
  mcu=$(grep -Eo 'busy sketch.*→ [0-9.]+×' "$P" | grep -Eo '→ [0-9.]+×' | grep -Eo '[0-9.]+' | sort -n | head -1)
  perf_gate "MCU-3 busy ATmega328P sketch" "$mcu" ">=" 3 "× real time"
  # garden-node alone and with 60 Hz telemetry snapshots: the slower of the two
  sim=$(grep -Eo 'garden-node(: [0-9]+ s simulated| \+ 60 Hz telemetry).*→ [0-9.]+×' "$P" | grep -Eo '→ [0-9.]+×' | grep -Eo '[0-9.]+' | sort -n | head -1)
  perf_gate "SIM-4 garden-node (engine + telemetry)" "$sim" ">=" 1 "× real time"
  spice=$(grep -Eo 'mesh \.op: [0-9.]+ ms' "$P" | grep -Eo '[0-9.]+' | tail -1)
  perf_gate "SPICE-5 1 024-node .op" "$spice" "<" 50 "ms"

  run_step "perf: Workbench frame time with garden-node running (release GUI test)" perf-gui \
    cargo test --release --workspace --test workbench -- --ignored perf --nocapture
  frame=$(grep -Eo 'CPU per frame avg [0-9.]+ ms' "$LOGS/perf-gui.log" | grep -Eo '[0-9.]+' | head -1)
  perf_gate "Workbench frame (UI + tessellation)" "$frame" "<" 16.7 "ms"

  if run_step "perf: build the release app" build-release cargo build --release --workspace --bins; then
    BIN="$CARGO_TARGET_DIR/release/breadbox"
    run_step "smoke: garden-node at max speed with the GUI open" smoke-max \
      "$BIN" --smoke garden-node --seconds 10 --speed max
    line=$(grep '^SMOKE' "$LOGS/smoke-max.log" | tail -1); echo "   $line"
    ff=$(sed -n 's/.*first_frame_ms=\([0-9.]*\).*/\1/p' <<<"$line")
    rt=$(sed -n 's/.* realtime=\([0-9.]*\).*/\1/p' <<<"$line")
    perf_gate "PLAT-4 start-up → first frame" "$ff" "<" 2000 "ms"
    perf_gate "SIM-4 garden-node with the GUI (max)" "$rt" ">=" 1 "× real time"
    run_step "smoke: garden-node paced at 1× with the GUI open" smoke-1x \
      "$BIN" --smoke garden-node --seconds 4 --speed 1
    line=$(grep '^SMOKE' "$LOGS/smoke-1x.log" | tail -1); echo "   $line"
    rt1=$(sed -n 's/.* realtime=\([0-9.]*\).*/\1/p' <<<"$line")
    perf_gate "SIM-4 garden-node holds 1× with the GUI" "$rt1" ">=" 0.97 "× real time"
    run_step "smoke: esp32-weather (API-level host board)" smoke-esp32 \
      "$BIN" --smoke esp32-weather --seconds 5 --speed max
    grep '^SMOKE' "$LOGS/smoke-esp32.log" | tail -1 | sed 's/^/   /'
  fi
fi

# ------------------------------------------------------------------ bundle (macOS app)
if skipped bundle; then skip_step "bundle" "SKIP / --quick"
elif [ "$(uname)" != Darwin ]; then skip_step "bundle" "not macOS"
else
  run_step "bundle: dist/Breadbox.app + check-bundle (sandboxed smoke runs, Finder package)" bundle \
    scripts/bundle-macos.sh --check
  grep -E '^  (ok|FAIL) ' "$LOGS/bundle.log" | grep -E 'FAIL|first frame|package|double-click' | sed 's/^/  /'
fi

# ------------------------------------------------------------------ summary
echo
echo "${B}Summary${N}"
for s in "${STEPS[@]}"; do
  IFS='|' read -r name st dt note <<<"$s"
  case "$st" in PASS) c=$G ;; FAIL) c=$R ;; *) c=$Y ;; esac
  printf '  %s%-5s%s %4ss  %s%s\n' "$c" "$st" "$N" "$dt" "$name" "${note:+  ($note)}"
done
if [ ${#PERF[@]} -gt 0 ]; then
  echo
  echo "${B}Performance gates (TEST-6)${N}"
  for p in "${PERF[@]}"; do
    IFS='|' read -r gate v lim st <<<"$p"
    case "$st" in PASS) c=$G ;; *) c=$R ;; esac
    printf '  %s%-5s%s %-42s %-18s (%s)\n' "$c" "$st" "$N" "$gate" "$v" "$lim"
  done
fi
echo
if [ $FAILED -eq 0 ]; then
  echo "${G}${B}ALL GREEN${N}"
else
  echo "${R}${B}FAILED${N} — logs in $LOGS"
fi
exit $FAILED
