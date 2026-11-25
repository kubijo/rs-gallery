use std::io::ErrorKind;

use percent_encoding::percent_decode_str;

use super::{
    output::{Rebuilt, filed, is_had, places},
    *,
};

/// A directory, and the crates whose tests are reported in its `out`;
/// no warning of theirs goes unseen.
struct Scratch {
    dir: tempfile::TempDir,
    warned: Arc<Mutex<Vec<String>>>,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            assert_eq!(self.warned(), Vec::<String>::new());
        }
    }
}

impl Scratch {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("a directory"),
            warned: Arc::default(),
        }
    }

    fn path(&self) -> &Utf8Path {
        Utf8Path::from_path(self.dir.path()).expect("UTF-8")
    }

    /// What was warned of, on whichever thread, since this was last asked.
    fn warned(&self) -> Vec<String> {
        std::mem::take(&mut self.warned.lock().expect("no panic"))
    }

    fn out(&self) -> Utf8PathBuf {
        self.path().join("out")
    }

    fn page(&self) -> String {
        fs::read_to_string(self.out().join(PAGE)).expect("the report")
    }

    /// Results of a run of its own, of smoke tests of these scenes of `krate`.
    fn results(&self, krate: &str, run: &str, scenes: &[&str]) -> Results {
        self.on("glow", krate, run, scenes)
    }

    fn on(&self, renderer: &'static str, krate: &str, run: &str, scenes: &[&str]) -> Results {
        let trials = scenes.iter().map(|scene| format!("smoke::{scene}"));
        let manifest_dir = self.path().join(krate);
        fs::create_dir_all(&manifest_dir).expect("made");
        let manifest = format!("[package]\nname = \"{krate}\"\n");
        fs::write(manifest_dir.join("Cargo.toml"), manifest).expect("written");
        let tested = Tested {
            output: self.out(),
            krate: krate.to_owned(),
            manifest_dir,
            renderer,
        };
        let warned = Arc::clone(&self.warned);
        let warn = move |warning: &Diagnostic| {
            warned.lock().expect("no panic").push(warning.plain());
        };
        let mut results = Results::new(tested, trials, warn);
        results.run = run.to_owned();
        results
    }

    /// Each test the page has, by name:
    /// its verdict, and whether it is marked as not run since.
    fn verdicts(&self) -> Vec<(String, Verdict, bool)> {
        let mut all = shown(&self.page());
        all.sort_by(|a, b| a.0.cmp(&b.0));
        all
    }

    /// What is kept of a test on glow.
    fn stored(&self, krate: &str, file: &str) -> Stored {
        let path = self.out().join(krate).join("results-glow").join(file);
        let json = fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        serde_json::from_slice(&json).expect("a result")
    }
}

/// Each test on `page`, in its order.
fn shown(page: &str) -> Vec<(String, Verdict, bool)> {
    let trials = page.match_indices("id=\"").filter_map(|(at, _)| {
        let id = page[at + 4..].split('"').next().expect("a whole tag");
        let of_trial = |at| id[at..].starts_with(".smoke::") || id[at..].starts_with(".play::");
        let dot = id
            .match_indices('.')
            .map(|(at, _)| at)
            .find(|at| of_trial(*at))?;
        let name = percent_decode_str(&id[dot + 1..]);
        let name = name.decode_utf8().expect("UTF-8").into_owned();
        let tag = &page[page[..at].rfind('<').expect("a tag")..at];
        let verdict = [Verdict::Passed, Verdict::Unfinished, Verdict::Failed]
            .into_iter()
            .find(|verdict| tag.contains(&format!("trial {verdict}")))
            .unwrap_or_else(|| panic!("no verdict in {tag}"));
        Some((name, verdict, tag.contains(" earlier")))
    });
    trials.collect()
}

fn passed(name: &str) -> (String, Verdict, bool) {
    (name.to_owned(), Verdict::Passed, false)
}

/// Run a smoke test of `scene` in `results`.
fn run(results: &Results, scene: &str, body: impl FnOnce() -> Checked) {
    let name = format!("smoke::{scene}");
    let trial = Trial {
        name: &name,
        scene,
        play: None,
    };
    results.of(trial, body);
}

/// Run a smoke test of `scene` in `results`, failing with `failure`.
fn drawn(results: &Results, scene: &str, failure: Option<&str>) {
    run(results, scene, || match failure {
        Some(failure) => Checked::failed(Diagnostic::new(failure)),
        None => Checked::default(),
    });
}

/// How many times the report was rebuilt.
fn rebuilt(dir: &Scratch) -> Rebuilt {
    let json = fs::read(dir.out().join(".gallery-rebuilt")).expect("counted");
    serde_json::from_slice(&json).expect("a count")
}

/// Have it counted that the report was rebuilt so many times.
fn count(dir: &Scratch, begun: u64, done: u64) {
    let count = serde_json::json!({ "gallery": VERSION, "begun": begun, "done": done });
    fs::write(dir.out().join(".gallery-rebuilt"), count.to_string()).expect("written");
}

