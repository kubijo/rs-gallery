//! The HTML report of catalog tests:
//! what it says of a play, and what it links to.

use std::sync::{Arc, Mutex};

use gallery_build::Video;

use super::*;
use crate::{
    diagnostic::Diagnostic,
    render::{Checked, Testing},
    verdicts::{PAGE, Results, Tested, Trial},
};

/// Test `run` on `scene` as a catalog's tests do,
/// its report in `dir/out`; what it came to.
fn reported(
    scene: SceneFn,
    run: PlayFn,
    dir: &Utf8Path,
    video: Video,
) -> (Checked, serde_json::Value) {
    std::fs::create_dir_all(dir).expect("made");
    let (manifest, plays) = catalog(scene, &[("greets", run)]);
    let warn: fn(&Diagnostic) = |warning| panic!("{}", warning.plain());
    let output = dir.join("out");
    let krate = "plays".to_owned();
    let testing = Testing {
        snapshots: Snapshots {
            dir: Some(dir.join("refs")),
            ..Snapshots::default()
        },
        films: Films::new(output.clone(), dir.join("recordings"), video, krate.clone()),
        warn,
        styled: false,
    };
    let name = "play::plays::scene::greets";
    let tested = Tested {
        output: output.clone(),
        krate,
        manifest_dir: dir.to_owned(),
        renderer: "glow",
    };
    let manifest_of_plays = "[package]\nname = \"plays\"\n";
    std::fs::write(dir.join("Cargo.toml"), manifest_of_plays).expect("written");
    let warned = Arc::new(Mutex::new(Vec::new()));
    let warnings = Arc::clone(&warned);
    let results = Results::new(tested, [name.to_owned()], move |warning: &Diagnostic| {
        warnings.lock().expect("no panic").push(warning.plain());
    });
    let session = crate::render::Session::open(Renderer::Glow).expect("a session");
    let mut shot = greeting_shot(dir);
    shot.out = None;
    let trial = Trial {
        name,
        scene: "plays::scene",
        play: Some(&plays[0]),
    };
    let setup = |_: &egui::Context| {};
    let checked = results.of(trial, || {
        crate::render::check_shot(&manifest, None, &plays, &session, &setup, &shot, &testing)
    });
    assert_eq!(results.close(), Some(output.join(PAGE)));
    assert_eq!(*warned.lock().expect("no panic"), Vec::<String>::new());
    let file = output.join("plays/results-glow/play/plays/scene/greets.json");
    let json = std::fs::read(file).expect("the result");
    (checked, serde_json::from_slice(&json).expect("JSON"))
}

/// Everything the report in `dir/out` links to or shows.
fn links(dir: &Utf8Path) -> Vec<String> {
    let page = std::fs::read_to_string(dir.join("out").join(PAGE)).expect("the report");
    let mut links: Vec<String> = [" href=\"", " src=\""]
        .iter()
        .flat_map(|attribute| page.split(attribute).skip(1))
        .map(|rest| rest.split('"').next().unwrap_or_default().to_owned())
        .filter(|link| !link.starts_with('#') && !link.starts_with("data:image/svg+xml,"))
        .collect();
    links.sort();
    links.dedup();
    links
}

fn presses_then_compares(p: &mut Play<'_>) -> PlayResult {
    p.press("Tab")?;
    p.press("Tab")?;
    greets(p)
}

#[test]
fn a_failed_comparison_is_reported_with_its_images_and_every_link_is_to_the_output() {
    let dir = scratch("reports-differs");
    std::fs::create_dir_all(&dir).expect("made");
    take(red, presses_then_compares, &dir);
    let (checked, result) = reported(blue, presses_then_compares, &dir, Video::Failing);
    let told = checked.outcome(false).expect_err("it is another colour");
    assert!(told.contains("✓ press Tab"), "its log follows: {told}");
    assert_eq!(result["verdict"], "failed", "{result:#}");

    let check = result["log"].as_array().and_then(|log| log.last());
    let check = check.expect("the check");
    let names: Vec<_> = check["files"]
        .as_array()
        .expect("files")
        .iter()
        .map(|file| file["name"].as_str().expect("a name"))
        .collect();
    assert_eq!(names, ["reference", "actual", "diff"]);
    let failure = result["failure"].as_str().expect("a failure");
    assert!(failure.contains("UPDATE_SNAPSHOTS=1"), "{failure}");
    assert!(
        told.contains("is recorded in") && !failure.contains("is recorded in"),
        "the report shows its video, which is nowhere in there for whoever else reads it: {failure}"
    );
    let links = links(&dir);
    assert!(links.len() >= 4, "three images and the video: {links:?}");
    for link in &links {
        let file = link.split('#').next().expect("a path");
        assert!(
            !file.contains("..") && !file.starts_with('/') && !file.contains(':'),
            "{file}"
        );
        assert!(dir.join("out").join(file).is_file(), "{file}");
    }
}

