# Plugins

Plugins add instruments, parts and boards, radios, world models, solvers and analyses, themes
and lesson packs. Each one is a sandboxed package.

## Installing

- **Install** opens a permission prompt first. A plugin only touches the circuit through the
  permissions it declares (`read:nets`, `inject:load`, `drive:pins`, `world:read`,
  `world:write`, `radio:space`, `ui:panel`, `fs:project`, `net:lan`).
- *Planned* plugins aren't in this version yet. Hover the button to see what's planned.
- **⋯** on a plugin's page has **Disable** and **Uninstall…**. Benches using an uninstalled
  plugin's parts show them as unknown until you reinstall it.
- World models run on every bench while they're enabled.

## Instruments

- **Place on bench** puts the instrument on the open project's bench. If it replaces the bench
  meter, the probes stay on the same nodes.
- Drag the probe tips onto any two nodes.
- On a running bench, the callout shows how much the instrument's input impedance shifts the
  node, the way it would on a real bench.

## Your own plugins

- **Load local plugin…** loads a folder with a `plugin.json`, or a `.bbplugin` package.
- **Edit model** makes a local copy in the `plugin-dev` folder and opens it. Saved changes
  reload by themselves and restart the bench.
- **Plugin SDK docs** has the manifest format, the host API and examples.
