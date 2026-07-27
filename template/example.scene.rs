//! An example scene, so a fresh scaffold shows something on the first `cargo run`.
//! Delete it once you have your own scenes, and point `gallery.toml`'s `scene_globs`
//! at wherever they live.
//!
//! A scene file sits next to the component it exercises: `scene_meta!`
//! names its place in the sidebar tree, `#[scene]` functions show
//! the component in each state.
//!
//! The canvas is plain, so headings and prose go straight onto `ctx.ui`;
//! whatever you are demonstrating goes in a `stage!`, which puts it
//! on the checkerboard and captions its size.

use gallery::prelude::*;

scene_meta! { title: "Example / Greeting" }

/// The simplest scene: one component, fitted.
#[scene("plain")]
fn plain(ctx: &mut SceneCtx) {
    stage!(ctx, |ui| {
        ui.heading("Hello, world");
    });
}

/// Controls (knobs) are declarative-by-use: each `ctx.<knob>` call
/// adds a control to the right-hand panel and returns its value.
/// Read them before the stage, whose closure borrows only the `Ui`.
#[scene("with controls")]
fn with_controls(ctx: &mut SceneCtx) {
    let name = ctx.text("name", "world");
    let size = ctx.slider("size", 24.0, 12.0, 64.0, 1.0);
    let shout = ctx.toggle("uppercase", false);
    let color = ctx.color("color", egui::Color32::from_rgb(0x6C, 0x9C, 0xD8));

    let name = if shout { name.to_uppercase() } else { name };
    stage!(ctx, |ui| {
        ui.label(
            egui::RichText::new(format!("Hello, {name}"))
                .size(size)
                .color(color),
        );
    });
}

/// Several demos with prose between them
/// — what the plain canvas is for.
#[scene("document")]
fn document(ctx: &mut SceneCtx) {
    ctx.ui.heading("Sized to its content");
    ctx.ui
        .label("A bare closure wraps whatever you draw, and the badge reports the result.");
    stage!(ctx, |ui| {
        ui.label("A short label takes only the room it needs.");
    });

    ctx.ui.add_space(12.0);
    ctx.ui.heading("A fixed viewport");
    ctx.ui.label(
        "A size pins it, for a component that behaves differently depending on how much room \
         it has — a scroll area, a wrapping layout, anything with a breakpoint.",
    );
    stage!(ctx, (300, 120), |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            for i in 0..12 {
                ui.label(format!("Item {i}"));
            }
        });
    });

    ctx.ui.add_space(12.0);
    ctx.ui.heading("Whatever is left");
    ctx.ui.label(
        "`fill` takes the rest of the canvas — here, what the sections above have not used.",
    );
    stage!(ctx, fill, |ui| {
        ui.centered_and_justified(|ui| ui.label("Filling the rest"));
    });
}

/// One component on the whole canvas — what `fill` exists for, and how every scene looked before
/// stages.
#[scene("full canvas")]
fn full_canvas(ctx: &mut SceneCtx) {
    stage!(ctx, fill, |ui| {
        ui.centered_and_justified(|ui| ui.heading("The whole canvas, one component"));
    });
}

/// Demonstrates the `buttons` knob. `weight` is a compact tri-state; `step`'s long labels wrap
/// onto further rows, and `size` and `note` follow the wrapped row to drive the scene preview.
#[scene("segmented buttons")]
fn segmented_buttons(ctx: &mut SceneCtx) {
    const WEIGHTS: &[&str] = &["light", "regular", "bold"];
    const STEPS: &[&str] = &[
        "idle",
        "connecting",
        "authenticating",
        "downloading",
        "verifying",
        "installing",
        "finalizing",
        "done",
    ];

    let weight = ctx.buttons("weight", WEIGHTS, 1);
    let step = ctx.buttons("step", STEPS, 0);
    let size = ctx.slider("size", 28.0, 12.0, 48.0, 1.0);
    let note = ctx.text("note", "");

    let mut text = egui::RichText::new(STEPS[step]).size(size);
    text = match weight {
        0 => text.weak(),
        2 => text.strong(),
        _ => text,
    };
    stage!(ctx, |ui| {
        ui.label(text);
        if !note.is_empty() {
            ui.weak(note);
        }
    });
}
