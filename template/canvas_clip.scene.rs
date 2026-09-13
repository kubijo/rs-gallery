//! A scene can replace a child UI's inherited clip rectangle.
//! The card at the far edge must remain inside the canvas
//! even when the fixed stage is wider than the gallery viewport.

use gallery::prelude::*;

scene_meta! { title: "Regression / Canvas containment" }

#[scene]
fn widened_child_clip(ctx: &mut SceneCtx, ui: &mut Ui) {
    stage!(ctx, ui, (960, 600), |ui| {
        let root = ui.available_rect_before_wrap();
        ui.painter()
            .rect_filled(root, 0.0, egui::Color32::from_gray(20));

        let card = egui::Rect::from_min_size(
            egui::pos2(root.right() - 128.0, root.top()),
            egui::vec2(128.0, 44.0),
        );
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(card));
        child.set_clip_rect(card);
        child
            .painter()
            .rect_filled(card, 4.0, egui::Color32::from_rgb(96, 24, 48));
        child.painter().rect_stroke(
            card,
            4.0,
            egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 64, 128)),
            egui::StrokeKind::Inside,
        );
        child.painter().text(
            card.center(),
            egui::Align2::CENTER_CENTER,
            "Escaped paint",
            egui::FontId::proportional(14.0),
            egui::Color32::WHITE,
        );
    });
}
