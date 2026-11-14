# Breadbox — Product Spec v1

> A very fun, very serious circuit playground. One app that is both a real SPICE workbench for
> seasoned engineers and a virtual electronics bench for hobbyists: breadboards, jumper wires,
> Arduinos running *real compiled firmware*, sensors sitting in a pot of soil, radios talking to a
> virtual network, instruments that behave like the real ones, and lessons that let you do the
> math yourself. Everything is extensible through sandboxed plugins.

This file is the **definition of done**. Every numbered requirement has an ID (e.g. `SIM-3`).
Work is complete when every requirement here is implemented, wired into the app, and covered by
an automated test listed in §11. Design reference: `docs/design/` (screenshots + source HTML of
the five screens). Architecture: `docs/ARCHITECTURE.md`. Visual tokens: `docs/DESIGN.md`.

---

## 0. Platform

- `PLAT-1` Written in Rust (stable). Native macOS app (Apple Silicon first, macOS 14+). Code stays
  portable: OS-specific code only behind `breadbox::platform`.
- `PLAT-2` GUI: `eframe`/`egui` (wgpu backend). Bundled fonts: Inter (UI), JetBrains Mono
  (numbers, code, console), Space Grotesk (brand + large headings). All OFL; license files shipped.
- `PLAT-3` Ships as `Breadbox.app` via `scripts/bundle-macos.sh` (Info.plist, icon, fonts,
  first-party plugins, lessons, example projects inside `Contents/Resources`).
- `PLAT-4` Starts in < 2 s to an interactive window on an M-series Mac (release build).
- `PLAT-5` Never use sparkle / magic-wand / four-point-star icons anywhere (see `CLAUDE.md`).

## 1. App shell & navigation

- `APP-1` Top bar on every screen: logo mark + "Breadbox" (Space Grotesk) / breadcrumb (project or
  section) / mode badge (`Playground` or `Pro`). Clicking "Breadbox" opens **Home**.
- `APP-2` **Home**: new project (Playground or Pro), open project (folder picker), recent projects,
  example projects gallery, Lessons, Plugins. Keyboard: ⌘N, ⌘O, ⌘S, ⌘⇧S, ⌘Z, ⌘⇧Z, ⌘W.
- `APP-3` Screens: Workbench, Firmware, Pro (Schematic / Analysis / PCB), Plugins, Lessons —
  matching `docs/design/screenshots/*` in layout, colour, typography and density.
- `APP-4` Native macOS menu bar (File / Edit / View / Simulate / Window / Help) mirroring the
  shortcuts. Dark theme by default; light theme token set exists (`View → Theme`).
- `APP-5` Every project autosaves (debounced) and restores window/screen state on relaunch.

## 2. Workbench (design screen 1)

- `WB-1` Header: Breadboard ⇄ Schematic segmented toggle; Run/Pause button (amber); sim clock
  `t = mm:ss.mmm`; speed (0.1×, 0.5×, 1× real-time, 2×, 10×, Max); run status dot
  (`running` green / `paused` / `stopped` / `error` red); **Share** (export menu, see `PRJ-5`).
- `WB-2` Left **Parts** panel: `Simple` (25 starter parts) / `Advanced` (full library) toggle,
  search, category sections (Boards, Building blocks, Light & sound, Sensors, Power, Radios,
  Displays, Motors & actuators, Semiconductors, ICs, Instruments, Plugins). Tiles drag onto the
  bench. Hover shows a one-line description.
- `WB-3` **Breadboard view** (the "hobbyist 3D" look): 2.5D top-down illustrated bench with soft
  shadows: 830-tie-point and 400-tie-point breadboards (rows a–j, columns, two power rail pairs,
  rail colour stripes, column numbers), boards (UNO R3, Nano, Mega 2560, ESP32 DevKit, Pico) with
  headers, parts whose leads snap into holes, off-board objects (pot of soil, lamp, the bench
  instruments). Zoom (pinch/⌘scroll, 25–400 %, readout in the header bar), pan (scroll/space-drag),
  grid background per design.
- `WB-4` **Jumper wires**: drag from any pin or hole to any pin or hole; wire sags as a bezier with
  highlight + shadow; colour picker (10 standard dupont colours; auto-colour: red for +, black for
  GND); bend points draggable; delete; wires and leads connected to the same net highlight on hover.
