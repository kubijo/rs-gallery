//! A component narrower than the canvas, stacked until the column outgrows it
//! — the shape that puts the canvas's own scrollbar to work.
//!
//! The bar belongs to the pane, not to the column:
//! widening the window leaves it against the right edge while the stages stay narrow.
//! `width` and `notices` drive the column past either bound.

use gallery::prelude::*;

scene_meta! { title: "Layout / Notice column" }

const LEVELS: [(&str, egui::Color32); 3] = [
    ("info", egui::Color32::from_rgb(0x6C, 0x9C, 0xD8)),
    ("warning", egui::Color32::from_rgb(0xC8, 0x9B, 0x3C)),
    ("error", egui::Color32::from_rgb(0xD8, 0x6C, 0x6C)),
];

/// Fixed stages rather than `fill`, so the column keeps its own width
/// however wide the pane gets.
#[scene(default)]
fn column(ctx: &mut SceneCtx) {
    let width = ctx.slider("width", 280.0, 160.0, 640.0, 10.0);
    let height = ctx.slider("height", 190.0, 140.0, 400.0, 10.0);
    let count = ctx.slider("notices", 4.0, 1.0, 12.0, 1.0) as usize;

    for i in 0..count {
        let (level, tint) = LEVELS[i % LEVELS.len()];
        ctx.stage((width, height), |ui| {
            ui.label(egui::RichText::new(level).color(tint).strong());
            ui.label(egui::RichText::new(format!("Notice {}", i + 1)).size(18.0));
            ui.label("Body text long enough to wrap, so the card fills the width it was given.");
            ui.add_space(4.0);
            ui.separator();
            ui.weak("Dismiss");
        });
    }
}