/// Wait until `done`, failing with `what` after longer than it can take.
fn until(what: &str, mut done: impl FnMut() -> bool) {
    let began = Instant::now();
    while !done() {
        assert!(began.elapsed() < Duration::from_secs(10), "{what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

mod logs {
    use super::*;
    use crate::play::{
        SourceAt,
        log::{Attachment, Kept},
    };

    fn check(outcome: Option<Outcome>, kept: Option<Kept>) -> Entry {
        Entry {
            depth: 2,
            frames: 3..5,
            at: Some(SourceAt {
                file: "plays.scene.rs".to_owned(),
                line: 7,
                column: 3,
            }),
            outcome,
            trace: None,
            what: What::Check(Box::new(Check {
                subject: "label `Total`".to_owned(),
                matcher: "to_have_value",
                negated: false,
                expected: "value is\n  equal to \"2\"".to_owned(),
                actual: Some("\"3\"".to_owned()),
                explanation: Some("Difference:\n-3\n+2".to_owned()),
                message: Some("two were added".to_owned()),
                kept,
            })),
        }
    }

    fn line(dir: &Scratch, entry: &Entry) -> Line {
        let mut keeper = Keeper {
            output: dir.path(),
            dir: Some(dir.path().join("plays/adds/snapshots-glow")),
            filmed: None,
            trouble: Vec::new(),
        };
        let line = keeper.line(entry);
        assert_eq!(keeper.trouble, Vec::<String>::new());
        line
    }

    #[test]
    fn a_check_says_what_it_found_unless_it_passed_or_never_ended() {
        let dir = Scratch::new();
        let said = "two were added".to_owned();
        let why = "Difference:\n-3\n+2".to_owned();
        for (outcome, shown, found, notes) in [
            (
                Some(Outcome::Failed),
                LineOutcome::Failed,
                true,
                vec![said.clone(), why.clone()],
            ),
            (
                Some(Outcome::Skipped),
                LineOutcome::Skipped,
                true,
                vec![said.clone(), why],
            ),
            (
                Some(Outcome::Passed),
                LineOutcome::Passed,
                false,
                vec![said.clone()],
            ),
            (None, LineOutcome::Unfinished, false, vec![said]),
        ] {
            let line = line(&dir, &check(outcome, None));
            assert_eq!(line.outcome, shown);
            assert_eq!(line.found.is_some(), found, "{outcome:?}");
            assert_eq!(line.notes, notes, "{outcome:?}");
            assert_eq!(line.text, "label `Total` value is equal to \"2\"");
            assert_eq!((line.depth, line.frames), (2, 2));
            let source = line.source.as_deref().expect("somewhere");
            assert!(source.ends_with("plays.scene.rs:7"), "{source}");
        }
    }

    #[test]
    fn a_snapshot_checks_files_are_copied_but_for_a_diff_it_shows() {
        let dir = Scratch::new();
        let attachments: Vec<Attachment> = [
            (AttachmentKind::Reference, "one.json"),
            (AttachmentKind::Actual, "one.new.json"),
            (AttachmentKind::Diff, "one.diff.json"),
        ]
        .into_iter()
        .map(|(kind, name)| {
            let path = dir.path().join(name);
            fs::write(&path, name).expect("written");
            Attachment { kind, path }
        })
        .collect();
        for (diff, names) in [
            (Some("-a\n+b"), &["reference", "actual"][..]),
            (None, &["reference", "actual", "diff"][..]),
        ] {
            let kept = Kept {
                acceptance: Acceptance::Offered,
                attachments: attachments.clone(),
                diff: diff.map(str::to_owned),
            };
            let line = line(&dir, &check(Some(Outcome::Failed), Some(kept)));
            assert_eq!(line.diff.as_deref(), diff);
            assert_eq!(line.acceptance, Some(Taking::Offered));
            let found: Vec<&str> = line.files.iter().map(|file| file.name.as_str()).collect();
            assert_eq!(found, names);
            for file in &line.files {
                assert!(dir.path().join(&file.path).is_file(), "{}", file.path);
            }
        }
        let taken = Kept {
            acceptance: Acceptance::Taken,
            ..Kept::default()
        };
        let line = line(&dir, &check(Some(Outcome::Passed), Some(taken)));
        assert_eq!(line.acceptance, Some(Taking::Taken));
    }

    #[test]
    fn what_an_earlier_run_left_is_removed_and_nothing_of_anothers() {
        let dir = Scratch::new();
        let snapshots = dir.path().join("plays/adds/snapshots-glow");
        let keeper = |at: &Utf8Path| Keeper {
            output: Utf8Path::new(""),
            dir: Some(at.to_owned()),
            filmed: None,
            trouble: Vec::new(),
        };
        fs::create_dir_all(&snapshots).expect("made");
        for name in ["one.png", "one.json", "thesis.txt"] {
            fs::write(snapshots.join(name), "").expect("written");
        }
        #[cfg(unix)]
        {
            let link = dir.path().join("linked");
            std::os::unix::fs::symlink(&snapshots, &link).expect("linked");
            keeper(&link).clear();
            assert!(snapshots.join("one.png").exists(), "it is a link");
        }
        keeper(&snapshots).clear();
        assert!(!snapshots.join("one.png").exists());
        assert!(!snapshots.join("one.json").exists());
        assert!(snapshots.join("thesis.txt").exists());
        fs::remove_file(snapshots.join("thesis.txt")).expect("removed");
        keeper(&snapshots).clear();
        assert!(!snapshots.exists(), "an empty one goes");
    }
}

mod filing {
    use super::*;

    #[test]
    fn a_test_is_filed_by_the_parts_of_its_name_and_never_elsewhere() {
        for (trial, path) in [
            ("play::plays::Todo::adds", "play/plays/Todo/adds"),
            ("smoke::plays::Calls back", "smoke/plays/Calls back"),
            ("smoke::plays::Přehled 100 %", "smoke/plays/Přehled 100 %"),
            ("smoke::v1.2", "smoke/v1.2"),
            ("smoke::..::a/b", "smoke/_/a_b"),
            ("smoke::C:\\x", "smoke/C__x"),
            ("smoke::con", "smoke/_"),
            ("smoke::plays::", "smoke/plays/_"),
            ("smoke::plays::   ", "smoke/plays/_"),
            ("smoke::::x", "smoke/_/x"),
        ] {
            assert_eq!(filed(trial), path, "{trial}");
        }
    }

    #[test]
    fn tests_filed_alike_are_told_apart_by_number() {
        let names = ["smoke::a/b", "smoke::a:b", "smoke::A_B 2", "smoke::A?b"];
        let places = places(&names.map(str::to_owned));
        let paths = names.map(|name| places[name].as_str());
        assert_eq!(
            paths,
            ["smoke/a_b", "smoke/a_b 2", "smoke/A_B 2 2", "smoke/A_b 3"]
        );
    }

    #[test]
    fn a_test_whatever_its_title_is_reported() {
        let dir = Scratch::new();
        let long = "x".repeat(300);
        let wide = "漢".repeat(100);
        let titles = ["", "  ", "::", ".", "..", &long, &format!("{long}y"), &wide];
        let scenes: Vec<String> = titles.iter().map(|it| format!("odd::{it}")).collect();
        let scenes: Vec<&str> = scenes.iter().map(String::as_str).collect();
        let results = dir.results("odd", "1", &scenes);
        for scene in &scenes {
            drawn(&results, scene, None);
        }
        results.close().expect("the report");
        let expected: Vec<String> = scenes.iter().map(|it| format!("smoke::{it}")).collect();
        let reported: Vec<String> = shown(&dir.page()).into_iter().map(|it| it.0).collect();
        assert_eq!(reported, expected);
    }

    #[test]
    fn a_test_that_is_filed_elsewhere_now_is_reported_once() {
        let dir = Scratch::new();
        let scenes = ["plays::a/b", "plays::a:b"];
        let first = dir.results("plays", "1", &scenes);
        for scene in scenes {
            drawn(&first, scene, None);
        }
        first.close().expect("the report");
        assert_eq!(dir.verdicts().len(), 2);

        let second = dir.results("plays", "2", &scenes[1..]);
        drawn(&second, "plays::a:b", None);
        second.close().expect("the report");
        assert_eq!(dir.verdicts(), [passed("smoke::plays::a:b")]);
    }

    #[test]
    fn a_test_named_as_anothers_file_is_reported_beside_it() {
        let dir = Scratch::new();
        let scenes = ["m::a", "m::a.json::b", "m::a.fragment::c"];
        let results = dir.results("m", "1", &scenes);
        for scene in scenes {
            drawn(&results, scene, None);
        }
        results.close().expect("the report");
        assert_eq!(dir.verdicts().len(), 3);
    }

    #[test]
    fn what_a_process_that_was_killed_left_is_swept_and_what_one_is_writing_is_not() {
        let dir = Scratch::new();
        let results = dir.results("plays", "1", &["plays::one"]);
        drawn(&results, "plays::one", None);
        let inside = dir.out().join("plays/results-glow/smoke/plays");
        let left = [dir.out(), inside].map(|dir| dir.join(".gallery-writing-dead"));
        let young = dir.out().join(".gallery-writing-young");
        for path in left.iter().chain([&young]) {
            fs::write(path, "half").expect("written");
        }
        for (path, age) in [(&left[0], 600), (&left[1], 600), (&young, 30)] {
            let file = fs::File::options().write(true).open(path).expect("opened");
            let then = std::time::SystemTime::now() - Duration::from_secs(age);
            file.set_modified(then).expect("aged");
        }
        results.close().expect("the report");
        assert!(left.iter().all(|path| !path.exists()));
        assert!(young.exists());
    }

    #[cfg(unix)]
    #[test]
    fn lock_files_that_can_only_be_read_will_do() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = Scratch::new();
        let results = dir.results("plays", "1", &["plays::one"]);
        drawn(&results, "plays::one", None);
        for file in dir.out().read_dir_utf8().expect("read").flatten() {
            if file.file_name().starts_with(".") && file.path().is_file() {
                fs::set_permissions(file.path(), fs::Permissions::from_mode(0o444)).expect("set");
            }
        }
        drawn(&results, "plays::one", None);
        results.close().expect("the report");
    }

    #[test]
    fn a_result_keeps_its_place_in_whatever_case_the_file_system_has_it() {
        let dir = Scratch::new();
        let results = dir.results("plays", "1", &["plays::Todo"]);
        drawn(&results, "plays::Todo", None);
        let results_dir = dir.out().join("plays/results-glow/smoke/plays");
        let [from, to] = ["Todo", "todo"].map(|name| results_dir.join(format!("{name}.fragment")));
        fs::rename(from, to).expect("moved");
        results.close().expect("the report");
        assert_eq!(dir.verdicts(), [passed("smoke::plays::Todo")]);
    }

    #[test]
    fn a_crates_directory_may_be_a_link() {
        let dir = Scratch::new();
        fs::create_dir_all(dir.path().join("elsewhere")).expect("made");
        fs::create_dir_all(dir.out()).expect("made");
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.path().join("elsewhere"), dir.out().join("plays"))
            .expect("linked");
        let results = dir.results("plays", "1", &["plays::one"]);
        drawn(&results, "plays::one", None);
        results.close().expect("the report");
        assert_eq!(dir.verdicts(), [passed("smoke::plays::one")]);
    }

    #[test]
    fn what_is_made_is_tagged_as_a_cache() {
        let dir = Scratch::new();
        let results = dir.results("plays", "1", &["plays::one"]);
        drawn(&results, "plays::one", None);
        assert!(dir.out().join("CACHEDIR.TAG").is_file());
    }
}

mod runs {
    use super::*;

    #[test]
    fn a_test_that_ends_its_process_is_left_unfinished() {
        let dir = Scratch::new();
        let scenes = ["plays::one"];
        let name = "smoke::plays::one";
        drawn(&dir.results("plays", "1", &scenes), "plays::one", None);
        let second = dir.results("plays", "2", &scenes);
        run(&second, "plays::one", || {
            let unfinished = (name.to_owned(), Verdict::Unfinished, false);
            assert_eq!(dir.verdicts(), [unfinished], "as an abort would leave it");
            Checked::default()
        });
        second.close().expect("the report");
        assert_eq!(dir.verdicts(), [passed(name)]);
    }

    #[test]
    fn a_test_that_ends_its_process_soon_after_another_is_the_one_left_unfinished() {
        let dir = Scratch::new();
        let scenes = ["plays::one", "plays::two"];
        for number in ["1", "2"] {
            let results = dir.results("plays", number, &scenes);
            drawn(&results, "plays::one", None);
            run(&results, "plays::two", || {
                let expected = [
                    passed("smoke::plays::one"),
                    ("smoke::plays::two".to_owned(), Verdict::Unfinished, false),
                ];
                assert_eq!(dir.verdicts(), expected, "as an abort would leave it");
                Checked::default()
            });
            results.close().expect("the report");
        }
    }

    #[test]
    fn the_report_follows_the_tests_while_one_of_them_hangs() {
        let dir = Scratch::new();
        let results = dir.results("plays", "1", &["plays::hangs", "plays::ends"]);
        let (began, begun) = mpsc::channel();
        let (ended, end) = mpsc::channel::<()>();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                run(&results, "plays::hangs", move || {
                    began.send(()).expect("heard");
                    _ = end.recv_timeout(Duration::from_secs(20));
                    Checked::default()
                });
            });
            begun.recv().expect("begun");
            drawn(&results, "plays::ends", None);
            until("it is shown to have passed", || {
                dir.verdicts()[0] == passed("smoke::plays::ends")
            });
            assert_eq!(dir.verdicts()[1].1, Verdict::Unfinished);

            std::thread::sleep(4 * Following::APART);
            let before = rebuilt(&dir).begun;
            std::thread::sleep(4 * Following::APART);
            assert_eq!(rebuilt(&dir).begun, before, "there is nothing new to it");

            drawn(&results, "plays::ends", Some("the second time"));
            let began = Instant::now();
            until("it follows on", || dir.verdicts()[0].1 == Verdict::Failed);
            assert!(began.elapsed() < 8 * Following::APART);
            drop(ended);
        });
    }

    #[test]
    fn one_thread_follows_all_of_a_process_until_it_closes() {
        let dir = Scratch::new();
        let results = dir.results("plays", "1", &["plays::one", "plays::two"]);
        for scene in ["plays::one", "plays::two"] {
            drawn(&results, scene, None);
        }
        assert_eq!(Arc::strong_count(&results.output), 2);
        results.close().expect("the report");
        assert_eq!(Arc::strong_count(&results.output), 1);
    }

    #[test]
    fn a_rebuild_that_failed_is_tried_again_while_its_test_goes_on() {
        let dir = Scratch::new();
        let results = dir.results("plays", "1", &["plays::one"]);
        fs::create_dir_all(dir.out()).expect("made");
        let held = fs::File::create(dir.out().join(".gallery-rebuilds")).expect("created");
        held.lock().expect("locked");
        run(&results, "plays::one", || {
            drop(held);
            until("it is told of", || dir.out().join(PAGE).exists());
            assert_eq!(dir.verdicts()[0].1, Verdict::Unfinished);
            Checked::default()
        });
        let warned = dir.warned();
        assert!(warned[0].contains("has it locked"), "{warned:?}");
    }

    #[test]
    fn a_rebuild_that_fails_while_a_test_goes_on_is_warned_of_and_tried_ever_later() {
        let dir = Scratch::new();
        let results = dir.results("plays", "1", &["plays::one"]);
        run(&results, "plays::one", || {
            fs::write(dir.out().join(PAGE), "<h1>My site</h1>").expect("written");
            let stored = dir.stored("plays", "smoke/plays/one.json");
            results.output.write(&stored).expect("written");
            until("it is warned of", || !dir.warned().is_empty());
            let before = rebuilt(&dir).begun;
            std::thread::sleep(8 * Following::APART);
            let tried = rebuilt(&dir).begun - before;
            assert!((1..=3).contains(&tried), "{tried} times");
            Checked::default()
        });
    }

    #[test]
    fn rebuilds_are_as_far_apart_as_they_are_slow_and_further_as_they_fail() {
        let least = Following::APART;
        let slow = Duration::from_secs(3);
        for (apart, rebuilt, expected) in [
            (least, Ok(Duration::ZERO), least),
            (slow, Ok(least / 8), least),
            (least, Ok(slow), 4 * slow),
            (least, Err(()), 2 * least),
            (PATIENCE, Err(()), PATIENCE),
        ] {
            assert_eq!(Following::apart(apart, &rebuilt), expected, "{rebuilt:?}");
        }
    }

    #[test]
    fn a_panic_in_a_test_fails_it_and_goes_on() {
        let dir = Scratch::new();
        let results = dir.results("plays", "1", &["plays::one"]);
        let panicked = std::panic::catch_unwind(AssertUnwindSafe(|| {
            run(&results, "plays::one", || panic!("the host's"));
        }));
        panicked.expect_err("it is raised again");
        results.close().expect("the report");
        assert_eq!(dir.verdicts()[0].1, Verdict::Failed);
        let page = dir.page();
        assert!(page.contains("panicked: the host&#39;s"), "{page}");
    }

    #[test]
    fn a_test_not_run_since_is_marked_and_one_that_is_no_more_is_dropped() {
        let dir = Scratch::new();
        let scenes = ["plays::kept", "plays::run", "plays::gone"];
        let first = dir.results("plays", "1", &scenes);
        for scene in scenes {
            drawn(&first, scene, None);
        }
        let other = dir.results("other", "1", &["other::scene"]);
        drawn(&other, "other::scene", None);

        let second = dir.results("plays", "2", &scenes[..2]);
        drawn(&second, "plays::run", Some("it fails now"));
        second.close().expect("the report");
        assert_eq!(
            dir.verdicts(),
            [
                passed("smoke::other::scene"),
                ("smoke::plays::kept".to_owned(), Verdict::Passed, true),
                ("smoke::plays::run".to_owned(), Verdict::Failed, false),
            ]
        );
        assert!(dir.page().contains("it fails now"));
    }

    #[test]
    fn a_crate_that_is_renamed_or_gone_is_dropped_and_another_renderer_is_not() {
        let dir = Scratch::new();
        for krate in ["renamed", "gone", "broken", "plays"] {
            let scene = format!("{krate}::one");
            let results = dir.results(krate, "1", &[&scene]);
            drawn(&results, &scene, Some("it failed"));
            results.close().expect("the report");
        }
        let manifest = |krate: &str| dir.path().join(krate).join("Cargo.toml");
        fs::write(manifest("renamed"), "[package]\nname = \"now-named\"\n").expect("written");
        fs::remove_dir_all(dir.path().join("gone")).expect("removed");
        fs::write(manifest("broken"), "being edited [").expect("written");

        let results = dir.on("wgpu", "plays", "2", &["plays::one"]);
        drawn(&results, "plays::one", None);
        results.close().expect("the report");
        let failed = |name: &str| (name.to_owned(), Verdict::Failed, false);
        assert_eq!(
            dir.verdicts(),
            [
                failed("smoke::broken::one"),
                failed("smoke::plays::one"),
                passed("smoke::plays::one"),
            ]
        );
    }

    #[test]
    fn a_crate_whose_manifest_is_gone_for_a_moment_is_back_with_it() {
        let dir = Scratch::new();
        let other = dir.results("other", "1", &["other::one"]);
        drawn(&other, "other::one", None);
        other.close().expect("the report");
        let manifest = dir.path().join("other/Cargo.toml");
        let aside = manifest.with_extension("aside");
        let plays = dir.results("plays", "1", &["plays::one"]);

        fs::rename(&manifest, &aside).expect("moved");
        drawn(&plays, "plays::one", None);
        plays.close().expect("the report");
        assert_eq!(dir.verdicts(), [passed("smoke::plays::one")]);

        fs::rename(&aside, &manifest).expect("moved back");
        plays.close().expect("the report");
        assert_eq!(dir.verdicts().len(), 2);
    }

    #[test]
    fn a_crate_whose_manifest_cannot_be_read_is_kept() {
        let dir = Scratch::new();
        let other = dir.results("other", "1", &["other::one"]);
        drawn(&other, "other::one", None);
        let manifest = dir.path().join("other/Cargo.toml");
        fs::remove_file(&manifest).expect("removed");
        fs::create_dir(&manifest).expect("a directory cannot be read as a file");
        let plays = dir.results("plays", "1", &["plays::one"]);
        drawn(&plays, "plays::one", None);
        plays.close().expect("the report");
        assert_eq!(dir.verdicts().len(), 2);
    }

    #[test]
    fn a_test_added_since_is_not_removed_by_a_process_that_does_not_know_it() {
        let dir = Scratch::new();
        let old = dir.results("plays", "1", &["plays::one"]);
        let older = dir.results("plays", "0", &["plays::one"]);
        drawn(&old, "plays::one", None);
        let new = dir.results("plays", "2", &["plays::one", "plays::added"]);
        drawn(&new, "plays::added", Some("it fails"));
        let added = ("smoke::plays::added".to_owned(), Verdict::Failed);
        for last in [&new, &old, &older] {
            drawn(last, "plays::one", None);
            last.close().expect("the report");
            let (name, verdict, _) = dir.verdicts().swap_remove(0);
            assert_eq!((name, verdict), added);
        }
    }

    #[test]
    fn a_test_filed_otherwise_by_a_newer_process_is_shown_once_and_from_there() {
        let dir = Scratch::new();
        let scenes = ["plays::a/b", "plays::a:b"];
        let old = dir.results("plays", "1", &scenes);
        drawn(&old, scenes[0], None);
        let new = dir.results("plays", "2", &scenes[1..]);
        drawn(&new, scenes[1], Some("as the newer has it"));
        drawn(&old, scenes[1], Some("as the older has it"));
        for last in [&old, &new] {
            last.close().expect("the report");
            assert_eq!(dir.verdicts().len(), 1, "{:?}", dir.verdicts());
            assert!(dir.page().contains("as the newer has it"));
            assert!(!dir.page().contains("as the older has it"));
        }
        let olders = new.output.results().join("smoke/plays/a_b 2.fragment");
        assert!(olders.exists(), "it may still be at it");
    }

    #[test]
    fn a_test_that_is_no_more_or_filed_elsewhere_is_on_no_processs_page() {
        let dir = Scratch::new();
        let scenes = ["plays::a/b", "plays::a:b", "plays::gone"];
        let first = dir.results("plays", "1", &scenes);
        for scene in scenes {
            drawn(&first, scene, Some("it failed"));
        }
        first.close().expect("the report");
        let second = dir.results("plays", "2", &scenes[1..2]);
        let writes = fs::File::open(dir.out().join(".gallery-writes")).expect("opened");
        writes.lock_shared().expect("locked");
        drawn(&second, scenes[1], None);
        second.close().expect("the report");
        let other = dir.results("other", "2", &["other::one"]);
        drawn(&other, "other::one", None);
        other.close().expect("the report");
        let left = second.output.results().join("smoke/plays/gone.fragment");
        assert!(left.exists(), "it could not be removed");
        assert_eq!(
            dir.verdicts(),
            [passed("smoke::other::one"), passed("smoke::plays::a:b")]
        );

        drop(writes);
        other.close().expect("the report");
        assert!(left.exists(), "it is not another's to remove");
        second.close().expect("the report");
        assert!(!left.exists());
    }

    #[test]
    fn a_process_removes_nothing_that_its_own_catalog_has() {
        let dir = Scratch::new();
        let scenes = ["plays::one", "plays::two"];
        let first = dir.results("plays", "1", &scenes);
        for scene in scenes {
            drawn(&first, scene, None);
        }
        first.close().expect("the report");
        let old = dir.results("plays", "2", &scenes);
        let new = dir.results("plays", "3", &scenes[..1]);
        let writes = fs::File::open(dir.out().join(".gallery-writes")).expect("opened");
        writes.lock_shared().expect("locked");
        drawn(&new, "plays::one", None);
        new.close().expect("the report");
        drop(writes);
        old.output.rebuild().expect("rebuilt");
        assert_eq!(dir.verdicts(), [passed("smoke::plays::one")]);
        let kept = old.output.results().join("smoke/plays/two.fragment");
        assert!(kept.exists());
    }

    /// Have `results` be of a process that began by a clock that was an hour ahead.
    fn fast(results: &mut Results) {
        let output = Arc::get_mut(&mut results.output).expect("nothing follows it yet");
        output.list.began = Timestamp::now() + Duration::from_hours(1);
    }

    #[test]
    fn a_list_by_a_clock_that_was_ahead_is_replaced() {
        let dir = Scratch::new();
        let mut ahead = dir.results("plays", "1", &["plays::one", "plays::gone"]);
        fast(&mut ahead);
        drawn(&ahead, "plays::gone", Some("it failed"));
        ahead.close().expect("the report");

        let next = dir.results("plays", "2", &["plays::one", "plays::added"]);
        drawn(&next, "plays::added", Some("it fails now"));
        next.close().expect("the report");
        let added = ("smoke::plays::added".to_owned(), Verdict::Failed, false);
        assert_eq!(dir.verdicts(), [added]);
    }

    #[test]
    fn a_process_by_a_clock_that_was_ahead_removes_nothing_written_since() {
        let dir = Scratch::new();
        let mut ahead = dir.results("plays", "1", &["plays::one"]);
        fast(&mut ahead);
        drawn(&ahead, "plays::one", None);
        let next = dir.results("plays", "2", &["plays::one", "plays::added"]);
        drawn(&next, "plays::added", Some("it fails now"));
        drawn(&ahead, "plays::one", None);
        for last in [&ahead, &next, &ahead] {
            last.close().expect("the report");
            assert_eq!(dir.verdicts().len(), 2);
        }
    }

    #[test]
    fn a_list_of_the_same_tests_is_left_as_it_is() {
        let dir = Scratch::new();
        let mut lists = ["1", "2"].map(|run| {
            let results = dir.results("plays", run, &["plays::one"]);
            drawn(&results, "plays::one", None);
            results.close().expect("the report");
            fs::read(results.output.results().join("order.json")).expect("read")
        });
        let [first, second] = &mut lists;
        assert_eq!(first, second);
    }

    #[test]
    fn a_catalog_that_lost_its_last_test_is_dropped() {
        let dir = Scratch::new();
        let first = dir.results("plays", "1", &["plays::gone"]);
        drawn(&first, "plays::gone", Some("it failed"));
        first.close().expect("the report");
        let left = first.output.results().join("smoke/plays/gone.fragment");
        assert!(left.exists());
        dir.results("plays", "2", &[]).close();
        assert_eq!(dir.verdicts(), []);
        assert!(!left.exists());
    }

    #[test]
    fn a_directory_that_is_not_a_renderers_results_holds_none() {
        let dir = Scratch::new();
        let results = dir.results("plays", "1", &["plays::one"]);
        drawn(&results, "plays::one", None);
        let fragment = results.output.results().join("smoke/plays/one.fragment");
        let elsewhere = dir.out().join("plays/backup/smoke/plays");
        fs::create_dir_all(&elsewhere).expect("made");
        fs::copy(fragment, elsewhere.join("one.fragment")).expect("copied");
        results.close().expect("the report");
        assert_eq!(dir.verdicts(), [passed("smoke::plays::one")]);
    }

    #[test]
    fn a_result_there_is_no_list_of_is_shown() {
        let dir = Scratch::new();
        let results = dir.results("plays", "1", &["plays::one"]);
        drawn(&results, "plays::one", None);
        results.close().expect("the report");
        fs::write(results.output.results().join("order.json"), "half").expect("written");
        results.close().expect("the report");
        assert_eq!(dir.verdicts(), [passed("smoke::plays::one")]);
    }

    #[test]
    fn an_output_that_is_removed_meanwhile_is_made_again() {
        let dir = Scratch::new();
        let scenes = ["plays::zebra", "plays::moth", "plays::ant"];
        let results = dir.results("plays", "1", &scenes);
        drawn(&results, "plays::moth", None);
        results.close().expect("the report");
        fs::remove_dir_all(dir.out()).expect("removed");
        for scene in scenes.iter().rev() {
            drawn(&results, scene, None);
        }
        results.close().expect("the report");
        let names: Vec<String> = shown(&dir.page()).into_iter().map(|it| it.0).collect();
        assert_eq!(names, scenes.map(|scene| format!("smoke::{scene}")));
    }

    #[test]
    fn another_versions_result_is_not_shown_and_is_not_removed() {
        let dir = Scratch::new();
        let scenes = ["plays::one", "plays::two"];
        let results = dir.results("plays", "1", &scenes);
        drawn(&results, "plays::one", Some("of another version"));
        let path = results.output.results().join("smoke/plays/one.fragment");
        let fragment = fs::read_to_string(&path).expect("read");
        let version = format!("\"gallery\":\"{}\"", env!("CARGO_PKG_VERSION"));
        let other = fragment.replacen(&version, "\"gallery\":\"0.0.0\"", 1);
        assert_ne!(other, fragment);
        fs::write(&path, other).expect("written");

        let next = dir.results("plays", "2", &scenes);
        drawn(&next, "plays::two", None);
        next.close().expect("the report");
        assert_eq!(dir.verdicts(), [passed("smoke::plays::two")]);
        assert!(!dir.page().contains("of another version"));
        assert!(path.exists());

        drawn(&next, "plays::one", None);
        next.close().expect("the report");
        assert_eq!(dir.verdicts().len(), 2, "it is written over");
    }

    #[test]
    fn a_crates_dashes_are_no_other_name() {
        let dir = Scratch::new();
        let results = dir.results("my_plays", "1", &["my_plays::one"]);
        let manifest = dir.path().join("my_plays/Cargo.toml");
        fs::write(manifest, "[package]\nname = \"my-plays\"\n").expect("written");
        drawn(&results, "my_plays::one", None);
        let other = dir.results("other", "1", &["other::one"]);
        drawn(&other, "other::one", None);
        other.close().expect("the report");
        assert_eq!(dir.verdicts().len(), 2);
    }

    #[test]
    fn tests_not_run_since_are_where_their_catalog_has_them_now() {
        let dir = Scratch::new();
        let first = dir.results("plays", "1", &["plays::b", "plays::c"]);
        for scene in ["plays::b", "plays::c"] {
            drawn(&first, scene, None);
        }
        let scenes = [
            "plays::a1",
            "plays::a2",
            "plays::b",
            "plays::a3",
            "plays::c",
        ];
        let second = dir.results("plays", "2", &scenes);
        for scene in ["plays::a1", "plays::a2", "plays::a3"] {
            drawn(&second, scene, None);
        }
        second.close().expect("the report");
        let names: Vec<String> = shown(&dir.page()).into_iter().map(|it| it.0).collect();
        assert_eq!(names, scenes.map(|scene| format!("smoke::{scene}")));
    }

    #[test]
    fn a_test_run_again_is_shown_as_it_came_out_last() {
        let dir = Scratch::new();
        let results = dir.results("plays", "1", &["plays::one"]);
        for failure in [Some("the first time"), None, Some("the third time")] {
            drawn(&results, "plays::one", failure);
            results.close().expect("the report");
            let expected = match failure {
                Some(_) => Verdict::Failed,
                None => Verdict::Passed,
            };
            assert_eq!(dir.verdicts()[0].1, expected);
            assert!(dir.page().contains(failure.unwrap_or_default()));
        }
        assert!(!dir.page().contains("the first time"));
    }

    #[test]
    fn a_run_is_nextests_or_else_the_processs_own() {
        let nextest = || Some("04ef8700".to_owned());
        assert_eq!(run_id(nextest()), run_id(nextest()));
        assert!(run_id(None).starts_with(&std::process::id().to_string()));
        assert_ne!(run_id(None), run_id(None), "a process number comes again");
    }

    #[test]
    fn a_run_of_no_test_touches_nothing() {
        let dir = Scratch::new();
        let results = dir.results("plays", "1", &["plays::one"]);
        assert_eq!(results.close(), None);
        assert!(!dir.out().exists());
    }

    #[test]
    fn a_test_took_as_long_as_it_ran_whoever_else_is_at_the_report() {
        let dir = Scratch::new();
        let results = dir.results("plays", "1", &["plays::one"]);
        fs::create_dir_all(dir.out()).expect("made");
        let held = fs::File::create(dir.out().join(".gallery-rebuilds")).expect("created");
        held.lock().expect("locked");
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(500));
                drop(held);
            });
            run(&results, "plays::one", || {
                std::thread::sleep(Duration::from_millis(50));
                Checked::default()
            });
        });
        let took = dir.stored("plays", "smoke/plays/one.json").kept.took_ms;
        assert!((50..400).contains(&took), "{took} ms");
    }

    #[test]
    fn a_tests_warnings_are_its_results() {
        let dir = Scratch::new();
        let results = dir.results("plays", "1", &["plays::one"]);
        run(&results, "plays::one", || {
            let mut checked = Checked::default();
            checked.warnings.push(Diagnostic::warning("it wobbles"));
            checked
        });
        results.close().expect("the report");
        assert!(dir.page().contains("it wobbles"), "{}", dir.page());
        assert!(
            dir.page().contains("<details"),
            "it is no plain test: {}",
            dir.page()
        );
    }

    #[test]
    fn tests_are_shown_in_their_catalogs_order() {
        let dir = Scratch::new();
        let scenes = ["plays::zebra", "plays::ant", "plays::moth"];
        let results = dir.results("plays", "1", &scenes);
        for scene in scenes.iter().rev() {
            drawn(&results, scene, None);
        }
        results.close().expect("the report");
        let names: Vec<String> = shown(&dir.page()).into_iter().map(|it| it.0).collect();
        assert_eq!(names, scenes.map(|scene| format!("smoke::{scene}")));
    }
}

