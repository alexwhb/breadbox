# Spec audit (`audit`, 2026-11-14)

Independent audit of every requirement in `SPEC.md` against the code, the tests and the running
app (driven with `egui_kittest`; screenshots in `docs/screenshots/`). Status claims in
`PROGRESS.md` were not trusted: each row below cites the implementation and the test(s) that
prove it. §14 (out of scope) is not audited.

**Rule for "done"**: implemented, reachable in the app where the spec describes UI, and at
least one automated test exercising the behaviour. Variant-level test depth that is still thin
is listed under *Notes* rather than downgrading the row.

| | done | partial | missing |
|---|---|---|---|
| **before the audit** | 73 | 26 | 1 |
| **after** | 97 | 2 | 1 |
| **after closeout-a** | 100 | 0 | 0 |

Remaining after the audit: `PLAT-1` (OS-specific code in `bb-firmware::toolchain`), `WORLD-6`
(ADC quantization not switchable), `PLAT-3` (`Breadbox.app` bundle on the `bundle` agent's
branch). **Closed by `closeout-a`** (see the rows and PROGRESS `### closeout-a`): `PLAT-1` —
`bb_firmware::platform` + a workspace grep test; `WORLD-6` — realistic / ideal measurement
(sensor noise + AVR ADC transfer function); `PLAT-3` — merged bundle, `.breadbox` as a document
package, `bundle` stage in `scripts/test-all.sh`. Also fixed: toolchain detection could hang
forever (`MCU-4`). **`closeout-b`** closed the audit's other open items (rows `MCU-6`, `RF-1`,
`RF-3`, `PRO-2`) and deepened thin rows (`MCU-2`, `PART-6`, `PLUG-5`, `PLUG-7`, `PRJ-4`), fixing
what the new tests found — see the rows marked *closeout-b* and PROGRESS `### closeout-b`.

Paths are relative to the repo root; `bb/` = `crates/breadbox/`. GUI tests live in
`bb/tests/`, headless e2e in `tests/e2e/tests/`.

## 0. Platform

| ID | before → after | Evidence | Tests | Notes |
|---|---|---|---|---|
| PLAT-1 | partial → partial → **done** (closeout-a) | `rust-toolchain.toml`; OS code only in `bb/src/platform.rs`, `crates/bb-firmware/src/platform.rs` (Homebrew prefixes, Arduino data folder, macOS privacy folders, arduino-cli asset, permissions, symlinks, process groups), `crates/bb-core/src/paths.rs` | `bb-firmware/tests/platform.rs` (greps every `.rs` in `crates/`, `tests/`, `plugins/` for `target_os`, and bb-firmware for `cfg(unix)` / Homebrew paths / `std::os::unix`, outside the three modules), `platform::tests` | SPEC wording: "OS-specific code lives only in `platform` modules". |
| PLAT-2 | done | eframe/egui 0.37 wgpu; fonts `assets/fonts/` + OFL texts, embedded in `bb/src/theme.rs:334` | `gui.rs` renders every screen; `widgets.rs` gallery snapshot | Shipping the licence files inside the `.app` is PLAT-3. |
| PLAT-3 | missing → missing → **done** (bundle, merged; closeout-a) | `scripts/bundle-macos.sh` (Info.plist, icon, fonts, plugins, lessons, examples in `Contents/Resources`); `.breadbox` = document package (UTI `app.breadbox.project` → `com.apple.package`, owner + editor, `LSTypeIsPackage`); Open accepts packages (`platform::pick_project`) | `scripts/check-bundle.sh` (static + sandboxed smoke runs + `open` + `open -a` + Finder package / default app / double-click), run by `scripts/test-all.sh` stage `bundle` | Ad-hoc signed (no notarization). |
| PLAT-4 | done | `bb/src/smoke.rs`, `STARTED` in `bb/src/lib.rs` | `test-all.sh` perf gate (first frame < 2 s) | |
| PLAT-5 | done | `bb/src/widgets/icons.rs` | `icons::tests::no_sparkle_icons`; grep for ✨✦✧ = none | New icons used by the audit: Pir, Sonar, Inductor, Trash, Close, Plus, ChevronRight/Down. |

## 1. App shell & navigation

