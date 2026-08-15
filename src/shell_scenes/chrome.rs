//! What every panel is built out of: the header bar it opens with, the caret that folds it, and
//! the rail it folds to.

use gallery::prelude::*;

use crate::{Caret, caret, collapsed_panel, header_bar, header_title};

scene_meta! { title: "Shell / Chrome" }

/// The slim bar each panel opens with: a title, and whatever the panel puts on the right.
#[scene(default)]
fn header(ctx: &mut SceneCtx, ui: &mut Ui) {
    let title = ctx.text("title", "Controls");
    ctx.stage(ui, egui::vec2(420.0, 28.0), |ui| {
        let mut bar = header_bar(ui);
        bar.label(header_title(&title));
        bar.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().button_padding = egui::vec2(4.0, 1.0);
            let mut showing = true;
            ui.selectable_value(&mut showing, false, "Source");
            ui.selectable_value(&mut showing, true, "Preview");
        });
    });
}

/// What folds a panel, pointing the way it goes — each hugs its panel's canvas-facing edge.
#[scene("carets", order = 1)]
fn carets(ctx: &mut SceneCtx, ui: &mut Ui) {
    ui.label("Hover one for the tooltip it carries.");
    ui.add_space(8.0);
    for (at, (dir, hint)) in [
        (Caret::Left, "Collapse scenes (Cmd+Shift+L)"),
        (Caret::Right, "Collapse controls (Cmd+Shift+R)"),
    ]
    .into_iter()
    .enumerate()
    {
        ui.push_id(at, |ui| {
            ctx.stage(ui, egui::vec2(60.0, 26.0), |ui| {
                caret(ui, dir).on_hover_text(hint);
            });
        });
    }
}

/// Where a folded panel goes: a rail down the edge, holding the caret that brings it back.
/// Clicking it opens a panel that is not there in a scene, so it vanishes; the knob puts it back.
#[scene("rail", order = 2)]
fn rail(ctx: &mut SceneCtx, ui: &mut Ui) {
    let mut folded = !ctx.toggle("panel open", false);
    ctx.stage(ui, egui::vec2(240.0, 80.0), |ui| {
        collapsed_panel(
            ui,
            "shell-scenes-rail",
            true,
            "Show scenes (Cmd+Shift+L)",
            &mut folded,
        );
    });
}
