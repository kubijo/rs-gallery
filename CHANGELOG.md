# Changelog

Notable changes to `gallery`, newest first, following [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Pre-1.0,
so a minor release may carry a breaking change.

## [Unreleased]

First public pre-release — it all lands in the initial commit, so there's nothing to migrate from yet; this log records
changes from the first tagged release on. Highlights (the [README](README.md) has the detail):

- **Discovery** — `#[scene]` / `scene_meta!` self-register via `inventory`; `build.rs` globs `*.scene.rs` from config.
  Scenes reach the shell `Linked` (compiled in) or via `HotDylib` (rebuilt and hot-swapped on `--hot`). A `**` glob from
  a crate root walks into `target/`, so a match found there is skipped as build output, and an entry that goes missing
  mid-walk — cargo writes and removes temp files as it builds — is skipped rather than failing the build.
- **Controls** — `text`, `slider`, `toggle`, `color`, `select` / `radio` / `buttons`, `pad2d`, `group`;
  declarative-by-use, with values persisted per scene across reloads.
- **Scenes read as documents** — the canvas is plain, so headings and prose drawn onto `ctx.ui` look like headings and
  prose. Each thing being demonstrated goes in a `stage!`, which carries the checkerboard, a size caption and a collapse
  toggle: a bare closure fits the content, `fill` takes the rest of the canvas, and anything else is a size —
  `(300.0, 200.0)`, `(300, 200)` or `200` for a square all convert, so a call site writes the dimensions the way it
  already holds them. A scene with several stages and prose between them is the point; a scene that is a single `fill`
  is the whole-canvas shape every scene had before stages, said out loud.
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
- **Headless render** — `--render <path.png> --scene <pattern>` writes a scene's canvas with no window, so an agent or
  CI can look at a component instead of asking someone for a screenshot. Drawn by the same code the shell draws with, so
  the image is the canvas alone and holds still as the chrome around it changes. The size is where the scene lays itself
  out rather than a crop of it: one that comes out bigger is captured whole, at the size it turned out to need.
- **Sheets** — a recipe's root-level `sheet = "sheet.png"` also gathers that run's captures onto one captioned image, so
  a change across a whole set is one thing to look at rather than a directory to click through. `rectangle-pack` places
  the panels; the sheet size is searched over candidate widths and scored on area *and* proportions, since a column and
  a strip cover the same page and both leave every panel unreadable once scaled to a screen. The shots still write their
  own PNGs. One capture writes no sheet: a sheet of one image is the image.
- **Capture recipes** — the defaults are rarely the state worth seeing, so `--capture <file.toml>` renders a list of
  shots, each naming its scene, size and knobs. A file rather than flags: labels contain spaces that shell wrappers
  drop, and a set of states is worth committing. Knobs go by label (or a regex over them), choices by option label —
  both survive a reordering. Values are applied between frames rather than seeded, since a knob doesn't exist until the
  scene asks for it. Anything that can't be honoured stops the run: a clean render of the wrong state is worse than
  none.
- **Recipe generator** — `--init-capture` prints a recipe with every knob at the value its scene declared, so the
  starting point already renders and the first edit is the state you were after.
- **Capture follows the renderer** — under `Renderer::Glow` it paints through an OpenGL context taken off an EGL device,
  with no window and no display server, so a scene drawing with `ctx.offscreen(...)` captures its content rather than
  the hint wgpu would leave in its place. egui_kittest ships no glow renderer, so this is one of gallery's own; the
  texture a scene registers goes through `egui_glow`'s painter, since the `Frame` a test harness builds carries no glow
  hook and nothing outside eframe can give it one.
- **Reference images** — captures are diffed against committed PNGs, so a change to stage sizing, the bundled fallback
  faces, a knob override or the offscreen GL path fails a test instead of going unnoticed. They come from an ordinary
  capture recipe (`tests/reference.toml`), which makes the fixture exercise what a caller writes rather than the structs
  it parses into. `UPDATE_SNAPSHOTS=1` takes an intended change and keeps the old image beside it. Comparable between
  machines only because the tests pin llvmpipe from the pinned mesa — a GPU antialiases the same scene its own way.
- **Command line** — parsed by clap, so `--help` lists the arguments and a bad one is an error, not a panic. Failures
  that come down to a choice name the candidates in a framed list, styled through `anstream` so a pipe or `NO_COLOR`
  gets the same text plain.
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
  own `[workspace]` plus a `justfile` with `just run` / `just hot` / `just update` / `just render` / `just capture` /
  `just knobs` / `just capture-init`), carrying example and knob scenes plus an animated one that drives the render loop
  for the performance window to measure, and a `capture.toml` showing the recipe format against those scenes.
- **Update check** — `just update` (`cargo run -- --check-updates`) fetches the upstream CHANGELOG over HTTPS and prints
  what's changed since the `gallery` version you're building against.
- **Docs formatting** — code fences in Markdown are formatted as code rather than left as prose: `just format` sends
  Rust fences through the rustfmt this repo already pins, Bash fences through beautysh and TOML fences through taplo, so
  the README's examples carry the same style as the source they mirror. taplo formats the `.toml` files too — bar
  `Cargo.toml`, which cargo itself writes.
