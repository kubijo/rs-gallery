//! Plays, run through a capture the way `--play` and a recipe's `play =` run them.

#![cfg(not(target_vendor = "apple"))]

use camino::{Utf8Path, Utf8PathBuf};

use crate::{
    By, Linked, Manifest, Play, PlayEntry, PlayError, PlayResult, Renderer, SceneCtx, SceneEntry,
    SceneKey, SceneSource,
    diagnostic::Diagnostic,
    render::{
        Capture, DEFAULT_SCALE, LogMode, Overrides, Shot, ShotPlay, Trace, render_with_globals,
    },
};

mod matchers;

/// A fresh directory of this test's own.
fn scratch(name: &str) -> Utf8PathBuf {
    let dir = Utf8PathBuf::from_path_buf(std::env::temp_dir())
        .expect("a UTF-8 temp dir")
        .join("gallery-play")
        .join(name);
    _ = std::fs::remove_dir_all(&dir);
    dir
}

fn shot(scene: &str, out: &Utf8Path, play: &str, trace: Trace) -> Shot {
    Shot {
        scene: scene.to_owned(),
        out: Some(out.to_owned()),
        size: egui::vec2(320.0, 240.0),
        scale: DEFAULT_SCALE,
        knobs: Overrides::default(),
        globals: Overrides::default(),
        frames: None,
        trim: true,
        settle: false,
        list: false,
        template: false,
        play: Some(ShotPlay {
            name: play.to_owned(),
            trace,
            steps: false,
        }),
        record: None,
    }
}

fn capture(
    manifest: &Manifest,
    plays: &[PlayEntry],
    shot: Shot,
    report: Option<Utf8PathBuf>,
) -> Result<(), Diagnostic> {
    render_with_globals(
        manifest,
        None,
        plays,
        Renderer::Glow,
        &|_: &egui::Context| {},
        &Capture {
            shots: vec![shot],
            sheet: None,
            report,
        },
        LogMode::Off,
    )
}

