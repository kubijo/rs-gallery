//! Gallery-owned native-window chrome: title-bar controls and borderless resize hit targets.

use crate::{HAIRLINE, HEADER_BG, Icons, MUTED, PANEL_BG};

pub(crate) const TITLE_BAR_H: f32 = 28.0;
const ACTION_W: f32 = 32.0;
const ACTIONS_W: f32 = ACTION_W * 3.0;
const ICON_SIZE: f32 = 12.0;
const TITLE_ICON_SIZE: f32 = 16.0;
const TITLE_ICON_GAP: f32 = 6.0;
const EDGE_W: f32 = 6.0;
const CORNER_W: f32 = 12.0;
const DOUBLE_CLICK_DELAY: f64 = 0.3;
const DOUBLE_CLICK_DISTANCE: f32 = 6.0;
const CLOSE_HOVER: egui::Color32 = egui::Color32::from_rgb(0xC4, 0x2B, 0x1C);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Action {
    Drag,
    Minimize,
    ToggleMaximize,
    Close,
}

#[derive(Clone, Copy, Default)]
struct TitleClick {
    time: f64,
    position: egui::Pos2,
}

/// Draw the window title and controls into a fixed-height top panel.
pub(crate) fn title_bar(
    ui: &mut egui::Ui,
    title: &str,
    maximized: bool,
    title_icon: Option<&egui::TextureHandle>,
    icons: &Icons,
) -> Option<Action> {
    let rect = egui::Rect::from_min_size(
        ui.max_rect().min,
        egui::vec2(ui.max_rect().width(), TITLE_BAR_H),
    );
    ui.advance_cursor_after_rect(rect);
    ui.painter().rect_filled(rect, 0.0, PANEL_BG);
    ui.painter().hline(
        rect.x_range(),
        rect.bottom(),
        egui::Stroke::new(1.0, HAIRLINE),
    );

    let controls = controls_rect(rect);
    let minimize_rect =
        egui::Rect::from_min_size(controls.min, egui::vec2(ACTION_W, rect.height()));
    let maximize_rect = minimize_rect.translate(egui::vec2(ACTION_W, 0.0));
    let close_rect = maximize_rect.translate(egui::vec2(ACTION_W, 0.0));

    let minimize = control(
        ui,
        minimize_rect,
        "gallery-window-minimize",
        "Minimize window",
        &icons.window_minimize,
        false,
    );
    let maximize = control(
        ui,
        maximize_rect,
        "gallery-window-maximize",
        if maximized {
            "Restore window"
        } else {
            "Maximize window"
        },
        if maximized {
            &icons.window_restore
        } else {
            &icons.window_maximize
        },
        false,
    );
    let close = control(
        ui,
        close_rect,
        "gallery-window-close",
        "Close window",
        &icons.window_close,
        true,
    );

    let drag_rect = egui::Rect::from_min_max(rect.min, egui::pos2(controls.left(), rect.bottom()));
    let drag = ui.interact(
        drag_rect,
        ui.make_persistent_id("gallery-window-drag"),
        egui::Sense::click_and_drag(),
    );
    drag.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "Window title bar")
    });
    let mut title_left = drag_rect.left() + 8.0;
    if let Some(icon) = title_icon {
        let icon_rect = egui::Rect::from_min_size(
            egui::pos2(title_left, drag_rect.center().y - TITLE_ICON_SIZE / 2.0),
            egui::Vec2::splat(TITLE_ICON_SIZE),
        );
        ui.painter().image(
            icon.id(),
            icon_rect,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::Pos2::new(1.0, 1.0)),
            egui::Color32::WHITE,
        );
        title_left = icon_rect.right() + TITLE_ICON_GAP;
    }
    ui.painter()
        .with_clip_rect(drag_rect.shrink2(egui::vec2(8.0, 0.0)))
        .text(
            egui::pos2(title_left, drag_rect.center().y),
            egui::Align2::LEFT_CENTER,
            title,
            egui::FontId::proportional(11.0),
            egui::Color32::WHITE,
        );

    if title_double_clicked(ui, &drag) {
        Some(Action::ToggleMaximize)
    } else if drag.drag_started_by(egui::PointerButton::Primary) {
        Some(Action::Drag)
    } else if minimize.clicked() {
        Some(Action::Minimize)
    } else if maximize.clicked() {
        Some(Action::ToggleMaximize)
    } else if close.clicked() {
        Some(Action::Close)
    } else {
        None
    }
}

