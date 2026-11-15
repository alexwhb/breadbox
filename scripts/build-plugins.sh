#!/usr/bin/env bash
# Builds the first-party plugins (plugins/*, plugins/examples/*) to WebAssembly and copies each
# module next to its plugin.json under the name its `provides.model` gives. The built .wasm
# files are committed so the app and the tests don't need a wasm toolchain.
#
#   scripts/build-plugins.sh            # build everything
#   scripts/build-plugins.sh dmm-handheld scope-4ch
set -euo pipefail
cd "$(dirname "$0")/../plugins"
export PATH="$HOME/.cargo/bin:$PATH"
rustup target list --installed 2>/dev/null | grep -q wasm32-unknown-unknown \
  || rustup target add wasm32-unknown-unknown

dirs=()
if [ $# -gt 0 ]; then
  for d in "$@"; do dirs+=("$d"); done
else
  while IFS= read -r m; do dirs+=("$(dirname "$m")"); done < <(find . -name Cargo.toml -mindepth 2 -not -path "*/target/*" | sort)
fi

pkgs=()
for d in "${dirs[@]}"; do
  pkgs+=("-p" "$(sed -n 's/^name = "\(.*\)"/\1/p' "$d/Cargo.toml" | head -1)")
done
cargo build --release --target wasm32-unknown-unknown "${pkgs[@]}"

for d in "${dirs[@]}"; do
  crate=$(sed -n 's/^name = "\(.*\)"/\1/p' "$d/Cargo.toml" | head -1)
  model=$(python3 -c "import json,sys; m=json.load(open(sys.argv[1])); print(m.get('provides',{}).get('model') or m.get('entry') or '')" "$d/plugin.json")
  if [ -z "$model" ]; then
    echo "skip $d (no provides.model)"; continue
  fi
  out="$d/${model#./}"
  cp "target/wasm32-unknown-unknown/release/$crate.wasm" "$out"
  printf '%-34s %8d bytes\n' "$out" "$(wc -c < "$out")"
done
