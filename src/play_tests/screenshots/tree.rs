//! AccessKit snapshots: how a tree reads, of the canvas
//! or of one widget, and what is told of a change in it.

use indoc::indoc;
use serde_json::{Value, json};

use super::*;

const REFERENCE: &str = "saved.accesskit.json";
const NEW: &str = "saved.accesskit.new.json";
const DIFF: &str = "saved.accesskit.diff.txt";

fn canvas(p: &mut Play<'_>) -> PlayResult {
    p.expect(Canvas).to_match_accesskit_snapshot("saved")
}

fn text(dir: &Utf8Path) -> String {
    std::fs::read_to_string(refs(dir).join(REFERENCE)).expect("the reference")
}

fn tree(dir: &Utf8Path) -> Value {
    serde_json::from_str(&text(dir)).expect("JSON")
}

#[test]
fn a_tree_reads_each_widget_with_its_value_and_states() {
    fn widgets(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
        ui.heading("Sign up");
        let label = ui.label("Name");
        ui.add(egui::TextEdit::singleline(&mut String::new()).hint_text("Your name"))
            .labelled_by(label.id);
        ui.text_edit_multiline(&mut "two\n\"lines\"".to_owned());
        ui.add(egui::Slider::new(&mut 0.1_f32, 0.0..=1.0).show_value(false));
        ui.checkbox(&mut true, "On");
        ui.checkbox(&mut false, "Off");
        ui.add(egui::Checkbox::new(&mut false, "Some").indeterminate(true));
        _ = ui.selectable_label(true, "Chosen");
        ui.horizontal(|ui| {
            ui.group(|ui| ui.add_enabled(false, egui::Button::new("Create")));
        });
        let more = ui.button("More");
        ui.ctx().accesskit_node_builder(more.id, |node| {
            node.set_expanded(false);
            node.set_selected(true);
            node.set_required();
            node.set_description("Shows the rest");
        });
        let gone = ui.label("Gone");
        ui.ctx()
            .accesskit_node_builder(gone.id, |node| node.set_hidden());
    }
    let dir = scratch("tree-reads");
    take(widgets, canvas, &dir);
    assert_eq!(
        tree(&dir),
        json!([
            { "role": "Label", "value": "Sign up" },
            { "role": "Label", "value": "Name" },
            { "role": "TextInput", "label": "Name", "value": "", "placeholder": "Your name" },
            { "role": "MultilineTextInput", "value": "two\n\"lines\"" },
            {
                "role": "Slider",
                "numeric_value": 0.1,
                "min_numeric_value": 0.0,
                "max_numeric_value": 1.0,
            },
            { "role": "CheckBox", "label": "On", "toggled": "True" },
            { "role": "CheckBox", "label": "Off", "toggled": "False" },
            { "role": "CheckBox", "label": "Some", "toggled": "Mixed" },
            { "role": "Button", "label": "Chosen", "toggled": "True" },
            { "role": "Button", "label": "Create", "disabled": true },
            {
                "role": "Button",
                "label": "More",
                "selected": true,
                "expanded": false,
                "required": true,
                "description": "Shows the rest",
            },
        ])
    );
    let again = judge(widgets, canvas, &dir, Snapshots::default());
    assert_eq!(again.actual(), "identical");
    assert_eq!(again.frames, 0);
}

