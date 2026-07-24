# TODO

## Publishing to crates.io (deferred)

Not required while the repo is source-only — each consumer's Cargo fetches the crates themselves. Do this only before an
actual `cargo publish`. Research: crates.io Trusted Publishing (RFC 3691), Cargo publishing reference.

### Manifests

- [ ] Flip `publish = false` on all three crates (`gallery`, `gallery-macros`, `gallery-build`).
- [ ] Add `repository = "https://github.com/kubijo/rs-gallery"` to `gallery-macros` + `gallery-build` (`gallery` already
  has it). Recommended metadata, and it's the repo a Trusted Publisher binds to; `license`/`description`/`readme`/
  `keywords`/`categories` are already set.

### Publish order (path-dep rule)

- [ ] crates.io forbids publishing a crate with a path-only dependency, so publish `gallery-macros` + `gallery-build`
  first, then `gallery` (it path-depends on `gallery-macros`). `release-plz` / `cargo-release` handle this ordering.

### Template

- [ ] `template/` depends on `gallery = { git = … }`; add a registry-dep variant (`gallery = "0.1"`) for post-publish
  consumers, or keep git while pre-release.

### Trusted Publishing (the only provenance mechanism crates.io has)

- [ ] Register a Trusted Publisher per crate on crates.io: owner `kubijo`, repo `rs-gallery`, workflow `release.yml`,
  environment `release`.
- [ ] Add `.github/workflows/release.yml`: tag-triggered, `permissions: id-token: write`,
  `rust-lang/crates-io-auth-action` → `cargo publish`. No long-lived `CRATES_IO_TOKEN` secret.
- [ ] crates.io account needs a verified email.
- Artifact provenance / sigstore attestation is explicitly out of scope on crates.io (RFC 3691) — nothing to set up
  there.

### Tagging

- [ ] Tag every release for `version ↔ commit` auditability. Workspace convention: `<pkg>-v<version>` (e.g.
  `gallery-v0.1.0`), or a single `v0.1.0` if the three version in lockstep.
- [ ] Consider `release-plz` — release PR from the Keep-a-Changelog `CHANGELOG.md` → tag → publish.

## Perf: honest measurement, profiling, scene snapshots

Prior art is `bmc-wasm-runtime`'s `just profile` (→ `tools/perf_record.py`): samply CPU sampling plus internal
per-frame/per-section timing, an A/B `perf_compare.py` ledger, and a `capture`/`verify` visual-regression suite. Most of
it ports; the fuel/instruction-count layer is wasmi-specific and doesn't (its native analogue is wall-clock spans).
Ordered by value: honest meter → profiler → snapshots.

Motivation — the current perf footer is not trustworthy (`src/lib.rs`), and fixing it is the prerequisite for everything
else here:

- Expanding the panel calls `ctx.request_repaint()` every frame, so *observing* the numbers pins the loop to max FPS —
  the reading measures "we're repainting because you're watching," not real work.
- It records `ui.input(|i| i.stable_dt)`, the inter-frame interval (≈ vsync), not frame work — so the "16.7 ms" it shows
  is the vsync cap, not a render cost.

### Tier 3 — honest frame-cost meter in its own viewport (do first)

Separate observer from observed by construction: a deferred egui viewport (`gallery · perf`) with its own repaint clock,
so its liveness never drives the main loop and its own draw cost lands in its own budget. The main window stays
reactive; idle → it holds the last real frame's numbers.

- [x] **Spike — answered: deferred viewports self-sustain, no keep-alive needed.** With the parent never self-repainting
  it went quiet at frame 20 (t=1.72s) and never ran again, while the perf viewport kept running its own closure at the
  requested ~5 Hz through t=7.4s (tick 46). The parent window's UI visibly froze at its last painted values, confirming
  it really had stopped. So an idle main loop sits at **zero** forced repaints while the perf window stays live.
- [x] Measures work, not interval — the shell's `ui()` is self-timed with an `Instant`. `frame.info().cpu_usage` was the
  obvious primitive and is wrong here: eframe reports it per *viewport* redraw with no root check, so the perf window's
  own repaints overwrite it and the meter ends up charging the shell for the instrument. Self-timing covers the shell's
  build only; tessellate and paint sit outside it, which is what the GPU layer below is for.
- [x] Main loop: dropped the forced `request_repaint()` and the `stable_dt` record; stats shared via
  `Arc<Mutex<PerfStats>>`. The shell is fully reactive now — an idle gallery forces zero repaints.
- [x] Perf window is a deferred viewport (`gallery · perf`) on its own ~10 Hz clock, reading the stats read-only. The
  bottom panel is gone; a `Perf` toggle sits in the top bar beside `Debug`, and closing the window unticks it via a
  single requested parent repaint.
