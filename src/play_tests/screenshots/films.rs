//! Videos of catalog tests: which are kept, what they show,
//! and that a test comes to the same with one and without.

use std::sync::atomic::{AtomicU32, Ordering};

use eframe::egui_glow;
use gallery_build::Video;

use super::*;
use crate::video::{decode_mp4, mp4_frames};

/// Frames a step's end is held for: half a second of 15 a second, rounded up.
const HELD: u32 = 8;

/// Counts its clicks, and says what time it is.
fn counts(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
    let id = ui.make_persistent_id("clicks");
    let mut clicks = ui.data_mut(|data| data.get_temp_mut_or_default::<u32>(id).to_owned());
    if ui.button("Count").clicked() {
        clicks += 1;
    }
    ui.data_mut(|data| data.insert_temp(id, clicks));
    ui.label(format!("{clicks} clicked"));
    ui.label(format!("{:.3} s", ui.input(|input| input.time)));
}

fn counts_to_two(p: &mut Play<'_>) -> PlayResult {
    p.click("Count")?;
    p.click("Count")?;
    p.expect("2 clicked").to_exist()
}

fn counts_to_three(p: &mut Play<'_>) -> PlayResult {
    p.click("Count")?;
    p.expect("3 clicked").within(2).to_exist()
}

fn films(dir: &Utf8Path, video: Video) -> Films {
    std::fs::create_dir_all(dir).expect("made");
    films_of("plays", dir, video)
}

fn films_of(krate: &str, dir: &Utf8Path, video: Video) -> Films {
    Films::new(
        dir.join("out"),
        dir.join("recordings"),
        video,
        krate.to_owned(),
    )
}

/// A test of `run` on `renderer`, filmed into `dir/out`:
/// its failure, if any, and its log.
fn filmed_on(
    renderer: Renderer,
    scene: SceneFn,
    run: PlayFn,
    dir: &Utf8Path,
    video: Video,
) -> (Result<(), String>, Vec<crate::play::log::Entry>) {
    filmed_by(&films(dir, video), renderer, scene, run, dir)
}

/// [`filmed_on`], by `films`.
fn filmed_by(
    films: &Films,
    renderer: Renderer,
    scene: SceneFn,
    run: PlayFn,
    dir: &Utf8Path,
) -> (Result<(), String>, Vec<crate::play::log::Entry>) {
    let (manifest, plays) = catalog(scene, &[("greets", run)]);
    let (outcome, log, _) = judged_shot(
        &manifest,
        &plays,
        renderer,
        &greeting_shot(dir),
        &Snapshots::default(),
        Some(films),
    );
    (outcome.map_err(|failure| failure.plain()), log)
}

#[test]
fn a_catalog_without_the_input_panel_films_its_tests_without_one() {
    fn clicks(p: &mut Play<'_>) -> PlayResult {
        p.click("Count")
    }
    let last_frame = |name: &str, recording: crate::render::RecordDefaults| {
        let dir = scratch(name);
        let films = films(&dir, Video::Always).recorded_as(recording);
        let (outcome, _) = filmed_by(&films, Renderer::Glow, counts, clicks, &dir);
        outcome.unwrap_or_else(|failure| panic!("{name}: {failure}"));
        decode_mp4(&video(&dir)).pop().expect("a frame")
    };
    let shown = last_frame("films-panel", crate::render::RecordDefaults::default());
    let without = crate::render::RecordDefaults {
        input: None,
        ..crate::render::RecordDefaults::default()
    };
    let hidden = last_frame("films-no-panel", without);
    // The panel sits bottom-left, lit by the click, far from the button at the top.
    let (width, height) = shown.dimensions();
    let differing = (0..width / 3)
        .flat_map(|x| (height * 2 / 3..height).map(move |y| (x, y)))
        .filter(|&(x, y)| shown.get_pixel(x, y).0[0].abs_diff(hidden.get_pixel(x, y).0[0]) > 40)
        .count();
    assert!(
        differing > 100,
        "only the film with a panel shows one: {differing} pixels differ"
    );
}

