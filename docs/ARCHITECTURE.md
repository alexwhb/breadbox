# Breadbox — Architecture

Companion to `SPEC.md` (what) — this file is *how*. Contract sketches below are binding in
spirit: an owner may refine a signature, but must keep this document updated in the same change
and must not break other crates' use of it.

## 1. Workspace map

```
crates/
  bb-core        shared types: SimTime, SI units/formatting, ids, PinDrive, Device trait,
                 WorldView, events. Tiny, dependency-light. Everyone depends on it.
  bb-spice       analog engine (parser, MNA, models, analyses, incremental transient session)
  bb-avr         AVR emulator (ATmega328P, ATmega2560): core, peripherals, loaders, debugger hooks
  bb-firmware    arduino-cli management + compile, ELF/DWARF (lines, symbols, types),
                 host-emulated ESP32/Pico runner (hostcore/ C++ sources + pipe protocol)
  bb-circuit     document model: project, parts library (declarative defs), placement in
                 breadboard + schematic + PCB views, wires, connectivity → nets, netlister
                 (to bb-spice), ERC/DRC, (de)serialisation + migrations, undo/redo command log
  bb-world       world state + models (soil, light/sun, thermal, objects), keyframes/expressions
  bb-radio       virtual RF space: sockets/IP model, AP, DHCP/DNS/NTP, MQTT broker, HTTP,
                 ESP-AT modem, HM-10/HC-05, BLE GATT + virtual phone, LoRa model,
                 fault injection, LAN bridge, packet log. Pure state machines over bytes + time.
  bb-sim         co-simulation engine: builds a runnable bench from a bb-circuit project;
                 owns the scheduler/time; runs MCUs + analog + devices + world + radios;
                 instruments runtime; damage; telemetry; command API used by the app
  bb-plugin      plugin manifests, registry/catalog, install/uninstall, wasmtime sandbox,
                 permission gate, adapters (wasm part → bb_core::Device, instrument, generator,
                 world-model, analysis)
  bb-plugin-sdk  guest-side SDK (compiled to wasm32-unknown-unknown; excluded from workspace)
  bb-lessons     lesson file format, step/checker engine, progress store, lesson editor model
  breadbox       the eframe/egui app (screens, canvas renderers, theme, platform glue)
plugins/         first-party plugin sources (Rust → wasm) + their plugin.json + art
lessons/         lesson content (TOML), one folder per track
examples/        example projects (*.breadbox/)
tests/e2e/       cross-crate headless scenario tests (a crate: `breadbox-e2e`, added when needed)
scripts/         test-all.sh, bundle-macos.sh, build-test-firmware.sh, regen-spice-golden.sh
assets/          fonts (+ OFL licenses), icons, app icon
docs/            ARCHITECTURE.md, DESIGN.md, PLUGIN-SDK.md, LESSON-FORMAT.md, design/
```

Dependency direction (no cycles):

```
bb-core ← bb-spice
bb-core ← bb-avr
bb-core ← bb-firmware (← bb-avr for ELF→debug glue only if needed)
bb-core ← bb-world
bb-core ← bb-radio
bb-core, bb-spice ← bb-circuit
bb-core, bb-spice, bb-avr, bb-firmware, bb-circuit, bb-world, bb-radio ← bb-sim
bb-core, bb-sim, bb-circuit ← bb-plugin
bb-core, bb-circuit, bb-sim ← bb-lessons
everything ← breadbox (app)
```

## 2. Shared conventions

- **Time**: `bb_core::SimTime(u64)` = picoseconds since sim start (≈ 213 days range). AVR cycle
  at 16 MHz = 62 500 ps exactly. Analog engine uses `f64` seconds internally; convert at the
  boundary with `SimTime::as_secs_f64()` / `SimTime::from_secs_f64()`.
- **Units**: SI `f64` everywhere (volts, amps, ohms, farads, seconds, lux, °C, %VWC as 0–100).
  `bb_core::si::format(value, "V")` → `"1.882 V"`, `parse("4k7")` → `4700.0`.
- **Errors**: libraries use `thiserror`; the app uses `anyhow`. No panics on user input
  (netlists, sketches, project files, plugin output). `unwrap()` only for true invariants.
