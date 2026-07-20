//! An example scene, so a fresh scaffold shows something on the first `cargo run`.
//! Delete it once you have your own scenes, and point `gallery.toml`'s `scene_globs`
//! at wherever they live.
//!
//! A scene file sits next to the component it exercises:
//! it names its place in the sidebar tree with `scene_meta!`
//! and stages the component in each state with `#[scene]` functions.

use gallery::prelude::*;

scene_meta! { title: "Example / Greeting" }

/// The simplest scene: no controls, just draw into `ctx.ui`.
#[scene("plain")]
fn plain(ctx: &mut SceneCtx) {
    ctx.ui.heading("Hello, world");
}

/// Controls (knobs) are declarative-by-use: each `ctx.<knob>` call adds
/// a control to the right-hand panel and returns its current value,
/// so editing a control re-renders the scene.
#[scene("with controls")]
fn with_controls(ctx: &mut SceneCtx) {
    let name = ctx.text("name", "world");
    let size = ctx.slider("size", 24.0, 12.0, 64.0, 1.0);
    let shout = ctx.toggle("uppercase", false);
    let color = ctx.color("color", egui::Color32::from_rgb(0x6C, 0x9C, 0xD8));

    let name = if shout { name.to_uppercase() } else { name };
    ctx.ui.label(
        egui::RichText::new(format!("Hello, {name}"))
            .size(size)
            .color(color),
    );
}

/// Demonstrates the `buttons` knob. `weight` is a compact tri-state; `stage`'s long labels wrap
/// onto further rows, and `size` and `note` follow the wrapped row to drive the scene preview.
#[scene("segmented buttons")]
fn segmented_buttons(ctx: &mut SceneCtx) {
    const WEIGHTS: &[&str] = &["light", "regular", "bold"];
    const STAGES: &[&str] = &[
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
    let stage = ctx.buttons("stage", STAGES, 0);
    let size = ctx.slider("size", 28.0, 12.0, 48.0, 1.0);
    let note = ctx.text("note", "");

    let mut text = egui::RichText::new(STAGES[stage]).size(size);
    text = match weight {
        0 => text.weak(),
        2 => text.strong(),
        _ => text,
    };
    ctx.ui.label(text);
    if !note.is_empty() {
        ctx.ui.weak(note);
    }
}
