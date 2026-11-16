# Breadbox — Architecture

Companion to `SPEC.md` (what) — this file is *how*. Contract sketches below are binding in
spirit: an owner may refine a signature, but must keep this document updated in the same change
and must not break other crates' use of it.

## 1. Workspace map

```
crates/
  bb-core        shared types: SimTime, SI units/formatting, ids, PinDrive, Device trait,
                 WorldView, events, `paths` (resources + user-data folders). Tiny,
                 dependency-light. Everyone depends on it.
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
scripts/         test-all.sh, bundle-macos.sh, check-bundle.sh, build-plugins.sh,
                 build-test-firmware.sh, regen-spice-golden.sh
assets/          fonts (+ OFL licenses), app icon (icon/breadbox-icon.svg)
docs/            ARCHITECTURE.md, DESIGN.md, PLUGIN-SDK.md, LESSON-FORMAT.md, design/
```

Dependency direction (no cycles):

```
bb-core ← bb-spice
bb-core ← bb-avr
bb-core ← bb-firmware (← bb-avr for ELF→debug glue only if needed)
bb-core, bb-spice ← bb-world            (expressions for timelines/scripts)
bb-core ← bb-radio
bb-core, bb-spice, bb-world, bb-firmware ← bb-circuit   (board pin maps, world config)
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
- **Portability** (`PLAT-1`): OS-specific code (`target_os`, Homebrew paths, privacy-protected
  folders, process groups, file permissions, AppKit) lives only in the `platform` modules:
  `breadbox::platform` (menus, documents, dialogs), `bb_firmware::platform` (toolchain paths,
  arduino-cli asset, process groups) and `bb_core::paths` (resources, user data).
  `bb-firmware/tests/platform.rs` greps the workspace and fails on `target_os` anywhere else.

## 3. Device contract (`bb-core`)

Every simulated thing that is not a plain SPICE element — sensors, LEDs-with-visuals, radio
modules, displays, motors, instruments, plugin parts — is a `bb_core::Device`. A part's
electrical behaviour stays in SPICE (its `model.spice` template, §3b); the device adds what
SPICE cannot: protocols, world coupling, visuals, damage, user input. MCUs are *not* devices:
`bb-sim` drives them through its own `McuCore` trait (§6).

```rust
pub enum PinDrive {                 // how a device drives one of its pins
    HighZ,                          // input / floating
    Thevenin { v: f64, r: f64 },    // push-pull output: source v behind r ohms
    Pull { v: f64, r: f64 },        // weak pull (internal pull-up, open-drain release)
    Current { i: f64 },             // current source into the node (supply draw: negative)
}

pub trait DeviceCtx {
    fn now(&self) -> SimTime;
    fn pin_voltage(&self, pin: usize) -> f64;               // vs ground at `now`
    fn pin_current(&self, pin: usize) -> f64;               // into the device via its drive
    fn element_current(&self, element: &str) -> Option<f64>;  // own SPICE element ("D", "R")
    fn element_charge(&self, element: &str) -> Option<f64>;   // ∫i dt since start (exact avg)
    fn track_element(&mut self, element: &str);             // start integrating (on_start)
    fn element_energy(&self, element: &str) -> Option<f64>; // ∫v·i dt (self-heating)
    fn set_drive(&mut self, pin: usize, drive: PinDrive);
    fn set_ext(&mut self, name: &str, value: f64);          // ext("<REF>.<name>") in its model
    fn wake_at(&mut self, at: SimTime, token: u64);         // → on_wake(token)
    fn cancel_wake(&mut self, token: u64);
    fn watch(&mut self, pin: usize, lo: f64, hi: f64);      // Schmitt edges → on_edge
    fn tick_every(&mut self, period: SimTime);              // → on_tick (ZERO stops)
    fn world(&self) -> &dyn WorldView;
    fn world_mut(&mut self) -> Option<&mut dyn WorldMut>;   // pumps, heaters, lamps, LEDs
    fn rng(&mut self) -> &mut Rng;                          // seeded per bench + part
    fn emit(&mut self, ev: DeviceEvent);
}

