//! A badge is a few dozen pixels in a canvas sized for a screen,
//! so most of its PNG is background — the dead space `trim` crops away.

use gallery::prelude::*;

scene_meta! { title: "Capture / Badge" }

#[scene(default)]
fn badge(ctx: &mut SceneCtx) {
    let label = ctx.text("label", "3 updates");
    let padding = ctx.slider("padding", 10.0, 2.0, 40.0, 1.0) as i8;
    let tint = ctx.color("tint", egui::Color32::from_rgb(0x6C, 0x9C, 0xD8));

    stage!(ctx, fit, |ui| {
        egui::Frame::new()
            .fill(tint)
            .corner_radius(u8::MAX)
            .inner_margin(egui::Margin::symmetric(padding * 2, padding))
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new(&label)
                        .color(egui::Color32::BLACK)
                        .strong(),
                );
            });
    });
}
