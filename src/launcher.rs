//! The config-driven entry point: read `gallery.toml`, build the scenes dylib from its globs, load
//! it, and open the window — plus the cargo plumbing that does the building and the watching.

use std::{
    collections::{HashMap, HashSet},
    io::{IsTerminal as _, Write as _},
    process::Command,
};

use camino::{Utf8Path, Utf8PathBuf};
use clap::Parser;

use crate::{
    HotDylib, RunOptions, SceneSource, Settings,
    diagnostic::Diagnostic,
    render, run_with,
    tree::resolve_scene,
    update::check_updates,
    watch::{self, HotStatus, SceneWatcher},
};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The consumer's entire `main`. Both arguments are required
/// — a `setup` closure and [`Settings`], which names
/// the [`Renderer`](crate::Renderer):
///
/// ```ignore
/// fn main() -> gallery::eframe::Result {
///     gallery::launch!(|_| {}, gallery::Settings::new(gallery::Renderer::Wgpu))
/// }
/// ```
///
/// Expands to [`launch()`] with the calling crate's name and manifest dir filled in.
/// `setup` runs against the fresh egui context
/// (e.g. `|ctx| gallery::egui_extras::install_image_loaders(ctx)`).
#[macro_export]
macro_rules! launch {
    ($setup:expr, $settings:expr) => {
        $crate::launch(
            env!("CARGO_PKG_NAME"),
            env!("CARGO_MANIFEST_DIR"),
            $settings,
            $setup,
        )
    };
}

/// Read the config, build the scenes dylib from its globs, load it,
/// and open the window — or, headlessly, render scenes to PNGs instead.
///
/// Prefer the [`launch!`](macro@crate::launch) macro, which fills `package`/`manifest_dir` from the calling crate.
/// `--help` lists the arguments.
///
/// # Panics
/// If the config can't be read or parsed, or the scenes dylib can't be built or loaded.
/// A bad argument, an unmatched scene or knob, or a failed render exits with a message instead.
pub fn launch(
    package: &str,
    manifest_dir: &str,
    settings: Settings,
    setup: impl Fn(&egui::Context) + 'static,
) -> eframe::Result {
    let cli = Cli::parse();
    anstream::eprintln!("{}", version_banner());
    // Before the config is read, so a stale or broken `gallery.toml`
    // can't stop you finding out that the pinned version is the reason.
    if cli.check_updates {
        check_updates();
        return Ok(());
    }
    let config_path = cli
        .config
        .clone()
        .unwrap_or_else(|| Utf8Path::new(manifest_dir).join("gallery.toml"));
    let config = gallery_build::Config::read(&config_path).unwrap_or_else(|e| panic!("{e}"));

    build_lib(manifest_dir, &config.path, headless(&cli));
    // Made here because both sides are handed it from here: the watcher reports into it,
    // and the shell draws from it.
    let hot = HotStatus::new();
    // The dylib is `lib<crate>.so`; the crate's lib name is the package name with dashes as underscores.
    let mut source = HotDylib::new(&package.replace('-', "_"), cli.hot)
        .expect("load the freshly built scenes dylib");
    if cli.hot {
        source.hot = Some(hot.clone());
    }
    let manifest = source.manifest();

    // Everything above is how scenes get here at all, so a headless run branches only where the window
    // would have opened — before any watcher, which nothing would be left to shut down.
    if let Some(capture) = shots(&cli, &config.path) {
        return match render::render(&manifest, settings.renderer, &setup, &capture) {
            Ok(()) => Ok(()),
            Err(reason) => fail(&reason),
        };
    }

    // Matched here rather than mid-frame: a miss can still be reported and exited on.
    let scene = cli.scene.as_ref().map(|pattern| {
        resolve_scene(&manifest.scenes, pattern).unwrap_or_else(|reason| fail(&reason))
    });
    let watcher = cli.hot.then(|| watch_scenes(manifest_dir, &config, &hot));
    let options = RunOptions {
        frames: cli.frames,
        scene: scene.map(crate::tree::scene_key),
        hot: cli.hot.then_some(hot),
    };
    let title = window_title(&config.title);
    let result = run_with(&title, source, settings, setup, options);
    // Window closed normally: stop the watcher (the Ctrl-C/SIGTERM path is handled in watch_scenes).
    if let Some(watcher) = &watcher {
        watcher.stop();
    }
    result
}

