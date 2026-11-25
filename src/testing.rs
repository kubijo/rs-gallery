//! A catalog's own tests: every scene drawn and every play run,
//! as trials for `cargo test` or nextest.
//!
//! The scenes come from the same dylib the window loads,
//! built the same way, so a test crosses the boundary the window does.

use std::{cell::RefCell, mem::ManuallyDrop, process::ExitCode, sync::Arc};

use camino::Utf8Path;
use libtest_mimic::{Arguments, Failed, Trial};

use crate::{
    GlobalEntry, Manifest, PlayEntry, Renderer, SceneSource as _,
    diagnostic::Diagnostic,
    hot::HotDylib,
    render::{
        self, Checked, DEFAULT_SCALE, DEFAULT_SIZE, Films, Overrides, Session, Shot, ShotPlay,
        Snapshots, Testing, Trace, Update,
    },
    style::Stream,
    tree::scene_key,
    verdicts::{self, Results, Tested},
};

/// A catalog's test binary:
///  - `smoke::<scene>` for every scene
///  - `play::<scene>::<name>` for every play
///
/// In `tests/scenes.rs`, declared with `harness = false`:
///
/// ```ignore
/// gallery::test_main!(gallery::Renderer::Wgpu);
/// ```
///
/// A second argument is the setup the window gets,
/// e.g. fonts: `test_main!(renderer, |ctx| …)`.
#[macro_export]
macro_rules! test_main {
    ($renderer:expr) => {
        $crate::test_main!($renderer, |_| {});
    };
    ($renderer:expr, $setup:expr) => {
        fn main() -> ::std::process::ExitCode {
            $crate::__test_main(
                env!("CARGO_PKG_NAME"),
                env!("CARGO_MANIFEST_DIR"),
                $renderer,
                $setup,
            )
        }
    };
}

/// [`test_main!`](macro@crate::test_main)'s body:
/// build and load the scenes dylib, then run the trials.
///
/// # Panics
/// If `gallery.toml` can't be read, or the scenes
/// dylib can't be built or loaded.
#[doc(hidden)]
pub fn __test_main(
    package: &str,
    manifest_dir: &str,
    renderer: Renderer,
    setup: fn(&egui::Context),
) -> ExitCode {
    let args = Arguments::from_args();
    let config = gallery_build::Config::read(&Utf8Path::new(manifest_dir).join("gallery.toml"))
        .unwrap_or_else(|e| panic!("{e}"));
    let dylib = crate::launcher::build_lib(package, manifest_dir, &config.path, true);
    // `name` in cargo's target directory.
    let in_target = |name: &str| {
        let profile = dylib.parent().unwrap_or(&dylib);
        profile.with_file_name(name)
    };
    let mut source =
        HotDylib::at(dylib.clone(), false).expect("load the freshly built scenes dylib");
    let manifest = Arc::new(source.manifest());
    let globals = source.globals();
    let plays: Arc<[PlayEntry]> = source.plays().into();
    let update = Update::from_env().unwrap_or_else(|e| panic!("{e}"));
    let output = config.tests.output.unwrap_or_else(|| in_target("gallery"));
    let krate = package.replace('-', "_");
    let testing = Testing {
        snapshots: Snapshots::new(&config.snapshots, renderer, update),
        films: Films::new(
            output.clone(),
            in_target("gallery-recordings"),
            config.tests.video,
            krate.clone(),
        ),
        warn: Diagnostic::report,
        styled: styled_for(&args, crate::style::coloured(Stream::Stdout)),
    };
    let tests = tests_of(&manifest, &plays);
    let names = tests.iter().map(|test| test.name.clone());
    let tested = Tested {
        output,
        krate,
        manifest_dir: manifest_dir.into(),
        renderer: renderer.name(),
    };
    let results = Results::new(tested, names, testing.warn);
    let catalog = Arc::new(Catalog {
        manifest,
        globals,
        plays,
        renderer,
        setup,
        testing,
        results,
    });

    let trials = tests
        .into_iter()
        .map(|test| {
            let catalog = Arc::clone(&catalog);
            let kind = test.kind();
            Trial::test(test.name.clone(), move || catalog.run(&test)).with_kind(kind)
        })
        .collect();
    let conclusion = libtest_mimic::run(&args, trials);
    let page = catalog.results.close();
    // Under nextest every test is a process, and would say so.
    if let Some(page) = page
        && std::env::var_os("NEXTEST").is_none()
        && !args.quiet
    {
        anstream::eprintln!("report: {}", crate::style::path_on_stderr(&page));
    }
    conclusion.exit_code()
}