- `WB-5` Editing: select, multi-select (box + ⇧), move (snaps to 0.1" pitch on boards), rotate (R),
  flip (F), delete, duplicate (⌘D), copy/paste, undo/redo (unlimited, per-project), edit
  properties inline (resistor value with colour bands updating live, LED colour, etc.).
- `WB-6` Live **net labels / probes**: pinned floating labels like `Night LED · D9 %`,
  `Light sensor → A1 V`, `Soil sensor → A0 V` showing live values while running; add by clicking a
  pin with the probe tool; hover any lead to see its net voltage + current.
- `WB-7` **Schematic view**: the *same* circuit drawn with standard symbols (IEC/ANSI toggle),
  orthogonal wires, junction dots, net labels, power/ground symbols, reference designators
  (R1, D1, U1, J1). Auto-placement for parts never placed in schematic; manual placement persisted.
  Editing in either view updates the shared netlist. Connections that exist logically but are not
  physically wired on the breadboard render as dashed ratsnest lines in breadboard view (and the
  reverse: physical wiring always shows up in the schematic).
- `WB-8` Right rail (icon buttons, design): World, Instruments, Probes, Inspector, Waveforms. Panels
  collapse/expand (`<|` / `|>` handles).
- `WB-9` **World panel**: sliders with named stops (Soil moisture: Bone dry / Field capacity /
  Saturated, % VWC; Ambient light: Night / Living room / Bright office, lux; Air temperature °C;
  Humidity %RH), checkboxes: Evaporation (rate shown, %/h), Day/night cycle (location picker,
  default Ashland, OR), Sensor noise & ADC quantization. "Water the pot" action. World time-warp.
- `WB-10` **Inspector**: selected part's real-world properties (e.g. *Capacitive soil sensor v1.2*:
  Supply 3.3–5.5 V, Output (dry→wet) 3.0→1.4 V, Response τ ≈ 1.8 s, Noise ±4 LSB,
  Model `soil.vwc → C(ε)`), editable parameters, datasheet-style notes, part health (OK / stressed /
  damaged).
- `WB-11` Bottom dock tabs: **Serial monitor** (timestamped lines, colour-coded: sim/system grey,
  radio/net cyan, warnings amber; port label `U1 · SoftwareSerial + USB · 115200 baud · NL`; line
  ending + baud selectors; send box; Clear; autoscroll; per-MCU tabs when several boards) and
  **Probes & power** (table of all probes with live value/min/max/avg, supply rails voltage +
  current, total power, per-part dissipation, current budget vs USB 500 mA).
- `WB-12` Status strip on canvas: `Bench · BB1 + U1 + pot 1 — drag wires from any pin`, ERC
  (`ERC 1 warning`), DRC (`DRC clean`) with click-to-list, zoom %.
- `WB-13` Fun mode physics: LEDs glow with brightness ∝ current (bloom), RGB LEDs mix, buzzers
  play audio (real pitch from the simulated waveform), motors spin at simulated speed, servos
  move, LCD/OLED show their simulated framebuffer, 7-segment shows segments, relays click.
  Overstress → **magic smoke** animation + part marked damaged (LED overcurrent, resistor
  over-power, reverse electrolytic, MCU pin > 40 mA, shorted supply → USB polyfuse trips).
  Damaged parts stay damaged until replaced or "Repair all".

## 3. Parts library (built in)

Each part = pins + electrical model + breadboard art + schematic symbol + inspector properties.

- `PART-1` Boards: Arduino UNO R3, Arduino Nano, Arduino Mega 2560 (AVR, cycle-accurate),
  ESP32 DevKit v1, Raspberry Pi Pico (API-level emulation, see `MCU-6`).
- `PART-2` Building blocks: breadboard (830, 400), jumper wire, resistor (E-series picker, colour
  bands, power rating), capacitor (ceramic, electrolytic with polarity), inductor, push button,
  slide switch, DIP switch, potentiometer (knob, interactive), trimmer, battery pack (AA×2/×4,
  9 V, LiPo 1S with internal resistance + discharge curve), USB power (5 V, 500 mA polyfuse),
  DC barrel jack.
- `PART-3` Light & sound: LED (red, amber, yellow, green, blue, white — each with its own
  Vf/Is/n model), RGB LED (common cathode/anode), 7-segment display, LED bar graph, WS2812
  (NeoPixel) strip, passive buzzer, active buzzer, small speaker.
