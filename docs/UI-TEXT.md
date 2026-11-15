# UI text rules

Breadbox's chrome should read like the design mockups (`docs/design/screenshots/`): short labels,
live values, almost no sentences. Explanations live in **tooltips** (hover) or the **Help
window** (`docs/guide/<screen>.md`, opened from each screen's `?` button) — not in panels.

## Rules

1. **Labels** are 1–3 words. **Buttons** are a verb, or verb + noun when the verb alone is
   ambiguous: `Add`, `Remove`, `Clip CH1`, `Run analyses`. Never a sentence.
2. **No instructional sentences in panels.** "Clip a lead, then click any pin…" becomes the
   tooltip of the control it's about (`Btn::tooltip`, `Response::on_hover_text`), or a line in
   the screen's Help page if it's a concept rather than a control.
3. **Section-header captions** (the muted text right of an uppercase header) only show live
   state or context (`Pot 1 · indoor`, `virtual RF space`, `U1 · 115200 baud`). Instructions
   (`drag leads onto pins`) and restated titles (`mini scope`) are removed.
4. **Empty states**: one short line, ≤ 6 words, no period — `No probes yet`, `Select a part`,
   `No scope on the bench`. Offer a button instead of explaining how.
5. **Card subtitles**: ≤ 3 short tokens, only when they add information (`2 channels`,
   `On bench`). Drop them if the title already says it.
6. **Nothing overlaps.** Any row with right-aligned controls truncates its left text with an
   ellipsis (`egui::Label::truncate`) and puts the full text in the tooltip. Check every panel
   at the minimum window size (1280×800) and with long project / part names.
7. **Tooltips** are one sentence, ≤ ~20 words, and say what the control does or how to use it.
   Don't tooltip things that are self-explanatory.
8. **Data stays.** Numbers, units, datasheet rows, net names, serial output, plot legends and
   the design's own copy are not "verbose" — keep them.
9. **Accessible names stay descriptive.** If a visible label shortens (`Add`), keep a full
   AccessKit name (`Add Oscilloscope`) so VoiceOver and the GUI tests still find it.
10. **Out of scope:** lesson content (step text, hints, worked solutions), plugin catalog
    descriptions, and the docs themselves. The chrome around them is in scope.
11. **Help pages** (`docs/guide/*.md`) are short and task-oriented: what the screen is for, then
    a few `##` sections ("Wiring", "Probes", "The World panel"…), each a few bullet points.
    Link the right section from a `?` where a concept is genuinely non-obvious.
