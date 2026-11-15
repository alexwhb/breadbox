# Breadbox Plugin SDK

A Breadbox plugin is a folder (or a `.bbplugin` zip of one) with a `plugin.json` manifest, an
optional WebAssembly module and its data. This document is the contract: the manifest, the
permissions, the sandbox, and the **plugin ABI 1** precisely enough to write plugins in any
language that compiles to core WebAssembly (Rust, C, Zig, AssemblyScript, TinyGo…).

- Manifest JSON Schema: [`docs/plugin.schema.json`](plugin.schema.json)
- Rust guest SDK: `crates/bb-plugin-sdk` (examples in `plugins/examples/`, the template is
  `plugins/examples/template`)
- Host implementation: `crates/bb-plugin` (`sandbox.rs` is the reference for every import)
- First-party plugins (good references): `plugins/dmm-handheld`, `scope-4ch`, `logic-analyzer`,
  `bench-psu`, `function-generator`, `soil-weather`, `lora-gateway`

## 1. Quick start (Rust)

```sh
cp -r plugins/examples/template my-plugin       # edit plugin.json: id, name
cd my-plugin && cargo build --release --target wasm32-unknown-unknown
cp target/wasm32-unknown-unknown/release/*.wasm blinker.wasm
```

In Breadbox: **Plugins → Load local plugin…** and pick the folder. Local plugins hot-reload
when any file in the folder changes. To share it, zip the folder (`plugin.json` at the root or
in a single top folder) and rename it `name.bbplugin`.

```rust
use bb_plugin_sdk::prelude::*;

struct Blinker { out: usize, on: bool }

impl Part for Blinker {
    fn init(cfg: &Config) -> Result<Self, String> {
        Ok(Blinker { out: cfg.pin("OUT").ok_or("no OUT pin")?, on: false })
    }
    fn on_start(&mut self, ctx: &Ctx) { ctx.tick_every(0.25); }
    fn on_tick(&mut self, ctx: &Ctx) {
        self.on = !self.on;
        ctx.set_drive(self.out, Drive::Thevenin { v: if self.on { 5.0 } else { 0.0 }, r: 30.0 });
    }
}

export_part!(Blinker);
```

`Cargo.toml`: `crate-type = ["cdylib"]`, dependency `bb-plugin-sdk`. Build with
`--target wasm32-unknown-unknown` (release; `opt-level = "s"`, `lto = true`,
`panic = "abort"` keep modules around 100–200 KB). The SDK is `#![no_std]` + `alloc`; `no_std`
plugins enable its `global-allocator` feature.

## 2. The manifest (`plugin.json`)

```json
{
  "id": "community.dmm-handheld",
  "version": "1.4.0",
  "name": "Handheld DMM",
  "kind": ["instrument"],
  "summary": "6000-count handheld multimeter that loads the circuit like the real thing.",
  "provides": {
    "instrument": "Handheld DMM",
    "probes": 2,
    "model": "./dmm.wasm",
    "view3d": "./dmm.bench.svg",
    "view2d": "./dmm.symbol.svg",
    "parts": "./parts.json",
    "panel": "./panel.json"
  },
  "spice": { "load": "R 10Meg ∥ C 100p" },
  "specs": { "counts": 6000, "dcv_ranges": ["600m", "6", "60", "600"],
             "dcv_accuracy": "0.05% + 1", "autorange_ms": 350 },
  "highlights": [{ "label": "Display", "value": "6000 counts" }],
  "permissions": ["read:nets", "inject:load"]
}
```

