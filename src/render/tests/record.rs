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
                keep: Trace::On,
                format: VideoFormat::Apng,
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
    let video = &report["shots"][0]["video"];
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
    let video = &report["shots"][0]["video"];
    assert_eq!(
        (video["frames"].as_u64(), video["written"].as_u64()),
        (Some(5), Some(1))
    );
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
