# Contributing to Breadbox

Thanks for wanting to help! New parts, plugins, lessons, example projects, bug reports and fixes
are all welcome.

## Getting set up

1. Install Rust with [rustup](https://rustup.rs). `rust-toolchain.toml` picks stable and adds the
   `wasm32-unknown-unknown` target. If Homebrew also installed an older `rustc`, put
   `~/.cargo/bin` first on your `PATH`.
2. Install the Xcode Command Line Tools (`xcode-select --install`).
3. Optional, for the full test suite: `brew install arduino-cli ngspice`. Tests that need them
   skip with a message when they're missing.
4. Run the app: `cargo run --release -p breadbox`.

## Before you open a pull request

```sh
scripts/test-all.sh          # fmt, clippy -D warnings, all tests, plugin + perf + bundle checks
scripts/test-all.sh --quick  # the same without the perf gates and the bundle stage
```

- Keep `cargo fmt` and `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Every change lands with a test. Prefer real behaviour over mocks: real compiled firmware,
  ngspice reference data, the GUI driven through `egui_kittest` by accessible labels.
- No panics on user input (netlists, sketches, project files, plugin output). Libraries use
  `thiserror`, the app `anyhow`.
- UI text follows [docs/UI-TEXT.md](docs/UI-TEXT.md): short labels, explanations in tooltips or
  the per-screen help pages in `docs/guide/`. Visual tokens are in [docs/DESIGN.md](docs/DESIGN.md).
- If you change a contract between crates, update [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)
  in the same pull request.

## Where things live

| you want to… | look at |
|---|---|
| add a built-in part | `crates/bb-circuit/parts/*.json` (+ a behavioural device in `crates/bb-sim/src/devices/` if it needs one, + bench art in `crates/breadbox/src/canvas/art/`) |
| add a plugin | [docs/PLUGIN-SDK.md](docs/PLUGIN-SDK.md), `plugins/examples/template`; build with `scripts/build-plugins.sh` |
| add a lesson | [docs/LESSON-FORMAT.md](docs/LESSON-FORMAT.md), `lessons/<track>/` — or use the in-app lesson editor |
| add an example project | `examples/<name>/project.json` (+ firmware under `firmware/`, prebuilt by `scripts/build-test-firmware.sh`) |
| fix the SPICE engine | `crates/bb-spice` — add a reference circuit and regenerate golden data with `scripts/regen-spice-golden.sh` |
| fix the AVR emulator | `crates/bb-avr` — instruction tests plus a real-firmware test sketch |

The full requirement list is [SPEC.md](SPEC.md); [docs/SPEC-AUDIT.md](docs/SPEC-AUDIT.md) maps
each requirement to its code and tests.

## Pull requests

- Keep them focused, one topic each, with a short description of the why.
- Include screenshots for UI changes. GUI tests can refresh the committed ones:
  `BREADBOX_UPDATE_SCREENSHOTS=1 cargo test -p breadbox`.
- Committed `.wasm` plugins and prebuilt firmware must be rebuilt with the scripts above, so
  they stay reproducible.

## Working with AI coding agents

`CLAUDE.md` / `AGENTS.md` hold the rules agents follow in this repo (toolchain, shared-checkout
etiquette, tests). They're a good summary for humans too.

## Code of conduct

This project follows the [Contributor Covenant](CODE_OF_CONDUCT.md). Be kind.
