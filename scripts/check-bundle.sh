#!/usr/bin/env bash
# Proves a built Breadbox.app is self-contained (PLAT-3) and starts fast (PLAT-4).
#
#   scripts/check-bundle.sh [path/to/Breadbox.app] [--no-window]
#
# 1. Static: signature, Info.plist, the resources the app loads, and no path of this machine
#    ($HOME, the checkout) in the binary or any bundled text file.
# 2. Runtime "no repo paths": the app is copied out of the checkout and every smoke run happens
#    under sandbox-exec with the checkout denied (read + write) and the bundle read-only, from a
#    scratch working directory, with scratch Application Support / Caches folders:
#      garden-node (bundled example, by path)        5 s, headless
#      lesson:ohms-law-led (lesson bench + DMM meter) 3 s, headless
#      garden-node --place community.dmm-handheld    plugin instrument, reads A0
#      esp32-weather                                 host-compiled ESP32 (bundled host core)
#      garden-node in a real window                  wgpu, first frame
#    Any output line mentioning the checkout fails the check.
# 3. The real app: `open` on the copied bundle (scratch user data), time to first frame
#    (< 2 s, PLAT-4), then quit it.
set -euo pipefail
cd "$(dirname "$0")/.."
repo="$(pwd -P)"
app="$repo/dist/Breadbox.app"
window=1
for a in "$@"; do
  case "$a" in
    --no-window) window=0 ;;
    -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
    *) app="$a" ;;
  esac
done
[ -d "$app" ] || { echo "no app at $app (run scripts/bundle-macos.sh)" >&2; exit 2; }
app="$(cd "$app" && pwd -P)"

failures=0
ok() { printf '  ok    %s\n' "$*"; }
bad() { printf '  FAIL  %s\n' "$*"; failures=$((failures + 1)); }
section() { printf '\n\033[1m%s\033[0m\n' "$*"; }
now() { perl -MTime::HiRes=time -e 'printf "%.3f", time'; }

