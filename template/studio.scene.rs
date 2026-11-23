//! A poster editor that fills its whole stage and a play
//! that touches it with every kind of input: a click,
//! a shortcut, typing, a drag, a scroll and a hover.
//!
//! Recorded with `input = true`, the keys and the mouse
//! show over it as a screencast would.

use gallery::prelude::*;

scene_meta! { title: "Example / Studio" }

const SWATCHES: [(&str, egui::Color32); 12] = [
    ("Coral", egui::Color32::from_rgb(232, 93, 78)),
    ("Amber", egui::Color32::from_rgb(230, 160, 40)),
    ("Lime", egui::Color32::from_rgb(120, 180, 50)),
    ("Teal", egui::Color32::from_rgb(30, 150, 140)),
    ("Sky", egui::Color32::from_rgb(60, 140, 220)),
    ("Indigo", egui::Color32::from_rgb(80, 80, 200)),
    ("Violet", egui::Color32::from_rgb(140, 70, 200)),
    ("Rose", egui::Color32::from_rgb(210, 70, 140)),
    ("Slate", egui::Color32::from_rgb(90, 100, 120)),
    ("Sand", egui::Color32::from_rgb(190, 160, 110)),
    ("Mint", egui::Color32::from_rgb(70, 190, 150)),
    ("Ink", egui::Color32::from_rgb(30, 30, 40)),
];

#[derive(Clone)]
struct Poster {
    title: String,
    bold: bool,
    size: f32,
    swatch: usize,
}

impl Default for Poster {
    fn default() -> Self {
        Self {
            title: "Poster".to_owned(),
            bold: false,
            size: 48.0,
            swatch: 0,
        }
    }
}

#[scene(default)]
fn studio(ctx: &mut SceneCtx, ui: &mut Ui) {
    ctx.stage(ui, Stage::Fill.padding(0), |ui| {
        let id = ui.make_persistent_id("studio-demo-state");
        let mut poster = ui.data_mut(|data| data.get_temp_mut_or_default::<Poster>(id).clone());

        let shortcut = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::B);
        if ui.input_mut(|input| input.consume_shortcut(&shortcut)) {
            poster.bold = !poster.bold;
            action(if poster.bold { "bold on" } else { "bold off" });
        }

        backdrop(ui);
        ui.horizontal_top(|ui| {
            egui::Frame::window(ui.style())
                .outer_margin(16)
                .show(ui, |ui| ui.vertical(|ui| controls(ui, &mut poster)));
            let (preview, _) = ui.allocate_exact_size(ui.available_size(), egui::Sense::hover());
            show(ui, preview, &poster);
        });

        ui.data_mut(|data| data.insert_temp(id, poster));
    });
}

/// Pale bands of every swatch across the whole stage.
fn backdrop(ui: &Ui) {
    let full = ui.max_rect();
    let width = full.width() / SWATCHES.len() as f32;
    for (at, (_, colour)) in SWATCHES.iter().enumerate() {
        let band = egui::Rect::from_min_size(
            full.min + egui::vec2(width * at as f32, 0.0),
            egui::vec2(width, full.height()),
        );
        ui.painter()
            .rect_filled(band, 0.0, colour.lerp_to_gamma(egui::Color32::WHITE, 0.7));
    }
}

fn controls(ui: &mut Ui, poster: &mut Poster) {
    ui.set_width(220.0);
    ui.heading("Poster");
    let label = ui.label("Title");
    ui.text_edit_singleline(&mut poster.title)
        .labelled_by(label.id);
    ui.checkbox(&mut poster.bold, "Bold (Ctrl+B)");
    ui.spacing_mut().slider_width = 140.0;
    ui.add(egui::Slider::new(&mut poster.size, 16.0..=120.0).text("Size"));
    ui.label("Colour");
    egui::ScrollArea::vertical()
        .id_salt("swatches")
        .max_height(110.0)
        .show(ui, |ui| {
            for (at, (name, _)) in SWATCHES.iter().enumerate() {
                if ui.selectable_value(&mut poster.swatch, at, *name).clicked() {
                    action(format!("colour {name}"));
                }
            }
        });
    if ui.button("Reset").clicked() {
        *poster = Poster::default();
        action("reset");
    }
}

/// The title as the poster sets it, centred in `area`.
fn show(ui: &Ui, area: egui::Rect, poster: &Poster) {
    let (_, colour) = SWATCHES[poster.swatch];
    let font = egui::FontId::proportional(poster.size);
    // egui's default fonts have no bold face; a second pass a point over thickens the strokes.
    let passes: &[f32] = if poster.bold { &[0.0, 1.0] } else { &[0.0] };
    for nudge in passes {
        ui.painter().text(
            area.center() + egui::vec2(*nudge, 0.0),
            egui::Align2::CENTER_CENTER,
            &poster.title,
            font.clone(),
            colour,
        );
    }
}

#[play(studio)]
fn tours_every_kind_of_input(p: &mut Play) -> PlayResult {
    p.step("name the poster", |p| {
        p.click("Title")?;
        p.press_with(egui::Modifiers::COMMAND, egui::Key::A)?;
        p.type_text("Hello, gallery")
    })?;
    p.step("make it bold", |p| {
        p.press_with(egui::Modifiers::COMMAND, egui::Key::B)?;
        p.expect_action("bold on")
    })?;
    p.step("grow the type", |p| {
        p.drag(
            By::new().role(egui::accesskit::Role::Slider),
            egui::vec2(30.0, 0.0),
        )
    })?;
    p.step("scroll to violet", |p| {
        p.scroll("Coral", egui::vec2(0.0, -120.0))
    })?;
    p.step("paint it violet", |p| {
        p.click("Violet")?;
        p.expect_action("colour Violet")
    })?;
    p.step("look it over", |p| p.hover("Reset"))
}
