//! What the checkerboard margin around a stage costs, and when to turn it off.
//!
//! Drag `padding` to nothing and watch the right-hand pair. The rectangle barely changes,
//! its own corners having covered the margin all along. The round face changes completely —
//! the margin was the only part of the stage still showing, so it read as a bezel.

use gallery::prelude::*;

scene_meta! { title: "Layout / Stage padding" }

/// Big enough that the default margin is a visible ring, small enough that two sit side by side.
const SIDE: f32 = 150.0;

#[scene(default)]
fn round_and_square(ctx: &mut SceneCtx, ui: &mut Ui) {
    let padding = ctx.slider("padding", 0.0, 0.0, 32.0, 1.0);
    let tint = ctx.color("face", egui::Color32::from_rgb(0x6C, 0x9C, 0xD8));
    #[expect(clippy::cast_possible_truncation, reason = "the slider stops at 32")]
    let padding = padding as i8;

    let face = |ui: &mut Ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(SIDE, SIDE), egui::Sense::hover());
        ui.painter().circle_filled(rect.center(), SIDE / 2.0, tint);
    };
    let panel = |ui: &mut Ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(SIDE, SIDE), egui::Sense::hover());
        ui.painter().rect_filled(rect, 4.0, tint);
    };

    ui.heading("A round face");
    ui.label(
        "The margin is the whole of what shows around a circle, so it reads as a smaller device \
         inside a larger bezel. The badge reports the same size either way — the stage is the \
         face, not the face plus a frame.",
    );
    ui.horizontal(|ui| {
        ctx.stage(ui, Stage::Fixed(egui::Vec2::splat(SIDE)), face);
        ctx.stage(
            ui,
            Stage::Fixed(egui::Vec2::splat(SIDE)).padding(padding),
            face,
        );
    });

    ui.add_space(12.0);
    ui.heading("A rectangle, for comparison");
    ui.label("Same two paddings. Its corners reach the stage edge, so there is little to see.");
    ui.horizontal(|ui| {
        ctx.stage(ui, Stage::Fixed(egui::Vec2::splat(SIDE)), panel);
        ctx.stage(
            ui,
            Stage::Fixed(egui::Vec2::splat(SIDE)).padding(padding),
            panel,
        );
    });

    ui.add_space(12.0);
    ui.weak(format!(
        "left: the default {PADDING} · right: padding({padding})"
    ));
}
