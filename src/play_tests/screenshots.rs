//! Snapshot checks: where references go, what a check
//! leaves behind, and how a run takes a new reference.

mod films;
mod reports;
mod tree;

use super::*;
use crate::{
    Canvas, Screenshot,
    play::log::{Check, Outcome, What},
    render::{Films, Record, Snapshots, Update, VideoFormat, VideoFormats, judged_shot},
};

fn hello_in(ui: &mut egui::Ui, colour: egui::Color32) {
    ui.colored_label(colour, "Hello");
}

fn red(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
    hello_in(ui, egui::Color32::RED);
}

/// [`red`] in another colour: the same size, other pixels.
fn blue(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
    hello_in(ui, egui::Color32::BLUE);
}

/// Wider than [`red`].
fn wide(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
    ui.colored_label(egui::Color32::RED, "Hello, world");
}

fn block(ui: &mut egui::Ui, level: u8) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(60.0, 20.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, 0.0, egui::Color32::from_gray(level));
}

fn grey_100(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
    block(ui, 100);
}

/// 25 levels from [`grey_100`]: just within the default threshold.
fn grey_125(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
    block(ui, 125);
}

/// Past the passes a shot draws before
/// its play begins, within the wait budget.
const SETTLES_AT: u64 = 40;

/// Looks alike from frame to frame while it asks to be redrawn,
/// then turns into [`grey_125`]: a save under way, then done.
fn saving(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
    if ui.ctx().cumulative_pass_nr() < SETTLES_AT {
        block(ui, 200);
        ui.ctx().request_repaint();
    } else {
        block(ui, 125);
    }
}

/// Another grey on every pass, for ever, without asking to be redrawn.
fn flickering_unasked(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
    let pass = u8::try_from(ui.ctx().cumulative_pass_nr() % 100).expect("below 100");
    block(ui, 100 + pass);
}

fn flickering(ctx: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
    flickering_unasked(ctx, ui);
    ui.ctx().request_repaint();
}

fn greets(p: &mut Play<'_>) -> PlayResult {
    p.expect(Canvas).to_match_screenshot("greeting")
}

/// One test of a play, references kept in its directory's `refs`.
struct Judged {
    passed: bool,
    /// What the log says of the check.
    outcome: Option<Outcome>,
    /// The panel a failure prints.
    panel: String,
    /// The play's last snapshot check.
    check: Check,
    /// How many frames it drew.
    frames: usize,
}

impl Judged {
    fn actual(&self) -> &str {
        self.check.actual.as_deref().expect("what it saw")
    }

    fn attachments(&self) -> Vec<&str> {
        let kept = self.check.kept.as_ref().expect("a snapshot check");
        kept.attachments.iter().map(|one| one.kind.name()).collect()
    }
}

/// The shot every test here draws, of the play its catalog names `greets`.
fn greeting_shot(dir: &Utf8Path) -> Shot {
    shot(
        "plays::scene",
        &dir.join("shot.png"),
        "greets",
        Trace::Failing,
    )
}

fn judge(scene: SceneFn, run: PlayFn, dir: &Utf8Path, snapshots: Snapshots) -> Judged {
    judge_filmed(scene, run, dir, snapshots, None)
}

/// [`judge`], the play filmed.
fn judge_filmed(
    scene: SceneFn,
    run: PlayFn,
    dir: &Utf8Path,
    snapshots: Snapshots,
    films: Option<&Films>,
) -> Judged {
    let (manifest, plays) = catalog(scene, &[("greets", run)]);
    let snapshots = Snapshots {
        dir: Some(dir.join("refs")),
        ..snapshots
    };
    let (outcome, log, _) = judged_shot(
        &manifest,
        &plays,
        Renderer::Glow,
        &greeting_shot(dir),
        &snapshots,
        films,
    );
    let (check, frames, logged) = log
        .into_iter()
        .rev()
        .find_map(|entry| match entry.what {
            What::Check(check) if check.matcher.starts_with("to_match_") => {
                Some((*check, entry.frames.len(), entry.outcome))
            }
            _ => None,
        })
        .expect("a snapshot check");
    Judged {
        passed: outcome.is_ok(),
        outcome: logged,
        panel: outcome
            .err()
            .map(|failure| failure.plain())
            .unwrap_or_default(),
        check,
        frames,
    }
}

fn updating(update: Update) -> Snapshots {
    Snapshots {
        update,
        ..Snapshots::default()
    }
}