| ID | before → after | Evidence | Tests | Notes |
|---|---|---|---|---|
| APP-1 | done | top bar `bb/src/app.rs:597` (logo + wordmark → Home, breadcrumb, mode badge) | `gui.rs` navigation | |
| APP-2 | partial → done | Home `bb/src/screens/home.rs:128`; new-project modal now focuses + selects the suggested, unique name | `session.rs` (New Playground / New Pro via the modal), `gui.rs` shortcuts, `radio_panel.rs` (⌘Z / ⌘⇧Z after a slider or button) | **Fixed**: typing a name appended to the pre-filled `my-bench` (`my-benchfirst-blink`); a second default project failed "already exists" (`00766c6`). **Fixed (closeout-b)**: ⌘Z after focusing a slider, button or the floor-plan marker never reached the project's undo (`text_has_focus` was true for any focused widget; now egui's `text_edit_focused`), and the Firmware screen did not handle the project's undo at all (`23bb639`). |
| APP-3 | done | `bb/src/screens/mod.rs` registry; all five screens real | `gui.rs::every_screen_renders`, screenshots | |
| APP-4 | done | `bb/src/platform.rs:41,176` (File / Edit / View + Theme / Simulate / Window / Help) | `platform::tests` (ids → commands) | Audit added File → Import .cir, Simulate → Step One Quantum / Step 1 ms / Reset MCU Only (`b5cb94b`). The bundle agent adds the native-menu check. |
| APP-5 | partial → done | persisted route/theme/recents/last project/screen state `bb/src/app.rs:459`; `BreadboxApp::flush` `bb/src/app.rs:504` (eframe `save` on quit) | `app::tests::persisted_state_*`; `audit.rs::quick_edits_survive_a_project_switch_and_quit` | **Fixed**: edits younger than the 1.2 s autosave debounce were lost on quit and on switching projects (`bf211f0`). |

## 2. Workbench

| ID | before → after | Evidence | Tests | Notes |
|---|---|---|---|---|
| WB-1 | done | header `bb/src/screens/workbench/mod.rs` (Breadboard ⇄ Schematic, Run/Pause, clock, speed, status, Share) | `gui.rs`, `workbench.rs`, `session.rs` | Speed menu is now the simulation menu (see SIM-3). |
| WB-2 | done | `bb/src/screens/workbench/palette.rs` | `workbench.rs::palette_drag_*`, `session.rs` (Breadboard / Resistor / LED tiles) | |
| WB-3 | done | art `bb/src/canvas/art/`; zoom/pan `bb/src/canvas/bench.rs:376` | `workbench.rs` (every part on one bench), `session.rs` (Zoom → Fit to view) | ⌘-scroll / pinch zoom, scroll / space- / middle-drag pan verified in code; fit now includes world objects. |
| WB-4 | done | wires, colours, bends `bb/src/canvas/bench.rs` | `workbench.rs::palette_drag_wire_drawing_*`, `session.rs` | |
| WB-5 | done | box select `bench.rs:521`, R / F / ⌘D / ⌘C / ⌘V `bench.rs:943`, undo via `Editor` | `workbench.rs` (R / ⌘D / ⌫, undo/redo, inspector 4k7), `session.rs` (220 Ω in the Inspector) | |
| WB-6 | done | probe labels `bench.rs:1589`, hover tooltips | `workbench.rs::probe_tool_*` | |
| WB-7 | done | `bb/src/canvas/schematic.rs:1490`, IEC/ANSI, ratsnest both ways | `workbench.rs` schematic + net labels | |
| WB-8 | done | rail + collapsible panels | `workbench.rs`, `session.rs` (World rail toggle) | |
| WB-9 | done | World panel `bb/src/screens/workbench/panels.rs` (stop sliders, evaporation, day/night + location `:34`, noise, Water the pot `:306`, time-warp `:334`), + Water automatically `:229`, Objects, Program the world | `workbench.rs::soil_slider_*`, e2e `examples` (3600× day/night), `audit.rs::world_panel_schedules_watering`, `audit.rs::hand_object_scripts_and_timelines_drive_the_world` | |
| WB-10 | done | `bb/src/screens/workbench/inspector.rs` (datasheet fields, params, health `:249`) | `workbench.rs` (J1 inspector), `session.rs` | |
| WB-11 | done | `bb/src/screens/workbench/dock.rs` (port label, baud, line ending, send, Clear, autoscroll, per-MCU tabs, annotations `:298`; Probes & power) | `workbench.rs` serial + tabs; `audit.rs::serial_monitor_radio_annotations_toggle` | |
| WB-12 | done | status strip `workbench/mod.rs:320` | `workbench.rs` | |
| WB-13 | done | LED bloom, RGB, buzzer audio `bb/src/bench/audio.rs:32`, motors/servos/LCD/OLED/7-seg/relays art, smoke + Repair all | bb-sim `devices_fw::tone_*`, `scenarios::servo_*`, `lcd_*`, `devices_world::dc_motor_*`, `relay`; `workbench.rs` LED burnout; `scenarios::mcu_pin_over_40ma_*`, `reversed_electrolytic_pops` | Pin rule: > 40 mA = stressed (inspector), > 80 mA = destroyed. |

