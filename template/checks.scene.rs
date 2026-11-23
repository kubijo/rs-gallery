//! A sign-up form, and a play that checks it with every matcher a play has.
//! The reference for what each one reads: record it to watch them pass in turn.
//!
//! egui's own widgets report placeholder, focus, disabled, toggled and value;
//! a description, expanded and hidden come from the scene, as a custom widget
//! of yours would give them, through `accesskit_node_builder`.

use gallery::{
    matchers::{gt, starts_with},
    prelude::*,
};

scene_meta! { title: "Example / Checks" }

#[derive(Clone, Copy, PartialEq)]
enum Plan {
    Free,
    Pro,
    Team,
}

impl Plan {
    fn name(self) -> &'static str {
        match self {
            Self::Free => "Free",
            Self::Pro => "Pro",
            Self::Team => "Team",
        }
    }
}

#[derive(Clone)]
struct Signup {
    name: String,
    plan: Plan,
    seats: f32,
    newsletter: bool,
    updates: bool,
    advanced: bool,
    tags: Vec<String>,
    created: bool,
}

impl Default for Signup {
    fn default() -> Self {
        Self {
            name: String::new(),
            plan: Plan::Free,
            seats: 1.0,
            newsletter: false,
            updates: false,
            advanced: false,
            tags: ["rust", "egui", "ui"].map(str::to_owned).to_vec(),
            created: false,
        }
    }
}

#[scene(default)]
fn checks(_ctx: &mut SceneCtx, ui: &mut Ui) {
    let id = ui.make_persistent_id("checks-demo-state");
    let mut form = ui.data_mut(|data| data.get_temp_mut_or_default::<Signup>(id).clone());

    ui.heading("Sign up");
    ui.horizontal(|ui| {
        let label = ui.label("Name");
        ui.add(egui::TextEdit::singleline(&mut form.name).hint_text("Your name"))
            .labelled_by(label.id);
    });
    let named = !form.name.trim().is_empty();
    let hint = ui.add_visible(!named, egui::Label::new("Name is required"));
    // `add_visible(false, …)` only stops the painting:
    // the hint stays in the tree, shown as ever.
    if named {
        ui.ctx()
            .accesskit_node_builder(hint.id, |node| node.set_hidden());
    }

    ui.separator();
    ui.horizontal(|ui| {
        for plan in [Plan::Free, Plan::Pro, Plan::Team] {
            ui.selectable_value(&mut form.plan, plan, plan.name());
        }
    });
    ui.add(
        egui::Slider::new(&mut form.seats, 1.0..=20.0)
            .step_by(1.0)
            .text("Seats"),
    );
    ui.label(format!("{} plan · {} seats", form.plan.name(), form.seats));

    ui.separator();
    let all = form.newsletter && form.updates;
    let some = form.newsletter || form.updates;
    let mut every = all;
    if ui
        .add(egui::Checkbox::new(&mut every, "All extras").indeterminate(some && !all))
        .changed()
    {
        form.newsletter = every;
        form.updates = every;
    }
    ui.indent("extras", |ui| {
        ui.checkbox(&mut form.newsletter, "Newsletter");
        ui.checkbox(&mut form.updates, "Product updates");
    });

    ui.separator();
    let disclosure = ui.button("Advanced");
    if disclosure.clicked() {
        form.advanced = !form.advanced;
    }
    // egui's buttons report no expanded state; a disclosure widget says it itself.
    ui.ctx()
        .accesskit_node_builder(disclosure.id, |node| node.set_expanded(form.advanced));
    if form.advanced {
        ui.horizontal_wrapped(|ui| {
            let mut removed = None;
            for (at, tag) in form.tags.iter().enumerate() {
                ui.label(tag);
                if ui.small_button(format!("Remove {tag}")).clicked() {
                    removed = Some(at);
                }
            }
            if let Some(at) = removed {
                let tag = form.tags.remove(at);
                action(format!("removed {tag}"));
            }
        });
    }

    ui.separator();
    let create = ui.add_enabled(named, egui::Button::new("Create account"));
    ui.ctx().accesskit_node_builder(create.id, |node| {
        node.set_description(if named {
            "Creates the account"
        } else {
            "Needs a name first"
        });
    });
    if create.clicked() {
        form.created = true;
        action(format!("created {}", form.name.trim()));
    }
    if form.created {
        ui.label("Account created");
    }

    ui.data_mut(|data| data.insert_temp(id, form));
}

#[play(checks)]
fn checks_with_every_matcher(p: &mut Play) -> PlayResult {
    p.step("the form starts empty", |p| {
        p.expect("Name").to_have_placeholder("Your name")?;
        p.expect("Name").to_have_value("")?;
        p.expect("Name is required").to_be_visible()?;
        p.expect("Create account").to_be_disabled()?;
        p.expect("Create account")
            .because("a disabled button says what it waits for")
            .to_have_description("Needs a name first")
    })?;
    p.step("give it a name", |p| {
        p.click("Name")?;
        p.expect("Name").to_be_focused()?;
        p.type_text("Ada")?;
        p.expect("Name").to_have_value("Ada")?;
        p.expect("Name is required").to_be_hidden()?;
        p.expect("Create account").to_be_enabled()
    })?;
    p.step("choose a team plan", |p| {
        p.click("Team")?;
        p.expect("Team").to_be_selected()?;
        p.expect("Free").not().to_be_selected()?;
        p.drag(By::slider(), [20.0, 0.0])?;
        p.expect(By::slider()).to_have_numeric_value(gt(1.0))?;
        p.expect(By::new().label_contains(" seats"))
            .to_have_label(starts_with("Team plan"))
    })?;
    p.step("take one extra, then both", |p| {
        p.click("Newsletter")?;
        p.expect("All extras").to_be_partially_checked()?;
        p.click("Product updates")?;
        p.expect("All extras").to_be_checked()
    })?;
    p.step("trim the tags", |p| {
        p.expect("Advanced").not().to_be_expanded()?;
        p.click("Advanced")?;
        p.expect("Advanced").to_be_expanded()?;
        let tags = By::new().label_contains("Remove ");
        p.expect(tags.clone()).to_have_count(3)?;
        p.click("Remove ui")?;
        p.expect_action("removed ui")?;
        p.expect(tags).to_have_count(2)?;
        p.expect("Remove ui").to_be_absent()
    })?;
    p.step("create the account", |p| {
        p.click("Create account")?;
        p.expect_action("created Ada")?;
        p.expect("Account created").within(5).to_exist()?;
        p.expect_value(|p| p.get("Name").value.unwrap_or_default(), "Ada")
    })
}
