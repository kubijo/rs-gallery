//! Renderer-independence proof, injected into the femtovg demo by `scripts/demo.sh` (not part of the
//! gallery crate — it's copied into a scaffolded instance that depends on `femtovg`).
//!
//! Under `Renderer::Glow`, `ctx.offscreen(size, draw)` lends the scene a bound framebuffer and a GL
//! proc-address loader. The scene builds a femtovg `Canvas` from that loader — at femtovg's own glow
//! version, which gallery never pins. That version is deliberately mismatched: femtovg 0.20.4 links
//! glow 0.16 while eframe links glow 0.17, two incompatible crates in one binary, and only the raw C
//! loader crosses between them. The scene draws vector shapes that react to the knobs, and gallery
//! shows the result inline as an egui texture. Under `Renderer::Wgpu` the loader is absent, so
//! `offscreen` renders a hint instead. The pure-egui scenes in this same demo render unchanged either
//! way — that, plus this scene surviving the glow mismatch, is what "renderer-independent" means.

use std::cell::RefCell;
use std::ffi::CStr;

use femtovg::renderer::OpenGl;
use femtovg::{Canvas, Color, Paint, Path};
use gallery::prelude::*;

scene_meta! { title: "Renderer / femtovg" }

thread_local! {
    // Building a `Canvas` recompiles femtovg's shaders, so keep one for the scene's lifetime and reuse
    // it every frame. The scene owns its renderer; gallery only lends the bound FBO and the GL loader.
    static CANVAS: RefCell<Option<Canvas<OpenGl>>> = const { RefCell::new(None) };
}

#[scene("vector shapes")]
fn vector_shapes(ctx: &mut SceneCtx) {
    let accent = ctx.color("accent", egui::Color32::from_rgb(0x6C, 0x9C, 0xD8));
    let radius = ctx.slider("corner radius", 28.0, 0.0, 80.0, 1.0);
    let stroke = ctx.slider("stroke width", 3.0, 0.5, 12.0, 0.5);
    let spokes = ctx.slider("spokes", 16.0, 3.0, 64.0, 1.0) as u32;
    let (cx, cy) = ctx.pad2d(
        "center",
        Pad2DSpec {
            default_x: 0.5,
            default_y: 0.5,
            min_x: 0.0,
            max_x: 1.0,
            min_y: 0.0,
            max_y: 1.0,
            invert_y: false,
        },
    );

    ctx.offscreen([520u32, 340], |o| {
        let loader = o.gl_loader();
        let fbo = o.fbo();
        let [w, h] = o.size();
        let (wf, hf) = (w as f32, h as f32);
        let accent = Color::rgba(accent.r(), accent.g(), accent.b(), accent.a());

        CANVAS.with_borrow_mut(|slot| {
            let canvas = slot.get_or_insert_with(|| {
                // SAFETY: `loader` resolves GL symbols for the context gallery bound; gallery's FBO
                // carries the depth/stencil attachment that femtovg's fills need.
                let mut renderer = unsafe { OpenGl::new_from_function_cstr(|s: &CStr| loader(s)) }
                    .expect("build femtovg OpenGl renderer");
                // femtovg re-targets "screen" on every `set_size`, binding the default framebuffer on
                // flush unless told otherwise. Point it at gallery's FBO — stable across resizes — so it
                // draws into the offscreen texture, not the window. `glow::NativeFramebuffer` is
                // femtovg's own glow (0.16); the FBO name is just a GL integer, valid in any context.
                renderer.set_screen_target(Some(glow::NativeFramebuffer(fbo)));
                Canvas::new(renderer).expect("build femtovg canvas")
            });
            canvas.set_size(w, h, 1.0);

            let margin = 16.0;
            let mut panel = Path::new();
            panel.rounded_rect(margin, margin, wf - 2.0 * margin, hf - 2.0 * margin, radius);
            canvas.fill_path(&panel, &Paint::color(Color::rgbf(0.12, 0.12, 0.14)));

            // A starburst of rays from the pad2d-controlled centre.
            let (px, py) = (cx * wf, cy * hf);
            let reach = wf.min(hf) * 0.42;
            let mut rays = Path::new();
            for i in 0..spokes {
                let angle = std::f32::consts::TAU * i as f32 / spokes as f32;
                rays.move_to(px, py);
                rays.line_to(px + reach * angle.cos(), py + reach * angle.sin());
            }
            let mut ray_paint = Paint::color(accent);
            ray_paint.set_line_width(stroke);
            canvas.stroke_path(&rays, &ray_paint);

            let mut hub = Path::new();
            hub.circle(px, py, stroke * 2.0 + 4.0);
            canvas.fill_path(&hub, &Paint::color(accent));

            canvas.flush();
        });
    });

    ctx.ui.add_space(6.0);
    ctx.ui.weak("femtovg → offscreen FBO → egui texture");
}