#[test]
fn what_groups_others_is_passed_over_only_when_it_says_nothing_itself() {
    fn node(ui: &mut egui::Ui, set: impl FnOnce(&mut egui::accesskit::Node)) {
        let (_, response) = ui.allocate_exact_size(
            egui::vec2(20.0, 20.0),
            egui::Sense::focusable_noninteractive(),
        );
        ui.ctx().accesskit_node_builder(response.id, set);
    }
    fn groups(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
        use egui::accesskit::{Node, Role, Toggled};
        ui.group(|ui| ui.horizontal(|ui| _ = ui.button("Grouped")));
        let nothing_said: [fn(&mut Node); 4] = [
            |_| {},
            Node::set_disabled,
            |node| node.set_numeric_value(f64::NAN),
            |node| node.set_description(""),
        ];
        let one_thing_said: [fn(&mut Node); 11] = [
            |node| node.set_label("a label"),
            |node| node.set_value("a value"),
            |node| node.set_numeric_value(0.5),
            |node| node.set_min_numeric_value(0.0),
            |node| node.set_max_numeric_value(1.0),
            |node| node.set_toggled(Toggled::False),
            |node| node.set_selected(false),
            |node| node.set_expanded(false),
            Node::set_read_only,
            Node::set_required,
            |node| node.set_description("a description"),
        ];
        for say in nothing_said {
            for role in [
                Role::Unknown,
                Role::GenericContainer,
                Role::Splitter,
                Role::ScrollBar,
            ] {
                node(ui, |node| {
                    node.set_role(role);
                    say(node);
                });
            }
        }
        for say in one_thing_said {
            node(ui, |node| {
                node.set_role(Role::Unknown);
                say(node);
            });
        }
        for role in [Role::GenericContainer, Role::Splitter, Role::ScrollBar] {
            node(ui, |node| {
                node.set_role(role);
                node.set_numeric_value(0.5);
            });
        }
    }
    let dir = scratch("tree-groups");
    take(groups, canvas, &dir);
    assert_eq!(
        tree(&dir),
        json!([
            { "role": "Button", "label": "Grouped" },
            { "label": "a label" },
            { "value": "a value" },
            { "numeric_value": 0.5 },
            { "min_numeric_value": 0.0 },
            { "max_numeric_value": 1.0 },
            { "toggled": "False" },
            { "selected": false },
            { "expanded": false },
            { "read_only": true },
            { "required": true },
            { "description": "a description" },
            { "role": "GenericContainer", "numeric_value": 0.5 },
            { "role": "Splitter", "numeric_value": 0.5 },
            { "role": "ScrollBar", "numeric_value": 0.5 },
        ])
    );
}

#[test]
fn a_subject_is_told_though_it_would_be_passed_over_inside_another() {
    fn splitter(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
        let (_, response) = ui.allocate_exact_size(egui::Vec2::splat(20.0), egui::Sense::hover());
        ui.ctx().accesskit_node_builder(response.id, |node| {
            node.set_role(egui::accesskit::Role::Splitter);
        });
    }
    fn only_it(p: &mut Play<'_>) -> PlayResult {
        p.expect(By::new().role(egui::accesskit::Role::Splitter))
            .to_match_accesskit_snapshot("saved")
    }
    let dir = scratch("tree-subject");
    take(splitter, only_it, &dir);
    assert_eq!(tree(&dir), json!([{ "role": "Splitter" }]));
}

/// Two cards to click, drawn and no more, and which has the focus.
fn cards(ui: &mut egui::Ui, focused: &str) {
    for name in ["one", "two"] {
        let card = ui.scope_builder(
            egui::UiBuilder::new()
                .id(egui::Id::new(name))
                .sense(egui::Sense::click()),
            |ui| ui.label(name),
        );
        if name == focused {
            card.response.request_focus();
        }
    }
}

fn first_card(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
    cards(ui, "one");
}

fn second_card(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
    cards(ui, "two");
}

#[test]
fn focus_is_told_wherever_it_is() {
    let dir = scratch("tree-focus");
    take(first_card, canvas, &dir);
    assert!(text(&dir).contains("\"focused\": true"), "{}", text(&dir));
    assert!(!judge(second_card, canvas, &dir, Snapshots::default()).passed);
}

#[test]
fn a_field_reads_with_what_was_typed_and_its_focus() {
    fn field(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
        let id = ui.make_persistent_id("field");
        let mut name = ui.data_mut(|data| data.get_temp_mut_or_default::<String>(id).clone());
        let label = ui.label("Name");
        ui.text_edit_singleline(&mut name).labelled_by(label.id);
        ui.data_mut(|data| data.insert_temp(id, name));
    }
    fn types(p: &mut Play<'_>) -> PlayResult {
        p.fill("Name", "Ada")?;
        p.expect(Canvas).to_match_accesskit_snapshot("saved")
    }
    let dir = scratch("tree-field");
    take(field, types, &dir);
    assert_eq!(
        tree(&dir),
        json!([
            { "role": "Label", "value": "Name" },
            { "role": "TextInput", "label": "Name", "value": "Ada", "focused": true },
        ])
    );
}