/// Track title clicks ourselves: native dragging can prevent a backend from feeding egui the first
/// click of its built-in multi-click sequence, which was the original maximize-toggle failure.
fn title_double_clicked(ui: &egui::Ui, response: &egui::Response) -> bool {
    let (primary_clicked, pointer) = ui.input(|input| {
        (
            input.pointer.button_clicked(egui::PointerButton::Primary),
            input.pointer.interact_pos(),
        )
    });
    let clicked = primary_clicked && pointer.is_some_and(|pointer| response.rect.contains(pointer));
    let clicked_elsewhere = primary_clicked && !clicked;
    let key = egui::Id::new((ui.ctx().viewport_id(), "previous-title-click"));
    if clicked_elsewhere {
        ui.data_mut(|data| data.remove_temp::<TitleClick>(key));
        return false;
    }
    if !clicked {
        return false;
    }

    let (time, position) = ui.input(|input| {
        (
            input.time,
            input
                .pointer
                .interact_pos()
                .unwrap_or(response.rect.center()),
        )
    });
    let previous = ui.data_mut(|data| data.get_temp::<TitleClick>(key));
    let double = previous.is_some_and(|previous| {
        time - previous.time <= DOUBLE_CLICK_DELAY
            && position.distance(previous.position) <= DOUBLE_CLICK_DISTANCE
    });
    ui.data_mut(|data| {
        if double {
            data.remove_temp::<TitleClick>(key);
        } else {
            data.insert_temp(key, TitleClick { time, position });
        }
    });
    double
}

fn control(
    ui: &egui::Ui,
    rect: egui::Rect,
    id: &str,
    label: &str,
    icon: &crate::svg::Icon,
    danger: bool,
) -> egui::Response {
    let response = ui.interact(rect, ui.make_persistent_id(id), egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    let fill = if response.is_pointer_button_down_on() || response.hovered() {
        if danger { CLOSE_HOVER } else { HEADER_BG }
    } else {
        PANEL_BG
    };
    ui.painter().rect_filled(rect, 0.0, fill);
    icon.paint(
        ui.painter(),
        egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(ICON_SIZE)),
        if response.hovered() {
            egui::Color32::WHITE
        } else {
            MUTED
        },
    );
    response.on_hover_text(label)
}

/// Translate one chrome action into the native viewport operation it represents.
pub(crate) fn apply(ctx: &egui::Context, action: Option<Action>, maximized: bool) {
    let Some(action) = action else {
        return;
    };
    let command = match action {
        Action::Drag => egui::ViewportCommand::StartDrag,
        Action::Minimize => egui::ViewportCommand::Minimized(true),
        Action::ToggleMaximize => egui::ViewportCommand::Maximized(!maximized),
        Action::Close => egui::ViewportCommand::Close,
    };
    ctx.send_viewport_cmd(command);
}

pub(crate) fn state(ctx: &egui::Context, fallback_title: &str) -> (String, bool) {
    ctx.input(|input| {
        (
            input
                .viewport()
                .title
                .clone()
                .unwrap_or_else(|| fallback_title.to_owned()),
            input.viewport().maximized.unwrap_or(false),
        )
    })
}

/// Give a decorationless window compositor-managed resize handles.
#[cfg(not(target_os = "macos"))]
pub(crate) fn resize(ctx: &egui::Context) {
    let unavailable = ctx.input(|input| {
        input.viewport().maximized.unwrap_or(false) || input.viewport().fullscreen.unwrap_or(false)
    });
    if unavailable {
        return;
    }
    let Some(pointer) = ctx.pointer_hover_pos() else {
        return;
    };
    let bounds = ctx.viewport_rect();
    // The top-right corner belongs to Close, as it does in conventional desktop chrome.
    if controls_rect(bounds).contains(pointer) {
        return;
    }
    let Some((direction, cursor)) = resize_target(bounds, pointer) else {
        return;
    };
    ctx.set_cursor_icon(cursor);
    if ctx.input(|input| input.pointer.primary_pressed()) {
        ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(direction));
    }
}

/// Cocoa retains native borderless resize handling; its programmatic drag-resize API is unsupported.
#[cfg(target_os = "macos")]
pub(crate) fn resize(_ctx: &egui::Context) {}

fn controls_rect(bounds: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::pos2(
            (bounds.right() - ACTIONS_W).max(bounds.left()),
            bounds.top(),
        ),
        egui::pos2(
            bounds.right(),
            (bounds.top() + TITLE_BAR_H).min(bounds.bottom()),
        ),
    )
}

