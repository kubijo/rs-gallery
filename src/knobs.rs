//! Knob data and rendering.
//!
//! [`Knob`] is the pure-data model of one control — the shell renders it with [`render_knobs`], and no
//! egui widget state lives here. The declarative-by-use accessors that create and read knobs live on
//! [`SceneCtx`](crate::SceneCtx).

use std::collections::HashMap;

/// One control in the knobs panel.
#[derive(Clone)]
pub enum Knob {
    Text {
        label: String,
        value: String,
    },
    Slider {
        label: String,
        value: f32,
        min: f32,
        max: f32,
        /// Snap increment; `0.0` for a smooth slider.
        step: f32,
    },
    Toggle {
        label: String,
        value: bool,
    },
    Color {
        label: String,
        value: egui::Color32,
    },
    Select {
        label: String,
        value: usize,
        options: Vec<String>,
        style: ChoiceStyle,
    },
    /// A 2-axis pad: two values dragged together (e.g. pitch/yaw). `invert_y` flips screen-Y → value-Y
    /// so dragging up increases y.
    Pad2D {
        label: String,
        x: f32,
        y: f32,
        min_x: f32,
        max_x: f32,
        min_y: f32,
        max_y: f32,
        invert_y: bool,
    },
    /// A labelled separator that visually groups the knobs below it.
    Group {
        label: String,
    },
}

/// How a [`Knob::Select`] renders. All three pick one option from a list; they differ only in shape:
/// `Dropdown` folds into a combo box, `Radio` stacks vertically, and `Buttons` lays out an inline
/// segmented row — condensed and glance-readable, a good fit for two- or three-state knobs.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ChoiceStyle {
    Dropdown,
    Radio,
    Buttons,
}

/// How a [`SceneCtx::pad2d`](crate::SceneCtx::pad2d) knob is set up: its default position, per-axis
/// ranges, and y-orientation.
#[derive(Clone, Copy)]
pub struct Pad2DSpec {
    pub default_x: f32,
    pub default_y: f32,
    pub min_x: f32,
    pub max_x: f32,
    pub min_y: f32,
    pub max_y: f32,
    pub invert_y: bool,
}

impl Default for Pad2DSpec {
    fn default() -> Self {
        Self {
            default_x: 0.0,
            default_y: 0.0,
            min_x: -1.0,
            max_x: 1.0,
            min_y: -1.0,
            max_y: 1.0,
            invert_y: false,
        }
    }
}

/// Each scene's persistent knobs, keyed by scene identity (so switching scenes and reloading keep
/// their own values).
pub type KnobStore = HashMap<String, Vec<Knob>>;

/// Render a scene's knobs into the controls panel. Returns `true` if the user changed any value.
pub fn render_knobs(ui: &mut egui::Ui, knobs: &mut [Knob]) -> bool {
    if knobs.is_empty() {
        ui.weak("This scene has no controls.");
        return false;
    }
    let mut changed = false;
    egui::Grid::new("gallery-knobs")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            for knob in knobs.iter_mut() {
                changed |= render_knob(ui, knob);
                ui.end_row();
            }
        });
    changed
}

/// Fractional digits in `step`, so a slider's readout matches its increment.
fn step_decimals(step: f32) -> usize {
    let mut decimals = 0;
    let mut scaled = step;
    while (scaled - scaled.round()).abs() > 1e-5 && decimals < 10 {
        scaled *= 10.0;
        decimals += 1;
    }
    decimals
}

fn render_knob(ui: &mut egui::Ui, knob: &mut Knob) -> bool {
    match knob {
        Knob::Group { label } => {
            ui.strong(label.as_str());
            ui.separator();
            false
        }
        Knob::Text { label, value } => {
            ui.label(label.as_str());
            ui.text_edit_singleline(value).changed()
        }
        Knob::Slider {
            label,
            value,
            min,
            max,
            step,
        } => {
            ui.label(label.as_str());
            let mut widget = egui::Slider::new(value, *min..=*max);
            if *step > 0.0 {
                widget = widget
                    .step_by(f64::from(*step))
                    .fixed_decimals(step_decimals(*step));
            }
            ui.add(widget).changed()
        }
        Knob::Toggle { label, value } => {
            ui.label(label.as_str());
            ui.checkbox(value, "").changed()
        }
        Knob::Color { label, value } => {
            ui.label(label.as_str());
            ui.color_edit_button_srgba(value).changed()
        }
        Knob::Select {
            label,
            value,
            options,
            style,
        } => {
            ui.label(label.as_str());
            let mut changed = false;
            match style {
                ChoiceStyle::Radio => {
                    ui.vertical(|ui| {
                        for (i, opt) in options.iter().enumerate() {
                            changed |= ui.radio_value(value, i, opt.as_str()).changed();
                        }
                    });
                }
                ChoiceStyle::Buttons => {
                    // The `vertical` wrapper is load-bearing, not redundant nesting. A bare `horizontal_wrapped`
                    // in a grid cell reports too little height for its rows, so the grid under-reserves the row
                    // and the next knob draws over them — the `wrapped_buttons_reserve_…` test guards it.
                    ui.vertical(|ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = egui::vec2(3.0, 3.0);
                            for (i, opt) in options.iter().enumerate() {
                                let active = *value == i;
                                if ui
                                    .add(egui::Button::new(opt.as_str()).selected(active))
                                    .clicked()
                                    && !active
                                {
                                    *value = i;
                                    changed = true;
                                }
                            }
                        });
                    });
                }
                ChoiceStyle::Dropdown => {
                    let selected = options.get(*value).map_or("", String::as_str);
                    egui::ComboBox::from_id_salt(label.as_str())
                        .selected_text(selected)
                        .show_ui(ui, |ui| {
                            for (i, opt) in options.iter().enumerate() {
                                changed |= ui.selectable_value(value, i, opt.as_str()).changed();
                            }
                        });
                }
            }
            changed
        }
        Knob::Pad2D {
            label,
            x,
            y,
            min_x,
            max_x,
            min_y,
            max_y,
            invert_y,
        } => {
            ui.label(label.as_str());
            render_pad2d(ui, x, y, *min_x, *max_x, *min_y, *max_y, *invert_y)
        }
    }
}

