# Changelog

Notable changes to `gallery`, newest first, following [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Pre-1.0,
so a minor release may carry a breaking change.

## [Unreleased]

First public pre-release — it all lands in the initial commit, so there's nothing to migrate from yet; this log records
changes from the first tagged release on. Highlights (the [README](README.md) has the detail):

- **Discovery** — `#[scene]` / `scene_meta!` self-register via `inventory`; `build.rs` globs `*.scene.rs` from config.
  Scenes reach the shell `Linked` (compiled in) or via `HotDylib` (rebuilt and hot-swapped on `--hot`).
- **Controls** — `text`, `slider`, `toggle`, `color`, `select` / `radio` / `buttons`, `pad2d`, `group`;
  declarative-by-use, with values persisted per scene across reloads.
- **Shell** — tree sidebar with fuzzy filter and keyboard nav, Preview/Source and Debug toggles, collapsible panels, and
  mesh-tessellated SVG icons.
- **Performance window** — frame cost and p95 in a separate viewport on its own repaint clock, so watching the numbers
  never drives the loop being measured. Reports the cost of building a frame, not the interval between frames, and holds
  still when the shell is idle rather than manufacturing traffic to look live.
- **Profiling** — `--frames <n>`, optionally with `--scene <key>`, renders a fixed count and exits, so two recordings
  are comparable rather than however long you happened to sit there. `just profile <scene>` records one under samply
  into `reports/`, resolving its addresses to symbols on the spot — samply defers that to view time, and by the next
  build the binaries the addresses point into are gone. `gallery-perf analyze` (the uv package in `tools/`) then splits
  self time per crate — separating gallery's own cost from the component's — over the samples that were drawing rather
  than parked on the event loop, reporting how many that excluded.
- **Host overrides** — `Settings` (e.g. the Controls-panel width) and `apply_default_style`, layered under the host's
  `setup` closure.
- **Fonts** — bundled Noto fallback faces (Sans, Symbols, Symbols 2, Math; SIL OFL 1.1, in `fonts/noto/`) fill the
  arrow/math/symbol glyphs egui's defaults render as tofu. Appended to each family's fallback chain, so the default look
  is unchanged; CJK/emoji stay out (add per-consumer via `setup`).
- **Renderer** — `Settings::new(Renderer)` picks the eframe backend (`Wgpu` or `Glow`), a required choice with no
  default. Under `Glow`, a scene renders non-egui content into an offscreen framebuffer with `ctx.offscreen(...)` — or
  the raw `ctx.gl_loader()` / `ctx.register_native_texture(...)` beneath it — at its own femtovg/glow version, which
  gallery never pins. `just demo-wgpu` and `just demo-femtovg` run the two backends; the femtovg demo exercises the
  offscreen path.
- **Scaffolding** — `cargo generate … template --name <dir> --no-workspace` lays down a standalone instance crate (its
  own `[workspace]` plus a `justfile` with `just run` / `just hot` / `just update`), carrying example and knob scenes
  plus an animated one that drives the render loop for the performance window to measure.
- **Update check** — `just update` (`cargo run -- --check-updates`) fetches the upstream CHANGELOG over HTTPS and prints
  what's changed since the `gallery` version you're building against.