/// A window over `label`, with a button called `button` in it.
fn windowed(ui: &mut egui::Ui, label: &str, button: &str) {
    ui.label(label);
    egui::Window::new("Aside").show(ui.ctx(), |ui| {
        ui.label("Sure?");
        _ = ui.button(button);
    });
}

fn sure(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
    windowed(ui, "Under", "Yes");
}

fn sure_over_another(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
    windowed(ui, "Underneath", "Yes");
}

fn unsure(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
    windowed(ui, "Under", "No");
}

#[test]
fn a_widgets_snapshot_holds_what_is_inside_it_and_nothing_else() {
    fn window(p: &mut Play<'_>) -> PlayResult {
        p.expect("Aside").to_match_accesskit_snapshot("saved")
    }
    let dir = scratch("tree-widget");
    take(sure, window, &dir);
    assert_eq!(
        text(&dir),
        indoc! {r#"
            [
              {
                "role": "Window",
                "label": "Aside",
                "children": [
                  {
                    "role": "Button",
                    "label": "Hide"
                  },
                  {
                    "role": "Label",
                    "value": "Sure?"
                  },
                  {
                    "role": "Button",
                    "label": "Yes"
                  }
                ]
              }
            ]
        "#}
    );
    let elsewhere = judge(sure_over_another, window, &dir, Snapshots::default());
    assert!(elsewhere.passed, "{}", elsewhere.panel);
    assert!(!judge(unsure, window, &dir, Snapshots::default()).passed);
}

#[test]
fn a_change_is_shown_as_a_diff_in_the_panel_and_a_file() {
    let dir = scratch("tree-changed");
    take(sure, canvas, &dir);
    let judged = judge(unsure, canvas, &dir, Snapshots::default());
    assert!(!judged.passed);
    assert_eq!(judged.actual(), "1 line removed, 1 added");
    let kept = judged.check.kept.as_ref().expect("a snapshot check");
    let shown = kept.diff.as_deref().expect("the diff");
    let words: Vec<String> = shown
        .lines()
        .map(|line| format!("{}{}", &line[..1], line[1..].trim_start()))
        .collect();
    assert_eq!(
        words,
        [
            " },",
            " {",
            " \"role\": \"Button\",",
            "-\"label\": \"Yes\"",
            "+\"label\": \"No\"",
            " }",
            " ]",
            " }",
        ]
    );
    for line in ["\"label\": \"Yes\"", "\"label\": \"No\""] {
        assert!(judged.panel.contains(line), "{line} in:\n{}", judged.panel);
    }
    assert_eq!(judged.attachments(), ["reference", "actual", "diff"]);
    assert_eq!(files(&dir), [DIFF, REFERENCE, NEW]);
    let file = std::fs::read_to_string(refs(&dir).join(DIFF)).expect("the diff");
    assert_eq!(file, shown);

    let taken = judge(unsure, canvas, &dir, updating(Update::Failing));
    let kept = taken.check.kept.expect("a snapshot check");
    assert_eq!(kept.diff, None, "a change taken is no difference left");
    assert_eq!(files(&dir), [REFERENCE]);
    let again = judge(unsure, canvas, &dir, Snapshots::default());
    assert_eq!(again.actual(), "identical");
}

#[test]
fn a_tree_still_changing_is_taken_once_it_settles() {
    fn counting(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
        let pass = ui.ctx().cumulative_pass_nr();
        if pass < SETTLES_AT {
            ui.label("Saving…");
            ui.ctx().request_repaint();
        } else {
            ui.label("Saved");
        }
    }
    let dir = scratch("tree-settles");
    take(counting, canvas, &dir);
    assert_eq!(tree(&dir), json!([{ "role": "Label", "value": "Saved" }]));
}

#[test]
fn a_scene_that_says_nothing_has_an_empty_tree() {
    fn silent(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
        block(ui, 100);
    }
    let dir = scratch("tree-empty");
    take(silent, canvas, &dir);
    assert_eq!(text(&dir), "[]\n");
    let again = judge(silent, canvas, &dir, Snapshots::default());
    assert_eq!(again.actual(), "identical");
}

#[test]
fn a_widget_that_is_not_there_not_alone_or_hidden_is_never_taken() {
    fn missing(p: &mut Play<'_>) -> PlayResult {
        p.expect("Nowhere")
            .within(4)
            .to_match_accesskit_snapshot("saved")
    }
    fn any_button(p: &mut Play<'_>) -> PlayResult {
        p.expect(By::button())
            .within(4)
            .to_match_accesskit_snapshot("saved")
    }
    fn gone(p: &mut Play<'_>) -> PlayResult {
        p.expect("Gone")
            .within(4)
            .to_match_accesskit_snapshot("saved")
    }
    fn hides(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
        let inside = ui.scope(|ui| ui.button("Gone"));
        ui.ctx()
            .accesskit_node_builder(inside.inner.id, |node| node.set_hidden());
    }
    let dir = scratch("tree-unmatched");
    let judged = judge(sure, missing, &dir, updating(Update::All));
    assert!(!judged.passed);
    assert_eq!(judged.check.actual, None);
    assert!(
        judged.panel.contains("found nothing matched") && judged.panel.contains("on screen"),
        "{}",
        judged.panel
    );
    assert_eq!(judged.frames, 4);
    let judged = judge(sure, any_button, &dir, updating(Update::All));
    assert_eq!(
        judged.actual(),
        "2 widgets match; narrow it with a role or an exact label"
    );
    let judged = judge(hides, gone, &dir, updating(Update::All));
    assert_eq!(judged.actual(), "hidden");
    assert_eq!(files(&dir), Vec::<String>::new());
}

#[test]
fn a_widget_gone_by_the_last_look_leaves_nothing_of_an_earlier_one() {
    /// The button is there until this many frames into the play.
    static LEFT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(u32::MAX);
    fn goes_away(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
        use std::sync::atomic::Ordering;
        let left = LEFT.load(Ordering::Relaxed);
        if left > 0 {
            _ = ui.button("Going");
        }
        if left != u32::MAX {
            LEFT.store(left.saturating_sub(1), Ordering::Relaxed);
        }
    }
    fn watches(p: &mut Play<'_>) -> PlayResult {
        LEFT.store(1, std::sync::atomic::Ordering::Relaxed);
        p.expect("Going")
            .within(2)
            .to_match_accesskit_snapshot("saved")
    }
    let dir = scratch("tree-going");
    let judged = judge(goes_away, watches, &dir, updating(Update::All));
    assert!(!judged.passed);
    assert_eq!(judged.check.actual, None, "it was gone at last");
    assert_eq!(files(&dir), Vec::<String>::new());
}

/// Labelled by a widget never drawn.
fn dangling(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
    ui.text_edit_singleline(&mut String::new())
        .labelled_by(egui::Id::new("nowhere"));
}

/// [`dangling`] until past the passes a shot draws before its play begins.
fn dangling_at_first(ctx: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
    if ui.ctx().cumulative_pass_nr() < SETTLES_AT {
        dangling(ctx, ui);
    }
    _ = ui.button("Save");
}

const DANGLES: &str = "labelled by one not drawn this frame";

#[test]
fn a_widget_labelled_by_one_not_drawn_fails_saying_so() {
    fn matches_a_widget(p: &mut Play<'_>) -> PlayResult {
        p.expect("Save").within(2).to_exist()
    }
    fn is_absent(p: &mut Play<'_>) -> PlayResult {
        p.expect("Save").within(2).to_be_absent()
    }
    fn clicks(p: &mut Play<'_>) -> PlayResult {
        p.click("Save")
    }
    fn gets(p: &mut Play<'_>) -> PlayResult {
        p.step("read", |p| {
            p.get("Save");
            Ok(())
        })
    }
    for run in [canvas as PlayFn, matches_a_widget, is_absent, clicks, gets] {
        let (manifest, plays) = catalog(dangling, &[("greets", run)]);
        let dir = scratch("tree-dangling");
        let snapshots = Snapshots {
            dir: Some(dir.join("refs")),
            ..updating(Update::All)
        };
        let shot = greeting_shot(&dir);
        let (outcome, _) = judged_shot(&manifest, &plays, Renderer::Glow, &shot, &snapshots);
        let panel = outcome.expect_err("it can't be read").plain();
        assert!(panel.contains(DANGLES), "{panel}");
        assert_eq!(files(&dir), Vec::<String>::new());
    }
}

#[test]
fn a_tree_not_readable_yet_is_waited_for() {
    fn waits(p: &mut Play<'_>) -> PlayResult {
        p.expect(Canvas).to_match_accesskit_snapshot("saved")
    }
    fn clicks(p: &mut Play<'_>) -> PlayResult {
        p.click("Save")?;
        waits(p)
    }
    fn checks(p: &mut Play<'_>) -> PlayResult {
        p.expect("Save").to_exist()?;
        waits(p)
    }
    for run in [waits as PlayFn, clicks, checks] {
        take(dangling_at_first, run, &scratch("tree-dangling-at-first"));
    }
}

#[test]
fn a_snapshot_has_no_opposite() {
    fn not(p: &mut Play<'_>) -> PlayResult {
        p.expect("Aside").not().to_match_accesskit_snapshot("saved")
    }
    let judged = judge(sure, not, &scratch("tree-not"), updating(Update::All));
    assert!(!judged.passed);
    assert!(
        judged.actual().contains("no opposite"),
        "{}",
        judged.actual()
    );
}

#[test]
fn a_reference_is_read_however_it_is_laid_out_and_refused_when_it_is_no_tree() {
    let dir = scratch("tree-laid-out");
    take(sure, canvas, &dir);
    let path = refs(&dir).join(REFERENCE);
    let written = text(&dir);
    for laid_out in [written.replace('\n', "\r\n"), tree(&dir).to_string()] {
        std::fs::write(&path, &laid_out).expect("written");
        let judged = judge(sure, canvas, &dir, Snapshots::default());
        assert_eq!(judged.actual(), "identical", "{laid_out}");
    }

    std::fs::write(&path, "version https://git-lfs").expect("written");
    let judged = judge(sure, canvas, &dir, Snapshots::default());
    assert!(
        judged.actual().starts_with("the reference can't be read: "),
        "{}",
        judged.actual()
    );
    take(sure, canvas, &dir);
    assert_eq!(text(&dir), written);
}

#[test]
fn a_tree_too_deep_to_parse_still_matches_itself() {
    fn nest(ui: &mut egui::Ui, depth: u32) {
        let inside = ui.scope(|ui| {
            if depth > 0 {
                nest(ui, depth - 1);
            }
        });
        let id = inside.response.id;
        ui.ctx().accesskit_node_builder(id, |node| {
            node.set_role(egui::accesskit::Role::TreeItem);
        });
    }
    fn deep(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
        nest(ui, 80);
    }
    let dir = scratch("tree-deep");
    take(deep, canvas, &dir);
    assert!(
        text(&dir).matches("TreeItem").count() > 70,
        "{}",
        text(&dir)
    );
    let again = judge(deep, canvas, &dir, Snapshots::default());
    assert_eq!(again.actual(), "identical");
    let other = judge(sure, canvas, &dir, Snapshots::default());
    assert!(
        other.actual().contains("recursion limit"),
        "{}",
        other.actual()
    );
}

#[test]
fn a_tree_may_share_a_name_with_a_screenshot() {
    fn both(p: &mut Play<'_>) -> PlayResult {
        p.expect(Canvas).to_match_screenshot("saved")?;
        p.expect(Canvas).to_match_accesskit_snapshot("saved")
    }
    let dir = scratch("tree-and-screenshot");
    take(sure, both, &dir);
    assert_eq!(files(&dir), [REFERENCE, "saved.glow.png"]);
}

#[test]
fn found_is_read_only_only_where_the_widget_says_so() {
    fn locked(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
        _ = ui.button("Free");
        let field = ui.button("Locked");
        ui.ctx()
            .accesskit_node_builder(field.id, |node| node.set_read_only());
    }
    fn reads(p: &mut Play<'_>) -> PlayResult {
        assert!(!p.get("Free").read_only);
        assert!(p.get("Locked").read_only);
        p.expect(Canvas).to_match_accesskit_snapshot("saved")
    }
    let dir = scratch("tree-read-only");
    take(locked, reads, &dir);
    assert_eq!(
        tree(&dir),
        json!([
            { "role": "Button", "label": "Free" },
            { "role": "Button", "label": "Locked", "read_only": true },
        ])
    );
}