/// Take `scene` as the reference `run` checks.
#[track_caller]
fn take(scene: SceneFn, run: PlayFn, dir: &Utf8Path) {
    let taken = judge(scene, run, dir, updating(Update::Failing));
    assert!(taken.passed, "{}", taken.panel);
}

/// Where the play's references go.
fn refs(dir: &Utf8Path) -> Utf8PathBuf {
    dir.join("refs/greets")
}

fn files(dir: &Utf8Path) -> Vec<String> {
    trace_files(&refs(dir))
}

const REFERENCE: &str = "greeting.glow.png";
const NEW: &str = "greeting.glow.new.png";
const DIFF: &str = "greeting.glow.diff.png";

fn reference(dir: &Utf8Path) -> Vec<u8> {
    std::fs::read(refs(dir).join(REFERENCE)).expect("the reference")
}

#[test]
fn a_capture_compares_no_snapshots_and_says_so_whatever_its_shot() {
    fn both(p: &mut Play<'_>) -> PlayResult {
        p.expect(Canvas).to_match_screenshot("greeting")?;
        p.expect(Canvas).to_match_accesskit_snapshot("greeting")?;
        p.expect("Nowhere").to_match_accesskit_snapshot("nowhere")?;
        p.expect("Hello").to_exist()?;
        p.idle(1)
    }
    let (manifest, mut plays) = catalog(red, &[("greets", both)]);
    let dir = scratch("screenshot-capture");
    std::fs::create_dir_all(&dir).expect("made");
    let scenes = dir.join("form.scene.rs");
    std::fs::write(&scenes, "").expect("written");
    plays[0].file = String::leak(scenes.into_string());
    let report = dir.join("capture.json");
    let recorded = Record {
        fps: 30,
        seconds: None,
        keep: Trace::Always,
        formats: VideoFormats::default(),
        hold: Some(0.0),
        input: None,
        glide: crate::render::Glide::default(),
    };
    for record in [None, Some(recorded)] {
        let shot = Shot {
            size: egui::vec2(640.0, 360.0),
            scale: 2.0,
            record,
            ..greeting_shot(&dir)
        };
        capture(&manifest, &plays, shot, Some(report.clone()))
            .unwrap_or_else(|failure| panic!("{}", failure.plain()));
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&report).expect("the report")).expect("JSON");
        let log = json["shots"][0]["play"]["log"].as_array().expect("a log");
        let told: Vec<_> = log
            .iter()
            .take(4)
            .map(|entry| (&entry["outcome"], &entry["frames"]))
            .collect();
        let skipped = (&serde_json::json!("skipped"), &serde_json::json!([0, 0]));
        let passed = (&serde_json::json!("passed"), &serde_json::json!([0, 0]));
        assert_eq!(told, [skipped, skipped, skipped, passed]);
        assert_eq!(log[0]["actual"], "not compared in a capture");
        assert!(!dir.join("__snapshots__").exists(), "nothing is written");
    }
}

#[test]
fn a_captures_canvas_keeps_its_size_past_a_check() {
    let (manifest, plays) = catalog(grows_to_an_end, &[("greets", grows)]);
    let dir = scratch("screenshot-capture-grows");
    let shot = Shot {
        record: Some(Record {
            fps: 30,
            seconds: None,
            keep: Trace::Always,
            formats: VideoFormat::Apng.into(),
            hold: Some(0.0),
            input: None,
            glide: crate::render::Glide::default(),
        }),
        ..greeting_shot(&dir)
    };
    capture(&manifest, &plays, shot, None).unwrap_or_else(|failure| panic!("{}", failure.plain()));
    let video = image::open(dir.join("shot.apng")).expect("the recording");
    assert_eq!((video.width(), video.height()), (320, 240));
}

#[test]
fn a_check_misused_fails_a_test_and_is_skipped_in_a_capture_like_any_other() {
    fn out_of_range(p: &mut Play<'_>) -> PlayResult {
        p.expect(Canvas)
            .to_match_screenshot(Screenshot::named("greeting").threshold(1.5))
    }
    fn negated(p: &mut Play<'_>) -> PlayResult {
        p.expect("Hello").not().to_match_accesskit_snapshot("tree")
    }
    fn nameless(p: &mut Play<'_>) -> PlayResult {
        p.expect(Canvas).to_match_screenshot("✓")
    }
    for (misused, why) in [
        (out_of_range as PlayFn, "threshold"),
        (negated, "no opposite"),
        (nameless, "name"),
    ] {
        let (manifest, plays) = catalog(red, &[("greets", misused)]);
        let dir = scratch("screenshot-misused");
        capture(&manifest, &plays, greeting_shot(&dir), None)
            .unwrap_or_else(|failure| panic!("{}", failure.plain()));
        let judged = judge(red, misused, &dir, updating(Update::All));
        assert!(!judged.passed, "{why}");
        assert!(judged.actual().contains(why), "{}", judged.actual());
    }
}

