//! One component at every size it has to survive, laid out across the pane rather than down it.
//!
//! `ctx.matrix` takes as many columns as the pane holds and aligns them in a grid,
//! so narrowing the window reflows the set instead of running it off the edge.

use gallery::prelude::*;

scene_meta! { title: "Layout / Size matrix" }

/// The sizes the component is expected to hold up at, smallest first.
const BREAKPOINTS: [(&str, f32, f32); 4] = [
    ("compact", 150.0, 110.0),
    ("small", 210.0, 130.0),
    ("medium", 280.0, 150.0),
    ("wide", 360.0, 130.0),
];

fn sizes() -> Vec<egui::Vec2> {
    BREAKPOINTS
        .iter()
        .map(|(_, width, height)| egui::vec2(*width, *height))
        .collect()
}

/// Every breakpoint at once, so a change that only breaks the narrow one
/// shows up without resizing anything.
#[scene(default)]
fn breakpoints(ctx: &mut SceneCtx, ui: &mut Ui) {
    let title = ctx.text("title", "Firmware update");
    let body = ctx.text(
        "body",
        "Version 4.2.1 is ready to install. The device restarts once and keeps its settings.",
    );
    let tint = ctx.color("tint", egui::Color32::from_rgb(0x6C, 0x9C, 0xD8));

    ctx.matrix(ui, &sizes(), |ui, at| {
        let (name, ..) = BREAKPOINTS[at];
        ui.label(egui::RichText::new(name).color(tint).small());
        ui.label(egui::RichText::new(&title).size(16.0).strong());
        ui.label(&body);
        ui.add_space(4.0);
        // A stage in a matrix interacts like any other.
        ui.horizontal(|ui| {
            for label in ["Install", "Later"] {
                if ui.button(label).clicked() {
                    action(format!("{label} · {name}"));
                }
            }
        });
    });
}

/// A copy length per breakpoint, which is the question a size matrix asks:
/// give the narrow one the long string and it outgrows the box it was staged at.
/// Pushing one size alone means a knob declared inside the cell, which `matrix_with` allows.
#[scene("copy length")]
fn copy_length(ctx: &mut SceneCtx, ui: &mut Ui) {
    /// What the knob calls each length, and the body text it stands for.
    const COPY: [(&str, &str); 2] = [
        ("short", "Version 4.2.1 is ready."),
        (
            "long",
            "Version 4.2.1 is ready to install. The device restarts once, keeps its settings, \
             and reconnects on its own.",
        ),
    ];

    let sizes = sizes();
    let lengths = COPY.map(|(length, _)| length);
    ctx.matrix_with(ui, &sizes, |ctx, ui, at| {
        let (name, ..) = BREAKPOINTS[at];
        let chosen = ctx.buttons(&format!("{name} copy"), &lengths, 0);
        ctx.stage(ui, sizes[at], |ui| {
            ui.label(egui::RichText::new(name).small());
            ui.label(egui::RichText::new("Firmware update").size(16.0).strong());
            ui.label(COPY[chosen].1);
        });
    });
}

/// The same breakpoints stacked, for comparison — one scroll each.
#[scene("stacked")]
fn stacked(ctx: &mut SceneCtx, ui: &mut Ui) {
    let tint = ctx.color("tint", egui::Color32::from_rgb(0x6C, 0x9C, 0xD8));
    for (name, width, height) in BREAKPOINTS {
        ctx.stage(ui, (width, height), |ui| {
            ui.label(egui::RichText::new(name).color(tint).small());
            ui.label("Firmware update");
        });
    }
}
