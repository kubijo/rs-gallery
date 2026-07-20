//! One scene per knob type, each showing that type's variants — a live reference for the controls
//! `SceneCtx` offers. Companion to `example.scene.rs`; delete both once you have your own scenes.

use gallery::prelude::*;

scene_meta! { title: "Example / Knobs" }

/// `text` — a single-line string field.
#[scene("text")]
fn text(ctx: &mut SceneCtx) {
    let value = ctx.text("label", "edit me");
    ctx.ui.label(format!("value = {value:?}"));
}

/// `slider` — numeric; `step` snaps the value and sets the readout's decimals (`0.0` is smooth).
#[scene("slider")]
fn slider(ctx: &mut SceneCtx) {
    let smooth = ctx.slider("smooth", 0.5, 0.0, 1.0, 0.0);
    let whole = ctx.slider("integer", 24.0, 12.0, 64.0, 1.0);
    let tenths = ctx.slider("tenths", 1.0, 0.0, 5.0, 0.1);
    let hundredths = ctx.slider("hundredths", 0.5, 0.0, 1.0, 0.01);
    ctx.ui
        .label(format!("{smooth} · {whole} · {tenths} · {hundredths}"));
}

/// `toggle` — a boolean checkbox.
#[scene("toggle")]
fn toggle(ctx: &mut SceneCtx) {
    let enabled = ctx.toggle("enabled", true);
    ctx.ui.label(if enabled { "enabled" } else { "disabled" });
}

/// `color` — an sRGBA colour picker.
#[scene("color")]
fn color(ctx: &mut SceneCtx) {
    let tint = ctx.color("tint", egui::Color32::from_rgb(0x6C, 0x9C, 0xD8));
    ctx.ui
        .label(egui::RichText::new("Tinted sample").size(28.0).color(tint));
}

/// `select` / `radio` / `buttons` — three styles for a one-of-N choice; each returns the index.
#[scene("choice")]
fn choice(ctx: &mut SceneCtx) {
    const OPTIONS: &[&str] = &["one", "two", "three"];
    let dropdown = ctx.select("select", OPTIONS, 0);
    let radio = ctx.radio("radio", OPTIONS, 1);
    let segmented = ctx.buttons("buttons", OPTIONS, 2);
    ctx.ui.label(format!(
        "select = {}, radio = {}, buttons = {}",
        OPTIONS[dropdown], OPTIONS[radio], OPTIONS[segmented],
    ));
}

/// `pad2d` — a 2-axis pad; `Pad2DSpec` sets its ranges and y-orientation.
#[scene("pad2d")]
fn pad2d(ctx: &mut SceneCtx) {
    let (x, y) = ctx.pad2d("centered, -1..1", Pad2DSpec::default());
    let (px, py) = ctx.pad2d(
        "y-up, 0..100",
        Pad2DSpec {
            min_x: 0.0,
            max_x: 100.0,
            min_y: 0.0,
            max_y: 100.0,
            invert_y: true,
            ..Pad2DSpec::default()
        },
    );
    ctx.ui
        .label(format!("({x:.2}, {y:.2}) · ({px:.0}, {py:.0})"));
}

/// `group` — a labelled separator that splits the knobs beneath it into sections.
#[scene("group")]
fn group(ctx: &mut SceneCtx) {
    ctx.group("position");
    let x = ctx.slider("x", 0.0, -1.0, 1.0, 0.1);
    let y = ctx.slider("y", 0.0, -1.0, 1.0, 0.1);
    ctx.group("style");
    let tint = ctx.color("tint", egui::Color32::WHITE);
    let bold = ctx.toggle("bold", false);
    let label = egui::RichText::new(format!("({x:.1}, {y:.1})")).color(tint);
    ctx.ui.label(if bold { label.strong() } else { label });
}