fn resize_target(
    bounds: egui::Rect,
    pointer: egui::Pos2,
) -> Option<(egui::ResizeDirection, egui::CursorIcon)> {
    let left = pointer.x <= bounds.left() + EDGE_W;
    let right = pointer.x >= bounds.right() - EDGE_W;
    let top = pointer.y <= bounds.top() + EDGE_W;
    let bottom = pointer.y >= bounds.bottom() - EDGE_W;
    let near_left = pointer.x <= bounds.left() + CORNER_W;
    let near_right = pointer.x >= bounds.right() - CORNER_W;
    let near_top = pointer.y <= bounds.top() + CORNER_W;
    let near_bottom = pointer.y >= bounds.bottom() - CORNER_W;
    match (near_left, near_right, near_top, near_bottom) {
        (true, _, true, _) => Some((
            egui::ResizeDirection::NorthWest,
            egui::CursorIcon::ResizeNorthWest,
        )),
        (_, true, true, _) => Some((
            egui::ResizeDirection::NorthEast,
            egui::CursorIcon::ResizeNorthEast,
        )),
        (true, _, _, true) => Some((
            egui::ResizeDirection::SouthWest,
            egui::CursorIcon::ResizeSouthWest,
        )),
        (_, true, _, true) => Some((
            egui::ResizeDirection::SouthEast,
            egui::CursorIcon::ResizeSouthEast,
        )),
        _ if top => Some((egui::ResizeDirection::North, egui::CursorIcon::ResizeNorth)),
        _ if bottom => Some((egui::ResizeDirection::South, egui::CursorIcon::ResizeSouth)),
        _ if left => Some((egui::ResizeDirection::West, egui::CursorIcon::ResizeWest)),
        _ if right => Some((egui::ResizeDirection::East, egui::CursorIcon::ResizeEast)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use egui_kittest::kittest::Queryable as _;

    use super::*;

    const BOUNDS: egui::Rect =
        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::Pos2::new(200.0, 120.0));

    #[test]
    fn corners_take_precedence_over_edges() {
        assert_eq!(
            resize_target(BOUNDS, egui::pos2(4.0, 8.0)),
            Some((
                egui::ResizeDirection::NorthWest,
                egui::CursorIcon::ResizeNorthWest
            ))
        );
        assert_eq!(
            resize_target(BOUNDS, egui::pos2(196.0, 112.0)),
            Some((
                egui::ResizeDirection::SouthEast,
                egui::CursorIcon::ResizeSouthEast
            ))
        );
    }

    #[test]
    fn center_is_not_a_resize_target() {
        assert_eq!(resize_target(BOUNDS, BOUNDS.center()), None);
    }

    #[test]
    fn controls_report_their_window_actions() {
        let actions = Rc::new(RefCell::new(Vec::new()));
        let observed = actions.clone();
        let icons = Icons::load();
        let mut harness = egui_kittest::Harness::builder()
            .with_step_dt(0.01)
            .build_ui(move |ui| {
                if let Some(action) = title_bar(ui, "gallery", false, None, &icons) {
                    observed.borrow_mut().push(action);
                }
            });

        for label in ["Minimize window", "Maximize window", "Close window"] {
            harness.get_by_label(label).click();
            harness.step();
        }

        assert_eq!(
            *actions.borrow(),
            vec![Action::Minimize, Action::ToggleMaximize, Action::Close]
        );
    }

    #[test]
    fn double_clicking_the_title_reports_toggle_maximize() {
        let actions = Rc::new(RefCell::new(Vec::new()));
        let observed = actions.clone();
        let icons = Icons::load();
        let mut harness = egui_kittest::Harness::builder()
            .with_step_dt(0.01)
            .build_ui(move |ui| {
                if let Some(action) = title_bar(ui, "gallery", false, None, &icons) {
                    observed.borrow_mut().push(action);
                }
            });

        harness.get_by_label("Window title bar").click();
        harness.step();
        harness.get_by_label("Window title bar").click();
        harness.step();

        assert!(
            actions.borrow().contains(&Action::ToggleMaximize),
            "observed actions: {:?}",
            actions.borrow()
        );
    }

    #[test]
    fn dragging_the_title_reports_native_drag() {
        let actions = Rc::new(RefCell::new(Vec::new()));
        let observed = actions.clone();
        let icons = Icons::load();
        let mut harness = egui_kittest::Harness::new_ui(move |ui| {
            if let Some(action) = title_bar(ui, "gallery", false, None, &icons) {
                observed.borrow_mut().push(action);
            }
        });

        let center = harness.get_by_label("Window title bar").rect().center();
        harness.drag_at(center);
        harness.hover_at(center + egui::vec2(10.0, 0.0));
        harness.step();

        assert!(actions.borrow().contains(&Action::Drag));
    }

    #[test]
    fn maximized_window_exposes_restore_instead_of_maximize() {
        let icons = Icons::load();
        let harness = egui_kittest::Harness::new_ui(move |ui| {
            title_bar(ui, "gallery", true, None, &icons);
        });

        harness.get_by_label("Restore window");
        assert!(harness.query_by_label("Maximize window").is_none());
    }

    #[test]
    fn title_icon_is_painted_only_when_supplied() {
        fn paints_icon(supplied: bool) -> bool {
            let texture_id = Rc::new(RefCell::new(None));
            let observed = texture_id.clone();
            let icons = Icons::load();
            let mut texture = None;
            let harness = egui_kittest::Harness::new_ui(move |ui| {
                let texture = texture.get_or_insert_with(|| {
                    ui.ctx().load_texture(
                        "consumer-window-icon",
                        egui::ColorImage::new([1, 1], vec![egui::Color32::WHITE]),
                        egui::TextureOptions::LINEAR,
                    )
                });
                *observed.borrow_mut() = Some(texture.id());
                title_bar(ui, "gallery", false, supplied.then_some(texture), &icons);
            });
            let texture_id = texture_id.borrow().expect("loaded icon texture");
            harness.output().shapes.iter().any(|shape| {
                matches!(
                    &shape.shape,
                    egui::Shape::Mesh(mesh) if mesh.texture_id == texture_id
                )
            })
        }

        assert!(!paints_icon(false));
        assert!(paints_icon(true));
    }
}
