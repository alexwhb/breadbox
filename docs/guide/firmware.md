# Firmware

Write the sketch for each microcontroller on the bench, compile it, flash it into the
simulation and debug it, with the bench's radios alongside.

## Sketch and toolchain

- **Verify** (⌘B) compiles the sketch. **Upload to sim** (⌘U) compiles it if needed and flashes
  it into the bench. The bench keeps its state.
- **Target** picks the microcontroller. Radio modules on the bench (ESP-01, HM-10, HC-05) are
  listed too: they run their own AT firmware. **Compile for board** builds the same sketch for
  another board.
- Examples aren't saved to disk (*Example · not saved*). ⌘S saves a copy of the project. Your
  own projects autosave.
- **Manage libraries…** searches the Arduino Library Manager. It needs the toolchain.
- The toolchain is arduino-cli plus the `arduino:avr` core, about 150 MB, installed in
  `~/Library/Application Support/Breadbox/toolchain`. On Apple Silicon without Rosetta,
  Homebrew adds a native avr-gcc.
- Without a toolchain, Upload uses the project's prebuilt firmware if it ships one.
- *arduino-cli: no answer* means arduino-cli didn't respond in time and was stopped. Breadbox
  gives it its own settings folder (never `~/Documents`), so it's probably stuck or slow.
  **Retry**, or **Install toolchain** for Breadbox's own copy.

## Serial monitor

- Shows the selected board's serial output at 115200 baud.
- Type a line and press Enter to send it. A newline is added.

## Debugging

- Turn on **Step · breakpoints**. The next upload builds with `-Og`, so lines and variables
  match the source.
- Click the gutter to set a breakpoint. It's saved even while step mode is off, and armed when
  step mode is on.
- The strip above the editor has Continue, Pause, Step over, Step into and Step out.
- **Debug → Variables**: call stack, locals and globals, registers. Release builds keep only
  a few locals.
- **Debug → Memory**: SRAM, flash and EEPROM as hex. Type an address or a pointer name (`SP`,
  `X`, `Y`, `Z`) and press **Go**. Amber bytes changed since the previous pause.
- **Debug → I/O registers**: every peripheral register with its bits. Filter by name
  (`PORT`, `TCCR`).
- Hex views are AVR-only. ESP32 and Pico sketches run API-level (compiled for your Mac), so
  they have no SRAM, flash or I/O space to show.

## Radios

- An ESP-01 or ESP32 on the bench joins the virtual access point **SIM-AP**. It gets DHCP,
  DNS and MQTT on the built-in broker `mqtt://10.0.0.1:1883`.
- **Placement**: drag the bench on the floor plan, or click to put it there. With the bench
  selected, the arrow keys nudge it 0.5 m. Signal strength follows distance and walls.
- **Walls**: **Add wall** puts an interior wall (−15 dB) between the access point and the bench.
  Click its tag to make it concrete (−25 dB).
- **Packet log**: every frame on the air. **Export CSV…** or **Export pcap-ng…** saves it.
  Open the pcap-ng in Wireshark (the IP frames are synthetic).
- **Network conditions**: latency, jitter and packet loss. *Drop the AP at t = N s* takes the
  access point down so you can test reconnecting.
- **Bridge to my real LAN broker** forwards MQTT to a broker on your network (`host:port`).
- Radio settings are saved with the project, and ⌘Z undoes them.

## Bluetooth and the virtual phone

- **Add BLE hat** wires an HM-10 to the first board (VCC, GND, TXD→D10, RXD→D11; RX2/TX2 on an
  ESP32, GP1/GP0 on a Pico). It advertises as `HMSoft`.
- **Virtual phone** is a BLE central. It can scan, connect, and read, write or subscribe to
  characteristics. **Terminal** talks to HM-10 FFE1, Nordic UART or HC-05 SPP; **Log** lists
  the traffic.

## MQTT explorer

- Subscribe to topics on the built-in broker. `+` matches one level and `#` matches
  everything below it (`garden/#`).
- Filter the message list, or publish with QoS 0/1 and *Retain*.