mod sharing {
    use super::*;

    #[test]
    fn tests_in_processes_of_their_own_all_come_to_the_report() {
        let dir = Scratch::new();
        let scenes: Vec<String> = (0..24).map(|n| format!("plays::scene_{n}")).collect();
        let scenes: Vec<&str> = scenes.iter().map(String::as_str).collect();
        std::thread::scope(|scope| {
            for scene in &scenes {
                scope.spawn(|| {
                    let own = dir.results("plays", "nextest", &scenes);
                    drawn(&own, scene, None);
                    own.close().expect("the report");
                });
            }
        });
        let verdicts = dir.verdicts();
        assert_eq!(verdicts.len(), scenes.len());
        for (name, verdict, earlier) in verdicts {
            assert_eq!((verdict, earlier), (Verdict::Passed, false), "{name}");
        }
    }

    #[test]
    fn a_process_that_is_done_leaves_a_report_with_its_test_in_it() {
        let dir = Scratch::new();
        let scenes: Vec<String> = (0..16).map(|n| format!("plays::scene_{n}")).collect();
        let scenes: Vec<&str> = scenes.iter().map(String::as_str).collect();
        std::thread::scope(|scope| {
            for &scene in &scenes {
                let (dir, scenes) = (&dir, &scenes);
                scope.spawn(move || {
                    let own = dir.results("plays", "nextest", scenes);
                    drawn(&own, scene, None);
                    own.close().expect("the report");
                    let name = format!("smoke::{scene}");
                    assert!(dir.verdicts().contains(&passed(&name)), "{name}");
                });
            }
        });
    }