#[test]
fn a_catalogs_hold_pauses_its_tests_videos() {
    let dir = scratch("films-catalog-hold");
    let catalog = crate::render::RecordDefaults {
        hold: Some(2.0),
        ..crate::render::RecordDefaults::default()
    };
    let films = films(&dir, Video::Always).recorded_as(catalog);
    let (outcome, log) = filmed_by(&films, Renderer::Glow, counts, counts_to_two, &dir);
    outcome.expect("it counts to two");
    let drawn = log.last().expect("a check").frames.end;
    assert_eq!(
        mp4_frames(&video(&dir)).1,
        1 + drawn + 2 * 30,
        "the frame the play began on, those it drew, and 2 s at 15 a second on each click's step"
    );
}

fn filmed(
    scene: SceneFn,
    run: PlayFn,
    dir: &Utf8Path,
    video: Video,
) -> (Result<(), String>, Vec<crate::play::log::Entry>) {
    filmed_on(Renderer::Glow, scene, run, dir, video)
}

/// A catalog's test of `run`, which writes no image.
fn checked(
    scene: SceneFn,
    run: PlayFn,
    dir: &Utf8Path,
    video: Video,
    warn: fn(&crate::diagnostic::Diagnostic),
) -> Result<(), String> {
    let mut shot = greeting_shot(dir);
    shot.out = None;
    let (manifest, plays) = catalog(scene, &[("greets", run)]);
    let session = crate::render::Session::open(Renderer::Glow).expect("a session");
    let testing = crate::render::Testing {
        snapshots: Snapshots::default(),
        films: films(dir, video),
        warn,
        styled: false,
    };
    let setup = |_: &egui::Context| {};
    crate::render::check_shot(&manifest, None, &plays, &session, &setup, &shot, &testing)
        .outcome(testing.styled)
}

fn video(dir: &Utf8Path) -> Utf8PathBuf {
    dir.join("out/plays/greets/video.glow.mp4")
}

#[test]
fn a_failing_play_keeps_its_video_and_the_panel_points_at_it() {
    for (renderer, name) in [(Renderer::Glow, "glow"), (Renderer::Wgpu, "wgpu")] {
        let dir = scratch("films-failing");
        let (outcome, log) = filmed_on(renderer, counts, counts_to_three, &dir, Video::Failing);
        let panel = outcome.expect_err("it counts to one");
        let video = dir.join(format!("out/plays/greets/video.{name}.mp4"));
        assert!(
            panel.contains(&format!("the play is recorded in {video}")),
            "{panel}"
        );
        let drawn = log.last().expect("a check").frames.end;
        assert_eq!(
            mp4_frames(&video),
            ((320, 240), 1 + drawn + HELD),
            "the frame the play began on, those it drew, and the pause on the click"
        );
        assert_eq!(
            trace_files(&dir.join("out")),
            [".gitignore", "CACHEDIR.TAG", "plays"]
        );
        assert_eq!(
            trace_files(&dir.join("out/plays/greets")),
            [format!("video.{name}.mp4")],
            "nothing is left beside it"
        );
        assert!(trace_files(&dir.join("recordings")).is_empty());
    }
}

#[test]
fn a_passing_play_keeps_its_video_only_when_every_one_is_kept() {
    let dir = scratch("films-passing");
    let (outcome, _) = filmed(counts, counts_to_two, &dir, Video::Failing);
    outcome.expect("it counts to two");
    assert!(!dir.join("out").exists());
    assert!(trace_files(&dir.join("recordings")).is_empty());

    let (outcome, _) = filmed(counts, counts_to_two, &dir, Video::Always);
    outcome.expect("it counts to two");
    assert!(video(&dir).exists());

    let (outcome, _) = filmed(counts, counts_to_two, &dir, Video::Failing);
    outcome.expect("it counts to two");
    assert_eq!(
        trace_files(&dir.join("out")),
        [".gitignore", "CACHEDIR.TAG"],
        "an earlier run's is not left either, nor its directories"
    );

    filmed(counts, counts_to_two, &dir, Video::Always)
        .0
        .expect("it counts to two");
    let (outcome, _) = filmed(counts, counts_to_three, &dir, Video::Off);
    let panel = outcome.expect_err("it counts to one");
    assert!(!panel.contains("recorded"), "{panel}");
    assert!(!video(&dir).exists(), "nor is it by a run without videos");
}

