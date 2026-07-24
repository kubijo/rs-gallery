//! The config-driven entry point: read `gallery.toml`, build the scenes dylib from its globs, load
//! it, and open the window — plus the cargo plumbing that does the building and the watching.

use std::{
    fs,
    process::Command,
    sync::{Arc, Mutex},
};

use camino::{Utf8Path, Utf8PathBuf};
use process_wrap::std::{ChildWrapper, CommandWrap};

use crate::{HotDylib, RunOptions, Settings, run_with, update::check_updates};

/// The consumer's entire `main`. Both arguments are required — a `setup` closure and [`Settings`], which
/// names the [`Renderer`](crate::Renderer):
///
/// ```ignore
/// fn main() -> gallery::eframe::Result {
///     gallery::launch!(|_| {}, gallery::Settings::new(gallery::Renderer::Wgpu))
/// }
/// ```
///
/// Expands to [`launch()`] with the calling crate's name and manifest dir filled in. `setup` runs
/// against the fresh egui context (e.g. `|ctx| egui_extras::install_image_loaders(ctx)`).
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

/// Read the config, build the scenes dylib from its globs, load it, and open the window. Prefer the
/// [`launch!`] macro, which fills `package`/`manifest_dir` from the calling crate.
///
/// Args: `--config <path>` (default `<manifest_dir>/gallery.toml`); `--hot` (rebuild + swap on edits);
/// `--frames <n>` with optional `--scene <key>` for a deterministic profiling run that renders exactly
/// `n` frames and exits.
///
/// # Panics
/// If an argument is unknown or missing its value, or the config can't be read or parsed.
pub fn launch(
    package: &str,
    manifest_dir: &str,
    settings: Settings,
    setup: impl FnOnce(&egui::Context) + 'static,
) -> eframe::Result {
    if std::env::args().skip(1).any(|arg| arg == "--check-updates") {
        check_updates();
        return Ok(());
    }
    let args = launch_args(manifest_dir);
    let config = read_config(&args.config);
    let base = args.config.parent().unwrap_or_else(|| Utf8Path::new("."));
    let globs: Vec<String> = config
        .scene_globs
        .iter()
        .map(|glob| resolve_glob(base, glob))
        .collect();

    build_lib(manifest_dir, &globs);
    let watcher = if args.hot {
        spawn_watcher(manifest_dir, &globs)
    } else {
        None
    };
    // The dylib is `lib<crate>.so`; the crate's lib name is the package name with dashes as underscores.
    let source = HotDylib::new(&package.replace('-', "_"), args.hot)
        .expect("load the freshly built scenes dylib");
    let result = run_with(&config.title, source, settings, setup, args.options);
    // Window closed normally: stop the watcher (the Ctrl-C/SIGTERM path is handled in spawn_watcher).
    if let Some(watcher) = &watcher {
        let _ = watcher.lock().unwrap().kill();
    }
    result
}

#[derive(serde::Deserialize)]
struct Config {
    scene_globs: Vec<String>,
    #[serde(default = "default_title")]
    title: String,
}

fn default_title() -> String {
    "gallery".to_owned()
}

struct LaunchArgs {
    config: Utf8PathBuf,
    hot: bool,
    options: RunOptions,
}

fn launch_args(manifest_dir: &str) -> LaunchArgs {
    let mut config = None;
    let mut hot = false;
    let mut options = RunOptions::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--hot" => hot = true,
            "--config" => config = Some(args.next().expect("--config needs a path")),
            "--frames" => {
                let count = args.next().expect("--frames needs a count");
                options.frames = Some(count.parse().expect("--frames needs a number"));
            }
            "--scene" => options.scene = Some(args.next().expect("--scene needs a scene key")),
            other => panic!("unknown argument: {other}"),
        }
    }
    let path = config
        .map(Utf8PathBuf::from)
        .unwrap_or_else(|| Utf8Path::new(manifest_dir).join("gallery.toml"));
    let config = path
        .canonicalize_utf8()
        .unwrap_or_else(|e| panic!("config `{path}`: {e}"));
    LaunchArgs {
        config,
        hot,
        options,
    }
}

fn read_config(path: &Utf8Path) -> Config {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("read `{path}`: {e}"));
    toml::from_str(&text).unwrap_or_else(|e| panic!("parse `{path}`: {e}"))
}

