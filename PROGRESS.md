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

### spice
Phase 1 — `bb-spice` (SPICE-1…8). Everything below is tested; the reference suite compares
against ngspice-47 golden data (checked in; `scripts/regen-spice-golden.sh` regenerates it).

- `SPICE-1` **done** — parser: title, `*`/`;`/`$` comments, `+` continuation, case-insensitive,
  SI suffixes incl. `mil`/`µ`/`4k7`, `{expr}`/`'expr'` (also `{p}` inside B expressions),
  `.param` (multi, unbraced, forward refs), `.func`, `.model`, `.subckt` (params, nesting,
  local models), `.include`, `.lib file section` / `.lib`…`.endl`, `.global`, `.temp`,
  `.options`, `.ic`, `.nodeset`, `.end`; `.control` skipped. Errors carry file:line:col and
  suggestions ("did you mean `led_amber`?"). Unknown subckts (`CAPSOIL`, `ATMEGA328P`) fail
  cleanly at elaboration; `Netlist::add_library` registers part libraries; non-numeric X params
  (`fw=garden-node.hex`) are kept for bb-sim. The full Pro-screen netlist parses.
- `SPICE-2` **done** — R (tc1/tc2/temp/dtemp/m, behavioural R), C/L (ic, tc), K, V/I (DC, AC,
  PULSE, SIN, EXP, PWL with r=/td=, SFFM, AM), E/F/G/H (linear, POLY(n), VALUE=, TABLE), B
  (full function list, exact AD derivatives, breakpoints for `time` comparisons), S/W with
  hysteresis (step lands on the threshold), diode (IS N RS CJO VJ M TT FC BV IBV NBV EG XTI),
  Gummel-Poon BJT (all params incl. IRB/RBM, XTF/VTF/ITF, PTF excess phase, CJS, XCJC, XTB),
  MOS1 (body effect, λ, Meyer caps, junction caps, NSUB/TOX-derived params; other LEVELs →
  level 1 + warning), JFET (incl. B), lossless T (Z0/TD or F/NL), X, co-sim ports. Temperature
  scaling follows ngspice.
- `SPICE-3` **done** — .op; .dc (source/element/.param/temp, lin/dec/oct/list, 2-level);
  .tran (LTE on all charges, breakpoints, trap/Gear-2/BE, UIC, tmax, tstart); .ac; .step
  (nested, param/source/element/temp); .mc (`tol=`, per-element `tol=`, gauss/uniform, seed,
  `analysis=`, agauss/gauss/aunif/unif/mc() in params) → `McResults::distribution`/histogram;
  multi-.temp; .meas (avg rms pp min max min_at max_at integ deriv find…at/when, when,
  trig/targ, param); .tf; .noise (thermal, shot; spectra + integrated).
- `SPICE-4` **done** — pnjlim/fetlim/limvds, dynamic gmin stepping, source stepping,
  timestep cut ÷8; options reltol abstol vntol chgtol trtol gmin itl1 itl2 itl4 method
  gminsteps srcsteps noopiter tnom temp. Failures list the worst unknowns (`V(out)`, `I(V1)`)
  and non-converging elements; singular matrices name the floating node.
- `SPICE-5` **done** — hand-written Markowitz sparse LU (threshold pivoting, symbolic fill
  reuse, Sparse-1.3-style refactorisation) + dense LU ≤ 12 unknowns; transpose solve for noise.
  1 024-node resistor/diode mesh `.op`: **13.5 ms** (release, M-series).
- `SPICE-6` **done** — 47 reference circuits (every element/model/analysis incl. the
  garden-node netlist minus the MCU, .step, nested .dc, temp sweep, noise, T-line, 555-style
  oscillator, Schmitt trigger, latch, op-amp subckt) all within the SPEC tolerances; worst DC/op
  case uses 1 % of the allowed error.
- `SPICE-7` **done** — `Session`: ports (`PinDrive`), `ext()`, `set_param`, Schmitt watches
  with interpolated crossing times, `advance_until_crossing`, `op()` re-solve at
  discontinuities, snapshot/restore, add_port mid-run. Speculative stepping + interpolation
  makes 1 µs quanta cheap: 490 Hz PWM into RC + LED for 1 s = **38 ms (26× real time)**; static
  circuits cost nothing per advance; settled circuits stop stepping.
- `SPICE-8` **done** — `Netlist::to_spice_text()` / `Session::to_spice_text()` (ngspice runs
  the output; ports → V+R/I, ext() → .param), CSV (mag/phase, dB/phase, re/im), ASCII rawfile
  write + read.
- Tests: 47 unit (parser, expressions incl. AD gradients, LU, waveforms, every model's DC
  Jacobian vs finite differences and AC vs transient charge Jacobian, temperature formulas),
  21 analysis (convergence aids + diagnostics, Schmitt/latch, MC statistics with fixed seed,
  .step/.dc/.meas, export round trip, design netlist), 48 reference, 16 session, 7 doctests;
  2 perf (`cargo test -p bb-spice --release --test perf -- --ignored --nocapture`). clippy
  `-D warnings` + fmt clean.

Notes / deviations:
- Golden data is generated with ngspice `.options reltol=1e-5` (injected by the script unless
  the netlist sets reltol): at ngspice's default 0.1 % its own DC sweeps stop up to 0.1 % off,
  i.e. at our tolerance. Transient NRMS is time-weighted (√(∫e²dt/T)/range); ngspice clusters
  points on fast edges, which would otherwise dominate. Circuits with ideal switches use a fine
  `.tran` step so ngspice's own switching instants are accurate (bb-spice lands on them).
- `pwr(x,y)` is `|x|^y` everywhere (ngspice B sources use sgn(x)·|x|^y; `pwrs()` provided).
  ngspice-47 silently treats `AM(...)` as DC, so AM is unit-tested against the manual's formula.
- Kept SPICE3 Jacobian approximations for ngspice parity (bias-dependent base resistance,
  MOS1 forward-bulk gmbs). Flicker noise (KF/AF), diode ISR/IKF/sidewall, BJT substrate diode
  current are not modelled (unknown params warn). `.save` is accepted but all vectors are kept.
- `Session::current()` of charge-storing devices (C, D, Q, M, J) is the latest accepted point's
  value, not interpolated (branch, resistor and port currents are interpolated).
- Debug helpers: `cargo run -p bb-spice --example spice -- file.cir`, `--example refdiff --
  <ref> <vector> <level>`, `--example dcdiff -- <ref>`, `--example export_cir -- file.cir`.
