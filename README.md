# gallery

An **egui-shelled component catalog** for Rust with **Storybook-style scene discovery** — browse your UI components in
isolation, one state at a time. Scenes live next to the components they exercise and are discovered from a config; the
shell finds them, with no central list.

> **Status: early**, pre-release, not on crates.io. The shape (`#[scene]` / `scene_meta!` / discovery / `SceneSource`)
> is settled; the shell is deliberately minimal. See [Status & roadmap](#status--roadmap).

## How it looks to a consumer

An instance is one flat crate plus a config — nothing else. Scaffold one into a sub-directory of your choosing with

```bash
cargo generate --git kubijo/rs-gallery template --name my-gallery --no-workspace
```

`--name` picks the directory; `--no-workspace` stops cargo-generate splicing the instance into an enclosing workspace,
so it is safe to run inside one — the instance carries its own `[workspace]`. It prompts for the gallery git URL, scene
glob and title, or copy [`template/`](template) and fill the `{{ … }}` markers by hand. It ships a runnable
`example.scene.rs` and a standalone `justfile`, so the first `just run` already shows something.

`just run` opens the window; `just hot` rebuilds and hot-swaps scenes as you edit them. (Both wrap `cargo run`, so plain
`cargo run` / `cargo run -- --hot` work too.) Without a window at all, `just render` and `just capture` write scenes to
PNGs — see [Rendering scenes to images](#rendering-scenes-to-images).

> The instance package must not be named `gallery` — its scenes dylib would clash with the framework crate at link time
> (the binary and directory still can). The scaffold names it `app-gallery`; that field is a plain literal rather than a
> placeholder, so rename it by hand.

## Authoring scenes

Scene files sit next to the components they show. Each file declares a tree title with `scene_meta!`; its scenes are
`#[scene]` functions:

```rust
// src/button.scene.rs
use gallery::prelude::*;

scene_meta! { title: "Components / Button" }

#[scene("enabled")]
fn enabled(ctx: &mut SceneCtx, ui: &mut Ui) {
    stage!(ctx, ui, |ui| {
        ui.button("Save");
    });
}

#[scene("disabled")]
fn disabled(ctx: &mut SceneCtx, ui: &mut Ui) {
    stage!(ctx, ui, |ui| {
        ui.add_enabled(false, egui::Button::new("Save"));
    });
}
```

The canvas is plain, so a scene reads as a **document**: headings and prose go straight onto `ui`, and each thing you
are demonstrating goes in a **stage** — on the checkerboard, so transparency and bounds read against the shell,
captioned with its size and collapsible. A pinned size is for a component that behaves differently depending on how much
room it has — a wrapping layout, anything with a breakpoint:

| `stage!`'s size argument | the stage is sized              |
| ------------------------ | ------------------------------- |
| omitted, or `fit`        | to fit its content              |
| `(300.0, 200.0)`         | to a pinned 300×200 viewport    |
| `(300, 200)`             | the same — integers convert too |
| `200`                    | to a 200×200 square             |
| `fill`                   | to whatever canvas is left      |
| `scroll`                 | to the canvas, and it scrolls   |

`fill` measures what is left where it is called, so a single `fill` gets the whole canvas while one placed after other
content takes only the remainder.

A stage whose content runs past it scrolls rather than growing — `scroll` is `fill` that scrolls, and any other size
takes `.scrollable()`, as in `Stage::Fixed(egui::vec2(300.0, 200.0)).scrollable()`. The box stays the size it declared
and the content scrolls inside it; [`example.scene.rs`](template/example.scene.rs) shows them running.

A scene takes two things: the `Ui` to draw into, and a `SceneCtx`. The `Ui` comes separately rather than on the context
so that stages go wherever widgets go — inside an `egui::Grid`, `ui.columns(..)`, a `Frame` or a `CollapsingHeader`.
(One exception: a stage does not wrap in `ui.horizontal_wrapped(..)`, since egui breaks a row on a size the widget
declares up front and a stage's isn't known until it has drawn.)

For the common case of one component at several sizes, `ctx.matrix(ui, &sizes, |ui, at| ..)` puts each on its own stage
in as many columns as the pane holds, aligned in a grid and reflowing as the window resizes —
[`matrix.scene.rs`](template/matrix.scene.rs).

On the context, `ctx.slider(...)`, `ctx.toggle(...)`, `ctx.text(...)`, `ctx.color(...)`, `ctx.select(...)` declare
**controls** (knobs). Calling one registers the control in the right-hand panel *and* returns its current value, so
tweaking it re-renders the scene — [`knobs.scene.rs`](template/knobs.scene.rs) exercises every kind. The
`ctx.set_slider(...)` family writes a value back by label, so content that does its own hit-testing — a slider drawn
inside the preview, a rendered button — drives the panel too.

`gallery::action(...)` reports something worth seeing into the **Actions** panel — a free function, not a method, so a
scene can call it from inside a callback it hands a component: `picker(ui, |row| action(format!("picked {row}")))`. The
component keeps its own event type; the scene's closure decides what's worth a line.

Under the glow renderer, `ctx.offscreen(...)` renders non-egui content (femtovg, raw glow) into a framebuffer gallery
owns and shows inline. `ctx.offscreen_input(...)` shows it the same way and hands back the pointer input that landed on
it — press, move, release and wheel, in the image's own pixels — so content with its own hit-testing is as live in the
gallery as it is on the device.

Under the wgpu renderer, `ctx.render_state()` hands the scene egui's own `RenderState` — the device, the queue, and the
target format a render pipeline has to be built against. A scene builds its pipeline on first draw, caches it in the
renderer's `callback_resources`, and draws through an `egui_wgpu::Callback` inside egui's render pass. The state reaches
a headless capture as well as a window, so a shader-drawn scene renders the same pixels into a PNG as on screen. It is
`None` under glow, where a scene falls back to ordinary egui drawing.

That callback draws into egui's own render pass, which carries no depth attachment — so a solid drawn through one is
sorted by the order its triangles were submitted and nothing else. `ctx.render_pass(ui, size, |target| ...)` is the
other route: gallery owns a colour texture and a depth buffer (one per call site, resized in place, registered once),
begins a cleared pass on them, and shows the result inline. It is the wgpu counterpart to `ctx.offscreen` under glow,
and what anything three-dimensional wants. A pipeline drawing through it is built against `ScenePass::FORMAT`,
`ScenePass::SAMPLES` and `ScenePass::depth_state(..)` — all three, since wgpu matches a pipeline against everything the
pass carries, including a depth buffer it never means to test.

[`wgpu.scene.rs`](template/wgpu.scene.rs) is a group of scenes over both: a gradient backdrop taking its colours through
a uniform buffer, a shader measuring in device pixels (which a window and a capture do not agree on), a callback egui
scissors inside a scrolling stage, one colour drawn three ways to show the routes agree on it, a clock-driven one that
never settles, and a cube drawn twice side by side — through egui's pass, where its far faces land on the near ones, and
through a pass of its own, where it comes out a cube.

A renderer that cannot be pointed at a foreign framebuffer goes the other way round: draw into a texture of your own,
register it once, and hand it to `ctx.texture_stage(ui, stage, StageTexture::new(id, size))` for the stage chrome with
no copy. `.showing(..)` draws part of a loosely-allocated texture, which is how a content-sized stage settles its height
in the frame it measured it; `.interactive()` asks for the pointer.

The title's slashes build the sidebar tree; the scenes are children:

```text
Components
╰─ Button
   ├─ enabled
   ╰─ disabled
```

A file with a single scene can mark it `#[scene(default)]` (or bare `#[scene]`); its group then shows as one flat entry
instead of a group with a lone child.

Within a group, scenes sort by `(order, name)`. Pin one with `#[scene("name", order = N)]` (lower first); scenes with no
`order` fall to the end, alphabetically. Folders stay in title order.

## Rendering scenes to images

A scene renders to a PNG with no window, so anyone without a screen — over SSH, in CI, an agent iterating on a layout —
can look at a component instead of asking for a screenshot:

```bash
just render Button /tmp/button.png            # the canvas at 1280x720
just render Button /tmp/button.png 480x320    # ...or at a size you pick
```

The scene is a whole key (`module_path::name`) or a case-insensitive regex over the keys, and must match exactly one.
The image is the canvas alone — no sidebar, controls or header — so it stays put when unrelated chrome moves.

A size is where the scene lays itself out, not a crop of the result. A scene that comes out bigger than the size asked
for — a wide table, a tall list — is captured whole, at the size it turned out to need, rather than cut off at the edge
the way the window would scroll it.

A size is in points, the unit a window lays out in; `scale` says how many device pixels go to one of them, as a
display's scale factor does. `--scale 2` is what a HiDPI screen shows: the same layout at four times the pixels. It
matters for anything a scene measures in device pixels rather than points — a shader drawing a fixed-pixel feature holds
its size while the rest of the picture grows — which is the one thing a window and a capture would otherwise silently
disagree about.

That uses the scene's default knobs, rarely the state worth seeing. Other states go in a capture recipe;
`just capture-init <scene>` writes one with the knobs already filled in, and `just capture` renders every shot in it:

```toml
out = "renders" # relative to this file; `just capture <file> <dir>` overrides it
size = "640x360" # in points, for any shot that doesn't state its own
scale = 2 # optional; device pixels to the point, one for one unless set
sheet = "sheet.png" # optional; gathers the run onto one captioned image as well
settle = true # optional; shoot each scene once it stops animating, `frames` being the most to draw
report = "capture.json" # optional; what the run came to, for something other than a person to read

[[shot]]
name = "night" # the shot's identity, and its filename: renders/night.png
scene = "vehicle"
knobs = { night = true, "sunroof open" = true, "body style" = "SUV" }

[[shot]]
name = "spinning"
scene = "orbit"
frames = 40 # an animated scene draws a different frame each time; pick one
knobs = { dots = 96, accent = "#6C9CD8" }
```

A knob key is its label, or a regex over the labels — the exact label wins, so punctuation like `width (chars)` needs no
escaping. Choices take an option label, colours a hex string. `just knobs <scene>` prints them ready to paste:

```text
buttons "body style" = "sedan"  (sedan | suv | hatch)
slider  "speed" = 1  (0.1 ..= 2, step 0.1)
color   "accent" = #4caf50ff
```

A pattern or label matching none or several, or a value its kind won't take, stops the run: a clean render of the wrong
state is worse than none.

`settle` replaces guessing a frame count. Without it a shot draws `frames` and captures whatever is on screen — too few
and it catches a scene mid-animation, too many and every settled scene in the set pays for the slowest one. With it, a
scene is shot once it stops asking egui to redraw it, and `frames` is only the ceiling. A scene that animates forever
never settles: it is captured at the ceiling and marked **still moving** in the run's report, so an unattended loop
neither hangs nor quietly diffs one arbitrary frame against another.

`report` writes that same outcome as JSON — a record per shot with its name, path, size, `settled` and the frames it
drew, plus the sheet's path if one was gathered. A loop that renders a set and inspects the results reads that instead
of scraping the text meant for a person.

`sheet` gathers the run onto one image beside the shots, so a change across a whole set is one thing to look at rather
than a directory to click through. The shots still write their own PNGs, and each panel is captioned with the shot that
made it. Packing is tight but not at any cost — a tall column and a long strip cover much the same area, and scaled to a
screen either leaves every panel unreadable — so a sheet is scored on its area *and* on how near it lands to a screen's
proportions. One capture writes no sheet and says so, a sheet of one image being the image.

Capture follows the renderer the instance configures. Under `Renderer::Glow` it paints through an OpenGL context taken
off an EGL device — no window, no display server — so a scene drawing with `ctx.offscreen(...)` captures its real
content. That needs an EGL driver present; without one the run stops and says so, rather than quietly producing a
picture the window would never show. Under `Renderer::Wgpu` the capture carries the render state a window does, so a
scene painting through a wgpu callback is captured rather than left out of the PNG.

## How it works

`#[scene]` self-registers through [`inventory`]; `build.rs` globs the scene files and compiles them in; `launch!` reads
`gallery.toml`, builds them into the crate's dylib and runs the shell. The dylib is what `--hot` swaps without
restarting — still one crate.

Three crates: `gallery` (shell and framework), `gallery-macros` (the `#[scene]` proc-macro), `gallery-build` (the
`build.rs` helper).

## Status & roadmap

Discovery, the tree, hot-reload, knobs — reading and writing them from the scene — source view, SVG icons and headless
capture all work, on either renderer. Open: a wgpu offscreen texture path (glow has `ctx.offscreen`; wgpu draws inline
through `ctx.render_state`) and publishing to crates.io.

## License

Code: [Unlicense](UNLICENSE) — public domain, no rights reserved.

Bundled fonts: the shell ships four Noto fallback faces (`fonts/noto/` — Sans, Symbols, Symbols 2, Math) so arrows,
math, and symbol glyphs render instead of tofu. They're [SIL OFL 1.1](fonts/noto/OFL.txt), which permits embedding and
redistribution. CJK and emoji aren't bundled (each is multi-megabyte); a catalog that needs them adds its own font in
the host `setup` closure.

[`inventory`]: https://docs.rs/inventory
