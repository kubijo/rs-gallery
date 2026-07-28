# Glow-backed headless capture + TOML formatting

The plan currently being implemented. Deferred work and "next we'll…" notes belong here rather than in source comments.
Landed items graduate to `CHANGELOG.md`; anything parked indefinitely moves to `TODO.md`.

## Context

Headless capture (`--render` / `--capture`) currently ignores the consumer's configured `Renderer` and always paints
through egui_kittest's `WgpuTestRenderer`. A `Renderer::Glow` instance therefore captures the "needs the glow renderer"
hint instead of its `ctx.offscreen(...)` content, and a scene calling `ctx.register_native_texture(...)` panics
mid-render (`Frame::_new_kittest` leaves `glow_register_native_texture: None` and `register_native_glow_texture` unwraps
it).

Separately: the repo has **no TOML formatting at all** — no taplo, no `*.toml` include in `nix/formatter.nix`, and
`on-unmatched = "debug"` silently skips all 11 tracked `.toml` files. Markdown fences format rust + bash only.

## Research verdicts (all source-verified)

- **Upstream is a dead end**: no kittest glow renderer exists in any release or on master; PR #5539 created the
  `TestRenderer` trait precisely so one could exist. `Frame.gl` / `glow_register_native_texture` are `pub(crate)`;
  **`cc.gl` / `cc.get_proc_address` are `pub`** and are the fields gallery actually reads (`install_context`,
  `Gallery::new`).
- **The probe's "no config" was a template bug, not a platform limit**: `ConfigTemplateBuilder` defaults to
  `ConfigSurfaceTypes::WINDOW` (glutin config.rs:328-364); device-platform displays have no window configs, and EGL
  returns "matched nothing" as success. glutin's own `glutin_examples/examples/egl_device.rs` is the exact recipe:
  `Device::query_devices()` → `Display::with_device` → template with `.with_surface_type(ConfigSurfaceTypes::empty())` →
  `create_context` (default, GLES fallback) → `make_current_surfaceless()` → FBO → read pixels. glutin's EGL backend
  never checks `EGL_KHR_surfaceless_context` — failure surfaces as `BadMatch` from `eglMakeCurrent`, so try surfaceless,
  fall back to a 1×1 pbuffer (`create_pbuffer_surface`; the config template then needs `ConfigSurfaceTypes::PBUFFER`).
- **`eframe::Frame` coupling is exactly 4 sites**, all `register_native_glow_texture`: `src/offscreen.rs:33` (param),
  `:83` (the call), `:189` (pass-through), `src/context.rs:260-266`. `egui_glow::Painter::register_native_texture`
  (painter.rs:645) is the public replacement.
- **Everything needed ships via eframe re-exports** (`eframe-0.35.0/src/lib.rs:158-159` `pub use {egui_glow, glow};`):
  `Painter::new(gl, "", None, false)`, `paint_primitives`, `read_screen_rgba` (reads the bound framebuffer, flips V,
  returns `ColorImage`). Only **glutin** becomes a new direct dependency (already in the lockfile at 0.32.3 via eframe).
- **glow-version trap**: gallery's dev-dependency `glow = "0.16"` means bare `glow::` in test code resolves to 0.16
  while `egui_glow` needs 0.17. All library code already imports `eframe::glow`; every new line must too. The
  `femtovg_demo_pins_a_glow_version_distinct_from_eframe` test must stay green.
- **Kittest mechanics to mirror** (`egui_kittest-0.35.0`): `HarnessBuilder::renderer(...)` must be chained **before**
  `build_eframe` (setup_eframe runs before the app closure, builder.rs:203-218); `handle_delta` is called every step and
  processes only `delta.set` (wgpu.rs:140-150 — keep parity, never free mid-run); `render` sizes as
  `ctx.content_rect().size() * ctx.pixels_per_point()` and tessellates with `ctx.tessellate(output.shapes.clone(), ppp)`
  (wgpu.rs:152-240); `render` can be called more than once per capture (`handle_screenshots`), so it must be idempotent
  — no `Painter::destroy()` in it.