## 3. Parts library

| ID | before → after | Evidence | Tests | Notes |
|---|---|---|---|---|
| PART-1 | done | `crates/bb-circuit/parts/boards.json` | `library::tests::every_spec_part_exists`, Mega / Pico / ESP32 bench tests | |
| PART-2 | done | `parts/blocks.json` | `every_spec_part_exists`, `every_library_part_netlists`, battery sag, polyfuse, `reversed_electrolytic_pops` (new) | DIP switch, trimmer, barrel jack, AA×2: generic `.op` coverage only. |
| PART-3 | done | `parts/light_sound.json` | `parts_behaviour::led_colours_have_their_own_forward_voltage` (new), NeoPixel, tone, RGB/7-seg | RGB common-anode, LED bar graph, active buzzer, speaker: generic only. |
| PART-4 | done | `parts/sensors.json` | HC-SR04, DS18B20, BME280, DHT22, PIR, `parts_behaviour::thermistor_follows_the_air_temperature` (new) | DHT11, tilt, resistive soil: generic only. |
| PART-5 | done | `parts/semis.json`, regulators in `blocks.json` | `parts_behaviour::{zener_*, mosfets_switch_and_op_amps_follow, logic_gates_truth_tables}` (new), 555, L293D, ULN2003, PC817, regulators | TIP120: generic only. |
| PART-6 | done | `parts/displays_motors.json`, `bb-sim/src/devices/lcd.rs` | LCD 4-bit + I²C, SSD1306, servo, stepper, DC motor, relay, `scenarios::lcd_on_the_8_bit_bus` (closeout-b: LiquidCrystal on D0…D7, CGRAM character, ROM 0xDF) | LCD 8-bit mode asserted (closeout-b); vibration motor, servo slew: not asserted. |
| PART-7 | done | `parts/misc.json`, `plugins/lora-gateway/parts.json` | `bb-radio` espat / ble / lora | |
| PART-8 | done | `simple: true` set (25 parts) | `library::tests::simple_set_is_the_25_starter_parts` | |
| PART-9 | done | `crates/bb-circuit/src/library.rs` | `plugin_parts_use_the_same_schema` | |

## 4. SPICE

| ID | before → after | Evidence | Tests | Notes |
|---|---|---|---|---|
| SPICE-1 | done | `crates/bb-spice/src/netlist/parse.rs` | parse unit tests, reference suite, `analyses::mil_suffix_dc_octave_and_step_dec_oct` (new) | |
| SPICE-2 | done | `bb-spice/src/devices/*`, `expr.rs` | `devices/tests.rs`, `devices/sources.rs` unit tests, 47 reference circuits | |
| SPICE-3 | partial → done | `analysis.rs` (.dc/.tran/.ac/.noise/.tf), `meas.rs`; **in the app**: Pro measurements table `bb/src/screens/pro/mod.rs:1370` | `analyses.rs` (+ `.dc oct`, `.step dec/oct` new), reference `meas.cir`, `tf_bjt`, `noise_*`; `audit.rs::pro_shows_every_analysis_card_and_meas_tf_results` | **Fixed**: `.meas` and `.tf` results were computed but never shown in Pro (`e73fd12`). |
| SPICE-4 | done | `engine.rs:528-636`, `devices/semi.rs` | `analyses.rs` gmin stepping, floating node, impossible circuit | The timestep-cut test checks recovery only (not the cut count). |
| SPICE-5 | done | `matrix.rs` | `perf.rs` (1 024-node `.op` < 50 ms, gated in test-all) | |
| SPICE-6 | done | `tests/reference/` (47 circuits + golden) | `reference.rs` | |
| SPICE-7 | done | `session.rs` | `session.rs` tests | |
| SPICE-8 | done | `netlist/mod.rs`, `export.rs` | `analyses::export_cir_round_trip`, e2e `projects` ngspice runs | |

