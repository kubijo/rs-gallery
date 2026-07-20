# gallery

An **egui-shelled component catalog** for Rust with **Storybook-style scene discovery** — browse your UI components in
isolation, one state at a time. Scenes live next to the components they exercise and are discovered from a config; the
shell finds them, with no central list.

> **Status: early**, pre-release, not on crates.io. The shape (`#[scene]` / `scene_meta!` / discovery / `SceneSource`)
> is settled; the shell is deliberately minimal. See [Status & roadmap](#status--roadmap).

## How it looks to a consumer

An instance is one flat crate plus a config — nothing else. Scaffold one into a sub-directory of your choosing with

```
cargo generate --git kubijo/rs-gallery template --name my-gallery --no-workspace
```

`--name` picks the directory; `--no-workspace` keeps cargo-generate from splicing the instance into an enclosing
workspace's members, so it's safe to run inside an existing Cargo workspace — the instance stays a standalone crate (it
carries its own `[workspace]`). The command prompts for the gallery git URL, scene glob, and title; or copy
[`template/`](template) and fill the `{{ … }}` markers by hand. It ships a runnable `example.scene.rs` and a standalone
`justfile`, so the first `just run` already shows something. The files it lays down are the whole contract, so they stay
the source of truth as the shape evolves.

`just run` opens the window; `just hot` rebuilds and hot-swaps scenes as you edit them. (Both wrap `cargo run`, so plain
`cargo run` / `cargo run -- --hot` work too.)

> The instance package must not be named `gallery` — its scenes dylib would clash with the framework crate at link time.
> (The binary and directory still can.) The scaffold names it `app-gallery`; that one field is a plain literal, not a
> generated placeholder, so rename it by hand — a git dependency's tree is parsed by every consumer, and an invalid
> package name would error there.

## Authoring scenes

Scene files sit next to the components they show. Each file declares a tree title with `scene_meta!`; its scenes are
`#[scene]` functions:

```rust
// src/button.scene.rs
use gallery::prelude::*;

scene_meta! { title: "Components / Button" }

#[scene("enabled")]
fn enabled(ctx: &mut SceneCtx) {
    ctx.ui.button("Save");
}

#[scene("disabled")]
fn disabled(ctx: &mut SceneCtx) {
    ctx.ui.add_enabled(false, egui::Button::new("Save"));
}
```

A scene takes a \[`SceneCtx`\]: `ctx.ui` is the egui `Ui` to draw into, and `ctx.slider(...)`, `ctx.toggle(...)`,
`ctx.text(...)`, `ctx.color(...)`, `ctx.select(...)` declare **controls** (knobs). Calling one both registers the
control in the right-hand panel and returns its current value, so tweaking it re-renders the scene:

```rust
#[scene("label")]
fn label(ctx: &mut SceneCtx) {
    let text = ctx.text("text", "Save");
    let wide = ctx.toggle("wide", false);
    let mut button = egui::Button::new(text);
    if wide {
        button = button.min_size(egui::vec2(120.0, 0.0));
    }
    ctx.ui.add(button);
}
```

The title's slashes build the sidebar tree; the scenes are children:

```
Components
  Button
    enabled
    disabled
```

A file with a single scene can mark it `#[scene(default)]` (or bare `#[scene]`); its group then shows as one flat entry
instead of a group with a lone child.

Within a group, scenes sort by `(order, name)`. Pin one with `#[scene("name", order = N)]` (lower first); scenes with no
`order` fall to the end, alphabetically. Folders stay in title order.

## How it works

- **`#[scene("name")]`** registers a `fn(&mut SceneCtx)` via [`inventory`], keyed by its `module_path`. With no argument
  the name defaults to the title-cased function name; `default` marks the group's default scene; `order = N` sets its
  sort position within the group (unset sorts last, by name).
- **Controls (knobs)** are declarative-by-use: `ctx.slider(...)` etc. register a control and return its value. Values
  persist per scene in the host, so they survive hot-reloads.
- **`scene_meta! { title: "A / B" }`** (once per file) sets the group's place in the tree. Scenes join their group by
  `module_path` (longest prefix); the title's slashes nest.
- **Discovery** — `build.rs` calls `gallery_build::discover_from_env()`, which globs `scene_globs` (handed down by the
  launcher) for `*.scene.rs`, compiles each in, and lets its `#[scene]`s self-register.
- **`gallery::launch!(setup, settings)`** reads `gallery.toml`, builds the scenes into the crate's dylib, loads it, and
  runs the shell. `settings` is a `Settings` whose required `Renderer` (`Wgpu` or `Glow`) picks the eframe backend;
  under `Glow` a scene can draw non-egui content — femtovg, raw OpenGL — into an offscreen framebuffer with
  `ctx.offscreen(...)`, or reach the raw `ctx.gl_loader()` / `ctx.register_native_texture(...)` beneath it. `--hot`
  rebuilds `--lib` on change and swaps it in live; the loader finds the dylib next to the running binary, so it tracks
  any `CARGO_TARGET_DIR`.

The scenes compile into a `dylib` target so hot mode can swap them without restarting the host — but it's still one
crate: `--hot` rebuilds only the library, and the host loads its own crate's `.so`.

## Crates

- **`gallery`** — the framework: the egui shell, the `#[scene]`/`scene_meta!` re-exports, `SceneSource` (`Linked`
  compiled-in, `HotDylib` reloaded), and the `launch!` / `scenes_dylib!` macros.
- **`gallery-macros`** — the `#[scene]` proc-macro (its own crate, as proc-macros must be).
- **`gallery-build`** — the `build.rs` discovery helper, kept light (`glob` + `camino`) so it's a cheap
  build-dependency.

## Status & roadmap

Working: discovery, the title tree, hot-reload, controls (slider/toggle/text/color/select/pad2d/group), the sidebar
fuzzy filter + keyboard navigation, per-scene source view, selection preserved across reloads, SVG icons, a required
`Renderer` choice (`wgpu` or `glow`), and offscreen non-egui rendering under glow (`ctx.offscreen`, exercised by the
femtovg demo).

What's left is genuinely new design, not more porting:

- a **wgpu-native** offscreen path — non-egui rendering already works under the glow backend, where `ctx.offscreen`
  draws femtovg or raw OpenGL into an FBO, but the wgpu backend has no equivalent yet;
- a preview→control interaction layer (drive a knob by clicking/dragging the component) — immediate-mode scenes handle
  interactions inline today, so this would be a fresh design rather than a copy;
- publishing to crates.io.

## License

Code: [Unlicense](UNLICENSE) — public domain, no rights reserved.

Bundled fonts: the shell ships four Noto fallback faces (`fonts/noto/` — Sans, Symbols, Symbols 2, Math) so arrows,
math, and symbol glyphs render instead of tofu. They're [SIL OFL 1.1](fonts/noto/OFL.txt), which permits embedding and
redistribution. CJK and emoji aren't bundled (each is multi-megabyte); a catalog that needs them adds its own font in
the host `setup` closure.

[`inventory`]: https://docs.rs/inventory
