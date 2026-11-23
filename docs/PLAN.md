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

Plays (slice 1), recording with MP4 folded in (slices 2 and 4), and slice 3's catalog runner have landed; what they left
undone is in `TODO.md`.

### What is reused

| Need                 | Taken from                                     | What stays ours               |
| -------------------- | ---------------------------------------------- | ----------------------------- |
| Queries, input       | `egui_kittest` 0.36 queries and input          | queries behind `PlayDriver`   |
| Structural snapshots | `insta` 1.49; Playwright `toMatchAriaSnapshot` | AccessKit tree to stable text |
| Verdicts, report     | a GPL-3.0 wasm-runtime harness, designs only   | all of the code               |

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

### Slice 4 — assertions that report themselves (next, ahead of slice 3)

A play asserts today with Rust's `assert!`, `wait_for` and `expect_action`: a failure carries whatever message its
author wrote, and a passing check leaves no trace. The model is Playwright's `expect(locator)`: matchers that retry
until they hold and fail with what was expected and what was found.

- `p.expect(by)` returns the matchers: `to_have_value`, `to_have_label`, `to_be_focused`, `to_be_disabled`,
  `to_be_checked`, `to_exist` and `to_be_absent`, with `p.expect_count(by, n)` beside them and `expect_action` joining
  the family. Each retries within the play's frame budget, so none needs a `wait_for` around it.
- Every check is recorded, passing or not, as data rather than a message: subject, matcher, expected, actual, frame.
  That one record is what the terminal's step list, the JSON report, the HTML report, the step screenshots and slice 5's
  panel all show, each in its own form.
- `Found` gains a toggle's state, which `to_be_checked` reads; it has none to read today.
- A plain `assert!` still works, and is reported as its step's failure without an expected or actual.

It comes first because slice 3's snapshots are assertions too — Playwright's `toHaveScreenshot` and
`toMatchAriaSnapshot` — so they land as two more matchers, recorded and reported as every other check is.

### Slice 3 — verdicts for catalogs

- Pixel comparison behind a `verify` feature, keeping dify's MPL-2.0 corner opt-in: `p.expect_screenshot("label")` and
  `--capture --verify`, with the verdicts above. kittest's result drops the pixel count of a difference within budget,
  so dify is called directly. References go in a co-located `__snapshots__/`, named
  `<file-stem>__<module_path>__<label>`; `UPDATE_SNAPSHOTS=1` takes a change.
- Verdicts in the JSON report, and the HTML report. Partial-frame APNGs decode at well over real time in ffmpeg (and so
  mpv); how browsers play them is judged there. The terminal summary drops its empty columns and gives a recording's
  length beside its frame count.
- `p.expect_aria_snapshot("label")`: the AccessKit tree (role, label, value, focus, toggled) as text, through `insta`.

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

### Open

- Whether `verify` keeps its feature gate. MP4's gate was dropped for leaving `--record-format mp4` failing in catalogs
  that had not turned it on; the same would hold for `--verify`, against keeping MPL-2.0 out of a catalog that never
  compares images.