pub trait Device: Send {
    fn kind(&self) -> &str;                                  // behaviour id ("dht22")
    fn on_start(&mut self, ctx: &mut dyn DeviceCtx) {}       // operating point already solved
    fn on_wake(&mut self, ctx: &mut dyn DeviceCtx, token: u64) {}
    fn on_edge(&mut self, ctx: &mut dyn DeviceCtx, pin: usize, high: bool) {}
    fn on_tick(&mut self, ctx: &mut dyn DeviceCtx) {}
    fn on_input(&mut self, ctx: &mut dyn DeviceCtx, input: &UserInput) {} // press, knob, switch
    fn set_param(&mut self, ctx: &mut dyn DeviceCtx, key: &str, value: f64) -> bool { false }
    fn visual(&self) -> VisualState;                          // Look + health + smoke
    fn readings(&self) -> Vec<(String, f64)> { vec![] }       // inspector live values
    fn save_state(&self) -> serde_json::Value;                // kept across bench rebuilds
    fn load_state(&mut self, s: &serde_json::Value);
}
```

- `UserInput`: `Press(bool)`, `Toggle{index}`, `SetSwitch{index,on}`, `Position(0…1)`,
  `Value{key,value}`.
- `DeviceEvent`: `Log{level,text}`, `Annotation{channel,text}` (shown inline in serial monitors,
  e.g. `[ESP-01] MQTT PUB garden/soil 41`), `Stress{reason}`, `Damage{reason}`, `Sound(Click |
  Pop | Beep)`, `Audio(AudioChunk)` (buzzers/speakers, 1 kHz…), `Erc{…}`.
- `VisualState { look: Look, health: Health, smoke: f32 }`; `Look` is a closed enum the
  renderers draw: `Led{color,brightness,current}`, `RgbLed`, `LedArray`, `SevenSeg`, `Motor`,
  `Servo`, `Stepper`, `TextLcd{cols,rows,text,codes,cgram,cursor,blink,display_on,backlight}`,
  `Pixels` (OLED), `Relay`, `Buzzer`, `Switch`, `Switches`, `Knob`, `Board{power,l,tx,rx}`,
  `Readout{text}`.
- `WorldView` (read): `time`, `world_secs`, `var(key)`, `soil_vwc(pot)`, `lux_at(pos)`,
  `temperature_at(pos)`, `humidity`, `pressure`, `distance(pos,dir,half_angle,max)` (sonar),
  `motion(pos,range,window)` (PIR), `magnetic_field(pos)` (hall), `noise_enabled`.
  `WorldMut`: `set_var`, `water(pot, ml)`, `set_heat_source`, `set_light_source`.
- Built-in behaviours live in `bb_sim::devices` behind a `DeviceRegistry` (behaviour id →
  factory); plugins register theirs (`bb_plugin::PluginHost`). `esp01`, `hm10`, `hc05` and
  `rylr896` are `bb_sim::devices::RadioModule`s bridging their UART pins to the bench's
  `RadioService` (§6c); without a radio service they fall back to `AtStub`.
- Additive hooks (all defaulted): `DeviceCtx::service(name) -> Option<&mut dyn Any>` (the
  bench's services, e.g. `"radio"`), `DeviceCtx::capture(pin, period)` / `samples(pin, from,
  to)` (waveform recording for instruments: every change on digital nets, at least every
  `period` on analog nets — the scheduler syncs that often while a capture is active),
  `Device::panel() -> Option<Value>` (live instrument panel → `PartTelemetry.panel`),
  `Device::i2c_transfer(ctx, addr, write, read)` / `spi_transfer(ctx, mosi)` (transaction-level
  buses for API-level boards).
- UART devices use `bb_sim::devices::UartPort`: it decodes the MCU's TX waveform from edge
  timestamps (`bb_radio::uart` decoder) and drives RX with exact bit timing.

## 3b. Parts library schema (`bb-circuit`, PART-1…9)

Parts are declarative JSON (`crates/bb-circuit/parts/*.json`, embedded at build time). Plugins
add parts with the *same* schema through `Library::add_json` (PART-9), so one loader, validator
and renderer path serves both. One file = `{"parts": [PartDef…]}` or a bare list.

```jsonc
{
  "id": "resistor", "name": "Resistor", "category": "building_blocks",   // boards, building_blocks,
  "simple": true, "simple_name": "Resistor", "icon": "res",               // light_sound, sensors,
  "ref_prefix": "R", "description": "…", "part_number": "",               // power, radios, displays,
  "props": [                                                              // motors_actuators, …
    {"key": "resistance", "label": "Resistance", "unit": "Ω", "type": "number",
     "default": 10000, "min": 0.01, "max": 1e9, "series": "E24", "live": true},
    {"key": "tolerance", "type": "choice", "default": "5",
     "options": [{"value": "1"}, {"value": "5", "sets": {"tempco": 100}}]}   // choices may set props
  ],                                                                    // types: number | choice | bool | text
  "pins": ["1", "2"],                     // short form "id:role:Name" (roles: io, input, output,
                                          // power_in, power_out, ground, nc, passive…)
  "bonds": [["GND", "GND.2"]],            // internally connected pins
  "model": {
    "spice":   ["$(el:R) $(1) $(2) {$(ext:r)*(1+$(tempco)*1e-6*($(ext:temp)-25))}"],
    "pro":     ["$(el:R) $(1) $(2) $(resistance)"],   // Pro-mode card (exported .cir)
    "library": [".model …"],                          // shared .model/.subckt cards (deduped)
    "damaged": {"open": […], "short": […]},           // cards after destruction
    "behaviour": "thermal",                           // bb_sim device factory id
    "device_pins": ["…"],                             // pins the device drives/watches
    "ext": [{"name": "r", "default": "$(resistance)"}, {"name": "temp", "default": 25}],
    "mcu": {"board": "uno", "core": "avr", "vcc_pin": "5V", "label": "ATmega328P"}  // boards
  },
  "inspector": [{"label": "Resistance", "value": "$(resistance|Ω) ±$(tolerance) %"}],
  "ratings": [{"element": "R", "kind": "power", "limit": "$(power)", "tau": 3.0,
               "destroy_at": 3.0, "message": "resistor overheated", "effect": "smoke"}],
  "bench":     {"art": "resistor_axial", "layout": {"axial": 4}, "flexible": true,
                "sockets": false, "off_board": false, "hidden": false, "leads": [], "size": [w, h]},
  "schematic": {"symbol": "resistor"},           // pins/geometry generated per symbol kind
  "pcb":       {"footprint": "Resistor_THT:…", "smd": "Resistor_SMD:R_0805_2012Metric"},
  "tags": ["spice"]
}
```

- **Templates** (expanded by the netlister): `$(pin)` → the pin's node; `$(prop)` /
  `$(prop:key)` → property value (SI-formatted); `$(el:R)` / `$(el:R:suffix)` → unique element
  name (`R_R1`, local name `R` for `element_current`); `$(n:local)` → a private node;
  `$(ext:name)` → `ext("REF.name")` (set live by devices / UI, default from `model.ext`);
  `$(world:key)` → the world variable (Pro export); `?key=value:` prefix → conditional line.
- **Layouts**: `axial: span` (2-lead, bendable), `row` (inline header), `dip: n` (straddles the
  channel), `rows` (explicit), `board` (MCU boards — header holes from `bb_firmware::Board`),
  `breadboard` (830 / 400 hole maps: `a1…j63`, rails `t+N t-N b+N b-N`). Coordinates are in
  0.1" grid units; `Placement` (at, rotation, flip) maps leads into holes (snap 0.26 grid).
- **Ratings** watch a filtered quantity (`power`, `current`, `voltage`, `reverse_voltage`) of a
  SPICE element; > 1× limit → `Stress`, > `destroy_at`× → `Damage` and the part's `damaged`
  cards (open / short) replace its model in a rebuild.
- `Library::builtin()` validates every part (unique ids, pins referenced by templates exist,
  ext defaults refer to props, layouts consistent) — `cargo test -p bb-circuit` fails on a bad
  part file. Queries: `get(id)`, `simple()` (Simple-mode palette order), `categories()`,
  `search(text)`, `to_json()`.

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
  {plot_to_csv, results_to_raw, read_raw}` and `Netlist::to_spice_text()` (ngspice-compatible;
  Breadbox-only cards — `.step`, `.mc`, multi-`.temp`, `.dc` over a `.param` — are written as
  `*!bb <card>`, a comment for ngspice that bb-spice parses back);
  `to_spice_text_with_ext(&[(name, value)])` freezes `ext()` variables at given values (bb-circuit's
  `.cir` export uses the parts' values — a battery's EMF — instead of 0).
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
  `bootloader` hand-off emulation, `vcc` + `bod_level`, start-up delays, `adc_model`
  (`AdcModel::Ideal` = exactly `floor(V/Vref·1024)` clamped; `AdcModel::Realistic(AdcErrors)` =
  per-chip offset / gain error + INL / DNL from the seed (`AdcChip`) and clipped Gaussian
  conversion noise from the seeded RNG; `AdcErrors::DATASHEET`; live via `set_adc_model`,
  `WORLD-6`), extra Gaussian `adc_noise_lsb` +
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
  `flash_generation()` changes (process-unique) whenever program memory changes (flash, load,
  SPM page write) so UIs cache a flash copy. The static table
  `bb_avr::io_registers(Mcu) -> &'static [IoRegister { name, addr, peripheral }]` (address
  order; `peripheral` = `"Port B"`, `"Timer/Counter1"`, `"USART0"`, `"ADC"`, `"SPI"`,
  `"TWI (I2C)"`, `"EEPROM"`, `"External interrupts"`, `"Watchdog"`, `"CPU"`, …) names every
  I/O register of the 328P and the 2560 (`PORTB` = `0x25`, `PORTH` = `0x102`).

**Toolchain + debugging (`bb-firmware`).**

- `Toolchain::detect()` (env `BREADBOX_ARDUINO_CLI` — when set, `PATH` and Homebrew are not
  searched, so a missing path simulates a first run — `PATH`, the package-manager folders of
  `bb_firmware::platform`, app-support dir; duplicates skipped; a candidate that fails or times
  out does not stop the search, and the first failure is returned when none works) →
  `{ arduino_cli, cli_version, source, config_file, data_dir, user_dir, downloads_dir,
  avr_core_version, avr_gcc_version, overrides, timeouts }`. **Private settings**: every call
  (`Toolchain::command`) passes Breadbox's `--config-file` (`toolchain/arduino-cli.yaml` for its
  own install, `toolchain/arduino-cli-system.yaml` for one from PATH / Homebrew) and the same
  directories as `ARDUINO_DIRECTORIES_*` (they beat inherited ones; `ARDUINO_CONFIG_FILE` is
  removed): sketchbook = `toolchain/sketchbook` (libraries), downloads = `toolchain/staging`,
  updater off; data = `toolchain/data`, except that an external arduino-cli reuses the user's
  Arduino data folder (`user_arduino_data_dirs`: `$ARDUINO_DIRECTORIES_DATA`, their config's
  `directories.data`, `~/Library/Arduino15`) when it holds arduino:avr and is not
  privacy-protected. Never `~/Documents/Arduino`: a Finder-launched app's subprocess touching it
  blocks on the macOS privacy prompt (this hung detection). **Time budgets**: every subprocess
  runs through `bb_firmware::process::run(cmd, limit, cancel, on_line)` (own process group,
  killed with its children on timeout / cancel → `FirmwareError::Timeout { what, limit }` /
  `Cancelled`); `Timeouts { query 20 s ($BREADBOX_CLI_TIMEOUT_MS), search 90 s, install 10 min,
  long_install 30 min, probe 10 s }`, downloads have connect / response / body limits.
  `ready()`, `compiler_label()` ("avr-gcc 7.3.0"). `Toolchain::install(dir, progress)` =
  `install_cli` (download + extract + private `arduino-cli.yaml`; `$BREADBOX_ARDUINO_CLI_URL`
  overrides the archive URL, `file://` works) + `install_core` (+ native avr-gcc / ctags on
  Rosetta-less Apple Silicon).
  `lib_search/lib_install/lib_list`. On Apple Silicon without Rosetta the bundled x86_64
  avr-gcc/ctags cannot run: `NativeOverrides` (native avr-gcc from Homebrew `osx-cross`, native
  Arduino ctags under `toolchain/native/`) are passed as build properties automatically.
  `Toolchain::install` makes that case automatic: after the core it calls
  `install_native_tools_with(&NativeInstallOptions { brew, brew_prefixes, ctags_url, app_dir },
  progress)` when `needs_native_tools()` — `brew install osx-cross/avr/avr-gcc@8` (merged into
  `toolchain/native/avr-gcc/bin` with avr-binutils) and `build_ctags(CTAGS_SOURCE_URL, …)`
  (download, patch for current SDKs, `configure && make`). Without Homebrew it fails with
  `FirmwareError::NativeTools` explaining the Rosetta / Homebrew options. With Rosetta nothing
  changes.
- `Toolchain::compile(sketch_dir, &CompileOptions { fqbn, debug, build_dir, warnings,
  properties, timeout (5 min), cancel: Option<CancelToken> }) -> BuildOutput { success, elf, hex,
  size: Option<SizeReport>, diagnostics,
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
- ESP32 DevKit / Raspberry Pi Pico API-level emulation (`MCU-6`): see §5b.

Co-simulation recipe (`bb-sim`): run each MCU to the quantum end; on `PinChange` read the
events (exact cycles) and update the analog drives at those times; after solving, push the
resolved logic levels with `set_pin_input` (Schmitt thresholds are the co-sim's job) and
comparator voltages with `set_pin_voltage`; schedule device-generated edges you already know
(UART bits) with `schedule_pin_input`; answer `AdcSampleNeeded` by advancing the analog side to
`cycle`. Hand every stop to the MCU's `Debugger` (if attached); `Pause` = stop calling `run`.

## 5b. Host-emulated boards (`bb-firmware::host`, `MCU-6`)

ESP32 DevKit v1 and Raspberry Pi Pico sketches compile natively (system `clang++`, or
`$BREADBOX_CXX`) against Breadbox's Arduino-compatible core in `crates/bb-firmware/hostcore/`
(embedded in the binary via `host::compile::HOSTCORE`, materialised once under the build cache)
and run as a **child process in lock-step with simulated time**.

- Build: `compile_host(sketch_dir, HostKind::{Esp32DevKit, Pico}, build_dir) -> HostBuild {
  success, exe, diagnostics, log, duration, cached }`. Arduino preprocessing: `.ino` tabs
  concatenated (main first), prototypes generated before the first function definition
  (`find_prototypes`, default arguments stripped), `#line` directives so `Diagnostic { tab, line,
  column }` point at the user's tabs; `.cpp/.c` tabs compiled, `.h` tabs on the include path.
  The host core is compiled once per board kind and cached (`hostcore-<hash>/obj-<kind>`).
  Board defines: `BB_HOST_ESP32` / `BB_HOST_PICO` + the usual `ESP32`, `ARDUINO_ARCH_*`.
- Core coverage: `Arduino.h` (pins, time, GPIO, `analogRead` 12-bit default on both boards +
  `analogReadResolution`, `analogWrite` / LEDC 2.x channel API and 3.x pin API, `tone`,
  `attachInterrupt(Arg)`, `pulseIn`, `shiftIn/Out`, `random` seeded from the board seed, `map`,
  `ESP.restart()`, FreeRTOS `vTaskDelay`), `String`, `Print`/`Stream`, `Serial`/`Serial1`/`Serial2`,
  `Wire`, `SPI`, `WiFi.h` (`WiFiClass`, `WiFiClient`, `WiFiServer`, `WiFiUDP`), `HTTPClient`,
  `PubSubClient` (real MQTT 3.1.1 bytes over the simulated TCP), ESP32 BLE Arduino subset
  (`BLEDevice/BLEServer/BLEService/BLECharacteristic/BLEAdvertising/BLE2902`, callbacks),
  `Preferences` (file-backed NVS), `configTime`/`getLocalTime` (real NTP packets), `DHT.h`
  (Adafruit-compatible, real single-wire protocol on the pin).
- Protocol (`host::protocol`, mirrored by `hostcore/bb_host.h`): child→parent `[u32 len][u8
  type][payload]`; parent→child replies `[u32 len][u64 now_us][u8 n][n irq pins][payload]`.
  One-way messages (pin mode/write, PWM, serial output, IRQ setup, logs) are buffered by the child
  and stamped by the parent's board clock; requests (time, reads, sleeps, pin waits, I²C/SPI,
  services) block the child until answered. Interrupt pins ride on every reply; ISRs run inside
  the child before the call returns.
- Time model: the child only runs between replies. Each message costs simulated time (time/status
  polls 0.25–1 µs, escalating in long busy-wait loops so `while (millis() < t) {}` advances;
  `analogRead` 10 µs ESP32 / 2 µs Pico; I²C/SPI by bit time; radio services 20 µs); `delay` /
  `delayMicroseconds` / `WAIT_PIN` block until the board clock reaches the deadline, an attached
  interrupt fires, or (async mode, enabled by BLE) every `poll_interval` (10 ms) so callbacks run.
- `HostBoard::spawn(exe, HostConfig { kind, seed, nvs_path, poll_interval, stop_on_output,
  hang_timeout })`; `run(until, &mut dyn HostIo) -> HostStop::{ReachedTime, Output, Panic,
  Exited { code, stderr }, Hung}` never lets the board clock pass `until` while the child waits
  (it may overshoot by one message cost while the child computes). `stop_on_output` returns after
  each pin/PWM change so a device (DHT22) can schedule its answer before the sketch samples, and
  after `begin` / writes on UART ports ≠ 0 (their TX waveform starts at the write time).
  Inputs pushed: `set_pin_input(gpio, Some(level)|None)`, `schedule_pin_input(gpio, level, at)`,
  `set_pin_voltage(gpio, v)`, `serial_inject(port, bytes)`. Outputs: `take_events() ->
  [HostEvent::{Pin { time, pin, state: HostPinState::{Input{pull}, Output{high}, OpenDrain{low}}
  }, Pwm { time, pin, duty, freq_hz }, Log, Restarted}]`, `take_serial(port) -> [(t, byte)]`
  (paced at the configured baud), `serial_baud(port)`, `serial_pins(port) -> (rx, tx)` (GPIOs
  given to ESP32 `SerialN.begin(baud, config, rx, tx)`, `None` = default; the message carries
  them as two optional bytes, 0xff = default), `pin_state`, `stderr`. `reset(Some(new_exe))` =
  re-flash (NVS kept; the UARTs close — `begin` again). `ESP.restart()` respawns the process.
- `HostIo` (pulled, at the exact board time): `analog_read(pin, t) -> Option<volts>`,
  `i2c_write/i2c_read(bus, addr, …)`, `spi_transfer(bus, bytes)`, and
  **`service(channel, request, t) -> reply`** — opaque Wi-Fi/socket (channel 1) and BLE (channel
  2) requests whose byte format is owned by `bb_radio::host`. bb-firmware does not depend on
  bb-radio; the co-sim forwards: `fn service(..) { radio.host_service(device, ch, req, t) }`.
- Catalogue: `HostKind::spec() -> HostBoardSpec { pins: [HostPin { gpio, name, adc: (unit,
  ch), input_only, touch, dac, pwm, functions }], adc_bits, adc_default_bits, pwm_channels,
  pwm_default_bits/freq, led_builtin, sda/scl/mosi/miso/sck/ss, serial, wifi, ble, headers,
  emulation: "API-level" }` for ESP32 DevKit v1 (30-pin DOIT) and Pico (40-pin).

## 6. Co-simulation scheduler (`bb-sim`)

`Bench::new(project, library, BenchOptions)` builds a runnable bench: `bb_circuit::netlist::build`
(nets, ports for every MCU/device pin, ratings, ext defaults) → `bb_spice::Session` (only if
the circuit has non-ground analog content) → one `McuCore` per board (`AvrMcu` for AVR boards,
`HostMcu` for ESP32/Pico when an executable is supplied, `IdleMcu` placeholder otherwise) → one
`Device` per behavioural part (registry) → world (`bb_world::World`) → instruments → services
(radio space, plugin hosts). The operating point is solved before devices' `on_start` so they
see real supply voltages.

**Net kinds.** `Analog(node)` nets live in the SPICE session. Nets that join only MCU pins and
device outputs with no analog element are `Digital` (closed-form Norton resolution of all
drives, no solver). Unconnected pins are `Isolated`. MCU pins on analog nets are two ports
(high side to the board's Vcc node, low side to ground) driven by `PinModel` (AVR: 25 Ω / 22 Ω,
35 kΩ pull-up, VIL 0.3·Vcc / VIH 0.6·Vcc, 40 mA abs-max; ESP32: 30 Ω, 45 kΩ, 0.25/0.75).

```
step(limit):                         // MCUs lead, the analog circuit lags
  q = adaptive quantum               // 1 µs…1 ms: max when the analog solver is quiescent
                                     // or no MCU; slope-based time-to-threshold for watched
                                     // MCU inputs; 1 µs with several MCUs
  target = min(t + q, limit, next device wake, next service wake)
  for each MCU: run_until(target) → stops early on a pin change, an ADC sample the bench
                cannot answer inline, a debugger pause, or a fault
  t_sync = earliest stop
  advance analog to t_sync, applying MCU pin events at their exact times; threshold crossings
    (advance_until_crossing) are delivered in time order → MCU inputs (set_input /
    schedule_input) and device on_edge; a device's drive change re-solves at that instant
  answer a pending ADC sample at its sample-and-hold time
  run due device wakes / ticks; world.step every 10 ms; services due (radio space)
  after_sync: serial bytes → monitors, reset causes, annotations, damage + pin damage,
    instruments (probe/scope/meter), telemetry counters
```

- **Inline answers.** When the circuit is quiescent and a single MCU runs, ADC samples are
  answered from the current solution without stopping (garden-node: 26 k MCU stops in 30 s
  instead of ~200 k). API-level boards always get inline answers.
- **Exactness.** Edges are delivered at the solver's interpolated crossing time; an MCU sees an
  input change at most one instruction late (it already ran past it when the circuit lags by a
  quantum — counted in `BenchStats::late_edges`). Multi-MCU benches sync every 1 µs. Late edges
  go through `McuCore::late_input(pin, level, at)` (default: `set_input`): a core that decodes
  inputs by timing (the API-level boards' UART receivers) keeps the edge's real time — a board's
  own TX looped back to its RX arrives as a batch of edges that are all in its past.
- **Damage & supply.** Ratings are first-order filtered (τ per rating); destruction rebuilds
  the bench with the damaged cards. MCU pins track current through their ports (> 80 mA →
  pin damaged → high-Z). USB supply has a polyfuse I²t model (hold 0.5 A, trip → 50 Ω, resets
  1 s after the load drops below 60 mA).
- **Rebuild keeping state** (`Bench::update_project`): re-netlists, re-creates the session with
  the old node voltages (`.ic` on nodes touching a capacitor, so stored charge carries over;
  `.nodeset` hints elsewhere), keeps MCUs (matched by part id), devices
  (`save_state` / `load_state`), world, instruments and services; live property edits use
  `Device::set_param` or `Session::set_param` / `set_ext` without a rebuild.
- **MCU trait.** `McuCore`: `label`, `engine`, `now`, `pin_names`, `pin_index`, `run_until(t,
  &mut dyn McuIo)`, `take_pin_events`, `pin_drive`, `set_input`, `schedule_input`,
  `late_input`, `set_analog`, `supply_adc`, `set_vcc` (brown-out), `set_aref`, `set_reset_line` (RESET / EN /
  RUN nets), `reset`, `reset_count`, `last_reset`, `in_reset`, `set_temperature`, `flash`
  (AVR image), `flash_exe` (host build), `take_serial`, `take_log`, `serial_inject`,
  `serial_tx_pin`, `set_serial_waveform`, `serial_baud`, `as_avr` / `avr_debug` (debugger).
  `McuIo` gives a core `adc_sample(pin, t)` and `service(channel, request, t)` (radio).
- **Host boards** (`bb_sim::host::HostMcu` over `bb_firmware::host::HostBoard`, §5b): pins by
  catalogue name (`D4`, `GP26`, `LED_BUILTIN`), `stop_on_output` so bit-banged protocols (DHT22)
  are answered at the right time, PWM reported as duty/frequency turned into real edges, EN /
  RUN low or Vcc < 2.3 V freezes the board (time offset tracked), release restarts the sketch.
  Radio requests go to `Service::host_service`; `bb_sim::RadioService` wraps a
  `bb_radio::RadioSpace` and serves them.
- **Thread + streams.** `SimHandle::spawn(project, library, options)` runs the bench on its own
  thread. Commands (bounded crossbeam channel): `Play`, `Pause`, `Step(dt)`, `SetSpeed`,
  `Reset`, `ResetMcu`, `Flash{part,image}`, `FlashHost{part,exe}`, `SetWorld`, `Water`,
  `SetPartParam`, `Input{part,input}`, `UpdateProject` (rebuild keeping state), `AddProbe` /
  `RemoveProbe`, `Scope{id,timebase,vdiv,trigger}`, `SerialSend`, `Debug(DebugCommand)`,
  `RepairAll`, `Shutdown` (`SimHandle::send`; streams via `serial_lines()`, `events()`,
  `audio()`).
  Out: `telemetry()` → `Arc<Telemetry>` via `ArcSwap` (≤ 60 Hz: time, run status, speed
  achieved, parts' visuals + readings, MCU states, rails / power report, probes, instruments,
  ERC/DRC summary, stats); bounded channels for serial lines, events and audio. The UI never
  blocks on the sim; slow consumers drop the oldest stream items.
- Services survive a full reset because `BenchOptions::services` holds factories
  (`Arc<dyn Fn(&Project) -> Box<dyn Service>>`), not instances. The default options hold the
  radio service (`bb_sim::radio::service_factory()`). `Service::take_device_wakes()` lets a
  service wake a device (`(part, time, token)`); `Service::take_notes()` returns serial
  annotations (`ServiceNote`), emitted as `SimEvent`s from the radio's designator and shown in
  the serial monitor of the MCU(s) sharing a signal net with it. `BenchOptions::world_models`
  adds `bb_world::WorldModel`s (world-model plugins).
- Integration of tracked element currents/energies uses the sample at the *end* of each
  interval (inputs change at sync points; capacitive nodes settle after the re-solve), and an
  MCU drive change on an analog net forces the next sync one quantum later. The analog
  session's max step is 10× the sync quantum (10 µs by default).
- Rebuilds keep pending device wake-ups (remapped to the new device list).
- `Bench::what_if_ext(name, value, nets) -> (with, changed)`: DC operating point of the frozen
  circuit with one ext variable changed (instrument loading callouts).
- API-level boards run with the whole bench at hand: `analogRead` from the solution, Wire →
  devices on the SDA net (`i2c_transfer`), SPI → devices on the SCK net (`spi_transfer`),
  Serial1/2 → real UART waveforms on their pins (TX idles high from `begin()`; the receiver
  opens at `begin()` from the line's current level — a floating or low line before it is not
  decoded — follows the configured baud between frames, and injects bytes at the stop-bit time;
  a board's own TX wired to its RX echoes; a reset closes both until `begin()` again; ESP32
  `Serial1/2` = UART1 (9/10) / UART2 (16/17), routed to any GPIO by `begin(baud, config, rx,
  tx)` — GPIO 9/10 are not on the DevKit header —, Pico `Serial1/2` = UART0 (GP0/1) / UART1
  (GP8/9)).

## 6b. Radios (`bb-radio`)

Pure state machines over bytes and `SimTime` (the LAN bridge's socket thread is the only
thread; nothing reads the wall clock). One `bb_radio::RadioSpace` per bench holds everything:

- `net: Network` — APs (default `SIM-AP` / WPA2 `breadbox` / ch 6, 16 dBm), stations, floor-plan
  `geometry` (walls) → RSSI via log-distance path loss, association + real DHCP DORA (pool from
  `.42`, lease 3600 s, renewals), DNS (`<host>.sim`, `broker.sim`, common public names alias the
  gateway), TCP (handshake, retransmission with RTO/Karn, reassembly, FIN/RST), UDP, ICMP, gateway
  services (MQTT broker `10.0.0.1:1883`, HTTP test server `:80`, NTP from sim time, echo `:7`),
  fault injector `NetConditions { latency_ms, jitter_ms, loss_pct, drop_ap_at_s, ap_down_s }`,
  `log()` → `PacketLog` (`to_pcapng`, `to_csv`), `broker_mut()` (virtual clients),
  `take_broker_events()`.
- `ble: BleSpace` — GATT peripherals + the virtual phone (`scan`, `connect(id, pin)`,
  `discover`, `read/write/subscribe(handle)`, `terminal_open/send`, `take_phone_events`).
- `lora: LoraSpace` — medium (terrain, region duty cycle, collisions/capture) + gateway → MQTT
  (`lora/<net>/<addr>/up`, downlinks on `…/down`).
- Modules: `add_esp01(label, pos, EspAtConfig)`, `add_hm10`, `add_hc05`, `add_rylr896`,
  `add_host_radio(label, pos)` (ESP32 Wi-Fi + BLE) → `DeviceId`; `power(id, on, t)`,
  `set_pin(id, ModulePin::{Reset, Enable, Key}, high, t)`, `set_position(id, pos)`.

**What bb-sim calls.**

1. Time: `advance(t)` whenever the bench reaches `t`, and include `next_wake()` in the
   scheduler's `t_target` (radio timers, packet arrivals, module latencies).
2. UART modules (`UartWire`, one per module ↔ MCU pair):
   `wire.mcu_edge(t, high)` for every MCU TX-pin level change (from `take_pin_events`, exact
   cycles); include `wire.next_decision()` in `t_target` (the stop-bit sample of the byte on the
   wire) so the module receives the byte at its exact time; then `wire.pump(&mut space, t)`;
   `space.advance(t)`; and schedule `wire.module_edges(&mut space)` on the MCU's RX pin with
   `avr.schedule_pin_input(rx, Some(level), avr.cycle_at(t))` (always ≥ now: modules answer
   ≥ 60 µs after the byte; the core's one-instruction overshoot is absorbed). The decoder follows
   the module's baud (`uart_config(id)`), including ESP-01 boot garbage at 74 880 baud and
   `AT+UART_CUR` switches. Pure helpers: `uart::{UartDecoder, UartEncoder, UartFrame,
   UartConfig}`.
3. Host boards: `HostIo::service` → `space.host_service(id, channel, request, t)`; the reply
   goes back to the child unchanged (format in `bb_radio::host`).
4. Serial monitor: `take_annotations() -> [Annotation { time, device, label, kind:
   AnnotationKind::{Wifi, Mqtt, Net, Ble, Lora}, text }]`; `line()` renders `wifi: AT+CWJAP=
   "SIM-AP" → OK rssi=-54 dBm ip=10.0.0.42` / `mqtt: PUB garden/pot1 {…} (40 B)`. Route by
   `device` to the MCU wired to that module (toggle per kind in the UI).
5. UI (`RF-7`): `cards() -> [RadioCard { title, status, rows }]` (e.g. `RSSI -54 dBm (3.2 m,
   1 wall)`, `IP 10.0.0.42 / DHCP`, `Broker mqtt://10.0.0.1 (built-in)`), `log_rows()` (Wi-Fi +
   BLE + LoRa, time ordered; `LogEntry::{time_label(), dir.label(), text()}` gives the design's
   `01.870 tx DHCP REQUEST 10.0.0.42` and amber `!` drop rows), `net.set_conditions(…)`,
   `explorer_subscribe/publish/take`, `set_lan_bridge(Some(BridgeConfig::new(host, port)))` +
   `lan_bridge_status()`, the phone API on `ble`.
6. Persistence: `settings() / apply_settings(&RadioSettings)` (serde) for `project.json`
   `radios`; module flash state (`EspAt::config.flash`) for per-part settings.
7. BLE timing: phone → peripheral packets (CCCD writes, writes, reads) go out in the master's
   slot of the next 30 ms connection event, peripheral notifications in the slave slot after it;
   `notify()` honours a CCCD write already queued for that event, so a notification sent right
   after the phone subscribes is delivered. `PhoneEvent::Subscribed` → phone log row
   "notifications on · FFE1"; `subscribe_pending(handle)` → `CharView::notify_pending` (the
   UI's "Turning on…").

Example sketches live in `examples/<project>/firmware/<MCU ref>/<sketch>/` with a `sketch.yaml`
(`default_fqbn`); `scripts/build-test-firmware.sh` compiles the AVR ones into
`examples/fixtures/<sketch>.<board>.{hex,elf}` (host-board sketches are compiled at run time).

## 6c. Radios on the bench (`bb_sim::radio`)

- `RadioService` (service `"radio"`) owns the bench's `RadioSpace`, built by
  `RadioService::for_project` from `project.radios` = `ProjectRadios { RadioSettings (flattened),
  bench_pos }` (default: `SIM-AP` at the origin, one interior wall at x = 1.5 m, bench at
  (3.2, 0) → `-54 dBm (3.2 m, 1 wall)`). `ProjectRadios::store(&mut project)` writes it back;
  `Command::Radio` keeps the engine's project in sync.
- Modules: `RadioModule` devices `attach(part, ModuleKind)` on start (the modem survives
  rebuilds); RX bytes go to `uart_rx` at their stop-bit time (framing errors dropped), modem
  frames come back through `take_frames` (and a `TOKEN_RADIO_TX` wake when the space produced
  them while stepping). EN/RST have 10 k pull-ups (unconnected = enabled); HC-05 KEY, HM-10 /
  HC-05 STATE (high while the phone is connected) are modelled.
- Host boards get a radio on their first Wi-Fi/BLE request. Extensions (`RadioExtension`, radio
  plugins) run after every radio step with `&mut RadioSpace`; `service_factory_with(setup)`
  applies radio-pack hooks.
- UI: `Telemetry.radio: Option<RadioTelemetry>` (cards, last 200 packet-log rows with
  `time_label/dir/text/dropped/proto`, conditions, APs, `has_ble`/`has_lora`, phone
  `{scan, connected, services, log, terminal}`, MQTT explorer rows, LAN bridge config/status,
  bench_pos, floor-plan `walls`, `bench_links` = each AP's `Link { distance_m, walls, rssi_dbm,
  … }` to the bench); `Command::Radio(RadioCommand)` (conditions, APs, LAN bridge, bench position,
  settings, explorer subscribe/unsubscribe/publish, clear log, `Phone(PhoneCommand)`);
  `Command::ExportPacketLog { format, reply }`; `RadioService::uart_transcript(part)` (UART spy).

## 7. Plugin runtime (`bb-plugin`)

The ABI and every import are specified in `docs/PLUGIN-SDK.md`; the manifest schema is
`docs/plugin.schema.json` (kept in sync by `bb-plugin/tests/schema.rs`).

- **Manifest** (`manifest.rs`): `Manifest { id, version, name, kind[], api, summary,
  description, provides { instrument, probes, model, view3d, view2d, panel, parts, boards[],
  world_model, generator, analysis, radio, theme, lessons }, spice { load, library }, specs,
  highlights[], permissions[], entry, assets[], min_breadbox, limits }`; `validate(root)` checks
  ids, semver, ABI, kinds vs contents, safe relative paths + existence, and the permissions the
  contents imply (`spice.load` → `inject:load`/`drive:pins`, world models → `world:*`, radio
  packs → `radio:space`, generators → `drive:pins`).
- **Registry** (`registry.rs`): bundled dir (`catalog.json`: order, preinstalled, planned),
  user dir (`~/Library/Application Support/Breadbox/plugins`, `state.json`), local folders.
  `catalog()`, `install_package(bytes, granted, allow_downgrade)`, `install_bundled`,
  `load_local`, `uninstall`, `set_enabled`, `grant`, `check_changes()` (hot reload),
  `compiled(id)`. Packages (`package.rs`): `inspect`, `unpack` (zip-slip safe, limits), `pack`.
- **Sandbox** (`sandbox.rs`): one wasmtime `Engine` (fuel + epoch interruption, 1 ms ticker
  thread); `Compiled` checks exports (`memory`, `bb_abi_version`, `bb_alloc`) and imports (`bb.*`
  only; WASI preview 1 only with `fs:project`, preopened to the project folder); `Sandbox` = one
  `Store` with `StoreLimits` (memory cap, trap on grow failure), fresh fuel + epoch deadline per
  call. During a call the host state holds a raw pointer to the caller's context
  (`DeviceCtx`, `WorldMut`, `RadioSpace`) — set before the synchronous call, cleared after.
  Traps → `PluginError::{Faulted, Violation}`; the adapter stops that instance only.
- **Adapters**: `device::WasmDevice` (parts/instruments; hooks → exports; pulls visual /
  readings / panel pushed by the guest), `world::WasmWorldModel` (`bb_world::WorldModel`),
  `radio::WasmRadioExtension` (`bb_sim::radio::RadioExtension`), `generator::part_json`
  (samples `bb_generator_eval` into a periodic `table()` B-source part),
  `analysis::run` (+ `run_netlist` behind `bb_spice_run`), `board::part_json` (declarative AVR
  variant → part with `mcu` pin mapping), `theme::Theme`.
- **Host** (`host.rs`): `PluginHost::new(&mut Registry, project_dir)` /
  `from_dirs(dirs, project_dir)` → `library()` (built-in + plugin parts; plugin art as
  `plugin://<id>/<file>`, `asset_path(url)`), `device_registry()`, `bench_options()` /
  `apply(&mut opts)` (registry, radio service with pack hooks, world models),
  `instruments()`, `panel_for_part(def)`, `themes()`, `lesson_dirs()`, `analyses()`,
  `run_analysis(id, netlist, options)`, `errors`. `loading_callout(&mut bench, part)` computes
  the instrument load's effect on the probed node (ext `rin`) from `Bench::what_if_ext`.
- **Instrument panels** (`panel.rs`): static `Panel { size, color, art, elements: [display |
  button | rotary | knob | switch | jack | led | screen | label] }` + live `PanelState {
  displays, leds, screens { traces, markers, header, cursors }, controls, readouts }`. Controls
  → `UserInput::Value { key: control id, value }`. Drawing on screens needs `ui:panel`.
- **First-party plugins** (`plugins/`, Rust → wasm via `scripts/build-plugins.sh`, `.wasm`
  committed): `community.dmm-handheld`, `breadbox.scope-4ch`, `breadbox.logic-analyzer`,
  `breadbox.bench-psu`, `breadbox.function-generator`, `breadbox.soil-weather`,
  `breadbox.lora-gateway`; SDK examples in `plugins/examples/` (template part, sawtooth
  generator, sensitivity analysis, Pro Mini board, Solarized theme).

## 7b. Lessons (`bb-lessons`)

The file format is specified in `docs/LESSON-FORMAT.md` (kept in sync by
`bb-lessons/tests/lessons.rs`); content lives in `lessons/<track>/`.

- **Format** (`format.rs`): `TrackDef`, `Lesson { id, track, order, title, summary, minutes,
  bench: BenchSpec { project | example, firmware, world, warmup }, steps }`, `Step { kind,
  title, body, formula, callouts, choice, predict, measure, table, quiz, code, checks, run,
  hints, damage_title/text, repair_label, next_label, solution: [Action], vars }`; `Check` (23
  goal types) and `Action` (21 worked-solution actions) are serde-tagged enums. Expressions
  are `bb_spice::expr`; templates (`text.rs`) format numbers the design's way.
- **Catalog** (`lib.rs`): `Catalog::builtin()` (`lessons/` from the repo or
  `Breadbox.app/Contents/Resources/lessons`, `BREADBOX_LESSONS`), `add_packs(host.lesson_dirs())`,
  `track(id)`, `lesson(id)`, `all()`, `errors`. `LoadedLesson { lesson, dir, file, pack }` →
  `start_project()`, `images(&project)` (AVR images), `host_firmware(&project, lib)` (host
  builds), `resolve(rel)`. `validate(lesson, lib)` lists authoring problems.
- **Session** (`session.rs`): the Lessons screen's state machine over a
  `bb_circuit::edit::Editor` holding the lesson circuit: `label()` ("Step 3 of 5"),
  `lesson_label(n)`, `current()`, `state()` (`StepState { choice, math, guess, verdict, hint,
  show_working, rows, quiz, firmware, passed }`), `vars(meter)`, `render(template, meter)`,
  `choose(i) → Edit`, `bypassed()`, `set_math(on)`, `meter_hidden()`, `predict(guess, meter) →
  Verdict { correct, text, expected }`, `working()`, `toggle_working()`, `record(meter)`,
  `checkpoint() → (passed, text)`, `answer_quiz(i)`, `set_firmware_loaded`, `mark`,
  `next_step()` / `prev_step()`, `is_last()`, `next_label()`, `free_play() → Project`. The
  bench is the caller's: an `Edit::SetProp` goes to `Command::SetPartParam`, `Edit::Project`
  to `Command::UpdateProject`.
- **Checks** (`check.rs`): `prepare(checks, &mut bench)` adds frequency/duty probes;
  `evaluate_step(step, project, lib, Option<&mut Bench>, state) → Vec<CheckResult { label, ok,
  detail, check }>` (friendly labels from `goal_label` unless the step's `labels` override
  them; `check` = the step's check index) covers connectivity / parts / props / ERC (bb-circuit), readings, voltages,
  element currents, LED state, damage, serial output, probes, world, MQTT and the phone
  (bb-sim, incl. plugin instruments' `readings`), and SPICE (`spice_value`: the project's Pro
  netlist with the check's analysis card), plus the goals a quiz, code step or table imply.
- **Runner** (`runner.rs`): a lesson on a headless `Bench` built with `PluginHost::
  bench_options()` (`None` for Pro lessons): `apply(&Action)`, `meter()`, `evaluate()`,
  `diagnose()`, `solve_step()`, `solve()` (the scripted solver; `transcript`), `next_step()`.
- **Applying actions** (`apply.rs`): `apply(&mut impl Target, &Action)` — the one code path
  for worked solutions. Circuit edits go through the session's `Editor`, then
  `Target::circuit_changed(live)`; everything else through `Target` (`edit`, `meter`, `run`,
  `input`, `set_world`, `flash`, `repair`, `phone_connect`, `radio`). `Runner` implements it
  on a headless bench, the app's `LessonRun` on its live `SimBench` ("Do it for me").
  `is_circuit(&Action)`, `solution_image(&Session)`.
- **Names** (`names.rs`): `Namer::new(project, lib, conn)` → `part` (`D1 (red LED)`, `R1
  (220 Ω)`, `the USB supply`), `pin` (`USB +5 V`, `R1's lead in column 5`, `D1's long leg
  (+)`, `UNO pin D13`, `the meter's mA jack`, `the top + rail`), `place` / `strip_range`
  (`column 5`, `a5–e5`), `wire` (`the red jumper from column 10 to the top − rail`),
  `socket`, `strip_holes`, `occupants`, `free_holes`; `current` / `voltage` / `hz` / `value`
  format with a floor (no `0 aA`).
- **Diagnosis** (`diagnose.rs`, "Test circuit"): `diagnose(step, project, lib, Option<&mut
  Bench>, state, &Options { running, compile_errors }) → Diagnosis { goals: [GoalReport {
  label, ok, detail, finding, same_as }], extra, passed, payoff }`; `Finding { kind: Mistake,
  what, fix, marks: [Mark { spot: Pin | Part | Wire | Control, tone: Ok | Target | Wrong |
  Remove | Focus, note }], action: Repair | Run }`. A bench scan finds loose / dead-end
  jumpers, parts not in holes, shorted / bypassed parts, reversed LEDs and shorted supplies;
  per goal, connectivity (what each side reaches, the nearest dead end, wrong pins) and live
  state (loop search from the LED to a source and to GND, LED current and health, the
  meter's dial / jacks / series-vs-parallel, probe frequency, serial output). `payoff(step,
  namer, goals)` = the step's `success` + live values. Pure: the app runs it on the engine
  thread (`with_bench`).
- **Show me** (`guide.rs`): `show_me(step, project, lib) → ShowMe { moves: [Move { text,
  marks, ghost: Ghost { a, b, color, label }, circuit, wait }], why }`, a move for every
  `Action`; `first_move` is a diagnosis's fallback next action; `control::*` names the step
  panel controls marks point at (`choice:3`, `compile`, `world:<key>`, `prop:<part>.<prop>`,
  `add:<ref>`, `publish:<topic>`, `phone:*`).
- **Audit** (`lib.rs`): `audit(&Lesson)` — hands-on steps need ≥ 2 hints, a solution and a
  `why` (built-in lessons; lesson packs only need `validate`).
- **Code steps** (`code.rs`): `fqbn_for(project, lib, mcu)`, `compile(source, fqbn) → Build {
  success, image, diagnostics, log }` (a temp sketch folder through `bb_firmware::Toolchain`);
  `Runner::upload(mcu, &image)`.
- **Progress** (`progress.rs`): `Progress::load(Progress::default_path())`, `step_done`,
  `set_current`, `record`, `complete`, `resume`, `is_done`, `done_count`, `reset`, `save`
  (atomic).
- **Editor model** (`editor.rs`): `Draft::new(title, track)` / `open(lesson, track)`,
  `capture_bench(&project)`, `add_step` / `remove_step` / `move_step` / `step_mut`,
  `add_check`, `suggest_for_part(lib, part, readings)` / `suggest_for_pins(lib, a, b, volts)` →
  `Suggestion { label, check }`, `validate`, `to_toml`, `save_pack(dir, PackInfo)` (a
  lesson-pack plugin folder), `package` (`.bbplugin` bytes).
- **Content**: 5 tracks, 24 lessons (Electricity you can see 8; Sensors & the real world,
  Microcontrollers from zero, Radios & IoT, Pro: SPICE analysis 4 each). Start circuits come
  from `cargo run -p bb-lessons --example write_benches`; AVR sketches are built into
  `firmware/*.hex` by `lessons/build-firmware.sh`.

## 8. App (`breadbox`)

**Stack.** `eframe`/`egui` 0.37 (wgpu, AccessKit, persistence), `egui_plot` 0.38, `rfd` (file
dialogs), `muda` (macOS menu bar), `image` (PNG export), `egui_kittest` 0.37 (wgpu snapshots)
for GUI tests. Fonts are bundled (`assets/fonts/`, OFL licenses next to them) and embedded with
`include_bytes!`: Inter 400/500/600, JetBrains Mono 400/500, Space Grotesk (variable, 500/600
via `FontTweak::coords`).

**Module map** (`crates/breadbox/src/`):

```
main.rs              env_logger + breadbox::run() (or `--smoke`)
lib.rs               native_options() (1440×900, min 1280×800, persisted geometry in
                     platform::app_state_file()), run(), STARTED, APP_ID
smoke.rs             `breadbox --smoke <project | example | lesson:<id>> [--place <plugin>]`
app.rs               BreadboxApp (eframe::App), AppCtx (shared services), AppOptions, PersistedState,
                     top bar (logo + wordmark → Home, breadcrumb + screen switcher, mode badge,
                     Pro tabs), shortcuts, command routing, raw_input_hook (menu → editor keys)
router.rs            Route, ProTab, ScreenSlot, Screen trait, ScreenRegistry, Command
platform.rs          resources() / app_support() / caches() (over bb_core::paths), MENUS (the
                     menu table) + NativeMenu (muda), open-documents Apple Event (`odoc`) +
                     command-line paths → queue_open/take_opened, file dialogs (rfd;
                     `pick_project`: an NSOpenPanel taking folders, `.breadbox` packages and
                     files; `BREADBOX_TEST_PICK` answers them in tests)
theme.rs             Palette DARK/LIGHT (DESIGN.md tokens), font helpers, style_for(), paper/dots grids
project.rs           Project (project.json as plain JSON: name, mode, MCUs + sketch dirs, radios),
                     examples scan, Resources (examples/fixtures location), recents
toolchain.rs         ToolchainService: background detect ("detecting toolchain… N s"; a timeout →
                     ToolchainState::Unresponsive with Retry / Install), one-click install job,
                     library list
jobs.rs              Job<T> (thread + progress + cancel + repaint on change)
bench/mod.rs         BenchLink trait + view models (the UI ↔ simulator contract, below)
bench/sim.rs         SimBench: the default BenchLink over bb_sim::SimHandle (+ Workbench API)
bench/audio.rs       AudioOut: buzzer PCM / relay clicks / pops through rodio on its own thread
bench/avr.rs         AvrBench: BenchLink over bb_avr::Avr + bb_firmware::Debugger (+ ESP-01 radio)
bench/service.rs     BenchService: owns the link, accumulates serial/debug/radio/pins/probes/events
circuit.rs           CircuitDoc: the open project's bb_circuit Editor + library + cached
                     connectivity / ERC-DRC, autosave; shared by Workbench, schematic, Pro
export.rs            Share/export: .breadbox.zip bundle + import, .cir, BOM CSV, SVG → PNG
canvas/pen.rs        View (pan/zoom, 100 % = 8 pt per 0.1"), Pen: world-unit drawing (shadows,
                     gradients, deferred glows, beziers, scaled text, text_up silkscreen)
canvas/art/          procedural 2.5D art for every library bench art id (boards, breadboard,
                     passives, modules, world objects, instruments); bounds; magic smoke
canvas/scene.rs      Scene: parts in draw order, world leads/bounds, pin + hole spatial index
canvas/bench.rs      BenchCanvas: the breadboard view (draw + every bench interaction)
canvas/schematic.rs  Sheet (symbols, mirrored bench wiring, flags, junctions), SchDraw (egui +
                     SVG backends), SchematicCanvas (Workbench + Pro → Schematic; net-label
                     tool: label_tool / label_snap onto pin anchors or schematic wires)
canvas/instruments.rs  BenchInstrument trait + InstrumentRegistry (mini scope, DMM; plugins)
canvas/world_objects.rs  world objects on the bench (WORLD-4): hand / person / wall / magnet drawn
                     at metres / 0.00254, hit-test, dragged live (SetWorld <id>.x/.y), new_object
canvas/plugin_parts.rs  plugin parts on the bench: instrument front panels with live controls,
                     SVG art (resvg → texture), probe leads (wires plugged into jacks) with
                     draggable tips, display masking (lesson meters)
plugins.rs           PluginService (AppCtx::plugins): Registry, lazily built PluginHost, the
                     plugin-aware bench factory, hot reload, theme palettes
widgets/             the kit (below)
screens/mod.rs       default_registry() — the ONE place screens are registered
screens/home.rs      Home (APP-2)
screens/placeholder.rs  generic "coming later" slot (unused by the default registry)
screens/plugins/     mod.rs (Plugins screen, PLUG-4), place.rs ("Place on bench")
screens/lessons/     mod.rs (Lessons screen + track nav + embedded bench + lesson-complete card
                     + chime), run.rs (LessonRun: Session + CircuitDoc + own SimBench + goals +
                     meter + progress + Test circuit / Show me / Do it for me player), step.rs
                     (step panel), editor.rs ("Make your own lesson", LES-6)
screens/firmware/    mod.rs (screen), build.rs (staging + compile), debug_panel.rs,
                     radio_panel.rs (RF-7 panel, reusable), libraries.rs (library manager, installer)
screens/pro/         mod.rs (Analysis tab), cards.rs (dot-card editing), model.rs (Results → view
                     data), demo.rs (design netlist + stand-in subcircuits), sync.rs (netlist
                     synced from the circuit), schematic.rs (Pro → Schematic tab), pcb/ (Pro →
                     PCB, PRO-7), plugin_analyses.rs (analysis plugins' cards)
screens/workbench/   mod.rs (screen: header + simulation menu, layout, status strip, rail),
                     palette.rs, panels.rs (World / Instruments / Probes / Waveforms),
                     world_program.rs (World panel: Objects, Program the world — scripts +
                     keyframe timelines), inspector.rs, dock.rs (Serial monitor, Probes &
                     power), share.rs (Share menu)
```

**Router.** `Route::{Home, Workbench, Firmware, Pro(ProTab::{Schematic, Analysis, Pcb}),
Plugins, Lessons}` maps to a `ScreenSlot`; `ScreenRegistry` holds one `Box<dyn Screen>` per
slot. To plug a screen in, replace its placeholder in `screens::default_registry()`:
`r.register(ScreenSlot::Workbench, Box::new(WorkbenchScreen::new()))`. The `Screen` trait:
`crumb(&AppCtx) -> Option<String>` (breadcrumb text), `header(&mut Ui, &mut AppCtx)` (top-bar
controls, right-aligned via a right-to-left layout), `ui(&mut Ui, &mut AppCtx)` (body below the
56 px bar), `command(&Command, &mut AppCtx) -> bool` (menu/shortcut commands, visible screen
first), `project_changed`, `on_enter`, `save_state`/`load_state` (JSON, persisted). Screens
navigate with `app.navigate(Route)` and queue app commands with `app.command(Command)`.

**AppCtx** (passed to every screen): `project: Option<Project>`, `resources`, `examples`,
`recent`, `toasts`, `toolchain: ToolchainService`, `bench: BenchService`, `theme`, `repaint`,
`project_generation`; `open_project`, `set_project`, `bench_link()` (creates the link for the
open project on first use).

**Commands + shortcuts** (`APP-2`, `APP-4`): ⌘N new, ⌘O open, File → Import Project Bundle
(`.breadbox.zip`), ⌘S save (examples are read-only: Save copies the project), ⌘⇧S save as,
⌘Z/⌘⇧Z undo/redo, ⌘X/⌘C/⌘V/⌘A cut/copy/paste/select all, ⌘W close project (window on Home), ⌘F
find, ⌘B verify, ⌘U upload, ⌘R run/pause, ⌘1–5 screens, ⌘⇧H Home, View → Theme (dark default,
light token set), Help → the guide / Plugin SDK / lesson format (`widgets::help`, the bundled
docs in a window). `platform::MENUS` is the one table of items (id, title, shortcut) the native
menu (muda) is built from; `command_for(id)` maps each to a `Command`. Items with key
equivalents consume the keys, so egui shortcut handling only runs without the menu (tests, other
OSes). Undo / Redo / Select All for a focused text field are re-injected as key events at the
start of the next frame (`AppCtx::text_has_focus` = egui's `text_edit_focused`: a focused
slider, button or floor-plan marker is not a text field, so ⌘Z there is the project's undo —
the Workbench, Pro Schematic and Firmware screens step `CircuitDoc` undo / redo); Cut / Copy / Paste send `ViewportCommand::RequestCut/Copy/Paste`, which
egui-winit answers with `Event::Cut/Copy/Paste` (text fields; the bench copies / cuts / pastes
the selected parts on those events). About uses the standard macOS About panel.

**Resources & user data** (`PLAT-3`): `bb_core::paths` is the one resolver — resources from
`$BREADBOX_RESOURCES`, else `Breadbox.app/Contents/Resources` when the executable sits in an app
bundle (never the checkout), else the repository (development builds; compiled out when built
with `BREADBOX_BUNDLE=1`). User data: `app_support()` = `~/Library/Application Support/Breadbox`
(`$BREADBOX_APP_SUPPORT`: plugins, `lessons/progress.json`, `toolchain/` — Breadbox's own
arduino-cli + `data/`, the private `sketchbook/` (libraries) and `staging/`, the arduino-cli
configs, `native/` avr-gcc + ctags — and `app-state.ron`),
`caches()` = `~/Library/Caches/Breadbox` (`$BREADBOX_CACHES`: `builds/`, `staging/`).
`bb_lessons::{examples_dir, lessons_dir}`, `Registry::default_bundled_dir/default_user_dir`,
`Toolchain::app_dir`, `build_cache_root`, `Progress::default_path` and the app's `Resources`
all resolve through it (their older specific env overrides still win). Bundled examples open
read-only; Save / Save As copy them into `~/Documents/Breadbox`. Documents handed over by
macOS (Finder "Open With", the Dock, `open -a`) or given on the command line are queued by
`platform` and opened by the app on its next frame.

**Simulation menu** (`SIM-1`, `SIM-3`): the header's speed label opens speed, Step one quantum /
1 ms (`AppCtx::step_sim` → pause + `bb_sim::Command::Step`), Reset bench, Reset MCU only · Ux
(`AppCtx::reset_mcu` → `ResetScope::Mcu`), Seed and Sync quantum (`AppCtx::apply_sim_settings`:
undoable `Editor::set_settings` (`Op::Settings`) → `UpdateProject` + `Reset`, so the same seed
replays the same run). The native Simulate menu sends `Command::StepSim(StepSize)` /
`Command::ResetMcu`; File → Import .cir and Home → Import .cir… send `Command::ImportCir`, and
`open_path` on a `.cir/.net/.sp/.spice/.ckt` file imports it as a new Pro project
(`export::import_cir_project`, original kept as `imported/<name>.cir`).

**Alerts** (`PLUG-2`): `BenchService::poll` collects device / plugin error-level logs (a sandbox
"permission denied" trap) and bench errors (not damage) into `alerts`; the app turns each into
an error toast once, on any screen. `events` (for the Firmware output) is bounded at 500.

**World programs** (`WORLD-4/5`): objects, scripts and timelines are edited in
`project.world` (undoable `set_world`); `AppCtx::sync_world_program` (each frame) pushes a
changed set into the running bench (`World::set_objects` / `set_scripts` / `set_timelines` via
`Command::With`), so panel edits, canvas drags and undo / redo all reach the sensors. The
palette's Hand tile adds a world hand object instead of a part. Scheduled watering is
`PotConfig::schedule` (live keys `<pot>.water_every_h`, `<pot>.water_ml`).

**Measurement** (`WORLD-6`): `WorldConfig::noise` ("Sensor noise & ADC quantization") picks
realistic or ideal measurement. Sensors read their noise through `WorldView::noise_enabled`;
`bb_sim::bench::adc_model_for(noise)` gives every AVR `AdcModel::realistic()` or
`AdcModel::Ideal` when it is built, on `set_world("world.noise", …)` and on every world step
(`Bench::sync_adc_models`), so scripts, timelines and undo reach the ADC too. Probes and meters
always show the continuous voltage; firmware reads codes. The World panel switch's hover text
words it with `panels::measurement_text`, the Inspector with `inspector::measurement_row` ("ADC" on AVR
boards, "Noise" on sensors).

**Persistence** (`APP-5`): eframe storage key `breadbox-app` → `PersistedState { route, theme,
recent, last_project, screens: {slot key → JSON} }`; window geometry by eframe; the file is
`~/Library/Application Support/Breadbox/app-state.ron` (copied once from the old
`dev.breadbox.Breadbox/app.ron`). `Screen::on_enter` / `on_leave` bracket a screen's visibility
(the Lessons screen pauses its own bench behind other screens). Projects
autosave edited sketch files 1.2 s after the last keystroke (bundled examples stay read-only).
`Screen::flush` + `BreadboxApp::flush` (called from eframe's `save`, i.e. on quit) write what the
debounce has not yet saved (Firmware tabs, a Pro file netlist, the circuit); `set_project`
saves the outgoing circuit and Firmware / Pro flush the previous project's edits in
`project_changed`.
Idle cost: nothing repaints unless input, a job, a toast or the running bench asks.

**Widget kit** (`widgets/`, all with AccessKit names): `Btn` (ghost / amber / mini / plain /
danger, icon, selected, disabled), `segmented` (`.seg`) and `mini_segmented` (`.mini`),
`part_tile` (`.tile`, drag source), `tag` / `kind_tag` / `filter_chip` / `status_dot` /
`legend_swatch`, `section_header(_row)`, `StopSlider` (World-panel slider with named stops +
mono value), `checkbox`, `tab_strip` / `tab` (amber underline), `rail_button` (`.rb`),
`collapsible_side` + `panel_title_with_collapse` (`<|` / `|>` handles, `.tab-out`),
`card_frame` / `inner_card_frame` / `panel_frame`, `data_table`, `Toasts`, `modal`,
`progress` / `spinner`, `select` + `menu_item`, `child_ui`, `wrapped_text`, `kv_grid`,
`icons::Icon` (the design's SVG paths parsed and stroked; part, tool and UI glyphs; never
sparkles), `paint_logo`; `CodeEditor` + `CodeDoc` + `EditorView` (gutter, accessible
"Breakpoint line N" targets, executing-line band, cursor line, Arduino C++ / SPICE
highlighting in `highlight.rs`, diagnostics squiggles + hover, find bar, `editor_tabs`);
`plot::{LinePlot (dual Y, log axes, cursors, markers), histogram_plot, bode_plot,
traces_to_csv, traces_to_svg, export_png, auto_cursors, fmt_si}`. `docs/screenshots/widgets.png`
shows them all.

**BenchLink** — the contract the `ui-bench` agent implements over `bb_sim::SimHandle`
(`bench/mod.rs`; `AvrBench` is the reference implementation):

```rust
pub trait BenchLink: Send {
    fn describe(&self) -> String;
    fn targets(&self) -> Vec<BenchTarget>;            // MCUs: id "U1", label, board, fqbn, emulation, sketch_dir
    fn upload(&mut self, target: &str, fw: &Firmware) -> Result<UploadReport, BenchError>;
    fn set_running(&mut self, running: bool);
    fn is_running(&self) -> bool;
    fn reset(&mut self, scope: ResetScope);           // Bench | Mcu(id)
    fn set_speed(&mut self, speed: SimSpeed);         // RealTime(k) | Max
    fn set_breakpoints(&mut self, target: &str, bps: &[BreakpointSpec]); // file (tab), line, enabled, hit_count
    fn debug(&mut self, target: &str, cmd: DebugCommand); // Continue/Pause/StepOver/StepInto/StepOut/StepInstruction
    fn send_serial(&mut self, target: &str, data: &[u8]);
    fn radio(&mut self, cmd: RadioCommand);           // SetConditions / SetLanBridge / SetBenchPos / Mqtt* / Phone
    fn poll(&mut self) -> BenchUpdate;                // never blocks
    fn watch_memory(&mut self, target: &str) {}       // Memory / I/O view visible: keep its snapshot fresh
}
pub struct BenchUpdate { running, sim_time, serial: Vec<SerialLine>, debug: Vec<DebugViewModel>,
                         radio: Option<RadioPanelModel>, pins: Vec<(String, Vec<PinView>)>,
                         probes: Vec<(String, f64)>, events: Vec<BenchEvent> }
```

Rules: methods return quickly (the bench runs on its own thread); the link calls the
`BenchSpec::repaint` callback when new data exists (≤ 30 Hz while running, only on change when
idle); `poll` drains serial lines / events and returns the latest view models. View models:
`DebugViewModel { state: NoFirmware|Running|Paused{reason}|Halted, location: SourcePos { file
(tab name), line }, function, call_stack: [FrameView], locals/globals: [VarView], registers:
RegistersView (r0–31, SREG, SP, PC, cycle), breakpoints: [BreakpointView { line, resolved_line,
hits, enabled }], debug_build, memory: Option<MemoryView> }` — `MemoryView { mcu, sram_start,
sram, eeprom, flash (Arc buffers; flash cached per `flash_gen`), flash_used, io: [IoRegView {
name, addr, peripheral, value, prev }], prev_sram, prev_eeprom (the previous pause), sp, x, y,
z, cycle }` is taken on the engine thread by `bench::memory_snapshot(&Avr, known_flash_gen)` at
every pause and ~4×/s while running when the UI calls `watch_memory`; `bench::MemoryHistory`
(per link) fills the flash cache and the previous-pause comparison base. API-level boards have
none. `RadioPanelModel { cards: [RadioCardView { label, title, status, tone, kind, rows }], log:
[PacketRow { time, dir, text, dropped }], conditions: NetConditionsView, ble: Option<BleView>,
mqtt, placement: Option<PlacementView { bench: (x, y) m, aps: [ApPlacement { ssid, channel, x,
y, distance_m, walls, rssi_dbm }], walls: [WallView] }> }` with
`RadioPanelModel::from_space(&RadioSpace, limit)` / `bench::placement_from_space`;
`RadioCommand::SetBenchPos { x, y }` moves the bench (every radio on it); `SerialLine { target, time, text, kind: Normal|System|Net|Warn|Error }`; `BenchEvent::
{Flashed(UploadReport), BreakpointArmed, Paused, Note, Error}`; `probes` keys match the Pro
`.op` table (`V(A0)`, `I(D1) avg`, `P(R2)`) for its "bench probe" column. Factories:
`BenchFactory = Arc<dyn Fn(&BenchSpec) -> Box<dyn BenchLink>>` with `BenchSpec { project_dir,
targets, radios, repaint, speed }`; register the sim-backed one with
`AppOptions::bench_factory` or `app.ctx.bench.set_factory(..)`.

**SimBench** (`bench/sim.rs`, the default `BenchFactory`): spawns `SimHandle` for the open
circuit (`BenchSpec::circuit`, the in-memory `CircuitDoc` project) with prebuilt firmware
(`examples/fixtures/<sketch>.<board>.elf` or the project's `firmware.image`) so Run works at
once; API-level boards are host-compiled in the background and the bench is rebuilt with their
executables. `upload` sends `Flash` (AVR) or `FlashHost` (ESP32 / Pico), then a
`Command::With` closure attaches a `bb_firmware::Debugger` (ELF) and syncs breakpoints on the
engine thread; debug view models are built by more closures and come back over a channel;
pauses are detected from `Telemetry::mcus[].paused`. `poll` also turns telemetry into the
Pro bench-probe keys (`V(A1)`, `I(D1) avg`, `P(R2)`), `RadioTelemetry` into the Radios panel
(+ virtual phone), device events into `BenchEvent`s and plays audio. Extra API for screens:
`telemetry()`, `send(bb_sim::Command)`, `update_project` (electrical edits → `UpdateProject`,
probe edits → `AddProbe`/`RemoveProbe`, instrument edits → `Instruments::set_instruments` +
rebuild; a board added/removed respawns), `adopt_project` (live `SetPartParam` edits),
`with_bench(closure)` (lessons, tests), `export_packet_log`. A watcher thread repaints ≤ 30 Hz
while the bench changes and stays quiet when idle. Screens reach it with
`app.sim()` / `app.bench.sim()` (`BenchLink::as_sim`).

**CircuitDoc** (`AppCtx::circuit`): every circuit edit goes through `doc.editor`
(`bb_circuit::edit::Editor`, undoable) followed by `doc.touched()` (autosave timer +
schematic auto-placement of new parts). `AppCtx::sync_circuit` (end of every frame) pushes new
revisions to the bench and autosaves user projects 1.2 s after the last edit (examples stay
in memory; Save As writes the edited copy).

**Workbench canvas contracts.** `BenchCanvas::show(ui, rect, doc, telemetry, generation) →
BenchOutput { sim: Vec<Command>, selection_changed, live_edit }` (inputs and live parameters
the screen forwards to the bench). Coordinates are bb-circuit bench units (0.1"); the view is
stored in egui temp data under `Id::unique("bench-view")` (tests map pins to screen points).
Palette tiles are egui DnD sources with `PartDrop(def_id)`; dropping snaps the first lead to
the grid (`scene::snap_drop`). **Instrument hook:** `canvas::instruments::BenchInstrument`
(`kind`, `view(def, telemetry) → InstrumentView`, optional `draw` to replace the part art with
a front panel, `lead_pin`, `lead_color`, `tip_label`) registered in
`BenchCanvas::instruments` (`InstrumentRegistry::register`); the mini scope and the DMM
placeholder use it. Plugin panels (`PartTelemetry::panel`, `bb_plugin::panel`) plug in there.
**Embedding the bench** (lesson canvas): a screen owns a `BenchCanvas` and calls
`show(ui, rect, app.circuit, telemetry, app.project_generation)`, forwarding `BenchOutput::sim`
to `app.sim()`; `SchematicCanvas::show` works the same way (its view + `Arc<Sheet>` are in temp
data under `schematic-view` / `schematic-sheet`; `screens::pro::schematic::net_label_control`
is the reusable "Net label" toolbar button). Bench art text must stay inside the bundled fonts
(JetBrains Mono / Inter / Hack / Ubuntu): `egui_kittest` panics on a missing glyph, so symbols
like ⏚ / ⎓ are drawn as shapes or spelled out.

`AvrBench` (kept for tests and boards-only use): one AVR target (UNO/Nano/Mega), fixed ADC voltages
(soil 42 % → 1.88 V on A0, 180 lux → 2.60 V on A1), and — if the project has an `esp01` part —
a `bb_radio::RadioSpace` ESP-01 on SoftwareSerial D2 (TX) / D3 (RX) via `UartWire`, exactly
like the garden-node example. Upload without a toolchain falls back to the project's prebuilt
image / `examples/fixtures/<sketch>.<board>.elf`.

**Firmware screen** (design screen 2): Verify stages the editor buffers into
`~/Library/Caches/Breadbox/staging/<hash>-<release|debug>/<sketch>/` under an exclusive
`StagingLock` (`staging/<hash>-<kind>/.lock`, held for stage + compile, so another build of the
same sketch — a debug build, a second app instance — never rewrites the sources mid-compile) and runs `Toolchain::compile` (AVR) or
`host::compile_host` (ESP32/Pico) on a job; `BuildOutput::output_lines()` fill the Output tab;
`Diagnostic`s become editor squiggles. "Step · breakpoints" builds `-Og` and arms the gutter's
breakpoints via `BenchLink::set_breakpoints`; pauses follow the executing line across tabs.
Bottom tabs: Output · Serial monitor · Debug (300 px tall; `debug_panel::debug_tab`, view
selector "Debug view" = Variables · Memory · I/O registers): Variables = call stack, locals +
globals, registers + SREG; Memory = "Memory region" SRAM · Flash · EEPROM hex dump (16 B/row
+ ASCII, virtualised `show_rows`, rows labelled `SRAM 0x0100: 00 …`), SP / X / Y / Z outlined in
SRAM ("Pointers" buttons jump there), bytes changed since the previous pause amber, "Go to
address" (`0x08f0`, `SP`, `Z`) + "Go"; I/O registers = groups per peripheral in balanced
columns, rows `PORTB 0x25 0x20 0010 0000` (changed bits amber), "Filter registers". The
selected target is watched (`watch_memory`) while either view is open; ESP32 / Pico get a
"needs the cycle-accurate AVR core" empty state. The right column is the Radios panel
(`radio_panel::radio_panel → RadioPanelOutput { commands, export, edits }`): cards, a
"Placement" section ("Floor plan" canvas: APs, walls, draggable "Bench position x m, y m" marker, arrows nudge 0.5 m;
readout `SIM-AP · 3.2 m · 1 wall · -54 dBm`; "Walls (n)" rows: "Add wall" — 4 m interior across
the AP → bench line —, a per-wall "Wall n: interior / concrete" toggle (−15 / −25 dB) and
"Remove wall n"), Packet log with "Export
pcap-ng…" / "Export CSV…" (`SimBench::export_packet_log`, save dialog or
`BREADBOX_EXPORT_DIR`, toast with the path), Network conditions (Latency, Jitter 0–200 ms,
Packet loss, "Drop the AP at t =" + "Drop the AP at (s)" field, LAN bridge) and the Bluetooth
LE card, whose "Add BLE hat" runs `radio_panel::add_ble_hat` (HM-10 right of the first board,
VCC→5V / 3V3 on host boards, GND, TXD→D10 / RXD→D11 — ESP32 RX2/TX2, Pico GP1/GP0 — one
undoable "Add BLE hat" transaction) and starts the bench so the phone scans `HMSoft`.
**Radio settings are the project's** (`project.radios` = `bb_sim::radio::ProjectRadios`): the
panel shows them (`RadioPanelState::sync_from_project`, on open and after undo / redo; no saved
settings = what the bench runs, 3 ms ± 1, no loss); a dropped bench, a released slider (live
`SetConditions` while dragging), the drop-AP field / checkbox, the LAN bridge and the walls
become `RadioEdit::{BenchPos, Conditions, LanBridge, Walls}` → `radio_panel::apply_radio_edit`
→ one undoable `Editor::set_radios(label, value)` (`Op::Radios { before, after }`, not
electrical: "Move the bench", "Network conditions", "LAN bridge", "Walls");
`SimBench::update_project` pushes what changed to the running bench (`SetConditions`,
`SetLanBridge`, `SetBenchPos`, `ApplySettings` for walls / APs).

**Pro → Analysis** (design screen 3): for a project with a circuit the netlist is **synced
from the schematic** (`screens/pro/sync.rs`: bb-circuit's Pro netlist with world values as
`.param`s and the MCU's outputs replayed from the live bench; a circuit without analysis cards
gets a `.op`); edits that change an R / C / L value round-trip into the circuit, analysis-card
edits from the sidebar or the editor (`.tran …`, `*off .mc …`) round-trip into
`project.spice.directives` (`Editor::set_directives`, undoable), anything else detaches it
(`project.spice.detached`, Re-sync button); the `.op` table's bench-probe column reads `SimBench`'s probe keys.
A project with an empty circuit edits its own file netlist `analyses/<name>.cir`
(`project::starter_netlist`, created with new Pro projects) instead of the built-in demo, and
that netlist **builds the circuit** while the circuit is netlist-only
(`spice_io::is_netlist_circuit`: resistors, capacitors, inductors, V / I sources, generic SPICE
cards, ground symbols, no bench wiring; label "autosaved · builds the schematic"): each edit that
parses becomes, after a 700 ms pause, `spice_io::netlist_to_circuit(text, project, lib)` (the
`.cir` import machinery on top of the project: parts, net labels / GND symbols at the pin
anchors, directives; parts still there keep their id + schematic placement; the round trip is
checked — every typed element in the circuit's Pro netlist with the same nodes) committed as
one `Editor::replace_circuit("Netlist", …)` transaction (diffs parts, wires, schematic wires +
labels, probes, directives). Half-typed text waits; `SpiceIoError::Unrepresentable` (e.g. a
subcircuit instance with more than the generic card's 16 pins) marks the netlist "detached
from the schematic" until the next edit or Re-sync (which rewrites it from the circuit); circuit
edits made elsewhere (Schematic tab, Inspector, undo there) rewrite the text; a board joining
the circuit ends it. `.cir` import maps V / I sources to library `vsource` / `isource` (value
text kept) and R / C / L to library parts only in the plain `R1 a b 1k` form (anything with
`tc1=` / `IC=` / `m=` stays a SPICE card). All
seven analysis cards are in the (scrollable) sidebar; `.meas` and `.tf` results fill a
measurements table (`model::MeasRow`), and the Bode card reads / marks the −3 dB corner
(`model::corner_frequency`). The
netlist editor (SPICE highlighting, parse errors as
diagnostics, autosave for `analyses/*.cir`) feeds `model::run` on a job (`run_with` + cancel +
progress). Breadbox-only parts the analog solver can't elaborate (`CAPSOIL`, `ATMEGA328P`) get
stand-in subcircuits from `demo::STUB_PARTS` and their file-name parameters are stripped; in
co-simulation bb-sim provides the real ones. Cards in the sidebar are dot-card lines; disabling
comments them out as `*off .ac …`. Export `.cir` is `Netlist::to_spice_text()` with stand-ins
inlined (ngspice-runnable); each plot exports CSV / PNG / SVG.

**Plugins in the app** (`plugins.rs`, `PLUG-4`/`PLUG-7`). `AppCtx::plugins: PluginService`
opens the registry (`AppOptions::plugin_dir` or `~/Library/Application Support/Breadbox/
plugins`, bundled first-party plugins) and builds the `PluginHost` on first use
(`host()`/`library()`); `bench_factory()` is the app's default `BenchFactory` (`SimBench::with`
the plugin library + an options hook adding plugin device factories, radio packs and world
models). Anything that changes the active set calls `changed()`; `AppCtx::apply_plugins`
(every frame) then hands the new library to the open `CircuitDoc` (plugin parts appear in the
palette) and respawns the bench, and polls local plugins for hot reload (toast "Reloaded …").
Theme plugins: `theme::palette_with_tokens` + `theme::set_plugin_palette` override `pal()`.
Lesson packs feed the Lessons catalog (`host.lesson_dirs()`); analysis plugins get cards in
Pro → Analysis (`plugin_analyses.rs`). Registering the host also registers its instrument
panels and part art with `canvas::plugin_parts` (`register`).

**Instrument panels** (`widgets/instrument.rs`): `Face::fit/exact` map a `Panel` (mm) into a
screen rect; `paint` draws the body, displays (LCD / 7-seg / VFD with annunciators, unit and
bar graph), buttons, the rotary dial (labels on the ring, current position amber), knobs,
switches, LEDs, jacks and screens (graticule, traces, markers, cursors, header); `controls`
lays AccessKit widgets over the face (`DMM1 HOLD`, `DMM1 dial`, `DMM1 dial Ω`, `DMM1 display:
1.882 V AUTO DC`) and returns `(control id, value)` pairs for `Command::Input`. `⎓` (missing
from every bundled font) is drawn as the DC sign (`ptext`). Plugin instrument parts put their
leads on the panel's jacks (`plugins/*/parts.json` lead positions = jack centres × 0.12 grid
per mm), so a wire into a jack starts at the jack. On the bench (`canvas/plugin_parts.rs`)
wires with exactly one end on a jack are drawn as probe leads (red / black from the jack ring)
whose free tip (`<REF> VΩ probe tip`) drags onto any pin or stays dangling (`Endpoint::Free`).
"Place on bench" (`screens/plugins/place.rs`) adds the part with its leads in one undoable
transaction; a meter takes over the built-in DMM placeholder's probes (and designator), a
scope copies the mini scope's channels, otherwise the leads dangle.

**Lessons** (`screens/lessons/`): the screen loads `bb_lessons::Catalog::builtin()` + packs and
`Progress` (`AppCtx::progress_path`); `LessonRun::open` builds a `Session`, a read-only
`CircuitDoc` of the lesson circuit (shown by an embedded `BenchCanvas` / `SchematicCanvas` for
Pro lessons) and its own `SimBench` (plugin library + hook + `loaded.images`), sends the world
overrides and an MQTT `#` subscription, and runs. Choices go to the session, the doc and the
bench (`SetPartParam`); student edits on the canvas reach the bench (`update_project`) and the
session; goals are evaluated on the engine thread every 0.5 s (`check::evaluate_step` via
`with_bench`) and gate Next; the step meter (`measure` / `predict.meter` × scale) is masked
`----` while `meter_hidden()`, settled readings fill "Your measurements"; damage shows the
break-it card + Repair (`RepairAll` + default choice); code steps compile on a job
(`code::compile`) and `Flash`; "Open in free play" writes the bench as a Playground project
under `default_projects_dir()` and opens it. The lesson editor saves a lesson-pack plugin into
the user plugin folder (it is active at once) and opens its first lesson.