## 5. Microcontrollers & firmware

| ID | before → after | Evidence | Tests | Notes |
|---|---|---|---|---|
| MCU-1 | done | `bb-avr/src/exec.rs`, `decode.rs` | `bb-avr/tests/instructions.rs` (35), real firmware | |
| MCU-2 | done | `bb-avr/src/{gpio,timer,usart,adc,spi,twi,misc}.rs` | `bb-avr/tests/*`; bb-sim `scenarios::{uno_usart_tx_looped_back_to_rx, mega_usart1_and_usart2_loop_back}` (closeout-b); hardware SPI mode 0 with MISO and TWI on the bench in bb-plugin `instruments::logic_analyzer_decodes_{spi,i2c}_*` (closeout-b) | Mega USART1 / USART2 now exercised on the bench (closeout-b); Mega Timer4 still not. |
| MCU-3 | done | — | `bb-avr/tests/perf.rs` (gated) | |
| MCU-4 | partial → done | `bb-firmware/src/toolchain.rs`, `compile.rs`, `process.rs`; Firmware screen + `StagingLock` `bb/src/screens/firmware/build.rs:105` | `toolchain.rs` tests, `gui.rs::verify_blink_*`, `build::tests::concurrent_builds_of_one_sketch_compile_their_own_sources` (new), `session.rs` (Verify + Upload of a typed sketch); closeout-a: `cli_timeouts.rs` (fake arduino-cli hanging on version / core list / lib / compile, cancel, private settings on every call), `process::tests`, `toolchain_first_run.rs::a_stuck_arduino_cli_shows_a_message_instead_of_detecting_forever` | **Fixed**: all builds of a sketch shared one staging folder, so a concurrent debug build or a second app instance compiled the wrong sources (the flaky Verify GUI test, `95f43c0`). **Fixed (closeout-a)**: detection could hang forever — arduino-cli's default sketchbook `~/Documents/Arduino` raised a macOS privacy prompt for the Finder-launched app and `core list` blocked in `open()`; now a private config (sketchbook in the toolchain folder) + time limits on every subprocess + an "arduino-cli: no answer" state with Retry. |
| MCU-5 | partial → done | debugger backend `bb-firmware/src/debugger.rs`; Debug tab Memory (SRAM / Flash / EEPROM hex, pointers, changed bytes, Go to) + I/O registers `bb/src/screens/firmware/debug_panel.rs` | `gui.rs` breakpoint/step, e2e `firmware`, `debug_memory.rs` (new, helper) | **Closed**: hex views and the I/O register view were missing in the UI (`ca6e46d`). |
| MCU-6 | partial → done | `bb-firmware/hostcore/`, `host/` | `host.rs`, `esp32.rs`, `host_boards::esp32_ledc_analog_write_and_micros` (new), `host_bridges` (Serial2 ↔ HC-05, Wire, SPI; closeout-b: `esp32_serial1_and_serial2_loop_back_on_the_same_board`, `pico_serial1_and_serial2_loop_back_on_the_same_board`, `an_unwired_uart_hears_nothing_and_a_reset_reopens_it`, `esp32_wifiserver_answers_a_virtual_client`) | **Fixed (closeout-b, `1707d22`)**: a UART looped back to the *same* board stayed silent — the board's own TX edges reached its RX after it had run past them and were stamped "now" (a frame collapsed into one instant; `McuCore::late_input` keeps their time), and the receiver decoded the floating RX line before `begin()`, was busy when `begin()` came and never switched baud. Also: Serial1/2 writes stop the board (TX starts at the write time), a reset closes the UARTs, ESP32 `begin(baud, cfg, rx, tx)` routes the UART to any GPIO. `WiFiServer` serves a virtual client station. |
| MCU-7 | done | per-MCU firmware, serial tabs, debugger targets | `workbench.rs` per-MCU tabs, bb-avr multi-MCU | |

## 6. Co-simulation

