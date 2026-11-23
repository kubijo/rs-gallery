//! One scene per knob type, each showing that type's variants — a live reference for the controls
//! `SceneCtx` offers. Companion to `example.scene.rs`; delete both once you have your own scenes.

use gallery::prelude::*;

use super::globals::Globals;

scene_meta! { title: "Example / Knobs" }

/// `button` — a momentary action whose callback runs once per click.
#[scene("button")]
fn button(ctx: &mut SceneCtx, ui: &mut Ui, globals: &Globals) {
    ctx.button("Log greeting", || action("Hello from the button knob"));
    stage!(ctx, ui, globals.stage(Stage::Fit), |ui| {
        ui.label("Click the control, then open the Actions panel.");
    });
}

/// `text` — a single-line string field.
#[scene("text")]
fn text(ctx: &mut SceneCtx, ui: &mut Ui, globals: &Globals) {
    let value = ctx.text("label", "edit me").get();
    stage!(ctx, ui, globals.stage(Stage::Fit), |ui| {
        ui.label(format!("value = {value:?}"));
    });
}

/// `slider` — numeric; `step` snaps the value and sets the readout's decimals (`0.0` is smooth).
#[scene("slider")]
fn slider(ctx: &mut SceneCtx, ui: &mut Ui, globals: &Globals) {
    let smooth = ctx.slider("smooth", 0.5, 0.0, 1.0, 0.0).get();
    let whole = ctx.slider("integer", 24.0, 12.0, 64.0, 1.0).get();
    let tenths = ctx.slider("tenths", 1.0, 0.0, 5.0, 0.1).get();
    let hundredths = ctx.slider("hundredths", 0.5, 0.0, 1.0, 0.01).get();
    stage!(ctx, ui, globals.stage(Stage::Fit), |ui| {
        ui.label(format!("{smooth} · {whole} · {tenths} · {hundredths}"));
    });
}

/// `toggle` — a boolean checkbox.
#[scene("toggle")]
fn toggle(ctx: &mut SceneCtx, ui: &mut Ui, globals: &Globals) {
    let enabled = ctx.toggle("enabled", true).get();
    stage!(ctx, ui, globals.stage(Stage::Fit), |ui| {
        ui.label(if enabled { "enabled" } else { "disabled" });
    });
}

/// `color` — an sRGBA colour picker.
#[scene("color")]
fn color(ctx: &mut SceneCtx, ui: &mut Ui) {
    let tint = ctx
        .color("tint", egui::Color32::from_rgb(0x6C, 0x9C, 0xD8))
        .get();
    // Theme-independent tint on a dark backdrop.
    stage!(ctx, ui, Stage::Fit.on_dark(), |ui| {
        ui.label(egui::RichText::new("Tinted sample").size(28.0).color(tint));
    });
}

/// `select` / `radio` / `buttons` — three styles for a one-of-N choice; each holds the index.
#[scene("choice")]
fn choice(ctx: &mut SceneCtx, ui: &mut Ui, globals: &Globals) {
    const OPTIONS: &[&str] = &["one", "two", "three"];
    let dropdown = ctx.select("select", OPTIONS, 0).get();
    let radio = ctx.radio("radio", OPTIONS, 1).get();
    let segmented = ctx.buttons("buttons", OPTIONS, 2).get();
    stage!(ctx, ui, globals.stage(Stage::Fit), |ui| {
        ui.label(format!(
            "select = {}, radio = {}, buttons = {}",
            OPTIONS[dropdown], OPTIONS[radio], OPTIONS[segmented],
        ));
    });
}

/// `pad2d` — a 2-axis pad; `Pad2DSpec` sets its ranges and y-orientation.
#[scene("pad2d")]
fn pad2d(ctx: &mut SceneCtx, ui: &mut Ui, globals: &Globals) {
    let Pad2D { x, y } = ctx.pad2d("centered, -1..1", Pad2DSpec::default()).get();
    let Pad2D { x: px, y: py } = ctx
        .pad2d(
            "y-up, 0..100",
            Pad2DSpec {
                min_x: 0.0,
                max_x: 100.0,
                min_y: 0.0,
                max_y: 100.0,
                invert_y: true,
                ..Pad2DSpec::default()
            },
        )
        .get();
    stage!(ctx, ui, globals.stage(Stage::Fit), |ui| {
        ui.label(format!("({x:.2}, {y:.2}) · ({px:.0}, {py:.0})"));
    });
}