- `PART-4` Sensors: LDR GL5528 (`R ≈ 350k·lux^-0.7`, ±10 %), capacitive soil sensor v1.2,
  resistive soil probe, TMP36, DHT11, DHT22 (single-wire protocol, bit-accurate timing),
  DS18B20 (1-Wire), BME280 (I²C), HC-SR04 ultrasonic (needs a distance in the world), PIR
  motion, tilt switch, hall sensor, microphone module (analog), thermistor 10k NTC.
- `PART-5` Semiconductors: diodes (1N4148, 1N4007, Schottky 1N5819, Zener 5V1), BJTs (2N2222,
  2N3904, 2N3906, TIP120), MOSFETs (2N7000, IRLZ44N), op-amps (LM358, TL072, ideal), 555 timer,
  74HC595 shift register, 74HC00/04/08/32 logic, ULN2003, L293D, 7805 + AMS1117-3.3 regulators,
  optocoupler PC817.
- `PART-6` Displays & actuators: 16×2 HD44780 LCD (4/8-bit, I²C backpack PCF8574), SSD1306
  128×64 OLED (I²C), DC motor (back-EMF model), SG90 servo (pulse-width → angle, with slew),
  28BYJ-48 stepper, relay module, vibration motor.
- `PART-7` Radios (see §7): ESP-01 ESP8266 Wi-Fi (AT firmware), HM-10 BLE (AT), HC-05 Bluetooth
  SPP, plus radios from plugins (LoRa RYLR896 in the first-party LoRa pack).
- `PART-8` Simple library = the 25 starter parts from the design: UNO R3, ESP32, Pico,
  breadboard, jumper wire, resistor, capacitor, push button, knob (pot), battery pack, USB power,
  LED, RGB LED, buzzer, LDR, capacitive soil sensor, TMP36, DHT22, servo, DC motor, 16×2 LCD,
  ESP-01 Wi-Fi, HM-10 BLE, transistor (2N2222), diode.
- `PART-9` Part data is declarative (`bb-circuit` part definitions in RON/JSON + a SPICE model or
  behavioural model id). Plugin parts use the identical schema.

## 4. Analog simulation — SPICE engine (`bb-spice`)

- `SPICE-1` Netlist parser for SPICE3/ngspice-compatible syntax: title line, comments (`*`, `;`,
  `$`), continuation `+`, case-insensitive, SI suffixes (f p n u µ m k meg g t, `mil`), `{expr}`
  and `'expr'` parameters, `.param`, `.func`, `.model`, `.subckt`/`.ends` (with params,
  nested), `.include`, `.lib`, `.global`, `.temp`, `.options`, `.ic`, `.nodeset`, `.end`.
- `SPICE-2` Elements: R (with tc1/tc2, temp), C (with IC), L (with IC), K (mutual coupling),
  V / I (DC, AC mag/phase, PULSE, SIN, EXP, PWL incl. repeat, SFFM, AM), E/F/G/H controlled
  sources (incl. POLY(1)), B behavioural sources (arbitrary expressions of node voltages, branch
  currents, time, with `if`, `min`, `max`, `abs`, `pwr`, `pow`, `exp`, `ln`, `log`, `log10`,
  `sqrt`, trig, `u()` step, `limit`, `table`), S/W voltage/current-controlled switches (with
  hysteresis), D (SPICE diode: IS, N, RS, CJO, VJ, M, TT, BV, IBV, EG, XTI, KF/AF ignored),
  Q (Gummel-Poon BJT: IS, BF, BR, NF, NR, VAF, VAR, IKF, IKR, ISE, NE, ISC, NC, RB, RE, RC, CJE,
  CJC, TF, TR, …), M (MOSFET level 1 Shichman-Hodges with body effect + channel-length
  modulation + Meyer caps; level 3 accepted with level-1 fallback + warning), J (JFET), T (lossless
  transmission line), X (subcircuit instances), plus Breadbox "digital port" elements used by
  co-simulation (see ARCHITECTURE §4).
- `SPICE-3` Analyses: `.op`; `.dc` (source, parameter, temperature; lin/dec/oct/list; nested
  2-level); `.tran` (adaptive time step with LTE control, breakpoints from sources, trapezoidal +
  Gear-2 + backward-Euler, `UIC`, `tmax`); `.ac` (lin/dec/oct) with complex small-signal models;
  `.step` (param lin/dec/oct/list, nested); `.mc` Monte Carlo (per-element tolerances from
  `tol=` or global `.mc ... tol=5%`, gaussian/uniform, seed, N runs; results as distributions);
  temperature sweep; `.meas` (avg, rms, pp, min, max, integ, deriv, find…at, when, trig/targ);
  `.tf`; `.noise` (resistor thermal + diode/BJT shot noise).
