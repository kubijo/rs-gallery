//! A task board across its whole stage, and a play that works it from corner to corner:
//! it searches, drags cards between columns, adds one through a dialog and archives.
//!
//! Recorded with `[record] glide` on, the pointer travels the board between its gestures.

use gallery::prelude::*;

scene_meta! { title: "Example / Tasks" }

const COLUMNS: [&str; 3] = ["To do", "Doing", "Done"];

#[derive(Clone)]
struct Board {
    search: String,
    columns: [Vec<String>; 3],
    adding: Option<String>,
}

impl Default for Board {
    fn default() -> Self {
        let cards = |titles: &[&str]| titles.iter().map(|title| (*title).to_owned()).collect();
        Self {
            search: String::new(),
            columns: [
                cards(&["Record a demo", "Sketch the board", "Name the release"]),
                cards(&["Write the plan", "Pick the colours"]),
                cards(&["Set up the repo"]),
            ],
            adding: None,
        }
    }
}

/// A card being dragged: its column and its place in it.
#[derive(Clone, Copy)]
struct Dragged {
    column: usize,
    at: usize,
}

#[scene(default)]
fn tasks(ctx: &mut SceneCtx, ui: &mut Ui) {
    ctx.stage(ui, Stage::Fill.padding(0), |ui| {
        let id = ui.make_persistent_id("board-demo-state");
        let mut board = ui.data_mut(|data| data.get_temp_mut_or_default::<Board>(id).clone());

        ui.painter()
            .rect_filled(ui.max_rect(), 0.0, ui.visuals().panel_fill);
        let mut dropped = None;
        egui::Frame::new().inner_margin(16).show(ui, |ui| {
            toolbar(ui, &mut board);
            ui.add_space(12.0);
            ui.horizontal_top(|ui| {
                let gaps = ui.spacing().item_spacing.x * (COLUMNS.len() - 1) as f32;
                let width = ((ui.available_width() - gaps) / COLUMNS.len() as f32).floor();
                for column in 0..COLUMNS.len() {
                    ui.vertical(|ui| {
                        ui.set_width(width);
                        if let Some(dragged) = lane(ui, &board, column) {
                            dropped = Some((dragged, column));
                        }
                    });
                }
            });
        });
        if let Some((Dragged { column, at }, to)) = dropped
            && column != to
        {
            let card = board.columns[column].remove(at);
            action(format!("moved {card} to {}", COLUMNS[to]));
            board.columns[to].push(card);
            // Known only after the lanes drew: drawing again shows the card where it went this frame.
            ui.ctx().request_discard("a card moved");
        }
        adding(ui, &mut board);

        ui.data_mut(|data| data.insert_temp(id, board));
    });
}

fn toolbar(ui: &mut Ui, board: &mut Board) {
    ui.horizontal(|ui| {
        let label = ui.label("Search");
        ui.add(egui::TextEdit::singleline(&mut board.search).desired_width(220.0))
            .labelled_by(label.id);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Archive done").clicked() {
                let done = std::mem::take(&mut board.columns[2]);
                action(format!("archived {}", done.len()));
            }
            if ui.button("New card").clicked() {
                board.adding = Some(String::new());
            }
        });
    });
}

/// One column, its cards matching the search; the card dropped on it, if one is.
fn lane(ui: &mut Ui, board: &Board, column: usize) -> Option<Dragged> {
    ui.heading(COLUMNS[column]);
    let search = board.search.to_lowercase();
    drop_zone(ui, &format!("{} cards", COLUMNS[column]), |ui| {
        ui.set_min_size(egui::vec2(ui.available_width(), 360.0));
        for (at, card) in board.columns[column].iter().enumerate() {
            if !card.to_lowercase().contains(&search) {
                continue;
            }
            let id = ui.id().with(("card", column, at));
            drag_source(ui, id, Dragged { column, at }, |ui| {
                egui::Frame::new()
                    .fill(ui.visuals().extreme_bg_color)
                    .corner_radius(6)
                    .inner_margin(10)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.add(egui::Label::new(card.as_str()).selectable(false));
                    });
            });
            ui.add_space(6.0);
        }
    })
}

