//! Matchers: what `p.expect(…)` and its kin check,
//! how long they wait, and what a failure says.

use super::*;
use crate::matchers::{contains_substring, gt};

/// Five frames after `Start` is clicked, `Waiting` gives way to `Late`,
/// which the scene also reports as an action.
const LATE_AFTER: u64 = 5;

#[derive(Clone, Default)]
struct Form {
    name: String,
    remember: bool,
    level: f32,
    started: Option<u64>,
    reported: bool,
}

/// A field, two checkboxes (one standing for several), a slider, two selectable labels,
/// and a label that turns up a few frames after `Start` is clicked.
fn widgets(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
    let id = ui.make_persistent_id("matchers-form");
    let mut form = ui.data_mut(|data| data.get_temp_mut_or_default::<Form>(id).clone());
    let label = ui.label("Name");
    ui.text_edit_singleline(&mut form.name)
        .labelled_by(label.id);
    ui.checkbox(&mut form.remember, "Remember");
    let mut some = false;
    ui.add(egui::Checkbox::new(&mut some, "Some").indeterminate(true));
    ui.add(egui::Slider::new(&mut form.level, 0.0..=10.0).text("Level"));
    _ = ui.selectable_label(true, "Chosen");
    _ = ui.selectable_label(false, "Other");
    let now = ui.ctx().cumulative_pass_nr();
    if ui.button("Start").clicked() {
        form.started = Some(now);
    }
    match form.started {
        Some(started) if now >= started + LATE_AFTER => {
            ui.label("Late");
            if !std::mem::replace(&mut form.reported, true) {
                crate::action("late");
            }
        }
        Some(_) => {
            ui.label("Waiting");
            ui.ctx().request_repaint();
        }
        None => {}
    }
    ui.data_mut(|data| data.insert_temp(id, form));
}

/// The shot a passing one-shot capture wrote, as its report lists it.
fn passed_shot(scene: SceneFn, name: &'static str, run: PlayFn) -> serde_json::Value {
    let (manifest, plays) = catalog(scene, &[(name, run)]);
    let dir = scratch(name);
    let report = dir.join("capture.json");
    capture(
        &manifest,
        &plays,
        shot(
            "plays::scene",
            &dir.join("shot.png"),
            name,
            Trace::RetainOnFailure,
        ),
        Some(report.clone()),
    )
    .unwrap_or_else(|failure| panic!("the play passes: {}", failure.plain()));
    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report).expect("the report")).expect("JSON");
    json["shots"][0].clone()
}

#[test]
fn a_matcher_waits_for_what_it_expects_without_a_wait_for() {
    fn late(p: &mut Play<'_>) -> PlayResult {
        p.click("Start")?;
        p.expect("Late").to_exist()?;
        p.expect("Waiting").to_be_absent()
    }
    passed_shot(widgets, "late", late);
}

#[test]
fn a_check_that_already_holds_draws_no_frames() {
    fn present(p: &mut Play<'_>) -> PlayResult {
        p.expect("Name").to_exist()?;
        p.expect("Remember").not().to_be_checked()
    }
    let shot = passed_shot(widgets, "present", present);
    assert_eq!(shot["play"]["frames"], 0, "{shot}");
}

#[test]
fn a_failed_matcher_reports_what_it_expected_and_what_it_found() {
    fn misnamed(p: &mut Play<'_>) -> PlayResult {
        p.fill("Name", "Adam")?;
        p.expect("Name").within(2).to_have_value("Ada")
    }
    let failed = failed_shot(widgets, "misnamed", misnamed);
    assert_eq!(
        reason(&failed),
        r#"expected "Name" value is equal to "Ada", got "Adam""#,
        "{failed}"
    );
}