/// Resolve a config-relative glob to an absolute one. Canonicalizes the directory prefix (up to the
/// first wildcard) so `..` is gone before it reaches `glob`, which walks components literally.
fn resolve_glob(config_dir: &Utf8Path, glob: &str) -> String {
    let wildcard = glob.find(['*', '?', '[']).unwrap_or(glob.len());
    let split = glob[..wildcard].rfind('/').map_or(0, |slash| slash + 1);
    let (dir, pattern) = glob.split_at(split);
    let base = config_dir.join(dir);
    let base = base.canonicalize_utf8().unwrap_or(base);
    base.join(pattern).into_string()
}

/// Build the scenes dylib once, blocking, so the loader finds a `.so` on first launch.
fn build_lib(manifest_dir: &str, globs: &[String]) {
    let mut command = cargo(manifest_dir, globs);
    command.args(["build", "--lib"]);
    if let Some(profile) = host_profile() {
        command.args(["--profile", &profile]);
    }
    let built = command.status().is_ok_and(|status| status.success());
    assert!(built, "`cargo build --lib` for the scenes dylib failed");
}

/// The cargo profile this binary was built under, read off its own path: cargo drops the host binary
/// in `<target>/<profile-dir>/`, and every profile's directory is its own name — `dev` alone differs,
/// building into `debug`.
///
/// The scenes dylib has to match, because [`HotDylib`] loads it from the executable's directory. Built
/// under any other profile it lands somewhere nothing reads, and the cold compile that produced it is
/// pure cost.
fn host_profile() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.file_name()?.to_str()?;
    Some(if dir == "debug" { "dev" } else { dir }.to_owned())
}

/// A running hot-reload watcher, shared so both the window-close path
/// and the signal handler can kill it.
type Watcher = Arc<Mutex<Box<dyn ChildWrapper>>>;

/// Rebuild the scenes dylib on every scene change; each fresh `.so` is what [`HotDylib`] reloads.
/// The watcher runs as a process group (unix) / job object (windows), so killing it takes down
/// its whole tree — on window close (via the returned handle) and on Ctrl-C/SIGTERM (via the handler).
fn spawn_watcher(manifest_dir: &str, globs: &[String]) -> Option<Watcher> {
    let mut command = cargo(manifest_dir, globs);
    command.arg("watch");
    for dir in watch_dirs(manifest_dir, globs) {
        command.args(["-w", &dir]);
    }
    // Same profile as `build_lib`, or the rebuilt dylib lands where the reloader never looks.
    let rebuild = match host_profile() {
        Some(profile) => format!("build --lib --profile {profile}"),
        None => "build --lib".to_owned(),
    };
    command.args(["-x", &rebuild]);

    let mut wrapped = CommandWrap::from(command);
    #[cfg(unix)]
    wrapped.wrap(process_wrap::std::ProcessGroup::leader());
    #[cfg(windows)]
    wrapped.wrap(process_wrap::std::JobObject);

    let child = match wrapped.spawn() {
        Ok(child) => Arc::new(Mutex::new(child)),
        Err(e) => {
            eprintln!("gallery: `cargo watch` did not start — edits will not rebuild: {e}");
            return None;
        }
    };

    let on_signal = Arc::clone(&child);
    if let Err(e) = ctrlc::set_handler(move || {
        let _ = on_signal.lock().unwrap().kill();
        std::process::exit(130);
    }) {
        eprintln!("gallery: no signal handler — the watcher may outlive the window: {e}");
    }

    Some(child)
}

/// A cargo command in the crate dir, carrying the resolved globs to the scenes `build.rs`.
fn cargo(manifest_dir: &str, globs: &[String]) -> Command {
    let mut command = Command::new("cargo");
    command
        .current_dir(manifest_dir)
        .env("GALLERY_SCENE_GLOBS", globs.join("\n"));
    command
}

/// Dirs for cargo-watch to monitor: the crate plus each glob's base dir — scene files usually live
/// outside the crate, so cargo-watch won't see edits to them without an explicit `-w`.
fn watch_dirs(manifest_dir: &str, globs: &[String]) -> Vec<String> {
    let mut dirs = vec![manifest_dir.to_owned()];
    for glob in globs {
        let end = glob.find(['*', '?', '[']).unwrap_or(glob.len());
        if let Some(slash) = glob[..end].rfind('/') {
            dirs.push(glob[..slash].to_owned());
        }
    }
    dirs.sort();
    dirs.dedup();
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_glob_joins_the_config_dir_and_keeps_the_wildcard_tail() {
        let resolved = resolve_glob(Utf8Path::new("cfg"), "a/b/*.scene.rs");
        assert!(resolved.contains("a/b"));
        assert!(resolved.ends_with("*.scene.rs"));
    }
}
