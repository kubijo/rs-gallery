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

### What is reused

| Need                       | Taken from                                      | What stays ours                     |
| -------------------------- | ----------------------------------------------- | ----------------------------------- |
| Queries, input             | `egui_kittest` 0.36 queries and input           | queries behind `PlayDriver`         |
| Play functions, steps      | Storybook `play()`, several tests per story     | `#[play]` and the step log          |
| Waiting                    | Playwright's retrying web-first assertions      | `wait_for`, budgeted in frames      |
| Spies                      | Storybook `fn()`                                | `gallery::action`, armed headlessly |
| Runner                     | Storybook's move to Vitest; `libtest-mimic` 0.8 | trial enumeration                   |
| Structural snapshots       | `insta` 1.49; Playwright `toMatchAriaSnapshot`  | AccessKit tree to stable text       |
| Verdicts, report, gestures | a GPL-3.0 wasm-runtime harness, designs only    | all of the code                     |
| Video (APNG, MP4)          | `png` 0.18; `openh264` 0.9, `muxide` 0.5        | frame diffing and timing            |

Kittest supplies `get_by_label`/`_role`, `click`, `hover`, `type_text`, `focus`, `key_*`, `drag_at` and `scroll_*`.
Storybook's play function is `play({ canvas, userEvent, step })`; the harness's designs are re-implemented, no code
taken.

From the wasm-runtime harness, as designs:

- Time is virtual. Frame *n* sits at an exact time and nothing reads the wall clock.
- Interactions name an element — here an AccessKit label or role — never a coordinate. Each gesture is a fixed frame
  sequence (click: press, frame, release, frame; drag: ten frames; scroll: five moves), so a script yields the same
  frames on every machine.
- Settle, then shoot. `settle()` stopping once `has_requested_repaint` goes quiet is already the egui form of it.
- Verdicts are `pass | tolerated | diff | missing | new`: a colour threshold, plus a budget of differing pixels inside
  which a diff is *tolerated* — kept and reported, not failed. Rasterisers flip steep antialiased edges at full
  contrast, which no colour threshold absorbs; the budget is what lets llvmpipe and a GPU share references. A missing or
  new reference fails.
- The HTML report is written to a temporary name and renamed into place, since a report existing is the evidence that a
  verdict was reached. Animated results sit side by side as two APNGs; nothing is re-encoded to compare them.
- CI uploads the artifacts before a separate step fails on them.

Left out of it: its event recorder (no recorder for now), network and device fixtures, and archived baselines, which pay
off only past the reference count this repo has.

### Video format

APNG by default. Pure Rust, lossless, byte-identical between runs — so a recording can itself be a test — and it plays
inline wherever an image does. Encode cost is held down by `Compression::Fast` and by writing only the rectangle that
changed since the previous frame; a frame with no change writes nothing and lengthens the one before it. Frames reach
the encoder through a channel two or three deep, which bounds memory and overlaps encoding with rendering.

An MP4 behind a `video-h264` feature (`openh264` and `muxide`) for small, lossy files with no external binary; it
compiles bundled C++, hence opt-in.

Frame timing holds by construction: frame *n* gets `RawInput.time = n / fps` set outright rather than accumulated, and
APNG stores delays as the fraction `1/fps`, summed exactly when frames merge. How fast a frame renders or encodes cannot
move it.

Ruled out: an `ffmpeg` subprocess (an external binary), `rav1e` (AV1 encode cost), animated WebP (C libwebp, slower, no
gain over APNG with changed-rectangle frames), GIF (256 colours), `gifski` (AGPL), the `webm` muxer (MPL-2.0).

Slice 1, play functions run headlessly, has landed; what it left undone is in `TODO.md`.

### Slice 2 — recording

- `src/video.rs`: a `VideoSink` with one implementation, APNG, on an encoder thread. The frame count `acTL` declares is
  known only once the stream ends, so the chunk — fixed-size — is patched in place with its CRC on close.
- A recording steps at `1/fps` with the time set per frame, and restores the animation time and cursor blink kittest
  zeroes; stills keep both off.
- `record = { fps = 60, seconds = 2 }` on a shot; on a play shot, `record = { fps = 60 }` records the whole play, its
  steps as frame ranges in the report. `video = "on" | "retain-on-failure"`; `--record FPS:SECONDS`.
- Tests decode the result: frame count, delays summing exactly to the duration, every frame equal to a still rendered at
  its time, a static scene collapsing to one frame, the same bytes twice.
- The report carries per-frame render, readback and encode time and the deepest the queue got.

### Slice 3 — a runner for catalogs

- `gallery::test_main!()` over `libtest-mimic` for a `harness = false` test in the template, linking scenes through
  `Linked`: `smoke::<scene>` (renders without panicking; never settling warns) and `play::<scene>::<name>`. Filtering,
  `--list` and nextest come with it. The glow session is `Rc`, so each test thread opens its own.
- Pixel comparison behind a `verify` feature, keeping dify's MPL-2.0 corner opt-in: `p.snapshot("label")` and
  `--capture --verify`, with the verdicts above. If kittest's result does not expose the pixel count *tolerated* needs,
  dify is called directly — the first thing to find out. References go in a co-located `__snapshots__/`, named
  `<file-stem>__<module_path>__<label>`; `UPDATE_SNAPSHOTS=1` takes a change.
- Verdicts in the JSON report, and the HTML report.
- `p.aria_snapshot("label")`: the AccessKit tree (role, label, value, focus, toggled) as text, through `insta`.
- A `test` recipe in the template justfile.

### Slice 4 — MP4

`openh264` and `muxide` as a second `VideoSink` behind `video-h264`; `video_format = "apng" | "mp4"`. A test reads the
file back with `mp4` (a dev-dependency) for its sample count and an unchanging sample duration.

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

- An APNG of many partial frames may play slowly in some viewers; to be judged on slice 2's output.