type PlayFn = fn(&mut Play<'_>) -> PlayResult;
type SceneFn = fn(&mut SceneCtx<'_>, &mut egui::Ui);

/// One scene and its plays, standing in for a scene file.
fn catalog(
    render: fn(&mut SceneCtx<'_>, &mut egui::Ui),
    plays: &[(&'static str, PlayFn)],
) -> (Manifest, Vec<PlayEntry>) {
    let manifest = Manifest {
        scenes: vec![SceneEntry {
            render,
            name: "scene",
            module_path: "plays",
            default: true,
            order: 0,
            source: "",
        }],
        groups: Vec::new(),
    };
    let plays = plays
        .iter()
        .map(|&(name, run)| PlayEntry {
            scene: SceneKey {
                module_path: "plays",
                name: "scene",
            },
            name,
            module_path: "plays",
            run,
        })
        .collect();
    (manifest, plays)
}

fn two_buttons(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
    _ = ui.button("Present");
    _ = ui.button("Also present");
}

fn trace_files(dir: &Utf8Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| entry.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

const FORM: &str = "gallery::scaffold_scenes::form::Form";
const KEYBOARD: &str = "gallery::scaffold_scenes::keyboard::Keyboard";

#[test]
fn the_form_play_saves_and_the_report_lists_its_steps() {
    let dir = scratch("form");
    let out = dir.join("form.png");
    let report = dir.join("capture.json");
    capture(
        &Linked.manifest(),
        &Linked.plays(),
        shot(FORM, &out, "saves_after_a_moment", Trace::On),
        Some(report.clone()),
    )
    .unwrap_or_else(|failure| panic!("{}", failure.plain()));

    assert!(out.exists(), "the post-play state was captured");
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report).expect("the report")).expect("JSON");
    let play = &report["shots"][0]["play"];
    assert_eq!(play["name"], "saves_after_a_moment");
    let labels: Vec<&str> = play["steps"]
        .as_array()
        .expect("steps")
        .iter()
        .map(|step| step["label"].as_str().expect("a label"))
        .collect();
    assert_eq!(
        labels,
        [
            r#"fill "Name""#,
            r#"click "Count""#,
            r#"click "Count""#,
            r#"click button "Save""#,
            "the save lands",
        ],
        "a gesture outside a step is a step of its own"
    );
    for step in play["steps"].as_array().expect("steps") {
        let trace = step["trace"]
            .as_str()
            .expect("`trace = on` keeps every step");
        assert!(Utf8Path::new(trace).exists(), "`{trace}` was written");
    }
}

#[test]
fn the_keyboard_plays_pass_and_keep_no_trace() {
    for play in ["tab_counts_while_focused", "typing_fills_a_field"] {
        let dir = scratch(&format!("keyboard-{play}"));
        let out = dir.join("keyboard.png");
        capture(
            &Linked.manifest(),
            &Linked.plays(),
            shot(KEYBOARD, &out, play, Trace::RetainOnFailure),
            None,
        )
        .unwrap_or_else(|failure| panic!("`{play}`: {}", failure.plain()));
        assert!(
            trace_files(&dir.join("keyboard.steps")).is_empty(),
            "a passing play keeps no trace unless asked"
        );
    }
}

#[test]
fn a_missing_widget_fails_its_step_naming_what_was_there_and_keeps_the_trace() {
    fn missing(p: &mut Play<'_>) -> PlayResult {
        p.set_wait_frames(2);
        p.step("press the missing button", |p| {
            p.click(By::new().label("Absent"))
        })
    }
    let (manifest, plays) = catalog(two_buttons, &[("missing", missing)]);
    let dir = scratch("missing");
    let failure = capture(
        &manifest,
        &plays,
        shot(
            "plays::scene",
            &dir.join("shot.png"),
            "missing",
            Trace::RetainOnFailure,
        ),
        None,
    )
    .expect_err("the play fails")
    .plain();

    assert!(
        failure.contains("step `press the missing button`"),
        "{failure}"
    );
    assert!(failure.contains("nothing matched"), "{failure}");
    assert!(
        failure.contains("`Present`"),
        "lists what was on screen: {failure}"
    );
    assert_eq!(
        trace_files(&dir.join("shot.steps")),
        ["01-press-the-missing-button.png"]
    );
}

#[test]
fn a_failed_play_reports_its_steps_reason_and_line_in_the_json() {
    fn missing(p: &mut Play<'_>) -> PlayResult {
        p.set_wait_frames(2);
        p.step("press the missing button", |p| p.click("Absent"))
    }
    let (manifest, plays) = catalog(two_buttons, &[("missing", missing)]);
    let dir = scratch("failed-report");
    let report = dir.join("capture.json");
    capture(
        &manifest,
        &plays,
        shot(
            "plays::scene",
            &dir.join("shot.png"),
            "missing",
            Trace::RetainOnFailure,
        ),
        Some(report.clone()),
    )
    .expect_err("the play fails");

    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report).expect("the report")).expect("JSON");
    assert_eq!(json["complete"], false);
    let failed = &json["failed_shots"][0];
    assert_eq!(failed["name"], "shot", "{json}");
    assert_eq!(failed["play"], "missing");
    assert_eq!(failed["failed_step"], "press the missing button");
    assert!(
        failed["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("nothing matched")),
        "{json}"
    );
    assert!(
        failed["on_screen"]
            .as_array()
            .is_some_and(|labels| labels.iter().any(|label| label == "Present")),
        "{json}"
    );
    assert!(
        failed["at"]
            .as_str()
            .is_some_and(|at| at.starts_with("src/play_tests.rs:")),
        "points at the play's own line: {json}"
    );
    let steps = failed["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 1, "{json}");
    assert!(
        steps[0]["trace"].is_string(),
        "the failure keeps its trace: {json}"
    );
}

#[test]
fn a_failed_play_does_not_stop_the_shots_after_it() {
    fn missing(p: &mut Play<'_>) -> PlayResult {
        p.set_wait_frames(2);
        p.click("Absent")
    }
    fn present(p: &mut Play<'_>) -> PlayResult {
        p.click("Present")
    }
    let (manifest, plays) = catalog(two_buttons, &[("missing", missing), ("present", present)]);
    let dir = scratch("keeps-going");
    let report = dir.join("capture.json");
    let failure = render_with_globals(
        &manifest,
        None,
        &plays,
        Renderer::Glow,
        &|_: &egui::Context| {},
        &Capture {
            shots: vec![
                shot(
                    "plays::scene",
                    &dir.join("first.png"),
                    "missing",
                    Trace::RetainOnFailure,
                ),
                shot(
                    "plays::scene",
                    &dir.join("second.png"),
                    "present",
                    Trace::RetainOnFailure,
                ),
            ],
            sheet: None,
            report: Some(report.clone()),
        },
        LogMode::Off,
    )
    .expect_err("one play failed")
    .plain();

    assert!(failure.contains("1 of 2 plays failed"), "{failure}");
    assert!(
        dir.join("second.png").exists(),
        "the shot after the failure still ran"
    );
    assert!(
        !dir.join("first.png").exists(),
        "the failed shot wrote no image"
    );
    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report).expect("the report")).expect("JSON");
    assert_eq!(json["shots"].as_array().map(Vec::len), Some(1), "{json}");
    assert_eq!(
        json["failed_shots"].as_array().map(Vec::len),
        Some(1),
        "{json}"
    );
    assert_eq!(json["failed_shots"][0]["name"], "first", "{json}");
}

#[test]
fn fill_replaces_what_a_field_already_holds() {
    fn prefilled(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
        let id = ui.make_persistent_id("prefilled");
        let mut text = ui.data_mut(|data| {
            data.get_temp_mut_or_insert_with(id, || "Bob".to_owned())
                .clone()
        });
        let label = ui.label("Name");
        ui.text_edit_singleline(&mut text).labelled_by(label.id);
        ui.data_mut(|data| data.insert_temp(id, text));
    }
    fn rename(p: &mut Play<'_>) -> PlayResult {
        p.fill("Name", "Ada")?;
        p.step("the field holds only the new name", |p| {
            assert_eq!(p.get_by_label("Name").value.as_deref(), Some("Ada"));
            Ok(())
        })?;
        p.fill("Name", "")?;
        p.step("filling nothing empties it", |p| {
            assert_eq!(p.get_by_label("Name").value.unwrap_or_default(), "");
            Ok(())
        })
    }
    let (manifest, plays) = catalog(prefilled, &[("rename", rename)]);
    let dir = scratch("fill-replaces");
    capture(
        &manifest,
        &plays,
        shot(
            "plays::scene",
            &dir.join("shot.png"),
            "rename",
            Trace::RetainOnFailure,
        ),
        None,
    )
    .unwrap_or_else(|failure| panic!("{}", failure.plain()));
}

#[test]
fn a_locator_matching_two_widgets_fails_rather_than_picking_one() {
    fn ambiguous(p: &mut Play<'_>) -> PlayResult {
        p.set_wait_frames(3);
        p.click(By::new().label_contains("resent"))
    }
    let (manifest, plays) = catalog(two_buttons, &[("ambiguous", ambiguous)]);
    let dir = scratch("ambiguous");
    let failure = capture(
        &manifest,
        &plays,
        shot(
            "plays::scene",
            &dir.join("shot.png"),
            "ambiguous",
            Trace::RetainOnFailure,
        ),
        None,
    )
    .expect_err("the play fails")
    .plain();
    assert!(failure.contains("2 widgets still match"), "{failure}");
}

#[test]
fn a_panic_in_a_step_fails_that_step() {
    fn panics(p: &mut Play<'_>) -> PlayResult {
        p.step("asserts", |_| panic!("boom"))
    }
    let (manifest, plays) = catalog(two_buttons, &[("panics", panics)]);
    let dir = scratch("panics");
    let failure = capture(
        &manifest,
        &plays,
        shot(
            "plays::scene",
            &dir.join("shot.png"),
            "panics",
            Trace::RetainOnFailure,
        ),
        None,
    )
    .expect_err("the play fails")
    .plain();
    assert!(failure.contains("failed at step `asserts`"), "{failure}");
    assert!(
        failure.contains("╰─▶ boom"),
        "the panic is the cause: {failure}"
    );
}

#[test]
fn wait_for_gives_up_after_its_budget() {
    fn never(p: &mut Play<'_>) -> PlayResult {
        p.set_wait_frames(3);
        p.wait_for(|p| p.query_by_label("Never").is_some())
    }
    let (manifest, plays) = catalog(two_buttons, &[("never", never)]);
    let dir = scratch("never");
    let failure = capture(
        &manifest,
        &plays,
        shot(
            "plays::scene",
            &dir.join("shot.png"),
            "never",
            Trace::RetainOnFailure,
        ),
        None,
    )
    .expect_err("the play fails")
    .plain();
    assert!(
        failure.contains("still waiting after 3 frames"),
        "{failure}"
    );
}

#[test]
fn an_unknown_play_names_the_ones_there_are() {
    fn nothing(_: &mut Play<'_>) -> PlayResult {
        Ok(())
    }
    let (manifest, plays) = catalog(two_buttons, &[("nothing", nothing)]);
    let dir = scratch("unknown");
    let failure = capture(
        &manifest,
        &plays,
        shot(
            "plays::scene",
            &dir.join("shot.png"),
            "nope",
            Trace::RetainOnFailure,
        ),
        None,
    )
    .expect_err("no such play")
    .plain();
    assert!(failure.contains("has no play `nope`"), "{failure}");
}

#[test]
fn a_knob_no_frame_declares_fails_its_play_and_not_the_run() {
    fn misspelt(p: &mut Play<'_>) -> PlayResult {
        p.set_knob("tpyo", 1);
        p.idle(2)
    }
    fn fine(p: &mut Play<'_>) -> PlayResult {
        p.idle(1)
    }
    let (manifest, plays) = catalog(two_buttons, &[("misspelt", misspelt), ("fine", fine)]);
    let dir = scratch("unknown-knob");
    let report = dir.join("capture.json");
    let failure = render_with_globals(
        &manifest,
        None,
        &plays,
        Renderer::Glow,
        &|_: &egui::Context| {},
        &Capture {
            shots: vec![
                shot(
                    "plays::scene",
                    &dir.join("first.png"),
                    "misspelt",
                    Trace::RetainOnFailure,
                ),
                shot(
                    "plays::scene",
                    &dir.join("second.png"),
                    "fine",
                    Trace::RetainOnFailure,
                ),
            ],
            sheet: None,
            report: Some(report.clone()),
        },
        LogMode::Off,
    )
    .expect_err("the misspelt play fails")
    .plain();

    assert!(failure.contains("1 of 2 plays failed"), "{failure}");
    assert!(dir.join("second.png").exists(), "the run went on");
    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report).expect("the report")).expect("JSON");
    let failed = &json["failed_shots"][0];
    assert!(
        failed["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("no knob matches `tpyo`")),
        "{json}"
    );
    assert!(
        failed["at"]
            .as_str()
            .is_some_and(|at| at.starts_with("src/play_tests.rs:")),
        "points at the `set_knob` line: {json}"
    );
}

#[test]
fn a_knob_set_as_the_last_thing_a_play_does_still_counts() {
    fn loudness(ctx: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
        let loud = ctx.toggle("loud", false).get();
        ui.label(if loud { "LOUD" } else { "quiet" });
    }
    fn turn_it_up_last(p: &mut Play<'_>) -> PlayResult {
        p.idle(1)?;
        p.set_knob("loud", true);
        Ok(())
    }
    let (manifest, plays) = catalog(loudness, &[("turn_it_up_last", turn_it_up_last)]);
    let dir = scratch("last-knob");
    capture(
        &manifest,
        &plays,
        shot(
            "plays::scene",
            &dir.join("shot.png"),
            "turn_it_up_last",
            Trace::RetainOnFailure,
        ),
        None,
    )
    .unwrap_or_else(|failure| panic!("{}", failure.plain()));
}

fn shown_slider(ctx: &mut SceneCtx<'_>, _: &mut egui::Ui) {
    if ctx.toggle("show", true).get() {
        ctx.slider("x", 1.0, 0.0, 10.0, 1.0);
    }
}

#[test]
fn a_value_past_its_range_fails_the_play_even_once_its_knob_is_hidden() {
    fn past_then_hide(p: &mut Play<'_>) -> PlayResult {
        p.set_knob("x", 12);
        p.idle(2)?;
        p.set_knob("show", false);
        p.idle(2)
    }
    let (manifest, plays) = catalog(shown_slider, &[("past_then_hide", past_then_hide)]);
    let dir = scratch("past-range-hidden");
    let failure = capture(
        &manifest,
        &plays,
        shot(
            "plays::scene",
            &dir.join("shot.png"),
            "past_then_hide",
            Trace::RetainOnFailure,
        ),
        None,
    )
    .expect_err("12 never landed")
    .plain();
    assert!(
        failure.contains("takes 0 ..= 10; 12 is outside it"),
        "{failure}"
    );
}

/// A group heading of the label left in view is no stand-in for the knob the play hid.
#[test]
fn a_recipe_knob_the_play_hides_behind_a_group_of_its_label_still_passes() {
    fn sectioned(ctx: &mut SceneCtx<'_>, _: &mut egui::Ui) {
        let open = ctx.toggle("open", true).get();
        ctx.group("size");
        if open {
            ctx.slider("size", 1.0, 0.0, 10.0, 1.0);
        }
    }
    fn close(p: &mut Play<'_>) -> PlayResult {
        p.set_knob("open", false);
        p.idle(3)
    }
    let (manifest, plays) = catalog(sectioned, &[("close", close)]);
    let dir = scratch("hidden-behind-group");
    let mut closing = shot(
        "plays::scene",
        &dir.join("shot.png"),
        "close",
        Trace::RetainOnFailure,
    );
    closing.knobs = [crate::render::KnobOverride {
        key: "size".to_owned(),
        value: "7".to_owned(),
    }]
    .into_iter()
    .collect();
    capture(&manifest, &plays, closing, None)
        .unwrap_or_else(|failure| panic!("{}", failure.plain()));
}

#[test]
fn a_failed_play_says_which_knob_never_took_its_value() {
    fn past_then_fail(p: &mut Play<'_>) -> PlayResult {
        p.set_knob("x", 12);
        // A step, so the report's screenshots hint stands beside the knob's.
        p.step("wait", |p| p.idle(2))?;
        Err(PlayError::new("expected x at 12"))
    }
    let (manifest, plays) = catalog(shown_slider, &[("past_then_fail", past_then_fail)]);
    let dir = scratch("past-range-hint");
    let failure = capture(
        &manifest,
        &plays,
        shot(
            "plays::scene",
            &dir.join("shot.png"),
            "past_then_fail",
            Trace::RetainOnFailure,
        ),
        None,
    )
    .expect_err("the play fails")
    .plain();
    assert!(failure.contains("expected x at 12"), "{failure}");
    assert!(
        failure.contains("not at the value set: knob `x` takes 0 ..= 10; 12 is outside it"),
        "{failure}"
    );
    assert!(
        failure.contains("screenshots of each step are in"),
        "{failure}"
    );
}

#[test]
fn a_later_value_past_its_range_fails_though_an_earlier_one_landed() {
    fn landed_then_past(p: &mut Play<'_>) -> PlayResult {
        p.set_knob("x", 5);
        p.idle(2)?;
        p.set_knob("x", 12);
        p.idle(2)?;
        p.set_knob("show", false);
        p.idle(2)
    }
    let (manifest, plays) = catalog(shown_slider, &[("landed_then_past", landed_then_past)]);
    let dir = scratch("landed-then-past");
    let failure = capture(
        &manifest,
        &plays,
        shot(
            "plays::scene",
            &dir.join("shot.png"),
            "landed_then_past",
            Trace::RetainOnFailure,
        ),
        None,
    )
    .expect_err("12 never landed, whatever 5 did")
    .plain();
    assert!(failure.contains("12 is outside it"), "{failure}");
}

#[test]
fn knobs_set_last_get_frames_to_land_one_after_another() {
    // No label contains another, or a key would land on the wrong knob as a regex.
    fn gated(ctx: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
        let top = ctx.slider("top", 10.0, 1.0, 20.0, 1.0).get();
        // Declared from last frame's toggle, so `level` shows up a frame after `show` lands.
        let id = egui::Id::new("shown last frame");
        let shown = ui.data(|data| data.get_temp::<bool>(id)).unwrap_or(false);
        let show = ctx.toggle("show", false).get();
        ui.data_mut(|data| data.insert_temp(id, show));
        if shown {
            ctx.slider("level", 1.0, 0.0, top, 1.0);
        }
    }
    fn reveal_last(p: &mut Play<'_>) -> PlayResult {
        p.idle(1)?;
        p.set_knob("show", true);
        p.set_knob("top", 20);
        p.set_knob("level", 15);
        Ok(())
    }
    let (manifest, plays) = catalog(gated, &[("reveal_last", reveal_last)]);
    let dir = scratch("tail-frames");
    capture(
        &manifest,
        &plays,
        shot(
            "plays::scene",
            &dir.join("shot.png"),
            "reveal_last",
            Trace::RetainOnFailure,
        ),
        None,
    )
    .unwrap_or_else(|failure| panic!("{}", failure.plain()));
}

#[test]
fn a_key_set_twice_is_blamed_on_its_last_set() {
    static LAST: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    fn twice(p: &mut Play<'_>) -> PlayResult {
        p.set_knob("x", 12);
        LAST.store(line!() + 1, std::sync::atomic::Ordering::Relaxed);
        p.set_knob("x", 13);
        p.idle(2)
    }
    let (manifest, plays) = catalog(shown_slider, &[("twice", twice)]);
    let dir = scratch("set-twice");
    let report = dir.join("capture.json");
    capture(
        &manifest,
        &plays,
        shot(
            "plays::scene",
            &dir.join("shot.png"),
            "twice",
            Trace::RetainOnFailure,
        ),
        Some(report.clone()),
    )
    .expect_err("13 is past the range");
    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report).expect("the report")).expect("JSON");
    let failed = &json["failed_shots"][0];
    assert!(
        failed["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("13 is outside it")),
        "{json}"
    );
    let line = LAST.load(std::sync::atomic::Ordering::Relaxed);
    assert!(
        failed["at"]
            .as_str()
            .is_some_and(|at| at.starts_with(&format!("src/play_tests.rs:{line}:"))),
        "points at the second `set_knob`, line {line}: {json}"
    );
}

