//! A task list with state that lasts across frames, and plays that drive it, for `just demo-plays`.
//! Some plays pass and some fail on purpose, each in its own way.
//! Copied into the demo beside the template's scenes; not part of the template.

use gallery::prelude::*;

scene_meta! { title: "Demo / Tasks" }

#[derive(Clone, Default)]
struct Tasks {
    draft: String,
    tasks: Vec<Task>,
    filter: Filter,
    sort: Sort,
    priority: f32,
}

#[derive(Clone)]
struct Task {
    name: String,
    done: bool,
}

#[derive(Clone, Copy, Default, PartialEq)]
enum Filter {
    #[default]
    All,
    Active,
    Done,
}

#[derive(Clone, Copy, Default, PartialEq)]
enum Sort {
    #[default]
    Added,
    Name,
}

#[scene(default)]
fn todo(_ctx: &mut SceneCtx, ui: &mut Ui) {
    let id = ui.make_persistent_id("tasks-demo-state");
    let mut state = ui.data_mut(|data| data.get_temp_mut_or_default::<Tasks>(id).clone());

    ui.heading("Tasks");
    ui.horizontal(|ui| {
        let label = ui.label("New task");
        let field = ui
            .text_edit_singleline(&mut state.draft)
            .labelled_by(label.id);
        let entered = field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        if (ui.button("Add").clicked() || entered) && !state.draft.trim().is_empty() {
            let name = std::mem::take(&mut state.draft).trim().to_owned();
            action(format!("added {name}"));
            state.tasks.push(Task { name, done: false });
        }
    });
    ui.horizontal(|ui| {
        if ui.button("Add 30 samples").clicked() {
            for n in 1..=30 {
                state.tasks.push(Task {
                    name: format!("Sample {n}"),
                    done: false,
                });
            }
            action("added 30 samples");
        }
        let done = state.tasks.iter().filter(|task| task.done).count();
        if ui.button("Clear done").clicked() {
            state.tasks.retain(|task| !task.done);
            action(format!("cleared {done}"));
        }
    });

    ui.horizontal(|ui| {
        for (filter, name) in [
            (Filter::All, "All"),
            (Filter::Active, "Active"),
            (Filter::Done, "Done"),
        ] {
            ui.selectable_value(&mut state.filter, filter, name);
        }
        ui.separator();
        egui::ComboBox::from_label("Sort by")
            .selected_text(match state.sort {
                Sort::Added => "Order added",
                Sort::Name => "Name",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut state.sort, Sort::Added, "Order added");
                ui.selectable_value(&mut state.sort, Sort::Name, "Name");
            });
    });
    ui.horizontal(|ui| {
        ui.add(egui::Slider::new(&mut state.priority, 0.0..=10.0).text("Priority"));
        ui.label(format!("priority {:.0}", state.priority));
    });

    let left = state.tasks.iter().filter(|task| !task.done).count();
    ui.label(format!("{left} left"));

    let mut shown: Vec<usize> = (0..state.tasks.len())
        .filter(|&at| match state.filter {
            Filter::All => true,
            Filter::Active => !state.tasks[at].done,
            Filter::Done => state.tasks[at].done,
        })
        .collect();
    if state.sort == Sort::Name {
        shown.sort_by(|&a, &b| state.tasks[a].name.cmp(&state.tasks[b].name));
    }
    let mut removed = None;
    let row = ui.spacing().interact_size.y;
    // Rows off screen are never laid out, so they are not in the tree until scrolled to.
    egui::ScrollArea::vertical()
        .max_height(120.0)
        .show_rows(ui, row, shown.len(), |ui, rows| {
            for &at in &shown[rows] {
                ui.horizontal(|ui| {
                    let task = &mut state.tasks[at];
                    if ui.checkbox(&mut task.done, task.name.clone()).changed() {
                        let verb = if task.done { "completed" } else { "reopened" };
                        action(format!("{verb} {}", task.name));
                    }
                    if ui.small_button("Remove").clicked() {
                        removed = Some(at);
                    }
                });
            }
        });
    if let Some(at) = removed {
        let task = state.tasks.remove(at);
        action(format!("removed {}", task.name));
    }

    ui.data_mut(|data| data.insert_temp(id, state));
}

fn add(p: &mut Play, name: &str) -> PlayResult {
    p.fill("New task", name)?;
    p.press(egui::Key::Enter)
}

/// The number at the end of the one label starting with `prefix`.
fn reading(p: &Play, prefix: &str) -> f32 {
    let label = p
        .get(By::new().label_contains(prefix))
        .label
        .unwrap_or_default();
    label[prefix.len()..].trim().parse().unwrap_or(f32::NAN)
}

