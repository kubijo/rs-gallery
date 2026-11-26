use super::*;

#[test]
fn a_byte_count_is_stated_in_the_largest_unit_it_fills() {
    assert_eq!(human(0), "0 B");
    assert_eq!(human(1023), "1023 B");
    assert_eq!(human(1024), "1.0 KiB");
    assert_eq!(human(1_572_864), "1.5 MiB");
}

#[test]
fn a_report_lines_the_sizes_up_however_long_the_paths_before_them_are() {
    let written = [
        OutputFile {
            path: "/tmp/a.png".into(),
            size: Size {
                width: 640,
                height: 360,
            },
            scale: DEFAULT_SCALE,
            bytes: 1,
            globals: BTreeMap::new(),
            frames: Frames {
                drawn: 1,
                motion: Motion::Settled,
            },
            play: None,
            videos: Vec::new(),
        },
        OutputFile {
            path: "/tmp/a-much-longer-name.png".into(),
            size: Size {
                width: 1280,
                height: 720,
            },
            scale: DEFAULT_SCALE,
            bytes: 1,
            globals: BTreeMap::new(),
            frames: Frames {
                drawn: 1,
                motion: Motion::Settled,
            },
            play: None,
            videos: Vec::new(),
        },
    ];
    let ran = Ran {
        order: &[Slot::Written(0), Slot::Written(1)],
        written: &written,
        failed: &[],
        ended: false,
        findings: &[],
        sheet: None,
    };
    let table = crate::style::plain(&table(ran, style::Stream::Stdout));
    let columns: Vec<_> = table
        .lines()
        .filter(|line| line.contains('×'))
        .map(|row| row.chars().take_while(|c| *c != '×').count())
        .collect();
    assert_eq!(columns.len(), 2, "a row per image:\n{table}");
    assert_eq!(columns[0], columns[1], "the sizes line up:\n{table}");
}

#[test]
fn a_summary_has_no_empty_column_and_says_how_long_a_recording_is() {
    let written = |videos| OutputFile {
        path: "/tmp/a.png".into(),
        size: Size {
            width: 640,
            height: 360,
        },
        scale: DEFAULT_SCALE,
        bytes: 1,
        globals: BTreeMap::new(),
        frames: Frames {
            drawn: 1,
            motion: Motion::Settled,
        },
        play: None,
        videos,
    };
    let shown = |written: &[OutputFile]| {
        let ran = Ran {
            order: &[Slot::Written(0)],
            written,
            failed: &[],
            ended: false,
            findings: &[],
            sheet: None,
        };
        crate::style::plain(&table(ran, style::Stream::Stdout))
    };
    let plain = shown(&[written(Vec::new())]);
    assert!(plain.contains("size"), "{plain}");
    for title in ["steps", "screenshots", "recording"] {
        assert!(!plain.contains(title), "{plain}");
    }
    let recorded = shown(&[written(vec![VideoOutcome {
        format: VideoFormat::Mp4,
        path: "/tmp/a.mp4".into(),
        fps: 30,
        frames: 45,
        held: 30,
        written: 75,
        draw: std::time::Duration::ZERO,
        capture: std::time::Duration::ZERO,
        encode: std::time::Duration::ZERO,
        deepest_queue: 0,
    }])]);
    assert!(
        recorded.contains("recording") && recorded.contains("45 frames · 2.5 s"),
        "with its pauses: {recorded}"
    );
}

#[test]
fn the_shot_that_ended_a_run_is_told_apart_from_those_that_failed() {
    let told = [
        (0, false),
        (0, true),
        (1, false),
        (1, true),
        (2, false),
        (2, true),
    ]
    .map(|(failed, ended)| failures(failed, ended));
    let told = told.each_ref().map(Option::as_deref);
    assert_eq!(
        told,
        [
            None,
            Some("1 shot ended the run"),
            Some("1 shot failed"),
            Some("1 shot failed, 1 ended the run"),
            Some("2 shots failed"),
            Some("2 shots failed, 1 ended the run"),
        ]
    );
}

