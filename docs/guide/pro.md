# Pro mode

The same circuit as the bench, as a SPICE netlist with analyses, a schematic and a PCB.

## Netlist

- The netlist is generated from the schematic (*synced from schematic*). Edits are autosaved
  and flow back into the schematic.
- Lines the schematic can't hold *detach* the netlist. **Re-sync** rewrites it from the
  schematic.
- The folder button opens a `.cir` file. **Export .cir** saves an ngspice-runnable netlist.

## Analyses

- Each card is one analysis (`.tran`, `.dc`, `.ac`, `.mc`, temperature, `.noise`, `.op`).
  Click a card to change its settings or turn it off. **Apply** reruns.
- **Run analyses** solves everything again. The top bar shows steps, solve time and
  convergence warnings.
- On the `.tran` plot, drag the two cursors. The readout gives Δt, frequency and duty cycle.
- The menu button at a plot's top right exports CSV, PNG or SVG.
- The `.op` table puts each solved value next to the live **bench probe** reading.
  `.meas` and `.tf` results get their own table.

## Mixed-signal

- The AVR core runs the real compiled firmware. Every sync step it hands its pin states
  to the analog solver, and ADC reads sample the solved node voltage.
- **Plugin analyses** come from installed analysis plugins. **Run** passes them the current
  netlist.

## Schematic

- Drag from a pin to draw a wire. **R** rotates the selected symbol.
- **Net label**: wires with the same label anywhere are one net. Type a name, press
  **Place**, then click a pin or wire (Esc cancels).
- **ANSI / IEC** switches the symbol style. **Fit** frames the sheet.
- The **Nets** list shows every net with its live voltage.

## PCB

- **Select**: drag footprints. **R** rotates, ⌫ deletes the selected trace or via.
- **Route**: click a pad, click each corner, then click the target pad. **V** drops a via and
  switches layer. Esc cancels.
- **Via**: click the board to drop a via.
- Pick the copper layer and the trace width before routing.
- **Auto-place** lays every footprint out in rows and sizes the outline around them. **Fit
  outline** resizes the outline only.
- **Design rules** lists unrouted nets and clearance, width, drill and edge violations.
- **Export Gerbers…** writes Gerber RS-274X files (top and bottom copper, solder mask,
  silkscreen, outline) plus an Excellon drill file.
