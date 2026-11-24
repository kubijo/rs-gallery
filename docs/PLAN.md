# Plan

Where work in flight is described: the shape of a change, what was ruled out, and anything deferred while it is being
built. Source comments point here rather than carrying "next we'll…" notes of their own.

A plan lives here only while it is being implemented. What landed graduates to `CHANGELOG.md`, what is parked moves to
`TODO.md`, and the reasoning stays in the commits — so a stale plan is worse than none.

## Interactions, a test runner, and video

A capture can set knobs but not hover, focus, type or drag, so a scene's interactive states can be neither shown nor
tested. The model to reach is Storybook's: a scene is a smoke test, a play function makes it an interaction test, and
any run can be recorded. It is built from existing parts rather than invented here.

The work is sliced so every slice is checked headlessly — tests in `just dev::validate`, artifacts under `.tmp/` — and
the in-window panel, the one part a person would otherwise have to click through, comes last.

Plays (slice 1), recording with MP4 folded in (slice 2), slice 3's catalog runner and slice 4's assertions have landed;
what they left undone is in `TODO.md`.

### What is reused

| Need                 | Taken from                                   | What stays ours               |
| -------------------- | -------------------------------------------- | ----------------------------- |
| Queries, input       | `egui_kittest` 0.36 queries and input        | queries behind `PlayDriver`   |
| Assertions           | Playwright `expect`; jest-dom's state names  | retrying over `By`, in frames |
| Value matchers       | `googletest` 0.14 `Matcher` and its matchers | the widget matchers over them |
| Interaction log      | Storybook's Interactions panel               | one log for every view of it  |
| Structural snapshots | `similar` 3; `egui_inspection_tools`' JSON   | AccessKit tree to stable text |
| Verdicts, report     | a GPL-3.0 wasm-runtime harness, designs only | all of the code               |

From the wasm-runtime harness, as designs:

- Verdicts are `pass | tolerated | diff | missing | new`: a colour threshold, plus a budget of differing pixels inside
  which a diff is *tolerated* — kept and reported, not failed. Rasterisers flip steep antialiased edges at full
  contrast, which no colour threshold absorbs; the budget is what lets llvmpipe and a GPU share references. A missing or
  new reference fails.
- The HTML report is written to a temporary name and renamed into place, since a report existing is the evidence that a
  verdict was reached. Animated results sit side by side as two APNGs; nothing is re-encoded to compare them.
- CI uploads the artifacts before a separate step fails on them.

Left out of it: its event recorder, network and device fixtures, and archived baselines, which pay off only past the
reference count this repo has.

### Slice 3 — verdicts for catalogs

Both snapshots are matchers like slice 4's, Playwright's `toHaveScreenshot` and `toMatchAriaSnapshot`: retried within
the frame budget, recorded in the play's log as a check, and failing with the same panel. A check keeps the reference,
the actual and the diff, which slice 5's panel shows.

Tests and captures stay apart. A test runs a play once, at one size and scale with the scene's own knobs, and judges its
snapshots; a capture makes pictures and recordings of a play at any of those, and judges none, logging each check as
skipped. Compared in captures too, a reference would have to carry every setting of the shot it was taken in.

Three pieces, each committed on its own.

#### 3a — `p.expect(Canvas).to_match_screenshot("label")`

- `expect` takes a `Subject`: a `By` (or what converts to one) gives `Expect`, the `Canvas` marker gives `ExpectCanvas`,
  which has only `within`, `because` and `to_match_screenshot`. Comparison settings travel with the name, float-cmp's
  `impl Into<Margin>` style: `"saved"` or `Screenshot::named("saved").tolerance(400)`.
- Compared by dify directly, with no feature gate (as MP4 lost its own): kittest's result drops the pixel count of a
  difference within budget.
- References sit beside the play's file in `__snapshots__/<module>/<play>/<label>.<renderer>.png`, since wgpu and glow
  never render alike. The module is the play's `module_path` without the crate, with the number gallery-build gives a
  second scene file of one name; the label is lower-cased and dashed. A label used twice in one play fails the second
  check.
- A frame that already matches passes at once, drawing nothing. Otherwise frames are drawn until two in a row ask for no
  repaint, as a shot's `settle` does, and the settled one is judged, within the wait budget. One still changing then
  fails and is never taken.
- The canvas grows to hold its content, and the crop takes in the `Area`s over it, which a content size leaves out.
- No reference yet fails ("no reference yet") and writes `<name>.new.png`; a difference past the budget fails and writes
  `.new.png` and `.diff.png`. `UPDATE_SNAPSHOTS=1` takes the settled frame as the reference instead. A size mismatch
  says both sizes.
- Budget: `.tolerance(pixels)` and `.threshold(t)` on the check, else `[snapshots.<renderer>]`, else `[snapshots]` in
  `gallery.toml`, else 0 pixels at dify's 0.1. The check logs what applied: `312 pixels differ, 400 allowed`.
- A failure panel lists the reference, the actual and the diff; the host writes them, the play only names them.
- Comparison is host-side: `PlayDriver` gains a compare, and a keep that writes the files for the final look only.

#### 3b — `to_match_accesskit_snapshot("label")` and `--capture --verify`

- The AccessKit tree of the canvas or of one widget's subtree, as the JSON `egui_inspection_tools` writes: the nearest
  thing egui has to a text format, which needs egui 0.37, keeps ids and drops the toggled, selected and expanded states.
  Not Playwright's YAML, whose ARIA roles egui's would have to be translated to.
- Compared and diffed with `similar`, with 3a's `.new` and `UPDATE_SNAPSHOTS` flow rather than insta's, so a failure is
  a check in the log, not a panic. `<label>.accesskit.json`, with no renderer.
- `--capture --verify` compares every shot's PNG with `__snapshots__/<shot>.<renderer>.png` beside the recipe: captures'
  own verdict, with nothing of plays in it.

#### 3c — the HTML report

- Verdicts and attachments in an HTML report. Partial-frame APNGs decode at well over real time in ffmpeg (and so mpv);
  how browsers play them is judged there.
- The terminal summary drops its empty columns and gives a recording's length beside its frame count.

### Slice 5 — an interactions panel driving the live canvas

- `LiveDriver`: AccessKit enabled on the window's context, each frame's `TreeUpdate` fed to its own `kittest::State` and
  searched there, in host code, as the headless driver does; events queued for eframe's `raw_input_hook` to put into the
  next frame. The play runs on a worker thread; `advance_frame` hands its events over and blocks until that frame's tree
  comes back. Play bodies stay straight-line code, and neither `Harness` nor `Context` crosses a thread.
- The panel lists a scene's plays and their steps — run, pause, step, restart (the scene's knobs and temporary memory
  reset, then from step 0) — marks the failing step with its error and the labels last seen, and replays on hot reload.
- Live frames follow vsync, so a scene that depends on time can differ from a headless run; the headless run is the one
  whose verdict counts.
- Checked through kittest tests of the shell that run a play via `LiveDriver`, and a headless image of the panel.
