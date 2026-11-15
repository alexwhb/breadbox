# Progress

Build plan: at most two agents work at once, each owning disjoint crates. The lead (orchestrator)
reviews and integrates between phases. Agents update **only their own section** below.

## Phases

| phase | agent A | agent B |
|---|---|---|
| 1 | `spice` — bb-spice (SPICE-*) ✅ | `mcu` — bb-avr + bb-firmware AVR path (MCU-1..5, 7) ✅ |
| 2 | `sim` — bb-core device contract, bb-circuit, bb-world, bb-sim, parts library (PART-*, SIM-*, WORLD-*) | `radio` — bb-radio (RF-1..6, 8), example sketches, ESP32/Pico host boards (MCU-6), native-toolchain installer ✅ |
| 3 | `plugins` (after `sim`) — bb-plugin, bb-plugin-sdk, first-party plugins (PLUG-*), bb-lessons engine + content (LES-1,3,4), radio/host-board ↔ sim glue | `ui-core` — app shell, theme, widget kit, Home, Firmware screen (editor/compile/libraries/toolchain), Pro Analysis tab, kittest harness |
| 4 | `ui-bench` — Workbench (breadboard + schematic canvases, parts palette, world/inspector/probes, serial dock), sim wiring for firmware upload + debugger + radio panel | `ui-plug-les` — Plugins screen, Lessons screen + lesson editor (LES-2,5,6), instruments on bench, PCB (PRO-7) |
| 5 | `e2e` — headless + GUI e2e suites, perf, fixes (TEST-*) | `qa` — independent spec audit + fixes, bundle-macos, docs |

## Ownership (current phase)

- bb-core: lead (frozen except additive changes; the `sim` agent owns it in phase 2)

## Requests (cross-crate asks)

- (lead) Toolchain installer must work on Apple Silicon without Rosetta (avr-gcc 7.3.0 + ctags are x86_64-only) → assigned to `radio` in phase 2.

- ~~(sim → plugins/radio glue) Replace the `esp01` / `hm10` / `hc05` factories …~~ **done** (plugins): `bb_sim::devices::RadioModule` bridges ESP-01 / HM-10 / HC-05 / RYLR896 pins to the real modems; `BenchOptions::default().services` holds `bb_sim::radio::service_factory()` (RadioService from `project.radios`); host boards get a radio on first use. See `### plugins`.
- ~~(sim → plugins) Wasm parts plug in as `DeviceRegistry::register` + `Library::add_json` …~~ **done** (plugins): `bb_plugin::PluginHost` does exactly that; plugin instruments load the circuit through their part's SPICE template (ext-controlled elements, `inject:load`), `Bench::add_load` stays for non-part probe leads.

- (audit → bundle) Toolchain detection runs `arduino-cli core list` with no timeout. An
  `arduino-cli` started from a `Breadbox.app` launch was found hung for 15+ min in `open()` (most
  likely a macOS privacy prompt for `~/Documents/Arduino`, which arduino-cli touches as its
  sketchbook), so detection never settles. The first-run toolchain flow should time out these
  calls (and/or point arduino-cli's `directories.user` away from `~/Documents`).

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
  makes 1 µs quanta cheap: 490 Hz PWM into RC + LED for 1 s = **31–38 ms (26–32× real time)**; static
  circuits cost nothing per advance; settled circuits stop stepping.
- `SPICE-8` **done** — `Netlist::to_spice_text()` / `Session::to_spice_text()` (ngspice runs
  the output; ports → V+R/I, ext() → .param; Breadbox-only cards such as `.step`, `.mc`,
  multi-`.temp`, `.dc` over a `.param` are written as `*!bb <card>` — a comment for ngspice,
  read back by bb-spice), CSV (mag/phase, dB/phase, re/im), ASCII rawfile write + read.
- Tests: 47 unit (parser, expressions incl. AD gradients, LU, waveforms, every model's DC
  Jacobian vs finite differences and AC vs transient charge Jacobian, temperature formulas),
  23 analysis (convergence aids + diagnostics, Schmitt/latch, MC statistics with fixed seed,
  .step/.dc/.meas, export round trip + `*!bb` directives, design netlist, noise
  contributions), 48 reference, 16 session, 2 robustness (2 940 netlist mutants + hostile
  inputs, no panics), 7 doctests;
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
- For `scripts/test-all.sh` (not created yet): `cargo test -p bb-spice` (unit + 47-circuit
  reference suite + session + robustness) and `cargo test -p bb-spice --release --test perf --
  --ignored` (TEST-6 / SPICE-5 / SPICE-7 budgets).
- Debug helpers: `cargo run -p bb-spice --example spice -- file.cir`, `--example refdiff --
  <ref> <vector> <level>`, `--example dcdiff -- <ref>`, `--example export_cir -- file.cir`.

### radio
Phase 2 — `bb-radio`, example firmware, ESP32/Pico host emulation, native toolchain installer.
Everything below runs in `cargo test -p bb-radio -p bb-firmware -p bb-avr` (clippy `-D warnings`
+ fmt clean). The end-to-end tests run **real compiled firmware**: AVR sketches built by
arduino-cli/avr-gcc (fixtures in `examples/fixtures/`), ESP32/Pico sketches built by clang++
against `hostcore/` at test time.

- `RF-1` **done** — APs (`SIM-AP`/`breadbox`/ch 6), stations, WPA2 password check, scan,
  log-distance path loss + walls → RSSI (default bench reads the design's `-54 dBm (3.2 m, 1
  wall)`), sensitivity limit, weak-signal loss.
- `RF-2` **done** — real DHCP DORA + renewals (pool from 10.0.0.42, lease 3600 s), DNS (`*.sim`,
  `broker.sim`, station hostnames), TCP (handshake, RTO/Karn retransmission, reassembly,
  FIN/RST, station↔station + listeners), UDP, ICMP ping, NTP from sim time, HTTP test server
  (httpbin-style routes), echo; MQTT 3.1.1 broker on 10.0.0.1:1883 parsing the firmware's bytes
  (QoS 0/1 + inbound QoS 2 handshake, retained, `+`/`#`, keep-alive ×1.5, LWT, persistent
  sessions + queued QoS 1, takeover, auth, virtual clients).
- `RF-3` **done** — latency ± jitter, loss %, "drop the AP at t" (stations disconnect, links die,
  broker notices via keep-alive → will; auto-reconnect when the AP returns); LAN bridge
  (`set_lan_bridge`) — blocking I/O on its own thread, channels only, loop prevention, reconnect
  with back-off; tested against a loopback TCP MQTT broker + a "dashboard" client.
- `RF-4` **done** — packet log rows (time, tx/rx/!, radio, protocol-aware summary: `DHCP REQUEST
  10.0.0.42`, `MQTT CONNECT client=garden-node`, `PUBLISH garden/pot1 · 40 B · QoS 0`, `dropped
  by fault injector (loss 2%)`), pcap-ng with synthetic Ethernet/IPv4/TCP/UDP/ICMP frames
  (checksums valid), CSV; BLE/LoRa rows; merged `log_rows()`.
- `RF-5` **done** — ESP-01 ESP-AT modem: ROM boot log at 74 880 baud then `ready`, echo,
  `busy p...`/`busy s...`, every listed command (+ CIPSERVER, CIPDOMAIN, CWAUTOCONN, UART_DEF,
  MQTTCONNCFG/MQTTUNSUB/MQTTCLEAN), `+IPD`, `>`/`Recv n bytes`/`SEND OK` on ACK, UDP, SNTP,
  ESP-AT MQTT client (`+MQTTSUBRECV`), flash settings (default baud, saved AP + auto-connect).
- `RF-6` **done** — HM-10 (HMSoft AT without line endings, NOTI, BAUD on reset, FFE0/FFE1
  transparent UART, 20-byte notifications), HC-05 (KEY → AT mode at 38400, SPP data mode, PIN),
  GATT model, virtual phone (scan + RSSI, connect, discover, read/write/notify, terminal for
  HM-10 / Nordic UART / SPP); ESP32 native BLE via `MCU-6` appears to the same phone.
- `RF-7` **API ready** (UI is phase 4): `cards()`, `log_rows()`, conditions, BLE phone API,
  `explorer_subscribe/publish/take`, bridge status, `settings()/apply_settings()` (serde).
- `RF-8` **done** — structured `Annotation { device, kind, text }` (`wifi: AT+CWJAP="SIM-AP" →
  OK rssi=-54 dBm ip=10.0.0.42`, `mqtt: PUB garden/pot1 {"m":41,"lux":180,"t":21} (40 B)`,
  ble/lora/net lines), filterable by kind.
- LoRa — medium (terrain classes, Semtech airtime, EU868 duty cycle, half duplex, SNR
  sensitivity per SF, collisions + 6 dB capture), RYLR896 AT modem, multi-SF gateway ↔ MQTT.
- `bb_radio::uart` — `UartDecoder`/`UartEncoder` (any format, framing/parity errors, glitch
  rejection, `next_decision()` for exact co-sim stops) + `UartWire` (module ↔ MCU pins).
- `MCU-6` **done** (engine) — `hostcore/` Arduino core (see ARCHITECTURE §5b), `compile_host`
  (clang++, prototypes, `#line`-mapped diagnostics, cached core), `HostBoard` lock-step runner
  (sleeps, interrupts, pin waits, busy-wait progress, async BLE polling, restart, NVS),
  `HostBoardSpec` for ESP32 DevKit v1 + Pico. 12 s of esp32-weather sim time ≈ 0.2 s wall.