/// A log entry as a play leaves it, finished with `outcome`.
fn entry(depth: usize, frames: std::ops::Range<u32>, passed: bool, what: What) -> Entry {
    Entry {
        depth,
        frames,
        at: None,
        outcome: Some(if passed {
            Outcome::Passed
        } else {
            Outcome::Failed
        }),
        trace: None,
        what,
    }
}

fn step(label: &str) -> What {
    What::Step {
        label: label.to_owned(),
        path: label.to_owned(),
    }
}

#[test]
fn a_diff_is_coloured_cut_where_it_would_fill_the_screen_and_acts_on_no_terminal() {
    let shown = diff_lines("-old `a\n+new\n same\n", false);
    assert!(shown.contains("\u{1b}[31m-old `a"), "{shown:?}");
    assert!(shown.contains("\u{1b}[32m+new"), "{shown:?}");
    assert!(shown.ends_with("\n same"), "{shown:?}");

    let long = "+line\n".repeat(42);
    let shown = style::plain(&diff_lines(&long, true));
    assert_eq!(shown.lines().count(), 41);
    assert!(
        shown.ends_with("… and 2 more lines, in the diff"),
        "{shown}"
    );
    let shown = style::plain(&diff_lines(&"+line\n".repeat(41), true));
    assert!(shown.ends_with("… and 1 more line, in the diff"), "{shown}");

    let shown = diff_lines("+\u{9b}31m \u{202e}rtl", true);
    assert_eq!(style::plain(&shown), "+\\u{9b}31m \\u{202e}rtl");
    assert!(
        !shown.contains('\u{9b}') && !shown.contains('\u{202e}'),
        "{shown:?}"
    );
}

#[test]
fn lines_cut_off_a_diff_still_count_in_how_its_lines_pair() {
    // Cut at 40, one removed line meets one of three added in its place,
    // which as a whole are changed whole, as the report shows them.
    let diff = format!(
        "{}-  \"on\": true\n{}",
        " {\n".repeat(39),
        "+  \"on\": false\n".repeat(3)
    );
    let shown = diff_lines(&diff, true);
    assert!(
        !shown.contains("48;5;88m") && !shown.contains("48;5;28m"),
        "{shown:?}"
    );
}

#[test]
fn a_diffs_backticks_are_its_own() {
    let kept = Kept {
        diff: Some(" `name`\n".to_owned()),
        ..Kept::default()
    };
    let stream = style::Stream::Stderr;
    let panel = with_kept(Diagnostic::new("it differs"), &kept, stream).panel(stream);
    assert!(panel.contains(" `name`"), "{panel:?}");
}

#[test]
fn a_skipped_check_says_why() {
    let check = crate::play::log::Check {
        subject: "canvas".to_owned(),
        matcher: "to_match_screenshot",
        negated: false,
        expected: "matches screenshot \"saved\"".to_owned(),
        actual: Some("not compared in a capture".to_owned()),
        explanation: None,
        message: None,
        kept: None,
    };
    let mut skipped = entry(0, 0..0, true, What::Check(Box::new(check)));
    skipped.outcome = Some(Outcome::Skipped);
    let list = style::plain(&step_list("shot", "play", &[skipped], false));
    assert!(
        list.contains("– canvas matches screenshot \"saved\"  not compared in a capture"),
        "{list}"
    );
}

#[test]
fn a_plays_log_lists_steps_gestures_and_checks_as_a_tree_with_the_failure_marked() {
    let saved = crate::play::log::Check {
        subject: "\"Saved\"".to_owned(),
        matcher: "to_exist",
        negated: false,
        expected: "exists".to_owned(),
        actual: None,
        explanation: None,
        message: None,
        kept: None,
    };
    let log = [
        entry(0, 0..3, true, step("open")),
        entry(0, 3..3, true, step("ready")),
        entry(0, 3..65, false, step("save")),
        entry(
            1,
            3..5,
            true,
            What::Gesture {
                label: "type \"Ada\"".to_owned(),
            },
        ),
        entry(1, 5..65, false, What::Check(Box::new(saved))),
    ];
    let list = crate::style::plain(&step_list("form", "saves", &log, true));
    let lines: Vec<&str> = list.lines().collect();
    assert_eq!(lines[0], "form · saves");
    let rows: Vec<(char, &str)> = lines[1..]
        .iter()
        .map(|line| {
            let line = line.trim_start();
            let mark = line.chars().next().expect("a mark");
            let text = line[mark.len_utf8()..].trim_start();
            (mark, text.split("  ").next().expect("a label"))
        })
        .collect();
    assert_eq!(
        rows,
        [
            ('✓', "open"),
            ('✓', "ready"),
            ('×', "save"),
            ('✓', "type \"Ada\""),
            ('×', "\"Saved\" exists")
        ],
        "in the order they began, each marked as it ended:\n{list}"
    );
    assert!(
        lines[5].contains("got nothing matched"),
        "the failed check says what it found:\n{list}"
    );
    let column: Vec<_> = lines[1..]
        .iter()
        .map(|line| line.chars().count() - "frames".len())
        .collect();
    assert!(
        column.windows(2).all(|pair| pair[0] == pair[1]),
        "the frame counts line up:\n{list}"
    );
}