/// The write half: every accessor returns a [`Knob`],
/// and content that does its own hit-testing writes it back with `set`.
/// The panel on the right follows; plain egui buttons stand in for that content here.
#[scene("writeback")]
fn writeback(ctx: &mut SceneCtx, ui: &mut Ui, globals: &Globals) {
    const MODES: &[&str] = &["off", "auto", "on"];
    const TINTS: [(&str, egui::Color32); 3] = [
        ("red", egui::Color32::from_rgb(0xD8, 0x6C, 0x6C)),
        ("gold", egui::Color32::from_rgb(0xC8, 0x9B, 0x3C)),
        ("blue", egui::Color32::from_rgb(0x6C, 0x9C, 0xD8)),
    ];

    let count = ctx.slider("count", 3.0, 0.0, 10.0, 1.0);
    let power = ctx.toggle("power", false);
    let note = ctx.text("note", "");
    let tint = ctx.color("tint", TINTS[2].1);
    let mode = ctx.select("mode", MODES, 1);
    let aim = ctx.pad2d("aim", Pad2DSpec::default());

    stage!(ctx, ui, globals.stage((300.0, 190.0)), |ui| {
        ui.horizontal(|ui| {
            ui.label("count");
            let nudge = |step: f32| {
                count.set(count.get() + step);
                // Every write is also worth a line in the Actions panel; turn it on in the header.
                action(format!("count {:+}", step as i32));
            };
            if ui.button("−").clicked() {
                nudge(-1.0);
            }
            ui.label(format!("{}", count.get()));
            if ui.button("+").clicked() {
                nudge(1.0);
            }
        });
        ui.horizontal(|ui| {
            ui.label("power");
            let on = power.get();
            if ui.button(if on { "on" } else { "off" }).clicked() {
                power.set(!on);
                action(if on { "powered off" } else { "powered on" });
            }
        });
        ui.horizontal(|ui| {
            ui.label("note");
            for key in ["a", "b", "c"] {
                if ui.button(key).clicked() {
                    note.set(note.get() + key);
                }
            }
            if ui.button("⌫").clicked() {
                note.set(String::new());
            }
        });
        ui.horizontal(|ui| {
            ui.label("tint");
            for (name, colour) in TINTS {
                if ui
                    .add(
                        egui::Button::new(egui::RichText::new(name).color(egui::Color32::BLACK))
                            .fill(colour),
                    )
                    .clicked()
                {
                    tint.set(colour);
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("mode");
            // By option label, then the same knob by index: the scene wraps, the handle clamps.
            for name in MODES {
                if ui
                    .add(egui::Button::new(*name).selected(MODES[mode.get()] == *name))
                    .clicked()
                {
                    mode.select(name);
                    action(format!("mode: {name}"));
                }
            }
            if ui.button("cycle").clicked() {
                mode.set((mode.get() + 1) % MODES.len());
            }
        });
        ui.horizontal(|ui| {
            ui.label("aim");
            // `click_and_drag`, as the pad knob itself senses: with `click` alone
            // egui hands the gesture off once it reads as a drag, and stops reporting the pointer.
            let (rect, pad) =
                ui.allocate_exact_size(egui::vec2(96.0, 48.0), egui::Sense::click_and_drag());
            ui.painter()
                .rect_filled(rect, 2.0, egui::Color32::from_gray(0x2A));
            let Pad2D { x, y } = aim.get();
            let at = rect.lerp_inside(egui::vec2((x + 1.0) / 2.0, (y + 1.0) / 2.0));
            ui.painter().circle_filled(at, 3.0, tint.get());
            if (pad.dragged() || pad.clicked())
                && let Some(pos) = pad.interact_pointer_pos()
            {
                let norm = (pos - rect.min) / rect.size();
                aim.set(Pad2D {
                    x: norm.x * 2.0 - 1.0,
                    y: norm.y * 2.0 - 1.0,
                });
            }
        });
    });
}

/// `group` — a labelled separator that splits the knobs beneath it into sections.
#[scene("group")]
fn group(ctx: &mut SceneCtx, ui: &mut Ui) {
    ctx.group("position");
    let x = ctx.slider("x", 0.0, -1.0, 1.0, 0.1).get();
    let y = ctx.slider("y", 0.0, -1.0, 1.0, 0.1).get();
    ctx.group("style");
    let tint = ctx.color("tint", egui::Color32::WHITE).get();
    let bold = ctx.toggle("bold", false).get();
    let label = egui::RichText::new(format!("({x:.1}, {y:.1})")).color(tint);
    // Theme-independent tint on a dark backdrop.
    stage!(ctx, ui, Stage::Fit.on_dark(), |ui| {
        ui.label(if bold { label.strong() } else { label });
    });
}