fn version_banner() -> String {
    format!("gallery {VERSION}")
}

fn window_title(title: &str) -> String {
    format!("{title} · {}", version_banner())
}

/// Report a headless failure the way clap reports a bad argument — on stderr, then a non-zero exit —
/// rather than a panic, which buries the part the caller has to act on under a backtrace notice.
fn fail(problem: &Diagnostic) -> ! {
    problem.report();
    std::process::exit(1)
}

/// Whether this run writes to stdout and exits instead of opening a window.
fn headless(cli: &Cli) -> bool {
    cli.capture.is_some() || cli.render.is_some() || cli.list_knobs || cli.init_capture
}

/// The shots this invocation asks for, or `None` when it wants the window.
///
/// A recipe describes its own. The single-scene flags describe one shot between them,
/// so asking for several at once stays coherent — a listing or a generated recipe
/// then says what the image beside it was set to.
fn shots(cli: &Cli, config: &Utf8Path) -> Option<render::Capture> {
    if !headless(cli) {
        return None;
    }
    if let Some(recipe) = &cli.capture {
        // Relative to the config, like the scene globs: both describe the instance, not the shell.
        let recipe = config.parent().unwrap_or(Utf8Path::new(".")).join(recipe);
        return Some(
            render::read_recipe(&recipe, cli.out.as_deref()).unwrap_or_else(|reason| fail(&reason)),
        );
    }
    let scene = cli.scene.clone().expect("clap requires --scene for these");
    // `--frames` also drives a windowed profiling run, where any count is meaningful,
    // so it is checked here rather than by clap.
    render::check_frames(cli.frames).unwrap_or_else(|reason| fail(&reason.into()));
    let scale = cli.scale.unwrap_or(render::DEFAULT_SCALE);
    render::check_scale(scale).unwrap_or_else(|reason| fail(&reason.into()));
    Some(render::Capture {
        shots: vec![render::Shot {
            scene,
            out: cli.render.clone(),
            size: cli
                .size
                .as_deref()
                .map_or(Ok(render::DEFAULT_SIZE), render::parse_size)
                .unwrap_or_else(|reason| fail(&reason.into())),
            scale,
            knobs: Vec::new(),
            frames: cli.frames,
            trim: !cli.no_trim,
            // A recipe option: one shot on the command line is drawn as asked.
            settle: false,
            list: cli.list_knobs,
            template: cli.init_capture,
        }],
        // Both gather a recipe's shots; one shot on the command line has nothing to gather.
        sheet: None,
        report: None,
    })
}

// This doc comment is the `--help` text, so it reads as instructions rather than as rationale; why
// knobs are set in a file instead of in flags is in the `render` module's own docs.
/// An egui component gallery: browse scenes in a window, or render them to PNGs headlessly.
///
/// Knob values are set in a `--capture` recipe rather than in flags.
/// `--list-knobs` prints the labels a scene declares; `--render` captures one scene as it stands.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Config to read scene globs from [default: <manifest-dir>/gallery.toml]
    #[arg(long, value_name = "PATH")]
    config: Option<Utf8PathBuf>,

    /// Rebuild and hot-swap scenes as they are edited
    #[arg(long, conflicts_with_all = ["render", "capture", "list_knobs", "init_capture"])]
    hot: bool,

    /// Report whether a newer gallery is out, and what changed since this one
    #[arg(long)]
    check_updates: bool,

    /// The scene: a whole key, else a case-insensitive regex. Must match exactly one
    #[arg(long, value_name = "PATTERN")]
    scene: Option<String>,

    /// Frames to draw before exiting (windowed) or capturing (headless)
    #[arg(long, value_name = "N")]
    frames: Option<u32>,

    /// Render one scene's canvas to a PNG, at its default knobs, and exit
    #[arg(long, value_name = "PATH", requires = "scene")]
    render: Option<Utf8PathBuf>,

    /// Canvas size to render at [default: 1280x720]
    #[arg(long, value_name = "WxH")]
    size: Option<String>,

    /// Device pixels to the point, as a display's scale factor would set it — the same
    /// picture at that many times the pixels [default: 1]
    #[arg(long, value_name = "N")]
    scale: Option<f32>,

    /// Keep the whole canvas, rather than cropping the PNG to what the scene drew
    #[arg(long)]
    no_trim: bool,

    /// Print the scene's knobs, their kinds and their values, and exit
    #[arg(long, requires = "scene")]
    list_knobs: bool,

    /// Print a capture recipe for the scene, its knobs filled in at their current values, and exit
    #[arg(long, requires = "scene")]
    init_capture: bool,

    /// Render every shot in a capture recipe (TOML, relative to the config) and exit
    #[arg(long, value_name = "PATH", conflicts_with_all = ["render", "scene", "list_knobs", "init_capture", "size", "scale", "no_trim"])]
    capture: Option<Utf8PathBuf>,

    /// Where --capture writes, overriding the recipe's own `out`
    #[arg(long, value_name = "DIR", requires = "capture")]
    out: Option<Utf8PathBuf>,
}