#[test]
fn a_play_leaves_what_is_not_its_own_video() {
    let dir = scratch("films-others");
    let (outcome, _) = filmed(counts, counts_to_three, &dir, Video::Failing);
    outcome.expect_err("it counts to one");
    let kept = dir.join("out/plays/greets");
    std::fs::write(kept.join("notes.txt"), "").expect("written");
    let (outcome, _) = filmed(counts, counts_to_two, &dir, Video::Failing);
    outcome.expect("it counts to two");
    assert_eq!(trace_files(&kept), ["notes.txt"]);
}

#[test]
fn renderers_and_crates_keep_their_videos_apart() {
    let dir = scratch("films-apart");
    for renderer in [Renderer::Glow, Renderer::Wgpu] {
        let (outcome, _) = filmed_on(renderer, counts, counts_to_three, &dir, Video::Failing);
        outcome.expect_err("it counts to one");
    }
    assert_eq!(
        trace_files(&dir.join("out/plays/greets")),
        ["video.glow.mp4", "video.wgpu.mp4"]
    );

    for (module, catalog_of, kept) in [
        ("plays::form", "plays", "plays/form/greets"),
        ("form", "plays", "plays/extern-form/greets"),
        ("other::form", "other", "other/form/greets"),
    ] {
        let (manifest, mut plays) = catalog(counts, &[("greets", counts_to_three)]);
        plays[0].module_path = module;
        let (outcome, ..) = judged_shot(
            &manifest,
            &plays,
            Renderer::Glow,
            &greeting_shot(&dir),
            &Snapshots::default(),
            Some(&films_of(catalog_of, &dir, Video::Failing)),
        );
        outcome.expect_err("it counts to one");
        let kept = dir.join("out").join(kept).join("video.glow.mp4");
        assert!(kept.exists(), "{kept}");
    }
}

#[test]
fn a_scene_that_fails_before_its_play_leaves_no_earlier_video() {
    fn too_tall(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
        ui.set_min_height(1.0e6);
    }
    let dir = scratch("films-too-tall");
    let (outcome, _) = filmed(counts, counts_to_three, &dir, Video::Failing);
    outcome.expect_err("it counts to one");
    let (outcome, _) = filmed(too_tall, counts_to_three, &dir, Video::Failing);
    outcome.expect_err("it can't be drawn");
    assert!(!video(&dir).exists());
}

#[test]
fn a_test_comes_to_the_same_with_a_video_and_without() {
    fn reads_the_time(p: &mut Play<'_>) -> PlayResult {
        p.click("Count")?;
        p.step("later", |p| p.idle(7))?;
        p.expect("never").within(3).to_exist()
    }
    let [off, always] = [Video::Off, Video::Always].map(|video| {
        let dir = scratch("films-alike");
        let (outcome, log) = filmed(counts, reads_the_time, &dir, video);
        let panel = outcome.expect_err("it never turns up");
        let told: Vec<String> = panel
            .lines()
            .filter(|line| !line.contains("recorded in"))
            .map(str::to_owned)
            .collect();
        let frames: Vec<_> = log.into_iter().map(|entry| entry.frames).collect();
        (told, frames)
    });
    let (told, _) = &off;
    assert!(
        told.iter()
            .any(|line| line.contains("`1 clicked`") && line.ends_with(" s`")),
        "the time is among the labels on screen: {told:#?}"
    );
    assert_eq!(off, always);
}