| ID | before → after | Evidence | Tests | Notes |
|---|---|---|---|---|
| SIM-1 | partial → done | `bb-sim/src/sched.rs`; quantum from `project.settings`, now editable in the simulation menu `workbench/mod.rs:87` | e2e `sim_control::single_steps_*`, `audit.rs::simulation_menu_*` (quantum 1 → 10 µs, step follows) | **Closed**: the quantum was configurable only by editing project.json. |
| SIM-2 | done | `bb-sim/src/mcu.rs:234` | `scenarios::button_with_input_pullup_*`, `analog_write_into_rc_*` | Leakage / hysteresis not asserted directly. |
| SIM-3 | partial → done | `SimHandle`; simulation menu (Step 1 quantum / 1 ms, Reset bench, Reset MCU only · Ux, Seed, Sync quantum) + Simulate menu items | e2e `sim_control` (replay, steps, MCU reset), `audit.rs::{simulation_menu_steps_resets_mcu_and_replays_with_a_seed, native_menu_step_and_reset_mcu_commands_work}` | **Closed**: no UI for step / reset-MCU-only / seed (`b5cb94b`). |
| SIM-4 | done | — | bb-sim perf + GUI smoke (gated) | |
| SIM-5 | done | `handle.rs`, `telemetry.rs` | `engine.rs`, perf with telemetry | |
| SIM-6 | partial → done | `bb-sim/src/damage.rs` | `scenarios::{led_without_resistor_*, usb_short_*, shorted_uno_rail_*, mcu_pin_over_40ma_is_stressed_and_a_short_destroys_it (new), reversed_electrolytic_pops (new)}`, battery sag | |
| SIM-7 | done | `handle.rs` Flash / ResetMcu | `engine::reflash_and_mcu_reset_*`, e2e `sim_control` | |

## 7. Radios

| ID | before → after | Evidence | Tests | Notes |
|---|---|---|---|---|
| RF-1 | partial → done | `bb-radio/src/net`, `geo.rs`; Radios panel floor plan (draggable bench, walls, live distance / walls / RSSI) | `net.rs`, `radio_panel.rs::placement_*` (new, helper), `radio_panel.rs::{radio_settings_are_saved_with_the_project_and_undoable, floor_plan_walls_are_editable_undoable_and_saved}` (closeout-b), `radio_panel::tests` | **Closed**: no UI for station placement. **Closed (closeout-b, `23bb639`, `e013128`)**: bench position and walls (new: Add wall / interior ↔ concrete / remove) are undoable `Op::Radios` edits of `project.radios`, restored on open; undo / redo reach the running bench. |
| RF-2 | done | `net/services.rs`, `mqtt/broker.rs` | `net.rs`, `broker.rs` | |
| RF-3 | partial → done | `set_conditions`, LAN bridge; panel: latency, **jitter**, loss, **drop the AP at t = N s**, bridge host:port | `bridge.rs`, e2e `garden_node` fault injector, `radio_panel.rs` (jitter, drop at 30 s; closeout-b: latency / loss / drop-AP / LAN bridge saved, undone, redone, reopened) | **Closed**: jitter was fixed at latency/4, the AP drop at 60 s. **Closed (closeout-b, `23bb639`)**: latency, jitter, loss, drop-AP time and the LAN bridge host:port are saved in `project.json`, restored on open and undoable; the panel no longer showed / pushed its own 40 ms / 2 % defaults over what the bench ran. |
| RF-4 | partial → done | `log.rs` pcap-ng / CSV; Packet log "Export pcap-ng…" / "Export CSV…" | `bb-radio` export tests, e2e garden-node export, `radio_panel.rs` (magic `0x0A0D0D0A`, CSV header) | **Closed**: export existed only as an API. |
| RF-5 | done | `bb-radio/src/espat.rs` | `espat.rs` (+ ATE1, new) | |
| RF-6 | done (flaky) → done | `ble/{mod,hm10,hc05}.rs`; phone `bb/src/widgets/radio_tools.rs` | `ble.rs::notify_right_after_subscribe_is_delivered` (new), e2e `boards::ble_button_*` (+ `press_right_after_subscribe`, new), `radio_tools.rs` | **Fixed** the flaky GUI test's root cause: a notification sent < 10 ms after the phone's subscribe was dropped (`05b3af0`). The phone logs "notifications on · FFE1"; the toggle shows "Turning on…" until the CCCD lands. |
| RF-7 | partial → done | Radios panel cards / log / conditions / BLE card; **Add BLE hat** places + wires an HM-10 (one undoable edit) | `gui.rs` garden-node radios, `radio_tools.rs`, `radio_panel.rs::add_ble_hat_*` (new) | **Closed**: Add BLE hat only posted a note. |
| RF-8 | partial → done | `space.rs` annotations; dock toggle `dock.rs:298` | e2e `garden_node`, `audit.rs::serial_monitor_radio_annotations_toggle` (new) | |

