# Lessons

Guided tracks on a live bench. Each lesson walks through a real circuit one step at a time.

## Taking a lesson

- Pick a track and a lesson on the left. A tick marks lessons you've finished.
- Steps ask you to wire something, pick a part, measure, answer a question or upload a sketch.
  **Goals** tick green as you meet them, and **Next** lights up once they all hold. Hover a
  disabled Next to see what's left.
- **Do the math myself**: predict the value, then **Check** it against the meter. **Show me the
  working** walks through the calculation.
- **Open in free play** copies the lesson's bench into your projects so you can keep
  experimenting.
- The speaker button in the top bar turns the success chime on or off.

## When you're stuck

- **Test circuit** checks every goal now. Each open goal says what's wrong, where it is on the
  bench and what to do next. The bench marks it: green for what's already connected, pulsing
  amber rings where to go, a red marker on a wrong wire end (hover it for a note).
- While you drag a wire, the hole under the pointer says what it joins (`column 5 · R1`).
- **Hint** shows the step's hints one at a time. After the last one, **Show me** draws the next
  move on the bench (a ghost jumper, the wire to remove, the control to use) and says why it
  works.
- **Do it for me** does the step for you and shows why it works, so you can always move on.
  ⌘Z undoes its wiring.
- Some steps break a part on purpose. The damage card explains what happened, and **Repair**
  puts it back.

## Controls

- Things the bench can't show — the room's light or temperature, a pot's soil moisture, a
  part's value, a part to add, the virtual phone, an MQTT message — get a control in the step
  panel when the step needs them.
- Steps that listen to a sketch show its **Serial** output (and **MQTT** / **Phone** messages).

## Code steps

- **Compile & upload** builds the step's sketch and flashes it into the bench.
- Without the Arduino toolchain, **Load the reference build** flashes the lesson's prebuilt
  solution.

## Make your own lesson

- Give it a title, a track and a lesson pack name.
- **Add step** offers these kinds: *explain* (what the circuit does), *build* (wiring),
  *predict*, *measure* (an instrument reading), *experiment*, *break* (damage on purpose),
  *quiz* (a question) and *code* (a sketch).
- **Goals**: pick a part for suggested goals, or type two pins (`r1.1`, `d1.a`). A step without
  goals is read, then **Next**.
- **Capture bench** copies the open Workbench (or lesson) bench as the lesson's start circuit.
- **Validate** checks that the lesson loads and that every step can be completed.
- **Save lesson pack** installs it, so it appears under Lessons. **Package .bbplugin…** makes
  a file to share. Others install it from Plugins → **Load local plugin…**. A lesson pack is a
  plugin of kind `lesson-pack`.
- The full file format is under Help → *Lesson format*.