- `PRJ-2` **sketches done** — garden-node, blink, night-light, thermometer (TMP36 + 16×2 LCD),
  ble-button (HM-10), servo-sweep, esp32-weather (ESP32 + DHT22 + PubSubClient), plus pico-blink;
  555-astable / rc-filter-bode / op-amp-amplifier are circuit-only (no firmware; bb-circuit
  projects). project.json files come with bb-circuit.
- `PRJ-3` **firmware done** — garden-node (`garden-node.ino`, `wifi_at.h`, `mqtt_lite.h`, lines
  1–31 as in the design editor; `Serial.printf` via a tiny `PrintfSerial` wrapper) compiles with
  avr-gcc (10,288 B), joins SIM-AP, gets 10.0.0.42, publishes `{"m":…,"lux":…,"t":…}` (t from the
  ATmega328P's internal temperature sensor) once per second (long-run rate), prints
  `soil_raw=… moisture=…% light_raw=… led_pwm=…` every ~0.5 s tracking A0/A1.
- `MCU-4` installer — `Toolchain::install` on Rosetta-less Apple Silicon now installs avr-gcc@8
  via Homebrew and builds Arduino ctags from source automatically (progress + clear errors);
  Rosetta path unchanged.
- Tests: bb-radio 55 (21 unit + net 10, espat 6, garden_node 3, ble 4, lora 3, bridge 2, esp32 2,
  examples 4); bb-firmware +10 (host 3, native_install 2 + 1 ignored network build, catalogue +
  prototype unit tests); bb-avr unchanged (76).

Deviations / notes:
- Pins follow the design's schematic, not the brief: garden-node `SoftwareSerial esp(3, 2)` =
  RX D3 ← ESP TX, TX D2 → ESP RX (schematic labels "D2 SW_TX", "D3 SW_RX").
- garden-node's loop is `delay(500)` + work, so publishes come every ~1.1 s with `every()`
  catching up (long-run 1/s); the boot banner line of the design's serial log is not printed
  (it would shift the editor's line numbers; line 30 is still the `Serial.printf`).
- Pico `analogRead` defaults to 12 bits as SPEC says (arduino-pico defaults to 10).
- ESP-AT strings follow ESP-AT 2.2 with v1.7 forms where Arduino libraries parse them
  (`+CWJAP:n` + `FAIL`, `+CWLAP:(…)`); `SEND OK` waits for the TCP ACK (NonOS behaviour).
- TCP has no window/congestion control; a FIN closes both directions. LoRa EU duty cycle is
  enforced per transmission (T_off = T_air·(100/d − 1)); the RYLR896 reports it as `+ERR=10`.
- Host boards: one process per board; no TLS (`https://` URLs fail), HTTP bodies need
  Content-Length or connection close; I²C master only; FreeRTOS tasks not emulated.
- §3 of ARCHITECTURE mentions a `UartPins` helper in bb-sim — `bb_radio::UartWire` provides it
  (documented in §6b).

### sim
Phase 2 — `bb-core` (device contract), `bb-circuit`, `bb-world`, `bb-sim`, parts library,
example projects. All MCU scenarios run real arduino-cli-compiled firmware (fixtures under
`crates/bb-sim/tests/fixtures`, sources in `tests/firmware/uno/*`; examples use
`examples/fixtures/*.uno.hex` from `radio`); ESP32/Pico sketches are compiled by
`bb_firmware::host` in the tests.

- `PART-1…8` **done** — 82 part definitions (variants as properties: LED colours, diode/BJT/MOSFET types, battery kinds) in `crates/bb-circuit/parts/*.json` (boards UNO/Nano/Mega
  with header maps from `bb_firmware::Board`, ESP32 DevKit + Pico with `mcu.core = "host"`;
  building blocks, light & sound, sensors, semiconductors, ICs, displays, motors, radios,
  instruments). Simple library = the 25 design parts in `SIMPLE_ORDER`.
- `PART-9` **done** — one JSON schema for built-in and plugin parts (`Library::add_json`),
  validated on load; geometry for bench (layouts axial/row/dip/rows/board/breadboard), schematic
  (generated symbol pins) and PCB (KiCad footprint names); ratings + inspector fields. Documented
  in `docs/ARCHITECTURE.md` §3b.
- `PRJ-1` **done** — `Project` (`breadbox/1`, migration from `breadbox/0`, `TooNew` error),
  atomic save, firmware refs (`sketch`, `fqbn`, `image`), probes, instruments, world, settings.
  Editing via `edit::Editor` (transactions, undo/redo, copy/paste/duplicate, auto wire colours).
- `PRJ-2/3` **done** — 11 examples written by `cargo run -p bb-circuit --example
  write_examples` (garden-node exactly as designed; blink, night-light (UNO + LDR + 2N2222),
  thermometer, ble-button, servo-sweep, esp32-weather, pico-blink, 555-astable, rc-filter-bode,
  op-amp-amplifier). Every example: empty bench ratsnest, no ERC errors, DRC clean (test).
- `PRJ-4` **done** — `.cir` export (Pro cards + `* breadbox mcu` notes) and import with
  auto-placed schematic (clearance packing).
- `WB-7` **done** (model) — physical / schematic / logical connectivity (union-find), net
  naming (labels, MCU pins, power symbols), ratsnest for bench and schematic (Prim MST).
- `WB-12` **done** (model) — ERC (missing ground, unconnected pin, floating input, output
  conflict, supply short, overvoltage e.g. "U2.RX is 3.3 V tolerant, driven by 5 V D2 — add a
  divider", LED without resistor) + DRC (not plugged, loose lead, leads shorted by a strip, two
  leads in one hole) + `summary()` → `ERC 1 warning · DRC clean`.
- `WORLD-1…6` **done** — soil pots (evaporation 1.4 %/h at 21 °C calibrated, drainage above
  field capacity, watering), light (manual / solar elevation at a location, LED + lamp light
  sources, inverse square), air temperature + humidity with diurnal cycle, objects (hand,
  person, wall, magnet), keyframe timelines + expression scripts (`moisture = 45 -
  0.8*t_hours`), time-warp, global noise toggle.
- `SIM-1` **done** — event-driven co-sim (MCUs lead, analog lags; crossings at interpolated
  times; adaptive quantum 1 µs…1 ms; inline ADC answers when quiescent). Blink 1 Hz within
  ±0.1 % on the real sketch.
- `SIM-2` **done** — `PinModel` (Thevenin 25/22 Ω, 35 kΩ pull-up, Schmitt 0.3/0.6 Vcc,
  leakage), ADC S/H at the exact sample time, quantization + optional noise; analogWrite → RC
  matches a pure SPICE run within 1 %.
- `SIM-3` **done** — `SimHandle` pacing 0.1×–10× / Max, pause, step, reset, reset-MCU,
  deterministic replay (same seed → identical serial + events).
- `SIM-4` **done** — garden-node at **16.9× real time** (release, M3 Max; 16.7× with 60 Hz
  telemetry). `cargo test -p bb-sim --release --test perf -- --ignored --nocapture`.
- `SIM-5` **done** — `Telemetry` via `ArcSwap` (parts' visuals + readings, MCUs, rails / power
  report, probes, scopes, meters, ERC/DRC), bounded serial / event / audio streams.
- `SIM-6` **done** — ratings with stress/destroy (LED burnout test), MCU pin damage > 80 mA,
  USB polyfuse I²t trip + recovery, AVR brown-out via `set_vcc`, battery sag.
- `SIM-7` **done** — re-flash (`Command::Flash` / `FlashHost`) keeps bench + world; MCU-only
  reset; `Bench::update_project` rebuilds keeping MCU/device/world state (nodesets from the old
  solution).
- Host boards (`MCU-6` glue) **done** — `bb_sim::HostMcu` runs ESP32/Pico sketches behind
  `McuCore`; `RadioService` serves their Wi-Fi/BLE from `bb_radio::RadioSpace`.
  esp32-weather on the bench: DHT22 *device* answers the bit protocol, MQTT publishes reach the
  broker every 2 s, a dashboard command switches GPIO2.

Tests (debug): bb-core 7, bb-world 13, bb-circuit 40, bb-sim 50 (+3 perf, ignored): blink,
button + INPUT_PULLUP + RC debounce, PWM → RC vs SPICE, garden-node (serial, soil τ, D9 duty vs
lux, A0 probe vs DMM), LED burnout, polyfuse trip, DHT22 (Adafruit lib), DS18B20, BME280,
SSD1306, LCD (LiquidCrystal + I²C backpack), servo sweep, stepper, 74HC595, tone, WS2812,
HC-SR04, every UNO example on its fixture, pico-blink, esp32-weather, deterministic replay,
SimHandle threading, 555/motor/relay/regulators/gates/L293D without firmware. clippy
`-D warnings` + fmt clean.

Deviations / open issues:
- `esp01` / `hm10` / `hc05` run a small AT responder (`AtStub`) until the radio glue replaces the
  factories (Requests above). Host boards' I²C/SPI (`HostIo::i2c_*`) and Serial1/2 on pins are
  not bridged to bench devices yet (DHT-style bit-banged GPIO works); host PWM at duty 0 < d < 1
  with 0 Hz (DC) is treated as on/off.
- Inputs reach an AVR at most one instruction late when the circuit lags a quantum
  (`BenchStats::late_edges`); benches with several MCUs sync every 1 µs.
- WS2812 DOUT is not forwarded (chained strips = one long strip); the Pro-mode 555 has no
  oscillating SPICE macro-model (the Simple/bench 555 is behavioural and exact).
- Bench rebuilds restart the analog session from nodesets of the old solution, not a full state
  copy (capacitor charge on renamed nodes is re-derived from the operating point).
- The board L LED level in telemetry is averaged over the frame interval (≥ 50 ms).
- Small additive changes outside my crates: `bb-spice` `CircuitBuilder::cards`,
  `Session::is_quiescent`, `Session::with_ext` (tests green); `scripts/build-test-firmware.sh`
  builds `bb-sim` sketches too.

### ui-core
Phase 3 — `crates/breadbox` (app shell, theme, widget kit, Home, Firmware screen, Pro Analysis
tab, kittest harness) + `assets/fonts`. Module map, router, widget kit and the `BenchLink`
contract: ARCHITECTURE §8. Screenshots (1440×900, rendered by the GUI tests):
`docs/screenshots/{home,home-light,firmware,firmware-serial,firmware-debug,firmware-error,
pro-analysis,pro-schematic,pro-pcb,workbench,plugins,lessons,widgets}.png`.

- `PLAT-2` **done** — eframe/egui 0.37 on wgpu; Inter 400/500/600, JetBrains Mono 400/500,
  Space Grotesk (variable, 500/600) bundled in `assets/fonts/` with their OFL texts.
- `PLAT-4` **done** — release build: first frame 596 ms after start (logged at info level);
  0.0 % CPU when idle (nothing repaints without input/jobs/running bench; GUI test asserts it).
- `PLAT-5` **done** — vector icon set from the design's SVG paths; a unit test rejects
  sparkle/magic/wand names; none used anywhere.
- `APP-1` **done** — top bar on every screen: logo mark (painted from the design SVG) +
  "Breadbox" (Space Grotesk 600, click → Home), breadcrumb (project / section with a screen
  switcher menu), mode badge (Playground / Pro).
- `APP-2` **done** — Home: new Playground/Pro project (modal → `~/Documents/Breadbox/<name>.
  breadbox`), Open project (folder picker), recent projects, examples gallery (11 projects from
  `examples/*/project.json`, open read-only, ⌘S copies), Lessons + Plugins entry cards,
  toolchain status card. Shortcuts ⌘N ⌘O ⌘S ⌘⇧S ⌘Z ⌘⇧Z ⌘W (+ ⌘F ⌘B ⌘U ⌘R ⌘1–5 ⌘⇧H).
- `APP-3` **partial (mine done)** — Home, Firmware, Pro→Analysis built to the design; Workbench,
  Pro→Schematic, Pro→PCB, Plugins, Lessons are clean placeholders registered in
  `screens::default_registry()` for phase 4 (each names its slot + owner).
- `APP-4` **done** — native menu bar via muda (Breadbox / File / Edit / View (+ Theme) /
  Simulate / Window / Help) mirroring the shortcuts; dark default, light token set (screenshot).
- `APP-5` **done** — window geometry (eframe), route, theme, recents, last project and per-
  screen state (breakpoints, step mode, tabs) persist; edited sketches autosave (debounced
  1.2 s) in user projects; tested by a persistence round trip.
- `MCU-4` UI **done** — sketch tabs from the sketch folder, Libraries list (from `#include`s,
  user-library versions), toolchain box (`compiler_label()`, "cycle-accurate AVR core" /
  "API-level", arduino-cli version), target selector (project MCUs, radio modules shown,
  "Compile for board" = `Board::all()` + ESP32/Pico host boards), Verify on a job with the real
  arduino-cli lines + size report, diagnostics → editor squiggles/hover + Output, library
  manager (search + install with progress), one-click toolchain install with progress.
- `MCU-5` UI **done** — breakpoint gutter ("Breakpoint line N"), "Step · breakpoints" (-Og
  builds, arms breakpoints, `note: line 8 — breakpoint armed on U1 (hits: 0)`), executing-line
  band, debug toolbar (continue / pause / step over / into / out), Debug tab (call stack,
  locals + globals, registers + SREG flags). Tested against real firmware (hit line 8 of blink,
  step over → 9, continue → 8).
- `RF-7` UI **done** (component) — Radios panel from `RadioPanelModel` (cards, packet log with
  amber drop rows, latency / loss sliders, AP-drop + LAN-bridge checkboxes, BLE card / empty
  state) emitting `RadioCommand`s; live with garden-node via the AVR bench's ESP-01.
- `PRO-2` **partial** — SPICE-highlighted editable netlist with parse diagnostics; shows the
  design's garden-node netlist as a built-in demo or a project's `analyses/*.cir`. "synced from
  schematic" + detach/re-sync need bb-circuit (phase 4); header says "built-in demo · editable".
- `PRO-3` **done** — Analyses cards (Transient, DC sweep, AC/Bode, Monte Carlo, Temperature
  sweep, Noise, Op point) with config popovers that rewrite dot-cards, enable/disable (`*off`),
  Run analyses on a job with progress + Cancel, header `Results::summary()`.
- `PRO-4` **done** — .tran (V + mA axes, draggable cursors → `Δt 2.041 ms · f 490.0 Hz · duty
  47.8%`), .dc + .step family on log x with `op: 180 lx → 2.60 V`, .mc histogram with
  threshold marker, .ac Bode (mag + phase), .noise, .op table with "bench probe" column fed by
  `BenchUpdate::probes` (empty until the bb-sim link provides values).
- `PRO-5` **done** — mixed-signal panel (sync step from `sync=`, solver, integration +
  reltol from `.options`). The analog-only run uses stand-ins for `CAPSOIL` / `ATMEGA328P`
  (MCU replays its D9 PWM) — the real firmware co-sim is bb-sim's.
- `PRO-6` **done** for plots + netlist — Export .cir (ngspice-runnable, stand-ins inlined,
  `*!bb` directives), per-plot CSV / PNG (viewport screenshot crop) / SVG. Schematic export is
  the schematic editor's (phase 4).
- `TEST-5` **done for my screens** — `cargo test -p breadbox`: 45 unit, 12 GUI (navigation
  Home→Firmware→Pro→Home, garden-node upload → serial + MQTT + radio panel, blink breakpoint
  hit + step over + continue, Verify + real compile error mapped to the editor (skips without
  a toolchain), Pro auto-run → plots + op table, every screen rendered, keyboard shortcuts,
  idle repaint, light theme, analysis-card popover disables .mc → histogram gone, library
  manager modal), 8 widget tests (segmented, mini-segmented/checkbox/tabs, stop slider,
  buttons, editor gutter, editor find, collapsible panel, gallery snapshot). clippy
  `-D warnings` + fmt clean.

Contracts for phase 4 (`ui-bench`): implement `bench::BenchLink` over `bb_sim::SimHandle` and
register it as the `BenchFactory`; replace the Workbench / Pro Schematic placeholders in
`screens::default_registry()`; reuse `screens::firmware::radio_panel::radio_panel`,
`debug_panel`, `widgets::*` (World sliders = `StopSlider`, right rail = `rail_button`, panels =
`collapsible_side`), and feed `BenchUpdate::probes` for the Pro bench column.

Notes / deviations:
- The AVR bench is single-board with fixed ADC voltages; the ESP-01 wiring (D2/D3) is fixed —
  real wiring comes from bb-circuit nets via bb-sim.
- Firmware screen design shows "wasm · in-browser" in the toolchain box; Breadbox shows the
  arduino-cli version instead (true for this app).
- egui_plot draws the plot area; overlays (cursors, markers, threshold labels) are painted on
  top. The design's `.op` "bench probe" values need the live bench (phase 4).

### ui-bench
Phase 4 — the Workbench, breadboard + schematic canvases, `SimBench`, Pro → Schematic, Pro
netlist sync + bench probes, Share/export. Module map + contracts: ARCHITECTURE §8.
Screenshots (1440×900, rendered by `tests/workbench.rs`): `docs/screenshots/{workbench,
workbench-schematic,workbench-edit,workbench-smoke,workbench-power,workbench-parts,
pro-schematic,pro-analysis-synced,firmware-phone}.png` (`workbench-parts` = every library part
on one bench; the test also writes zoomed QA tiles to `target/…/tmp/parts-qa/`).

- `SimBench` **done** — the app's default `BenchLink` over `bb_sim::SimHandle`: prebuilt
  firmware at start (Run works without the toolchain), Upload → `Flash` (AVR) / `FlashHost`
  (ESP32 / Pico; host-compiled in the background, bench rebuilt around the executable), source
  debugger attached + breakpoints synced on the engine thread (`Command::With`), step / pause /
  continue, per-MCU debug + serial (`MCU-5`, `MCU-7`), Radios panel + virtual phone from
  `RadioTelemetry` (`RF-6/7`), probe keys for the Pro bench column (`PRO-4`), device events →
  output/toasts, buzzer PCM + relay clicks via `rodio`, idle-quiet repaint watcher. Circuit
  edits flow in via `update_project` (electrical → `UpdateProject`, probes → `Add/RemoveProbe`,
  instruments → `set_instruments` + rebuild, live props → `SetPartParam`) keeping MCU/world
  state (`SIM-7`). `AvrBench` stays for tests.
- `WB-1` **done** — header: Breadboard ⇄ Schematic, Run/Pause (amber), `t = mm:ss.mmm`, speed
  menu (0.1×…Max), status dot (running / paused / stopped / error), Share.
- `WB-2` **done** — palette: Simple (25 tiles by category) / Advanced (all parts by category,
  part numbers, filter chips), search, hover descriptions, drag tiles onto the bench (egui DnD,
  ghost preview with the holes it lands in) or click-then-place.
- `WB-3` **done** — 2.5D bench art for every `Library::builtin()` art id (UNO/Nano/Mega/ESP32/
  Pico with vertical silkscreen header labels and live ON/L/TX/RX LEDs, 830/400 breadboards with rails + numbers,
  resistors with live colour bands, LEDs/RGB with bloom ∝ current, caps, DIP ICs, ESP-01,
  HM-10/HC-05, sensors, LCD + OLED framebuffers, 7-seg, bar graph, NeoPixels, buzzer waves,
  motor/servo/stepper angles, relay, draggable pot knob, pressable buttons, batteries, USB, pot
  of soil with plant + sensor depth, mini scope with live trace, DMM); soft shadows, gradients,
  night dimming with ambient light; zoom (⌘-scroll / pinch, 25–400 %, readout), pan (scroll,
  space- / middle-drag), `.dots` grid.
- `WB-4` **done** — jumper wires from any pin/hole to any pin/hole (press-origin snapping),
  sagging beziers with shadow + highlight + plug ends, auto colours (red +, black GND), colour
  picker (inspector + context menu), bend points (double-click add/remove, drag), delete, net
  highlight on hover.
- `WB-5` **done** — select / ⇧ / box select, move with 0.1" snapping, R / F / ⌫ / ⌘D / ⌘C /
  ⌘V / arrows, undo/redo through `Editor` (⌘Z / ⌘⇧Z), inline properties in the inspector
  (E-series stepping, colour bands update live).
- `WB-6` **done** — floating probe labels with live values (leaders to the nearest point of the
  probed net), probe tool, hover tooltips with net voltage + LED current.
- `WB-7` **done** — schematic view + Pro → Schematic: ANSI / IEC symbols for every symbol id,
  designators + values, bench wiring mirrored as orthogonal routes, power / ground flags, net
  labels for long nets, junction dots, live value chips, ERC line + title block; editing (move,
  rotate, draw orthogonal wires, delete, **Net label** tool: name it, click a pin or wire —
  same name joins nets on the bench netlist too; select + ⌫, undo) through the shared model;
  auto-placement of new parts; ratsnest both ways. garden-node's sheet laid out like the design.
- `WB-8…12` **done** — right rail + collapsible panels (World with stop sliders, evaporation
  rate, day/night + location picker, noise, water, humidity + time-warp under "More settings";
  Instruments with add/remove + "Clip CH1/+/COM" probe clipping; Probes with min/max/avg;
  Inspector with datasheet fields, live readings, editable params, health, Repair all;
  Waveforms with the scope screen + timebase/V-div), bottom dock (Serial monitor: colour-coded
  timestamped lines, port label with the standard baud, per-MCU tabs, send box, line ending,
  baud, Clear, autoscroll, radio annotations; Probes & power: probe cards, rails, total draw,
  dissipation table, USB budget bar), status strip (bench summary, ERC/DRC click-to-list → select
  parts, zoom menu).
- `WB-13` **done** — LED bloom ∝ current (perceptual), RGB mixing, buzzer audio + waves, motors
  / servos / steppers move, LCD/OLED framebuffers, 7-seg, relay clicks, magic smoke (top layer)
  + damage badges, Repair all.
- `PRO-1/2` **done** — Pro → Schematic tab (net list with live values, ANSI/IEC, SVG/PNG/
  .cir/BOM export); Pro → Analysis "synced from schematic" (world values as `.param`s so
  `.dc lux` / `.step` act on the parts, MCU outputs replayed from the bench), value edits
  round-trip, other edits detach with Re-sync; bench-probe column live (`PRO-4`).
- `PRJ-5` **done** (Gerbers are the PCB agent's) — `.breadbox.zip` (+ opening bundles from
  Open / `open_path`), `.cir`, BOM CSV, schematic SVG + PNG (resvg with the bundled fonts), bench
  PNG (viewport screenshot crop).
- Instrument hook + embedding — `canvas::instruments::{BenchInstrument, InstrumentRegistry}`
  (mini scope + DMM go through it); `BenchCanvas` / `SchematicCanvas` are embeddable widgets;
  `SimBench::factory_with(library, options_hook)` + `CircuitDoc::set_library` take plugin parts
  and device factories.
- `TEST-5` — `cargo test -p breadbox`: 67 unit, 12 GUI (`tests/gui.rs`), 8 widget, 12 + 1
  workbench (`tests/workbench.rs`: every library part rendered on one bench (art QA);
  schematic net labels join D9 + D8, delete + undo; garden-node run + serial + J1 inspector + Probes & power +
  Advanced palette; probe tool + R / ⌘D / ⌫ + per-MCU serial tabs; schematic toggle +
  Pro schematic; palette drag + jumper wire + ratsnest + undo/redo + inspector 4k7 + colour
  bands; soil slider → `moisture=0%`; LED on the rail burns out with smoke; Firmware upload on
  the live bench + Pro synced netlist + bench probes; ESP32 host board + FlashHost; virtual
  phone connects to the HM-10; Share bundle/.cir/BOM/SVG/PNG + reopen; perf, ignored).
  clippy `-D warnings` + fmt clean.
- Perf (`cargo test --release -p breadbox --test workbench -- --ignored perf --nocapture`, M3
  Max): Workbench with garden-node running at 1×: **0.65 ms CPU per frame** (UI + tessellation,
  worst 1.0 ms, ≈ 41 k triangles) — far inside 60 fps; the bench holds **1.00× real time**.

Deviations / notes:
- The bench is drawn flat 2.5D (no CSS-style perspective tilt): parts stand up off the board,
  shadows/gradients give the depth.
- Changes outside my paths (small, committed alone): `bb_sim::Instruments::set_instruments`
  (additive); garden-node schematic placements in `bb-circuit/examples/write_examples.rs` +
  `examples/garden-node/project.json`. ui-core's `tests/gui.rs` updated where it asserted the
  old placeholders / the ideal-5 V demo netlist.
- `Cargo.lock` carries both my deps (rodio/cpal, zip, resvg, crossbeam-channel) and the plugins
  agent's; left for whoever commits the lock next.
- Open: Logic analyzer / bench supply / function generator panels render through the
  instrument hook once the plugins UI registers them; PCB (`PRO-7`) and Gerbers are phase 4b.

### ui-plug-les
Phase 4 — Plugins screen, plugin instruments on the bench, Lessons + lesson editor, virtual
phone + MQTT explorer UI, PCB. Module map + contracts: ARCHITECTURE §8 ("Plugins in the app",
"Instrument panels", "Lessons", "Radio tools", "PCB"). Screenshots (1440×900, rendered by the
GUI tests with `BREADBOX_UPDATE_SCREENSHOTS=1`): `docs/screenshots/{plugins,lessons,pcb,phone}.png`
(+ `pro-pcb.png` from `tests/gui.rs`).

- `PLUG-4` **done** — Plugins screen per design: category nav (6 categories) + installed list
  (+ lesson packs), search, detail page (live front-panel art from the plugin's `panel.json`,
  `INSTRUMENT · installed` chip, specs grid from `highlights`, the live loading callout
  `Probing A0 (J1 out): meter load shifts the node by −0.4 mV — …` from
  `host::loading_callout` on the running bench, Place on bench, Edit model (a local, hot-reloaded
  copy in `plugin-dev/`), Enable / Disable, Uninstall (confirm), Apply theme / Open in Pro /
  Open in Lessons), "More to add" cards with Install → permission prompt (`Permission::describe`)
  → `install_bundled` / `install_package` / `load_local`, read-only `plugin.json` pane with JSON
  highlighting, "Load local plugin…" (folder or `.bbplugin`), "Plugin SDK docs" (renders
  `docs/PLUGIN-SDK.md`), hot-reload toasts. App glue: `PluginService` (`AppCtx::plugins`) —
  plugin-aware bench factory, plugin library on the open circuit (plugin parts in the palette),
  theme palettes over the tokens, lesson packs in Lessons, analysis plugins in Pro → Analysis.
- `PLUG-6` **done** (rendering) — `widgets::instrument`: declarative panels with live
  `PanelState` (LCD / 7-seg / VFD digits + annunciators + bar graph, buttons, rotary dial,
  knobs, switches, LEDs, jacks, scope / logic screens with graticule, traces, markers, cursors)
  and working controls → `Command::Input`. On the bench (`canvas::plugin_parts`, hooked into
  `BenchCanvas` / `art::draw`): the panel is the part's art, controls work in place, wires in
  jacks are probe leads whose tips drag onto any pin (or dangle). The Handheld DMM looks like the
  design's meter (grey body, green LCD, dial labels on the ring, red/black jacks).
- `PLUG-7` UI surfaces **done** — themes (Apply theme), analyses (Pro → Analysis cards + Run),
  parts (palette via the plugin library), lesson packs (Lessons catalog), instruments (bench).
- `LES-2` **done** — Lessons screen per design: track nav with ✓ / current / numbers + More
  tracks + Make your own lesson, the lesson's live bench (the Workbench's `BenchCanvas`
  embedded over the lesson circuit with its own `SimBench`; Pro lessons show the schematic),
  callouts under it, the step panel for every step kind (step bar, `Step 3 of 5`, body, formula
  card, `Pick R1` grid, "Do the math myself" → prediction + Check + conditional hints + "Show me
  the working", meter masked `----` until right, "Your measurements" filled from the settled
  meter + checkpoint, damage card + Repair, quiz, code editor + Compile & upload (or the
  reference build without a toolchain), goals), `Lesson 3 of 8` progress bar, Back / Next gated
  by the goals, progress saved per step.
- `LES-5` **done** — "Open in free play" writes the lesson bench (+ its sketches) as a
  Playground project and opens it on the Workbench.
- `LES-6` **done** (UI) — lesson editor: title / track / pack, steps (add by kind, reorder,
  remove, text, formula, quiz, prediction), "Capture the current bench" (Workbench project or the
  open lesson), goals suggested for a picked part (with its live readings) or two pins, Validate,
  Save lesson pack (user plugin folder → active → opens in Lessons), Package .bbplugin….
- Content fix (bb-lessons): "Nailed it. The meter agrees: …" quotes the meter's reading
  (`{meter}`, e.g. 13.4 mA through the meter's burden) instead of the formula (13.6 mA); the same
  for the voltage-divider lesson; LESSON-FORMAT documents it.
- `RF-6` / `RF-7` UI **done** — `widgets::radio_tools`: virtual phone window (scan list with RSSI
  bars, connect / disconnect, services → characteristics with props, value text + hex, Read,
  Notify, Write, Bluetooth serial terminal, activity log) and MQTT explorer window (wildcard
  subscriptions, live messages with size / QoS / retain, filter, publish with QoS + retain);
  buttons in the Radios panel and two Workbench rail buttons.
- `PRO-7` **done** — `bb_circuit::pcb` (footprints for every part's footprint name — THT pitch /
  drill / shapes parsed from the KiCad-style names, 0805/0603/1206/SOT-23/SOD-123/SOT-223/PLCC4
  SMD, boards / modules from their bench leads; placement + auto-place + outline; copper
  connectivity; ratsnest; DRC: clearance, shorts, width, drill, edge, unrouted; Gerber RS-274X F/B
  copper, F/B mask, F silk, Edge.Cuts + Excellon, with readers) and Pro → PCB (drag / rotate,
  Auto-place, Fit outline, Route tool pad → corners → pad on Top / Bottom with 0.25–1.0 mm widths,
  V = via + layer switch, Via tool, delete, live DRC list + header summary, Export Gerbers…,
  own undo). Persisted in `project.json` (`PartInstance::pcb`, `Project::pcb`). Share →
  "PCB Gerbers + drill (.zip)" on the Workbench (`PRJ-5`).
- `TEST-5` (my screens) — `cargo test -p breadbox`: `tests/plugins.rs` 2 (install the scope
  through the permission prompt; DMM detail; Place on bench → DMM1 on garden-node A0 reads
  ≈ 1.88 V AUTO DC; the live callout; probe tips dragged onto a 4k7 → dial Ω → `4.7… kΩ`; local
  plugin + hot reload, theme on/off, SDK docs, analysis plugin run in Pro), `tests/lessons.rs` 3
  (lesson 3 → step 3: wrong value → hint, full-5-V hint, working, 13.6 → "Nailed it. The meter
  agrees: 13.x mA", three rows → Checkpoint passed → break it (None) → LED pops → Repair → free
  play; the design screenshot; the lesson editor → pack → Lessons), `tests/radio_tools.rs` 2
  (phone ↔ ble-button: scan, connect, notify on, button press → `pressed #1` notification; MQTT
  explorer `garden/#` sees `garden/pot1` and its own publish), `tests/pcb.rs` 1 (blink:
  auto-place, unrouted → routed on two layers → `DRC clean`, a via, 7 Gerber/drill files parse
  back with every pad + hole inside the board); unit tests for the panel geometry, markdown /
  JSON, place-on-bench, plugin service, guess parsing, editor helpers, plugin analyses;
  bb-circuit pcb 5 (footprints for every part, placement, copper + ratsnest, DRC, Gerbers).

Changes outside my paths (small, all tests green): `app.rs` (`AppCtx::plugins`,
`AppCtx::progress_path`, `AppOptions::{plugin_dir, progress_path}` — tests get fresh temp
folders, `apply_plugins` each frame, radio tool windows), `theme.rs` (plugin palette override),
`canvas/bench.rs` + `canvas/art/mod.rs` (plugin-part hooks: `plugin_parts::interact/draw/
draw_leads`, `InstrumentView::panel`, plugin art bounds), `screens/mod.rs` (registry),
`screens/pro/mod.rs` (plugin analyses section; Bode legend `φ` — Inter has no `∠`),
`screens/firmware/radio_panel.rs` + `screens/workbench/mod.rs` (radio tool buttons),
`screens/workbench/share.rs` (Gerbers item),
`tests/gui.rs` (asserts the real Plugins / Lessons / PCB screens), `plugins/*/parts.json`
(instrument leads on their panel jacks), `plugins/dmm-handheld/panel.json` (design grey),
`lessons/1-electricity/0{3,5}-*.toml` + `bb-lessons` test (`{meter}`).

Notes / open issues:
- The lesson bench keeps running in the background while another screen is open (cheap for
  the lesson circuits; it pauses when a new lesson opens).
- The design's lesson meter is a landscape bench meter; Breadbox draws the Handheld DMM plugin's
  own (portrait) front panel everywhere for consistency.
- PCB: manual routing only (no autorouter); traces are straight polylines (no arcs / 45°
  helpers); the silkscreen has outlines + references on screen but only outlines in the
  Gerber; rotations are 90° steps; SMD footprints are placed on the top side only.
- "Edit model" copies a bundled plugin into `plugin-dev/<id>` next to the user plugin folder and
  runs it as a local (hot-reloaded) plugin; rebuilding its wasm is up to the author's toolchain.

### plugins
Phase 3 — radio/host-board glue, `bb-plugin`, `bb-plugin-sdk`, first-party plugins, `bb-lessons`.
**All five items done.**

- Radio glue **done** — `bb_sim::devices::RadioModule` (esp01/hm10/hc05/rylr896: UART pins
  decoded with exact bit timing following the module's baud, TX frames as Thevenin edges,
  VCC/EN/RST/KEY/STATE pins, AtStub fallback without a radio service); `bb_sim::radio::
  RadioService` (+ `service_factory()` in `BenchOptions::default()`, `ProjectRadios` =
  `project.radios` incl. `bench_pos`, default floor plan reads `-54 dBm (3.2 m, 1 wall)`);
  annotations routed to the serial monitor of the MCU wired to the radio; host boards: Wire →
  `Device::i2c_transfer` (BME280, SSD1306, PCF8574 LCD), SPI → `Device::spi_transfer` (74HC595),
  Serial1/2 as real UART waveforms on their pins.
- `PLUG-1/2/7` **done** (engine) — see ARCHITECTURE §7. `PLUG-3` SDK **done** (docs in progress).
  `PLUG-5` **done** — 7 first-party plugins built to wasm and committed + 5 SDK examples.

- `LES-1/3/4` + model side of `LES-5/6` **done** — `bb-lessons` (ARCHITECTURE §7b,
  `docs/LESSON-FORMAT.md`): TOML format with 8 step kinds, 23 check types (circuit, bench,
  instruments incl. the DMM plugin, serial, pin frequency/duty, MQTT, phone, SPICE), 21
  worked-solution actions; `Session` (the design's step 3 verbatim: choice, "Do the math
  myself", 4 conditional hints, "Show me the working", measurements table + checkpoint,
  damage card), headless `Runner` (scripted solver), `Progress`, `editor::Draft` (capture
  bench → steps → suggested checks → lesson-pack plugin). Content: 5 tracks / 24 lessons
  (Electricity you can see 8, Sensors 4, Microcontrollers 4, Radios & IoT 4, Pro SPICE 4),
  every start circuit a real project (`write_benches` example), AVR sketches prebuilt
  (`lessons/build-firmware.sh`).

APIs for the UI (details in ARCHITECTURE §6c/§7/§7b):
- Lessons screen: `bb_lessons::Catalog::builtin()` + `add_packs(&host.lesson_dirs())` → tracks
  (`def.label/title`, lessons in order), `Progress::load(Progress::default_path())` for the
  ✓ marks / resume. Open a lesson: `Session::new(&loaded, host.library())`; bench project =
  `session.project()`, firmware `loaded.images(&p)` + `loaded.host_firmware(&p, &lib)`, world
  from `lesson.bench.world`. Step panel: `label()`, `current()` (title, body, formula,
  callouts → canvas callouts, choice, predict, table, quiz, code), `render(text, meter)`,
  `choose(i)` → send the returned `Edit` (`SetProp` → `Command::SetPartParam`, `Project` →
  `UpdateProject`), `set_math`, `meter_hidden()` (show "----"), `predict(guess, meter)` →
  `Verdict.text`, `working()` / `toggle_working()`, `record(meter)` when the meter settles,
  `checkpoint()`, `answer_quiz`, damage card when `Telemetry` shows the part damaged
  (`damage_title/text/repair_label`; Repair → `Command::RepairAll` + `choose(default)`),
  goals: `check::prepare` on step entry and `check::evaluate_step(step, project, lib,
  Some(bench), state)` inside `Command::With`; `next_step()` / `prev_step()`, `next_label()`,
  "Open in free play" → `free_play()`. "Show me how" can replay `step.solution` actions.
  Code steps: `code::fqbn_for(project, lib, mcu)` + `code::compile(source, fqbn)` → `Build {
  success, image, diagnostics, log }` → `Command::Flash` → `set_firmware_loaded(true)`.
- Make your own lesson: `editor::Draft::new(title, track)` / `open(..)`, `capture_bench(&p)`,
  `add_step(kind, title)`, `suggest_for_part(lib, part, bench.readings(part))`,
  `suggest_for_pins(lib, a, b, volts)`, `add_check`, `validate(lib)`, `save_pack(dir, PackInfo)`
  (then `Registry::load_local`), `package(..)` for sharing.
- Radios panel / phone: `Telemetry.radio: Option<RadioTelemetry>` (cards, packet log rows,
  conditions, APs, `has_ble` empty state, phone scan/services/log/terminal, MQTT explorer
  rows, LAN bridge status) and `Command::Radio(RadioCommand::{SetConditions,
  SetAccessPoints, SetLanBridge, SetBenchPos, ApplySettings, ExplorerSubscribe/Unsubscribe/
  Publish, ClearLog, Phone(PhoneCommand::{Connect, Disconnect, Read, Write, Subscribe,
  TerminalOpen, TerminalSend})})`; `Command::ExportPacketLog { format: "pcapng"|"csv",
  reply }`; `Command::With(BenchFn)` runs a closure on the engine thread.
- Plugins screen: `Registry::open_default()` → `catalog()` (cards with status Installed /
  Available / UpdateAvailable / Disabled / NeedsApproval / Planned / Error, chip, category,
  permissions), `get(id)` → `Plugin { manifest, root, manifest_text(), asset(path) }`,
  `package::inspect(bytes)` (permission prompt: `Permission::describe()`),
  `install_package/install_file/install_bundled(id, granted)`, `load_local(dir, granted)`,
  `uninstall`, `set_enabled`, `grant`, `check_changes()` (hot reload; then rebuild the host and
  send the sim a reset with the new library/options).
- Bench integration: `PluginHost::new(&mut registry, project_dir)` → `library()`,
  `bench_options()` / `apply(&mut opts)`; `instruments()` (part to place, `Panel`, art);
  `panel_for_part(def)`; `asset_path("plugin://id/file.svg")` for part art;
  `host::loading_callout(&mut bench, part)` (design callout text); `themes()`,
  `lesson_dirs()`, `analyses()` + `run_analysis(id, netlist, options)`.
- Instrument panels: static `bb_plugin::panel::Panel` (display, button, rotary, knob, switch,
  jack, led, screen, label; mm coordinates) + live `PartTelemetry.panel` (`PanelState`:
  displays text/unit/annunciators/bar, leds, screens traces/markers/header/cursors, controls,
  readouts). Controls → `Command::Input { part, input: UserInput::Value { key: <control id>,
  value } }` (rotary/switch = index, knob = value, button = 1).

Tests: bb-lessons 1 unit + lessons 9 (all 24 lessons validate, Track 1 = the design's
list, lesson 3 step 3 verbatim, "None" pops the LED and Repair restores it, progress,
editor round-trip, make-your-own → lesson-pack plugin → installed → solved, doc sync) +
solver 6 (the scripted solver completes all 24 lessons through the public engine API; every
code step's starter and solution compile with the Arduino toolchain and the freshly compiled
solution passes its goals);
bb-plugin 7 unit + sandbox 6 + registry 4 + dmm 4 + instruments 4 + packs 8 + schema 3;
bb-sim + radio 5 + host_bridges 4; bb-spice +1 (watch mid-trail).

Changes outside my crates (all tests green): bb-core additive hooks (`DeviceCtx::service /
capture / samples`, `Device::i2c_transfer / spi_transfer / panel`); bb-sim (radio, captures,
world-model factories, `what_if_ext`, device wakes survive rebuilds, end-of-interval
integration + 1 µs settle sync after MCU edges on analog nets, 10 µs analog max step);
bb-radio (pending station events / gateway downlinks wake the space, AT parser byte-safe);
bb-spice (watch added mid-trail no longer panics); bb-world (`<pot>.evaporation_scale`);
bb-sim rebuilds keep capacitor charge (`.ic` on capacitor nodes instead of `.nodeset`);
capsoil v1.2 output resistance 2.2 kΩ (design's −0.4 mV meter load).

### e2e
Phase 5 — `tests/e2e` (crate `breadbox-e2e`, TEST-4), `scripts/test-all.sh` (TEST-7), perf gates
+ `breadbox --smoke` (TEST-6, PLAT-4), GUI flows (TEST-5), and the bugs they found. How it works:
ARCHITECTURE §10.

Scenarios (all green; `cargo test -p breadbox-e2e`, ~10 s wall with the toolchain present):

| file | scenario | |
|---|---|---|
| `garden_node` | full run: boot line, `soil_raw=… moisture=…% light_raw=… led_pwm=…` in the design's ranges, `wifi: AT+CWJAP … rssi=-54 dBm ip=10.0.0.42` / `mqtt: PUB` annotations, MQTT explorer (virtual subscriber) gets `garden/pot1` JSON at 0.85–1.15 Hz, radio card + packet log (DHCP DORA, CONNECT/CONNACK, PUBLISH; CSV + pcap-ng export), probes / bench meter (A0 ≈ sensor model) / mini scope (490 Hz, 47.8 %), LED currents, rails, ERC line | ✅ |
| | D9 duty vs ambient light (5 → 2000 lx; LDR model incl. the night LED's own light) | ✅ |
| | soil slider: `moisture=` covers > 50 % of the step within 2 s (τ 1.8 s), settles, D8 water LED, publish follows, "Water the pot" | ✅ |
| | Handheld DMM plugin (from the registry) on A0: reads the sensor model, its 10 MΩ load really shifts the node by the callout's −0.4 mV vs. the same bench without it, follows the slider | ✅ |
| | fault injector: 25 % loss → amber `!` rows, TCP keeps publishing; AP dropped for 4 s → card not connected, no publishes, rejoin + broker reconnect + publishes resume | ✅ |
| `boards` | blink timing (firmware compiled by the toolchain; probe-tool frequency 0.5 Hz ±0.1 %, duty 50 %, mid-phase levels, LED) | ✅ |
| | esp32-weather (API-level): joins SIM-AP, DHT22 every 2 s → `weather/esp32`, retained LWT `online`, explorer command `led on/off` drives GPIO2, readings follow the room | ✅ |
| | ble-button: phone scan (RSSI) → connect (`phone connected`) → FFE1 notify `pressed #1/#2` → read → terminal `on` lights D13, `?` → `presses=2` → disconnect (`phone disconnected`) | ✅ |
| | pico-blink knob (12-bit), thermometer LCD + serial follow the room temperature | ✅ |
| `damage` | LED without its resistor: ERC warning, burns out (smoke, open), stays damaged after the circuit is fixed, Repair all → original current; UNO 5 V short trips the USB polyfuse (stress event, current limited, brown-out, power LED off) and recovers + reboots + blinks after the short is removed; USB power block fuse | ✅ |
| `sim_control` | deterministic replay of a scripted session through `SimHandle` (serial, probes, MQTT, visuals bit-identical; another seed differs); re-flash keeps analog / world / ESP-01 state; MCU-only reset; full reset; exact 1 ms / 1 µs steps; Play after an interrupted Step | ✅ |
| `lessons` | lesson 3 "Ohm's law with an LED" through `Session` + `SimHandle` + checkers: meter in series, math mode with the design's hints / working, 3-row checkpoint, break it ("None") + repair, quiz, progress saved + reloaded, free play | ✅ |
| `projects` | Save As of an edited example → reopen identical → runs; `.breadbox.zip` export → import (×2) → runs; every example's `.cir` runs in ngspice (`-b -r`, no errors) and its `.op` agrees with bb-spice (0.1 % / 1 nA; gmin-dominated nodes detected and skipped); `.cir` import → Pro project; Pro → Analysis on every example's synced netlist (no errors/warnings); garden-node Pro `.op` = bench probes, R1 edit round-trips | ✅ |
| `projects` (cont.) | Pro 555-astable `.tran` oscillates at the formula's frequency in bb-spice and, exported, in ngspice (within 2 %) | ✅ |
| `examples` | every example runs 5 s: no engine error, damage, error log, ERC error, DRC problem, warnings, MCU resets; day/night at 3600× time-warp drives the LDR + night LED; world script `moisture = 45 - 0.8*t_hours`; 555-astable LED at 1.44/((R1+2R2)C); night-light on in the dark | ✅ |
| `plugins` | fresh registry catalog (preinstalled / available / planned), install the 4-ch scope (refused without its permissions), place on garden-node → 490 Hz / 47.8 % like the mini scope, disable / uninstall; `.bbplugin` package install; local plugin hot reload | ✅ |
| `firmware` | toolchain debug build → Flash + Attach on the live bench → breakpoint `blink.ino:8` hit → step over to 9 (LED on) → continue → next hit 2.000 s later → remove breakpoint runs freely; a compile error maps to the tab / line | ✅ |

GUI (TEST-5, `crates/breadbox/tests/`): the existing flows (open example, Run, Breadboard ⇄
Schematic, palette drag, wire, resistor value + bands, serial, Firmware Verify / Upload /
breakpoint / step, Pro analyses on the synced netlist, Share export / import) plus **Pause**
(status + clock stop, Run resumes), a Monte Carlo card toggle that keeps the netlist *synced*
(and stores `*off .mc` with the project), and Pro → Analysis on a circuit without cards
(night-light → `.op` table).

Bugs found and fixed (all with regression tests):
- bb-radio: a phone hang-up never woke the radio space → HM-10 / HC-05 / ESP32 BLE sketches never
  saw `OK+LOST` (`02b90f7`).
- bb-sim scheduler: after processing something due *now* (a radio command) the MCUs ran a full
  quantum past wakes that processing had just scheduled; the HM-10's first UART start bit
  arrived late and SoftwareSerial read garbage. Services due now step before the MCUs; the
  quantum is re-bounded by new wakes (`b470b8e`).
- bb-sim `SimHandle`: Play after a Step interrupted by Pause stopped at the stale step end
  (`7852655`).
- Pro / Share: `.cir` export and the synced netlist froze every `ext()` at 0 (the 555's 9 V
  battery exported as 0 V); MCU stand-ins lacked `sync` (warnings on thermometer / ESP32 /
  Pico); card edits from the Analyses sidebar **detached** the synced netlist (now round-trip
  into `project.spice.directives` via an undoable `Editor::set_directives`); circuits without
  cards showed an empty Analysis tab (now `.op`) (`8de3232`).
- Share → `.cir` did not run in ngspice at all: per-element `tol=` ("unknown parameter"),
  `.tran 0 10m` ("TSTEP is invalid"), no analysis for Playground circuits (`deb4579`).
- Save As of an example kept `firmware.image = ../fixtures/…`, unresolvable from the copy
  (`3291343`).
- ERC: a source with + on ground and − on VEE (op-amp-amplifier's −12 V rail) was an error
  "shorted to ground" (`ffb9b89`).
- Plugins: 4 committed `.wasm` were stale and every module embedded the build machine's absolute
  paths; builds are now path-remapped and reproducible, `build-plugins.sh --check` (`f416926`).
- GUI tests rewrote 13 committed `docs/screenshots/*.png` on every run (`d8c6daf`).
- Pro → Analysis: after a mapped edit the comparison baseline was never refreshed, so
  re-enabling a card looked like "no change" (the `*off` stayed stored) (`bf34a35`).
- Pro / `.cir`: the NE555 was only its divider — 555-astable never oscillated in Pro or ngspice.
  New behavioural subcircuit (comparators, hysteretic-switch latch, output stage, discharge;
  convergent power-on operating point): 1.44/((R1+2R2)C) in bb-spice and ngspice (`1fc3ac8`).
- bb-spice: a switch closing onto a nonlinear load (555 output → LED) died with "timestep too
  small" where ngspice runs; after a switch changes state, Newton retries resume from the last
  iterate (reference suite unchanged) (`9af2893`).

Perf (release, M3 Max, `scripts/test-all.sh` — ALL GREEN in 3 min warm, 7.5 min cold):

| gate | measured | limit |
|---|---|---|
| MCU-3 busy ATmega328P sketch (slower of logging / stop-on-every-pin-change) | 23.4× real time (run 1: 17.3×) | ≥ 3× |
| SIM-4 garden-node engine (+ 60 Hz telemetry) | 15.9× | ≥ 1× |
| SIM-4 garden-node in the app, GUI drawing at 60 Hz (`--smoke --speed max`) | 15.95× | ≥ 1× |
| SIM-4 garden-node paced at 1× with the GUI | 1.00× | ≥ 0.97× |
| SPICE-5 1 024-node `.op` | 15.4 ms | < 50 ms |
| Workbench frame, garden-node running (UI + tessellation) | 0.73 ms | < 16.7 ms |
| PLAT-4 process start → first frame (window) | 247 ms (cold 409–478 ms) | < 2 s |

`cargo test --workspace`: 617 passed, 0 failed, 9 ignored (perf), 105 s; e2e 35 scenarios.

Left for the `ui-plug-les` follow-up (after their branch merges): GUI flows "install a plugin
from the catalog and place it" and "complete a lesson step with the math check" (TEST-5) in
`crates/breadbox/tests/`, using the same `common` harness; the headless halves of both are
covered here (`plugins.rs`, `lessons.rs`). PCB (PRO-7) / Gerber export e2e once PCB lands.

Open notes:
- ngspice and bb-spice place junction gmin slightly differently: nodes held only by picoamps
  (an off transistor's collector behind a 1 GΩ MCU stand-in) differ by tens of mV — ~3 pA,
  inside SPEC-6's 1 nA; the `.cir` comparison skips nodes that move with gmin.
- The `sim` section's note "the Pro-mode 555 has no oscillating SPICE macro-model" is resolved
  (`1fc3ac8`).

### audit
Independent spec audit + fixes (lead's phase-5 `qa` role, minus the `bundle` agent's items).
Full table: **`docs/SPEC-AUDIT.md`** (every requirement, before → after, evidence, tests).
Done / partial / missing: **73 / 26 / 1 before → 97 / 2 / 1 after** (left: `PLAT-1` by design,
`WORLD-6` needs a product decision, `PLAT-3` is the bundle agent's branch). One helper
(`audit-dbg`, worktree branch `audit-dbg-radio`, merged in `065d993`) did MCU-5 and RF-1/3/4/7.

Flaky tests — root causes fixed in product code:
- `radio_tools.rs::phone_scans_connects_subscribes_and_gets_the_button_notify`: the HM-10
  checked the CCCD when its UART buffer flushed, but the phone's subscribe only lands at the
  next 30 ms connection event; a button pressed < ~10 ms (sim time) after tapping Notify — a
  starved engine thread at Max speed, or a paused bench — lost its notification. Phone packets
  now use the master slot of a connection event and notifications the slave slot after it, and
  `notify()` honours a CCCD already queued for that event; phone log row "notifications on ·
  FFE1", "Turning on…" toggle (`05b3af0`; regression tests in bb-radio and e2e).
- `gui.rs::verify_blink_then_a_compile_error_lands_in_the_editor` (found by the 5× stress loop):
  every build of a sketch shared one staging folder keyed only by the sketch path, so the
  parallel blink debug build (or a second Breadbox instance) re-staged the clean sketch while
  arduino-cli compiled the broken one. Release / debug stage separately under an exclusive
  `StagingLock` (`95f43c0`; regression test fails 2/3 without the fix).
- 5 rounds of every GUI test file + the e2e crate under extra CPU load (load average up to
  ~250 while other agents built): only the staging race above. Final verification, after all
  fixes: 5 more rounds of gui / lessons / pcb / plugins / radio_tools / widgets / workbench +
  e2e (36 scenarios) and 5 rounds of the new audit / session / debug_memory / radio_panel
  files, all green.

`scripts/test-all.sh` (CARGO_TARGET_DIR=target/audit), two consecutive runs at `3f484fc`:
**ALL GREEN** both times — fmt, clippy, `cargo test --workspace` (690 passed, 0 failed,
9 ignored perf; 118–123 s), plugin wasm clippy + byte-identical rebuild, perf gates: MCU-3
23.7–24.0×, SIM-4 engine 16.5–16.7×, SPICE-5 12.5–13.1 ms, Workbench frame 0.68 ms, PLAT-4
first frame 133–200 ms, SIM-4 with the GUI 15.9–16.0× (max) / 1.00× (paced).

Gaps closed (✚ = also a bug fix):
- `SIM-1/3` Workbench simulation menu: Step one quantum / 1 ms, Reset bench, Reset MCU only ·
  Ux, Seed, Sync quantum (undoable `Op::Settings`, restarts the bench); Simulate menu items
  (`b5cb94b`).
- `PRJ-4` Home → Import .cir…, File → Import .cir Netlist…, opening a `.cir` (`b5cb94b`).
- `PLUG-2` sandbox violations / engine errors → error toasts on every screen (were visible only
  in the Firmware output); bounded event backlog (`bf211f0`).
- ✚ `APP-5` edits younger than the 1.2 s autosave were lost on quit and on switching projects;
  `Screen::flush` / `BreadboxApp::flush` from eframe `save` (`bf211f0`).
- `PRO-3` / `SPICE-3` all seven analysis cards; `.meas` / `.tf` results table (`e73fd12`).
- ✚ `WORLD-4` draggable hand / person / wall / magnet on the bench + World panel Objects; the
  palette's Hand tile used to drop an inert picture the sensors never read (`8024f00`).
- `WORLD-5` "Program the world": validated expression scripts, keyframe timelines with
  interpolation / repeat / preview; pushed to the running bench incl. undo (`8024f00`).
- `WORLD-1` scheduled watering ("Water automatically", exact at any time-warp) (`688635c`).
- ✚ `PLUG-5` the DMM's ⊣⊢ position (capacitor symbol) ran a diode test — now capacitance,
  60 nF–6 mF by RC timing; COM leaks to ground through 1 GΩ so a loose capacitor no longer
  floats with the meter (`3444a0e`).
- `MCU-5` Debug tab Memory (SRAM / Flash / EEPROM hex, pointers, changed bytes, Go to) + I/O
  registers; `RF-1` floor plan with draggable bench + live RSSI; `RF-3` jitter + editable AP-drop
  time; `RF-4` pcap-ng / CSV export; `RF-7` Add BLE hat wires an HM-10 (helper, `ca6e46d` …
  `afdb968`).
- New-user session (`tests/session.rs`, `docs/screenshots/session-*.png`) — friction fixed:
  ✚ new-project name field appended to the suggestion and collided on reuse (`00766c6`);
  ✚ a new Pro project edited the garden-node *demo* netlist, never saved (`cfd36a3`);
  the Bode card now reads + marks the −3 dB corner (`cfd36a3`); ✚ quiz answers overflowed
  (`d327ed9`).
- Test depth (engine sweep): SIM-6 pin over-current + reversed electrolytic, MCU-6 LEDC /
  analogWrite / micros, RF-8 annotation toggle, SPICE `mil` / `.dc oct` / `.step dec|oct`, ESP-AT
  ATE1, parts behaviour (LED colours, Zener / Schottky, MOSFETs, op-amp followers, 74HC
  gates, NTC) (`634cc72` … `5ee4fe1`).

Deliberately left / open:
- `PLAT-1`: OS-specific toolchain code lives in `bb-firmware::toolchain` (the library cannot use
  the app's `platform` module). `WORLD-6`: the toggle switches sensor noise; AVR ADC readings are
  always 10-bit integers, so "quantization off" needs a product decision.
- Serial2 looped back to the *same* ESP32's RX2 does not echo (external Serial2 ↔ HC-05 works) —
  worth a look in `bb_sim::HostMcu` (UART decoder fed by the board's own TX edges).
- Radio placement is not undoable (no `Op` for `project.radios`); network conditions are not
  saved with the project. Typing a netlist into an empty Pro project does not build a schematic
  (Import .cir does).
- Test depth still thin (listed per row in SPEC-AUDIT): scope pulse trigger / coupling, logic
  analyzer I²C / SPI / 1-Wire decoders, function-generator waveforms, WiFiServer, LCD 8-bit, a
  plugin part with a `.subckt`.
- Changes outside "my" files were all for the audited behaviour: `bb-radio` (BLE timing),
  `bb-world` (objects, scripts check, watering), `bb-circuit` (`Op::Settings`),
  `plugins/dmm-handheld` (+ rebuilt wasm), docs.


### bundle
Phase 5 — `Breadbox.app` (`PLAT-3`), runtime resource resolution, start-up (`PLAT-4`),
README, polish (`APP-4`, `MCU-4` first run, PCB silkscreen, glyphs). Contracts: ARCHITECTURE
§8 ("Resources & user data", "Commands + shortcuts") and §10.

- `PLAT-3` **done** — `scripts/bundle-macos.sh [--dmg] [--check]` → `dist/Breadbox.app` (44 MB;
  `dist/Breadbox.dmg` 19 MB): release build with `BREADBOX_BUNDLE=1` + remapped paths (own
  target dir), `Info.plist` (`app.breadbox.Breadbox`, workspace version, macOS 14+, high-DPI,
  dark mode, document types: `.breadbox` project folders, `.breadbox.zip`, folders as
  Alternate), `Breadbox.icns` rendered from the design's logo mark
  (`assets/icon/breadbox-icon.svg`, `app_icon` example → `iconutil`), `Contents/Resources`
  (examples + fixtures, lessons + firmware, plugins + wasm + art + sources, the plugin SDK
  crate, docs, fonts + OFL, the ESP32/Pico host core, `licenses/THIRD-PARTY.md`,
  `build-info.json`), ad-hoc `codesign`; `--dmg` via `hdiutil`.
- Runtime resources **done** — `bb_core::paths` is the one resolver (bundle →
  `Contents/Resources`, never the checkout; dev → the repo, compiled out of bundle builds; user
  data in `~/Library/Application Support/Breadbox` + `~/Library/Caches/Breadbox`, overridable
  with `BREADBOX_APP_SUPPORT` / `BREADBOX_CACHES`). bb-lessons, bb-plugin, bb-firmware and the
  app (`platform::resources()`) go through it; eframe state moved to
  `Application Support/Breadbox/app-state.ron` (copied once from the old folder). Examples open
  read-only from the bundle, Save / Save As copy them out.
- "No repo paths at runtime" **proven** — `scripts/check-bundle.sh`: no `$HOME` path in the
  binary or bundled files; the copied app runs under `sandbox-exec` with the checkout denied,
  from a scratch cwd with scratch user data:
  `garden-node` (bundled path) 5 s headless `realtime=15.8 status=ok`; `lesson:ohms-law-led`
  `instruments=DMM1:13.97mA status=ok`; `garden-node --place community.dmm-handheld`
  `instruments=DMM1:1.878V status=ok`; `esp32-weather` host-compiled against the bundled host
  core `status=ok`; garden-node in a real wgpu window `first_frame_ms≈450–600 status=ok`; the
  checkout untouched. A control run of the dev binary under the same sandbox finds no examples.
- `PLAT-4` **done (bundle)** — `open Breadbox.app` → first frame **143–175 ms** after process
  start, **~320 ms** from `open`; quit. `open -a Breadbox.app <project folder>` opens it
  (Apple Event `odoc` handler, installed at `NSApplicationWillFinishLaunching` so launch
  documents arrive too; `breadbox <path>` on the command line).
- `--smoke` grew `lesson:<id>` (a lesson's own bench) and `--place <plugin id>` (a plugin
  instrument placed like "Place on bench"; readings in the SMOKE line as `instruments=`).
- `APP-4` menu bar **audited + fixed** — `platform::MENUS` is the one table; `tests/menu.rs`
  sends every item through its command on the real app and checks the effect. Fixes: Edit →
  Cut / Copy / Paste / Select All were predefined items with no responder (greyed out) — now
  real items (`ViewportCommand::RequestCut/Copy/Paste`, Select All into the focused field); the
  bench copied/pasted parts only on ⌘C/⌘V *key* events, which egui-winit never sends (it sends
  `Event::Copy/Paste`) — now both, plus Cut; File → Import Project Bundle (Open only picks
  folders); About → the standard macOS panel; Help → a Help window with the guide, Plugin SDK
  and lesson format (was: a Finder folder). Menu → text-field key injection moved from eframe's
  `raw_input_hook` into the frame (testable).
- `MCU-4` first run **verified end to end** — `tests/toolchain_first_run.rs` simulates a Mac
  without a toolchain (`BREADBOX_ARDUINO_CLI` → nowhere now skips PATH/Homebrew; scratch
  Application Support): Verify offers the install, an offline install fails cleanly and can be
  retried; `--ignored`: the real one-click install (arduino-cli 1.6.0 + arduino:avr + native
  avr-gcc 8.5.0 + ctags built from source, 29 s) then Verify compiles blink. The user's own
  toolchain is never touched. `BREADBOX_ARDUINO_CLI_URL` overrides the download.
- Lessons: the lesson bench **pauses** when another screen opens (`Screen::on_leave`) and
  resumes on return (test). PCB: **reference designators** on the F_SilkS Gerber (stroke font,
  where the screen shows them; read back in the test).
- Glyphs **checked** — `tests/glyphs.rs` scans string/char literals of every crate + plugin
  source, parts / lesson / plugin / example data, example + lesson sketches and the rendered
  docs, and fails on any character a font family can't draw. Fixes: `∥` → `‖` (Inter has no
  `∥`; data, docs, DMM + scope plugins), waveform switch symbols `∿ ⊓ ⋀ ⎍` drawn as shapes
  like `⎓`, LCD unknown char `□`, docs rendered through `markdown::displayable`, the
  plugin.json pane escapes glyphs the code font lacks, plugin SVG art renders its text with the
  bundled fonts (its font database was empty — e.g. "RYLR896" never showed).
- `scripts/build-plugins.sh` remaps the checkout path (the SDK path dependency leaked
  `~/…/breadbox/crates/…` into every module, so `--check` failed from any other checkout);
  all wasm rebuilt, `--check` is green.
- README.md written (what it is, screenshots, requirements + the no-Rosetta toolchain path,
  build/test/bundle, layout, first plugin, make your own lesson).
- Firmware staging is keyed by sketch **and** build flavour (debug `-Og` vs Verify), so the
  two never overwrite one staging folder (this raced in `tests/gui.rs`: the compile-error test
  clobbered the breakpoint test's sources).

Changes outside the bundle scope (small, tests green): bb-core (`paths`, additive),
bb-firmware (`app_dir`/`build_cache_root` via `paths`, `BREADBOX_ARDUINO_CLI` semantics,
`BREADBOX_ARDUINO_CLI_URL`), bb-lessons / bb-plugin (dirs via `paths`), bb-sim (LCD `□`),
bb-circuit (silkscreen references), plugins (`‖`, rebuilt wasm), `docs/PLUGIN-SDK.md` +
`docs/plugin.schema.json` (`‖`), `lessons/1-electricity/05-voltage-dividers.toml` (`‖`),
`crates/breadbox` app files (`app.rs`, `platform.rs`, `router.rs`, `canvas/bench.rs`, …).
Not touched: `tests/e2e/**`, `scripts/test-all.sh`, `tests/gui.rs`, `tests/workbench.rs`,
`screens/pro/mod.rs`.

Open issues / notes:
- `scripts/check-bundle.sh` is not in `scripts/test-all.sh` (owned by `e2e`); suggested:
  `scripts/bundle-macos.sh --check` as an optional (slow, ~3 min) stage.
- The bundle is ad-hoc signed (no Developer ID / notarization): on another Mac Gatekeeper asks
  for right-click → Open the first time. JIT (wasmtime) needs no entitlement without the
  hardened runtime.
- `.breadbox` project folders are plain folders in Finder (double-click navigates in); they
  open in Breadbox via Open With, the Dock icon or `open -a`. A multi-dot extension
  (`.breadbox.zip`) may not bind in LaunchServices — File → Import Project Bundle works.
- The menu bar could not be inspected from outside here (no Accessibility permission for
  scripted UI); `tests/menu.rs` covers each item's behaviour through its command.
- `Help` shows docs as text (images in the README are skipped).
