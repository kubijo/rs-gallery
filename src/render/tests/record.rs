use super::*;
use crate::video::decode;

/// A bar `40 × time` points wide,
/// so a frame's bar says when it was drawn.
fn clock_bar(_: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
    ui.ctx().request_repaint();
    let time = ui.input(|i| i.time);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(200.0, 20.0), egui::Sense::hover());
    #[expect(clippy::cast_possible_truncation, reason = "a few seconds of clock")]
    let width = (time * 40.0) as f32;
    ui.painter().rect_filled(
        egui::Rect::from_min_size(rect.min, egui::vec2(width, 20.0)),
        0.0,
        egui::Color32::RED,
    );
}

/// Nothing that moves, and no frame asked for.
fn still(_: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
    ui.label("still");
}

fn manifest(render: fn(&mut crate::SceneCtx<'_>, &mut egui::Ui)) -> Manifest {
    Manifest {
        scenes: vec![SceneEntry {
            render,
            name: "recorded",
            module_path: "reference",
            default: true,
            order: 0,
            source: "",
        }],
        groups: Vec::new(),
    }
}

/// Record `seconds` of the one scene in `manifest`
/// at `fps`; the shot's PNG, and the run's report.
fn recorded(
    name: &str,
    manifest: &Manifest,
    fps: u16,
    seconds: f32,
) -> (Utf8PathBuf, serde_json::Value) {
    let dir = Utf8PathBuf::from_path_buf(std::env::temp_dir())
        .expect("a UTF-8 temp dir")
        .join("gallery-record")
        .join(name);
    _ = std::fs::remove_dir_all(&dir);
    let out = dir.join("shot.png");
    let report = dir.join("capture.json");
    let capture = Capture {
        shots: vec![Shot {
            scene: "recorded".to_owned(),
            out: Some(out.clone()),
            size: egui::vec2(240.0, 60.0),
            knobs: Overrides::default(),
            globals: Overrides::default(),
            frames: None,
            trim: true,
            settle: false,
            scale: DEFAULT_SCALE,
            list: false,
            template: false,
            play: None,
            record: Some(Record {
                fps,
                seconds: Some(seconds),
                keep: Trace::Always,
                formats: VideoFormat::Apng.into(),
                hold: None,
                input: None,
            }),
        }],
        sheet: None,
        report: Some(report.clone()),
    };
    render(manifest, Renderer::Glow, &|_: &egui::Context| {}, &capture)
        .unwrap_or_else(|failure| panic!("{}", failure.plain()));
    let json = serde_json::from_slice(&std::fs::read(&report).expect("the report")).expect("JSON");
    (out, json)
}

/// The widest run of red in any row: the bar's width in pixels.
fn bar(image: &image::RgbaImage) -> usize {
    image
        .rows()
        .map(|row| {
            row.filter(|p| p[0] > 128 && p[1] < 100 && p[2] < 100)
                .count()
        })
        .max()
        .unwrap_or(0)
}

#[cfg(not(target_vendor = "apple"))]
#[test]
fn each_recorded_frame_is_the_scene_at_exactly_its_time() {
    let (out, report) = recorded("clock", &manifest(clock_bar), 10, 1.0);
    let (declared, shown) = decode(&video_path(&out, VideoFormat::Apng));

    assert_eq!(declared, 10, "a second at 10 fps, every frame different");
    assert_eq!(shown.len(), 10);
    assert!(
        shown.iter().all(|frame| frame.delay == (1, 10)),
        "each frame shows for exactly a tenth of a second"
    );
    // 40 points a second, so a tenth of a second apart is 4 pixels apart.
    let widths: Vec<_> = shown.iter().map(|frame| bar(&frame.image)).collect();
    assert!(
        widths.windows(2).all(|pair| pair[1] == pair[0] + 4),
        "frame n is drawn at n / fps: {widths:?}"
    );
    let video = &report["shots"][0]["videos"][0];
    assert_eq!(video["frames"], 10);
    assert_eq!(video["written"], 10);
    assert_eq!(video["fps"], 10);
}

#[cfg(not(target_vendor = "apple"))]
#[test]
fn a_recording_of_a_still_scene_is_one_frame_held() {
    let (out, report) = recorded("still", &manifest(still), 10, 0.5);
    let (declared, shown) = decode(&video_path(&out, VideoFormat::Apng));
    assert_eq!((declared, shown.len()), (1, 1));
    assert_eq!(
        shown[0].delay,
        (5, 10),
        "the five frames drawn, merged into one"
    );
    let video = &report["shots"][0]["videos"][0];
    assert_eq!(
        (video["frames"].as_u64(), video["written"].as_u64()),
        (Some(5), Some(1))
    );
}

/// What a shot recorded in both formats came to.
#[cfg(not(target_vendor = "apple"))]
struct Recorded {
    dir: Utf8PathBuf,
    _scratch: tempfile::TempDir,
    outcome: Result<(), String>,
    /// The play's log as a terminal without redrawing shows it.
    log: String,
}

/// Record a canvas of 8×8 pixels, where OpenH264 takes 16×16 and up: the APNG is written
/// and the MP4 lost, which its encoder may not tell before the recording is closed.
#[cfg(not(target_vendor = "apple"))]
fn losing_the_mp4(
    render: fn(&mut crate::SceneCtx<'_>, &mut egui::Ui),
    play: Option<fn(&mut crate::Play<'_>) -> crate::PlayResult>,
    seconds: f32,
) -> Recorded {
    let scratch = tempfile::tempdir().expect("a directory");
    let dir = Utf8PathBuf::from_path_buf(scratch.path().to_owned()).expect("UTF-8");
    let shot = Shot {
        scene: "recorded".to_owned(),
        out: Some(dir.join("shot.png")),
        size: egui::vec2(32.0, 32.0),
        knobs: Overrides::default(),
        globals: Overrides::default(),
        frames: None,
        trim: false,
        settle: false,
        scale: 0.25,
        list: false,
        template: false,
        play: play.map(|_| ShotPlay {
            name: "plays".to_owned(),
            trace: Trace::Failing,
            steps: false,
        }),
        record: Some(Record {
            fps: 10,
            seconds: Some(seconds),
            keep: Trace::Failing,
            formats: VideoFormats::of([VideoFormat::Apng, VideoFormat::Mp4]).expect("two"),
            hold: Some(0.0),
            input: None,
        }),
    };
    let plays: Vec<_> = play
        .map(|run| PlayEntry {
            scene: crate::SceneKey {
                module_path: "reference",
                name: "recorded",
            },
            name: "plays",
            module_path: "reference",
            file: file!(),
            run,
        })
        .into_iter()
        .collect();
    let (mut view, written) = log_view::view(LogMode::Linear, 24);
    let mut watch = Watch::new(&mut view);
    let setup = |_: &egui::Context| {};
    let scenes = manifest(render);
    let outcome = shoot_with_globals(&scenes, None, &plays, &glow(), &setup, &shot, &mut watch);
    Recorded {
        dir,
        _scratch: scratch,
        outcome: outcome.map(drop).map_err(|failure| failure.plain()),
        log: written.text(),
    }
}

#[cfg(not(target_vendor = "apple"))]
impl Recorded {
    /// That it failed for the MP4 and kept the APNG, in a panel that says `recorded`.
    #[track_caller]
    fn kept_the_apng_alone(&self, recorded: &str) -> &str {
        let panel = self.outcome.as_ref().expect_err("the MP4 is lost");
        let path = |name: &str| self.dir.join(name);
        assert_eq!(panel.matches("shot.mp4`").count(), 1, "{panel}");
        assert!(
            panel.contains(&format!("{recorded} {}", path("shot.apng"))),
            "{panel}"
        );
        assert!(!path("shot.mp4").exists());
        assert!(!decode(&path("shot.apng")).1.is_empty());
        panel
    }
}

/// Draws nothing, so its canvas stays as small as asked.
#[cfg(not(target_vendor = "apple"))]
fn blank(_: &mut crate::SceneCtx<'_>, _: &mut egui::Ui) {}

#[cfg(not(target_vendor = "apple"))]
#[test]
fn a_play_that_loses_one_recording_fails_and_lists_the_other() {
    let lost = losing_the_mp4(blank, Some(|p| p.idle(1)), 0.0);
    let panel = lost.kept_the_apng_alone("the play is recorded in");
    assert!(
        panel.contains("play `plays` on `recorded` failed"),
        "{panel}"
    );
    assert!(
        lost.log.contains("× failed outside any step"),
        "its log says so too: {}",
        lost.log
    );
}

#[cfg(not(target_vendor = "apple"))]
#[test]
fn a_play_that_fails_says_which_recording_it_also_lost() {
    let fails: fn(&mut crate::Play<'_>) -> crate::PlayResult = |p| {
        p.idle(1)?;
        Err("its own failure".into())
    };
    let lost = losing_the_mp4(blank, Some(fails), 0.0);
    let panel = lost.kept_the_apng_alone("the play is recorded in");
    assert!(panel.contains("its own failure"), "{panel}");
    assert!(panel.contains("not recorded: write"), "{panel}");
}

#[cfg(not(target_vendor = "apple"))]
#[test]
fn a_shot_that_loses_one_recording_fails_and_points_at_the_other() {
    let lost = losing_the_mp4(blank, None, 0.1);
    lost.kept_the_apng_alone("the shot is recorded in");
}

#[cfg(not(target_vendor = "apple"))]
#[test]
fn a_shot_that_fails_as_it_is_recorded_says_so_before_what_it_lost() {
    thread_local! {
        static LEFT: std::cell::Cell<u32> = const { std::cell::Cell::new(u32::MAX) };
    }
    fn panics(ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
        crate::__run_scene(ctx, ui, "recorded", |_, _| {
            let left = LEFT.get();
            assert_ne!(left, 0, "its own failure");
            LEFT.set(left - 1);
        });
    }
    let one_frame = losing_the_mp4(panics, None, 0.1);
    one_frame.kept_the_apng_alone("the shot is recorded in");
    let drawn = u32::MAX - LEFT.get();
    LEFT.set(drawn);

    let two_frames = losing_the_mp4(panics, None, 0.2);
    let panel = two_frames.outcome.expect_err("the second panics");
    let [own, mp4] = ["its own failure", "not recorded: write"].map(|said| panel.find(said));
    assert!(own.is_some() && own < mp4, "{panel}");
}

#[cfg(not(target_vendor = "apple"))]
#[test]
fn two_recordings_of_one_shot_are_the_same_bytes() {
    let scenes = manifest(clock_bar);
    let (first, _) = recorded("twice-a", &scenes, 20, 0.5);
    let (second, _) = recorded("twice-b", &scenes, 20, 0.5);
    assert_eq!(
        std::fs::read(video_path(&first, VideoFormat::Apng)).expect("the first recording"),
        std::fs::read(video_path(&second, VideoFormat::Apng)).expect("the second recording"),
    );
}