- `SPICE-4` Convergence: Newton-Raphson with pnjlim/fetlim limiting, gmin stepping, source
  stepping, automatic timestep cut on non-convergence, `.options reltol abstol vntol gmin itl1
  itl4 method`. Clear diagnostics naming the offending node/element.
- `SPICE-5` Solver: sparse LU with Markowitz pivoting and symbolic reuse across iterations; dense
  fallback for tiny systems. 1 000-node circuits solve `.op` in < 50 ms.
- `SPICE-6` Accuracy: a reference suite of ≥ 30 circuits (`crates/bb-spice/tests/reference/`)
  is compared against ngspice output (checked-in golden data, regenerable with
  `scripts/regen-spice-golden.sh` when ngspice is installed): `.op`/`.dc` within 0.1 % (or
  1 µV/1 nA), `.ac` within 0.5 % magnitude / 0.5° phase, `.tran` waveforms within 1 % normalised
  RMS error.
- `SPICE-7` Incremental co-simulation API: a transient session that can be advanced to an
  arbitrary time, accepts source/port value changes between steps, reports node voltages / branch
  currents at any time, supports threshold-crossing watches, and can save/restore state.
- `SPICE-8` Export: ngspice-compatible `.cir`; results as CSV and raw (ASCII rawfile).

## 5. Microcontrollers & firmware

- `MCU-1` **AVR core** (`bb-avr`): ATmega328P (UNO, Nano) and ATmega2560 (Mega). Every
  instruction implemented with datasheet-exact cycle counts and flag behaviour; 16 MHz default
  clock. Validated by instruction-level unit tests and by running real `arduino-cli`-compiled
  sketches.
- `MCU-2` Peripherals: GPIO (DDR/PORT/PIN incl. PIN-write toggle), external interrupts INT0/1
  (+INT2–7 on 2560), pin-change interrupts, Timer0/1/2 (+3/4/5 on 2560) all waveform modes
  (normal, CTC, fast PWM, phase-correct, phase-frequency-correct), OCnx output compare pins,
  input capture, prescalers; USART0 (+1–3 on 2560) with interrupts, U2X, frame formats; ADC
  (10-bit, prescaler-accurate 13/25-cycle conversion, AVcc / internal 1.1 V / AREF, auto-trigger,
  free-running, temperature sensor channel); SPI (master + slave); TWI/I²C (master + slave);
  EEPROM (with write timing); watchdog (reset + interrupt); sleep modes; analog comparator;
  brown-out detection tied to simulated supply voltage; reset pin.
- `MCU-3` Performance: ≥ 3× real-time for a busy 16 MHz ATmega328P sketch in isolation on an
  M-series Mac (release build), so co-simulation can sustain 1× with analog + devices.
- `MCU-4` **Toolchain**: `bb-firmware` locates or installs `arduino-cli` (Homebrew path, PATH,
  or downloaded into `~/Library/Application Support/Breadbox/toolchain/`) and the `arduino:avr`
  core (avr-gcc 7.3.0). Compiles multi-file sketches (`.ino` + `.h`/`.cpp` tabs) with libraries
  (core libs: SoftwareSerial, Wire, SPI, EEPROM, Servo; library manager search + install, e.g.
  ArduinoJson 7). Output panel reproduces arduino-cli's size report ("Sketch uses 9,842 bytes
  (30%) of program storage space…"). Compiler errors map to editor diagnostics (file, line, col).
  Build cache for fast re-verify. If no toolchain is present the app offers a one-click install
  with progress.