/// Build the scenes dylib once, blocking, so the loader finds a `.so` on first launch.
///
/// A headless run's entire output is the paths it wrote, which a screen of `Compiling` lines
/// buries. `--quiet` drops that progress and still lets errors through.
fn build_lib(manifest_dir: &str, config: &Utf8Path, quiet: bool) {
    let mut command = build_command(manifest_dir, config);
    if quiet {
        command.arg("--quiet");
    }
    let progress = Progress::start(quiet);
    let built = command.status().is_ok_and(|status| status.success());
    progress.clear();
    assert!(built, "`cargo build --lib` for the scenes dylib failed");
}

/// A line saying the scenes are compiling, rubbed out again when they are.
///
/// A quiet build prints nothing for as long as it takes, which reads as a hang.
/// This goes on stderr, so `--init-capture`'s TOML on stdout stays pipeable, and only to a terminal,
/// since a CI log has no cursor to move back over.
struct Progress(bool);

impl Progress {
    const LINE: &'static str = "gallery: compiling scenes…";

    fn start(quiet: bool) -> Self {
        let shown = quiet && std::io::stderr().is_terminal();
        if shown {
            eprint!("{}", Self::LINE);
            let _ = std::io::stderr().flush();
        }
        Self(shown)
    }

    fn clear(self) {
        if self.0 {
            eprint!("\r{:width$}\r", "", width = Self::LINE.chars().count());
            let _ = std::io::stderr().flush();
        }
    }
}

/// The build the first one and every rebuild both run, so they write the one dylib the loader reads.
fn build_command(manifest_dir: &str, config: &Utf8Path) -> Command {
    let mut command = cargo(manifest_dir, config);
    command.args(["build", "--lib"]);
    if let Some(profile) = host_profile() {
        command.args(["--profile", &profile]);
    }
    command
}

/// The cargo profile this binary was built under, read off its own path: cargo drops the host binary
/// in `<target>/<profile-dir>/`, and every profile's directory is its own name — `dev` alone differs,
/// building into `debug`.
///
/// The scenes dylib has to match, because [`HotDylib`] loads it from the executable's directory. Built
/// under any other profile it lands somewhere nothing reads, and the cold compile that produced it is
/// pure cost.
fn host_profile() -> Option<String> {
    let exe = Utf8PathBuf::from_path_buf(std::env::current_exe().ok()?).ok()?;
    let dir = exe.parent()?.file_name()?;
    Some(if dir == "debug" { "dev" } else { dir }.to_owned())
}

/// Rebuild the scenes dylib on every scene change; each fresh `.so` is what [`HotDylib`] reloads,
/// and the cycle between the two is what `hot` carries to the shell.
///
/// A build runs as a process group (unix) / job object (windows), so killing it takes down its whole
/// tree — on window close (via the returned handle) and on Ctrl-C/SIGTERM (via the handler).
fn watch_scenes(
    manifest_dir: &str,
    config: &gallery_build::Config,
    hot: &HotStatus,
) -> SceneWatcher {
    let paths = watch_paths(manifest_dir, &config.hot_watch_paths);
    // Owned by the closure: it outlives this call, being what every later rebuild is made from.
    let manifest_dir = manifest_dir.to_owned();
    let config = config.path.clone();
    let watcher = watch::spawn(
        paths,
        Box::new(move || build_command(&manifest_dir, &config)),
        hot,
    );

    let on_signal = watcher.clone();
    if let Err(e) = ctrlc::set_handler(move || {
        on_signal.stop();
        std::process::exit(130);
    }) {
        eprintln!("gallery: no signal handler — a build may outlive the window: {e}");
    }

    watcher
}