#[test]
fn a_play_that_fails_before_it_draws_shows_the_frame_it_failed_on() {
    fn at_once(p: &mut Play<'_>) -> PlayResult {
        p.expect("Nowhere").within(0).to_exist()
    }
    let dir = scratch("films-at-once");
    let (outcome, _) = filmed(counts, at_once, &dir, Video::Failing);
    outcome.expect_err("it is nowhere");
    assert_eq!(mp4_frames(&video(&dir)).1, 1);
}

/// Whether `frame` shows a pressed mouse button, by the colour it is lit in.
fn lit(frame: &image::RgbImage) -> bool {
    let lit = frame.pixels().filter(|pixel| {
        let [red, green, blue] = pixel.0;
        red > 200 && (150..230).contains(&green) && blue < 120
    });
    lit.count() > 50
}

#[test]
fn a_video_shows_the_input_from_the_frame_it_came_at() {
    fn late(p: &mut Play<'_>) -> PlayResult {
        p.idle(5)?;
        p.click("Count")?;
        p.expect("Nowhere").within(1).to_exist()
    }
    fn unmoving(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
        _ = ui.button("Count");
    }
    for scene in [counts as SceneFn, unmoving] {
        let dir = scratch("films-input");
        let (outcome, log) = filmed(scene, late, &dir, Video::Failing);
        outcome.expect_err("it is nowhere");
        let frames = decode_mp4(&video(&dir));
        let pressed = frames.iter().position(lit).expect("a lit button");
        assert_eq!(
            pressed,
            1 + 5 + 1,
            "after the frame the play began on, five idle ones and the pointer's move"
        );
        let clicked = 1 + log[0].frames.end + HELD;
        let after = &frames[usize::try_from(clicked).expect("a few frames")];
        assert!(!lit(after), "the pause on the click outlasts its light");
    }
}

/// Alike in shape on every frame, the image it shows apart.
fn retextures(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
    let id = ui.make_persistent_id("image");
    let texture = ui.data_mut(|data| data.get_temp::<egui::TextureHandle>(id));
    let mut texture = texture.unwrap_or_else(|| {
        let blank = egui::ColorImage::filled([8, 8], egui::Color32::BLACK);
        ui.ctx().load_texture("image", blank, Default::default())
    });
    let shade = u8::try_from(ui.ctx().cumulative_pass_nr() % 8 * 30).expect("below 256");
    let image = egui::ColorImage::filled([8, 8], egui::Color32::from_gray(shade));
    texture.set(image, Default::default());
    ui.image((texture.id(), egui::vec2(120.0, 120.0)));
    ui.data_mut(|data| data.insert_temp(id, texture));
}

#[test]
fn a_video_shows_a_change_its_shapes_do_not() {
    fn waits(p: &mut Play<'_>) -> PlayResult {
        p.idle(6)?;
        p.expect("Nowhere").within(0).to_exist()
    }
    let dir = scratch("films-retextured");
    let (outcome, _) = filmed(retextures, waits, &dir, Video::Failing);
    outcome.expect_err("it is nowhere");
    let shades: Vec<u8> = decode_mp4(&video(&dir))
        .iter()
        .map(|frame| frame.get_pixel(60, 60).0[0])
        .collect();
    assert_eq!(shades.len(), 7);
    assert!(
        shades.windows(2).all(|pair| pair[0].abs_diff(pair[1]) > 20),
        "each frame is of another shade than the last: {shades:?}"
    );
}

#[test]
fn only_a_frame_that_may_look_another_is_rendered() {
    fn still(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
        ui.label("Hello");
    }
    fn own_texture(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
        ui.image((egui::TextureId::User(7), egui::vec2(40.0, 40.0)));
    }
    fn feathers(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
        ui.label("Hello");
        let odd = ui.ctx().cumulative_pass_nr() % 2 == 1;
        ui.ctx()
            .options_mut(|options| options.tessellation_options.feathering = odd);
    }
    fn waits(p: &mut Play<'_>) -> PlayResult {
        p.idle(6)
    }
    fn points(p: &mut Play<'_>) -> PlayResult {
        p.hover("Hello")?;
        p.idle(6)
    }
    for (why, scene, run, rendered) in [
        ("nothing changes", still as SceneFn, waits as PlayFn, 1),
        ("the pointer moves once", still, points, 2),
        ("a texture is the scene's own", own_texture, waits, 7),
        ("the tessellation changes", feathers, waits, 7),
    ] {
        let dir = scratch("films-rendered");
        let before = crate::render::RENDERED.get();
        filmed(scene, run, &dir, Video::Failing)
            .0
            .expect("it passes");
        assert_eq!(crate::render::RENDERED.get() - before, rendered, "{why}");
    }
}

