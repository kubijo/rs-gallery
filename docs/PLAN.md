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
  verdict was reached.
- The report's minimap (a rail of every section, coloured by verdict, that jumps to one), its filter by verdict, and its
  lightbox, which steps through reference, actual and diff in place.
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

#### 3b — `to_match_accesskit_snapshot("label")`

- The AccessKit tree of the canvas or of one widget's subtree, as the JSON `egui_inspection_tools` writes: the nearest
  thing egui has to a text format, which needs egui 0.37, keeps ids and drops the toggled, selected and expanded states.
  Not Playwright's YAML, whose ARIA roles egui's would have to be translated to.
- Compared and diffed with `similar`, with 3a's `.new` and `UPDATE_SNAPSHOTS` flow rather than insta's, so a failure is
  a check in the log, not a panic. `<label>.accesskit.json`, with no renderer.

#### Videos of catalog tests

- `[tests]` in `gallery.toml`: `video = "off" | "failing" | "always"`, the recipes' words, and the `output` directory
  videos and the report share, as coverage has one. `failing` and `always` record alike and differ in what they keep.
- A video only watches: a test keeps its own time (0.25 s a frame, kittest's) and no animations, and a recording that
  breaks stops without failing the test. A scene can tell it is filmed by its paint callbacks running, and by the wall
  clock. A callback's panic ends the process: `egui_wgpu::Callback` and `egui_glow::CallbackFn` keep theirs private, so
  nothing in the dylib can be put around it. The dylib's panic hook names the scene first.
- A frame painted from the same shapes as the last is held, not rendered again.
- A kept video is an MP4, which a report can pause and seek. A check can grow the canvas and an H.264 stream cannot, so
  a play is recorded as an APNG that grows with it, and becomes an MP4 that holds its every frame only if it is kept.
  The input is noted frame by frame and laid over them then too, so a passing test does not pay for it.
- A recording is made in cargo's target directory, and only a kept video goes to `output`, which may be the user's own:
  what comes and goes there would show in git and set off a hot rebuild.
- Each test writes its own `<crate>/<module>/<play>/video.<renderer>.mp4` alone, since nextest runs every one in a
  process of its own, and a package may test under both renderers.

#### 3c — the HTML report

- Verdicts and attachments in an HTML report at the root of the tests' `output`, linking only to files in there, so CI
  can upload the directory as it is. It reads top to bottom without JavaScript.
- A minimap of scenes and their plays that jumps to each, a filter by verdict, and a lightbox with zoom that steps
  through reference, actual and diff. A line of the log seeks the play's video to it.
- Under nextest each test writes its own result, then rebuilds the page from all of them under a file lock. One that did
  not run keeps its earlier result, marked as such.
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
