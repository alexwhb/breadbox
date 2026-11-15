# Lessons

Guided tracks on a live bench. Each lesson walks through a real circuit one step at a time.

## Taking a lesson

- Pick a track and a lesson on the left. A tick marks lessons you've finished.
- Steps can ask you to wire something, pick a part, measure, answer a question or upload a
  sketch. **Next** lights up once the step's goals are met.
- **Do the math myself**: predict the value, then **Check** it against the meter. **Show me the
  working** walks through the calculation.
- **Need a hint?** shows the step's hints.
- Some steps break a part on purpose. The damage card explains what happened, and **Repair**
  puts it back.
- **Open in free play** copies the lesson's bench into your projects so you can keep
  experimenting.

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