#[test]
fn a_paint_callback_is_rendered_on_every_frame_filmed() {
    static PAINTED: AtomicU32 = AtomicU32::new(0);
    fn calls_back(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
        ui.label("Hello");
        let rect = ui.max_rect();
        let callback = egui_glow::CallbackFn::new(|_, _| {
            PAINTED.fetch_add(1, Ordering::Relaxed);
        });
        ui.painter().add(egui::PaintCallback {
            rect,
            callback: std::sync::Arc::new(callback),
        });
    }
    fn waits(p: &mut Play<'_>) -> PlayResult {
        p.idle(6)
    }
    let dir = scratch("films-callback");
    let painted = |video| {
        PAINTED.store(0, Ordering::Relaxed);
        checked(calls_back, waits, &dir, video, |_| {}).expect("it passes");
        PAINTED.load(Ordering::Relaxed)
    };
    assert_eq!(
        painted(Video::Off),
        0,
        "a test without a video renders nothing"
    );
    assert_eq!(
        painted(Video::Failing),
        7,
        "the first frame and six idle ones"
    );
}

#[test]
fn a_plays_panics_are_kept_off_the_terminal_but_for_those_of_a_render() {
    /// What went on, and whether a panic in it would have been quiet.
    static WENT: std::sync::Mutex<Vec<(&str, bool)>> = std::sync::Mutex::new(Vec::new());
    fn note(what: &'static str) {
        let quiet = crate::play::is_quiet();
        WENT.lock().expect("the notes").push((what, quiet));
    }
    fn calls_back(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
        note("drawn");
        ui.label("Hello");
        let callback = egui_glow::CallbackFn::new(|_, _| note("rendered"));
        ui.painter().add(egui::PaintCallback {
            rect: ui.max_rect(),
            callback: std::sync::Arc::new(callback),
        });
    }
    fn steps(p: &mut Play<'_>) -> PlayResult {
        p.step("both", |p| {
            p.step("first", |p| p.idle(1))?;
            p.step("second", |p| p.idle(1))
        })?;
        p.expect("Nowhere").within(0).to_exist()
    }
    fn looks(p: &mut Play<'_>) -> PlayResult {
        p.step("look", greets)
    }
    for (render, run, filmed, out) in [
        ("a step's screenshot", steps as PlayFn, Video::Off, true),
        ("a snapshot", looks, Video::Off, false),
        ("a video's frame", steps, Video::Failing, false),
    ] {
        WENT.lock().expect("the notes").clear();
        let dir = scratch("films-quiet");
        let (manifest, plays) = catalog(calls_back, &[("greets", run)]);
        let mut shot = greeting_shot(&dir);
        shot.out = shot.out.filter(|_| out);
        let snapshots = Snapshots {
            dir: Some(dir.join("refs")),
            ..Snapshots::default()
        };
        let films = films(&dir, filmed);
        let glow = Renderer::Glow;
        judged_shot(&manifest, &plays, glow, &shot, &snapshots, Some(&films))
            .0
            .expect_err("it fails");
        let went = WENT.lock().expect("the notes");
        let first = went.iter().position(|(what, _)| *what == "rendered");
        let after = &went[first.expect("a render")..];
        assert!(
            after.iter().any(|(what, _)| *what == "drawn"),
            "{render}: {after:?}"
        );
        for (what, quiet) in after {
            assert_eq!(*quiet, *what == "drawn", "{render}: {after:?}");
        }
    }
}