#[test]
fn the_report_logs_every_step_gesture_and_check_in_the_order_they_ran() {
    fn toured(p: &mut Play<'_>) -> PlayResult {
        p.step("name it", |p| {
            p.fill("Name", "Ada")?;
            p.expect("Name").to_have_value("Ada")
        })?;
        p.click("Remember")?;
        p.expect("Remember").to_be_checked()
    }
    let shot = passed_shot(widgets, "toured", toured);
    let log = shot["play"]["log"].as_array().expect("a log");
    let kinds: Vec<_> = log
        .iter()
        .map(|entry| {
            (
                entry["kind"].as_str().unwrap_or("?"),
                entry["depth"].as_u64(),
            )
        })
        .collect();
    assert_eq!(
        kinds,
        [
            ("step", Some(0)),
            ("gesture", Some(1)),
            ("check", Some(1)),
            ("gesture", Some(0)),
            ("check", Some(0)),
        ],
        "{shot}"
    );
    let check = &log[2];
    assert_eq!(check["subject"], "\"Name\"");
    assert_eq!(check["matcher"], "to_have_value");
    assert_eq!(check["expected"], "value is equal to \"Ada\"");
    assert_eq!(check["actual"], "\"Ada\"");
    assert_eq!(check["outcome"], "passed");
    assert!(
        check["at"]
            .as_str()
            .is_some_and(|at| at.contains("matchers.rs")),
        "{check}"
    );
    assert_eq!(
        shot["play"]["steps"].as_array().map(Vec::len),
        Some(2),
        "the step, and the click outside it: {shot}"
    );
}

#[test]
fn a_failed_check_is_in_the_report_beside_the_one_reason() {
    fn misnamed(p: &mut Play<'_>) -> PlayResult {
        p.fill("Name", "Adam")?;
        p.expect("Name").within(2).to_have_value("Ada")
    }
    let failed = failed_shot(widgets, "misnamed-report", misnamed);
    assert_eq!(failed["check"]["matcher"], "to_have_value", "{failed}");
    assert_eq!(failed["check"]["actual"], "\"Adam\"", "{failed}");
    assert_eq!(
        failed.to_string().matches("\"reason\":").count(),
        1,
        "only the failure's own reason is named so: {failed}"
    );
}

#[test]
fn a_failed_checks_panel_says_what_was_expected_found_and_where() {
    fn misnamed(p: &mut Play<'_>) -> PlayResult {
        p.fill("Name", "Adam")?;
        p.expect("Name")
            .because("the field keeps what was typed")
            .within(2)
            .to_have_value("Ada")
    }
    let (manifest, plays) = catalog(widgets, &[("misnamed-panel", misnamed)]);
    let dir = scratch("misnamed-panel");
    let panel = capture(
        &manifest,
        &plays,
        shot(
            "plays::scene",
            &dir.join("shot.png"),
            "misnamed-panel",
            Trace::RetainOnFailure,
        ),
        None,
    )
    .expect_err("the play fails")
    .plain();
    for line in [
        "the field keeps what was typed",
        r#"expected "Name" value is equal to "Ada""#,
        r#"found "Adam""#,
        "expected here",
    ] {
        assert!(panel.contains(line), "`{line}` in:\n{panel}");
    }
}

#[test]
fn a_negated_matcher_waits_for_the_state_to_go() {
    fn gone(p: &mut Play<'_>) -> PlayResult {
        p.click("Start")?;
        p.expect("Waiting").to_exist()?;
        p.expect("Waiting").not().to_exist()
    }
    passed_shot(widgets, "gone", gone);
}

#[test]
fn a_plain_value_and_a_googletest_matcher_are_both_accepted() {
    fn named(p: &mut Play<'_>) -> PlayResult {
        p.fill("Name", "Ada")?;
        p.expect("Name").to_have_value("Ada")?;
        p.expect("Name").to_have_value(String::from("Ada"))?;
        p.expect("Name").to_have_value(contains_substring("Ad"))?;
        p.expect(By::new().label_contains("e")).to_have_count(gt(2))
    }
    passed_shot(widgets, "named", named);
}