#[play(todo)]
fn adds_completes_and_filters(p: &mut Play) -> PlayResult {
    add(p, "Write docs")?;
    p.fill("New task", "Ship it")?;
    p.click("Add")?;
    p.expect_action("added Ship it")?;
    p.click("Write docs")?;
    p.expect_action("completed Write docs")?;
    p.step("one task is left", |p| {
        p.get_by_label("1 left");
        Ok(())
    })?;
    p.click("Active")?;
    p.step("only the open task shows", |p| {
        assert!(
            p.query_by_label("Write docs").is_none(),
            "the done task is hidden"
        );
        assert!(p.query_by_label("Ship it").is_some(), "the open task shows");
        Ok(())
    })
}

#[play(todo)]
fn sorts_by_name_from_the_menu(p: &mut Play) -> PlayResult {
    add(p, "Zebra")?;
    add(p, "Apple")?;
    p.click(By::new().role(egui::accesskit::Role::ComboBox))?;
    p.click("Name")?;
    p.step("Apple comes first", |p| {
        let apple = p.get_by_label("Apple").rect.top();
        let zebra = p.get_by_label("Zebra").rect.top();
        assert!(apple < zebra, "Apple at {apple}, Zebra at {zebra}");
        Ok(())
    })
}

#[play(todo)]
fn drags_the_priority_up(p: &mut Play) -> PlayResult {
    p.drag(
        By::new().role(egui::accesskit::Role::Slider),
        egui::vec2(80.0, 0.0),
    )?;
    p.step("the priority rose", |p| {
        let priority = reading(p, "priority");
        assert!(priority > 0.0, "priority is {priority}");
        Ok(())
    })
}

#[play(todo)]
fn scrolls_to_the_end_of_a_long_list(p: &mut Play) -> PlayResult {
    p.click("Add 30 samples")?;
    p.step("the last sample starts off screen", |p| {
        assert!(
            p.query_by_label("Sample 30").is_none(),
            "it is not laid out yet"
        );
        Ok(())
    })?;
    p.scroll("Sample 1", egui::vec2(0.0, -800.0))?;
    p.step("the last sample comes into view", |p| {
        p.wait_for(|p| p.query_by_label("Sample 30").is_some())
    })
}

#[play(todo)]
fn clears_done_tasks(p: &mut Play) -> PlayResult {
    add(p, "Write docs")?;
    add(p, "Ship it")?;
    p.click("Write docs")?;
    p.click("Clear done")?;
    p.expect_action("cleared 1")?;
    p.step("only the open task is left", |p| {
        p.get_by_label("1 left");
        assert!(
            p.query_by_label("Write docs").is_none(),
            "the done task is gone"
        );
        Ok(())
    })
}

/// Fails: every row has a Remove button, so the label alone names two widgets.
#[play(todo)]
fn removes_by_an_ambiguous_label(p: &mut Play) -> PlayResult {
    add(p, "Write docs")?;
    add(p, "Ship it")?;
    p.click("Remove")
}

/// Fails: the scene reports `added Write docs`, not what the play expects.
#[play(todo)]
fn expects_the_wrong_action(p: &mut Play) -> PlayResult {
    add(p, "Write docs")?;
    p.expect_action("added Write the docs")
}

/// Fails: one of two tasks is done, so the count reads `1 left`.
#[play(todo)]
fn counts_the_wrong_number_left(p: &mut Play) -> PlayResult {
    add(p, "Write docs")?;
    add(p, "Ship it")?;
    p.click("Write docs")?;
    p.step("two tasks are left", |p| {
        p.get_by_label("2 left");
        Ok(())
    })
}

/// Fails: sorting by name puts Apple first, and the play asserts otherwise.
#[play(todo)]
fn asserts_the_wrong_order(p: &mut Play) -> PlayResult {
    add(p, "Zebra")?;
    add(p, "Apple")?;
    p.click(By::new().role(egui::accesskit::Role::ComboBox))?;
    p.click("Name")?;
    p.step("Zebra comes first", |p| {
        let apple = p.get_by_label("Apple").rect.top();
        let zebra = p.get_by_label("Zebra").rect.top();
        assert!(zebra < apple, "Zebra at {zebra}, Apple at {apple}");
        Ok(())
    })
}

/// Fails: nothing in the scene ever says `Saved`.
#[play(todo)]
fn waits_for_a_save_that_never_comes(p: &mut Play) -> PlayResult {
    add(p, "Write docs")?;
    p.set_wait_frames(10);
    p.step("the list is saved", |p| {
        p.wait_for(|p| p.query_by_label("Saved").is_some())
    })
}

/// Fails: a panic outside any step, which the scenes dylib's own `std` has to catch.
#[play(todo)]
fn panics_outside_a_step(p: &mut Play) -> PlayResult {
    p.get_by_label("Nowhere");
    Ok(())
}