#[test]
fn a_video_grows_with_its_canvas() {
    fn clicks_once_grown(p: &mut Play<'_>) -> PlayResult {
        p.click("Grow")?;
        _ = p.expect(Canvas).to_match_screenshot("grown");
        p.click("Grow")?;
        p.expect("Nowhere").within(1).to_exist()
    }
    let dir = scratch("films-grown");
    let judged = judge_filmed(
        grows_to_an_end,
        clicks_once_grown,
        &dir,
        Snapshots::default(),
        Some(&films(&dir, Video::Failing)),
    );
    assert!(!judged.passed);
    assert!(judged.panel.contains("recorded in"), "{}", judged.panel);
    let frames = decode_mp4(&video(&dir));
    assert!(frames[0].height() > 400, "{}", frames[0].height());
    let pressed = frames.iter().map(lit);
    let presses = pressed
        .clone()
        .zip(pressed.skip(1))
        .filter(|(before, now)| !before && *now);
    assert_eq!(
        presses.count(),
        2,
        "before the canvas grew, and on it grown"
    );
}

#[test]
fn a_canvas_past_what_a_video_can_be_says_so() {
    fn tall(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
        ui.set_min_height(4_000.0);
    }
    let dir = scratch("films-too-large");
    let (outcome, _) = filmed(tall, counts_to_three, &dir, Video::Failing);
    let panel = outcome.expect_err("nothing to click");
    assert!(
        panel.contains("the play is not recorded: ") && panel.contains("is past the 3840×2160"),
        "{panel}"
    );
    let (outcome, _) = filmed(tall, |p| p.idle(1), &dir, Video::Failing);
    outcome.expect("its play passes");
}

#[test]
fn a_canvas_that_turns_on_its_side_past_what_a_video_can_be_says_so() {
    /// Wide, then tall: neither too large, but both together.
    fn turns(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
        let wide = !ui.button("Turn").clicked();
        let size = if wide {
            (3_000.0, 500.0)
        } else {
            (500.0, 3_000.0)
        };
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::InnerSize(size.into()));
    }
    fn turns_it(p: &mut Play<'_>) -> PlayResult {
        p.click("Turn")?;
        p.expect("Nowhere").within(2).to_exist()
    }
    let dir = scratch("films-turned");
    let (outcome, _) = filmed(turns, turns_it, &dir, Video::Failing);
    let panel = outcome.expect_err("it is nowhere");
    assert!(
        panel.contains("the play is not recorded: 3000×3000 is past the 3840×2160"),
        "{panel}"
    );
}

#[test]
fn a_zoomed_scenes_video_shows_the_pointer_where_it_is() {
    fn zoomed(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
        ui.ctx().set_zoom_factor(2.0);
        ui.add_space(40.0);
        ui.horizontal(|ui| {
            ui.add_space(80.0);
            ui.label("Hello");
        });
    }
    static AIMED: std::sync::Mutex<egui::Pos2> = std::sync::Mutex::new(egui::Pos2::ZERO);
    fn presses(p: &mut Play<'_>) -> PlayResult {
        *AIMED.lock().expect("the aim") = p.get("Hello").rect.center();
        p.idle(1)?;
        p.click("Hello")?;
        p.expect("Nowhere").within(0).to_exist()
    }
    let dir = scratch("films-zoomed");
    let (outcome, _) = filmed(zoomed, presses, &dir, Video::Failing);
    outcome.expect_err("it is nowhere");
    let frames = decode_mp4(&video(&dir));
    let [before, .., after] = frames.as_slice() else {
        panic!("{} frames", frames.len());
    };
    let aimed = *AIMED.lock().expect("the aim") * 2.0;
    // The panel lights the button too, in the bottom left corner, far below.
    let (width, height) = before.dimensions();
    let panel = |x: u32, y: u32| x < width / 3 && y > height * 2 / 3;
    let strays = before.enumerate_pixels().filter(|(x, y, pixel)| {
        let moved = pixel.0[0].abs_diff(after.get_pixel(*x, *y).0[0]) > 60;
        let off = aimed.distance(egui::pos2(*x as f32, *y as f32));
        !panel(*x, *y) && moved && off > 40.0
    });
    let near = |x: f32, y: f32| {
        after
            .get_pixel((aimed.x + x) as u32, (aimed.y + y) as u32)
            .0[0]
    };
    assert_eq!(strays.count(), 0, "nothing is drawn away from the pointer");
    assert!(
        near(5.0, 5.0) > 200,
        "its pressed cursor, lit inside, is on it"
    );
}