## 8. World

| ID | before → after | Evidence | Tests | Notes |
|---|---|---|---|---|
| WORLD-1 | partial → done | `bb-world/src/lib.rs` pots, `water`, **scheduled watering** (`PotConfig::schedule`, `scheduled_watering`); World panel "Water automatically" | `tests::pot_dries_and_drains`, `watering_*`, `scheduled_watering_waters_every_interval_even_at_high_time_warp` (new), `audit.rs::world_panel_schedules_watering` (new), pump | **Closed**: scheduled watering was not implemented (`688635c`). |
| WORLD-2 | done | `sun.rs`, light sources | `tests::solar_*`, `bench_light_sources_*`, e2e day/night | |
| WORLD-3 | done | air config, heat sources | `heat_sources_warm_nearby_sensors`, `resistors_heat_up_*`, `thermistor_follows_the_air_temperature` (new) | Tempco shift from self-heating and humidity diurnal swing not asserted. |
| WORLD-4 | partial → done | objects in `bb-world`; on the bench `bb/src/canvas/world_objects.rs:70` (hand / person / wall / magnet, drag, fit); World panel Objects | `tests::objects_*` (+ `objects_can_be_added_moved_and_removed_at_run_time`, new), `world_objects::tests`, `bench::tests::dropping_the_hand_tile_adds_a_world_object`, `audit.rs::hand_object_scripts_and_timelines_drive_the_world` (sonar sketch: 26 → 16 cm) | **Fixed + closed**: no object could be placed or dragged in the app; the palette's Hand tile dropped an inert picture the sensors never saw (`8024f00`). |
| WORLD-5 | partial → done | timelines + scripts in `bb-world`; World panel "Program the world" `bb/src/screens/workbench/world_program.rs:260` (scripts with validation, keyframe timelines with interpolation / repeat / preview) | `tests::scripts_and_timelines_drive_variables`, `scripts_are_checked_without_a_world` (new), `audit.rs::hand_object_scripts_and_timelines_drive_the_world` | **Closed**: engine-only before. |
| WORLD-6 | partial → partial → **done** (closeout-a) | Lead's decision: on = realistic (sensor noise + AVR ADC with per-chip offset / gain error, INL / DNL ≈ ±0.5 LSB, ±1 LSB noise, seeded — `bb_avr::AdcModel::Realistic`, `AdcChip`), off = ideal (noise-free sensors, exact `floor(V/Vref·1024)`); `bb_sim::bench::adc_model_for` + `sync_adc_models` (live); World panel text + Inspector "ADC" / "Noise" rows | `bb-avr/tests/adc.rs` (ideal ramp exact, realistic bounded + seeded, steady input ±1 code, INL / DNL of 24 chips, live switch), `bb-sim/tests/garden_node.rs::soil_raw_jitters_only_with_realistic_measurement`, `audit.rs::world_noise_switch_picks_realistic_or_ideal_measurement`, `inspector::tests::measurement_rows_follow_the_world_switch` | Ideal still follows the circuit: on the full garden node the PWM night LED sags AVcc (the ADC reference) by a few mV, so `soil_raw` may toggle between two neighbouring codes — identically for every seed. Firmware always reads integer codes; probes show continuous volts. |

## 9. Pro

| ID | before → after | Evidence | Tests | Notes |
|---|---|---|---|---|
| PRO-1 | done | Schematic / Analysis / PCB tabs | `gui.rs`, `workbench.rs` | |
| PRO-2 | partial → done | synced netlist `bb/src/screens/pro/sync.rs`, detach / re-sync; new Pro projects edit `analyses/<name>.cir` (`bb/src/project.rs:577`) | e2e `projects` (R1 edit round-trips), `gui.rs`, `session.rs::session_2_*` (netlist saved in the project), `session.rs::session_2b_typed_netlist_builds_the_schematic_and_stays_in_sync` (closeout-b), bb-circuit `spice_io::tests` | **Fixed**: a new (empty) Pro project fell back to the built-in garden-node demo netlist; what the user typed was never saved (`cfd36a3`). **Closed (closeout-b, `ecaaa13`)**: a netlist typed into a new / netlist-only Pro project builds the circuit + auto-placed schematic (`spice_io::netlist_to_circuit`, one undoable "Netlist" step per edit, placements kept), stays in sync both ways, and only unrepresentable content detaches it (Re-sync). Screenshot `docs/screenshots/pro-typed-netlist-schematic.png`. |
| PRO-3 | partial → done | all seven cards (scrollable sidebar) `pro/mod.rs:813` | `gui.rs` (.mc toggle), `audit.rs::pro_shows_every_analysis_card_*`, `session.rs` (AC card enable → Bode) | **Closed**: Noise / Op point cards were hidden unless already present. |
| PRO-4 | done | `bb/src/widgets/plot.rs` (dual Y, log axes, cursors, histogram markers), Bode −3 dB marker `pro/model.rs:69` | `gui.rs` Pro plots, `pro::model::tests::corner_*` (new), `session.rs` (corner 1.00 kHz) | Added: the Bode card reads and marks the −3 dB corner. |
| PRO-5 | done | mixed-signal panel | `gui.rs` | |
| PRO-6 | done | `.cir`, CSV / PNG / SVG of plots + schematic | `workbench.rs` Share, e2e `projects` | |
| PRO-7 | done | `bb-circuit/src/pcb`, `bb/src/screens/pro/pcb/mod.rs` | `pcb.rs`, bb-circuit pcb tests | Silkscreen reference designators in the Gerber: `bundle` agent. |