Feedback (`run.rs`, `step.rs`, `canvas/lesson_overlay.rs`): **Test circuit** runs
`bb_lessons::diagnose` on the engine thread and keeps the `Diagnosis` until the circuit's
generation changes; the step panel shows each open goal's finding under it. Hints escalate
(`hints_shown`) to **Show me** (`guide::show_me`) and **Do it for me**: a `Player` replays the
solution through `bb_lessons::apply` (`LessonRun: Target`), waiting on sim time for `run`,
committing consecutive circuit edits with `Editor::replace_circuit("Do it for me")` (one undo
step; the screen handles ⌘Z / ⇧⌘Z on the lesson doc). `LessonRun::overlay()` →
`BenchCanvas::overlay` (marks, ghost jumpers, the success pulse, net hints while dragging a
wire); `Spot::Control` marks ring step-panel widgets. A **Controls** card gives the solution's
non-canvas actions a widget (world sliders, property fields, Add part, phone, MQTT publish)
and Serial / MQTT / Phone boxes show what the step's goals listen to. `track_success` ticks
goals, computes the payoff, pulses the parts and queues the chime (`AudioMsg::Chime` through
a lazily opened `AudioOut`, `LessonsState::sound`); Next becomes the solid primary and tells
what's left when disabled; finishing a lesson shows its completion card with "Next lesson".