    #[test]
    fn those_that_wait_for_the_report_together_have_it_rebuilt_once() {
        let dir = Scratch::new();
        let scenes: Vec<String> = (0..8).map(|n| format!("plays::scene_{n}")).collect();
        let scenes: Vec<&str> = scenes.iter().map(String::as_str).collect();
        let runs: Vec<Results> = scenes
            .iter()
            .map(|scene| {
                let own = dir.results("plays", "nextest", &scenes);
                drawn(&own, scene, None);
                own
            })
            .collect();
        let before = rebuilt(&dir).done;
        let held = fs::File::create(dir.out().join(".gallery-rebuilds")).expect("created");
        held.lock().expect("locked");
        std::thread::scope(|scope| {
            for run in &runs {
                scope.spawn(|| run.output.rebuild().expect("rebuilt"));
            }
            std::thread::sleep(Duration::from_millis(500));
            drop(held);
        });
        assert_eq!(rebuilt(&dir).done - before, 1);
        assert!(dir.verdicts().iter().all(|it| it.1 == Verdict::Passed));
    }

    #[test]
    fn a_process_that_holds_a_lock_for_ever_is_waited_for_no_longer_than_that() {
        for lock in [".gallery-rebuilds", ".gallery-writes"] {
            let dir = Scratch::new();
            let results = dir.results("plays", "1", &["plays::one"]);
            fs::create_dir_all(dir.out()).expect("made");
            let held = fs::File::create(dir.out().join(lock)).expect("created");
            held.lock().expect("locked");
            let (done, over) = mpsc::channel();
            std::thread::spawn(move || {
                drawn(&results, "plays::one", None);
                results.close();
                done.send(()).expect("heard");
            });
            let over = over.recv_timeout(Duration::from_secs(8));
            over.expect("it goes on without");
            let warned = dir.warned();
            assert!(
                matches!(&warned[..], [one] if one.contains(lock)),
                "{warned:?}"
            );
        }
    }