#[test]
fn a_play_failing_outside_its_steps_says_so_after_them() {
    let log = [entry(0, 0..3, true, step("open"))];
    let list = crate::style::plain(&step_list("form", "saves", &log, true));
    let last = list.lines().last().expect("a line");
    assert!(
        last.trim_start().starts_with("× failed outside any step"),
        "{list}"
    );
    assert!(list.contains("✓ open"), "the step itself passed:\n{list}");
}

#[test]
fn a_written_path_is_reported_absolute_with_its_detours_resolved_away() {
    let landed = landed(Utf8Path::new("src/../Cargo.toml"));
    assert!(landed.is_absolute(), "`{landed}` is absolute");
    assert!(!landed.as_str().contains(".."), "`{landed}` kept no detour");
    assert!(landed.ends_with("Cargo.toml"), "`{landed}` names the file");
}

/// A run hands back what it did as JSON, so an unattended loop reads a file
/// rather than scraping the text meant for a person.
///
/// The scene never settles, which is the case that matters: the shot is still written,
/// and `settled: false` is what stops the loop diffing a moment the frame ceiling
/// landed on against one the scene chose.
#[test]
fn a_run_reports_what_it_wrote_and_whether_each_shot_settled() {
    fn restless(_ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
        ui.ctx().request_repaint();
        ui.label("never still");
    }
    let manifest = Manifest {
        scenes: vec![SceneEntry {
            render: restless,
            name: "restless",
            module_path: "reference",
            default: true,
            order: 0,
            source: "",
        }],
        groups: Vec::new(),
    };
    let dir = Utf8PathBuf::from_path_buf(std::env::temp_dir())
        .expect("a UTF-8 temp dir")
        .join("gallery-report");
    let report = dir.join("capture.json");
    let capture = Capture {
        shots: vec![Shot {
            scene: "restless".to_owned(),
            out: Some(dir.join("restless.png")),
            size: egui::vec2(80.0, 40.0),
            knobs: Overrides::default(),
            globals: Overrides::default(),
            frames: Some(5),
            trim: true,
            settle: true,
            scale: DEFAULT_SCALE,
            list: false,
            template: false,
            play: None,
            record: None,
        }],
        sheet: None,
        report: Some(report.clone()),
    };
    _ = std::fs::remove_file(&report);
    render(&manifest, Renderer::Wgpu, &|_: &egui::Context| {}, &capture).expect("the shot renders");

    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&report).expect("a report"))
            .expect("valid JSON");
    let shot = &json["shots"][0];
    assert_eq!(shot["name"], "restless", "named by its file stem");
    assert_eq!(
        shot["settled"], false,
        "a scene asking to be redrawn forever never settles"
    );
    assert_eq!(shot["frames"], 5, "so it drew every frame it was allowed");
    assert!(
        shot["path"].as_str().is_some_and(|p| p.ends_with(".png")),
        "and points at the image it wrote: {shot}"
    );
    assert_eq!(json["complete"], true, "every shot asked for was written");
    assert_eq!(json["requested"], 1, "and one was asked for");
    assert!(
        json.get("sheet").is_none(),
        "no sheet was asked for, so none is named"
    );
    assert!(
        json.get("failed").is_none() && json.get("warnings").is_none(),
        "nothing went wrong, so nothing is said to have: {json}"
    );
}

