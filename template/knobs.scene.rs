//! One scene per knob type, each showing that type's variants — a live reference for the controls
//! `SceneCtx` offers. Companion to `example.scene.rs`; delete both once you have your own scenes.

use gallery::prelude::*;

scene_meta! { title: "Example / Knobs" }

/// `button` — a momentary action whose callback runs once per click.
#[scene("button")]
fn button(ctx: &mut SceneCtx, ui: &mut Ui) {
    ctx.button("Log greeting", || action("Hello from the button knob"));
    stage!(ctx, ui, |ui| {
        ui.label("Click the control, then open the Actions panel.");
    });
}

/// `text` — a single-line string field.
#[scene("text")]
fn text(ctx: &mut SceneCtx, ui: &mut Ui) {
    let value = ctx.text("label", "edit me");
    stage!(ctx, ui, |ui| {
        ui.label(format!("value = {value:?}"));
    });
}

/// `slider` — numeric; `step` snaps the value and sets the readout's decimals (`0.0` is smooth).
#[scene("slider")]
fn slider(ctx: &mut SceneCtx, ui: &mut Ui) {
    let smooth = ctx.slider("smooth", 0.5, 0.0, 1.0, 0.0);
    let whole = ctx.slider("integer", 24.0, 12.0, 64.0, 1.0);
    let tenths = ctx.slider("tenths", 1.0, 0.0, 5.0, 0.1);
    let hundredths = ctx.slider("hundredths", 0.5, 0.0, 1.0, 0.01);
    stage!(ctx, ui, |ui| {
        ui.label(format!("{smooth} · {whole} · {tenths} · {hundredths}"));
    });
}

/// `toggle` — a boolean checkbox.
#[scene("toggle")]
fn toggle(ctx: &mut SceneCtx, ui: &mut Ui) {
    let enabled = ctx.toggle("enabled", true);
    stage!(ctx, ui, |ui| {
        ui.label(if enabled { "enabled" } else { "disabled" });
    });
}

/// `color` — an sRGBA colour picker.
#[scene("color")]
fn color(ctx: &mut SceneCtx, ui: &mut Ui) {
    let tint = ctx.color("tint", egui::Color32::from_rgb(0x6C, 0x9C, 0xD8));
    stage!(ctx, ui, |ui| {
        ui.label(egui::RichText::new("Tinted sample").size(28.0).color(tint));
    });
}

/// `select` / `radio` / `buttons` — three styles for a one-of-N choice; each returns the index.
#[scene("choice")]
fn choice(ctx: &mut SceneCtx, ui: &mut Ui) {
    const OPTIONS: &[&str] = &["one", "two", "three"];
    let dropdown = ctx.select("select", OPTIONS, 0);
    let radio = ctx.radio("radio", OPTIONS, 1);
    let segmented = ctx.buttons("buttons", OPTIONS, 2);
    stage!(ctx, ui, |ui| {
        ui.label(format!(
            "select = {}, radio = {}, buttons = {}",
            OPTIONS[dropdown], OPTIONS[radio], OPTIONS[segmented],
        ));
    });
}

