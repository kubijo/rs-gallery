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
    hot::HotDylib,
    render::{
        self, DEFAULT_SCALE, DEFAULT_SIZE, Overrides, Session, Shot, ShotPlay, Snapshots, Trace,
        Update,
    },
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
    let snapshots = Arc::new(Snapshots::new(&config.snapshots, renderer, update));

    let mut trials = Vec::new();
    for scene in &manifest.scenes {
        let key = scene_key(scene);
        let catalog = Catalog {
            manifest: Arc::clone(&manifest),
            globals,
            plays: Arc::clone(&plays),
            renderer,
            setup,
            snapshots: Arc::clone(&snapshots),
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
            snapshots: Arc::clone(&snapshots),
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

/// What every trial draws from, shared between the test threads.
struct Catalog {
    manifest: Arc<Manifest>,
    globals: Option<GlobalEntry>,
    plays: Arc<[PlayEntry]>,
    renderer: Renderer,
    setup: fn(&egui::Context),
    snapshots: Arc<Snapshots>,
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
                &self.snapshots,
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
            trace: Trace::RetainOnFailure,
            steps: false,
        }),
        record: None,
    }
}