- `MCU-5` **Debugger**: breakpoints by source line (DWARF line table from the ELF), conditional
  hit counts ("breakpoint armed on U1 (hits: 24)"), pause / continue / step over / step into /
  step out (by source line), current line highlight in the editor, call stack (frame-pointer +
  DWARF CFI best effort), registers + SREG flags, globals and locals of basic types (DWARF),
  SRAM/flash/EEPROM hex views, I/O register view. Flashing preserves bench state ("Flashed
  virtual U1 in 0.21 s · core reset · bench state kept").
- `MCU-6` **ESP32 DevKit & Raspberry Pi Pico** — API-level emulation: the sketch is compiled with
  the system C++ compiler against Breadbox's Arduino-compatible host core
  (`crates/bb-firmware/hostcore/`) and runs in a child process in lock-step with simulated time
  via a pipe protocol. Supported API: pinMode/digitalWrite/digitalRead/analogRead (12-bit ESP32,
  12-bit Pico)/analogWrite + ledc* (ESP32)/attachInterrupt/millis/micros/delay/
  delayMicroseconds/Serial/Serial1/Wire/SPI/WiFi (ESP32: `WiFi.h`, `WiFiClient`, `WiFiServer`,
  `HTTPClient` subset, `WiFiUdp`)/MQTT via `PubSubClient`-compatible header/BLE (ESP32 BLE
  Arduino subset: `BLEDevice`, `BLEServer`, `BLEService`, `BLECharacteristic`, notify)/
  Preferences. UI labels these cores "API-level" (vs "cycle-accurate AVR core").
- `MCU-7` Multiple MCUs per bench, each with its own firmware, serial monitor tab and debugger.

## 6. Mixed-signal co-simulation (`bb-sim`)

- `SIM-1` One engine advances analog circuit, MCUs, behavioural devices, world and radios in
  lock-step. Default sync quantum 1 µs (configurable); event-driven breakpoints so digital edges,
  ADC samples, UART bits and device wake-ups land on exact simulated times (cycle-exact for AVR).
- `SIM-2` Pin electrical model: outputs as Thevenin sources (AVR: ≈ 25 Ω, voltage sag per
  datasheet at high current), inputs with Schmitt thresholds (0.3/0.6 Vcc) + leakage, internal
  pull-ups (≈ 35 kΩ), ADC sample-and-hold, 10-bit quantization, optional ±noise.
- `SIM-3` Real-time pacing (0.1×–10× and Max), pause/step (single quantum, single ms), reset,
  "reset MCU only", deterministic replay given a seed.
- `SIM-4` The garden-node example (UNO + ESP-01 + soil sensor + LDR divider + 2 LEDs + mini
  scope + DMM) runs at ≥ 1× real-time in release builds, with GUI open.
- `SIM-5` Telemetry to the UI at 60 Hz: probe values, part visual states (LED brightness,
  motor speed, LCD buffer…), waveform ring buffers for scopes/logic analyser, serial streams,
  radio packet log, ERC/health events.
- `SIM-6` Damage + supply modelling as in `WB-13` (USB polyfuse 500 mA hold, brownout below
  BOD level resets the AVR, batteries discharge and sag).
- `SIM-7` Firmware re-flash keeps bench state; analog state and world persist across MCU resets.

## 7. Radios — virtual RF space (`bb-radio`) (design screen 2, right)

- `RF-1` Virtual RF space with one or more access points (default `SIM-AP`, WPA2 password
  `breadbox`, channel 6). RSSI from distance + walls (log-distance path loss); configurable
  station placement.
- `RF-2` Network services: DHCP (10.0.0.0/24, gateway 10.0.0.1, lease 3600 s), DNS (`*.sim`
  names + `broker.sim`), built-in MQTT 3.1.1 broker at `10.0.0.1:1883` (QoS 0/1, retained,
  wildcards, keep-alive, LWT), HTTP echo/test server, NTP (sim clock), ICMP ping.
- `RF-3` Network conditions: latency ± jitter (ms), packet loss %, "drop the AP at t = N s
  (test reconnect)", "bridge to my real LAN broker" (host:port; bidirectional TCP bridge for MQTT
  traffic so real dashboards see simulated devices).
- `RF-4` Packet log: time, dir (tx/rx/!), frame summary (DHCP REQUEST/ACK, MQTT CONNECT/PUBLISH
  with size + QoS, TCP SYN…); fault-injector drops shown as amber `!` rows; export as pcap-ng
  (synthetic IP frames) and CSV.
- `RF-5` ESP-01 ESP8266 module speaking ESP-AT over its UART pins at its configured baud: AT,
  ATE0/1, AT+RST, AT+GMR, AT+CWMODE, AT+CWJAP, AT+CWQAP, AT+CWLAP, AT+CIFSR, AT+CIPSTA?,
  AT+CIPMUX, AT+CIPSTART (TCP/UDP), AT+CIPSEND (incl. `>` prompt), AT+CIPCLOSE, AT+CIPSTATUS,
  +IPD delivery, AT+PING, AT+UART_CUR, AT+CIPSNTPCFG/AT+CIPSNTPTIME?, AT+MQTTUSERCFG/
  AT+MQTTCONN/AT+MQTTPUB/AT+MQTTSUB (ESP-AT MQTT). Realistic timing (boot banner garbage at
  74880 baud then `ready`, command latencies, `busy p...`, `WIFI CONNECTED`/`WIFI GOT IP`).
