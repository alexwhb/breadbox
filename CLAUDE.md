# Breadbox

Circuit playground + SPICE workbench for macOS, written in Rust. Fun mode (breadboards, Arduinos
running real firmware, sensors in a pot of soil, radios) and Pro mode (SPICE analyses, PCB) over
one shared circuit model, extensible through sandboxed WebAssembly plugins.

**Read before working:**

| | |
|---|---|
| [`SPEC.md`](SPEC.md) | the definition of done — every requirement has an ID |
| [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) | crate map, contracts between crates, scheduler, file formats |
| [`docs/DESIGN.md`](docs/DESIGN.md) | visual tokens; screenshots in `docs/design/screenshots/` |
| [`PROGRESS.md`](PROGRESS.md) | who owns what, status per phase, open issues |

## Rules

- **Toolchain**: put `~/.cargo/bin` first: `export PATH="$HOME/.cargo/bin:$PATH"` (Homebrew's
  older rustc is also installed and must not be used).
- **Parallel agents share this checkout.** Use your own target dir
  (`export CARGO_TARGET_DIR=target/<your-agent-name>`), build/test with `-p <crate>` for the
  crates you own, and never edit files owned by another active agent (see `PROGRESS.md`). If you
  need a change in someone else's crate, write it under "Requests" in `PROGRESS.md`.
- **Commits**: commit only your own paths (`git add <paths> && git commit -m "..." -- <paths>`);
  retry if `.git/index.lock` is busy. Small, frequent commits with clear messages.
- Keep `docs/ARCHITECTURE.md` in sync when you change a cross-crate contract.
- No panics on user input; libraries use `thiserror`, the app `anyhow`.
- Every feature lands with tests. Real behaviour beats mocks: run real compiled firmware,
  compare against ngspice golden data, drive the GUI with `egui_kittest`.
- **No sparkle / magic-wand / four-point-star icons, ever** (no ✨ ✦ ✧). Pick an icon that says
  what the feature does.
- `scripts/test-all.sh` must stay green.