    #[test]
    fn a_rebuild_that_was_cut_short_is_no_ones_report() {
        let dir = Scratch::new();
        let results = dir.results("plays", "1", &["plays::one"]);
        drawn(&results, "plays::one", None);
        results.close().expect("the report");
        let held = fs::File::create(dir.out().join(".gallery-rebuilds")).expect("created");
        held.lock().expect("locked");
        std::thread::scope(|scope| {
            let closing = scope.spawn(|| results.output.rebuild());
            std::thread::sleep(Duration::from_millis(300));
            count(&dir, 1_000, 0);
            fs::remove_file(dir.out().join(PAGE)).expect("removed");
            drop(held);
            closing.join().expect("no panic").expect("rebuilt");
        });
        assert_eq!(dir.verdicts(), [passed("smoke::plays::one")]);
    }

    #[test]
    fn a_rebuild_already_begun_is_not_the_report_of_what_is_written_after() {
        let dir = Scratch::new();
        let results = dir.results("plays", "1", &["plays::one"]);
        drawn(&results, "plays::one", None);
        results.close().expect("the report");
        let held = fs::File::create(dir.out().join(".gallery-rebuilds")).expect("created");
        held.lock().expect("locked");
        count(&dir, 1_000, 999);
        std::thread::scope(|scope| {
            let closing = scope.spawn(|| results.output.rebuild());
            std::thread::sleep(Duration::from_millis(300));
            count(&dir, 1_000, 1_000);
            fs::remove_file(dir.out().join(PAGE)).expect("removed");
            drop(held);
            closing.join().expect("no panic").expect("rebuilt");
        });
        assert_eq!(dir.verdicts(), [passed("smoke::plays::one")]);
    }