/// The report a failed one-shot capture wrote, and the shot it failed.
fn failed_shot(scene: SceneFn, name: &'static str, run: PlayFn) -> serde_json::Value {
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
    .expect_err("the shot fails");
    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report).expect("the report")).expect("JSON");
    json["failed_shots"][0].clone()
}

fn reason(failed: &serde_json::Value) -> &str {
    failed["reason"].as_str().unwrap_or_default()
}

/// `x` also reads as a regex matching `max`, which it lands on while its own knob is hidden.
fn shadowed(ctx: &mut SceneCtx<'_>, _: &mut egui::Ui) {
    ctx.slider("max", 1.0, 0.0, 10.0, 1.0);
    if ctx.toggle("show", false).get() {
        ctx.slider("x", 1.0, 0.0, 10.0, 1.0);
    }
}

#[test]
fn a_key_that_moves_to_another_knob_fails_rather_than_set_both() {
    fn reveal(p: &mut Play<'_>) -> PlayResult {
        p.set_knob("x", 5);
        p.idle(2)?;
        p.set_knob("show", true);
        p.idle(2)
    }
    let failed = failed_shot(shadowed, "reveal", reveal);
    assert!(
        reason(&failed).contains("knob key `x` matched `max`, then `x`"),
        "{failed}"
    );
}