/// A run that stops partway still reports, and says so — a loop counting the records alone
/// cannot tell a recipe of one from a recipe of three that failed on the second.
///
/// The sheet is asked for and skipped, which is a warning rather than a failure — it leaves
/// no path, and the reason belongs in the file, not only in the text a person reads.
#[test]
fn a_run_that_fails_partway_reports_what_it_managed_and_what_stopped_it() {
    fn plain(_ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
        ui.label("fine");
    }
    let manifest = Manifest {
        scenes: vec![SceneEntry {
            render: plain,
            name: "plain",
            module_path: "reference",
            default: true,
            order: 0,
            source: "",
        }],
        groups: Vec::new(),
    };
    let dir = Utf8PathBuf::from_path_buf(std::env::temp_dir())
        .expect("a UTF-8 temp dir")
        .join("gallery-partial");
    let report = dir.join("capture.json");
    let shot = |name: &str, knobs: Overrides| Shot {
        scene: "plain".to_owned(),
        out: Some(dir.join(format!("{name}.png"))),
        size: egui::vec2(60.0, 40.0),
        knobs,
        globals: Overrides::default(),
        frames: None,
        trim: true,
        settle: false,
        scale: DEFAULT_SCALE,
        list: false,
        template: false,
        play: None,
        record: None,
    };
    let capture = Capture {
        shots: vec![
            shot("first", Overrides::default()),
            // Names a knob the scene never declares, which stops the run on the second shot.
            shot("second", knobs(&[("headlights", "true")])),
            shot("third", Overrides::default()),
        ],
        sheet: Some(dir.join("sheet.png")),
        report: Some(report.clone()),
    };
    _ = std::fs::remove_file(&report);
    render(&manifest, Renderer::Wgpu, &|_: &egui::Context| {}, &capture)
        .expect_err("the second shot names no knob");

    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&report).expect("a report even so"))
            .expect("valid JSON");
    assert_eq!(json["complete"], false, "the run did not finish");
    assert_eq!(json["requested"], 3, "three were asked for");
    assert_eq!(
        json["shots"].as_array().map(Vec::len),
        Some(1),
        "one landed"
    );
    assert!(
        json["failed"]
            .as_str()
            .is_some_and(|why| why.contains("headlights")),
        "and the reason names the knob: {json}"
    );
    assert!(
        json.get("sheet").is_none(),
        "a run that stopped gathered no sheet"
    );
}

/// The sheet a run does gather is named, so a loop can find it without guessing the filename.
#[test]
fn a_report_names_the_sheet_it_gathered() {
    fn plain(_ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
        ui.label("fine");
    }
    let manifest = Manifest {
        scenes: vec![SceneEntry {
            render: plain,
            name: "plain",
            module_path: "reference",
            default: true,
            order: 0,
            source: "",
        }],
        groups: Vec::new(),
    };
    let dir = Utf8PathBuf::from_path_buf(std::env::temp_dir())
        .expect("a UTF-8 temp dir")
        .join("gallery-sheet-report");
    let report = dir.join("capture.json");
    let shot = |name: &str| Shot {
        scene: "plain".to_owned(),
        out: Some(dir.join(format!("{name}.png"))),
        size: egui::vec2(60.0, 40.0),
        knobs: Overrides::default(),
        globals: Overrides::default(),
        frames: None,
        trim: true,
        settle: false,
        scale: DEFAULT_SCALE,
        list: false,
        template: false,
        play: None,
        record: None,
    };
    let capture = Capture {
        // Two, since a sheet of one panel is that panel and is skipped.
        shots: vec![shot("one"), shot("two")],
        sheet: Some(dir.join("sheet.png")),
        report: Some(report.clone()),
    };
    _ = std::fs::remove_file(&report);
    render(&manifest, Renderer::Wgpu, &|_: &egui::Context| {}, &capture)
        .expect("both shots render");

    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&report).expect("a report"))
            .expect("valid JSON");
    assert!(
        json["sheet"]
            .as_str()
            .is_some_and(|path| path.ends_with("sheet.png")),
        "the gathered sheet is named: {json}"
    );
    assert_eq!(
        json["shots"].as_array().map(Vec::len),
        Some(2),
        "and it is not counted among the shots, having no scene behind it"
    );
}