    #[test]
    fn a_rebuild_done_by_another_is_waited_for_no_longer() {
        let dir = Scratch::new();
        let results = dir.results("plays", "1", &["plays::one"]);
        drawn(&results, "plays::one", None);
        results.close().expect("the report");
        let next = rebuilt(&dir).begun + 1;
        let held = fs::File::create(dir.out().join(".gallery-rebuilds")).expect("created");
        held.lock().expect("locked");
        std::thread::scope(|scope| {
            let closing = scope.spawn(|| results.output.rebuild());
            std::thread::sleep(Duration::from_millis(300));
            count(&dir, next, next);
            let closed = closing.join().expect("no panic");
            assert_eq!(closed, Ok(Duration::ZERO), "still locked");
        });
        drop(held);
    }

    #[test]
    fn another_versions_rebuild_is_not_ours() {
        let dir = Scratch::new();
        let results = dir.results("plays", "1", &["plays::one"]);
        drawn(&results, "plays::one", None);
        results.close().expect("the report");
        let next = rebuilt(&dir).begun + 1;
        let held = fs::File::create(dir.out().join(".gallery-rebuilds")).expect("created");
        held.lock().expect("locked");
        std::thread::scope(|scope| {
            let closing = scope.spawn(|| results.output.rebuild());
            std::thread::sleep(Duration::from_millis(300));
            let others = serde_json::json!({ "gallery": "0.0.0", "begun": next, "done": next });
            fs::write(dir.out().join(".gallery-rebuilt"), others.to_string()).expect("written");
            let page = "<title>passed · catalog tests</title>";
            fs::write(dir.out().join(PAGE), page).expect("written");
            drop(held);
            closing.join().expect("no panic").expect("rebuilt");
        });
        assert_eq!(dir.verdicts(), [passed("smoke::plays::one")]);
    }

    #[test]
    fn a_rebuild_is_counted_as_it_begins_and_done_only_with_its_page_in_place() {
        let dir = Scratch::new();
        let results = dir.results("plays", "1", &["plays::one"]);
        drawn(&results, "plays::one", None);
        results.close().expect("the report");
        let before = rebuilt(&dir);
        assert_eq!(before.begun, before.done);
        fs::write(dir.out().join(PAGE), "<h1>My site</h1>").expect("written");
        results.output.rebuild().expect_err("it is no report");
        let after = rebuilt(&dir);
        assert_eq!(
            (after.begun, after.done),
            (before.begun + 1, before.done),
            "one that fails is as one that is cut short"
        );
    }

