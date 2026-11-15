# Security

Breadbox runs third-party code in two places, and both are meant to be safe to open:

- **Plugins** run in a wasmtime sandbox with fuel, time and memory limits, and can only reach
  the circuit, the world, the radios, the project folder or the network through the
  permissions declared in `plugin.json` and approved by the user.
- **Project files and netlists** are parsed defensively; malformed input must never crash the
  app or touch anything outside the project.

ESP32 / Pico sketches are compiled and run as native child processes (API-level emulation), so
only open sketches you trust — the same as running any program.

## Reporting a vulnerability

If you find a way for a plugin to escape its sandbox or exceed its permissions, for a project
file to read or write outside its folder, or anything similar, please report it privately with
[GitHub's private vulnerability reporting](https://github.com/alexwhb/breadbox/security/advisories/new)
rather than in a public issue. Include steps to reproduce (a minimal plugin or project is ideal).
You'll get a reply as soon as possible, and credit in the fix if you'd like it.