#[test]
fn a_test_judges_on_after_a_check_that_failed_and_was_ignored() {
    fn ignores(p: &mut Play<'_>) -> PlayResult {
        _ = p.expect(Canvas).to_match_screenshot("✓");
        _ = p.expect(Canvas).to_match_screenshot("other");
        _ = p.expect(Canvas).to_match_screenshot("other");
        p.expect(Canvas).to_match_screenshot("greeting")
    }
    let dir = scratch("screenshot-ignored");
    let judged = judge(red, ignores, &dir, Snapshots::default());
    assert!(!judged.passed, "it has no reference");
    assert_eq!(judged.actual(), "no reference yet");
}

#[test]
fn an_update_is_no_business_of_a_captures() {
    fn fails_after(p: &mut Play<'_>) -> PlayResult {
        p.expect(Canvas).to_match_screenshot("greeting")?;
        p.expect("Nowhere").within(1).to_exist()
    }
    let (manifest, plays) = catalog(red, &[("greets", fails_after)]);
    let dir = scratch("screenshot-capture-fails");
    let failure = capture(&manifest, &plays, greeting_shot(&dir), None)
        .expect_err("an ordinary check still fails the play")
        .plain();
    assert!(failure.contains("expected \"Nowhere\" exists"), "{failure}");
    assert!(!failure.contains("UPDATE_SNAPSHOTS"), "{failure}");
}

#[test]
fn with_no_reference_a_screenshot_fails_and_leaves_what_it_saw() {
    let dir = scratch("screenshot-missing");
    let judged = judge(red, greets, &dir, Snapshots::default());
    assert!(!judged.passed, "nothing to compare with is no pass");
    assert_eq!(judged.actual(), "no reference yet");
    assert_eq!(judged.attachments(), ["actual"]);
    assert_eq!(files(&dir), [NEW]);
    for line in ["actual: ", "`UPDATE_SNAPSHOTS=1` takes"] {
        assert!(
            judged.panel.contains(line),
            "`{line}` in:\n{}",
            judged.panel
        );
    }
}

#[test]
fn an_update_takes_the_reference_and_the_next_run_passes_without_drawing() {
    let dir = scratch("screenshot-update");
    let taken = judge(red, greets, &dir, updating(Update::Failing));
    assert!(taken.passed, "{}", taken.panel);
    assert_eq!(taken.actual(), "no reference yet; taken as the reference");
    assert_eq!(files(&dir), [REFERENCE]);

    let again = judge(red, greets, &dir, Snapshots::default());
    assert!(again.passed, "{}", again.panel);
    assert_eq!(again.actual(), "identical");
    assert_eq!(again.frames, 0, "a canvas already alike draws no frames");
}

#[test]
fn a_changed_canvas_fails_with_its_diff_and_passes_within_the_budget() {
    let dir = scratch("screenshot-changed");
    take(red, greets, &dir);

    let changed = judge(blue, greets, &dir, Snapshots::default());
    assert!(!changed.passed, "red is not blue");
    assert!(
        changed.actual().ends_with("pixels differ, 0 allowed"),
        "{}",
        changed.actual()
    );
    assert_eq!(changed.attachments(), ["reference", "actual", "diff"]);
    assert_eq!(files(&dir), [DIFF, NEW, REFERENCE]);

    let lenient = Snapshots {
        tolerance: u32::MAX,
        ..Snapshots::default()
    };
    let within = judge(blue, greets, &dir, lenient);
    assert!(within.passed, "{}", within.panel);
    assert_eq!(
        files(&dir),
        [REFERENCE],
        "a pass clears what an earlier failure left"
    );
}

