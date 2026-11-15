# Workbench

The Workbench is the breadboard bench: place parts, wire them, run real firmware and watch
the circuit, the world around it and the serial output. The **Breadboard / Schematic** switch (top
left) shows the same circuit as a schematic.

## Parts

- **Simple** shows the 25 starter parts as tiles; **Advanced** lists the whole library by
  category with part numbers. Hover a part to read what it is.
- Drag a part onto the bench, or click it and then click the bench to drop it (**Esc**
  cancels).
- Search (⌘F) matches names, part numbers and SPICE `.model` names. The Advanced filters show
  parts with a SPICE model, boards that run firmware, or parts you can interact with.
- Missing a part? Define it in a part file or a plugin — see **Plugin SDK** in this window.

## Wiring

- Drag from any pin or breadboard hole to another to add a jumper wire. Breadboard rows and
  rails connect the way they do on a real board.
- **Double-click** a wire to add a bend; drag a bend to route the wire; double-click a bend
  to remove it. **Straighten** in the Inspector removes them all.
- Hovering a pin highlights its whole net; the tooltip shows the net name and live voltage.
- Hold **⌥** while dragging from a pin to move the part instead of starting a wire.

## Editing

- Click to select, **⇧**-click to add to the selection, drag on empty bench to box-select.
- **R** rotates, **F** flips, **⌘D** duplicates, **⌘C / ⌘X / ⌘V** copy / cut / paste,
  **⌫** deletes, the arrow keys nudge, **⌘Z / ⇧⌘Z** undo / redo.
- Right-click for the same actions and a wire's colour.
- Scroll to pan (or hold **Space** / the middle button and drag); pinch or ⌘-scroll to zoom.
  The zoom % in the status strip has **Fit to view**.

## Running the simulation

- **Run / Pause** starts and stops the whole bench: boards run their firmware, the circuit and
  the world advance together. `t =` is simulated time.
- The speed menu (`1× real-time`, `Max`, …) sets how fast simulated time runs against your
  clock.
- **Step** runs one sync quantum or one millisecond, then pauses.
- **Reset bench** restarts everything; **Reset MCU only** resets one board (like its reset
  button) while the circuit keeps running.
- **Seed** makes runs repeatable: the same seed replays the same sensor noise. **Sync
  quantum** is how often the boards and the analog circuit exchange pin states — smaller is
  more exact, larger is faster. Changing either restarts the bench.

## Probes

- **Probes → Add probe**, then click a pin: a live label is pinned to the bench and the panel
  shows the value with its min / max / average.
- Probes also appear as cards in the dock's **Probes & power** tab and in Pro mode next to the
  SPICE operating point.

## Instruments

- **Instruments → Add** puts the oscilloscope (2 channels) or the multimeter (DC volts) beside
  the bench; **Remove** takes it away again.
- To connect a lead, press its **Clip** button (`Clip CH1`, `Clip +`, `Clip COM`), then click
  any pin or breadboard hole. The row shows where the lead is clipped. **Esc** cancels.
- **Waveforms** shows the scope's screen large, with timebase and V/div buttons and the
  measured frequency, duty cycle, Vpp and average.
- More instruments — logic analyzer, bench supply, function generator — are plugins: **Browse
  plugins** opens the Plugins screen.

## Inspector

- Select a part to see its datasheet values, live readings and health; edit its settings
  (resistor values step through the E-series with ‹ ›). Select a wire for its ends and colour.
- A part pushed past its ratings shows **stressed** or **damaged** with the reason;
  **Repair all** puts every damaged part back.

## The World panel

- The sliders set soil moisture, ambient light and air temperature for the running bench.
- **Evaporation** dries the pot over world time (the rate depends on temperature, humidity
  and light). **Auto-water** adds 100 ml on a schedule; **Water the pot** adds 100 ml now.
- **Day/night cycle** drives the light from the sun at the chosen location.
- **Sensor noise & ADC quantization** switches between realistic sensors (noise, ADC offset /
  gain error, ±1 LSB) and ideal ones. Probes and meters always show the exact voltage.
- **More settings** adds humidity and **World time**: how fast world time (sun, evaporation,
  schedules) runs against bench time.
- **Objects** appear when the bench has a distance, motion or hall sensor: add a hand, a
  person, a wall or a magnet and drag it on the bench to move it.

## Programming the world

Open **Program the world** to drive the world from a script or a timeline.

- A **script** is `variable = expression` statements (separate them with `;` or new lines,
  `#` starts a comment). They run every step:
  - `moisture = 45 - 0.8*t_hours` — the pot dries 0.8 % per hour
  - `temp = 20 + 5*sin(t_hours)`
- Variables: `moisture`, `lux`, `temp`, `humidity`, `clouds`, `warp`, any object's `hand.x`
  / `hand.y`, and the time: `t` (seconds), `t_min`, `t_hours`, `t_days`, `hour` (local time).
- Functions: `sin`, `cos`, `exp`, `sqrt`, `abs`, `min`, `max`, `pow`, `limit`, `if`, …
- A **timeline** sets one variable from keyframes (time → value) with linear, step or smooth
  interpolation, optionally repeating. The preview shows the curve and the current world time.

## Serial monitor

- Shows each board's serial output with timestamps; network events from radios are
  annotated in blue (turn off **Radio annotations** to hide them). With several boards, pick
  one with the chips in the dock header.
- Type in the send box and press Enter; choose the line ending and baud rate beside it.
- **Clear** empties the log; **Autoscroll** keeps the newest line in view.

## Probes & power

- One card per probe plus the supply rails, the total current and power, the USB budget
  (500 mA) and each part's dissipation.

## ERC / DRC

- The status strip shows the electrical (ERC) and design (DRC) rule check results, e.g.
  `ERC 1 warning · DRC clean`. Click it for the list; click a finding to select its parts.

## Schematic view

- The same netlist as the bench, drawn as a schematic: wiring either view changes both.
- **ANSI / IEC** switches the symbol style; net labels join nets by name.

## Sharing

**Share** exports the project bundle (`.breadbox.zip`, opens on any Mac with Breadbox), a
SPICE netlist (`.cir`), a bill of materials (`.csv`), the schematic as SVG or PNG, a picture
of the bench, or PCB Gerbers.

## Radios

The rail's phone and antenna buttons open the **virtual phone** (connects to BLE modules
such as the HM-10) and the **MQTT explorer** (the bench's built-in broker). See
**Firmware** in this window for radios in sketches.
