#!/usr/bin/env bash
# Builds the first-party plugins (plugins/*, plugins/examples/*) to WebAssembly and copies each
# module next to its plugin.json under the name its `provides.model` gives. The built .wasm
# files are committed so the app and the tests don't need a wasm toolchain.
#
#   scripts/build-plugins.sh            # build everything
#   scripts/build-plugins.sh dmm-handheld scope-4ch
#   scripts/build-plugins.sh --check    # rebuild into a scratch dir and verify the committed
#                                       # .wasm files are byte-identical (scripts/test-all.sh)
#
# Paths are remapped (the checkout → `/breadbox`, `$HOME` → `~`) so the modules carry no
# machine- or checkout-specific paths and rebuild identically from any clone or worktree with
# the same toolchain.
set -euo pipefail
cd "$(dirname "$0")/../plugins"
export PATH="$HOME/.cargo/bin:$PATH"
rustup target list --installed 2>/dev/null | grep -q wasm32-unknown-unknown \
  || rustup target add wasm32-unknown-unknown
repo_root="$(cd .. && pwd)"
# rustc applies the last matching mapping: the checkout wins over $HOME (path dependencies
# such as crates/bb-plugin-sdk are passed to rustc as absolute paths)
export RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$HOME=~ --remap-path-prefix=$repo_root=/breadbox"

check=0
dirs=()
for a in "$@"; do
  case "$a" in
    --check) check=1 ;;
    *) dirs+=("$a") ;;
  esac
done
if [ ${#dirs[@]} -eq 0 ]; then
  while IFS= read -r m; do dirs+=("$(dirname "$m")"); done < <(find . -name Cargo.toml -mindepth 2 -not -path "*/target/*" | sort)
fi

target_dir="target"
if [ $check -eq 1 ]; then
  target_dir="$(mktemp -d "${TMPDIR:-/tmp}/bb-plugins-check.XXXXXX")"
  trap 'rm -rf "$target_dir"' EXIT
fi

pkgs=()
for d in "${dirs[@]}"; do
  pkgs+=("-p" "$(sed -n 's/^name = "\(.*\)"/\1/p' "$d/Cargo.toml" | head -1)")
done
CARGO_TARGET_DIR="$target_dir" cargo build --release --target wasm32-unknown-unknown "${pkgs[@]}"

stale=0
for d in "${dirs[@]}"; do
  crate=$(sed -n 's/^name = "\(.*\)"/\1/p' "$d/Cargo.toml" | head -1)
  model=$(python3 -c "import json,sys; m=json.load(open(sys.argv[1])); print(m.get('provides',{}).get('model') or m.get('entry') or '')" "$d/plugin.json")
  if [ -z "$model" ]; then
    echo "skip $d (no provides.model)"; continue
  fi
  out="$d/${model#./}"
  built="$target_dir/wasm32-unknown-unknown/release/$crate.wasm"
  [ -f "$built" ] || built="$target_dir/wasm32-unknown-unknown/release/${crate//-/_}.wasm"
  if [ $check -eq 1 ]; then
    if cmp -s "$built" "$out"; then
      printf 'up to date  %s\n' "$out"
    else
      printf 'STALE       %s (rebuild with scripts/build-plugins.sh and commit)\n' "$out"
      stale=1
    fi
  else
    cp "$built" "$out"
    printf '%-34s %8d bytes\n' "$out" "$(wc -c < "$out")"
  fi
done
exit $stale