#[test]
fn a_screenshot_within_its_budget_is_tolerated_saying_how_much_of_it_differs() {
    let dir = scratch("screenshot-tolerated");
    take(red, greets, &dir);
    let lenient = Snapshots {
        tolerance: 5_000,
        ..Snapshots::default()
    };
    let within = judge(blue, greets, &dir, lenient.clone());
    assert!(within.passed, "{}", within.panel);
    assert_eq!(within.outcome, Some(Outcome::Tolerated));
    assert!(
        within.actual().ends_with("pixels differ, 5000 allowed"),
        "{}",
        within.actual()
    );
    let same = judge(red, greets, &dir, lenient);
    assert_eq!(same.outcome, Some(Outcome::Passed), "{}", same.actual());
}

#[test]
fn a_check_of_its_own_overrides_the_runs_budget() {
    fn lenient(p: &mut Play<'_>) -> PlayResult {
        p.expect(Canvas)
            .to_match_screenshot(Screenshot::named("greeting").tolerance(u32::MAX))
    }
    fn exact(p: &mut Play<'_>) -> PlayResult {
        p.expect(Canvas)
            .to_match_screenshot(Screenshot::named("greeting").threshold(0.0))
    }
    let dir = scratch("screenshot-own-budget");
    take(red, greets, &dir);
    let judged = judge(blue, lenient, &dir, Snapshots::default());
    assert!(judged.passed, "{}", judged.panel);

    let dir = scratch("screenshot-own-threshold");
    take(grey_100, greets, &dir);
    let near = judge(grey_125, greets, &dir, Snapshots::default());
    assert!(near.passed, "{}", near.panel);
    assert_eq!(near.actual(), "no pixel differs past the threshold");
    assert_eq!(
        near.outcome,
        Some(Outcome::Passed),
        "a difference under the threshold takes none of the budget"
    );
    let judged = judge(grey_125, exact, &dir, Snapshots::default());
    assert_eq!(judged.actual(), "1200 pixels differ, 0 allowed");
}

#[test]
fn a_threshold_out_of_range_fails_the_check() {
    fn off_the_scale(p: &mut Play<'_>) -> PlayResult {
        p.expect(Canvas)
            .to_match_screenshot(Screenshot::named("greeting").threshold(1.5))
    }
    let dir = scratch("screenshot-threshold");
    let judged = judge(red, off_the_scale, &dir, updating(Update::All));
    assert!(!judged.passed);
    assert!(
        judged.actual().starts_with("threshold 1.5 is out of range"),
        "{}",
        judged.actual()
    );
}

#[test]
fn a_resized_canvas_says_both_sizes() {
    let dir = scratch("screenshot-resized");
    take(red, greets, &dir);
    let judged = judge(wide, greets, &dir, Snapshots::default());
    assert!(!judged.passed);
    assert!(
        judged.actual().contains(", the reference "),
        "{}",
        judged.actual()
    );
    assert_eq!(judged.attachments(), ["reference", "actual"]);
}

#[test]
fn a_scene_still_asking_for_frames_is_not_settled_however_alike_they_look() {
    let dir = scratch("screenshot-settles");
    take(saving, greets, &dir);
    let settled = reference(&dir);
    let done = judge(grey_125, greets, &dir, Snapshots::default());
    assert_eq!(done.actual(), "identical", "the end state was taken");

    let again = judge(saving, greets, &dir, Snapshots::default());
    assert!(again.passed, "{}", again.panel);
    assert!(again.frames > 0, "it waited for the save to be done");
    assert_eq!(reference(&dir), settled);
}

#[test]
fn a_canvas_that_never_settles_is_never_taken() {
    fn at_once(p: &mut Play<'_>) -> PlayResult {
        p.expect(Canvas).within(0).to_match_screenshot("greeting")
    }
    let dir = scratch("screenshot-unsettled");
    for update in [Update::Off, Update::Failing, Update::All] {
        let judged = judge(flickering, greets, &dir, updating(update));
        assert!(!judged.passed, "{update:?}");
        assert_eq!(
            judged.actual(),
            "no reference yet; not settled within 60 frames"
        );
        assert!(
            !judged.panel.contains("UPDATE_SNAPSHOTS"),
            "an update would not take it either:\n{}",
            judged.panel
        );
        assert_eq!(files(&dir), [NEW], "{update:?}");
    }
    let unasked = judge(flickering_unasked, greets, &dir, updating(Update::Failing));
    assert!(!unasked.passed, "it changes, whether it says so or not");

    take(red, greets, &dir);
    let before = reference(&dir);
    let judged = judge(blue, at_once, &dir, updating(Update::All));
    assert!(!judged.passed, "no frame was left to tell it settled");
    assert_eq!(judged.frames, 0);
    assert!(
        judged.actual().ends_with("not settled within 0 frames"),
        "{}",
        judged.actual()
    );
    assert_eq!(reference(&dir), before);
}