**Radio tools** (`widgets/radio_tools.rs`, `RF-6`/`RF-7`): the virtual phone (scan + RSSI,
connect, GATT read / notify / write, BLE serial terminal, log) and the MQTT explorer (wildcard
subscriptions, live messages, publish) are floating windows drawn by the app every frame from
the open bench's `Telemetry.radio`, sending `RadioCommand`s; `radio_tools::open` / `buttons` /
`rail_buttons` (Radios panel, Workbench rail).

**PCB** (`screens/pro/pcb`, model in `bb_circuit::pcb`, `PRO-7`): footprints come from the
parts' footprint names (pitch, drill, shape parsed from KiCad-style names; boards / modules
from their bench leads × 2.54 mm); `PartInstance::pcb` = placement in mm (90° rotations),
`Project::pcb` (`PcbLayout`) = outline `(x0, y0, x1, y1)` + traces `{ id, layer: top|bottom,
width, points }` + vias `{ id, x, y, drill, diameter }` (JSON values, `pcb::Board::load/store`).
`pcb::Copper::build` turns pads / trace segments / vias into capsules joined by union-find
(pins bonded inside a part count as joined) → `ratsnest()`, `unrouted()`; `pcb::drc::check`
(clearance 0.2 mm between copper groups on a shared layer, shorts, width ≥ 0.2, drill ≥ 0.3,
edge 0.3, unrouted) → `summary()` (`DRC clean`); `pcb::gerber::export` writes F/B copper, F/B
mask, F silk, Edge.Cuts (RS-274X, 4.6 mm, Y up from the board's bottom-left) + Excellon
drill, and `gerber::parse` / `parse_excellon` read them back. The screen edits through the
shared `CircuitDoc` (own undo stack), auto-places unplaced footprints, routes (pad → corners →
pad, V = via + layer switch), and exports into a folder (`BREADBOX_EXPORT_DIR/gerbers` in tests);
Share → "PCB Gerbers + drill (.zip)" zips the same files (`pro::pcb::gerber_zip`).

**Tests** (`TEST-5`): `cargo test -p breadbox` — unit tests in modules, `tests/gui.rs` (the app
driven by AccessKit labels through `egui_kittest` + wgpu; renders 1440×900 screenshots into
the test scratch dir, or refreshes `docs/screenshots/*.png` with `BREADBOX_UPDATE_SCREENSHOTS=1`), `tests/widgets.rs` (widget behaviour + `tests/snapshots/widget-gallery.png`
snapshot; `UPDATE_SNAPSHOTS=1` to refresh), `tests/workbench.rs` (the Workbench on the real
co-simulation: run + serial, World slider, palette drag-and-drop, wire drawing + ratsnest,
inspector edits, undo/redo, LED burnout, upload on the live bench, ESP32 host board, virtual
phone, Pro synced netlist + bench probes, Share + reopen, schematic net labels, every library
part on one bench (art QA, zoomed tiles in `target/…/tmp/parts-qa/`); `--ignored perf`
measures frame time with garden-node running), `tests/plugins.rs` (catalog → permission
prompt → install, Handheld DMM on garden-node A0 ≈ 1.88 V, probe tips dragged across a resistor,
dial → Ω; local plugin + hot reload, theme, SDK docs, analysis plugin), `tests/lessons.rs`
(lesson 3 step 3 math mode + hints + working + measurements, break it + Repair, free play;
the lesson editor → pack → Lessons), `tests/radio_tools.rs` (phone ↔ ble-button notify, MQTT
explorer `garden/pot1`), `tests/debug_memory.rs` (blink: SRAM / I/O / EEPROM / flash views
across two pauses, PORTB bit 5, Go to address; API-level empty state;
`docs/screenshots/firmware-memory.png`), `tests/radio_panel.rs` (garden-node bench drag →
lower RSSI + stored position, jitter / drop-AP, pcap-ng + CSV export; Add BLE hat → HMSoft;
`docs/screenshots/radios-placement.png`), `tests/pcb.rs` (blink: auto-place, route, DRC, Gerbers parse back), `tests/audit.rs` (spec
items closed by the audit: simulation menu, `.cir` import, plugin-violation toasts, flush on
quit / switch, Pro cards + measurements, world objects / scripts / timelines on a sonar sketch,
scheduled watering, radio annotations), `tests/session.rs` (a scripted first session: Playground
LED blink from a typed sketch, Pro RC low-pass corner, Track 1 lesson 1; `session-*.png`), `tests/menu.rs` (every
`platform::MENUS` item through its command on the real app), `tests/glyphs.rs` (every character
in UI strings, part / lesson / plugin / example data and the rendered docs is drawable by the
bundled fonts; `widgets::instrument::SHAPED_SYMBOLS` — `⎓ ∿ ⊓ ⋀ ⎍` — are painted as shapes),
`tests/toolchain_first_run.rs` (no toolchain: the install offer + an offline failure;
`--ignored`: the real one-click install into scratch folders, then Verify). `BREADBOX_EXPORT_DIR` / `BREADBOX_PROJECTS_DIR` redirect Share and
bundle imports in tests. GUI tests use `AppOptions::test()` (no persistence,
no native menu, bench at max speed); compile tests skip gracefully without a toolchain.

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
  `arduino-cli core install arduino:avr`), `ngspice` (golden-data regeneration and the e2e
  check that every example's `.cir` export runs in ngspice), a C++ compiler (ESP32 / Pico host
  boards). Every test degrades gracefully without them (prebuilt fixtures, bb-spice-only checks,
  skipped host boards).