section "Static checks ($app)"
contents="$app/Contents"
res="$contents/Resources"
bin="$contents/MacOS/breadbox"
if codesign --verify --deep --strict "$app" 2>/dev/null; then ok "code signature (ad hoc) verifies"; else bad "codesign --verify"; fi
plist="$contents/Info.plist"
if plutil -lint "$plist" >/dev/null; then ok "Info.plist lints"; else bad "Info.plist"; fi
want_version=$(cargo metadata --no-deps --format-version 1 2>/dev/null \
  | python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"]=="breadbox"))' || echo "?")
for kv in "CFBundleIdentifier=app.breadbox.Breadbox" "CFBundleExecutable=breadbox" \
          "LSMinimumSystemVersion=14.0" "CFBundleShortVersionString=$want_version" \
          "NSHighResolutionCapable=true" "CFBundleIconFile=Breadbox"; do
  k="${kv%%=*}"; v="${kv#*=}"
  got=$(plutil -extract "$k" raw "$plist" 2>/dev/null || echo missing)
  if [ "$got" = "$v" ]; then ok "$k = $v"; else bad "$k = $got (want $v)"; fi
done
for f in Breadbox.icns examples/garden-node/project.json examples/esp32-weather/project.json \
         examples/fixtures/garden-node.uno.hex lessons/1-electricity/track.toml \
         lessons/1-electricity/03-ohms-law-led.toml lessons/1-electricity/firmware/blink.uno.hex \
         plugins/catalog.json plugins/dmm-handheld/plugin.json plugins/dmm-handheld/dmm.wasm \
         plugins/examples/template/plugin.json crates/bb-plugin-sdk/src/lib.rs \
         docs/PLUGIN-SDK.md docs/LESSON-FORMAT.md docs/README.md fonts/Inter-Regular.ttf \
         fonts/Inter-OFL.txt hostcore/Arduino.h licenses/THIRD-PARTY.md build-info.json; do
  if [ -e "$res/$f" ]; then :; else bad "missing Resources/$f"; fi
done
ok "Resources: examples, lessons, plugins, SDK, docs, fonts + licences, hostcore"
plugins=$(find "$res/plugins" -maxdepth 2 -name plugin.json | wc -l | tr -d ' ')
wasm=$(find "$res/plugins" -name '*.wasm' | wc -l | tr -d ' ')
ok "$plugins plugin manifests, $wasm wasm modules"
if strings -a "$bin" | grep -qF "$HOME"; then
  bad "the binary contains paths under \$HOME: $(strings -a "$bin" | grep -F "$HOME" | head -3 | tr '\n' ' ')"
else
  ok "the binary contains no path under \$HOME (checkout, cargo registry)"
fi
if grep -rIlF -e "$repo" -e "$HOME/" "$res" >/dev/null 2>&1; then
  bad "bundled files mention this machine: $(grep -rIlF -e "$repo" -e "$HOME/" "$res" | head -3 | tr '\n' ' ')"
else
  ok "no bundled text file mentions the checkout or \$HOME"
fi

section "Sandboxed smoke runs (checkout denied)"
before=$(cd "$repo" && git status --porcelain 2>/dev/null || true)
scratch="$(mktemp -d "${TMPDIR:-/tmp}/breadbox-check.XXXXXX")"
scratch="$(cd "$scratch" && pwd -P)"
[ "${KEEP:-0}" = 1 ] || trap 'rm -rf "$scratch"' EXIT
cp -R "$app" "$scratch/Breadbox.app"
copy="$scratch/Breadbox.app"
mkdir -p "$scratch/support" "$scratch/caches" "$scratch/cwd"
profile="$scratch/deny-repo.sb"
cat > "$profile" <<SB
(version 1)
(allow default)
(deny file-read* file-write* (subpath "$repo"))
(deny file-write* (subpath "$copy"))
SB
smoke() { # smoke <label> <timeout s> <args…>
  local label="$1" secs="$2"; shift 2
  local log="$scratch/$(echo "$label" | tr -c 'a-zA-Z0-9\n' '-').log"
  local status=0
  ( cd "$scratch/cwd" && env -u BREADBOX_RESOURCES -u BREADBOX_EXAMPLES -u BREADBOX_LESSONS \
      -u BREADBOX_BUNDLED_PLUGINS -u BREADBOX_PLUGIN_DIR -u BREADBOX_PROGRESS \
      -u BREADBOX_TOOLCHAIN_DIR -u BREADBOX_BUILD_CACHE -u CARGO_MANIFEST_DIR \
      BREADBOX_APP_SUPPORT="$scratch/support" BREADBOX_CACHES="$scratch/caches" \
      RUST_LOG="info" \
      perl -e 'alarm shift; exec @ARGV or die "exec: $!"' "$secs" \
      sandbox-exec -f "$profile" "$copy/Contents/MacOS/breadbox" --smoke "$@" ) \
      > "$log" 2>&1 || status=$?
  local line
  line=$(grep '^SMOKE ' "$log" | tail -1 || true)
  if [ $status -ne 0 ] || [[ "$line" != *"status=ok"* ]]; then
    bad "$label (exit $status): ${line:-no SMOKE line}"
    sed 's/^/        /' "$log" | tail -15
  elif grep -qF "$repo" "$log"; then
    bad "$label: output mentions the checkout: $(grep -F "$repo" "$log" | head -2)"
  elif grep -qi "operation not permitted" "$log"; then
    bad "$label: hit the sandbox: $(grep -i "operation not permitted" "$log" | head -2)"
  else
    ok "$label"
    echo "        $line"
  fi
  SMOKE_LINE="$line"
}
smoke "garden-node (bundled example path), 5 s headless" 300 \
  "$copy/Contents/Resources/examples/garden-node" --seconds 5 --headless
smoke "lesson bench: Ohm's law with an LED (DMM plugin meter), 3 s" 300 \
  lesson:ohms-law-led --seconds 3 --headless
smoke "plugin instrument: Handheld DMM placed on garden-node" 300 \
  garden-node --place community.dmm-handheld --seconds 3 --warmup 1 --headless
if [[ "${SMOKE_LINE:-}" != *"instruments=DMM"* ]]; then bad "the placed DMM reported no reading"; fi
if command -v clang++ >/dev/null 2>&1 || command -v c++ >/dev/null 2>&1; then
  smoke "ESP32 example (host-compiled against the bundled host core)" 600 \
    esp32-weather --seconds 3 --headless
  if ls -d "$scratch"/caches/builds/hostcore-* >/dev/null 2>&1; then
    ok "the host core was written to the scratch cache and compiled ($(basename "$(ls -d "$scratch"/caches/builds/hostcore-* | head -1)"))"
  else
    bad "no host core build in the scratch cache"
  fi
else
  echo "  skip  ESP32 example (no C++ compiler)"
fi
if [ $window -eq 1 ]; then
  smoke "garden-node in a real window (wgpu), 3 s" 300 garden-node --seconds 3
fi
if [ "$(cd "$repo" && git status --porcelain 2>/dev/null || true)" != "$before" ]; then
  bad "the runs changed the checkout"
else
  ok "the checkout is untouched"
fi

if [ $window -eq 1 ]; then
  section "The real app (open → first frame → quit)"
  log="$scratch/open.log"
  : > "$log"
  pkill -f "$copy/Contents/MacOS/breadbox" 2>/dev/null || true
  t0=$(now)
  open -n -F --env BREADBOX_APP_SUPPORT="$scratch/support" --env BREADBOX_CACHES="$scratch/caches" \
    --env RUST_LOG=info --stdout "$log" --stderr "$log" "$copy"
  first=""
  for _ in $(seq 1 200); do
    first=$(sed -n 's/.*first frame \([0-9]*\) ms after start.*/\1/p' "$log" | head -1)
    [ -n "$first" ] && break
    sleep 0.1
  done
  t1=$(now)
  pid=$(pgrep -f "$copy/Contents/MacOS/breadbox" | head -1 || true)
  if [ -z "$first" ]; then
    bad "the app never drew a frame (log: $(tail -3 "$log" | tr '\n' ' '))"
  else
    wall=$(python3 -c "print(round(($t1 - $t0) * 1000))")
    if [ "$first" -lt 2000 ]; then
      ok "first frame ${first} ms after process start (PLAT-4 < 2000 ms); ${wall} ms from \`open\`"
    else
      bad "first frame ${first} ms after process start (PLAT-4 wants < 2000 ms)"
    fi
    echo "first_frame_ms=$first open_to_frame_ms=$wall" > "$scratch/startup.txt"
  fi
  if grep -qF "$repo" "$log"; then bad "the app log mentions the checkout"; fi
  quit_app() {
    local pid="$1"
    [ -n "$pid" ] || return 0
    kill -TERM "$pid" 2>/dev/null || true
    for _ in $(seq 1 50); do kill -0 "$pid" 2>/dev/null || break; sleep 0.1; done
    if kill -0 "$pid" 2>/dev/null; then kill -KILL "$pid" 2>/dev/null || true; bad "the app did not quit on SIGTERM"; else ok "quit"; fi
  }
  quit_app "$pid"

  # a project folder handed over by the system (Finder "Open With", the Dock, open -a)
  project="$scratch/My Garden.breadbox"
  cp -R "$copy/Contents/Resources/examples/garden-node" "$project"
  log2="$scratch/open-document.log"
  : > "$log2"
  open -n -F --env BREADBOX_APP_SUPPORT="$scratch/support" --env BREADBOX_CACHES="$scratch/caches" \
    --env RUST_LOG=info --stdout "$log2" --stderr "$log2" -a "$copy" "$project"
  opened=""
  for _ in $(seq 1 150); do
    opened=$(grep -F "opened project" "$log2" | grep -F "$project" | head -1 || true)
    [ -n "$opened" ] && break
    sleep 0.1
  done
  if [ -n "$opened" ]; then
    ok "open -a Breadbox.app <project folder> opens it (Apple Event odoc)"
  else
    bad "the app did not open the project it was handed (log: $(tail -3 "$log2" | tr '\n' ' '))"
  fi
  quit_app "$(pgrep -f "$copy/Contents/MacOS/breadbox" | head -1 || true)"
fi

echo
if [ $failures -eq 0 ]; then
  echo "bundle check passed: $(du -sh "$app" | cut -f1) $(basename "$app")"
else
  echo "bundle check: $failures failure(s)"
  exit 1
fi