- `RF-6` HM-10 BLE module (AT command set + transparent UART) and HC-05 SPP module. A **virtual
  phone** acts as central: scan list with RSSI, connect, discover services, read/write/notify
  GATT characteristics, Bluetooth serial terminal. ESP32 native BLE (via `MCU-6`) appears to the
  same virtual phone.
- `RF-7` Radio panel UI per design: per-radio cards (status chip, AP, RSSI, IP, broker), packet
  log, network-conditions sliders and checkboxes, Bluetooth LE card ("No BLE radio on this bench.
  Add a BLE hat…" empty state with Add button), MQTT explorer (subscribe/publish to the built-in
  broker).
- `RF-8` Serial monitor annotations for radio events (`wifi: AT+CWJAP="SIM-AP" → OK rssi=-54 dBm
  ip=10.0.0.42`, `mqtt: PUB garden/pot1 {...} (40 B)`), toggleable.

## 8. World models (`bb-world`)

- `WORLD-1` Pot of soil: volumetric water content state; evaporation driven by temperature,
  humidity, light; drainage above field capacity; watering events (button, scheduled, or
  firmware-driven pump/relay part watering the pot through a "tube" connection); soil sensors
  read VWC through their transfer functions + response time constant.
- `WORLD-2` Light: ambient lux slider or day/night cycle from solar elevation at a location
  (lat/long, date, cloud cover); bench light sources (LEDs, lamps) add lux to nearby light
  sensors with inverse-square falloff (an LED next to the LDR measurably changes its reading).
- `WORLD-3` Air temperature + humidity with diurnal cycle; thermal sensors read with their own
  time constants; parts dissipating power heat up (self-heating affects resistors' tempco and the
  TMP36 if placed near).
- `WORLD-4` Distance/motion objects for HC-SR04 / PIR (draggable "hand" object), tilt, magnet.
- `WORLD-5` Programmable world: per-variable keyframe timeline and expression scripts
  (`moisture = 45 - 0.8*t_hours` style) and world-model plugins; world time-warp independent of
  circuit speed.
- `WORLD-6` Sensor noise + ADC quantization toggle (global) and per-sensor noise parameters.

## 9. Pro mode (design screen 3)

- `PRO-1` Tabs: Schematic (the full schematic editor, `WB-7`), Analysis, PCB.
- `PRO-2` Left: netlist editor (`garden-node.cir — synced from schematic`), syntax highlighted,
  editable; edits that parse round-trip into the circuit where representable, otherwise the
  netlist is marked "detached" with a re-sync button.
- `PRO-3` Analyses sidebar: cards for Transient, DC sweep, AC/Bode, Monte Carlo, Temperature
  sweep, Noise, Op point — each configurable, enable/disable; "Run analyses" runs all enabled in
  a background thread with progress; header shows `3 steps · 1.82 s solve · 0 convergence
  warnings`.
- `PRO-4` Plot panels: interactive waveform plots (pan/zoom, cursors with Δt, frequency, duty
  readouts, multiple traces with legend, dual Y axes for V/I, log axes for DC/AC sweeps,
  `.step` families, Monte Carlo histograms with threshold markers), operating-point table with
  a "bench probe" column comparing against live bench measurements.
- `PRO-5` Mixed-signal note panel: sync step, solver, integration method; mixed-signal transient
  can run the real firmware against the analog circuit (`XU1 ... ATMEGA328P fw=garden-node.hex`).
- `PRO-6` Export `.cir`, CSV of any plot, PNG/SVG of plots and schematic.
- `PRO-7` **PCB (basic)**: footprints per part (THT + common SMD), board outline, placement,
  ratsnest from the netlist, manual two-layer trace routing with vias, design-rule check
  (clearance, trace width, unrouted nets), Gerber RS-274X + Excellon drill export.

## 10. Plugins & instruments (design screen 4)

- `PLUG-1` Plugin = folder or `.bbplugin` zip with `plugin.json` (id, version, name, kind[],
  provides, spice, specs, permissions, entry, assets). Kinds: `instrument`, `part`, `board`,
  `radio`, `world-model`, `generator`, `analysis`, `theme`, `lesson-pack`.