#[test]
fn a_matcher_on_two_widgets_fails_saying_how_many_matched() {
    fn either(p: &mut Play<'_>) -> PlayResult {
        p.expect(By::new().label_contains("resent"))
            .within(1)
            .to_be_enabled()
    }
    let failed = failed_shot(two_buttons, "either", either);
    assert!(reason(&failed).contains("2 widgets match"), "{failed}");
}

#[test]
fn a_matcher_on_a_missing_widget_lists_what_was_on_screen() {
    fn absent(p: &mut Play<'_>) -> PlayResult {
        p.expect("Absent").within(1).to_be_enabled()
    }
    let failed = failed_shot(two_buttons, "absent", absent);
    assert!(reason(&failed).contains("got nothing matched"), "{failed}");
    assert!(
        failed["on_screen"]
            .as_array()
            .is_some_and(|labels| labels.iter().any(|label| label == "Present")),
        "{failed}"
    );
}

#[test]
fn a_custom_message_leads_the_failure() {
    fn explained(p: &mut Play<'_>) -> PlayResult {
        p.expect("Remember")
            .because("remembering is on by default")
            .within(1)
            .to_be_checked()
    }
    let failed = failed_shot(widgets, "explained", explained);
    assert_eq!(reason(&failed), "remembering is on by default", "{failed}");
}

#[test]
fn a_checkbox_reads_as_checked_unchecked_or_mixed() {
    fn toggled(p: &mut Play<'_>) -> PlayResult {
        p.expect("Remember").not().to_be_checked()?;
        p.click("Remember")?;
        p.expect("Remember").to_be_checked()?;
        p.expect("Some").to_be_partially_checked()?;
        p.expect("Some").not().to_be_checked()
    }
    passed_shot(widgets, "toggled", toggled);
}

#[test]
fn a_selected_selectable_label_reads_as_selected() {
    fn chosen(p: &mut Play<'_>) -> PlayResult {
        p.expect("Chosen").to_be_selected()?;
        p.expect("Other").not().to_be_selected()
    }
    passed_shot(widgets, "chosen", chosen);
}

#[test]
fn a_slider_is_read_by_its_number() {
    fn raised(p: &mut Play<'_>) -> PlayResult {
        p.expect(By::slider()).to_have_numeric_value(0.0)?;
        p.drag(By::slider(), [40.0, 0.0])?;
        p.expect(By::slider()).to_have_numeric_value(gt(0.0))
    }
    passed_shot(widgets, "raised", raised);
}

#[test]
fn expect_value_polls_until_the_value_matches_and_reports_the_last_it_saw() {
    fn polled(p: &mut Play<'_>) -> PlayResult {
        p.click("Start")?;
        p.expect_value(|p| p.query_by_label("Late").is_some(), true)?;
        p.set_wait_frames(2);
        p.expect_value(|p| p.actions().len(), 2)
    }
    let failed = failed_shot(widgets, "polled", polled);
    assert_eq!(
        reason(&failed),
        "expected value is equal to 2, got 1",
        "{failed}"
    );
}

#[test]
fn a_panic_while_polling_counts_as_not_yet_and_is_reported_if_it_lasts() {
    fn missing(p: &mut Play<'_>) -> PlayResult {
        p.set_wait_frames(2);
        p.expect_value(|p| p.get("Absent").rect.top(), gt(0.0))
    }
    let failed = failed_shot(widgets, "missing-poll", missing);
    assert!(
        reason(&failed).contains("nothing matches \"Absent\""),
        "{failed}"
    );
}

#[test]
fn expect_action_waits_for_an_action_reported_frames_later() {
    fn reported(p: &mut Play<'_>) -> PlayResult {
        p.click("Start")?;
        p.expect_action("late")?;
        p.expect_action(contains_substring("at"))
    }
    passed_shot(widgets, "reported", reported);
}