| key | |
|---|---|
| `id` | lowercase reverse-DNS style with at least one `.` (`community.dmm-handheld`) |
| `version` | semver; installing an older version over a newer one is refused |
| `kind[]` | `instrument`, `part`, `board`, `radio`, `world-model`, `generator`, `analysis`, `theme`, `lesson-pack` |
| `api` | plugin ABI, `1` (default) |
| `name`, `summary`, `description`, `author`, `license`, `category` | catalog / detail page |
| `provides.model` (or `entry`) | the `.wasm` module |
| `provides.parts` | part definitions in the bb-circuit schema (ARCHITECTURE §3b) — a list or `{"parts": […]}` |
| `provides.panel` | instrument front panel (§6) |
| `provides.view3d` / `view2d` | bench art (svg/png) / schematic symbol (svg) |
| `provides.instrument`, `probes` | instrument name, lead count or names |
| `provides.boards[]` | declarative AVR board variants (§7.4) |
| `provides.world_model` | `{ name, replaces: ["evaporation"], vars: […] }` |
| `provides.generator` | `{ part, name, period, points, r_out }` (§7.5) |
| `provides.analysis` | `{ name, description, options }` (§7.6) |
| `provides.radio` | `{ lora, gateway, extension }` (§7.7) |
| `provides.theme` | theme file (§7.8) |
| `provides.lessons` | folder of lesson TOML files (`docs/LESSON-FORMAT.md`) |
| `spice.load` | the load it puts on the circuit, for people (needs `inject:load`, or `drive:pins` for a source's output impedance) |
| `specs` | free-form values the model receives in `bb_init` |
| `highlights[]` | the detail page's specs grid (`{label, value}`) |
| `permissions[]` | §3 |
| `assets[]` | other files shipped with it |
| `min_breadbox` | oldest compatible app version |
| `limits` | `{ fuel_per_call, memory_mb }` (clamped to the host caps) |

Every path is relative to the plugin folder; `..` and absolute paths are rejected. Validation
errors are reported on the Plugins screen with the offending field.

## 3. Permissions

Declared permissions are shown in the install prompt; the user must approve all of them. Every
host import checks one; a call without it **stops the plugin** with
`permission denied: plugin X tried to … without declaring Y` (the bench keeps running).

| permission | grants |
|---|---|
| `read:nets` | `bb_pin_voltage`, `bb_pin_current`, `bb_element_*`, `bb_track_element`, `bb_watch`, `bb_capture`, `bb_samples` |
| `inject:load` | `bb_set_ext` (changing the part's own SPICE elements); SPICE elements in an instrument's parts; `spice.load` |
| `drive:pins` | `bb_set_drive`, `bb_set_ext`; generator sources |
| `world:read` | `bb_world_get`, `bb_world_secs` |
| `world:write` | `bb_world_set`, `bb_world_water` |
| `radio:space` | `bb_mqtt_*`; radio module parts (`rylr896`, `esp01`, …); radio extensions |
| `ui:panel` | `bb_set_panel` with screen traces/markers (displays, LEDs and readouts need nothing) |
| `fs:project` | WASI preview 1, with only the project folder preopened (as `.`) |
| `net:lan` | `bb_lan_bridge` (the MQTT bridge to the real LAN) |

## 4. The sandbox

- **Engine**: wasmtime, core WebAssembly (no component model), one store per plugin instance
  (per part on the bench, per world model, per analysis run).
- **Fuel**: every call starts with `fuel_per_call` (default 50 M ≈ instructions, max 2 G). Out of
  fuel → the plugin is stopped ("used up its CPU budget…").
- **Epoch deadline**: a 1 ms ticker; a call longer than 250 ms of wall time is stopped (also
  catches slow host calls in loops).
- **Memory**: linear memory capped at `memory_mb` (default 32 MiB, max 256), one memory,
  ≤ 4 tables / 65 536 elements; growing past the cap traps.
- **Imports**: only module `bb` (§5) and, with `fs:project`, `wasi_snapshot_preview1`. Unknown
  imports are rejected at load with the list of available functions.
- **Failure**: a trap marks the instance stopped; its part shows `plugin stopped`, the reason is
  logged once, other parts and the simulation continue.

## 5. ABI 1

Conventions: little-endian wasm32. **Strings and buffers** are `(ptr: i32, len: i32)` pairs in
the plugin's linear memory, UTF-8 for text, JSON for structured data. **Times** are `i64`
picoseconds of simulated time unless the name ends in `_s` (`f64` seconds). Pins are `i32`
indices into the part's pin list (the order of `pins` in its part definition, also given in
`bb_init`). `f64` results are `NaN` when there is nothing to read.

### 5.1 Exports (guest → host)

Always required:

| export | signature | |
|---|---|---|
| `memory` | memory | linear memory |
| `bb_abi_version` | `() -> i32` | must return `1` |
| `bb_alloc` | `(len: i32) -> i32` | returns a buffer of `len` bytes the host writes into; the next call that receives `(ptr, len)` owns it (the Rust SDK frees it) |

Optional, by role (absent exports are simply not called):

| export | signature | role |
|---|---|---|
| `bb_init` | `(ptr, len) -> i32` | all: config JSON (§5.3); return 0, or non-zero after `bb_error(msg)` |
| `bb_on_start` | `()` | part: bench started (set drives / watches / ticks here) |
| `bb_on_wake` | `(token: i64)` | part: a `bb_wake_at` is due |
| `bb_on_edge` | `(pin: i32, high: i32)` | part: a watched pin crossed a threshold |
| `bb_on_tick` | `()` | part: periodic tick (`bb_tick_every`) |
| `bb_on_input` | `(ptr, len)` | part: user input JSON (§5.4) |
| `bb_set_param` | `(key_ptr, key_len, value: f64) -> i32` | part: live property change; 1 = applied, 0 = rebuild |
| `bb_i2c_transfer` | `(addr: i32, ptr, len, read: i32) -> i32` | part: transaction-level I²C from an API-level board; write `len` bytes then reply `read` bytes with `bb_result`; return −1 = NACK |
| `bb_world_step` | `(dt_world_s: f64)` | world model |
| `bb_generator_eval` | `(t: f64) -> f64` | generator: volts at `t` s into the period |
| `bb_analysis_run` | `(ptr, len) -> i32` | analysis: input JSON `{netlist, options}`; result JSON via `bb_result`; 0 = ok |
| `bb_radio_step` | `(now_s: f64) -> f64` | radio extension: returns the next wake (s) or −1 |

Hooks run synchronously at an exact simulated instant (`bb_now`). Drive changes take effect at
that instant; reads inside the same call see the circuit as it was solved before the call.

### 5.2 Imports (host → guest, module `bb`)

| import | signature | permission | |
|---|---|---|---|
| `bb_log` | `(level: i32, ptr, len)` | — | 0 debug, 1 info, 2 warn, 3 error |
| `bb_error` | `(ptr, len)` | — | message for a failing `bb_init` / `bb_analysis_run` |
| `bb_now` | `() -> i64` | — | simulated time (ps) |
| `bb_now_s` | `() -> f64` | — | simulated time (s) |
| `bb_rng` | `() -> f64` | — | deterministic random in [0, 1) (seeded per instance) |
| `bb_pin_voltage` | `(pin) -> f64` | read:nets | volts vs ground |
| `bb_pin_current` | `(pin) -> f64` | read:nets | current into the part through its drive |
| `bb_element_current` | `(ptr, len) -> f64` | read:nets | current of one of the part's SPICE elements by local name (`"R:in"`) into its first terminal |
| `bb_track_element` | `(ptr, len)` | read:nets | start integrating an element (call in `bb_on_start`) |
| `bb_element_charge` | `(ptr, len) -> f64` | read:nets | ∫ i dt since tracking began (C); Δcharge/Δt = exact average current |
| `bb_element_energy` | `(ptr, len) -> f64` | read:nets | ∫ v·i dt (J); for a resistor R, ΔE·R/Δt = mean v² (true RMS) |
| `bb_watch` | `(pin, lo: f64, hi: f64)` | read:nets | Schmitt watch → `bb_on_edge` (replaces the pin's previous watch) |
| `bb_capture` | `(pin, period_ps: i64)` | read:nets | record the pin's voltage at least every `period` (0 stops); digital nets record every change |
| `bb_samples` | `(pin, from_ps: i64, to_ps: i64, out: i32, max: i32) -> i32` | read:nets | writes up to `max` `(t_s: f64, v: f64)` pairs (16 bytes each, newest kept); returns the count |
| `bb_set_drive` | `(pin, kind: i32, v: f64, r: f64)` | drive:pins | kind 0 high-Z, 1 Thevenin (v behind r Ω), 2 pull (v through r Ω), 3 current (v = amps into the node) |
| `bb_set_ext` | `(ptr, len, value: f64)` | inject:load or drive:pins | set `$(ext:name)` of the part's SPICE template (no rebuild) |
| `bb_wake_at` | `(t_ps: i64, token: i64)` | — | `bb_on_wake(token)` at `t` (≥ now) |
| `bb_cancel_wake` | `(token: i64)` | — | cancel pending wakes with `token` |
| `bb_tick_every` | `(period_ps: i64)` | — | `bb_on_tick` every period (0 stops; ≥ 1 µs) |
| `bb_world_get` | `(ptr, len) -> f64` | world:read | world variable (`air.temp_c`, `pot1.vwc`, `light.ambient_lux`, `world.hour`, …) |
| `bb_world_secs` | `() -> f64` | world:read | world clock (s, time-warped) |
| `bb_world_set` | `(ptr, len, value: f64)` | world:write | set a world variable (`pot1.vwc`, user vars like `weather.rain_mm_h`) |
| `bb_world_water` | `(ptr, len, ml: f64)` | world:write | water a pot |
| `bb_emit` | `(ptr, len)` | — | a `bb_core::DeviceEvent` as JSON (§5.5) |
| `bb_set_visual` | `(ptr, len)` | — | the part's look (`bb_core::Look` / `VisualState` JSON, or `{"text": "…"}`) |
| `bb_set_readings` | `(ptr, len)` | — | live values: `[["name", value], …]` or `{"name": value}` |
| `bb_set_panel` | `(ptr, len)` | ui:panel for screens | instrument panel state (§6.2) |
| `bb_result` | `(ptr, len)` | — | return value of the current export |
| `bb_spice_run` | `(ptr, len) -> i32` | — | run a SPICE netlist (every analysis in it, 20 s limit); result JSON (§5.6) goes to the inbox; returns its length (negative = error JSON) |
| `bb_inbox_read` | `(out, max) -> i32` | — | copy the inbox (from `bb_spice_run` / `bb_mqtt_poll`) to `out`; returns bytes copied, or −needed if `max` is too small |
| `bb_mqtt_subscribe` | `(ptr, len) -> i32` | radio:space | subscribe the plugin's broker client (radio extensions) |
| `bb_mqtt_publish` | `(tptr, tlen, pptr, plen, qos, retain) -> i32` | radio:space | publish to the built-in broker |
| `bb_mqtt_poll` | `() -> i32` | radio:space | next message as `{"topic", "payload"}` in the inbox; returns its length, 0 = none |
| `bb_lan_bridge` | `(ptr, len) -> i32` | net:lan | `"host:port"` starts the LAN bridge, `""` stops it |

Imports that need a part context (`bb_pin_*`, `bb_set_*`, `bb_wake_*`, …) trap when called
from `bb_init` or from a non-part role.

### 5.3 `bb_init` config

```json
{
  "plugin": "community.dmm-handheld", "version": "1.4.0",
  "role": "device",                               // device | world-model | analysis | generator | radio
  "part": {                                       // devices only
    "id": "dmm1", "reference": "DMM1", "def": "dmm_handheld", "behaviour": "community.dmm-handheld",
    "pins": ["VO", "COM", "A", "MA"],             // pin index = position here
    "props": { "mode": {"Text": "dc_v"} },        // effective properties ({"Num": x} / {"Text": s} / {"Bool": b})
    "pos": { "x": 0.8, "y": 0.1 }
  },
  "specs": { … },                                 // the manifest's specs
  "world": { … },                                 // world models: the project's world settings
  "permissions": ["read:nets", "inject:load"]
}
```

### 5.4 Input JSON (`bb_on_input`)

`{"kind": "press", "on": true}` · `{"kind": "toggle", "index": 0}` ·
`{"kind": "switch", "index": 0, "on": true}` · `{"kind": "position", "value": 0.25}` ·
`{"kind": "value", "key": "dial", "value": 2}` — panel controls arrive as `value` with the
control id as `key` (rotary/switch: position index; knob: value; button: 1, momentary 1/0).

### 5.5 Events (`bb_emit`)

Externally tagged `bb_core::DeviceEvent`:
`{"Log": {"level": "Info", "text": "…"}}` · `{"Annotation": {"channel": "mqtt", "text": "…"}}`
(serial-monitor line) · `{"Stress": {"reason": "…"}}` · `{"Damage": {"reason": "…"}}` ·
`{"Sound": {"Beep": {"freq_hz": 2730, "secs": 0.1}}}` · `{"Sound": "Click"}` · `{"Sound": "Pop"}`.

### 5.6 `bb_spice_run` result

```json
{ "plots": [ { "kind": "op", "run": "", "vectors": [ { "name": "v(out)", "values": [7.5] } ] },
             { "kind": "ac", "run": "", "vectors": [ { "name": "v(out)", "values": [...], "phase_deg": [...] } ] } ],
  "measures": [ ["tdelay", 1.2e-3] ],
  "error": null }
```

Scale vectors (`time`, `frequency`, sweep) come first. AC vectors give magnitude in `values`
and phase in `phase_deg`.

### 5.7 Writing a plugin in C / Zig / AssemblyScript

The Rust SDK is a thin layer over §5.1–5.2; any language works if it exports the functions
above with these exact core-wasm types. A minimal C part:

```c
// clang --target=wasm32 -nostdlib -O2 -Wl,--no-entry -Wl,--export-dynamic -o part.wasm part.c
__attribute__((import_module("bb"), import_name("bb_tick_every"))) void bb_tick_every(long long);
__attribute__((import_module("bb"), import_name("bb_set_drive")))
void bb_set_drive(int pin, int kind, double v, double r);

static unsigned char heap[65536]; static int top;
int bb_abi_version(void) { return 1; }
int bb_alloc(int n) { int p = (int)(heap + top); top = (top + n + 7) & ~7; if (top > 60000) top = 0; return p; }
int bb_init(int ptr, int len) { return 0; }      /* parse the JSON if you need pins/props */
static int on;
void bb_on_start(void) { bb_tick_every(250000000000LL); }      /* 0.25 s */
void bb_on_tick(void) { on = !on; bb_set_drive(2, 1, on ? 5.0 : 0.0, 30.0); }
```

AssemblyScript: declare imports with `@external("bb", "bb_set_drive")` and export the hooks
from the entry file; pass `--exportRuntime` off, use `memory.data` for buffers. Zig:
`extern "bb" fn bb_set_drive(pin: i32, kind: i32, v: f64, r: f64) void;` and
`export fn bb_on_tick() void {…}` with `-target wasm32-freestanding -dynamic -rdynamic`.

## 6. Instrument panels

### 6.1 Layout (`provides.panel`)

Millimetres on the instrument face, origin top-left:

```json
{ "size": [80, 160], "color": "#F2C14E", "art": "dmm.bench.svg",
  "elements": [
    {"type": "display", "id": "lcd", "rect": [8, 10, 64, 34], "digits": 4, "style": "lcd",
     "units": ["mV", "V", "Ω"], "annunciators": ["AUTO", "HOLD"], "bar": 31},
    {"type": "button", "id": "hold", "rect": [8, 50, 18, 8], "label": "HOLD", "momentary": false},
    {"type": "rotary", "id": "dial", "rect": [15, 64, 50, 50], "default": 2,
     "positions": [{"value": "off", "label": "OFF", "angle": -135}, …]},
    {"type": "knob", "id": "timebase", "rect": [...], "label": "s/div", "min": 1e-7, "max": 10, "log": true, "unit": "s"},
    {"type": "switch", "id": "coupling1", "rect": [...], "label": "Coupling", "options": ["DC", "AC", "GND"]},
    {"type": "jack", "id": "j_vo", "rect": [60, 132, 12, 12], "pin": "VO", "label": "VΩ", "color": "#D9453B"},
    {"type": "led", "id": "beep", "rect": [...], "label": "·)))", "color": "#F7C77A"},
    {"type": "screen", "id": "screen", "rect": [...], "kind": "scope", "grid": [10, 8],
     "channels": [{"id": "CH1", "label": "CH1", "color": "#F7C77A"}]},
    {"type": "label", "rect": [...], "text": "…"}
  ] }
```

Ids are unique; jacks name pins of the instrument's part (probes are wires from those pins).

### 6.2 Live state (`bb_set_panel`, shown as `PartTelemetry.panel`)

```json
{ "displays": {"lcd": {"text": "1.880", "unit": "V", "annunciators": ["AUTO", "DC"], "bar": 0.31}},
  "leds": {"beep": 0.0},
  "screens": {"screen": {"header": "CH1 2 V/div · 1 ms/div · Trig'd",
     "traces": [{"channel": "CH1", "points": [[0, -1.2], [0.5, 1.1]], "digital": false}],
     "markers": [{"x": 5, "y": 1.25, "text": "T", "channel": "CH1"}], "cursors": [2, 8]}},
  "controls": {"dial": 2},
  "readouts": [["Freq", "490.2 Hz"], ["Duty", "47.4 %"]] }
```

Screen coordinates are divisions: x 0…grid x, y −grid y/2…grid y/2 (logic screens: one row per
channel).

## 7. Plugin kinds

### 7.1 `part`
`provides.parts` adds parts with the bb-circuit schema (same validator as built-ins; ids may not
collide with built-in parts). `model.behaviour`: `"wasm"` (or the plugin id) runs the plugin's
model as the part's device; a built-in behaviour id reuses it (radio modules need
`radio:space`); none = a pure SPICE part. Art in the plugin folder becomes
`plugin://<id>/<file>`.

### 7.2 `instrument`
A part (with jacks as pins) + model + panel. Its input impedance is SPICE elements of its
template between its probe pins, set live with `bb_set_ext` (`inject:load`); call the input
resistor's ext `rin` and the Plugins screen shows the live loading callout ("Probing A0 (J1 out):
meter load shifts the node by −0.4 mV…"). Use `bb_track_element` + charge/energy for exact
averages and true RMS, `bb_capture`/`bb_samples` for waveforms, `bb_watch` for edges.

### 7.3 `world-model`
`bb_world_step(dt_world_s)` runs after the built-in world models every world step (10 ms of sim
time; `dt` includes time-warp). `replaces: ["evaporation"]` switches the pots' built-in
evaporation off (`<pot>.evaporation_scale = 0`).

### 7.4 `board`
Declarative ATmega variants: `base` (`uno`/`nano`/`mega`: core + pin map), header `pins` (I/O
names must exist on the base: `D2`, `A3`; `GND.2` style ids for duplicates), `vcc_pin`, `RAW`
gets a 5 V regulator, `RST` a 10 k pull-up, optional `rows` and `art`.

### 7.5 `generator`
`bb_generator_eval(t)` is sampled over one `period` (`points` samples) when the plugin loads and
becomes a source part (`OUT`, `COM`, `gain` / `offset` / `frequency` properties) whose
behavioural SPICE source plays the table back exactly. Needs `drive:pins`.

### 7.6 `analysis`
`bb_analysis_run({netlist, options})` → `bb_result` with
`{"summary": "…", "plots": [{"title", "x_label", "x_log", "x": [...], "series": [{"name", "unit", "values"}]}], "table": [[header…], [row…]], "text": "…"}`.
Call `bb_spice_run` as often as needed (it runs on the analysis thread).

### 7.7 `radio`
`provides.radio`: `lora: true` enables the LoRa medium + gateway, `gateway` applies
`bb_radio::lora::Gateway` settings, `extension: true` runs `bb_radio_step` next to the modems
with MQTT access (`radio:space`). Module parts reuse the built-in `rylr896`/`esp01`/`hm10`/`hc05`
behaviours.

### 7.8 `theme`
`{"name": "…", "base": "dark" | "light", "tokens": {"bg": "#RRGGBB", "accent": "#RRGGBBAA", …}}`
over the token set in `docs/DESIGN.md`.

### 7.9 `lesson-pack`
`provides.lessons` = folder of lesson TOML files (`docs/LESSON-FORMAT.md`). The lesson editor
saves lesson packs in this form.

## 8. Packaging, install, hot reload

- `.bbplugin` = zip of the folder (`plugin.json` at the root or in one top folder; ≤ 4 000
  files, ≤ 128 MB; no absolute paths or `..`).
- Install copies it to `~/Library/Application Support/Breadbox/plugins/<id>/` after the user
  approves the permissions; `state.json` there keeps installed / enabled / granted.
- First-party plugins ship inside the app (`Contents/Resources/plugins`, `plugins/` in the repo)
  with `catalog.json` (card order, preinstalled, planned).
- Local folders ("Load local plugin…") run in place and hot-reload on any file change.