/// `pad2d` — a 2-axis pad; `Pad2DSpec` sets its ranges and y-orientation.
#[scene("pad2d")]
fn pad2d(ctx: &mut SceneCtx, ui: &mut Ui) {
    let Pad2D { x, y } = ctx.pad2d("centered, -1..1", Pad2DSpec::default());
    let Pad2D { x: px, y: py } = ctx.pad2d(
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
    stage!(ctx, ui, |ui| {
        ui.label(format!("({x:.2}, {y:.2}) · ({px:.0}, {py:.0})"));
    });
}

/// The `set_*` family — the write half: content that does its own hit-testing writes the knob back,
/// and the panel on the right follows. Plain egui buttons stand in for that content here.
///
/// A stage holds the `SceneCtx` for the call, so each click is noted
/// here and applied once the stage has closed.
#[scene("writeback")]
fn writeback(ctx: &mut SceneCtx, ui: &mut Ui) {
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
    let Pad2D { x, y } = ctx.pad2d("aim", Pad2DSpec::default());

    let (mut step, mut flip) = (0.0, false);
    let (mut typed, mut picked, mut chose, mut cycled, mut aimed) = (None, None, None, false, None);

    stage!(ctx, ui, (300.0, 190.0), |ui| {
        ui.horizontal(|ui| {
            ui.label("count");
            if ui.button("−").clicked() {
                step = -1.0;
            }
            ui.label(format!("{count}"));
            if ui.button("+").clicked() {
                step = 1.0;
            }
        });
        ui.horizontal(|ui| {
            ui.label("power");
            if ui.button(if power { "on" } else { "off" }).clicked() {
                flip = true;
            }
        });
        ui.horizontal(|ui| {
            ui.label("note");
            for key in ["a", "b", "c"] {
                if ui.button(key).clicked() {
                    typed = Some(format!("{note}{key}"));
                }
            }
            if ui.button("⌫").clicked() {
                typed = Some(String::new());
            }
        });
        ui.horizontal(|ui| {
            ui.label("tint");
            for (name, colour) in TINTS {
                if ui.add(egui::Button::new(name).fill(colour)).clicked() {
                    picked = Some(colour);
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("mode");
            // By option label, then the same knob by index — the scene wraps, the setter clamps.
            for name in MODES {
                if ui
                    .add(egui::Button::new(*name).selected(MODES[mode] == *name))
                    .clicked()
                {
                    chose = Some(*name);
                }
            }
            cycled = ui.button("cycle").clicked();
        });
        ui.horizontal(|ui| {
            ui.label("aim");
            // `click_and_drag`, as the pad knob itself senses: with `click` alone
            // egui hands the gesture off once it reads as a drag, and stops reporting the pointer.
            let (rect, pad) =
                ui.allocate_exact_size(egui::vec2(96.0, 48.0), egui::Sense::click_and_drag());
            ui.painter()
                .rect_filled(rect, 2.0, egui::Color32::from_gray(0x2A));
            let at = rect.lerp_inside(egui::vec2((x + 1.0) / 2.0, (y + 1.0) / 2.0));
            ui.painter().circle_filled(at, 3.0, tint);
            if (pad.dragged() || pad.clicked())
                && let Some(pos) = pad.interact_pointer_pos()
            {
                let norm = (pos - rect.min) / rect.size();
                aimed = Some((norm.x * 2.0 - 1.0, norm.y * 2.0 - 1.0));
            }
        });
    });

    if step != 0.0 {
        ctx.set_slider("count", count + step);
        // Every write is also worth a line in the Actions panel — turn it on in the header.
        action(format!("count {:+}", step as i32));
    }
    if flip {
        ctx.set_toggle("power", !power);
        action(if power { "powered off" } else { "powered on" });
    }
    if let Some(text) = typed {
        ctx.set_text("note", text);
    }
    if let Some(colour) = picked {
        ctx.set_color("tint", colour);
    }
    if let Some(name) = chose {
        ctx.set_select("mode", name);
        action(format!("mode: {name}"));
    }
    if cycled {
        ctx.set_select_index("mode", (mode + 1) % MODES.len());
    }
    if let Some((x, y)) = aimed {
        ctx.set_pad2d("aim", x, y);
    }
}

/// `group` — a labelled separator that splits the knobs beneath it into sections.
#[scene("group")]
fn group(ctx: &mut SceneCtx, ui: &mut Ui) {
    ctx.group("position");
    let x = ctx.slider("x", 0.0, -1.0, 1.0, 0.1);
    let y = ctx.slider("y", 0.0, -1.0, 1.0, 0.1);
    ctx.group("style");
    let tint = ctx.color("tint", egui::Color32::WHITE);
    let bold = ctx.toggle("bold", false);
    let label = egui::RichText::new(format!("({x:.1}, {y:.1})")).color(tint);
    stage!(ctx, ui, |ui| {
        ui.label(if bold { label.strong() } else { label });
    });
}
