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
        ui.horizontal_wrapped(|ui| {
            for i in 0..12 {
                ui.label(format!("Item {i}"));
            }
        });
    });

    ctx.ui.add_space(12.0);
    ctx.ui.heading("More than fits");
    ctx.ui.label(
        "`scrollable()` hands the scroll area to the stage, which is what keeps the bar and the \
         clipped edge on the checkerboard with the padding inside them. It is a modifier, not a \
         size, so it goes on a pinned stage as readily as on `fill`.",
    );
    ctx.stage(Stage::Fixed(egui::vec2(300.0, 120.0)).scrollable(), |ui| {
        for i in 0..40 {
            ui.label(format!("Line {i}"));
        }
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

/// `scroll` is `fill` that scrolls once its content outgrows it.
///
/// A long list need not draw every row: the stage owns the scroll area,
/// so a scene reads `ui.clip_rect()` for the visible slice and spaces
/// over the rest — what `ScrollArea::show_rows` does when the scene owns it instead.
///
/// The label reports how much of `rows` that skips.
/// Drag `rows` down until the content fits and the bar retires.
///
/// The readout goes above the stage, not below it: a `fill` stage takes
/// the whole canvas, so anything after one pushes the canvas past
/// its own viewport and you get a second scrollbar around the first.
#[scene("scrolling")]
fn scrolling(ctx: &mut SceneCtx) {
    let rows = ctx.slider("rows", 200.0, 0.0, 1000.0, 1.0) as usize;
    let row_height =
        ctx.ui.text_style_height(&egui::TextStyle::Body) + ctx.ui.spacing().item_spacing.y;
    // Last frame's count: this frame's is not known until the stage below has drawn.
    let drawn_id = egui::Id::new("scrolling-drawn");
    let drawn = ctx.ui.data(|d| d.get_temp::<usize>(drawn_id).unwrap_or(0));
    ctx.ui
        .label(format!("{drawn} of {rows} rows drawn last frame"));

    stage!(ctx, scroll, |ui| {
        let top = ui.cursor().top();
        let clip = ui.clip_rect();
        let first = (((clip.top() - top) / row_height).floor().max(0.0) as usize).min(rows);
        let last = (((clip.bottom() - top) / row_height).ceil().max(0.0) as usize).min(rows);

        ui.add_space(first as f32 * row_height);
        for row in first..last {
            ui.label(format!("Row {row}"));
        }
        ui.add_space((rows - last) as f32 * row_height);
        ui.data_mut(|d| d.insert_temp(drawn_id, last - first));
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
