//! A form whose save takes a moment, and a play that drives it.
//! What a play expects is waited for as the scene shows it, not counted out in frames.

use gallery::prelude::*;

scene_meta! { title: "Example / Form" }

/// How long a save takes, in the scene's own clock.
const SAVE_SECONDS: f64 = 1.0;

#[scene(default)]
fn form(_ctx: &mut SceneCtx, ui: &mut Ui) {
    let id = ui.make_persistent_id("form-demo-state");
    let (mut name, mut clicks, mut saving_since) = ui.data_mut(|data| {
        data.get_temp_mut_or_default::<(String, u32, Option<f64>)>(id)
            .clone()
    });

    let label = ui.label("Name");
    ui.text_edit_singleline(&mut name).labelled_by(label.id);
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui.button("Count").clicked() {
            clicks += 1;
            action(format!("counted {clicks}"));
        }
        ui.label(format!("Clicks: {clicks}"));
    });
    ui.add_space(8.0);
    if ui.button("Save").clicked() {
        saving_since = Some(ui.input(|input| input.time));
    }
    if let Some(since) = saving_since {
        if ui.input(|input| input.time) - since < SAVE_SECONDS {
            ui.label("Saving…");
            ui.ctx().request_repaint();
        } else {
            ui.label(format!("Saved {name}"));
        }
    }
    ui.data_mut(|data| data.insert_temp(id, (name, clicks, saving_since)));
}

#[play(form)]
fn saves_after_a_moment(p: &mut Play) -> PlayResult {
    p.fill("Name", "Ada")?;
    p.expect("Name").to_have_value("Ada")?;
    p.click("Count")?;
    p.click("Count")?;
    p.expect_action("counted 2")?;
    // A bare string is a label; `By` narrows when that is ambiguous.
    p.click(By::button().label("Save"))?;
    p.step("the save lands", |p| {
        // Waits until the save is done, a second of the scene's clock away.
        p.expect("Saved Ada").to_exist()?;
        p.expect("Saving…").to_be_absent()
    })
}
