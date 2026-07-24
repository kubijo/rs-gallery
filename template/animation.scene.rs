//! An animated scene — something that drives the render loop.
//!
//! egui is reactive, so a still scene renders no frames and the performance window reports `idle`. A
//! live component keeps itself going by asking for the next frame, which is what `animate` does; the
//! `dots` knob then scales per-frame cost. Plain egui, so it runs the same under either `Renderer`.

use gallery::prelude::*;

scene_meta! { title: "Motion / Orbit" }

#[scene(default)]
fn orbit(ctx: &mut SceneCtx) {
    let animate = ctx.toggle("animate", true);
    let count = ctx.slider("dots", 24.0, 1.0, 400.0, 1.0) as usize;
    let speed = ctx.slider("speed", 1.0, 0.0, 4.0, 0.1);
    let radius = ctx.slider("radius", 90.0, 10.0, 200.0, 1.0);
    let dot = ctx.slider("dot size", 5.0, 1.0, 20.0, 0.5);
    let accent = ctx.color("accent", egui::Color32::from_rgb(0x4C, 0xAF, 0x50));

    // Ask for the next frame and the shell keeps rendering; off, it falls back to rest.
    if animate {
        ctx.ui.ctx().request_repaint();
    }

    let t = ctx.ui.input(|i| i.time) as f32 * speed;
    let span = radius * 2.0 + dot * 2.0 + 8.0;
    let (rect, _) = ctx
        .ui
        .allocate_exact_size(egui::vec2(span, span), egui::Sense::hover());
    let painter = ctx.ui.painter_at(rect);
    let center = rect.center();

    for i in 0..count {
        let phase = std::f32::consts::TAU * i as f32 / count as f32;
        let angle = phase + t;
        let pos = egui::pos2(
            center.x + radius * angle.cos(),
            center.y + radius * angle.sin(),
        );
        // Fade around the ring so the motion reads even at low speed.
        let fade = 0.35 + 0.65 * (0.5 + 0.5 * angle.sin());
        painter.circle_filled(pos, dot, accent.gamma_multiply(fade));
    }
}
