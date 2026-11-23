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
        },
    ];
    let ran = Ran {
        order: &[Slot::Written(0), Slot::Written(1)],
        written: &written,
        failed: &[],
        findings: &[],
        sheet: None,
    };
    let table = crate::style::plain(&table(
        ran,
        Some(Utf8Path::new("/tmp")),
        style::Stream::Stdout,
    ));
    let columns: Vec<_> = table
        .lines()
        .filter(|line| line.contains('×'))
        .map(|row| row.chars().take_while(|c| *c != '×').count())
        .collect();
    assert_eq!(columns.len(), 2, "a row per image:\n{table}");
    assert_eq!(columns[0], columns[1], "the sizes line up:\n{table}");
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