- **Logging**: `log` facade; app installs `env_logger` (`RUST_LOG=breadbox=debug`).
- **Determinism**: every random source takes an explicit seed (`rand_chacha`/`rand_pcg`), so
  sim runs replay bit-identically.
- **Serialisation**: `serde` + JSON for project files and plugin manifests (pretty-printed,
  stable key order), TOML for lessons, RON allowed for built-in part definitions.

## 3. Device contract (`bb-core`)

Every simulated thing that is not a plain SPICE element — sensors, LEDs-with-visuals, radios,
displays, instruments, plugin parts — is a `Device`. MCUs are wrapped by `bb-sim` as devices too.

```rust
/// How a device drives one of its pins into the analog circuit.
pub enum PinDrive {
    HighZ,                          // input / floating (leakage modelled by the engine)
    Thevenin { v: f64, r: f64 },    // push-pull output: source v behind r ohms
    Pull { v: f64, r: f64 },        // weak pull (e.g. internal pull-up 35 kΩ to Vcc)
    Current { i: f64 },             // ideal current source into the node
}

pub trait DeviceCtx {
    fn now(&self) -> SimTime;
    fn pin_voltage(&self, pin: usize) -> f64;          // vs ground, at `now`
    fn pin_current(&self, pin: usize) -> f64;          // into the device through `pin`
    fn set_drive(&mut self, pin: usize, drive: PinDrive);
    fn set_ext(&mut self, name: &str, value: f64);     // analog external variable, see §4
    fn wake_at(&mut self, at: SimTime, token: u64);    // schedule on_wake
    fn watch(&mut self, pin: usize, lo: f64, hi: f64); // Schmitt-trigger edge notifications
    fn world(&self) -> &dyn WorldView;                 // read-only world access
    fn world_mut(&mut self) -> Option<&mut dyn WorldMut>; // only if permitted (plugins)
    fn emit(&mut self, ev: DeviceEvent);               // log line, visual change, damage, sound
}

pub trait Device: Send {
    fn kind(&self) -> &str;
    fn on_start(&mut self, ctx: &mut dyn DeviceCtx) {}
    fn on_wake(&mut self, ctx: &mut dyn DeviceCtx, token: u64) {}
    fn on_edge(&mut self, ctx: &mut dyn DeviceCtx, pin: usize, high: bool) {}
    /// Called after each accepted analog step / sync quantum if the device asked for it.
    fn on_tick(&mut self, ctx: &mut dyn DeviceCtx) {}
    fn visual(&self) -> VisualState;                   // LED brightness, LCD buffer, angle, …
    fn save_state(&self) -> serde_json::Value { serde_json::Value::Null }
    fn load_state(&mut self, _s: &serde_json::Value) {}
}
```

UART-speaking devices (ESP-01, HM-10, HC-05, LoRa RYLR896) use a shared `UartPins` helper in
`bb-sim` that decodes bit-banged/USART waveforms from edge timestamps and schedules TX bits, and
hands bytes to the pure `bb-radio` state machines (`feed(byte, now)` / `poll(now) -> Vec<(SimTime,
u8)>`). `bb-radio` never depends on `bb-sim`.

## 4. Analog engine boundary (`bb-spice`)

The rustdoc of `bb_spice` (crate root, `Session`, `CircuitBuilder`) is the detailed reference;
this is the contract other crates code against.

- **Parsing / Pro mode**: `Netlist::parse(text)`, `Netlist::parse_file(path)` or
  `Netlist::parse_with(text, &ParseOptions { file_name, base_dir, loader, libraries, .. })` →
  `Result<Netlist, ParseError>` (`ParseError { span: Span { file, line, col }, msg }`).
  `Netlist::run() -> Result<Results, SimError>` runs every analysis in the netlist with
  `.step`, multi-value `.temp`, `.mc` and `.meas`; `run_with(&RunOptions { cancel:
  Some(CancelToken), progress, skip_mc, only })` is the background-thread variant;
  `run_analysis(&Analysis) -> Vec<Plot>` runs one analysis at nominal parameters.
  `Results { runs: Vec<Run { label, params, plots, measures }>, mc: Option<McResults>, stats,
  warnings }`; `Results::summary()` is the Pro header (`3 steps · 1.82 s solve · 0 convergence
  warnings`). A `Plot { kind: PlotKind, vectors }` holds named vectors (`v(out)`, `i(v1)`,
  `i(d1)`, scale first: `time` / `frequency` / sweep); `McResults::distribution("v(a1)")` gives
  `Distribution { mean, sigma, min, max, values }` + `histogram(bins)`. Export: `export::
  {plot_to_csv, results_to_raw, read_raw}` and `Netlist::to_spice_text()` (ngspice-compatible).