#[test]
fn because_leads_the_failure_and_within_bounds_the_wait() {
    fn briefly(p: &mut Play<'_>) -> PlayResult {
        p.expect(Canvas)
            .because("the save is done")
            .within(3)
            .to_match_screenshot("greeting")
    }
    let dir = scratch("screenshot-because");
    let judged = judge(flickering, briefly, &dir, Snapshots::default());
    assert_eq!(judged.check.message.as_deref(), Some("the save is done"));
    assert_eq!(judged.frames, 3);
}

#[test]
fn force_retakes_any_difference_and_an_update_only_a_failing_one() {
    let dir = scratch("screenshot-force");
    take(grey_100, greets, &dir);
    let before = reference(&dir);
    let judged = judge(grey_125, greets, &dir, updating(Update::Failing));
    assert!(judged.passed, "{}", judged.panel);
    assert_eq!(reference(&dir), before, "a check that passes keeps its own");

    let forced = judge(grey_125, greets, &dir, updating(Update::All));
    assert!(forced.passed, "{}", forced.panel);
    assert_eq!(
        forced.actual(),
        "no pixel differs past the threshold; taken as the reference"
    );
    let again = judge(grey_125, greets, &dir, updating(Update::All));
    assert_eq!(again.actual(), "identical");
}

#[test]
fn force_takes_the_settled_frame_though_an_earlier_one_is_within_budget() {
    let dir = scratch("screenshot-force-settles");
    take(red, greets, &dir);
    take(grey_100, greets, &dir);
    let lenient = Snapshots {
        tolerance: u32::MAX,
        ..updating(Update::All)
    };
    let forced = judge(saving, greets, &dir, lenient);
    assert!(forced.passed, "{}", forced.panel);
    let done = judge(grey_125, greets, &dir, Snapshots::default());
    assert_eq!(done.actual(), "identical");
}

/// A menu of two items and a block of grey, which play [`opens_the_menu`] opens.
fn menu(ui: &mut egui::Ui, level: u8) {
    ui.menu_button("Menu", |ui| {
        _ = ui.button("Open");
        _ = ui.button("Close");
        block(ui, level);
    });
}

fn menu_in_grey_100(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
    menu(ui, 100);
}

fn menu_in_grey_200(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
    menu(ui, 200);
}

fn opens_the_menu(p: &mut Play<'_>) -> PlayResult {
    p.click("Menu")?;
    p.expect("Close").to_exist()?;
    p.expect(Canvas).to_match_screenshot("greeting")
}

#[test]
fn an_open_popup_is_compared_though_it_is_no_content() {
    let dir = scratch("screenshot-popup");
    take(menu_in_grey_100, opens_the_menu, &dir);
    let same = judge(menu_in_grey_100, opens_the_menu, &dir, Snapshots::default());
    assert!(same.passed, "{}", same.panel);
    let changed = judge(menu_in_grey_200, opens_the_menu, &dir, Snapshots::default());
    assert_eq!(
        changed.actual(),
        "1200 pixels differ, 0 allowed",
        "the whole of the menu's block"
    );
}

/// `Grow` adds rows, the last named `last`, past a 240 point canvas.
fn growing(ui: &mut egui::Ui, last: &str) {
    let id = ui.make_persistent_id("grown");
    let mut grown = ui.data_mut(|data| data.get_temp_mut_or_default::<bool>(id).to_owned());
    grown |= ui.button("Grow").clicked();
    if grown {
        for row in 0..30 {
            ui.label(format!("Row {row}"));
        }
        ui.label(last);
    }
    ui.data_mut(|data| data.insert_temp(id, grown));
}

fn grows_to_an_end(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
    growing(ui, "The end");
}

fn grows_to_a_fin(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
    growing(ui, "Fin, fin");
}

fn grows(p: &mut Play<'_>) -> PlayResult {
    p.click("Grow")?;
    p.expect(Canvas).to_match_screenshot("greeting")?;
    p.hover("Row 2")
}

