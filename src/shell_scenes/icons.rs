//! The bundled SVGs, tessellated rather than rasterised, so they hold their edges at any size.

use gallery::prelude::*;

use crate::svg::Icons;

scene_meta! { title: "Shell / Icons" }

// Loaded once, as the shell loads them: parsing and tessellating is no per-frame work.
thread_local! {
    static ICONS: Icons = Icons::load();
}

/// Every icon at the sizes the shell draws them, and past that: a curve that goes wrong shows up
/// long before it does at twelve points.
#[scene(default)]
fn sizes(ctx: &mut SceneCtx, ui: &mut Ui) {
    let tint = ctx.color("tint", crate::MUTED);
    ICONS.with(|icons| {
        for (name, icon) in [
            ("folder", &icons.folder),
            ("app", &icons.app),
            ("search", &icons.search),
            ("window-minimize", &icons.window_minimize),
            ("window-maximize", &icons.window_maximize),
            ("window-restore", &icons.window_restore),
            ("window-close", &icons.window_close),
        ] {
            ui.label(name);
            ctx.stage(ui, egui::vec2(220.0, 44.0), |ui| {
                ui.horizontal_centered(|ui| {
                    for size in [12.0, 16.0, 24.0, 32.0] {
                        icon.show(ui, size, tint);
                        ui.add_space(6.0);
                    }
                });
            });
        }
    });
}

/// The tints the sidebar sorts by: gold for a folder, blue for a scene.
#[scene("tints", order = 1)]
fn tints(ctx: &mut SceneCtx, ui: &mut Ui) {
    ICONS.with(|icons| {
        ctx.stage(ui, egui::vec2(160.0, 40.0), |ui| {
            ui.horizontal_centered(|ui| {
                icons.folder.show(ui, 16.0, crate::FOLDER_TINT);
                ui.add_space(10.0);
                icons.app.show(ui, 16.0, crate::SCENE_TINT);
            });
        });
    });
}