/// The pad's on-screen size — fixed, not stretched to the column width.
const PAD2D_SIZE: f32 = 80.0;

/// Render a 2-axis pad, updating `*x`/`*y` on drag/click. `invert_y` flips screen-Y → value-Y so
/// dragging up increases y.
#[expect(
    clippy::too_many_arguments,
    reason = "two values, two ranges, and an axis flag — not a meaningful struct"
)]
fn render_pad2d(
    ui: &mut egui::Ui,
    x: &mut f32,
    y: &mut f32,
    min_x: f32,
    max_x: f32,
    min_y: f32,
    max_y: f32,
    invert_y: bool,
) -> bool {
    let mut changed = false;
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(PAD2D_SIZE, PAD2D_SIZE),
        egui::Sense::click_and_drag(),
    );
    let range_x = max_x - min_x;
    let range_y = max_y - min_y;
    let to_value_y = |screen_y: f32| {
        let t = if invert_y { 1.0 - screen_y } else { screen_y };
        min_y + t * range_y
    };
    let from_value_y = |value_y: f32| {
        if range_y <= 0.0 {
            return 0.5;
        }
        let t = (value_y - min_y) / range_y;
        if invert_y { 1.0 - t } else { t }
    };

    if (response.dragged() || response.clicked())
        && let Some(pos) = response.interact_pointer_pos()
    {
        let nx = ((pos.x - rect.min.x) / PAD2D_SIZE).clamp(0.0, 1.0);
        let ny = ((pos.y - rect.min.y) / PAD2D_SIZE).clamp(0.0, 1.0);
        let new_x = min_x + nx * range_x;
        let new_y = to_value_y(ny);
        if (new_x - *x).abs() > f32::EPSILON || (new_y - *y).abs() > f32::EPSILON {
            *x = new_x;
            *y = new_y;
            changed = true;
        }
    }

    let painter = ui.painter_at(rect);
    let bg = egui::Color32::from_rgb(0x20, 0x20, 0x20);
    let border = egui::Color32::from_rgb(0x45, 0x45, 0x45);
    let cross = egui::Color32::from_rgb(0x38, 0x38, 0x38);
    let dot = egui::Color32::from_rgb(0x6C, 0x9C, 0xD8);
    painter.rect_filled(rect, 4.0, bg);
    painter.rect_stroke(
        rect,
        4.0,
        egui::Stroke::new(1.0, border),
        egui::StrokeKind::Inside,
    );
    let mid = rect.center();
    painter.line_segment(
        [
            egui::pos2(mid.x, rect.min.y + 4.0),
            egui::pos2(mid.x, rect.max.y - 4.0),
        ],
        egui::Stroke::new(1.0, cross),
    );
    painter.line_segment(
        [
            egui::pos2(rect.min.x + 4.0, mid.y),
            egui::pos2(rect.max.x - 4.0, mid.y),
        ],
        egui::Stroke::new(1.0, cross),
    );
    let norm_x = if range_x > 0.0 {
        (*x - min_x) / range_x
    } else {
        0.5
    };
    let norm_y = from_value_y(*y);
    let handle = egui::pos2(
        rect.min.x + norm_x.clamp(0.0, 1.0) * PAD2D_SIZE,
        rect.min.y + norm_y.clamp(0.0, 1.0) * PAD2D_SIZE,
    );
    painter.circle_filled(handle, 5.0, dot);
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapped_buttons_reserve_their_height_so_the_next_knob_sits_below_them() {
        use egui_kittest::kittest::Queryable;

        let options: Vec<String> = (0..8).map(|i| format!("opt-{i}")).collect();
        let mut knobs = vec![
            Knob::Select {
                label: "mode".to_owned(),
                value: 0,
                options: options.clone(),
                style: ChoiceStyle::Buttons,
            },
            Knob::Toggle {
                label: "after".to_owned(),
                value: false,
            },
        ];
        let mut harness = egui_kittest::Harness::new_ui(move |ui| {
            ui.set_max_width(180.0); // narrow enough to force the options onto several rows
            render_knobs(ui, &mut knobs);
        });
        // Second frame: the grid lays each row out from the previous frame's measured heights.
        harness.run();
        harness.run();

        let buttons_bottom = options
            .iter()
            .filter_map(|opt| harness.query_by_label(opt))
            .map(|node| node.rect().max.y)
            .fold(f32::MIN, f32::max);
        assert!(
            buttons_bottom > f32::MIN,
            "the option buttons should render"
        );
        let after_top = harness
            .query_by_label("after")
            .expect("the following knob's label renders")
            .rect()
            .min
            .y;
        assert!(
            after_top >= buttons_bottom,
            "a knob after wrapped buttons must sit below them, not overlap \
             (after.top {after_top} < buttons.bottom {buttons_bottom})"
        );
    }

    #[test]
    fn step_decimals_counts_the_fractional_digits_of_the_step() {
        assert_eq!(step_decimals(1.0), 0);
        assert_eq!(step_decimals(2.0), 0);
        assert_eq!(step_decimals(0.5), 1);
        assert_eq!(step_decimals(0.1), 1);
        assert_eq!(step_decimals(0.25), 2);
        assert_eq!(step_decimals(0.01), 2);
    }
}