#[test]
fn content_that_outgrows_the_canvas_is_compared_whole() {
    let dir = scratch("screenshot-grown");
    take(grows_to_an_end, grows, &dir);
    let image = image::load_from_memory(&reference(&dir)).expect("a PNG");
    assert!(image.height() > 400, "{} rows", image.height());
    let same = judge(grows_to_an_end, grows, &dir, Snapshots::default());
    assert!(same.passed, "{}", same.panel);
    let changed = judge(grows_to_a_fin, grows, &dir, Snapshots::default());
    assert!(!changed.passed, "the last row is another");
}

#[test]
fn a_scene_always_larger_than_its_canvas_fails_rather_than_growing_for_ever() {
    fn larger(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
        ui.set_min_height(ui.ctx().content_rect().height() + 10.0);
    }
    fn briefly(p: &mut Play<'_>) -> PlayResult {
        p.expect(Canvas).within(4).to_match_screenshot("greeting")
    }
    let dir = scratch("screenshot-larger");
    let judged = judge(larger, briefly, &dir, updating(Update::Failing));
    assert_eq!(
        judged.actual(),
        "the content outgrows its canvas; not settled within 4 frames"
    );
    assert_eq!(files(&dir), Vec::<String>::new());
}

/// An area of 200 points by 40, its left edge `from` the canvas's right one.
fn at_the_edge(ui: &mut egui::Ui, from: f32) {
    ui.label("Here");
    let corner = ui.ctx().content_rect().right_top() + egui::vec2(from, 0.0);
    egui::Area::new(egui::Id::new("drawer"))
        .fixed_pos(corner)
        .constrain(false)
        .show(ui.ctx(), |ui| ui.allocate_space(egui::vec2(200.0, 40.0)));
}

#[test]
fn a_drawer_parked_at_the_edge_is_out_of_sight() {
    fn parked(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
        at_the_edge(ui, 0.0);
    }
    let dir = scratch("screenshot-drawer");
    take(parked, greets, &dir);
    let image = image::load_from_memory(&reference(&dir)).expect("a PNG");
    assert!(image.width() < 320, "{} wide", image.width());
}

#[test]
fn a_drawer_hung_over_the_edge_fails_rather_than_being_chased_for_ever() {
    fn hung_over(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
        at_the_edge(ui, -20.0);
    }
    fn briefly(p: &mut Play<'_>) -> PlayResult {
        p.expect(Canvas).within(6).to_match_screenshot("greeting")
    }
    let dir = scratch("screenshot-drawer-over");
    let judged = judge(hung_over, briefly, &dir, updating(Update::All));
    assert_eq!(
        judged.actual(),
        "the content outgrows its canvas; not settled within 6 frames"
    );
    assert_eq!(files(&dir), Vec::<String>::new());
}

/// A dialog taller than a 240 point canvas, a block at its foot.
fn dialog(ui: &mut egui::Ui, level: u8) {
    egui::Modal::new(egui::Id::new("dialog")).show(ui.ctx(), |ui| {
        ui.add_space(400.0);
        block(ui, level);
    });
}

#[test]
fn a_dialog_taller_than_the_canvas_is_compared_whole() {
    fn in_grey_100(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
        dialog(ui, 100);
    }
    fn in_grey_200(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
        dialog(ui, 200);
    }
    let dir = scratch("screenshot-dialog");
    take(in_grey_100, greets, &dir);
    let same = judge(in_grey_100, greets, &dir, Snapshots::default());
    assert!(same.passed, "{}", same.panel);
    let changed = judge(in_grey_200, greets, &dir, Snapshots::default());
    assert_eq!(
        changed.actual(),
        "1200 pixels differ, 0 allowed",
        "the whole of the block at its foot"
    );
}

#[test]
fn a_popup_misplaced_for_one_frame_does_not_grow_the_canvas() {
    /// The canvas's width, which a canvas grown would change, and an area over it.
    fn area_at(ui: &mut egui::Ui, x: f32) {
        ui.label(format!("{} wide", ui.ctx().content_rect().width()));
        egui::Area::new(egui::Id::new("hops"))
            .fixed_pos(egui::pos2(x, 40.0))
            .constrain(false)
            .show(ui.ctx(), |ui| ui.allocate_space(egui::vec2(60.0, 20.0)));
    }
    fn still(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
        area_at(ui, 40.0);
    }
    fn hops(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
        let pass = ui.ctx().cumulative_pass_nr();
        area_at(ui, if pass == SETTLES_AT { 300.0 } else { 40.0 });
        if pass <= SETTLES_AT {
            ui.ctx().request_repaint();
        }
    }
    let dir = scratch("screenshot-hops");
    let hopped = judge(hops, greets, &dir, updating(Update::Failing));
    assert!(hopped.passed, "{}", hopped.panel);
    assert!(hopped.frames > 3, "it hopped while looked at");
    let same = judge(still, greets, &dir, Snapshots::default());
    assert_eq!(same.actual(), "identical");
}