    #[test]
    fn whoever_could_not_read_the_count_rebuilds_for_itself() {
        let count_of = |begun: u64, done: u64| {
            serde_json::json!({ "gallery": VERSION, "begun": begun, "done": done }).to_string()
        };
        for unreadable in [
            "half".to_owned(),
            count_of(u64::MAX, 0),
            count_of(0, u64::MAX),
        ] {
            let dir = Scratch::new();
            let results = dir.results("plays", "1", &["plays::one"]);
            drawn(&results, "plays::one", None);
            results.close().expect("the report");
            let held = fs::File::create(dir.out().join(".gallery-rebuilds")).expect("created");
            held.lock().expect("locked");
            fs::write(dir.out().join(".gallery-rebuilt"), &unreadable).expect("written");
            std::thread::scope(|scope| {
                let closing = scope.spawn(|| results.output.rebuild());
                std::thread::sleep(Duration::from_millis(300));
                count(&dir, 5, 5);
                fs::remove_file(dir.out().join(PAGE)).expect("removed");
                drop(held);
                closing.join().expect("no panic").expect("rebuilt");
            });
            assert_eq!(dir.verdicts(), [passed("smoke::plays::one")]);
        }
    }

    #[test]
    fn a_count_that_makes_no_sense_is_begun_anew() {
        for (begun, done) in [(u64::MAX, 0), (u64::MAX, u64::MAX), (3, u64::MAX)] {
            let dir = Scratch::new();
            let results = dir.results("plays", "1", &["plays::one"]);
            drawn(&results, "plays::one", None);
            results.close().expect("the report");
            count(&dir, begun, done);
            results.close().expect("the report");
            assert_eq!(dir.verdicts(), [passed("smoke::plays::one")]);
            assert_eq!(rebuilt(&dir).done, 1, "after {begun}, {done}");
        }
    }

    #[test]
    fn a_result_is_waited_to_be_written_whatever_was_warned_of_before() {
        let dir = Scratch::new();
        let results = dir.results("plays", "1", &["plays::one"]);
        results.output.bear(Err("something else".to_owned()));
        drawn(&results, "plays::one", None);
        let writes = fs::File::open(dir.out().join(".gallery-writes")).expect("opened");
        std::thread::scope(|scope| {
            run(&results, "plays::one", || {
                writes.lock().expect("locked");
                scope.spawn(|| {
                    std::thread::sleep(Duration::from_millis(300));
                    drop(writes);
                });
                Checked::failed(Diagnostic::new("it fails now"))
            });
        });
        results.close().expect("the report");
        assert!(dir.page().contains("it fails now"));
        let warned = dir.warned();
        assert!(warned[0].contains("something else"), "{warned:?}");
    }

    #[test]
    fn a_lock_that_was_stuck_is_still_waited_for_a_little_and_in_full_once_it_is_had() {
        let dir = Scratch::new();
        let results = dir.results("plays", "1", &["plays::one"]);
        drawn(&results, "plays::one", None);
        let stored = dir.stored("plays", "smoke/plays/one.json");
        let writes = fs::File::open(dir.out().join(".gallery-writes")).expect("opened");
        let written_after = |held: Duration| {
            writes.lock().expect("locked");
            std::thread::scope(|scope| {
                scope.spawn(|| {
                    std::thread::sleep(held);
                    writes.unlock().expect("unlocked");
                });
                results.output.write(&stored)
            })
        };
        let little = PATIENCE / 20;
        written_after(PATIENCE + little).expect_err("it is stuck");
        written_after(little / 2).expect("it is waited for a little");
        written_after(4 * little).expect("it is waited for in full");
    }

    #[test]
    fn a_file_system_without_locks_goes_without() {
        use fs::TryLockError::{Error, WouldBlock};
        assert!(is_had(Ok(())));
        assert!(!is_had(Err(WouldBlock)));
        for kind in [ErrorKind::Unsupported, ErrorKind::Other] {
            assert!(is_had(Err(Error(kind.into()))), "{kind}");
        }
        let interrupted = ErrorKind::Interrupted;
        assert!(!is_had(Err(Error(interrupted.into()))), "it is tried again");
    }

    #[test]
    fn a_process_that_holds_a_lock_for_ever_is_waited_for_once() {
        for lock in [".gallery-rebuilds", ".gallery-writes"] {
            let dir = Scratch::new();
            let scenes: Vec<String> = (0..8).map(|n| format!("plays::scene_{n}")).collect();
            let scenes: Vec<&str> = scenes.iter().map(String::as_str).collect();
            let results = dir.results("plays", "1", &scenes);
            fs::create_dir_all(dir.out()).expect("made");
            let held = fs::File::create(dir.out().join(lock)).expect("created");
            held.lock().expect("locked");
            let began = Instant::now();
            for scene in &scenes {
                drawn(&results, scene, None);
            }
            assert!(began.elapsed() < 4 * PATIENCE, "{lock}");
            assert_eq!(dir.warned().len(), 1);
        }
    }

    #[test]
    fn a_writer_that_is_suspended_keeps_no_result_off_the_report() {
        let dir = Scratch::new();
        let first = dir.results("plays", "1", &["plays::one", "plays::gone"]);
        for scene in ["plays::one", "plays::gone"] {
            drawn(&first, scene, None);
        }
        first.close().expect("the report");
        let second = dir.results("plays", "2", &["plays::one"]);
        let writes = fs::File::open(dir.out().join(".gallery-writes")).expect("opened");
        writes.lock_shared().expect("locked");
        drawn(&second, "plays::one", Some("it fails now"));
        second.close().expect("the report");
        assert_eq!(dir.verdicts().len(), 1);
        assert!(dir.page().contains("it fails now"));
    }

    /// NFS has no exclusive lock for a file opened to be read alone.
    #[test]
    fn a_lock_file_is_opened_to_be_written_too() {
        let dir = Scratch::new();
        let results = dir.results("plays", "1", &["plays::one"]);
        fs::create_dir_all(dir.out()).expect("made");
        for is_there in [false, true] {
            let lock = results
                .output
                .open_lock(".gallery-rebuilds")
                .expect("opened");
            assert!(lock.set_len(0).is_ok(), "there before: {is_there}");
        }
    }

    #[test]
    fn a_catalogs_order_is_written_once_it_can_be() {
        let dir = Scratch::new();
        let scenes = ["plays::b", "plays::a"];
        let results = dir.results("plays", "1", &scenes);
        let order = results.output.results().join("order.json");
        fs::create_dir_all(&order).expect("in the way");
        drawn(&results, "plays::b", None);
        until("its result is followed", || {
            dir.out().join(PAGE).exists() && dir.verdicts() == [passed("smoke::plays::b")]
        });
        run(&results, "plays::b", || {
            assert_eq!(dir.verdicts()[0].1, Verdict::Unfinished);
            Checked::default()
        });
        let warned = dir.warned();
        assert!(warned[0].contains("order.json"), "{warned:?}");
        fs::remove_dir(&order).expect("out of the way");
        drawn(&results, "plays::a", None);
        results.close().expect("the report");
        let names: Vec<String> = shown(&dir.page()).into_iter().map(|it| it.0).collect();
        assert_eq!(names, scenes.map(|scene| format!("smoke::{scene}")));
    }

    #[test]
    fn results_are_written_together_and_removed_alone() {
        let dir = Scratch::new();
        let scenes = ["plays::one", "plays::gone"];
        let first = dir.results("plays", "1", &scenes);
        for scene in scenes {
            drawn(&first, scene, None);
        }
        first.close().expect("the report");
        let path = dir
            .out()
            .join("plays/results-glow/smoke/plays/gone.fragment");
        let stored = dir.stored("plays", "smoke/plays/one.json");
        let second = dir.results("plays", "2", &scenes[..1]);
        let writes = fs::File::create(dir.out().join(".gallery-writes")).expect("created");

        writes.lock_shared().expect("locked");
        second.output.write(&stored).expect("written beside it");
        second.output.rebuild().expect("rebuilt");
        assert!(path.exists(), "another may be writing it");
        writes.unlock().expect("unlocked");
        second.output.rebuild().expect("rebuilt");
        assert!(!path.exists());
        assert!(!path.with_extension("json").exists());

        let order = second.output.results().join("order.json");
        let listed = || fs::read(&order).expect("read");
        let before = listed();
        let third = dir.results("plays", "3", &["plays::one", "plays::added"]);
        writes.lock().expect("locked");
        std::thread::scope(|scope| {
            let (wrote, written) = mpsc::channel();
            let (third, stored) = (&third, &stored);
            scope.spawn(move || wrote.send(third.output.write(stored)));
            let early = written.recv_timeout(Duration::from_millis(300));
            assert!(early.is_err(), "another may be removing it");
            assert_eq!(listed(), before);
            writes.unlock().expect("unlocked");
            written.recv().expect("written").expect("written");
        });
    }

