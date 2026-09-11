//! Knob data and rendering.
//!
//! [`Knob`] is the pure-data model of one control — the shell renders it with [`render_knobs`], and no
//! egui widget state lives here. The declarative-by-use accessors that create and read knobs live on
//! [`SceneCtx`](crate::SceneCtx).

use std::collections::HashMap;

use crate::Icon;

/// One scene or catalog-global control.
#[derive(Clone)]
pub enum Knob {
    /// A momentary action. `clicked` is set by the panel and consumed by the scene on its next
    /// [`SceneCtx::button`](crate::SceneCtx::button) call.
    Button {
        label: String,
        clicked: bool,
    },
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
    /// An icon choice with stable labels for persistence and accessibility.
    IconButtons {
        label: String,
        value: usize,
        options: Vec<String>,
        icons: Vec<Icon>,
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

/// Where a [`SceneCtx::pad2d`](crate::SceneCtx::pad2d) knob sits,
/// in the ranges its [`Pad2DSpec`] declared.
#[derive(Clone, Copy, Default, PartialEq, Debug, serde::Deserialize, serde::Serialize)]
pub struct Pad2D {
    pub x: f32,
    pub y: f32,
}

/// How a [`SceneCtx::pad2d`](crate::SceneCtx::pad2d) knob is set up: its default position, per-axis
/// ranges, and y-orientation.
#[derive(Clone, Copy, serde::Deserialize, serde::Serialize)]
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
    render_knob_grid(ui, "gallery-scene-knobs", knobs.iter_mut())
}

pub(crate) fn has_panel_globals(knobs: &[Knob]) -> bool {
    knobs
        .iter()
        .any(|knob| !matches!(knob, Knob::IconButtons { .. }))
}

pub(crate) fn render_panel_globals(ui: &mut egui::Ui, knobs: &mut [Knob]) -> bool {
    render_knob_grid(
        ui,
        "gallery-global-knobs",
        knobs
            .iter_mut()
            .filter(|knob| !matches!(knob, Knob::IconButtons { .. })),
    )
}

fn render_knob_grid<'a>(
    ui: &mut egui::Ui,
    id_salt: &'static str,
    knobs: impl Iterator<Item = &'a mut Knob>,
) -> bool {
    let mut changed = false;
    egui::Grid::new(ui.id().with(id_salt))
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            for knob in knobs {
                changed |= render_knob(ui, knob);
                ui.end_row();
            }
        });
    changed
}

pub(crate) fn render_global_toolbar(ui: &mut egui::Ui, knobs: &mut [Knob]) -> bool {
    let mut changed = false;
    ui.spacing_mut().item_spacing.x = 2.0 / ui.ctx().pixels_per_point();
    ui.spacing_mut().button_padding = egui::vec2(4.0, 1.0);

    let mut first = true;
    for knob in knobs.iter_mut().rev() {
        let Knob::IconButtons {
            label,
            value,
            options,
            icons,
        } = knob
        else {
            continue;
        };
        if !first {
            ui.add_space(8.0);
        }
        first = false;
        for i in (0..options.len()).rev() {
            changed |= render_icon_option(ui, label, value, i, options, icons, false);
        }
    }
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
        Knob::Button { label, clicked } => {
            // The label belongs on the button itself; the empty first cell keeps it aligned with
            // the widgets of value-carrying knobs without repeating the label beside it.
            ui.label("");
            if ui.button(label.as_str()).clicked() {
                *clicked = true;
                true
            } else {
                false
            }
        }
        Knob::Group { label } => {
            ui.strong(label.as_str());
            ui.separator();
            false
        }
        Knob::Text { label, value } => {
            let label = ui.label(label.as_str());
            ui.text_edit_singleline(value)
                .labelled_by(label.id)
                .changed()
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
                    // A wrapping row underreports its height in a grid.
                    // The vertical wrapper reserves the full row.
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
        Knob::IconButtons {
            label,
            value,
            options,
            icons,
        } => {
            ui.label(label.as_str());
            let mut changed = false;
            ui.vertical(|ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(3.0, 3.0);
                    for i in 0..options.len() {
                        changed |= render_icon_option(ui, label, value, i, options, icons, true);
                    }
                });
            });
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