#[test]
fn a_play_ending_with_its_knob_hidden_does_not_let_the_settle_move_the_key() {
    fn shown_shadowed(ctx: &mut SceneCtx<'_>, _: &mut egui::Ui) {
        ctx.slider("max", 1.0, 0.0, 10.0, 1.0);
        if ctx.toggle("show", true).get() {
            ctx.slider("x", 1.0, 0.0, 10.0, 1.0);
        }
    }
    fn set_then_hide(p: &mut Play<'_>) -> PlayResult {
        p.set_knob("x", 5);
        p.idle(2)?;
        p.set_knob("show", false);
        Ok(())
    }
    let (manifest, plays) = catalog(shown_shadowed, &[("set_then_hide", set_then_hide)]);
    let dir = scratch("set-then-hide");
    let failure = capture(
        &manifest,
        &plays,
        shot(
            "plays::scene",
            &dir.join("shot.png"),
            "set_then_hide",
            Trace::RetainOnFailure,
        ),
        None,
    )
    .expect_err("`x` would land on `max` once hidden")
    .plain();
    assert!(
        failure.contains("knob key `x` matched `x`, then `max`"),
        "{failure}"
    );
}

#[test]
fn a_value_set_while_its_knob_is_hidden_says_so_rather_than_no_knob_matches() {
    fn hide_then_set(p: &mut Play<'_>) -> PlayResult {
        p.set_knob("x", 5);
        p.idle(2)?;
        p.set_knob("show", false);
        p.idle(2)?;
        p.set_knob("x", 6);
        Ok(())
    }
    let failed = failed_shot(shown_slider, "hide_then_set", hide_then_set);
    assert!(
        reason(&failed).contains("knob `x`, which `x` matched before, is not declared now"),
        "{failed}"
    );
}