- `scripts/test-all.sh` is the single green-light command (TEST-7): fmt (workspace + plugins),
  clippy `-D warnings`, `cargo test --workspace` (a test over 60 s fails the step), the plugin
  wasm workspace (clippy for wasm32, `scripts/build-plugins.sh --check`: the committed `.wasm`
  files rebuild byte-identically — builds remap `$HOME`/the plugins folder so modules carry no
  machine paths), and the release perf gates (TEST-6: `cargo test --release --workspace --test
  perf -- --ignored` for MCU-3 / SIM-4 / SPICE-5, the Workbench frame-time test, and
  `breadbox --smoke`), and on macOS the `bundle` stage (`scripts/bundle-macos.sh --check`:
  `dist/Breadbox.app` + `scripts/check-bundle.sh`, ~3–6 min). `--quick` skips the perf gates
  and the bundle; `SKIP=perf,plugins,bundle` skips steps; logs land in
  `$CARGO_TARGET_DIR/test-all-logs/`.
- **Smoke mode** (`breadbox --smoke <project | example> [--seconds N] [--speed max|<x>]
  [--headless] [--warmup S]`, `crates/breadbox/src/smoke.rs`): the real app opens the project on
  the Workbench and runs its bench with the GUI drawing at 60 Hz (window, or egui frames without
  one when there is no display / `BREADBOX_HEADLESS=1`), waits for API-level boards to finish
  compiling (`SimBench::host_build_pending`), then prints `SMOKE project=… first_frame_ms=…
  realtime=… frame_ms_avg=… status=ok` and exits non-zero on a bench error, damage, ERC errors or
  a timeout. It never writes the user's saved state. PLAT-4 = `first_frame_ms` (process start →
  first frame), SIM-4 with the GUI = `realtime`.