/// A cargo command in the crate dir, naming the config only where `build.rs` would not find it.
///
/// The path rather than the globs it declares, so a bare `cargo build` and this one read the same
/// file rather than restating each other's globs. And nothing at all for the usual config:
/// a variable named in `rerun-if-env-changed` is part of the build script's fingerprint,
/// so one appearing and disappearing as you alternate rebuilds the dylib whatever the globs say.
fn cargo(manifest_dir: &str, config: &Utf8Path) -> Command {
    let mut command = Command::new("cargo");
    command.current_dir(manifest_dir);
    for name in inherited_from_cargo() {
        command.env_remove(name);
    }
    if config != default_config(manifest_dir) {
        command.env("GALLERY_CONFIG", config.as_str());
    }
    command
}

/// The variables cargo set for *this* binary, which describe the package the shell was launched from
/// and mean nothing to the build it is asking for.
///
/// They are inherited otherwise, and a build script that reads one has it in its fingerprint.
/// `ring` reads `CARGO_MANIFEST_DIR`: cargo saw it present under the launcher and absent under
/// a bare `cargo build`, so `ring` rebuilt on every alternation and took the dylib with it.
///
/// `CARGO_HOME`, `CARGO_TARGET_DIR` and the rest of the settings are the user's, and stay.
fn inherited_from_cargo() -> Vec<String> {
    std::env::vars()
        .map(|(name, _)| name)
        .filter(|name| {
            name.starts_with("CARGO_PKG_")
                || matches!(
                    name.as_str(),
                    "CARGO_MANIFEST_DIR"
                        | "CARGO_MANIFEST_PATH"
                        | "CARGO_CRATE_NAME"
                        | "CARGO_BIN_NAME"
                        | "CARGO_PRIMARY_PACKAGE"
                        | "OUT_DIR"
                )
        })
        .collect()
}

/// The `gallery.toml` a scenes `build.rs` reads when nothing points it elsewhere,
/// canonicalised as [`gallery_build::Config`] canonicalises the one it is given.
fn default_config(manifest_dir: &str) -> Utf8PathBuf {
    let path = Utf8Path::new(manifest_dir).join("gallery.toml");
    path.canonicalize_utf8().unwrap_or(path)
}

fn watch_paths(manifest_dir: &str, configured: &[Utf8PathBuf]) -> Vec<Utf8PathBuf> {
    let mut paths = match local_package_roots(manifest_dir) {
        Ok(paths) => paths,
        Err(e) => {
            eprintln!("gallery: file watcher: {e}");
            vec![canonical(Utf8Path::new(manifest_dir))]
        }
    };
    paths.extend(configured.iter().cloned());
    paths.sort();
    paths.dedup();
    paths.into_iter().fold(Vec::new(), |mut roots, path| {
        if !roots
            .iter()
            .any(|root: &Utf8PathBuf| root.is_dir() && path.starts_with(root))
        {
            roots.push(path);
        }
        roots
    })
}

fn local_package_roots(manifest_dir: &str) -> Result<Vec<Utf8PathBuf>, String> {
    let manifest_dir = canonical(Utf8Path::new(manifest_dir));
    let manifest = manifest_dir.join("Cargo.toml");
    let metadata = cargo_metadata::MetadataCommand::new()
        .current_dir(&manifest_dir)
        .manifest_path(&manifest)
        .exec()
        .map_err(|e| format!("cannot read Cargo metadata for `{manifest}`: {e}"))?;
    let package = metadata
        .packages
        .iter()
        .find(|package| package.manifest_path.parent() == Some(manifest_dir.as_path()))
        .ok_or_else(|| format!("Cargo metadata has no package at `{manifest_dir}`"))?;
    let resolve = metadata
        .resolve
        .as_ref()
        .ok_or_else(|| format!("Cargo metadata did not resolve `{manifest}`"))?;
    let nodes: HashMap<_, _> = resolve.nodes.iter().map(|node| (&node.id, node)).collect();
    let packages: HashMap<_, _> = metadata
        .packages
        .iter()
        .map(|package| (&package.id, package))
        .collect();
    let mut pending = vec![package.id.clone()];
    let mut reached = HashSet::new();
    while let Some(id) = pending.pop() {
        if !reached.insert(id.clone()) {
            continue;
        }
        if let Some(node) = nodes.get(&id) {
            pending.extend(node.dependencies.iter().cloned());
        }
    }

    let mut roots: Vec<Utf8PathBuf> = reached
        .iter()
        .filter_map(|id| packages.get(id))
        .filter(|package| package.source.is_none())
        .filter_map(|package| package.manifest_path.parent().map(canonical))
        .collect();
    roots.sort();
    roots.dedup();
    Ok(roots)
}