#[test]
fn a_bad_value_set_last_is_blamed_on_its_set_knob() {
    static AT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    fn bad_last(p: &mut Play<'_>) -> PlayResult {
        p.idle(1)?;
        AT.store(line!() + 1, std::sync::atomic::Ordering::Relaxed);
        p.set_knob("x", "abc");
        Ok(())
    }
    let failed = failed_shot(shown_slider, "bad_last", bad_last);
    assert!(
        reason(&failed).contains("`abc` is not a number"),
        "{failed}"
    );
    let line = AT.load(std::sync::atomic::Ordering::Relaxed);
    assert!(
        failed["at"]
            .as_str()
            .is_some_and(|at| at.starts_with(&format!("src/play_tests.rs:{line}:"))),
        "points at the `set_knob`, line {line}: {failed}"
    );
}

#[test]
fn a_state_a_passing_play_leaves_past_a_range_names_the_play() {
    fn ranged(ctx: &mut SceneCtx<'_>, _: &mut egui::Ui) {
        let top = ctx.slider("top", 10.0, 1.0, 20.0, 1.0).get();
        ctx.slider("level", 1.0, 0.0, top, 1.0);
    }
    fn narrow(p: &mut Play<'_>) -> PlayResult {
        p.set_knob("level", 8);
        p.idle(2)?;
        p.set_knob("top", 5);
        p.idle(2)
    }
    let (manifest, plays) = catalog(ranged, &[("narrow", narrow)]);
    let dir = scratch("narrow");
    let failure = capture(
        &manifest,
        &plays,
        shot(
            "plays::scene",
            &dir.join("shot.png"),
            "narrow",
            Trace::RetainOnFailure,
        ),
        None,
    )
    .expect_err("8 is past the narrowed range")
    .plain();
    assert!(
        failure.contains("play `narrow` on `scene` passed, but the state it left fails the shot"),
        "{failure}"
    );
    assert!(failure.contains("8 is outside it"), "{failure}");
}