- **Headless e2e** (`tests/e2e`, crate `breadbox-e2e`, TEST-4): scenarios drive the stack through
  the app's entry points — examples from `examples/`, `bb_sim::SimHandle` (commands + telemetry +
  streams), `bb_plugin::{Registry, PluginHost}`, `bb_lessons` sessions + checkers,
  `bb_firmware::Toolchain` (fixtures without it), the app crate's project / export / Pro sync
  code. `breadbox_e2e::Sim` makes the threaded engine deterministic: every run is a
  `Command::Step` at maximum speed and the driver waits (fence + a snapshot published after it)
  until the engine paused at exactly the requested time, so commands land at exact simulated
  times and nothing asserts on the wall clock; `Sim::with` runs a closure on the engine thread.
  Scenario files: `garden_node`, `boards`, `damage`, `sim_control`, `lessons`, `projects`,
  `examples`, `plugins`, `firmware`. GUI flows for the Plugins / Lessons screens belong in
  `crates/breadbox/tests/` next to `gui.rs` / `workbench.rs` (same harness helpers).
- `scripts/build-plugins.sh [--check]` rebuilds the plugin wasm with the checkout path and
  `$HOME` remapped, so the committed modules are byte-identical from any clone or worktree.
- `scripts/bundle-macos.sh [--dmg] [--check]` → `dist/Breadbox.app` (+ `dist/Breadbox.dmg`):
  release build with `BREADBOX_BUNDLE=1` and remapped paths in its own target dir
  (`target/bundle-macos`), `Info.plist` (`app.breadbox.Breadbox`, version from the workspace,
  `CFBundleVersion` = build number `git rev-list --count HEAD` so a newer build's declarations
  win in LaunchServices,
  macOS 14+, high-DPI, dark-mode friendly; document types: `.breadbox` project folders are a
  **document package** — exported UTI `app.breadbox.project` conforming to `com.apple.package`,
  Breadbox owner + editor with `LSTypeIsPackage`, so Finder shows one document and a
  double-click opens it — `.breadbox.zip` bundles (import) and any folder as Alternate), `Breadbox.icns` from `assets/icon/breadbox-icon.svg` (the
  `app_icon` example + `iconutil`), `Contents/Resources` = `examples/`, `lessons/`, `plugins/`
  (+ `crates/bb-plugin-sdk` so the plugin sources build), `docs/` (README, PLUGIN-SDK,
  LESSON-FORMAT, schema), `fonts/` + OFL, `hostcore/` (reference copy; the app compiles the
  embedded one), `licenses/THIRD-PARTY.md`, `build-info.json`; ad-hoc `codesign`.
- `scripts/check-bundle.sh [app] [--no-window]`: signature / plist / resources / no `$HOME`
  path in the binary or bundled files, then smoke runs of the copied app under `sandbox-exec`
  with the checkout denied (garden-node, a lesson bench, the DMM plugin placed on a bench, an
  ESP32 host build, a real window), the real app via `open` (first frame < 2 s), a project
  folder handed over with `open -a`, and — with the copy registered in LaunchServices — a
  `.breadbox` folder is a package (`NSWorkspace.isFilePackageAtPath`) whose default app is
  Breadbox and which a plain `open` (a double-click) opens.