- **Breadbox parts as subcircuits** (`XJ1 VCC 0 A0 CAPSOIL vwc=42`, `XU1 … ATMEGA328P
  fw=garden-node.hex`): an unknown subcircuit is a clean `SimError::Elaboration` naming the
  instance and line. Register parts with `Netlist::add_library(Arc<Library>)`
  (`Library::parse(".subckt …")`) or `CircuitBuilder::library`. Non-numeric instance parameters
  are kept in `netlist::ElemKind::Subckt { text_params, .. }` for bb-sim.
- **Programmatic construction** (bb-circuit's netlister): `CircuitBuilder::new(title)` with
  typed methods — `resistor(name, a, b, ohms)`, `resistor_expr(.., "expr")`, `capacitor`,
  `inductor`, `coupling`, `vsource(name, p, n, SourceDef::dc(5.0).ac(1.0, 0.0).pulse(..))`,
  `isource`, `vcvs/vccs/cccs/ccvs`, `bsource_v/bsource_i`, `switch/cswitch`, `diode/bjt/
  mosfet/jfet` + `model(name, ModelKind::D, &[("is", 1e-14)])`, `tline`, `subckt`,
  `port(name, p, n, PinDrive) -> PortId`, `param`, `option`, `ic`, `temp`, `analysis`,
  `library`, `build() -> Netlist`. Element methods return an `ElementRef` for extra instance
  parameters (`.param("w", 10e-6)`, `.param("tol", 0.05)` for Monte Carlo). Names get their
  SPICE letter prepended when missing (`resistor("LDR", …)` → `rldr`).
- **External variables**: `ext("name")` works in any expression — element values, model
  parameters, subcircuit parameters, B sources. `Session::set_ext(name, v)` re-evaluates only the
  dependents before the next solve (LDR: `R = {350k*pwr(max(ext("J2.lux"),0.01),-0.7)}`).
  Exported `.cir` text freezes them as `.param ext_j2_lux=<value>`.
- **Ports**: declared with `CircuitBuilder::port` (ids in creation order, element `p<name>`)
  or `Session::add_port(name, node_p, node_n) -> Result<PortId>` (after the start this rebuilds
  the solver, keeping node voltages and charges). `Session::set_port(id, PinDrive)`:
  `HighZ` = 1 pS leak, `Thevenin`/`Pull` = Norton equivalent, `Current { i }` = injected into
  `node_p`. Exported to `.cir` as an equivalent V+R / I source after a `* breadbox port` comment.
- **Incremental transient** — `Session::new(&Netlist)` or `Session::with_options(&nl,
  SessionOptions { max_step, method })`:
  - `advance_to(t) -> Result<Vec<Crossing>, SimError>`; `advance_until_crossing(t)` stops at
    the first crossing (`time()` then equals its time) so the host can react exactly there.
  - Reads at the visible time: `node(name) -> Option<NodeId>`, `voltage(name)`,
    `voltage_at(NodeId)` (fast path), `current(element)` (into the first terminal; collector /
    drain for transistors; SPICE sign for V sources), `port_current(PortId)` (sourced into the
    node).
  - `watch(node, lo, hi) -> WatchId` (Schmitt); `Crossing { watch, time, rising }` with times
    interpolated inside solver steps, delivered in time order.
  - Inputs — `set_port`, `set_ext`, `set_param(element, param, v)`, `set_temperature(°C)` —
    take effect at the current time. `op()` re-solves immediately (capacitor voltages and
    inductor currents continuous, algebraic nodes jump) and returns crossings caused by the
    jump; otherwise the next `advance_to` does it. Drive changes made before the first advance
    are an event at t = 0 after the initial operating point (which uses the declared drives).
  - `snapshot() -> SessionSnapshot` / `restore(&snap)`; `stats()`; `to_spice_text()` (drives and
    ext values frozen).
  - Cost model: the solver steps ahead with LTE-controlled steps and interpolates, so a 1 µs
    quantum is ~free; an input change inside a step rewinds by interpolation and re-solves.
    Circuits without C/L or time-dependent sources are solved only when an input changes;
    settled reactive circuits stop stepping. Measured: 490 Hz PWM into RC + LED at 1 µs
    quanta runs 26× faster than real time.

## 5. MCU boundary (`bb-avr`, `bb-firmware`)

**Core (`bb-avr`).** One `Avr` per MCU; instances are independent, `Send` and `Clone` (a clone
is a snapshot). Time is counted in oscillator cycles (`cycle()`); `time()` converts to
`SimTime` exactly at 16 MHz.

- Construction / firmware: `Avr::new(AvrConfig::new(Mcu::Atmega328p))` (`f_cpu`, `stop_on`,
  `bootloader` hand-off emulation, `vcc` + `bod_level`, start-up delays, `adc_noise_lsb` +
  `seed`, `shadow_stack`), `Avr::with_mcu(mcu)`. `flash(&FirmwareImage)`, `load_hex(&str)`,
  `load_elf(&[u8])` program flash (+ `.eeprom`) and reset; EEPROM contents are kept.
  `reset(ResetCause)`, `set_reset_pin(low)`, `set_vcc(v)` (brown-out / power-on reset),
  `set_aref(v)`, `set_temperature(°C)`.
- `run(&mut self, until_cycle: u64, io: &mut dyn AvrIo) -> StopReason`, where `StopReason` ∈
  {`ReachedCycle`, `PinChange`, `AdcSampleNeeded{pin, cycle}`, `Breakpoint{pc}`,
  `Watchpoint{addr, write, pc}`, `Break{pc}`, `StepDone`, `Sleep{mode}`, `Reset{cause}`,
  `Paused`, `Fault{kind, pc}`}. The core stops *early* on externally visible events (which
  ones: `StopOn`), at the next instruction boundary; it may overshoot `until_cycle` by up to one
  instruction. Peripherals are event driven (no per-cycle ticking).
- Outputs: `take_pin_events() -> Vec<PinEvent { cycle, pin, state }>` with exact cycle stamps
  (`PinState::Input { pullup }` / `Output { high }` — logical only; the Thevenin/pull-up
  electrical model lives in `bb-sim`). `pin_info(pin) -> PinInfo { ddr, port, pullup, owner,
  state, level, external }` (`owner`: GPIO / OC timer channel / USART / SPI / TWI).
- Inputs are *pushed*: `set_pin_input(pin, Some(level))` = resolved line level seen by the
  input buffer (the co-sim includes the MCU's own drive in its solution; `None` = disconnected,
  the pin then reads its own drive/pull-up). `schedule_pin_input(pin, level, at_cycle)` applies a
  change at an exact future cycle (UART frames from devices, test stimuli).
  `set_pin_voltage(pin, v)` feeds the analog comparator and is the ADC fallback.
- `AvrIo` (pulled): `adc_sample(pin, cycle) -> Option<f64>` at the exact sample-and-hold
  instant; `None` falls back to the pushed voltage, else `run` returns `AdcSampleNeeded` —
  advance the analog side to `cycle`, call `supply_adc_sample(v)`, run again.
  `spi_exchange(mosi, cycle) -> u8` is the byte-level SPI fast path (with
  `set_spi_waveform(false)`). `()` implements `AvrIo` with defaults.
- USART fast paths: `take_usart_tx(n) -> Vec<UsartByte { cycle, data }>` (serial monitor),
  `usart_inject(n, bytes)` (typed input, correctly timed frames), `set_usart_waveform(n, false)`
  (TX pin stays idle — use when nothing is wired to TX), `usart_baud(n)`. The TX pin still
  carries the exact bit waveform by default.
- Debug: `set_breakpoint(byte_addr)` / `clear_breakpoint` / `breakpoints()`,
  `set_step_filter(Some(StepFilter::{Instructions(n), OutOfRanges{..}, Return{depth}}))`,
  `step_instruction(io)`, `request_pause()`, `add_watchpoint(..)`, `call_stack() ->
  &[CallFrame]` (exact shadow stack maintained by CALL/RET/interrupts), `pc()` (byte address),
  `sp()`, `sreg()`, `reg(n)`, `rampz()`, `eind()`, `peek/poke(addr)`, `sram()`, `flash_bytes()`,
  `eeprom()`, `io_registers() -> Vec<(name, addr, value)>`, `instruction_at(pc)` (disassembly
  via `Display`), `instructions()`, `last_reset_cause()`, `sleep_mode()`, `in_reset()`.

**Toolchain + debugging (`bb-firmware`).**

- `Toolchain::detect()` (env `BREADBOX_ARDUINO_CLI`, `PATH`, Homebrew, app-support dir) →
  `{ arduino_cli, cli_version, source, avr_core_version, avr_gcc_version, overrides }`;
  `ready()`, `compiler_label()` ("avr-gcc 7.3.0"). `Toolchain::install(dir, progress)` =
  `install_cli` (download + extract + private `arduino-cli.yaml`) + `install_core`.
  `lib_search/lib_install/lib_list`. On Apple Silicon without Rosetta the bundled x86_64
  avr-gcc/ctags cannot run: `NativeOverrides` (native avr-gcc from Homebrew `osx-cross`, native
  Arduino ctags under `toolchain/native/`) are passed as build properties automatically.
- `Toolchain::compile(sketch_dir, &CompileOptions { fqbn, debug, build_dir, warnings,
  properties }) -> BuildOutput { success, elf, hex, size: Option<SizeReport>, diagnostics,
  log, build_dir, duration, banner }`. Per-sketch cache dir under
  `~/Library/Caches/Breadbox/builds` (fast re-verify). `debug: true` builds `-Og -fno-lto` for
  source-level debugging. `Diagnostic { file, tab, line, column, severity, message, excerpt,
  context }` is mapped back to the sketch's tabs (`.ino.cpp` `#line` remapping).
  `SizeReport::arduino_lines()` reproduces arduino-cli's text; `display_lines()` adds thousands
  separators (design); `problems()` gives the low-memory / too-big messages.
  `BuildOutput::image()` → `bb_avr::FirmwareImage`; `output_lines()` for the output panel.
- `DebugInfo::from_elf(&[u8])`: `line_addresses(file, line)` (breakpoint PCs, next line with
  code), `location(pc)`, `line_ranges(pc)`, `is_statement_start(pc)`, `function_at(pc)`,
  `function_name(pc)` (DWARF or symbol), `globals`, `symbols`, types, CFI (`cfi_at`, `unwind`),
  `read_variable(..) -> VarValue { name, type_name, value: Value, address, location }`.
- `Debugger::new(Arc<DebugInfo>)` is a policy layer over an `Avr` driven by someone else:
  `add_breakpoint(&mut avr, file, line, HitCondition::{Always, Equals(n), AtLeast(n),
  Every(n)})`, `on_stop(&mut avr, &StopReason) -> Decision::{Continue, Pause(PauseInfo)}` for
  *every* stop, `step_over/step_into/step_out/step_instruction/pause`, `call_stack`, `locals(avr,
  frame)`, `globals`, `read(avr, name, frame)`, `rebind(new_info, avr)` after re-flash,
  `run_until_pause` for standalone use.
- `Board::{all, by_id, by_fqbn}`: UNO R3 / Nano / Mega 2560 — FQBN, MCU, Arduino pin ↔ port/bit,
  ADC channel, PWM timer/channel/frequency, attachInterrupt number, bus pins, LED_BUILTIN,
  serial ports, header layouts.
- `HostBoard::spawn(...)` for ESP32/Pico API-level emulation (`MCU-6`) is phase 3.

Co-simulation recipe (`bb-sim`): run each MCU to the quantum end; on `PinChange` read the
events (exact cycles) and update the analog drives at those times; after solving, push the
resolved logic levels with `set_pin_input` (Schmitt thresholds are the co-sim's job) and
comparator voltages with `set_pin_voltage`; schedule device-generated edges you already know
(UART bits) with `schedule_pin_input`; answer `AdcSampleNeeded` by advancing the analog side to
`cycle`. Hand every stop to the MCU's `Debugger` (if attached); `Pause` = stop calling `run`.

## 6. Co-simulation scheduler (`bb-sim`)

```
loop (on sim thread):
  t_target = min(next scheduled wake, next MCU event estimate, t + quantum)
  for each MCU: run until t_target or an early stop (pin change / ADC / breakpoint)
  t_evt = earliest stop time
  analog.advance_to(t_evt)          // adaptive internal steps, threshold watches
  apply MCU/device drive changes → analog.op_at_discontinuity()
  deliver threshold crossings (in time order) → MCU input pins / device on_edge
  run due device wakeups; world.step(dt); radios.step(now)
  publish telemetry (≤ 60 Hz), pace against wall clock at chosen speed
```

- Commands in (crossbeam channel): play, pause, step, set_speed, reset, flash(mcu, image),
  set_world(var, value), set_part_param, probe add/remove, instrument controls, radio
  conditions, debugger ops.
- Telemetry out: an `Arc<ArcSwap<Telemetry>>` snapshot (or triple buffer) + bounded channels
  for serial/radio/log streams. UI never blocks on the sim.
- Bench rebuild on circuit edits: incremental where possible (param edits), otherwise rebuild
  the analog session and keep MCU/world/radio state.

## 7. Plugin runtime (`bb-plugin`)

- Manifest (`plugin.json`): see SPEC `PLUG-1`; JSON Schema in `docs/plugin.schema.json`.
- Sandbox: wasmtime `Engine` with fuel + epoch deadline, `StoreLimits` memory cap, no WASI by
  default (WASI only with `fs:project` permission, preopened to the project folder).
- ABI: core-wasm exports (`bb_abi_version`, `bb_alloc`, `bb_init(cfg_ptr,len)`, `bb_on_wake`,
  `bb_on_edge`, `bb_on_tick`, `bb_visual`, `bb_instrument_read`, `bb_generator_eval(t)`,
  `bb_analysis_run`, `bb_world_step`) and host imports in module `bb` (`now`, `pin_voltage`,
  `set_drive`, `set_ext`, `wake_at`, `watch`, `world_get`, `world_set`, `emit`, `log`, `solve_op`
  …) — each import checks the declared permission. Messages bigger than scalars are JSON in
  guest memory. Exact ABI lives in `docs/PLUGIN-SDK.md`.
- Adapters turn a loaded plugin into `bb_core::Device` (parts, instruments), a generator
  (custom source), a world model, or an analysis — registered into `bb-sim`/`bb-circuit` the
  same way built-ins are.

## 8. App (`breadbox`)

- `eframe` + `egui` (wgpu), `egui_extras` (tables, svg), `egui_plot` (plots),
  `egui_kittest` for GUI tests, `rfd` for native dialogs, `muda` (or eframe menus) for the
  macOS menu bar, `rodio`/`cpal` for buzzer/speaker audio.
- `screens/{home,workbench,firmware,pro,plugins,lessons}.rs`; `canvas/{breadboard,schematic,
  pcb}.rs` renderers + hit-testing; `theme.rs` (tokens from `docs/DESIGN.md`); `widgets/`
  (segmented control, tiles, sliders with stops, chips, code editor, plots, instrument faces).
- Every interactive widget has an AccessKit label so `egui_kittest` can drive it by name.

## 9. Project file layout

```
garden-node.breadbox/
  project.json          { "format": "breadbox/1", circuit, views, world, radios, instruments,
                          probes, settings }
  firmware/U1/garden-node/garden-node.ino, wifi_at.h, mqtt_lite.h
  analyses/*.json       saved Pro analyses
  .breadbox-cache/      build output (gitignored)
```

## 10. Build, test, tooling

- Toolchain: rustup stable (`~/.cargo/bin` must be first on PATH — Homebrew's older rustc is
  also installed). `rust-toolchain.toml` adds `wasm32-unknown-unknown`.
- External tools used by tests/dev (install with Homebrew when missing): `arduino-cli` (+
  `arduino-cli core install arduino:avr`), `ngspice` (golden-data regeneration only).
- `scripts/test-all.sh` is the single green-light command.
