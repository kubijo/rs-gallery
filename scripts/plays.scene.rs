//! A task list with state that lasts across frames, and plays that drive it, for `just demo::plays`.
//! Some plays pass and some fail on purpose, each in its own way.
//! Copied into the demo beside the template's scenes; not part of the template.

use gallery::{
    egui_wgpu,
    matchers::{gt, lt},
    prelude::*,
};

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
    p.press("Enter")
}

/// The label counting the tasks still open.
fn count_left() -> By {
    By::new().label_contains(" left")
}

/// Where a task's row sits from the top.
fn top(p: &Play, task: &str) -> f32 {
    p.get(task).rect.top()
}

#[play(todo)]
fn adds_completes_and_filters(p: &mut Play) -> PlayResult {
    add(p, "Write docs")?;
    p.fill("New task", "Ship it")?;
    p.click("Add")?;
    p.expect_action("added Ship it")?;
    p.click("Write docs")?;
    p.expect_action("completed Write docs")?;
    p.expect("Write docs").to_be_checked()?;
    p.expect(count_left()).to_have_label("1 left")?;
    p.click("Active")?;
    p.expect("Active").to_be_selected()?;
    p.step("only the open task shows", |p| {
        p.expect("Write docs").to_be_absent()?;
        p.expect("Ship it").not().to_be_checked()
    })
}

#[play(todo)]
fn sorts_by_name_from_the_menu(p: &mut Play) -> PlayResult {
    add(p, "Zebra")?;
    add(p, "Apple")?;
    p.click(By::combo_box())?;
    p.click("Name")?;
    p.step("Apple comes first", |p| {
        let zebra = top(p, "Zebra");
        p.expect_value(|p| top(p, "Apple"), lt(zebra))
    })
}

#[play(todo)]
fn drags_the_priority_up(p: &mut Play) -> PlayResult {
    p.drag(By::slider(), [80.0, 0.0])?;
    p.expect(By::slider()).to_have_numeric_value(gt(0.0))
}

#[play(todo)]
fn scrolls_to_the_end_of_a_long_list(p: &mut Play) -> PlayResult {
    p.click("Add 30 samples")?;
    p.expect("Sample 30")
        .because("rows off screen are not laid out")
        .to_be_absent()?;
    p.scroll("Sample 1", [0.0, -800.0])?;
    p.expect("Sample 30").to_exist()
}

#[play(todo)]
fn clears_done_tasks(p: &mut Play) -> PlayResult {
    add(p, "Write docs")?;
    add(p, "Ship it")?;
    p.click("Write docs")?;
    p.click("Clear done")?;
    p.expect_action("cleared 1")?;
    p.step("only the open task is left", |p| {
        p.expect(count_left()).to_have_label("1 left")?;
        p.expect("Write docs").to_be_absent()
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
        p.expect(count_left()).within(10).to_have_label("2 left")
    })
}

/// Fails: sorting by name puts Apple first, and the play asserts otherwise.
#[play(todo)]
fn asserts_the_wrong_order(p: &mut Play) -> PlayResult {
    add(p, "Zebra")?;
    add(p, "Apple")?;
    p.click(By::combo_box())?;
    p.click("Name")?;
    p.step("Zebra comes first", |p| {
        let apple = top(p, "Apple");
        p.set_wait_frames(10);
        p.expect_value(|p| top(p, "Zebra"), lt(apple))
    })
}

/// Fails: nothing in the scene ever says `Saved`.
#[play(todo)]
fn waits_for_a_save_that_never_comes(p: &mut Play) -> PlayResult {
    add(p, "Write docs")?;
    p.step("the list is saved", |p| {
        p.expect("Saved").within(10).to_exist()
    })
}

/// Fails: no reference was ever taken for this screenshot,
/// so the check leaves what it saw beside where one would go.
#[play(todo)]
fn compares_with_a_reference_never_taken(p: &mut Play) -> PlayResult {
    add(p, "Write docs")?;
    p.expect(Canvas).to_match_screenshot("one task")
}

/// Fails: its reference in `__snapshots__/` was altered on purpose (a brightened button,
/// a shifted label, a blur, a circle and a `v2`), so the diff shows each kind of change.
#[play(todo)]
fn differs_from_its_reference(p: &mut Play) -> PlayResult {
    add(p, "Write docs")?;
    p.expect(Canvas).to_match_screenshot("one task")
}

/// Passes, tolerated: its reference is the altered one above,
/// and its budget allows more pixels than differ.
#[play(todo)]
fn tolerates_what_its_budget_allows(p: &mut Play) -> PlayResult {
    add(p, "Write docs")?;
    p.expect(Canvas)
        .to_match_screenshot(Screenshot::named("one task").tolerance(5_000))
}

/// Fails: its reference in `__snapshots__/` was altered on purpose
/// (a button renamed, a filter chosen, a label gone), so the diff shows each.
#[play(todo)]
fn reads_other_than_its_reference(p: &mut Play) -> PlayResult {
    add(p, "Write docs")?;
    p.click("Write docs")?;
    p.expect(Canvas)
        .to_match_accesskit_snapshot("one task done")
}

/// Fails: a panic outside any step, which the scenes dylib's own `std` has to catch.
#[play(todo)]
fn panics_outside_a_step(p: &mut Play) -> PlayResult {
    p.get_by_label("Nowhere");
    Ok(())
}

/// Draws nothing, and panics at it when the environment says to.
struct Callback;

impl egui_wgpu::CallbackTrait for Callback {
    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        _pass: &mut egui_wgpu::wgpu::RenderPass<'static>,
        _resources: &egui_wgpu::CallbackResources,
    ) {
        if std::env::var_os("DEMO_CALLBACK_PANICS").is_some() {
            panic!("the callback panics");
        }
    }
}

/// Paints with a callback once its button was clicked.
#[scene("Calls back")]
fn calls_back(_ctx: &mut SceneCtx, ui: &mut Ui) {
    let id = ui.make_persistent_id("clicked");
    let mut clicked = ui.data_mut(|data| data.get_temp::<bool>(id).unwrap_or_default());
    clicked |= ui.button("Paint").clicked();
    ui.data_mut(|data| data.insert_temp(id, clicked));
    if clicked {
        ui.painter().add(egui_wgpu::Callback::new_paint_callback(
            ui.max_rect(),
            Callback,
        ));
    }
}

/// Passes, unless the callback is told to panic, which ends the process.
#[play(calls_back)]
fn paints_with_a_callback(p: &mut Play) -> PlayResult {
    p.click("Paint")
}

/// Has egui call it at the end of every frame once its button was clicked.
#[scene("Ends frames")]
fn ends_frames(_ctx: &mut SceneCtx, ui: &mut Ui) {
    if ui.button("Arm").clicked() {
        let panics = |_: &mut Ui| panic!("the frame's end panics");
        ui.ctx().on_end_pass("panics", std::sync::Arc::new(panics));
    }
}

/// Fails: the scene panics once it is drawn, where only the play can catch it.
#[play(ends_frames)]
fn panics_at_a_frames_end(p: &mut Play) -> PlayResult {
    p.click("Arm")
}

/// Fails: the scene itself panics, which its `smoke::`
/// test reports as that scene's failure alone.
#[scene("Panics")]
fn panics(_ctx: &mut SceneCtx, _ui: &mut Ui) {
    panic!("this scene panics on every frame");
}