    #[test]
    fn a_result_written_as_the_one_there_is_removed_is_kept() {
        let dir = Scratch::new();
        let scenes = ["plays::a/b", "plays::a:b"];
        let before = dir.results("plays", "1", &scenes);
        let now = dir.results("plays", "2", &scenes[1..]);
        drawn(&before, scenes[0], None);
        drawn(&now, scenes[1], None);
        let path = dir
            .out()
            .join("plays/results-glow/smoke/plays/a_b.fragment");
        let fresh = dir.stored("plays", "smoke/plays/a_b.json");
        let mut stale = fresh.kept.clone();
        stale.trial = "smoke::plays::a/b".to_owned();
        stale.at = Timestamp::UNIX_EPOCH;
        let stale = format!("{}\n", serde_json::to_string(&stale).expect("JSON"));
        let mut lost = 0;
        for _ in 0..300 {
            fs::write(&path, &stale).expect("written");
            std::thread::scope(|scope| {
                scope.spawn(|| now.output.write(&fresh).expect("written"));
                now.output.rebuild().expect("rebuilt");
            });
            lost += u32::from(!path.exists());
        }
        assert_eq!(lost, 0, "of 300 results");
    }

    #[test]
    fn a_report_that_cannot_be_written_is_warned_of_once_and_fails_no_test() {
        let dir = Scratch::new();
        let results = dir.results("plays", "1", &["plays::one", "plays::two"]);
        fs::write(dir.out(), "a file where the directory goes").expect("written");
        drawn(&results, "plays::one", None);
        drawn(&results, "plays::two", None);
        assert_eq!(results.close(), None);
        let warned = dir.warned();
        assert!(
            matches!(&warned[..], [one] if one.contains("the tests' report is incomplete")),
            "{warned:?}"
        );
    }

    #[test]
    fn a_result_that_cannot_be_read_is_replaced_and_hides_no_other() {
        let dir = Scratch::new();
        let results = dir.results("plays", "1", &["plays::one", "plays::two"]);
        drawn(&results, "plays::one", None);
        let other = results.output.results().join("smoke/plays/two.fragment");
        fs::write(&other, "{}\n").expect("written");
        results.close().expect("the report");
        assert_eq!(dir.verdicts().len(), 1);
        drawn(&results, "plays::two", None);
        results.close().expect("the report");
        assert_eq!(dir.verdicts().len(), 2);
    }

    #[test]
    fn a_file_that_is_no_result_is_let_be() {
        let dir = Scratch::new();
        let results = dir.results("plays", "1", &["plays::one"]);
        drawn(&results, "plays::one", None);
        let others = ["package.json", "one.fragment"].map(|name| {
            let other = results.output.results().join(name);
            fs::write(&other, "{}").expect("written");
            other
        });
        results.close().expect("the report");
        assert!(others.iter().all(|other| other.exists()));
    }

    #[test]
    fn a_page_that_is_no_report_is_not_written_over() {
        let dir = Scratch::new();
        fs::create_dir_all(dir.out()).expect("made");
        fs::write(dir.out().join(PAGE), "<h1>My site</h1>").expect("written");
        let results = dir.results("plays", "1", &["plays::one"]);
        drawn(&results, "plays::one", None);
        assert_eq!(results.close(), None);
        assert_eq!(dir.page(), "<h1>My site</h1>");
        let warned = dir.warned();
        assert!(
            matches!(&warned[..], [one] if one.contains("no report")),
            "{warned:?}"
        );
    }

    #[test]
    fn a_page_that_was_never_filled_is_written_over() {
        let dir = Scratch::new();
        fs::create_dir_all(dir.out()).expect("made");
        fs::write(dir.out().join(PAGE), "").expect("written");
        let results = dir.results("plays", "1", &["plays::one"]);
        drawn(&results, "plays::one", None);
        results.close().expect("the report");
        assert_eq!(dir.verdicts(), [passed("smoke::plays::one")]);
    }

    #[cfg(unix)]
    #[test]
    fn what_cannot_be_read_is_warned_of_and_looked_for_again() {
        use std::os::unix::fs::PermissionsExt as _;
        for what in [
            "other/results-glow",
            "other/results-glow/smoke/other",
            "other/results-glow/smoke/other/one.fragment",
            "other/results-glow/order.json",
        ] {
            let dir = Scratch::new();
            let other = dir.results("other", "1", &["other::one"]);
            drawn(&other, "other::one", Some("it failed"));
            other.close().expect("the report");
            let results = dir.results("plays", "1", &["plays::one"]);
            drawn(&results, "plays::one", None);
            results.close().expect("the report");
            assert_eq!(dir.verdicts().len(), 2);

            let path = dir.out().join(what);
            let mode = path.metadata().expect("there").permissions().mode();
            let set = |mode| fs::set_permissions(&path, fs::Permissions::from_mode(mode));
            set(0o000).expect("set");
            if fs::File::open(&path).is_ok() {
                return; // Root may read anything.
            }
            assert_eq!(results.close(), None, "{what}");
            let verdicts = dir.verdicts();
            assert!(verdicts.contains(&passed("smoke::plays::one")), "{what}");
            let warned = dir.warned();
            assert!(warned[0].contains(what), "{warned:?}");
            let count = rebuilt(&dir);
            assert!(count.done < count.begun, "{what}");
            set(mode).expect("set");
            results.output.rebuild_if_changed().expect("rebuilt");
            assert_eq!(dir.verdicts().len(), 2, "{what}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_directory_of_anothers_that_cannot_be_read_is_let_be() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = Scratch::new();
        let theirs = dir.out().join("lost+found");
        fs::create_dir_all(&theirs).expect("made");
        let set = |mode| fs::set_permissions(&theirs, fs::Permissions::from_mode(mode));
        set(0o000).expect("set");
        let results = dir.results("plays", "1", &["plays::one"]);
        drawn(&results, "plays::one", None);
        results.close().expect("the report");
        set(0o755).expect("set");
        assert_eq!(dir.verdicts(), [passed("smoke::plays::one")]);
    }

    #[cfg(unix)]
    #[test]
    fn a_page_that_cannot_be_read_is_not_written_over() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = Scratch::new();
        fs::create_dir_all(dir.out()).expect("made");
        let page = dir.out().join(PAGE);
        fs::write(&page, "<h1>My site</h1>").expect("written");
        let set = |mode| fs::set_permissions(&page, fs::Permissions::from_mode(mode));
        set(0o000).expect("set");
        if fs::File::open(&page).is_ok() {
            return; // Root may read anything.
        }
        let results = dir.results("plays", "1", &["plays::one"]);
        drawn(&results, "plays::one", None);
        assert_eq!(results.close(), None);
        set(0o644).expect("set");
        assert_eq!(dir.page(), "<h1>My site</h1>");
        let warned = dir.warned();
        assert!(warned[0].contains("index.html"), "{warned:?}");
    }

    #[test]
    fn what_a_test_says_is_no_markup() {
        let dir = Scratch::new();
        let scene = "plays::<b>bold</b>";
        let results = dir.results("plays", "1", &[scene]);
        drawn(&results, scene, Some("<script>alert(1)</script>"));
        results.close().expect("the report");
        let page = dir.page();
        assert!(!page.contains("<b>bold"), "{page}");
        assert!(!page.contains("<script>alert"), "{page}");
        assert!(page.contains("&#60;script&#62;alert(1)"), "{page}");
    }
}
