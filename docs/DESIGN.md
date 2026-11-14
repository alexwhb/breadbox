# Breadbox — Visual design tokens

Source of truth: `docs/design/screenshots/*.png` (rendered at 1440×900) and the markup in
`docs/design/source/*.html` (CSS classes `.seg`, `.ghost`, `.tile`, `.rb`, `.tab`, `.inst`,
`.fchip`, `.paper`, `.dots`, … — read them for exact sizes). The full original bundle is
`docs/design/breadbox-design-bundle.html` (open in Chrome; screens are interactive, press Play).

## Colour (dark theme, default)

| token | hex | use |
|---|---|---|
| `bg.app` | `#0A0A0A` | window background, `.paper` grid base |
| `bg.canvas` | `#060606` | bench / lesson canvas (`.dots` grid) |
| `bg.sunken` | `#070707` | segmented-control wells, inputs |
| `bg.panel` | `#111111` – `#141414` | side panels, cards |
| `bg.raised` | `#171717` | ghost buttons, inputs |
| `bg.hover` | `#1A1A1A` / `#1C1C1C` | hover rows / buttons |
| `bg.selected` | `#2A2A2A` | segmented "on" |
| `border` | `#242424` | panel separators, tiles |
| `border.strong` | `#2E2E2E` | buttons, chips |
| `text` | `#EAEAEA` (primary) / `#F5F5F5` (emphasis) |
| `text.dim` | `#D9D9D9` | secondary labels |
| `text.muted` | `#A3A3A3` | captions, section headers (uppercase 11 px, tracking .08em) |
| `text.faint` | `#858585` / `#6E6E6E` | timestamps, units |
| `accent` | `#F2A33A` | primary amber: Run/Pause, Upload, Check, active tab underline, sliders |
| `accent.soft` | `#F7C77A` | values on amber, selected text |
| `accent.bg` | `#2A1F0E` / `#1D1810` | amber button / selected tile bg |
| `accent.border` | `#6B4A16` | amber button / selected tile border |
| `info` | `#4CC3F0` | links, net annotations, cyan console lines, kind chips |
| `ok` | `#3FBF6A` | running dot, connected chip, completed lesson |
| `danger` | `#C8352B` | errors, red wire, damage |
| `trace.yellow` | `#F2D03B` | scope CH1 |
| `code.keyword` | `#82AAFF` | |

Wire colours (dupont): red `#D23B2E`, black `#1E1E1E`, orange `#E8892B`, yellow `#E7C531`,
green `#3E9E4F`, blue `#3567C6`, purple `#7E4FC4`, grey `#8A8A8A`, white `#E8E8E8`,
brown `#7A4A21`. Breadboard body `#EDEAE2`, holes `#3A352D`, rail stripes red/blue.
UNO PCB `#1F5F8B`, ESP32 PCB `#1B1B1B`, Pico PCB `#2E7D32`.

## Typography

- Inter 400/500/600 — UI (13 px default, 12 px dense, 11 px captions).
- JetBrains Mono 400/500 — numbers, code, console, netlists, readouts (12–13 px).
- Space Grotesk 600 — brand wordmark (16 px), big headings ("Handheld DMM" 28 px,
  lesson step titles 22 px).

## Shape & spacing

- Radii: buttons 6 px, tiles/cards 8 px, chips 999 px / 4 px tags, panels 10–12 px.
- Heights: header 56 px; buttons 32 px; mini buttons 26 px; tabs 36 px; right-rail buttons
  40×40.
- Panels: parts library 292 px wide; world/inspector rail ~300 px; bottom dock ~220 px.
- Grid backgrounds: `.paper` 60 px major / 12 px minor lines at 4 % / 1.6 % white; `.dots`
  80/16 px at 7.5 % / 2.8 %.

## Iconography

Simple 1.3–1.5 px stroke line icons drawn as vector paths (see SVGs in the source HTML).
**Never** sparkle / magic-wand / four-point-star glyphs — pick an icon that says what the
feature does.

## Logo mark

26×26 rounded square (`#171717`, amber 1.5 px stroke), three amber dots on top row, a cyan
S-curve wire from the first dot to a cyan dot bottom-right (exact SVG in
`docs/design/source/1-workbench.html` header).