- `PLUG-2` Behaviour code runs in a WebAssembly sandbox (wasmtime, fuel + epoch interruption,
  memory caps). Plugins touch the circuit only through the permissions they declare
  (`read:nets`, `inject:load`, `drive:pins`, `world:read`, `world:write`, `radio:space`,
  `ui:panel`, `fs:project`, `net:lan`). Permission prompt on install; violations are errors.
- `PLUG-3` Rust guest SDK `bb-plugin-sdk` (no_std-friendly, macros for exports) + ABI documented
  in `docs/PLUGIN-SDK.md` so any wasm language (AssemblyScript, Zig, C) can target it.
- `PLUG-4` Plugins screen: category nav (Instruments, Parts & boards, Radios & networks, World
  models, Solvers & analyses, Views & themes), installed list, search, detail page (art, kind
  chips, specs grid, live callouts such as "Probing A0 (J1 out): meter load shifts the node by
  -0.4 mV — shown as it would be on a real bench."), "Place on bench", "Edit model",
  read-only `plugin.json` pane, "More to add" catalog with Install, "Load local plugin…",
  "Plugin SDK docs". Hot-reload of local plugins on file change.
- `PLUG-5` First-party plugins (in `plugins/`, built to wasm, shipped in the app, installable from
  the catalog):
  - **Handheld DMM** — 6000 counts, DC V ±(0.05 % + 1), input 10 MΩ ‖ 100 pF loads the circuit,
    10 A burden 0.03 V/A, auto-range 350 ms, ranges 600m/6/60/600 V, last-digit flicker from
    noise, true-RMS AC, modes V⎓ V~ mV Ω (with test current) ⊣⊢ continuity (beep) A mA Hz,
    HOLD / MIN MAX / RANGE, rotary dial, probes clip to any two nodes; series current
    measurement requires wiring the meter in series (per lesson screen).
  - **4-channel oscilloscope** — timebase, V/div, coupling, 1×/10× probes with compensation,
    edge/pulse triggering (auto/normal/single), measurements, cursors, FFT.
  - **Logic analyzer** — 16 channels, UART/I²C/SPI/1-Wire decoders.
  - **Bench power supply** — CV/CC, current limit, foldback, sag curve, output enable.
  - **Function generator** — sine/square/triangle/pulse/noise/arbitrary, 50 Ω output.
  - **Soil & weather pack** (world model) — evaporation, drainage, rain events, sun position.
  - **LoRa + virtual gateway** (radio) — RYLR896 UART module, path loss by distance and terrain,
    airtime from SF/BW/CR, duty-cycle limits, gateway → MQTT bridge.
- `PLUG-6` The built-in mini oscilloscope on the bench (2-channel, design screen 1) is a core
  instrument; plugin instruments render on the bench from their declared art + panel layout.
- `PLUG-7` A plugin can add: parts (with SPICE subcircuits and/or wasm behaviour), custom signal
  generators, analyses (consume the netlist + call the solver), world models, radios, boards
  (declarative AVR board variants, e.g. Pro Mini), themes, lesson packs.

## 11. Lessons (design screen 5)

- `LES-1` Lesson tracks: **Track 1 · Electricity you can see** (8 lessons: What is a circuit?;
  Volts, amps, ohms; Ohm's law with an LED; Series & parallel; Voltage dividers; Capacitors &
  time; Buttons & pull-ups; Your first sketch: blink), **Sensors & the real world**,
  **Microcontrollers from zero**, **Radios & IoT**, **Pro: SPICE analysis** (each ≥ 4 lessons),
  and **Make your own lesson**.
- `LES-2` Lesson layout per design: track nav with progress checkmarks, live bench canvas with
  the lesson circuit, step panel (step bar `Step 3 of 5`, title, explanation, formula card,
  interactive choices like "Pick R1", "Do the math myself" checkbox → prediction input with
  units + Check + "Show me the working", "Your measurements" table: R1 / you said / meter,
  "Try 3 more values", Back / Next).
- `LES-3` Step kinds: explain, build (goal = connectivity/part checks), predict (numeric answer
  with tolerance + units; the meter "waits for your prediction" when math mode is on), measure
  (instrument reading recorded), experiment (change a value and observe), break-it-on-purpose
  (damage events as goals), quiz (multiple choice), code (sketch must compile + behaviour
  checks, e.g. LED blinks at 1 Hz ± 5 %).
- `LES-4` Lessons are data files (`lessons/<track>/<lesson>.toml`) with starting circuit, steps,
  checkers, hints and worked solutions. Progress persists per user.
- `LES-5` "Open in free play" copies the lesson bench into a Playground project.
- `LES-6` **Make your own lesson**: an in-app lesson editor (write steps, capture the current
  bench as the starting circuit, add checkers by picking parts/nets/instruments) that saves a
  lesson pack shareable as a plugin (`kind: lesson-pack`).

## 12. Projects & files

- `PRJ-1` Project = folder `<name>.breadbox/` with `project.json` (circuit, views, world, radios,
  instruments, settings, version) + `firmware/<MCU ref>/` sketch folders + `analyses/`.
  Versioned schema with migrations.
- `PRJ-2` Example projects: garden-node (from the design), blink, night-light (LDR + transistor),
  thermometer (TMP36 + LCD), ble-button (HM-10 + virtual phone), esp32-weather (ESP32 Wi-Fi +
  DHT22 + MQTT), servo-sweep, 555-astable, rc-filter-bode (Pro), op-amp-amplifier (Pro).
- `PRJ-3` The **garden-node** example reproduces the design: UNO R3 (U1) + ESP-01 (U2) on
  SoftwareSerial D3/D2 + capacitive soil sensor (J1 → A0) in a pot + GL5528 LDR with 10 k divider
  (→ A1) + amber "night" LED on D9 via 220 Ω + red "water" LED on D8 via 330 Ω, mini scope CH1 on
  D9, DMM on A0. Its sketch (`garden-node.ino`, `wifi_at.h`, `mqtt_lite.h`) compiles with real
  avr-gcc, joins `SIM-AP`, publishes `garden/pot1` JSON every second, and prints
  `soil_raw=… moisture=…% light_raw=… led_pwm=…` lines whose values track the World sliders.
- `PRJ-4` Import: ngspice/LTspice-style `.cir` netlists into Pro mode (auto-schematic).
- `PRJ-5` Share/export: `.breadbox.zip` bundle, `.cir`, BOM CSV, schematic SVG/PNG, bench PNG,
  Gerbers.

## 13. Testing & quality (definition of done)

- `TEST-1` `cargo test --workspace` passes; `cargo clippy --workspace -- -D warnings` clean;
  `cargo fmt --check` clean.
- `TEST-2` SPICE reference suite vs ngspice golden data (`SPICE-6`).
- `TEST-3` AVR: per-instruction tests (flags, cycles), peripheral tests, and real-firmware tests
  (compiled by arduino-cli in `scripts/build-test-firmware.sh`; compiled `.hex/.elf` fixtures
  checked in so tests run without the toolchain).
- `TEST-4` Headless end-to-end scenario tests (`tests/e2e/`): garden-node full run (serial
  output, MQTT publishes received by the broker, PWM duty on D9 vs light, DMM on A0 ≈ sensor
  model, soil slider changes `moisture=` within 2 s sim time), blink timing, ESP32 Wi-Fi +
  MQTT, BLE virtual phone read/notify, LED burnout, USB polyfuse trip, plugin DMM loading the
  node, lesson 3 completion via checkers.
- `TEST-5` GUI end-to-end tests with `egui_kittest` (AccessKit queries + rendered snapshots):
  open example, run/pause, toggle breadboard⇄schematic, drag a part from the palette, draw a
  wire, change a resistor value, see serial output, open firmware screen + verify + upload, set a
  breakpoint and hit it, run Pro analyses and see plots, install a plugin from the catalog and
  place it, complete a lesson step with the math check.
- `TEST-6` Performance tests (`--release`, ignored by default, run by `scripts/test-all.sh`):
  `MCU-3`, `SIM-4`, `SPICE-5`.
- `TEST-7` `scripts/test-all.sh` runs everything (fmt, clippy, unit, reference, e2e, GUI, perf)
  and is green.

## 14. Explicitly out of scope for v1

- Emulated Linux SBCs (Raspberry Pi 4 "40-pin HAT host" running Linux).
- Cycle-accurate Xtensa (ESP32) / ARM (RP2040) cores — v1 uses API-level emulation (`MCU-6`).
- True 3D mesh rendering (`.glb`); the bench is 2.5D illustrated.
- Cloud share links / accounts; Windows & Linux builds (kept portable).