/// One test of a catalog.
struct Test {
    name: String,
    scene: String,
    /// The play it runs, if it does not just draw the scene.
    play: Option<PlayEntry>,
}

impl Test {
    fn kind(&self) -> &'static str {
        match self.play {
            Some(_) => "play",
            None => "smoke",
        }
    }
}

/// A test for each scene of `manifest`, and one for each of `plays`.
fn tests_of(manifest: &Manifest, plays: &[PlayEntry]) -> Vec<Test> {
    let smoke = manifest.scenes.iter().map(|scene| {
        let scene = scene_key(scene);
        Test {
            name: format!("smoke::{scene}"),
            scene,
            play: None,
        }
    });
    let played = plays.iter().map(|play| {
        let scene = play.scene.to_string();
        Test {
            name: format!("play::{scene}::{}", play.name),
            scene,
            play: Some(*play),
        }
    });
    smoke.chain(played).collect()
}

/// Whether failures are printed in colour, with links, when stdout is `coloured`:
/// a log file takes stdout's place.
fn styled_for(args: &Arguments, coloured: bool) -> bool {
    args.logfile.is_none() && coloured
}

/// What every trial draws from,
/// shared between the test threads.
struct Catalog {
    manifest: Arc<Manifest>,
    globals: Option<GlobalEntry>,
    plays: Arc<[PlayEntry]>,
    renderer: Renderer,
    setup: fn(&egui::Context),
    testing: Testing,
    results: Results,
}

thread_local! {
    /// One per test thread, opened by its first trial: a glow session is not `Send`.
    ///
    /// Never dropped: dropping it while the thread's locals are torn down reaches for others
    /// already gone, and the test binary aborts with "thread local panicked on drop".
    /// The worker threads last until the process ends, which reclaims it.
    static SESSION: RefCell<Option<ManuallyDrop<Session>>> = const { RefCell::new(None) };
}

impl Catalog {
    fn run(&self, test: &Test) -> Result<(), Failed> {
        let trial = verdicts::Trial {
            name: &test.name,
            scene: &test.scene,
            play: test.play.as_ref(),
        };
        let shot = shot(test.scene.clone(), test.play.map(|play| play.name));
        let checked = self.results.of(trial, || self.shoot(&shot, Session::open));
        checked.outcome(self.testing.styled).map_err(Failed::from)
    }

    /// Draw `shot` in the thread's session, which it is to `open` if there is none.
    fn shoot(
        &self,
        shot: &Shot,
        open: impl FnOnce(Renderer) -> Result<Session, Diagnostic>,
    ) -> Checked {
        SESSION.with_borrow_mut(|session| {
            let session = match session {
                Some(session) => session,
                None => match open(self.renderer) {
                    Ok(opened) => session.insert(ManuallyDrop::new(opened)),
                    Err(failure) => return Checked::failed(failure),
                },
            };
            render::check_shot(
                &self.manifest,
                self.globals,
                &self.plays,
                session,
                &self.setup,
                shot,
                &self.testing,
            )
        })
    }
}

