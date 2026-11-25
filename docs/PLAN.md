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

Plays (slice 1), recording with MP4 folded in (slice 2), a catalog's tests with their snapshots, videos and report
(slice 3) and the assertions (slice 4) have landed; what they left undone is in `TODO.md`.

### Slice 5 — an interactions panel driving the live canvas

- `LiveDriver`: AccessKit enabled on the window's context, each frame's `TreeUpdate` fed to its own `kittest::State` and
  searched there, in host code, as the headless driver does; events queued for eframe's `raw_input_hook` to put into the
  next frame. The play runs on a worker thread; `advance_frame` hands its events over and blocks until that frame's tree
  comes back. Play bodies stay straight-line code, and neither `Harness` nor `Context` crosses a thread.
- The panel, after Storybook's Interactions, lists a scene's plays and their steps — run, pause, step, restart (the
  scene's knobs and temporary memory reset, then from step 0) — marks the failing step with its error and the labels
  last seen, and replays on hot reload. It reads the log the terminal and the report show, and shows the reference, the
  actual and the diff a snapshot check keeps.
- Live frames follow vsync, so a scene that depends on time can differ from a headless run; the headless run is the one
  whose verdict counts.
- Checked through kittest tests of the shell that run a play via `LiveDriver`, and a headless image of the panel.