#[test]
fn two_plays_of_one_name_for_one_scene_are_refused_naming_both() {
    fn nothing(_: &mut Play<'_>) -> PlayResult {
        Ok(())
    }
    let (manifest, mut plays) = catalog(two_buttons, &[("smoke", nothing)]);
    plays.push(PlayEntry {
        module_path: "elsewhere",
        ..plays[0]
    });
    let dir = scratch("twin-plays");
    let failure = capture(
        &manifest,
        &plays,
        shot(
            "plays::scene",
            &dir.join("shot.png"),
            "smoke",
            Trace::RetainOnFailure,
        ),
        None,
    )
    .expect_err("the name is ambiguous")
    .plain();
    assert!(failure.contains("2 plays named `smoke`"), "{failure}");
    assert!(failure.contains("plays::smoke"), "{failure}");
    assert!(failure.contains("elsewhere::smoke"), "{failure}");
}

#[test]
fn a_recipe_knob_the_scene_hides_does_not_fail_a_play() {
    fn modal(ctx: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
        if ctx.toggle("advanced", true).get() {
            let gain = ctx.slider("gain", 1.0, 0.0, 10.0, 1.0).get();
            ui.label(format!("gain {gain}"));
        }
        ui.label("modal");
    }
    fn look(p: &mut Play<'_>) -> PlayResult {
        p.idle(2)?;
        p.step("the simple mode shows", |p| {
            p.get_by_label("modal");
            assert!(p.query_by_label("gain 2").is_none(), "gain is hidden");
            Ok(())
        })
    }
    let (manifest, plays) = catalog(modal, &[("look", look)]);
    let dir = scratch("hidden-knob");
    let mut simple = shot(
        "plays::scene",
        &dir.join("shot.png"),
        "look",
        Trace::RetainOnFailure,
    );
    simple.knobs = [
        crate::render::KnobOverride {
            key: "advanced".to_owned(),
            value: "false".to_owned(),
        },
        crate::render::KnobOverride {
            key: "gain".to_owned(),
            value: "2".to_owned(),
        },
    ]
    .into_iter()
    .collect();
    capture(&manifest, &plays, simple, None)
        .unwrap_or_else(|failure| panic!("{}", failure.plain()));
}

#[test]
fn a_knob_a_play_sets_outranks_the_recipe_through_the_capture() {
    thread_local! {
        static LAST: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
    }
    fn loudness(ctx: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
        let loud = ctx.toggle("loud", false).get();
        LAST.set(Some(loud));
        ui.label(if loud { "LOUD" } else { "quiet" });
    }
    fn turn_it_up(p: &mut Play<'_>) -> PlayResult {
        p.set_knob("loud", true);
        p.idle(3)?;
        p.step("the scene took it", |p| {
            p.get_by_label("LOUD");
            Ok(())
        })
    }
    let (manifest, plays) = catalog(loudness, &[("turn_it_up", turn_it_up)]);
    let dir = scratch("knob");
    let mut quiet = shot(
        "plays::scene",
        &dir.join("shot.png"),
        "turn_it_up",
        Trace::RetainOnFailure,
    );
    quiet.knobs = [crate::render::KnobOverride {
        key: "loud".to_owned(),
        value: "false".to_owned(),
    }]
    .into_iter()
    .collect();
    capture(&manifest, &plays, quiet, None).unwrap_or_else(|failure| panic!("{}", failure.plain()));
    assert_eq!(
        LAST.get(),
        Some(true),
        "the captured frame drew the play's value"
    );
}

#[test]
fn a_play_writes_the_same_bytes_twice() {
    let run = |name: &str| {
        let dir = scratch(name);
        let out = dir.join("form.png");
        capture(
            &Linked.manifest(),
            &Linked.plays(),
            shot(FORM, &out, "saves_after_a_moment", Trace::RetainOnFailure),
            None,
        )
        .unwrap_or_else(|failure| panic!("{}", failure.plain()));
        std::fs::read(&out).expect("the capture")
    };
    assert!(
        run("same-once") == run("same-twice"),
        "two runs of one play differ"
    );
}

#[test]
fn a_passing_run_clears_an_old_trace_and_nothing_else() {
    let dir = scratch("stale");
    let trace = dir.join("form.steps");
    std::fs::create_dir_all(&trace).expect("the trace dir");
    std::fs::write(trace.join("07-an-old-failure.png"), b"stale").expect("a stale trace");
    std::fs::write(trace.join("notes.txt"), b"mine").expect("an unrelated file");
    capture(
        &Linked.manifest(),
        &Linked.plays(),
        shot(
            FORM,
            &dir.join("form.png"),
            "saves_after_a_moment",
            Trace::RetainOnFailure,
        ),
        None,
    )
    .unwrap_or_else(|failure| panic!("{}", failure.plain()));
    assert_eq!(trace_files(&trace), ["notes.txt"]);
}

/// The number at the end of the one label starting with `prefix`.
fn reading(p: &Play<'_>, prefix: &str) -> f32 {
    let found = p.get(By::new().label_contains(prefix));
    let label = found.label.expect("a labelled reading");
    label[prefix.len()..]
        .trim()
        .parse()
        .unwrap_or_else(|_| panic!("`{label}` ends in a number"))
}

