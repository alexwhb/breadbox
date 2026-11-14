# Progress

Build plan: at most two agents work at once, each owning disjoint crates. The lead (orchestrator)
reviews and integrates between phases. Agents update **only their own section** below.

## Phases

| phase | agent A | agent B |
|---|---|---|
| 1 | `spice` — bb-spice (SPICE-*) | `mcu` — bb-avr + bb-firmware AVR path (MCU-1..5) |
| 2 | `sim` — bb-core device contract, bb-circuit, bb-world, bb-sim, parts library (PART-*, SIM-*, WORLD-*) | `radio` — bb-radio (RF-*), bb-lessons + lesson content (LES-*), example sketches |
| 3 | `ui-bench` — app shell, theme, Home, Workbench (APP-*, WB-*) | `plugins` — bb-plugin, bb-plugin-sdk, first-party plugins, radio↔sim integration, ESP32/Pico host boards (PLUG-*, MCU-6/7) |
| 4 | `ui-pro` — Firmware screen + debugger UI, Pro mode, PCB (PRO-*) | `ui-plug-les` — Plugins screen, Lessons screen + lesson editor, radio panel, instruments on bench |
| 5 | `e2e` — headless + GUI e2e suites, perf, fixes (TEST-*) | `qa` — independent spec audit + fixes, bundle-macos, docs |

## Ownership (current phase)

- bb-core: lead (frozen except additive changes; the `sim` agent owns it in phase 2)

## Requests (cross-crate asks)

_none yet_

## Status

### lead
- Phase 0 done: workspace skeleton, SPEC/ARCHITECTURE/DESIGN docs, bb-core time + SI units.