- **eframe context attributes to mirror** (glow_integration.rs:1060-1089): default attributes (→ GL 3.3 Core on EGL)
  with a `ContextApi::Gles(None)` fallback, `build(None)`. Config template: `.with_api(Api::OPENGL | Api::GLES2)` so
  desktop GL is reachable, depth/stencil 0.
- **`GlDeps::render` restores the framebuffer to `None`** (offscreen.rs:207-209). Harmless under kittest (egui paints
  later, after we bind the capture FBO), but harden to save/restore the previous binding — upstream documents exactly
  this contract (painter.rs:290-292).
- **TOML tooling**: standalone = `pkgs.taplo` (0.10.0, in nixpkgs by-name; `taplo format`, and treefmt-nix's own module
  uses `args = ["format"]`). Fences: `mdformat-config` exists on PyPI but is not in nixpkgs; instead clone the repo's
  own local-plugin pattern (`mdformat-rustfmt-local` in nix/formatter.nix:17-68) shelling out to the same nixpkgs taplo
  — one taplo version for files and fences.

## Design — Part A: glow capture

### 1. Decouple `GlDeps` from `eframe::Frame`

`src/offscreen.rs`: replace `frame: &'a mut eframe::Frame` with
`register: &'a mut dyn FnMut(glow::Texture) -> egui::TextureId`. `RenderTarget::create` takes the closure;
`src/context.rs:register_native_texture` calls it. Windowed path (`src/lib.rs` `Gallery::ui`):
`let mut register = |t| frame.register_native_glow_texture(t);`. Harden `GlDeps::render`: read
`glow::DRAW_FRAMEBUFFER_BINDING` before binding the scene FBO and restore that (the viewport is reset by
`Painter::prepare_painting`).

### 2. New module `src/glow_capture.rs`

- **Context creation** (Diagnostic on failure, listing the devices tried): iterate `Device::query_devices()` (via
  `glutin::api::egl`), `Display::with_device(dev, None)`, template `.with_surface_type(ConfigSurfaceTypes::empty())`
  `.with_api(Api::OPENGL | Api::GLES2)` `.with_depth_size(0)` `.with_stencil_size(0)`, pick a config, eframe's two-step
  context creation, `make_current_surfaceless()` else a 1×1 pbuffer (template retried with `PBUFFER` if needed). Same
  thread throughout; the context stays current for the whole shot.
- **`GlowTestRenderer: TestRenderer`** owning the glutin display/context/surface, an `Rc<RefCell<egui_glow::Painter>>`,
  a capture FBO (RGBA8 colour texture, no depth), and the loader (an `Arc` closure over `display.get_proc_address` —
  `GlLoader`-shaped, `&CStr` signature confirmed to match).
  - `setup_eframe`: `cc.gl = Some(gl.clone()); cc.get_proc_address = Some(loader.clone());`
  - `handle_delta`: `painter.set_texture(id, delta)` for each `set` (never free).
  - `render`: size/tessellate exactly as wgpu.rs; create/resize the FBO; bind; clear transparent;
    `paint_primitives(size_px, ppp, &tessellated)`; `gl.finish()`; `read_screen_rgba(size_px)` →
    `image::RgbaImage::from_raw` (ColorImage→RgbaImage is ~3 lines; kittest's converter goes the other way and is
    private).
  - `Drop`: `painter.destroy()` before the context is torn down.
- All glow/egui_glow types via `eframe::{glow, egui_glow}` — never bare `glow::`.

### 3. Thread the renderer choice

- `launcher.rs`: `render::render(&manifest, settings.renderer, &setup, &shots)` — `settings` is live at the call site,
  and the early return precedes the `run_with` move.