## 10. Plugins & instruments

| ID | before → after | Evidence | Tests | Notes |
|---|---|---|---|---|
| PLUG-1 | done | `bb-plugin/src/manifest.rs`, `package.rs` | `schema.rs`, e2e `plugins` | |
| PLUG-2 | partial → done | `bb-plugin/src/sandbox.rs` (permission-checked imports, fuel, epochs, memory caps); permission prompt; violations → `BenchService::alerts` → error toast on every screen `bb/src/bench/service.rs:29` | `sandbox.rs`, `plugins.rs` (prompt), `audit.rs::a_plugin_violating_its_permissions_shows_an_error_on_the_workbench` (new) | **Closed**: a sandbox violation was only visible in the Firmware output panel (`bf211f0`). |
| PLUG-3 | done | `crates/bb-plugin-sdk`, `docs/PLUGIN-SDK.md` | via every first-party plugin + raw-ABI WAT tests | SDK has no unit tests of its own. |
| PLUG-4 | done | `bb/src/screens/plugins/mod.rs` | `plugins.rs` (catalog → prompt → install → place, local plugin hot reload, theme, SDK docs) | |
| PLUG-5 | partial → done | 7 plugins under `plugins/` with committed reproducible wasm | `bb-plugin/tests/{dmm,instruments,packs}.rs` (+ `dmm_capacitance_mode_measures_and_autoranges`, new); closeout-b in `instruments.rs`: `scope_coupling_probe_compensation_and_pulse_width_trigger`, `logic_analyzer_decodes_{i2c_to_a_bme280, spi_mosi_and_miso, one_wire_to_a_ds18b20}`, `function_generator_every_waveform_into_a_50_ohm_load` | **Fixed + closed**: the DMM's ⊣⊢ position was labelled with the capacitor symbol but ran a diode test; it now measures capacitance 60 nF – 6 mF (`3444a0e`). **Deepened (closeout-b, `09a5dd5`, `c39116a`)**: scope DC / AC / GND coupling, 10× compensation, pulse-width trigger; LA I²C / SPI / 1-Wire on real AVR traffic; FG every waveform into 50 Ω. Fixed on the way: FG "Output off" left the DC offset on; scope Normal / Single kept "Trig'd" + `triggered = 1` with nothing to trigger on; the scope's counter timed edges on the sample grid (1 kHz read 1008 Hz); the LA packed DallasTemperature's ~11 000 status-poll slots into `0x00` bytes (1-Wire now decodes transactions: named commands, `busy ×N → done`, `search · N slots`). Scope cursors still only covered by the panel state. |
| PLUG-6 | done | mini scope core instrument; plugin panels `bb/src/canvas/plugin_parts.rs:102` | `plugins.rs`, `workbench.rs` | |
| PLUG-7 | done | `bb-plugin/src/{generator,analysis,world,radio,board,theme}.rs` | `packs.rs`, `plugins.rs` (analysis plugin in Pro, theme), `packs::plugin_part_with_a_subckt_model_regulates_on_the_bench_and_in_pro` (closeout-b) | **Closed (closeout-b, `53470e6`)**: `plugins/examples/lm317` — a part plugin whose analog model is a `.subckt` in its `parts.json` (+ wasm watching it) regulates on the bench, its `.subckt` reaches Pro analyses (`.op` = bench to 1 mV) and `.cir` export, and drops out with a warning. |