#[test]
fn a_reference_that_is_no_image_fails_and_an_update_replaces_it() {
    let dir = scratch("screenshot-corrupt");
    take(red, greets, &dir);
    let path = refs(&dir).join(REFERENCE);
    std::fs::write(&path, "version https://git-lfs.github.com/spec/v1").expect("written");
    let judged = judge(red, greets, &dir, Snapshots::default());
    assert!(!judged.passed);
    assert!(
        judged.actual().starts_with("the reference can't be read: "),
        "{}",
        judged.actual()
    );
    assert_eq!(judged.attachments(), ["reference", "actual"]);
    take(red, greets, &dir);
    assert!(image::load_from_memory(&reference(&dir)).is_ok());
}

#[test]
fn a_reference_that_cannot_be_written_leaves_nothing_half_written() {
    let dir = scratch("screenshot-unwritable");
    let in_the_way = refs(&dir).join(REFERENCE);
    std::fs::create_dir_all(&in_the_way).expect("a directory where the reference goes");
    let judged = judge(red, greets, &dir, updating(Update::Failing));
    assert!(!judged.passed);
    assert!(judged.actual().contains("write `"), "{}", judged.actual());
    assert_eq!(judged.attachments(), Vec::<&str>::new());
    assert_eq!(files(&dir), [REFERENCE]);
}

#[test]
fn a_run_removes_what_one_cut_short_left_half_written() {
    let dir = scratch("screenshot-cut-short");
    take(red, greets, &dir);
    let [old, young, old_new, old_diff] = [
        (REFERENCE, "old"),
        (REFERENCE, "young"),
        (NEW, "old"),
        (DIFF, "old"),
    ]
    .map(|(of, name)| format!(".{of}.new.{name}"));
    let then = std::time::SystemTime::now() - std::time::Duration::from_mins(5);
    for name in [&old, &young, &old_new, &old_diff] {
        let file = std::fs::File::create(refs(&dir).join(name)).expect("written");
        if name != &young {
            file.set_modified(then).expect("aged");
        }
    }
    assert!(judge(red, greets, &dir, Snapshots::default()).passed);
    assert_eq!(files(&dir), [young, REFERENCE.to_owned()]);
}

#[test]
fn a_check_refused_leaves_nothing_of_an_earlier_run() {
    fn out_of_range(p: &mut Play<'_>) -> PlayResult {
        p.expect(Canvas)
            .to_match_screenshot(Screenshot::named("greeting").threshold(1.5))
    }
    let dir = scratch("screenshot-refused");
    take(red, greets, &dir);
    judge(blue, greets, &dir, Snapshots::default());
    assert_eq!(files(&dir), [DIFF, NEW, REFERENCE]);
    let judged = judge(red, out_of_range, &dir, Snapshots::default());
    assert!(!judged.passed);
    assert_eq!(files(&dir), [REFERENCE]);
}

#[test]
fn how_to_take_what_a_check_saw_comes_last_and_apart() {
    let dir = scratch("screenshot-offer");
    let judged = judge_filmed(
        red,
        greets,
        &dir,
        Snapshots::default(),
        Some(&Films::new(
            dir.join("out"),
            dir.join("recordings"),
            gallery_build::Video::Failing,
            "plays".to_owned(),
        )),
    );
    let lines: Vec<&str> = judged.panel.lines().map(str::trim).collect();
    let [.., video, "", offer] = lines.as_slice() else {
        panic!("{}", judged.panel);
    };
    assert!(video.contains("recorded in"), "{video}");
    assert!(offer.starts_with("`UPDATE_SNAPSHOTS=1` takes"), "{offer}");
}

#[cfg(unix)]
#[test]
fn a_passing_check_needs_no_writable_references() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = scratch("screenshot-read-only");
    take(red, greets, &dir);
    let refs = refs(&dir);
    std::fs::write(refs.join(NEW), "an earlier failure's").expect("written");
    let set = |mode| {
        std::fs::set_permissions(&refs, std::fs::Permissions::from_mode(mode)).expect("set");
    };
    set(0o555);
    let judged = judge(red, greets, &dir, Snapshots::default());
    set(0o755);
    assert!(judged.passed, "{}", judged.panel);
}