/// The scene at its defaults, running `play` when there is one;
/// drawn, settled, and written nowhere.
fn shot(scene: String, play: Option<&str>) -> Shot {
    Shot {
        scene,
        out: None,
        size: DEFAULT_SIZE,
        scale: DEFAULT_SCALE,
        knobs: Overrides::default(),
        globals: Overrides::default(),
        frames: None,
        trim: true,
        settle: true,
        list: false,
        template: false,
        play: play.map(|name| ShotPlay {
            name: name.to_owned(),
            trace: Trace::Failing,
            steps: false,
        }),
        record: None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use gallery_build::Video;

    use super::*;
    use crate::{Play, PlayResult, SceneCtx, SceneEntry, SceneKey};

    /// A catalog of one scene, with a play that passes and one that fails;
    /// and what its report warns of.
    fn catalog(dir: &Utf8Path) -> (Catalog, Vec<Test>, Arc<Mutex<Vec<String>>>) {
        fn says_hello(_: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
            ui.label("Hello");
        }
        fn passes(p: &mut Play<'_>) -> PlayResult {
            p.expect("Hello").to_exist()
        }
        fn fails(p: &mut Play<'_>) -> PlayResult {
            p.expect("Goodbye").within(0).to_exist()
        }
        let manifest = Manifest {
            scenes: vec![SceneEntry {
                render: says_hello,
                name: "Greeting",
                module_path: "plays",
                default: true,
                order: 0,
                source: "",
            }],
            groups: Vec::new(),
        };
        let play = |name, run| PlayEntry {
            scene: SceneKey {
                module_path: "plays",
                name: "Greeting",
            },
            name,
            module_path: "plays",
            file: file!(),
            run,
        };
        let plays = [play("passes", passes), play("fails", fails)];
        let tests = tests_of(&manifest, &plays);
        let manifest_of_plays = "[package]\nname = \"plays\"\n";
        std::fs::write(dir.join("Cargo.toml"), manifest_of_plays).expect("written");
        let tested = Tested {
            output: dir.join("out"),
            krate: "plays".to_owned(),
            manifest_dir: dir.to_owned(),
            renderer: "glow",
        };
        let warn: fn(&Diagnostic) = |warning| panic!("{}", warning.plain());
        let warned = Arc::new(Mutex::new(Vec::new()));
        let warnings = Arc::clone(&warned);
        let names = tests.iter().map(|test| test.name.clone());
        let catalog = Catalog {
            manifest: Arc::new(manifest),
            globals: None,
            plays: Arc::new(plays),
            renderer: Renderer::Glow,
            setup: |_| {},
            testing: Testing {
                snapshots: Snapshots::default(),
                films: Films::new(
                    dir.join("out"),
                    dir.join("recordings"),
                    Video::Off,
                    "plays".to_owned(),
                ),
                warn,
                styled: false,
            },
            results: Results::new(tested, names, move |warning: &Diagnostic| {
                warnings.lock().expect("no panic").push(warning.plain());
            }),
        };
        (catalog, tests, warned)
    }

    #[test]
    fn a_test_passes_or_fails_as_its_scene_and_play_do() {
        let dir = tempfile::tempdir().expect("a directory");
        let dir = Utf8Path::from_path(dir.path()).expect("UTF-8");
        let (catalog, tests, warned) = catalog(dir);
        let outcomes: Vec<_> = tests
            .iter()
            .map(|test| (test.name.as_str(), catalog.run(test).is_ok()))
            .collect();
        assert_eq!(
            outcomes,
            [
                ("smoke::plays::Greeting", true),
                ("play::plays::Greeting::passes", true),
                ("play::plays::Greeting::fails", false),
            ]
        );
        let page = catalog.results.close().expect("the report");
        assert_eq!(*warned.lock().expect("no panic"), Vec::<String>::new());
        let page = std::fs::read_to_string(page).expect("read");
        assert!(page.contains("<title>1 failed"), "{page}");
    }

    #[test]
    fn a_test_fails_that_there_is_nothing_to_draw_on_for() {
        let dir = tempfile::tempdir().expect("a directory");
        let (catalog, tests, _) = catalog(Utf8Path::from_path(dir.path()).expect("UTF-8"));
        let shot = shot(tests[0].scene.clone(), None);
        let checked = catalog.shoot(&shot, |_| Err(Diagnostic::new("no EGL device")));
        let failure = checked.outcome(false).expect_err("it was not drawn");
        assert!(failure.contains("no EGL device"), "{failure}");
    }

    #[test]
    fn a_log_file_takes_no_colour_whatever_the_terminal_does() {
        let args = |logfile: Option<&str>| Arguments {
            logfile: logfile.map(str::to_owned),
            ..Arguments::default()
        };
        assert!(styled_for(&args(None), true));
        assert!(!styled_for(&args(None), false));
        assert!(!styled_for(&args(Some("log.txt")), true));
    }
}