fn canonical(path: &Utf8Path) -> Utf8PathBuf {
    path.canonicalize_utf8().unwrap_or_else(|_| path.to_owned())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn the_running_version_is_visible_in_the_terminal_and_window_title() {
        assert_eq!(version_banner(), format!("gallery {VERSION}"));
        assert_eq!(
            window_title("components"),
            format!("components · gallery {VERSION}")
        );
    }

    /// Through clap rather than a hand-built `Cli`, so the flags themselves are part of what is tested.
    fn cli(args: &[&str]) -> Cli {
        Cli::parse_from(args)
    }

    fn one_shot(args: &[&str]) -> render::Shot {
        let capture = shots(&cli(args), Utf8Path::new("gallery.toml")).expect("a headless run");
        assert!(capture.sheet.is_none(), "one shot has nothing to gather");
        assert_eq!(capture.shots.len(), 1, "one scene is one shot");
        capture.shots.into_iter().next().expect("the one shot")
    }

    #[test]
    fn a_run_is_headless_only_when_it_asks_for_something_written() {
        for (args, headless_run) in [
            (&["gallery"][..], false),
            (
                &["gallery", "--scene", "orbit", "--render", "o.png"][..],
                true,
            ),
            (&["gallery", "--scene", "orbit", "--list-knobs"][..], true),
            (&["gallery", "--scene", "orbit", "--init-capture"][..], true),
            (&["gallery", "--capture", "capture.toml"][..], true),
        ] {
            assert_eq!(headless(&cli(args)), headless_run, "{args:?}");
        }
        assert!(
            shots(&cli(&["gallery"]), Utf8Path::new("gallery.toml")).is_none(),
            "a windowed run asks for no shots"
        );
    }

    #[test]
    fn one_scene_on_the_command_line_becomes_one_shot_at_the_defaults() {
        let shot = one_shot(&["gallery", "--scene", "orbit", "--render", "orbit.png"]);
        assert_eq!(shot.scene, "orbit");
        assert_eq!(shot.out.as_deref(), Some(Utf8Path::new("orbit.png")));
        assert_eq!(shot.size, render::DEFAULT_SIZE);
        assert!(shot.trim, "a command-line render crops like a recipe does");
        assert!(!shot.list && !shot.template, "a render only renders");
    }

    /// A recipe-less run has nowhere to write `trim = false`, so the flag is the only way to keep the canvas.
    #[test]
    fn no_trim_keeps_the_whole_canvas_that_a_render_would_otherwise_crop() {
        let shot = one_shot(&[
            "gallery",
            "--scene",
            "orbit",
            "--render",
            "orbit.png",
            "--no-trim",
            "--size",
            "800x600",
        ]);
        assert!(!shot.trim, "--no-trim leaves the size asked for");
        assert_eq!(
            shot.size,
            egui::vec2(800.0, 600.0),
            "--size replaces the default"
        );
    }

    fn local_package(path: &Utf8Path, name: &str, dependencies: &str, source: &str) {
        fs::create_dir_all(path.join("src")).expect("package source directory");
        fs::write(
            path.join("Cargo.toml"),
            format!(
                "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\
                 [dependencies]\n{dependencies}"
            ),
        )
        .expect("package manifest");
        fs::write(path.join("src/lib.rs"), source).expect("package source");
    }

    #[test]
    fn hot_watch_reaches_every_transitive_local_path_dependency_and_explicit_path() {
        let root = Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .expect("a UTF-8 temp dir")
            .join("gallery-local-dependencies");
        let _ = fs::remove_dir_all(&root);
        let app = root.join("gallery");
        let feature = root.join("feature");
        let ui = root.join("ui");
        let unrelated = root.join("unrelated");
        local_package(
            &app,
            "app-gallery",
            "feature = { path = \"../feature\" }\n",
            "include!(\"example.scene.rs\");\n",
        );
        fs::write(
            app.join("src/example.scene.rs"),
            "pub fn scene() { feature::draw(); }\n",
        )
        .expect("scene calling the sibling component crate");
        fs::create_dir(app.join(".cargo")).expect("fixture Cargo config directory");
        fs::write(app.join(".cargo/config.toml"), "[net]\noffline = true\n")
            .expect("fixture Cargo config");
        local_package(
            &feature,
            "feature",
            "ui = { path = \"../ui\" }\n",
            "pub fn draw() { ui::draw(); }\n",
        );
        local_package(&ui, "ui", "serde = \"1\"\n", "pub fn draw() {}\n");
        local_package(&unrelated, "unrelated", "", "pub fn draw() {}\n");
        let asset = root.join("shared/palette.json");
        fs::create_dir_all(asset.parent().expect("asset directory")).expect("asset directory");
        fs::write(&asset, "{}").expect("asset");

        let watched = watch_paths(app.as_str(), std::slice::from_ref(&asset));

        for path in [&app, &feature, &ui, &asset] {
            assert!(watched.contains(path), "`{path}` is watched: {watched:?}");
        }
        assert!(
            !watched.contains(&unrelated),
            "a local package outside the resolved graph is not watched: {watched:?}"
        );
        assert_eq!(
            watched.len(),
            4,
            "registry dependencies have a source and stay out: {watched:?}"
        );
    }

    /// Cargo sets these for the shell binary it launched. Inherited into the build the shell asks
    /// for, they reach every build script cargo runs and land in its fingerprint — `ring` reads
    /// `CARGO_MANIFEST_DIR`, so it rebuilt whenever a launcher run and a bare `cargo build`
    /// alternated, and the scenes dylib came with it.
    #[test]
    fn a_cargo_command_drops_what_cargo_set_for_the_shell_itself() {
        let command = cargo("/work/app", Utf8Path::new("/work/app/gallery.toml"));
        let dropped: Vec<&str> = command
            .get_envs()
            .filter(|(_, value)| value.is_none())
            .filter_map(|(name, _)| name.to_str())
            .collect();

        assert!(
            dropped.contains(&"CARGO_MANIFEST_DIR"),
            "the one that was observed rebuilding: {dropped:?}"
        );
        assert!(
            dropped.iter().any(|name| name.starts_with("CARGO_PKG_")),
            "and the rest describing this package: {dropped:?}"
        );
        assert!(
            !dropped.contains(&"CARGO_HOME") && !dropped.contains(&"CARGO_TARGET_DIR"),
            "settings are the user's, not ours to drop: {dropped:?}"
        );
    }

    #[test]
    fn a_cargo_command_runs_in_the_crate_and_names_only_a_config_of_its_own() {
        let carried = |command: &Command| {
            command
                .get_envs()
                .find(|(key, _)| key.to_str() == Some("GALLERY_CONFIG"))
                .map(|(_, value)| {
                    value
                        .and_then(std::ffi::OsStr::to_str)
                        .unwrap_or("")
                        .to_owned()
                })
        };

        // The one `build.rs` finds by itself. Saying it anyway would put the variable in the build
        // script's fingerprint, and a bare `cargo build` in between would then rebuild the dylib.
        let usual = cargo("/work/app", Utf8Path::new("/work/app/gallery.toml"));
        assert_eq!(carried(&usual), None, "the default goes unsaid");

        let command = cargo("/work/app", Utf8Path::new("/elsewhere/gallery.toml"));

        assert_eq!(command.get_program(), "cargo");
        assert_eq!(
            command.get_current_dir(),
            Some(std::path::Path::new("/work/app")),
            "cargo runs in the scenes crate, not wherever the shell was"
        );
        assert_eq!(
            carried(&command).as_deref(),
            Some("/elsewhere/gallery.toml"),
            "a config from elsewhere is named — the file itself, for `build.rs` to read the globs \
             out of rather than be told them"
        );
    }

    #[test]
    fn list_knobs_asks_for_a_listing_rather_than_a_png() {
        let shot = one_shot(&["gallery", "--scene", "orbit", "--list-knobs"]);
        assert!(shot.list, "the listing is what was asked for");
        assert!(shot.out.is_none(), "and it writes no image");
    }
}