#[test]
fn a_line_of_the_log_says_where_the_video_shows_it_begin() {
    let dir = scratch("reports-seek");
    std::fs::create_dir_all(&dir).expect("made");
    take(red, presses_then_compares, &dir);
    let (checked, result) = reported(blue, presses_then_compares, &dir, Video::Failing);
    let log = result["log"].as_array().expect("the log");
    let seeks: Vec<f64> = log
        .iter()
        .map(|line| line["seek"].as_f64().expect("somewhere"))
        .collect();

    // The frame it began with, then each press's last,
    // at the end of the pause of 8 frames after it.
    let frames: Vec<u32> = checked.log.iter().map(|entry| entry.frames.end).collect();
    let at = |frame: u32| (f64::from(frame) + 0.5) / 15.0;
    let expected = [at(0), at(frames[0] + 8), at(frames[1] + 16)];
    for (seek, expected) in seeks.iter().zip(expected) {
        assert!((seek - expected).abs() < 1e-9, "{seeks:?}, {expected}");
    }
    let page = std::fs::read_to_string(dir.join("out").join(PAGE)).expect("the report");
    assert_eq!(page.matches("class=\"seek\"").count(), 3);
}

#[test]
fn a_passing_play_is_reported_with_its_log_and_its_reference() {
    let dir = scratch("reports-passing");
    std::fs::create_dir_all(&dir).expect("made");
    take(red, greets, &dir);
    let (checked, result) = reported(red, greets, &dir, Video::Failing);
    checked.outcome(false).expect("it is alike");
    assert_eq!(result["verdict"], "passed", "{result:#}");
    assert_eq!(result["video"], serde_json::Value::Null);
    assert_eq!(result["log"][0]["outcome"], "passed");
    assert_eq!(result["log"][0]["files"][0]["name"], "reference");
    assert_eq!(result["log"][0]["seek"], serde_json::Value::Null);

    let (_, result) = reported(red, greets, &dir, Video::Always);
    assert_eq!(result["video"], "plays/greets/video.glow.mp4");
}

#[cfg(unix)]
#[test]
fn a_crates_directory_that_is_a_link_has_its_videos_and_snapshots_in_the_report() {
    let dir = scratch("reports-linked");
    for made in ["out", "elsewhere"] {
        std::fs::create_dir_all(dir.join(made)).expect("made");
    }
    std::os::unix::fs::symlink(dir.join("elsewhere"), dir.join("out/plays")).expect("linked");
    take(red, greets, &dir);
    let (_, result) = reported(red, greets, &dir, Video::Always);
    assert_eq!(result["video"], "plays/greets/video.glow.mp4");
    let links = links(&dir);
    let files = links.iter().map(|link| link.split('#').next());
    let mut files: Vec<&str> = files.map(|file| file.expect("a path")).collect();
    files.dedup();
    assert_eq!(files.len(), 2, "its reference and its video: {files:?}");
    for file in files {
        assert!(dir.join("out").join(file).is_file(), "{file}");
    }
}

#[test]
fn a_play_leaves_no_snapshots_of_an_earlier_run() {
    fn says_nothing(p: &mut Play<'_>) -> PlayResult {
        p.idle(1)
    }
    let dir = scratch("reports-stale");
    std::fs::create_dir_all(&dir).expect("made");
    take(red, greets, &dir);
    reported(blue, greets, &dir, Video::Off);
    let snapshots = dir.join("out/plays/greets/snapshots-glow");
    assert_eq!(trace_files(&snapshots).len(), 3);
    reported(blue, says_nothing, &dir, Video::Off);
    assert!(!snapshots.exists());
}

#[test]
fn a_tree_that_changed_is_reported_with_its_lines() {
    fn reads(p: &mut Play<'_>) -> PlayResult {
        p.expect(crate::Canvas)
            .to_match_accesskit_snapshot("greeting")
    }
    fn says(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
        ui.label("Hello");
    }
    fn says_other(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
        ui.label("Goodbye");
    }
    let dir = scratch("reports-tree");
    std::fs::create_dir_all(&dir).expect("made");
    take(says, reads, &dir);
    reported(says_other, reads, &dir, Video::Off);
    let page = std::fs::read_to_string(dir.join("out").join(PAGE)).expect("the report");
    assert!(page.contains("<span class=\"removed\">-"), "{page}");
    assert!(page.contains("<span class=\"added\">+"), "{page}");
    assert!(page.contains("Goodbye"), "{page}");
}

#[test]
fn a_file_that_cannot_be_copied_is_warned_of_in_its_result() {
    let dir = scratch("reports-no-copy");
    std::fs::create_dir_all(dir.join("out/plays/greets")).expect("made");
    let in_the_way = dir.join("out/plays/greets/snapshots-glow");
    std::fs::write(in_the_way, "a file where the directory goes").expect("written");
    take(red, greets, &dir);
    let (checked, result) = reported(red, greets, &dir, Video::Off);
    checked.outcome(false).expect("it is alike");
    assert_eq!(result["verdict"], "passed");
    let warning = result["warnings"][0].as_str().expect("a warning");
    assert!(warning.contains("snapshots-glow"), "{warning}");
}
