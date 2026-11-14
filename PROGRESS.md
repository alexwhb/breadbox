# Progress

Build plan: at most two agents work at once, each owning disjoint crates. The lead (orchestrator)
reviews and integrates between phases. Agents update **only their own section** below.

## Phases

| phase | agent A | agent B |
|---|---|---|
| 1 | `spice` — bb-spice (SPICE-*) | `mcu` — bb-avr + bb-firmware AVR path (MCU-1..5, 7) ✅ |
| 2 | `sim` — bb-core device contract, bb-circuit, bb-world, bb-sim, parts library (PART-*, SIM-*, WORLD-*) | `radio` — bb-radio (RF-1..6, 8), example firmware sketches, ESP32/Pico host-emulated boards (MCU-6), native-toolchain installer |
| 3 | `ui-bench` — app shell, theme, Home, Workbench (APP-*, WB-*) | `plugins` — bb-plugin, bb-plugin-sdk, first-party plugins (PLUG-*), bb-lessons engine + content (LES-1,3,4), radio/host-board ↔ sim glue |
| 4 | `ui-pro` — Firmware screen + debugger UI, Pro mode, PCB (PRO-*, MCU-5 UI) | `ui-plug-les` — Plugins screen, Lessons screen + lesson editor (LES-2,5,6), radio panel (RF-7), instruments on bench |
| 5 | `e2e` — headless + GUI e2e suites, perf, fixes (TEST-*) | `qa` — independent spec audit + fixes, bundle-macos, docs |

## Ownership (current phase)

- bb-core: lead (frozen except additive changes; the `sim` agent owns it in phase 2)

## Requests (cross-crate asks)

- (lead) Toolchain installer must work on Apple Silicon without Rosetta (avr-gcc 7.3.0 + ctags are x86_64-only) → assigned to `radio` in phase 2.

## Status

### lead
- Phase 0 done: workspace skeleton, SPEC/ARCHITECTURE/DESIGN docs, bb-core time + SI units.

### mcu
Phase 1 — `bb-avr` + `bb-firmware` (AVR path). All items below are tested against real
arduino-cli-compiled firmware (26 sketches, fixtures checked in; `scripts/build-test-firmware.sh`
rebuilds them).

- `MCU-1` **done** — ATmega328P + ATmega2560 core (3-byte PC, EIND/EIJMP/EICALL, RAMPZ/ELPM),
  every opcode with datasheet cycles/flags, pre-decoded flash. 36 instruction tests (carry/borrow
  chains, Z propagation, MUL/MULS/MULSU/FMUL*, LPM/ELPM/SPM incl. page erase/write + signature,
  skips over 2-word opcodes, SLEEP/WDR/BREAK, SEI/RETI one-instruction rule, 4/5-cycle
  interrupt response, faults).
- `MCU-2` **done** — GPIO (PIN toggle, PUD, DIDR), INT0–7 (level/any/falling/rising, async edge
  rules in sleep), PCINT, Timer0–5 (all WGM modes incl. PFC, double-buffered OCR, OC pins, FOC,
  input capture + noise canceler + ACIC, external clock, TSM/PSRSYNC, PRR), USART0–3 (5–9 bits,
  parity, 1–2 stop, U2X, MPCM, FIFO + FE/DOR/UPE, baud-clock-grid TX, pin-level RX), ADC
  (prescaler-accurate 13/25/13.5-clock timing, AVcc/AREF/1.1/2.56 V, auto-trigger, free running,
  temperature sensor, differential + MUX5 on 2560, noise-reduction sleep), SPI master/slave (all
  modes, WCOL, mode fault), TWI master/slave bit-level (clock stretching, arbitration, general
  call, TWAMR), EEPROM timing + EE_READY, watchdog (interrupt / reset / both), sleep modes with
  clock gating + 16K CK wake-up, analog comparator, BOD tied to `set_vcc`, reset pin, CLKPR,
  SPM. Arduino bootloader hand-off emulated (MCUSR→r2, WDT off) — configurable.
- `MCU-3` **done** — busy sketch (float math, PWM, ADC, serial) runs at **24× real time**
  (~280 MIPS, release, M3 Max), unchanged with a stop on every pin change.
  `cargo test -p bb-avr --release --test perf -- --ignored --nocapture`.
- `MCU-4` **done** — `Toolchain::detect/install/install_cli/install_core`, library
  search/install/list (ArduinoJson 7 compiled + run), `compile()` with build cache, diagnostics
  mapped to tabs (`broken.ino:7:3`, second `.ino` tab), size report identical to arduino-cli's
  text (+ design variant with thousands separators), `-Og -fno-lto` debug builds.
- `MCU-5` **done** (backend) — `DebugInfo` (line table both ways, functions, scoped
  params/locals incl. location lists, globals incl. EEMEM, types, symbols, CFI) and `Debugger`
  (breakpoints by file:line with Always/Equals/AtLeast/Every hit conditions, step
  over/into/out/instruction, pause, call stack via shadow stack + CFI, locals in outer frames)
  working as a policy layer under an outer scheduler. Register/SRAM/flash/EEPROM/I/O views are
  `Avr` accessors. UI is phase 4.
- `MCU-7` **done** (engine side) — independent instances: 4 MCUs interleaved in lockstep and 4
  parallel threads match solo runs bit-for-bit; two AVRs talk I²C (Wire master ↔ Wire slave).
- Tests: bb-avr 76 (+1 perf, ignored), bb-firmware 20 (+1 network install, ignored);
  clippy `-D warnings` + fmt clean.

Deviations / notes:
- **Toolchain on this Mac**: no Rosetta, so Arduino's x86_64 avr-gcc 7.3.0 and ctags cannot run.
  Fixtures are built with native **avr-gcc 8.5.0** (`brew install osx-cross/avr/avr-gcc@8`) and
  a natively built Arduino ctags (the script builds it into
  `~/Library/Application Support/Breadbox/toolchain/native/`). `Toolchain` detects this and
  passes `compiler.path` / `runtime.tools.ctags.path` overrides; with Rosetta the bundled 7.3.0
  is used unchanged.
- bb-core: fixed two clippy lints (`si.rs`, `time.rs`; no behaviour change) so
  `cargo clippy -p bb-avr -p bb-firmware -- -D warnings` can pass.
- `Cargo.lock` has uncommitted entries from both phase-1 agents — left for the lead to commit.

Known gaps (none block the spec tests):
- USART synchronous mode and master-SPI mode (MSPIM) not implemented.
- Pin synchronizer latency (½–1½ cycles on PINx, ~2–3 cycles to INT/PCINT flags) not modelled;
  interrupts are taken at the first instruction boundary at/after the flag.
- Timer2 asynchronous (32.768 kHz TOSC) mode uses a rounded tick period; 2560 OC0A/OC1C
  modulator on PB7 not modelled (last writer wins); PRR only gates timers; BODS ignored; no
  XMEM; TWI clock is not gated in sleep.
- Debugger: variables inside inlined functions are not listed; optimised (-Os -flto) release
  builds show few locals — use `CompileOptions::debug(true)` for debugging sessions.
- **Open issue for the lead**: the one-click installer fetches arduino-cli + the AVR core, but
  on Rosetta-less Apple Silicon compiling also needs native avr-gcc + ctags. Options: prompt to
  install Rosetta, or ship/download prebuilt arm64 avr-gcc + ctags into `toolchain/native/`.