fn render_icon_option(
    ui: &mut egui::Ui,
    group: &str,
    value: &mut usize,
    index: usize,
    options: &[String],
    icons: &[Icon],
    show_caption: bool,
) -> bool {
    let active = *value == index;
    let option = &options[index];
    let occurrence = options[..index]
        .iter()
        .filter(|candidate| *candidate == option)
        .count();
    let icon_id = ui
        .id()
        .with(("gallery-icon-button", group, option.as_str(), occurrence));
    let icon_size = if show_caption { 14.0 } else { 16.0 };
    let icon_atom = egui::Atom::custom(icon_id, egui::Vec2::splat(icon_size));
    let button = if show_caption {
        egui::Button::new((icon_atom, option.as_str())).gap(6.0)
    } else {
        egui::Button::new(icon_atom).min_size(egui::Vec2::splat(22.0))
    };
    let rendered = button.selected(active).atom_ui(ui);
    let icon_rect = rendered.rect(icon_id);
    let mut response = rendered.response;
    if !show_caption {
        let tooltip = format!("{group}: {option}");
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), active, &tooltip)
        });
        response = response.on_hover_text(tooltip);
    }
    if let (Some(icon), Some(rect)) = (icons.get(index), icon_rect) {
        let color = ui
            .style()
            .interact_selectable(&response, active)
            .fg_stroke
            .color;
        icon.paint(ui.painter(), rect, color);
    }
    if response.clicked() && !active {
        *value = index;
        true
    } else {
        false
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

    /// One render over every variant: `render_knob` is a match with an arm per kind,
    /// and a kind that panicked or drew nothing would otherwise only show up in the shell.
    #[test]
    fn every_knob_kind_renders_under_its_own_label() {
        use egui_kittest::kittest::Queryable;

        let options = |a: &str, b: &str| vec![a.to_owned(), b.to_owned()];
        let icon = Icon::from_svg(include_bytes!("../assets/icons/app.svg"));
        let mut knobs = vec![
            Knob::Button {
                label: "rebuild".to_owned(),
                clicked: false,
            },
            Knob::Group {
                label: "section".to_owned(),
            },
            Knob::Text {
                label: "note".to_owned(),
                value: "hello".to_owned(),
            },
            Knob::Slider {
                label: "size".to_owned(),
                value: 2.0,
                min: 0.0,
                max: 4.0,
                step: 0.5,
            },
            Knob::Toggle {
                label: "enabled".to_owned(),
                value: true,
            },
            Knob::Color {
                label: "tint".to_owned(),
                value: egui::Color32::from_rgb(0x6C, 0x9C, 0xD8),
            },
            Knob::Select {
                label: "dropdown".to_owned(),
                value: 0,
                options: options("alpha", "beta"),
                style: ChoiceStyle::Dropdown,
            },
            Knob::Select {
                label: "radio".to_owned(),
                value: 1,
                options: options("gamma", "delta"),
                style: ChoiceStyle::Radio,
            },
            Knob::Select {
                label: "buttons".to_owned(),
                value: 0,
                options: options("epsilon", "zeta"),
                style: ChoiceStyle::Buttons,
            },
            Knob::IconButtons {
                label: "icon buttons".to_owned(),
                value: 0,
                options: options("eta", "theta"),
                icons: vec![icon.clone(), icon],
            },
            Knob::Pad2D {
                label: "aim".to_owned(),
                x: 0.0,
                y: 0.0,
                min_x: -1.0,
                max_x: 1.0,
                min_y: -1.0,
                max_y: 1.0,
                invert_y: true,
            },
        ];
        let mut harness = egui_kittest::Harness::new_ui(move |ui| {
            render_knobs(ui, &mut knobs);
        });
        harness.run();
        harness.run();

        for label in [
            "rebuild",
            "section",
            "note",
            "size",
            "enabled",
            "tint",
            "dropdown",
            "radio",
            "buttons",
            "icon buttons",
            "aim",
        ] {
            assert!(
                harness.query_by_label(label).is_some(),
                "`{label}` should render"
            );
        }
        // Only the styles that draw their options inline: a collapsed dropdown
        // holds its selection as a value rather than a label, and its options until it opens.
        for option in ["gamma", "delta", "epsilon", "zeta"] {
            assert!(
                harness.query_by_label(option).is_some(),
                "option `{option}` should render"
            );
        }
        for option in ["eta", "theta"] {
            assert!(
                harness.query_by_label(option).is_some(),
                "icon button option `{option}` should render"
            );
        }
    }

    #[test]
    fn icon_buttons_paint_their_svg_inside_the_button() {
        fn render(icon: Icon) -> image::RgbaImage {
            let mut knobs = vec![Knob::IconButtons {
                label: "theme".to_owned(),
                value: 0,
                options: vec!["Light".to_owned()],
                icons: vec![icon],
            }];
            let mut harness = egui_kittest::Harness::new_ui(move |ui| {
                render_knobs(ui, &mut knobs);
            });
            harness.run();
            harness.render().expect("the icon button renders")
        }

        let painted = render(Icon::from_svg(include_bytes!(
            "../template/assets/theme-light.svg"
        )));
        let empty = render(Icon::from_svg(
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24"/>"#,
        ));
        assert_ne!(
            painted.as_raw(),
            empty.as_raw(),
            "the supplied SVG geometry must change the button's pixels"
        );
    }

    #[test]
    fn catalog_and_scene_knob_grids_do_not_reuse_an_egui_id() {
        let slider = |label: &str| Knob::Slider {
            label: label.to_owned(),
            value: 0.0,
            min: 0.0,
            max: 1.0,
            step: 0.1,
        };
        let mut globals = vec![slider("Global slider")];
        let mut scene = vec![slider("Scene slider")];
        let harness = egui_kittest::Harness::new_ui(move |ui| {
            render_panel_globals(ui, &mut globals);
            ui.add_space(24.0);
            render_knobs(ui, &mut scene);
        });

        let grid_id_warnings: Vec<&str> = harness
            .output()
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) if text.galley.text().contains("use of Grid ID") => {
                    Some(text.galley.text())
                }
                _ => None,
            })
            .collect();
        assert!(
            grid_id_warnings.is_empty(),
            "global and scene grids must have distinct IDs: {grid_id_warnings:?}"
        );
    }

    #[test]
    fn repeated_raw_icon_labels_do_not_reuse_an_atom_id() {
        let icon = Icon::from_svg(include_bytes!("../assets/icons/app.svg"));
        let mut knobs = vec![Knob::IconButtons {
            label: "theme".to_owned(),
            value: 0,
            options: vec!["same".to_owned(), "same".to_owned()],
            icons: vec![icon.clone(), icon],
        }];
        let mut harness = egui_kittest::Harness::new_ui(move |ui| {
            render_global_toolbar(ui, &mut knobs);
        });
        harness.run();

        let id_warnings: Vec<&str> = harness
            .output()
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text)
                    if text.galley.text().contains("use of")
                        && text.galley.text().contains("ID") =>
                {
                    Some(text.galley.text())
                }
                _ => None,
            })
            .collect();
        assert!(
            id_warnings.is_empty(),
            "raw icon labels must still receive distinct atom IDs: {id_warnings:?}"
        );
    }

    #[test]
    fn clicking_an_action_button_sets_its_one_shot_event() {
        use std::{cell::RefCell, rc::Rc};

        use egui_kittest::kittest::Queryable;

        let clicked = Rc::new(RefCell::new(false));
        let observed = clicked.clone();
        let mut knob = Knob::Button {
            label: "Rebuild shaders".to_owned(),
            clicked: false,
        };
        let mut harness = egui_kittest::Harness::new_ui(move |ui| {
            render_knob(ui, &mut knob);
            if let Knob::Button { clicked, .. } = &knob {
                *observed.borrow_mut() = *clicked;
            }
        });

        harness.get_by_label("Rebuild shaders").click();
        harness.step();

        assert!(*clicked.borrow());
    }

    #[test]
    fn a_scene_with_no_knobs_says_so_rather_than_drawing_an_empty_grid() {
        use egui_kittest::kittest::Queryable;

        let mut harness = egui_kittest::Harness::new_ui(|ui| {
            assert!(!render_knobs(ui, &mut []), "nothing to change");
        });
        harness.run();
        assert!(
            harness
                .query_by_label("This scene has no controls.")
                .is_some(),
            "the panel says why it is empty"
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
