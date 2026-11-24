//! A catalog's own tests: every scene drawn and every play run,
//! as trials for `cargo test` or nextest.
//!
//! The scenes come from the same dylib the window loads,
//! built the same way, so a test crosses the boundary the window does.

use std::{cell::RefCell, mem::ManuallyDrop, process::ExitCode, sync::Arc};

use camino::{Utf8Path, Utf8PathBuf};
use libtest_mimic::{Arguments, Failed, Trial};

use crate::{
    GlobalEntry, Manifest, PlayEntry, Renderer, SceneSource as _,
    diagnostic::Diagnostic,
    hot::{HotDylib, profile_dir},
    render::{
        self, DEFAULT_SCALE, DEFAULT_SIZE, Films, Overrides, Session, Shot, ShotPlay, Snapshots,
        Testing, Trace, Update,
    },
    style::Stream,
    tree::scene_key,
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
    crate::launcher::build_lib(manifest_dir, &config.path, true);
    let mut source = HotDylib::new(&package.replace('-', "_"), false)
        .expect("load the freshly built scenes dylib");
    let manifest = Arc::new(source.manifest());
    let globals = source.globals();
    let plays: Arc<[PlayEntry]> = source.plays().into();
    let update = Update::from_env().unwrap_or_else(|e| panic!("{e}"));
    let testing = Arc::new(Testing {
        snapshots: Snapshots::new(&config.snapshots, renderer, update),
        films: Films::new(
            config.tests.output.unwrap_or_else(|| in_target("gallery")),
            in_target("gallery-recordings"),
            config.tests.video,
            package.replace('-', "_"),
        ),
        warn: Diagnostic::report,
        styled: styled_for(&args, crate::style::coloured(Stream::Stdout)),
    });

    let mut trials = Vec::new();
    for scene in &manifest.scenes {
        let key = scene_key(scene);
        let catalog = Catalog {
            manifest: Arc::clone(&manifest),
            globals,
            plays: Arc::clone(&plays),
            renderer,
            setup,
            testing: Arc::clone(&testing),
        };
        trials.push(
            Trial::test(format!("smoke::{key}"), move || {
                catalog.shoot(shot(key, None))
            })
            .with_kind("smoke"),
        );
    }
    for play in plays.iter() {
        let key = play.scene.to_string();
        let catalog = Catalog {
            manifest: Arc::clone(&manifest),
            globals,
            plays: Arc::clone(&plays),
            renderer,
            setup,
            testing: Arc::clone(&testing),
        };
        let name = play.name;
        trials.push(
            Trial::test(format!("play::{key}::{name}"), move || {
                catalog.shoot(shot(key, Some(name)))
            })
            .with_kind("play"),
        );
    }
    libtest_mimic::run(&args, trials).exit_code()
}

/// `name` in cargo's target directory.
fn in_target(name: &str) -> Utf8PathBuf {
    let profile = profile_dir().unwrap_or_else(|e| panic!("find the target directory: {e}"));
    profile.with_file_name(name)
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
    testing: Arc<Testing>,
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
    fn shoot(&self, shot: Shot) -> Result<(), Failed> {
        SESSION.with_borrow_mut(|session| {
            let session = match session {
                Some(session) => session,
                None => session.insert(ManuallyDrop::new(
                    Session::open(self.renderer).map_err(|e| e.plain())?,
                )),
            };
            render::check_shot(
                &self.manifest,
                self.globals,
                &self.plays,
                session,
                &self.setup,
                &shot,
                &self.testing,
            )
            .map_err(Failed::from)
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
    use super::*;

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
