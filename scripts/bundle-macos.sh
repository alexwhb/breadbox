#!/usr/bin/env bash
# Builds dist/Breadbox.app (PLAT-3) — a self-contained, ad-hoc signed macOS app.
#
#   scripts/bundle-macos.sh            # release build → dist/Breadbox.app
#   scripts/bundle-macos.sh --dmg      # … plus dist/Breadbox.dmg (drag-to-Applications image)
#   scripts/bundle-macos.sh --check    # … then scripts/check-bundle.sh (sandboxed smoke runs
#                                      #     that prove the bundle never reads the checkout)
#
# The binary is built with BREADBOX_BUNDLE=1 (no source-checkout fallback compiled in) and with
# remapped paths, so nothing in the bundle points at this machine or checkout. It finds its
# data in Contents/Resources (bb_core::paths); user data stays in
# ~/Library/Application Support/Breadbox and ~/Library/Caches/Breadbox.
#
# Contents/Resources:
#   Breadbox.icns        app icon (assets/icon/breadbox-icon.svg, rendered by the app_icon example)
#   examples/            example projects + prebuilt firmware fixtures (opened read-only)
#   lessons/             lesson tracks: TOML, start benches, prebuilt firmware, sketches
#   plugins/             first-party plugins (manifests, .wasm, art, sources) + SDK examples
#   crates/bb-plugin-sdk the guest SDK the plugin sources build against
#   docs/                README, guide/ (per-screen help), PLUGIN-SDK, LESSON-FORMAT, plugin.schema.json
#   fonts/               Inter, JetBrains Mono, Space Grotesk + their OFL licences
#   hostcore/            the Arduino host core for ESP32 / Pico builds (the app compiles its
#                        embedded copy; this is the readable reference)
#   licenses/            third-party notices (Rust crates, fonts)
#   build-info.json      version, commit, build date
#
# Info.plist declares `.breadbox` project folders as a document package (UTI
# app.breadbox.project, conforming to com.apple.package; Breadbox is its owner and editor), so
# Finder shows one document and a double-click opens it here; `.breadbox.zip` bundles import.
set -euo pipefail
cd "$(dirname "$0")/.."
repo="$PWD"
export PATH="$HOME/.cargo/bin:$PATH"

dmg=0
check=0
for a in "$@"; do
  case "$a" in
    --dmg) dmg=1 ;;
    --check) check=1 ;;
    -h|--help) sed -n '2,8p' "$0"; exit 0 ;;
    *) echo "unknown option $a" >&2; exit 2 ;;
  esac
done

say() { printf '\033[1m==> %s\033[0m\n' "$*"; }