#[test]
fn an_earlier_video_that_cannot_be_removed_is_warned_of() {
    let dir = scratch("films-immovable");
    std::fs::create_dir_all(video(&dir)).expect("a directory where the video goes");
    let (manifest, plays) = catalog(counts, &[("greets", counts_to_two)]);
    let (outcome, _, warnings) = judged_shot(
        &manifest,
        &plays,
        Renderer::Glow,
        &greeting_shot(&dir),
        &Snapshots::default(),
        Some(&films(&dir, Video::Failing)),
    );
    outcome.expect("it counts to two");
    let [warning] = warnings.as_slice() else {
        panic!("{warnings:?}");
    };
    assert!(
        warning.plain().contains("remove the earlier"),
        "{warning:?}"
    );
}

#[test]
fn a_catalogs_test_reports_what_it_is_warned_of() {
    thread_local! {
        static WARNED: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
    }
    let dir = scratch("films-warned");
    std::fs::create_dir_all(video(&dir)).expect("a directory where the video goes");
    checked(counts, counts_to_two, &dir, Video::Failing, |warning| {
        WARNED.with_borrow_mut(|warned| warned.push(warning.plain()));
    })
    .expect("it counts to two");
    let warned = WARNED.take();
    assert!(
        matches!(&warned[..], [one] if one.contains("remove the earlier")),
        "{warned:?}"
    );
}

/// Shows nothing where `/dev/shm` and the temporary files share a file system.
#[cfg(target_os = "linux")]
#[test]
fn a_video_recorded_on_another_file_system_is_kept() {
    let dir = scratch("films-across");
    let elsewhere = tempfile::tempdir_in("/dev/shm").expect("a directory");
    let recordings = Utf8Path::from_path(elsewhere.path()).expect("UTF-8");
    let krate = "plays".to_owned();
    let films = Films::new(dir.join("out"), recordings.to_owned(), Video::Always, krate);
    let (manifest, plays) = catalog(counts, &[("greets", counts_to_two)]);
    let (outcome, _, warnings) = judged_shot(
        &manifest,
        &plays,
        Renderer::Glow,
        &greeting_shot(&dir),
        &Snapshots::default(),
        Some(&films),
    );
    outcome.expect("it counts to two");
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(mp4_frames(&video(&dir)).0, (320, 240));
}

#[test]
fn a_video_that_cannot_be_made_fails_no_play() {
    for in_the_way in ["out", "recordings"] {
        let dir = scratch("films-in-the-way");
        std::fs::create_dir_all(&dir).expect("made");
        let file = dir.join(in_the_way);
        std::fs::write(&file, "a file where the directory goes").expect("written");
        for (video, warned) in [(Video::Always, 1), (Video::Failing, 0)] {
            let (manifest, plays) = catalog(counts, &[("greets", counts_to_two)]);
            let (outcome, _, warnings) = judged_shot(
                &manifest,
                &plays,
                Renderer::Glow,
                &greeting_shot(&dir),
                &Snapshots::default(),
                Some(&films(&dir, video)),
            );
            outcome.expect("it counts to two");
            assert_eq!(warnings.len(), warned, "{warnings:?}");
        }
        let (outcome, _) = filmed(counts, counts_to_three, &dir, Video::Always);
        let panel = outcome.expect_err("it counts to one");
        assert!(
            panel.contains(&format!("the play is not recorded: create `{file}`")),
            "{panel}"
        );
    }
}