## 11. Lessons

| ID | before → after | Evidence | Tests | Notes |
|---|---|---|---|---|
| LES-1 | done | `lessons/` — Track 1 = the 8 named lessons; 4 more tracks × 4 | `lessons::track_one_is_the_designs_eight_lessons`, solver for every track | |
| LES-2 | done | `bb/src/screens/lessons/` | `lessons.rs` (step 3 math check), `session.rs::session_3_*` | **Fixed**: long quiz answers overflowed their option (`d327ed9`). |
| LES-3 | done | every step kind used in real content (explain / build / quiz: L1; predict / measure: L2; experiment / break: L3; code: L8) | `solver.rs` completes all 24 lessons; code steps compile | |
| LES-4 | done | TOML + `Progress` | `lessons::every_lesson_loads_and_validates`, `progress_persists_*`, `session.rs` (progress file) | 6 lessons have no hints. |
| LES-5 | done | Open in free play | `lessons.rs` | |
| LES-6 | done | `bb/src/screens/lessons/editor.rs` | `lessons.rs` editor → pack → Lessons | |

## 12. Projects & files

| ID | before → after | Evidence | Tests | Notes |
|---|---|---|---|---|
| PRJ-1 | done | `bb-circuit/src/project.rs` (format, migration) | `project_edit.rs` | |
| PRJ-2 | done | 11 examples | e2e `examples` (each runs 5 s cleanly), `gui.rs` open example | |
| PRJ-3 | done | `examples/garden-node` | e2e `garden_node` | |
| PRJ-4 | partial → done | `bb-circuit/src/spice_io.rs`; **Home → Import .cir…**, File → Import .cir Netlist…, Open on a `.cir` → `export::import_cir_project` `bb/src/export.rs:80` | e2e `projects::cir_import_builds_a_pro_project`, `audit.rs::cir_netlist_imports_as_a_pro_project_and_runs` (new) | **Closed**: no UI entry point. **Fixed (closeout-b)**: `R1 a b 1k tc1=…` / `C1 a b 1u IC=…` lost their extra parameters (imported as plain library parts) — only the plain form maps to a library part now; V / I sources import as source symbols instead of 16-pin generic boxes. |
| PRJ-5 | done | `bb/src/export.rs`, Share menu (+ Gerbers zip `share.rs:52`) | `workbench.rs` Share + reopen, e2e `projects` | |

## 13. Testing

| ID | before → after | Evidence | Tests | Notes |
|---|---|---|---|---|
| TEST-1 | done | `scripts/test-all.sh` fmt / clippy / test | — | |
| TEST-2 | done | SPICE reference suite | `reference.rs` | |
| TEST-3 | done | bb-avr instruction / peripheral / firmware fixtures | `bb-avr/tests` | |
| TEST-4 | done | `tests/e2e` | 37 scenarios | |
| TEST-5 | partial → done | `bb/tests/*.rs` | gui, workbench, plugins, lessons, radio_tools, pcb, widgets, + `audit.rs`, `session.rs`, `debug_memory.rs`, `radio_panel.rs` | Two flaky GUI tests fixed at the root (RF-6, MCU-4). |
| TEST-6 | done | `#[ignore]` perf tests | test-all perf gates | |
| TEST-7 | done | `scripts/test-all.sh` | two consecutive green runs (PROGRESS `### audit`) | |

## New-user session

`bb/tests/session.rs` scripts a first session through the UI only (screenshots
`docs/screenshots/session-*.png`): Home → New Playground project → breadboard, resistor (set to
220 Ω in the Inspector), LED, D13 / GND jumpers → my own blink sketch typed in the Firmware
editor → Verify → Upload to sim → the bench LED blinks (12.6 mA); Home → New Pro project →
typed RC low-pass → AC card enabled → Run analyses → "−3 dB at 1.00 kHz"; Home → Open lessons
→ Track 1 lesson 1 (wire VBUS → a5, delete the GND jumper, quiz) → done ✓.

Friction found and fixed on the way: the new-project name field (APP-2), a new Pro project
editing the demo netlist (PRO-2), no readable Bode corner (PRO-4), quiz answers overflowing
(LES-2). Notes: a new Playground project already contains an UNO R3 and a starter sketch, so
the session does not drag a second board; a dropped resistor is 10 kΩ until changed.