# Bundle builds get their own target dir: their flags differ from everyday release builds.
target_dir="${BREADBOX_BUNDLE_TARGET_DIR:-${CARGO_TARGET_DIR:-$repo/target}/bundle-macos}"
dist="$repo/dist"
app="$dist/Breadbox.app"
version=$(cargo metadata --no-deps --format-version 1 \
  | python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"]=="breadbox"))')
commit=$(git rev-parse --short HEAD 2>/dev/null || echo unknown)
# CFBundleVersion = build number (commits on this branch): newer builds compare higher, so
# LaunchServices takes their document-type declarations over an older copy's
build=$(git rev-list --count HEAD 2>/dev/null || echo 1)
git diff --quiet HEAD 2>/dev/null || commit="$commit-dirty"
bundle_id="app.breadbox.Breadbox"

say "Building Breadbox $version ($commit) for the bundle"
export BREADBOX_BUNDLE=1
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$HOME=~ --remap-path-prefix=$repo=/breadbox"
CARGO_TARGET_DIR="$target_dir" cargo build --release --locked -p breadbox --bin breadbox --example app_icon
bin="$target_dir/release/breadbox"

work="$(mktemp -d "${TMPDIR:-/tmp}/breadbox-bundle.XXXXXX")"
trap 'rm -rf "$work"' EXIT
stage="$work/Breadbox.app"
contents="$stage/Contents"
res="$contents/Resources"
mkdir -p "$contents/MacOS" "$res"

say "Assembling Breadbox.app"
cp "$bin" "$contents/MacOS/breadbox"
strip -x "$contents/MacOS/breadbox"
printf 'APPL????' > "$contents/PkgInfo"

# app icon: every iconset size from the SVG
"$target_dir/release/examples/app_icon" "$work/Breadbox.iconset" >/dev/null
iconutil -c icns "$work/Breadbox.iconset" -o "$res/Breadbox.icns"

copy() { # copy <src dir> <dest dir> [rsync excludes…]
  local src="$1" dest="$2"; shift 2
  mkdir -p "$dest"
  rsync -a --exclude '.DS_Store' --exclude 'target/' --exclude '.breadbox-cache/' "$@" "$src/" "$dest/"
}
copy examples "$res/examples"
copy lessons "$res/lessons" --exclude '*.sh'
copy plugins "$res/plugins"
copy crates/bb-plugin-sdk "$res/crates/bb-plugin-sdk"
copy crates/bb-firmware/hostcore "$res/hostcore"
copy assets/fonts "$res/fonts"
mkdir -p "$res/docs"
cp README.md docs/PLUGIN-SDK.md docs/LESSON-FORMAT.md docs/plugin.schema.json "$res/docs/"
mkdir -p "$res/docs/guide"
cp docs/guide/*.md "$res/docs/guide/"

# third-party notices: every crate linked into the app (normal dependencies, this platform)
mkdir -p "$res/licenses"
cp assets/fonts/*-OFL.txt "$res/licenses/"
cargo metadata --locked --format-version 1 --filter-platform "$(rustc -vV | sed -n 's/^host: //p')" \
  | python3 -c '
import json, sys
m = json.load(sys.stdin)
pk = {p["id"]: p for p in m["packages"]}
nodes = {n["id"]: n for n in m["resolve"]["nodes"]}
root = next(p["id"] for p in m["packages"] if p["name"] == "breadbox")
seen, todo = set(), [root]
while todo:
    i = todo.pop()
    if i in seen:
        continue
    seen.add(i)
    for d in nodes[i]["deps"]:
        if any(k["kind"] is None for k in d["dep_kinds"]):
            todo.append(d["pkg"])
rows = sorted((pk[i]["name"], pk[i]["version"], pk[i].get("license") or "see crate", pk[i].get("repository") or "")
              for i in seen if pk[i]["source"])
print("# Third-party software in Breadbox\n")
print("Breadbox links the Rust crates below (license expressions as published on crates.io).")
print("Fonts: Inter, JetBrains Mono and Space Grotesk under the SIL Open Font License 1.1")
print("(`*-OFL.txt` next to this file). arduino-cli and the Arduino AVR core are downloaded")
print("on first use and keep their own licenses.\n")
print("| crate | version | license | repository |\n|---|---|---|---|")
for r in rows:
    print("| %s | %s | %s | %s |" % r)
' > "$res/licenses/THIRD-PARTY.md"

cat > "$res/build-info.json" <<JSON
{ "version": "$version", "build": "$build", "commit": "$commit", "built": "$(date -u +%Y-%m-%dT%H:%M:%SZ)" }
JSON

cat > "$contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Breadbox</string>
  <key>CFBundleDisplayName</key><string>Breadbox</string>
  <key>CFBundleIdentifier</key><string>$bundle_id</string>
  <key>CFBundleExecutable</key><string>breadbox</string>
  <key>CFBundleIconFile</key><string>Breadbox</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleSignature</key><string>????</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$build</string>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>LSMinimumSystemVersion</key><string>14.0</string>
  <key>LSArchitecturePriority</key><array><string>arm64</string></array>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
  <key>NSRequiresAquaSystemAppearance</key><false/>
  <key>NSHumanReadableCopyright</key><string>Breadbox $version — a circuit playground and SPICE workbench.</string>
  <key>UTExportedTypeDeclarations</key>
  <array>
    <dict>
      <!-- a .breadbox project folder is a document package: Finder shows one document and a
           double-click opens it in Breadbox -->
      <key>UTTypeIdentifier</key><string>app.breadbox.project</string>
      <key>UTTypeDescription</key><string>Breadbox Project</string>
      <key>UTTypeConformsTo</key><array><string>com.apple.package</string><string>public.composite-content</string></array>
      <key>UTTypeIconFile</key><string>Breadbox</string>
      <key>UTTypeTagSpecification</key>
      <dict><key>public.filename-extension</key><array><string>breadbox</string></array></dict>
    </dict>
    <dict>
      <key>UTTypeIdentifier</key><string>app.breadbox.bundle</string>
      <key>UTTypeDescription</key><string>Breadbox Project Bundle</string>
      <key>UTTypeConformsTo</key><array><string>public.zip-archive</string></array>
      <key>UTTypeIconFile</key><string>Breadbox</string>
      <key>UTTypeTagSpecification</key>
      <dict><key>public.filename-extension</key><array><string>breadbox.zip</string></array></dict>
    </dict>
  </array>
  <key>CFBundleDocumentTypes</key>
  <array>
    <dict>
      <key>CFBundleTypeName</key><string>Breadbox Project</string>
      <key>CFBundleTypeRole</key><string>Editor</string>
      <key>LSHandlerRank</key><string>Owner</string>
      <key>LSTypeIsPackage</key><true/>
      <key>LSItemContentTypes</key><array><string>app.breadbox.project</string></array>
    </dict>
    <dict>
      <key>CFBundleTypeName</key><string>Breadbox Project Bundle</string>
      <key>CFBundleTypeRole</key><string>Editor</string>
      <key>LSHandlerRank</key><string>Owner</string>
      <key>LSItemContentTypes</key><array><string>app.breadbox.bundle</string></array>
    </dict>
    <dict>
      <!-- any folder holding a project.json (drop it on the Dock icon, Open With) -->
      <key>CFBundleTypeName</key><string>Folder</string>
      <key>CFBundleTypeRole</key><string>Viewer</string>
      <key>LSHandlerRank</key><string>Alternate</string>
      <key>LSItemContentTypes</key><array><string>public.folder</string></array>
    </dict>
  </array>
</dict>
</plist>
PLIST
plutil -lint "$contents/Info.plist" >/dev/null

say "Signing (ad hoc)"
codesign --force --deep --sign - --identifier "$bundle_id" "$stage"
codesign --verify --deep --strict "$stage"

mkdir -p "$dist"
rm -rf "$app"
mv "$stage" "$app"
size=$(du -sh "$app" | cut -f1)
say "dist/Breadbox.app ($size)"

if [ $dmg -eq 1 ]; then
  say "Building dist/Breadbox.dmg"
  img="$work/dmg"
  mkdir -p "$img"
  cp -R "$app" "$img/"
  ln -s /Applications "$img/Applications"
  rm -f "$dist/Breadbox.dmg"
  hdiutil create -quiet -volname "Breadbox $version" -srcfolder "$img" -ov -format UDZO "$dist/Breadbox.dmg"
  say "dist/Breadbox.dmg ($(du -h "$dist/Breadbox.dmg" | cut -f1))"
fi

if [ $check -eq 1 ]; then
  "$repo/scripts/check-bundle.sh" "$app"
fi
