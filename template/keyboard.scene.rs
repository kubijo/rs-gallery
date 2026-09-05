//! Focus locks keep keys with a widget; consuming handled events prevents gallery navigation.

use gallery::prelude::*;

scene_meta! { title: "Example / Keyboard" }

#[scene(default)]
fn keyboard(ctx: &mut SceneCtx, ui: &mut Ui) {
    ui.heading("Keyboard ownership");
    ui.add_space(6.0);
    ui.label("Click the panel, then press Tab, Shift+Tab, and Escape. Escape releases focus.");
    ui.label("Outside the panel, Tab cycles scenes; in text fields it moves focus.");
    ui.label("The panel shows the last key received; Actions keeps a log.");
    ui.add_space(6.0);
    ui.weak(format!("Loaded code: {:?}", ctx.scene_revision()));
    ui.add_space(12.0);

    let state_id = ui.make_persistent_id("keyboard-demo-state");
    let (mut counts, mut first, mut second, mut code, mut last_key) = ui.data_mut(|data| {
        data.get_temp_mut_or_default::<([u32; 3], String, String, String, String)>(state_id)
            .clone()
    });

    let (rect, response) = ui.allocate_exact_size(egui::vec2(360.0, 90.0), egui::Sense::click());
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "Key capture"));
    if response.clicked() {
        response.request_focus();
    }
    if response.has_focus() {
        ui.memory_mut(|memory| {
            memory.set_focus_lock_filter(
                response.id,
                egui::EventFilter {
                    tab: true,
                    escape: true,
                    horizontal_arrows: true,
                    vertical_arrows: true,
                },
            );
        });
        let mut released = false;
        ui.input_mut(|input| {
            input.events.retain(|event| {
                if released {
                    return true;
                }
                let egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } = event
                else {
                    return true;
                };
                last_key = egui::KeyboardShortcut::new(*modifiers, *key)
                    .format(&egui::ModifierNames::NAMES, cfg!(target_os = "macos"));
                action(format!("Key pressed: {last_key}"));
                match key {
                    egui::Key::Tab if modifiers.is_none() || modifiers.shift_only() => {
                        counts[usize::from(modifiers.shift)] += 1;
                        false
                    }
                    egui::Key::Escape if modifiers.is_none() => {
                        counts[2] += 1;
                        released = true;
                        false
                    }
                    _ => true,
                }
            });
        });
        if released {
            response.surrender_focus();
        }
    }
    let focused = response.has_focus();
    ui.painter()
        .rect_filled(rect, 0.0, egui::Color32::from_gray(24));
    ui.painter().rect_stroke(
        rect,
        0.0,
        egui::Stroke::new(
            1.0,
            if focused {
                egui::Color32::LIGHT_BLUE
            } else {
                egui::Color32::GRAY
            },
        ),
        egui::StrokeKind::Inside,
    );
    let status = if focused {
        "Capturing keys"
    } else {
        "Click to capture keys"
    };
    let last_key_label = format!(
        "Last key: {}",
        if last_key.is_empty() {
            "—"
        } else {
            &last_key
        }
    );
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        format!("{status}\n{last_key_label}"),
        egui::FontId::proportional(18.0),
        egui::Color32::WHITE,
    );
    ui.interact(rect, response.id.with("last-key"), egui::Sense::hover())
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, &last_key_label));
    ui.add_space(6.0);
    ui.label(format!(
        "Tab: {}   Shift+Tab: {}   Escape: {}",
        counts[0], counts[1], counts[2]
    ));
    ui.add_space(4.0);
    if ui.button("Reset counters").clicked() {
        counts = [0; 3];
    }

    ui.add_space(16.0);
    let label = ui.label("First text field");
    ui.text_edit_singleline(&mut first).labelled_by(label.id);
    ui.add_space(10.0);
    let label = ui.label("Second text field");
    ui.text_edit_singleline(&mut second).labelled_by(label.id);
    ui.add_space(10.0);
    let label = ui.label("Code editor (Tab indents)");
    ui.add(
        egui::TextEdit::multiline(&mut code)
            .code_editor()
            .desired_rows(2),
    )
    .labelled_by(label.id);
    ui.data_mut(|data| data.insert_temp(state_id, (counts, first, second, code, last_key)));
}