#[test]
fn a_name_checked_twice_in_one_play_fails_as_a_check() {
    fn twice(p: &mut Play<'_>) -> PlayResult {
        p.expect(Canvas).to_match_screenshot("Greeting!")?;
        p.expect(Canvas).to_match_screenshot("greeting")
    }
    let dir = scratch("screenshot-twice");
    let judged = judge(red, twice, &dir, updating(Update::Failing));
    assert!(!judged.passed);
    assert!(
        judged.actual().contains("already checked in this play"),
        "{}",
        judged.actual()
    );
    assert!(
        judged
            .panel
            .contains("taken as a reference before it failed"),
        "the first check's reference is named:\n{}",
        judged.panel
    );
}

#[test]
fn a_name_keeps_all_it_says_and_must_say_something() {
    fn alike_at_first(p: &mut Play<'_>) -> PlayResult {
        let long = "the settings as they are once the third tab is opened";
        p.expect(Canvas)
            .to_match_screenshot(format!("{long}: light"))?;
        p.expect(Canvas)
            .to_match_screenshot(format!("{long}: dark"))
    }
    fn says_nothing(p: &mut Play<'_>) -> PlayResult {
        p.expect(Canvas).to_match_screenshot("✓")
    }
    let dir = scratch("screenshot-names");
    take(red, alike_at_first, &dir);
    assert_eq!(files(&dir).len(), 2, "{:?}", files(&dir));
    let judged = judge(red, says_nothing, &dir, updating(Update::Failing));
    assert_eq!(
        judged.actual(),
        "\"✓\" has no letter or digit to name a reference by"
    );
}

#[test]
fn one_check_leaves_nothing_of_its_own_to_the_next() {
    fn idles_between(p: &mut Play<'_>) -> PlayResult {
        p.expect(Canvas).to_match_screenshot("greeting")?;
        p.idle(5)?;
        p.expect(Canvas).to_match_accesskit_snapshot("tree")
    }
    let dir = scratch("screenshot-next");
    take(red, greets, &dir);
    let judged = judge(red, idles_between, &dir, updating(Update::Failing));
    assert!(judged.passed, "{}", judged.panel);
    assert_eq!(
        judged.frames, 2,
        "the quiet frames before a check are not its own"
    );
}

#[test]
fn references_sit_beside_the_plays_file_wherever_the_check_is_called() {
    let dir = scratch("screenshot-beside");
    std::fs::create_dir_all(&dir).expect("made");
    let scenes = dir.join("form.scene.rs");
    std::fs::write(&scenes, "").expect("written");
    let (manifest, mut plays) = catalog(red, &[("greets", greets)]);
    plays[0].file = String::leak(scenes.into_string());
    plays[0].module_path = "catalog::widgets::form";
    let (outcome, log, _) = judged_shot(
        &manifest,
        &plays,
        Renderer::Glow,
        &greeting_shot(&dir),
        &updating(Update::Failing),
        None,
    );
    outcome.expect("taken");
    assert_eq!(log[0].outcome, Some(Outcome::Passed));
    assert_eq!(
        trace_files(&dir.join("__snapshots__/widgets/form/greets")),
        [REFERENCE]
    );
}

#[test]
fn update_snapshots_takes_the_values_egui_kittest_takes() {
    for (value, update) in [
        (None, Update::Off),
        (Some("0"), Update::Off),
        (Some("off"), Update::Off),
        (Some("1"), Update::Failing),
        (Some("yes"), Update::Failing),
        (Some("force"), Update::All),
    ] {
        assert_eq!(Update::parse(value), Ok(update), "{value:?}");
    }
    assert!(Update::parse(Some("please")).is_err());
}

#[test]
fn a_renderers_own_budget_outranks_the_catalogs() {
    let config = gallery_build::Snapshots {
        threshold: Some(0.2),
        tolerance: Some(10),
        wgpu: gallery_build::Budget {
            threshold: None,
            tolerance: Some(400),
        },
        glow: gallery_build::Budget::default(),
    };
    let wgpu = Snapshots::new(&config, Renderer::Wgpu, Update::Off);
    assert_eq!((wgpu.threshold, wgpu.tolerance), (0.2, 400));
    let glow = Snapshots::new(&config, Renderer::Glow, Update::Off);
    assert_eq!((glow.threshold, glow.tolerance), (0.2, 10));
}