- [x] Dropped FPS entirely — it measured repaint frequency, not cost — for frame cost + p95; the sparkline plots cost
  against the 17/33 ms budget lines.
- [x] Hot-reload polling is gated on `--hot` (`HotDylib::new(lib_name, watching)`). It ran unconditionally, repainting
  the shell 5×/s forever, so the loop never came to rest for a cost reading to mean anything.
- [x] `template/animation.scene.rs` (Motion / Orbit) drives the loop so the meter has something to measure: an `animate`
  toggle demonstrates `rendering` vs `idle`, and `dots` pushes per-frame cost past the budget lines. Plain egui, so it
  exercises both renderers.
- [x] The window parks beside the shell from `outer_rect`/`monitor_size`, frozen at open so dragging it sticks. A no-op
  on Wayland, which reports no window geometry and leaves placement to the compositor; kept for the other platforms.
- [ ] GPU paint cost via wgpu `TIMESTAMP_QUERY` (async readback, N frames later) as a second layer.
- [ ] An explicit continuous/benchmark mode that deliberately drives the loop, for when a steady-state rate *is* what
  you want to read.
- [ ] Multi-viewport is native-only; if a web target ever appears, fall back to an inline reactive panel.

### Tier 2 — per-scene profiling with automatic gallery-vs-their-code attribution

`just profile <scene>`: a fixed-frame headless pump of one scene at default knobs (bmc's `--perf-frames`), under samply,
with an A/B ledger. The framework-vs-component attribution the profile must make obvious comes from stacking four
layers, three of them automatic:

- [ ] **Crate attribution (free):** samply symbolication → per-crate breakdown (`gallery` / component crate / egui /
  epaint / wgpu). Port bmc's `perf_analyze.py` + `perf_compare.py` (they only need samply's Firefox-Profiler format) and
  the colour-Δ comparison table.
- [ ] **`#[scene]` auto-span:** the `gallery-macros` proc-macro injects `profile_scope!("scene::<module_path>")` into
  the generated wrapper — per-scene names for free, no manual annotation. Gated behind a `profiling` feature (a no-op
  otherwise).
- [ ] **Gallery boundary spans:** bracket `gallery::shell` / `gallery::knobs` / `gallery::offscreen` so the egui/wgpu
  time crate attribution can't split (chrome vs scene) becomes attributable. Headline line:
  `gallery N% · scene::<name> N% · epaint N% · wgpu N%`.
- [ ] **Opt-in fine spans:** re-export `gallery::profile_scope!("label")` for component authors; nests under the scene
  span.
- [ ] Backend: puffin (egui-native scopes + `puffin_egui` in-app flamegraph, feature-gated) for the live/semantic view;
  samply for the external CPU + A/B view. The same spans feed the Tier-3 perf window.

### Tier 1 — scene snapshots (parked)

Static snapshots are thin without interaction replay (which gallery lacks — bmc's testbed has it). Worth revisiting once
a record-replay layer exists; far more valuable then.

- [ ] Snapshot-backend-agnostic API (`snapshot!`) that can target **pixels** (egui_kittest wgpu image + an odiff-style
  diff) *or* **structure** (serialize the scene's AccessKit tree / `Shape` list — jest-style, text-diffable in a PR, no
  GPU/AA flakiness). Pick per use; don't hard-code one.
- [ ] Design a record-replay layer for knob/pointer interactions first (informed by bmc's `record` / `--perf-frames` and
  jest snapshot ergonomics); without it these stay static default-knob renders.
- [ ] Storage location: default to **co-located with the file of origin**, derived by the `snapshot!` macro itself from
  `file!()` + `module_path!()` + its label, so a path is never written by hand. Overriding the *root* is then the only
  config needed, and it should be strongly-typed Rust in the snapshot harness — the harness is a test binary, so its
  config is just code — rather than a new `gallery.toml` key. Either way it is test-time config, so it stays off the
  runtime `Settings` passed to `launch!`.
- [ ] Key the name on `<file-stem>` + scene/`module_path` + label, not file + label alone: two scenes in one file can
  reuse a label. Keep the base name backend-agnostic so a pixel (`.png`) and a structural (`.snap`) snapshot of the same
  point coexist. Prefer a co-located `__snapshots__/`-style subdir (jest's convention) — still travels with the file and
  reviews next to the code, without filling scene dirs with artefacts.

## Later

- [ ] Auto-re-sync an instance's scaffolded files (`justfile` / `build.rs` / `main.rs`) from upstream as the template
  evolves. Hard to do safely — it must not clobber consumer edits — so `just update`'s "you're behind, here's what
  changed" is the lightweight stand-in for now.