#[test]
fn a_drag_moves_the_pointer_across_the_widget_while_held() {
    fn pad(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
        let id = ui.make_persistent_id("dragged");
        let mut dragged: f32 = ui.data_mut(|data| *data.get_temp_mut_or_default(id));
        let (_, response) = ui.allocate_exact_size(egui::vec2(200.0, 60.0), egui::Sense::drag());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "pad"));
        dragged += response.drag_delta().x;
        ui.data_mut(|data| data.insert_temp(id, dragged));
        ui.label(format!("dragged {dragged}"));
    }
    fn drag_right(p: &mut Play<'_>) -> PlayResult {
        p.drag("pad", [100.0, 0.0])?;
        p.step("the pad followed", |p| {
            let dragged = reading(p, "dragged");
            // `Response::drag_delta` is zero until `InputState::is_decidedly_dragging`,
            // so the first move may go uncounted.
            assert!(dragged > 50.0, "dragged {dragged} of 100 points");
            Ok(())
        })
    }
    let (manifest, plays) = catalog(pad, &[("drag_right", drag_right)]);
    let dir = scratch("drag");
    capture(
        &manifest,
        &plays,
        shot(
            "plays::scene",
            &dir.join("shot.png"),
            "drag_right",
            Trace::RetainOnFailure,
        ),
        None,
    )
    .unwrap_or_else(|failure| panic!("{}", failure.plain()));
}

#[test]
fn a_scroll_moves_the_content_under_the_widget() {
    fn list(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
        let id = ui.make_persistent_id("offset");
        let offset: f32 = ui.data_mut(|data| *data.get_temp_mut_or_default(id));
        ui.label(format!("offset {offset}"));
        let output = egui::ScrollArea::vertical()
            .max_height(80.0)
            .show(ui, |ui| {
                for row in 0..40 {
                    ui.label(format!("row {row}"));
                }
            });
        ui.data_mut(|data| data.insert_temp(id, output.state.offset.y));
    }
    fn scroll_down(p: &mut Play<'_>) -> PlayResult {
        p.scroll("row 1", [0.0, -200.0])?;
        p.step("the list moved", |p| {
            let offset = reading(p, "offset");
            assert!(offset > 0.0, "scrolled to {offset}");
            Ok(())
        })
    }
    let (manifest, plays) = catalog(list, &[("scroll_down", scroll_down)]);
    let dir = scratch("scroll");
    capture(
        &manifest,
        &plays,
        shot(
            "plays::scene",
            &dir.join("shot.png"),
            "scroll_down",
            Trace::RetainOnFailure,
        ),
        None,
    )
    .unwrap_or_else(|failure| panic!("{}", failure.plain()));
}

fn switch(ctx: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
    let on = ctx.toggle("on", false).get();
    ui.label(if on { "ON" } else { "off" });
}

fn flip(p: &mut Play<'_>) -> PlayResult {
    p.step("before", |p| p.idle(2))?;
    p.set_knob("on", true);
    p.step("after", |p| p.idle(3))
}

fn flip_then_fail(p: &mut Play<'_>) -> PlayResult {
    p.idle(2)?;
    Err(PlayError::new("it was never going to pass"))
}

fn nested(p: &mut Play<'_>) -> PlayResult {
    p.step("outer", |p| p.step("inner", |p| p.idle(2)))
}

fn tab_then_hover(p: &mut Play<'_>) -> PlayResult {
    p.step("tab", |p| p.press("Tab"))?;
    p.step("hover", |p| p.hover("off"))
}

/// Every file in `dir`, by name, with its bytes.
fn files(dir: &Utf8Path) -> Vec<(String, Vec<u8>)> {
    let mut files: Vec<_> = dir
        .read_dir_utf8()
        .expect("the directory")
        .map(|entry| {
            let entry = entry.expect("an entry");
            (
                entry.file_name().to_owned(),
                std::fs::read(entry.path()).expect("the file"),
            )
        })
        .collect();
    files.sort();
    files
}

#[test]
fn a_plays_input_shows_on_its_recording_and_nowhere_else() {
    let record = |input| crate::render::Record {
        fps: 30,
        seconds: None,
        keep: Trace::On,
        formats: crate::render::VideoFormat::Apng.into(),
        hold: None,
        input,
    };
    let (plain, _) = record_play("input-off", tab_then_hover, Trace::On, record(None));
    let (shown, _) = record_play(
        "input-on",
        tab_then_hover,
        Trace::On,
        record(Some(crate::render::Corner::BottomLeft)),
    );
    let png = |video: &Utf8Path| std::fs::read(video.with_extension("png")).expect("the PNG");
    assert_eq!(
        png(&plain),
        png(&shown),
        "the shot's PNG never shows the input"
    );
    let steps = |video: &Utf8Path| files(&video.with_extension("steps"));
    assert_eq!(steps(&plain), steps(&shown), "nor do its step screenshots");

    let last = |video: &Utf8Path| {
        let (_, frames) = crate::video::decode(video);
        frames.last().expect("a frame").image.clone()
    };
    let (plain, shown) = (last(&plain), last(&shown));
    let (width, height) = shown.dimensions();
    let bottom_left = |image: &image::RgbaImage| {
        image::imageops::crop_imm(image, 0, height / 2, width / 2, height / 2).to_image()
    };
    assert_ne!(
        bottom_left(&plain),
        bottom_left(&shown),
        "the Tab pressed a second ago is listed in the bottom-left corner"
    );
}

#[test]
fn one_play_records_in_every_format_asked_for_from_the_same_frames() {
    let record = |formats| crate::render::Record {
        fps: 30,
        seconds: None,
        keep: Trace::On,
        formats,
        hold: None,
        input: None,
    };
    let both = crate::render::VideoFormats::of([
        crate::render::VideoFormat::Apng,
        crate::render::VideoFormat::Mp4,
    ])
    .expect("two formats");
    let (apng, report) = record_play("both-formats", flip, Trace::On, record(both));
    let mp4 = apng.with_extension("mp4");
    assert!(mp4.exists(), "`{mp4}` beside `{apng}`");
    let videos = report["shots"][0]["videos"]
        .as_array()
        .expect("a list of recordings");
    let formats: Vec<_> = videos
        .iter()
        .map(|video| video["format"].as_str())
        .collect();
    assert_eq!(formats, [Some("apng"), Some("mp4")]);
    assert_eq!(
        videos[0]["frames"], videos[1]["frames"],
        "both files hold every frame the play drew"
    );

    let (alone, _) = record_play(
        "apng-alone",
        flip,
        Trace::On,
        record(crate::render::VideoFormat::Apng.into()),
    );
    assert_eq!(
        std::fs::read(&apng).expect("the APNG"),
        std::fs::read(&alone).expect("the lone APNG"),
        "the MP4 beside it changes nothing in the APNG"
    );
}

