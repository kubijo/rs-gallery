# Changelog

Notable changes to `gallery`, newest first, following [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Pre-1.0,
so a minor release may carry a breaking change.

## 2026-08-06

- **Sidebar folding** — `Settings::collapsed(..)` says which folders a gallery opens with folded: `true` for all of
  them, or a list of top-level names (compared without case) for those and no others. A starting state rather than a
  fixed one — opening a folder keeps it open for the session, and a filter still reaches into one that started folded. A
  name matching no root folds nothing, as does one naming a title of a single segment, which the sidebar shows as a
  scene rather than a folder.
- **Fixed** — only the first shot of a `--capture` run drew, for any scene holding a GL object between frames. Each shot
  built its own EGL context and dropped it with its harness, while the scene's cache lives in a thread-local in the
  scenes dylib that outlives every harness — so from the second shot on a scene painted with textures and framebuffers
  belonging to a context that was gone, and the image came back empty. The context is opened once now and lent to every
  shot, including the sheet. Scenes need no change, and could not have worked around it: a fresh context reissues GL
  names from 1 and a freed handle's address is commonly reused, so neither a name nor a pointer tells a scene its
  context has been replaced.
- **Breaking: a scene takes its `Ui` as an argument** — `fn(&mut SceneCtx)` becomes `fn(&mut SceneCtx, &mut Ui)`,
  `ctx.ui` is gone, and the five methods that draw take the `Ui` to draw into: `stage`, `offscreen`, `offscreen_input`,
  `offscreen_stage`, `offscreen_input_stage`. `stage!` gains a second argument, `stage!(ctx, ui, ..)`. Migrating a scene
  is three mechanical edits: add `ui: &mut Ui` to the signature, drop the `ctx.` from every `ctx.ui.…`, and pass `ui` to
  the calls above. **The knob API is untouched** — no accessor or `set_*` ever drew anything, so
  `ctx.slider("count", ..)` and friends are unchanged, as is `gallery::action`.
- **Stages go wherever widgets go** — which is the point of the above. A stage sits in `egui::Grid`, `ui.columns(..)`, a
  `Frame`, a `CollapsingHeader` or a `ScrollArea`, and anything egui adds later works without gallery knowing about it.
  One limit worth stating: a stage does **not** wrap in `ui.horizontal_wrapped(..)`. egui breaks a row on a size the
  widget declares before it draws, and a stage's size isn't known until its content has drawn, so a row of them runs off
  the pane rather than wrapping. Use `matrix` below.
- **`SceneCtx::matrix`** — one stage per size, in as many columns as the pane holds, aligned in a grid; the column count
  is measured against the widest of them, so resizing the window reflows the set (CSS's `repeat(auto-fit, ..)`). A grid
  rather than a wrapping row because the point is comparison — columns line up, where a row of differing sizes leaves
  every cell at its own offset. Nothing is packed: the order given is the order shown, a size matrix reading smallest to
  largest. The scaffold gains `matrix.scene.rs`.
- **Fixed** — a stage put the badge row and its content into the parent layout as two separate items, so in any
  horizontal layout the collapse arrow and the size caption landed beside the component instead of above it. A stage is
  one block now, whatever it is placed in. Invisible before this release, since the canvas only ever stacked them.
- **`Ui` joins the prelude**, since every scene signature names it.
- **Knob writeback** — `SceneCtx::set_slider`, `set_toggle`, `set_text`, `set_color`, `set_select`, `set_select_index`
  and `set_pad2d` write a knob back from the scene, for rendered content that does its own hit-testing. Each takes the
  first knob of its kind carrying exactly that label and returns whether one matched — an unknown label writes nothing
  and creates nothing. Values are clamped as the panel would clamp them and snapped to a slider's `step`, where a
  recipe's out-of-range value is still an error: a write overshoots by nature, a recipe states its values on purpose. A
  write lands where a panel edit would, so anything that resets a knob — the scene dropping it, or changing its label or
  kind at that position — drops the write too.
- **Capture precedence** — a recipe's overrides are re-applied before every frame and once more after the last, so a
  scene writing its own knobs cannot outlast them: the image and the store both end on the recipe, and `--list-knobs`
  and `--init-capture` therefore report what was captured. Two consequences for recipes that predate this. A key is now
  resolved afresh each frame, so a regex that only turns ambiguous once another override reveals a second matching knob
  stops the run rather than slipping through on the frame it happened to be unique. A key whose knob applied and then
  disappeared still counts as honoured, and does not fail the run.
- **Offscreen input** — `SceneCtx::offscreen_input` shows an offscreen image as `offscreen` does and reports the pointer
  that landed on it as `Pointer::{Down, Move, Up, Wheel}`, in that image's own pixels: a press captures until its
  release, so a drag off the edge keeps reporting and never leaves the content held; coordinates come off the whole
  image, so scrolling part of it out of sight doesn't shift them; and the image takes the drag and the wheel, so neither
  reaches the canvas behind it. Whatever egui reports as a pointer arrives, so a touchscreen drives it too.
- **Actions** — `gallery::action(..)` reports a line into a panel of its own, timestamped and scoped to the scene, with
  a toggle beside Perf and a Clear button. A free function, so it reaches into a callback handed to a component: the
  component keeps its own event type and never names gallery. Heard only while a scene renders, so a headless capture
  and a spawned thread both drop it.
- **Prelude** — `action` and `Pointer` join it, and the scaffold's `knobs.scene.rs` gains a `writeback` scene exercising
  all seven setters and reporting each write as an action.
- **Staged offscreen** — `SceneCtx::offscreen_stage` and `offscreen_input_stage` put a rendered frame on a stage, so GL
  content gets the checkerboard, size caption and collapse toggle egui content has always had; a scene mixing prose,
  widgets and rendered frames now reads as one document. A folded stage runs no GL at all and returns `None`, and keeps
  its target for when it opens again. `stage` itself is unchanged — the image is drawn to its texture first, leaving
  only an image for the stage to hold.
- **Fixed** — every `offscreen` call in a scene shared one framebuffer, keyed by scene alone, so each image showed
  whatever the last call drew, stretched to the size its own call had asked for — and two sizes reallocated the target
  twice a frame. Each call site now keeps its own, told apart by the order the scene makes them, as knobs are. A scene
  that stops making a call keeps that target for its return rather than freeing a `TextureId` eframe cannot release.
- **Fixed** — a capture asked for less room than its scene lays out is drawn twice, and the second pass used to run on a
  rebuilt GL context. A scene that had cached anything against the first — a femtovg canvas, a compiled shader — drew
  into nothing, so the shot came back with the egui parts and none of the GL. The canvas is resized in place now, and
  the context lives as long as the shot.

## 2026-07-31

- **Trimmed captures** — a shot's size is the canvas its scene lays out in, and the PNG is now cropped to what the scene
  actually drew, rather than padded out to that size with background. A recipe's `trim = false`, or `--no-trim`, keeps
  the whole canvas. This narrows what "Headless render" below says about the size being where the scene lays out: that
  is still what governs the layout, but no longer what the file ends up measuring.

## 2026-07-30

- **Fixed** — the canvas scrollbar sat at the right edge of the content rather than of the pane, leaving dead space
  beside it whenever a scene was narrower than the pane and taller than it.

## 2026-07-20

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