- `render.rs` `shoot()`: on `Renderer::Glow`, construct `GlowTestRenderer` (Diagnostic on failure), clone its painter
  `Rc` into the build closure, chain `.renderer(...)` before `.build_eframe(...)`. `Canvas` gains
  `gl: Option<Arc<glow::Context>>` + `loader: Option<GlLoader>` (stashed from `cc` in the closure),
  `target: Option<RenderTarget>`, `painter: Option<Rc<RefCell<Painter>>>`; its `ui` builds `GlDeps` with a registration
  closure over the painter, mirroring `Gallery::ui`. The wgpu path is unchanged (kittest default).

### 4. Docs

README (drop the "Capture is wgpu-only" paragraph → capture follows the configured renderer; note the EGL requirement
and the failure mode), CHANGELOG (Headless render bullet + a new line for glow), TODO Tier 1 (glow landed; the CI note
becomes: lavapipe for wgpu, `EGL_MESA_device_software` for glow — both unverified on GitHub runners, which ship no mesa
at all).

## Design — Part B: TOML formatting

- `nix/formatter.nix`: add
  `toml = { command = lib.getExe pkgs.taplo; options = [ "format" ]; includes = [ "*.toml" ]; };`
- Add a local mdformat fence plugin `mdformat-taplo-local` cloned from the `mdformat-rustfmt-local` pattern (pyproject +
  module shelling to the pinned `taplo format -` over stdin — verify the stdin flag during implementation; taplo
  supports `-` with `--stdin-filepath`), registered for `toml`, with `--codeformatters toml` appended (the explicit list
  makes a non-loading plugin an error rather than a silent skip).
- Run `just format`; **review the one-time diff** across the 11 TOMLs before accepting.
- Fix the stale `format` recipe doc-comment while there (it lists 5 of 8 formatters).

## Files

| File                      | Change                                                                        |
| ------------------------- | ----------------------------------------------------------------------------- |
| `src/glow_capture.rs`     | **new** — EGL device/surfaceless context, `GlowTestRenderer`                  |
| `src/offscreen.rs`        | `frame` → registration closure; save/restore the FBO binding                  |
| `src/context.rs`          | `register_native_texture` through the closure                                 |
| `src/lib.rs`              | `Gallery::ui` builds the closure over `frame`; `mod glow_capture`             |
| `src/render.rs`           | `shoot()` renderer branch; `Canvas` gl fields                                 |
| `src/launcher.rs`         | pass `settings.renderer`                                                      |
| `Cargo.toml`              | `glutin = { version = "0.32", default-features = false, features = ["egl"] }` |
| `nix/formatter.nix`       | taplo block + toml fence plugin                                               |
| README / CHANGELOG / TODO | per above                                                                     |

## Verification

1. `just demo-femtovg --scene offscreen --render /tmp/glow.png` — **open it**: femtovg content, not the yellow hint.
   This is the acceptance test.
2. The same pure-egui scene captured under demo-wgpu and demo-femtovg — open both; near-identical (AA/gamma differences
   acceptable; if colours are visibly off under glow, switch the capture FBO to `SRGB8_ALPHA8` + `FRAMEBUFFER_SRGB` and
   re-check).
3. A capture recipe with knob overrides under glow — proves the shared pipeline still applies knobs.
4. Failure path: force no device (e.g. a bogus `__EGL_VENDOR_LIBRARY_FILENAMES`) → Diagnostic, exit 1.
5. `femtovg_demo_pins_a_glow_version_distinct_from_eframe` still green (glow 0.16/0.17 both locked).
6. TOML: `just format`, review the diff, then check a toml fence in README round-trips.
7. `just validate` bare; one grep of the persisted log. Comment-discipline pass including the dangling-word grep;
   adversarial self-review.

## Risks / limits

- Surfaceless support is driver-decided at `eglMakeCurrent`; pbuffer is the fallback, and both failing is a clean
  Diagnostic ("capture on wgpu, or install mesa EGL").
- No render test in the suite (unchanged policy) — GitHub runners ship no mesa at all.
- `Painter` requires GL ≥ 2.0; the GL 3.3 Core default matches eframe, and VAOs are handled by egui_glow.