/// [`Ui::dnd_drag_source`], but a dragged card keeps where it was grabbed under the pointer:
/// egui's jumps to centre itself on it.
fn drag_source(ui: &mut Ui, id: egui::Id, payload: Dragged, add_contents: impl FnOnce(&mut Ui)) {
    if !ui.ctx().is_being_dragged(id) {
        let card = ui.scope(add_contents).response;
        ui.interact(card.rect, id, egui::Sense::drag())
            .on_hover_cursor(egui::CursorIcon::Grab);
        return;
    }
    egui::DragAndDrop::set_payload(ui.ctx(), payload);
    let layer = egui::LayerId::new(egui::Order::Tooltip, id);
    ui.scope_builder(egui::UiBuilder::new().layer_id(layer), add_contents);
    let moved =
        ui.input(|input| Some(input.pointer.interact_pos()? - input.pointer.press_origin()?));
    if let Some(moved) = moved {
        ui.ctx()
            .transform_layer_shapes(layer, egui::emath::TSTransform::from_translation(moved));
    }
}

/// [`Ui::dnd_drop_zone`] for a card, named `label`, its colour easing in and out
/// as a card is dragged over it rather than switching at once.
fn drop_zone(ui: &mut Ui, label: &str, add_contents: impl FnOnce(&mut Ui)) -> Option<Dragged> {
    let mut frame = egui::Frame::group(ui.style()).inner_margin(8).begin(ui);
    add_contents(&mut frame.content_ui);
    let response = frame.allocate_space(ui);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, label));

    let over =
        egui::DragAndDrop::has_payload_of_type::<Dragged>(ui.ctx()) && response.contains_pointer();
    let eased = ui.ctx().animate_bool_with_time(response.id, over, 0.2);
    let (idle, lit) = (ui.visuals().widgets.inactive, ui.visuals().widgets.active);
    frame.frame.fill = idle.bg_fill.lerp_to_gamma(lit.bg_fill, eased);
    frame.frame.stroke = egui::Stroke::new(
        egui::lerp(idle.bg_stroke.width..=lit.bg_stroke.width, eased),
        idle.bg_stroke
            .color
            .lerp_to_gamma(lit.bg_stroke.color, eased),
    );
    frame.paint(ui);

    response
        .dnd_release_payload::<Dragged>()
        .map(|dragged| *dragged)
}

/// The new card's dialog, while it is open.
fn adding(ui: &Ui, board: &mut Board) {
    let Some(mut title) = board.adding.take() else {
        return;
    };
    let mut open = true;
    egui::Window::new("New card")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ui.ctx(), |ui| {
            let label = ui.label("Card title");
            ui.text_edit_singleline(&mut title).labelled_by(label.id);
            ui.horizontal(|ui| {
                if ui.button("Add").clicked() && !title.is_empty() {
                    action(format!("added {title}"));
                    board.columns[0].push(std::mem::take(&mut title));
                    open = false;
                }
                if ui.button("Cancel").clicked() {
                    open = false;
                }
            });
        });
    if open {
        board.adding = Some(title);
    }
}

#[play(tasks)]
fn tours_the_board(p: &mut Play) -> PlayResult {
    p.set_pace(Pace::showcase());
    p.step("find a card", |p| {
        p.click("Search")?;
        p.type_text("demo")?;
        p.expect("Sketch the board").to_be_absent()?;
        p.press("Cmd+A")?;
        p.press("Backspace")?;
        p.expect("Sketch the board").to_exist()
    })?;
    p.step("start the demo", |p| {
        move_card(p, "Record a demo", "Doing")?;
        p.expect_action("moved Record a demo to Doing")
    })?;
    p.step("finish the plan", |p| {
        move_card(p, "Write the plan", "Done")?;
        p.expect_action("moved Write the plan to Done")
    })?;
    p.step("add a card", |p| {
        p.click("New card")?;
        p.click("Card title")?;
        p.type_text("Glide the pointer")?;
        p.click("Add")?;
        p.expect_action("added Glide the pointer")
    })?;
    p.step("archive what is done", |p| {
        p.click("Archive done")?;
        p.expect_action("archived 2")?;
        p.expect("Write the plan").to_be_absent()
    })
}

/// Drag `card` onto the middle of the column headed `column`,
/// then let the column's highlight fade before the step's pause holds a frame.
fn move_card(p: &mut Play, card: &str, column: &str) -> PlayResult {
    let from = p.get(card).rect.center();
    let to = p.get(format!("{column} cards").as_str()).rect.center();
    p.drag(card, to - from)?;
    p.idle(15)
}