/// A play shot recording at 30 fps, kept as `keep` says
/// and pausing `hold` seconds at step ends;
/// the recording and the run's report.
fn recorded_play(
    name: &'static str,
    run: PlayFn,
    keep: Trace,
    hold: Option<f32>,
) -> (Utf8PathBuf, serde_json::Value) {
    record_play(
        name,
        run,
        Trace::RetainOnFailure,
        crate::render::Record {
            fps: 30,
            seconds: None,
            keep,
            formats: crate::render::VideoFormat::Apng.into(),
            hold,
            input: None,
        },
    )
}

/// A play shot recorded as `record` says, its step screenshots
/// kept as `trace` says; the recording and the run's report.
fn record_play(
    name: &'static str,
    run: PlayFn,
    trace: Trace,
    record: crate::render::Record,
) -> (Utf8PathBuf, serde_json::Value) {
    let (manifest, plays) = catalog(switch, &[(name, run)]);
    let dir = scratch(name);
    let report = dir.join("capture.json");
    let mut recorded = shot("plays::scene", &dir.join("shot.png"), name, trace);
    recorded.record = Some(record);
    _ = capture(&manifest, &plays, recorded, Some(report.clone()));
    let json = serde_json::from_slice(&std::fs::read(&report).expect("the report")).expect("JSON");
    (dir.join("shot.apng"), json)
}

/// How many frames a recording runs, every written frame's delay added up.
fn recorded_frames(video: &Utf8Path) -> u64 {
    let (_, shown) = crate::video::decode(video);
    shown.iter().map(|frame| u64::from(frame.delay.0)).sum()
}

#[test]
fn a_recorded_play_holds_every_frame_the_play_drew() {
    let (video, report) = recorded_play("flip", flip, Trace::On, Some(0.0));
    let shot = &report["shots"][0];
    let played = shot["play"]["frames"].as_u64().expect("the play's frames");
    assert_eq!(shot["videos"][0]["frames"].as_u64(), Some(played));
    assert_eq!(shot["videos"][0]["held"].as_u64(), Some(0));
    assert_eq!(
        recorded_frames(&video),
        played,
        "with no hold, the recording lasts the play, frame for frame"
    );
    let (_, shown) = crate::video::decode(&video);
    assert!(shown.len() >= 2, "the label's change is in it");
}

#[test]
fn a_recorded_play_pauses_at_each_steps_end() {
    let (video, report) = recorded_play("flip-held", flip, Trace::On, None);
    let shot = &report["shots"][0];
    let played = shot["play"]["frames"].as_u64().expect("the play's frames");
    assert_eq!(
        shot["videos"][0]["held"].as_u64(),
        Some(30),
        "two steps, each held half a second at 30 fps"
    );
    assert_eq!(shot["videos"][0]["frames"].as_u64(), Some(played));
    assert_eq!(recorded_frames(&video), played + 30);
}

#[test]
fn a_step_ending_with_the_one_around_it_pauses_once() {
    let (video, report) = recorded_play("nested-held", nested, Trace::On, None);
    let shot = &report["shots"][0];
    let played = shot["play"]["frames"].as_u64().expect("the play's frames");
    assert_eq!(shot["videos"][0]["held"].as_u64(), Some(15));
    assert_eq!(recorded_frames(&video), played + 15);
}

#[test]
fn a_passing_play_drops_a_recording_kept_only_on_failure() {
    let (video, report) = recorded_play("flip-kept-on-failure", flip, Trace::RetainOnFailure, None);
    assert!(!video.exists(), "the play passed, so `{video}` is gone");
    assert!(report["shots"][0].get("videos").is_none());
}

#[test]
fn a_failing_play_keeps_its_recording_and_the_report_points_at_it() {
    let (video, report) = recorded_play(
        "fail-recorded",
        flip_then_fail,
        Trace::RetainOnFailure,
        None,
    );
    assert!(video.exists(), "the play failed, so `{video}` stays");
    let failed = &report["failed_shots"][0];
    assert!(
        failed["videos"][0]
            .as_str()
            .is_some_and(|path| path.ends_with("shot.apng")),
        "{failed}"
    );
}

#[test]
fn a_mistyped_key_fails_its_press_saying_what_was_meant_and_where() {
    fn ready(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
        ui.label("ready");
    }
    fn mistyped(p: &mut Play<'_>) -> PlayResult {
        p.press("Cmd+Entr")
    }
    let (manifest, plays) = catalog(ready, &[("mistyped-key", mistyped)]);
    let dir = scratch("mistyped-key");
    let panel = capture(
        &manifest,
        &plays,
        shot(
            "plays::scene",
            &dir.join("shot.png"),
            "mistyped-key",
            Trace::RetainOnFailure,
        ),
        None,
    )
    .expect_err("the play fails")
    .plain();
    for line in [
        "failed at step `press Cmd+Entr`",
        "`Entr` is no key; did you mean `Enter`?",
        r#"p.press("Cmd+Entr")"#,
    ] {
        assert!(panel.contains(line), "`{line}` in:\n{panel}");
    }
}
