//! Build-time scene discovery for gallery. A scenes `build.rs` calls [`discover_from_env`]; it globs
//! for `*.scene.rs`, writes their `#[path] mod …;` includes to `OUT_DIR` (mod named after the file
//! stem, so `module_path!()` is the tree node), and reruns when the set changes. The crate `include!`s it.
//!
//! [`Config`] is here rather than in the launcher because both read it,
//! and the globs they arrive at have to be the same strings:
//! cargo compares the environment it built with, so two spellings of one glob
//! rebuild the dylib on every alternation.

use std::{collections::HashSet, env, fmt::Write as _, fs};

use camino::{Utf8Path, Utf8PathBuf};

/// A `gallery.toml`, canonicalised and resolved — the reading both the launcher
/// and a scenes `build.rs` go through.
pub struct Config {
    /// The file itself, canonicalised.
    pub path: Utf8PathBuf,
    /// What the window is called.
    pub title: String,
    /// Every `scene_globs` entry as an absolute pattern.
    pub globs: Vec<String>,
    /// Extra hot-reload paths.
    pub hot_watch_paths: Vec<Utf8PathBuf>,
}

/// The file's own shape, before the paths in it mean anything.
#[derive(serde::Deserialize)]
struct Declared {
    scene_globs: Vec<String>,
    #[serde(default)]
    hot_watch_paths: Vec<Utf8PathBuf>,
    #[serde(default = "default_title")]
    title: String,
}

fn default_title() -> String {
    "Gallery".to_owned()
}

impl Config {
    /// # Errors
    /// If the path cannot be canonicalised, read, or parsed — each naming the path.
    pub fn read(path: &Utf8Path) -> Result<Self, String> {
        let path = path
            .canonicalize_utf8()
            .map_err(|e| format!("config `{path}`: {e}"))?;
        let text = fs::read_to_string(&path).map_err(|e| format!("read `{path}`: {e}"))?;
        let declared: Declared =
            toml::from_str(&text).map_err(|e| format!("parse `{path}`: {e}"))?;
        let dir = path.parent().unwrap_or(Utf8Path::new("."));
        let globs = declared
            .scene_globs
            .iter()
            .map(|glob| resolve_glob(dir, glob))
            .collect();
        let mut hot_watch_paths = declared
            .hot_watch_paths
            .iter()
            .map(|watch| {
                let watch = dir.join(watch);
                watch
                    .canonicalize_utf8()
                    .map_err(|e| format!("hot watch path `{watch}`: {e}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        hot_watch_paths.sort();
        hot_watch_paths.dedup();
        Ok(Self {
            path,
            title: declared.title,
            globs,
            hot_watch_paths,
        })
    }
}

/// Resolve a config-relative glob to an absolute one.
/// Canonicalizes the directory prefix — up to the first wildcard — so `..` is gone
/// before it reaches `glob`, which walks components literally.
fn resolve_glob(config_dir: &Utf8Path, glob: &str) -> String {
    let wildcard = glob.find(['*', '?', '[']).unwrap_or(glob.len());
    let split = glob[..wildcard].rfind('/').map_or(0, |slash| slash + 1);
    let (dir, pattern) = glob.split_at(split);
    let base = config_dir.join(dir);
    let base = base.canonicalize_utf8().unwrap_or(base);
    base.join(pattern).into_string()
}

/// The whole of a scenes crate's `build.rs`: `gallery_build::discover_from_env()`.
///
/// The globs come from `gallery.toml` — the one `GALLERY_CONFIG` names when the launcher was pointed
/// elsewhere, otherwise the one beside `CARGO_MANIFEST_DIR` — so a bare `cargo build` finds the scenes
/// a launcher run finds. A config missing or unparseable finds nothing rather than failing the build:
/// a crate part-way through `cargo generate` has placeholders where its globs will be.
pub fn discover_from_env() {
    println!("cargo:rerun-if-env-changed=GALLERY_CONFIG");
    println!("cargo:rerun-if-env-changed=GALLERY_SCENE_GLOBS");
    discover(env_globs());
}

fn env_globs() -> Vec<String> {
    // `GALLERY_SCENE_GLOBS` still wins where something sets it: what a pinned consumer passes
    // by hand, and what a launcher from before `GALLERY_CONFIG` passes itself.
    match env::var("GALLERY_SCENE_GLOBS") {
        Ok(raw) if !raw.trim().is_empty() => raw
            .lines()
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect(),
        _ => {
            let path = config_path();
            println!("cargo:rerun-if-changed={path}");
            Config::read(&path)
                .map(|config| config.globs)
                .unwrap_or_default()
        }
    }
}

fn config_path() -> Utf8PathBuf {
    match env::var("GALLERY_CONFIG") {
        Ok(named) => Utf8PathBuf::from(named),
        Err(_) => {
            let dir = env::var("CARGO_MANIFEST_DIR")
                .expect("discover_from_env() must run from a build script");
            Utf8PathBuf::from(dir).join("gallery.toml")
        }
    }
}

/// Discover scene files matching `globs`, write the module-include file to `OUT_DIR`, and return its
/// path (to `include!`). Registers each matched file and each glob's base dir with `rerun-if-changed`.
///
/// # Panics
/// If `OUT_DIR` is unset, a glob is malformed, or the generated file can't be written.
pub fn discover<I, S>(globs: I) -> Utf8PathBuf
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let out_dir = env::var("OUT_DIR").expect("discover() must run from a build script");
    let out_dir = Utf8PathBuf::from(out_dir);
    let generated = out_dir.join("gallery_scenes.rs");
    let modules = includes(globs, &out_dir);
    // Left alone when it would say the same thing: the crate `include!`s this file,
    // so a fresh mtime recompiles it even where not a byte moved.
    if fs::read_to_string(&generated).is_ok_and(|current| current == modules) {
        return generated;
    }
    fs::write(&generated, modules).expect("write generated scene includes");
    generated
}

/// The `#[path] mod …;` line for every scene file the globs match,
/// and the `rerun-if-changed` directives that go with them.
fn includes<I, S>(globs: I, out: &Utf8Path) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let found = scan(globs, Some(out));
    for path in &found.watch {
        println!("cargo:rerun-if-changed={path}");
    }
    found.modules
}

/// What a discovery came to: the module lines to write, and what cargo should watch for the set
/// to be discovered again.
struct Discovered {
    modules: String,
    watch: Vec<Utf8PathBuf>,
}

/// Every scene file the globs match, and the directories holding them.
///
/// Cargo takes a watched directory's newest descendant,
/// so the build dirties any directory it writes into.
/// A glob's static prefix is a repo root where scenes sit beside their components,
/// `target/` and `.git` under it, so a `git status` between two builds rebuilt the dylib.
/// A scene's own directory is not that — unless `out` sits beneath it, as the scaffold's does.
/// Skipping that one costs notice of a scene *added* there; the files are watched either way.
fn scan<I, S>(globs: I, out: Option<&Utf8Path>) -> Discovered
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut modules = String::new();
    let mut used = HashSet::new();
    let mut watch = Vec::new();
    let mut watched = HashSet::new();
    for pattern in globs {
        let pattern = pattern.as_ref();
        let matches =
            glob::glob(pattern).unwrap_or_else(|e| panic!("invalid scene glob `{pattern}`: {e}"));
        for entry in matches {
            // A walk races with whatever else is writing under the tree: cargo creates
            // and removes temp files in `target/` as it builds, so an entry can go missing
            // between being listed and being read.
            // Skipping loses nothing — what cannot be read cannot be a scene.
            let Ok(path) = entry else { continue };
            let path = Utf8PathBuf::from_path_buf(path)
                .unwrap_or_else(|p| panic!("scene path is not UTF-8: {}", p.display()));
            if build_output(&path) {
                continue;
            }
            let abs = path.canonicalize_utf8().unwrap_or(path);
            if let Some(dir) = abs.parent()
                && !out.is_some_and(|out| out.starts_with(dir))
                && watched.insert(dir.to_owned())
            {
                watch.push(dir.to_owned());
            }
            watch.push(abs.clone());
            let module = unique_module(&abs, &mut used);
            writeln!(modules, "#[path = {:?}]\nmod {module};", abs.as_str())
                .expect("write to String");
        }
    }
    Discovered { modules, watch }
}

/// Whether a path came out of a build directory rather than a source tree.
///
/// A `**` glob from a crate root walks straight into `target/`, where a scene file is cargo's own
/// copy — compiling it in would declare the same scenes twice.
/// The launcher's watcher asks the same of every file event,
/// a directory the build writes into being one it would rebuild for forever.
///
/// What marks it is the `CACHEDIR.TAG` cargo writes there, not the name:
/// a source directory called `target` keeps its scenes, and any other cache
/// that tags itself is skipped too.
///
/// The walk still descends either way, which only costs time
/// — `glob` takes no directory to prune.
#[must_use]
pub fn build_output(path: &Utf8Path) -> bool {
    path.ancestors()
        .any(|dir| dir.join("CACHEDIR.TAG").is_file())
}

/// A unique module name derived from a scene file's stem (`greeting.scene.rs` → `greeting`), suffixed
/// on collision so two files with the same stem don't clash.
fn unique_module(path: &Utf8Path, used: &mut HashSet<String>) -> String {
    let file = path.file_name().unwrap_or("scene");
    let stem = file.strip_suffix(".scene.rs").unwrap_or(file);
    let base = sanitize(stem);
    let mut name = base.clone();
    let mut n = 1;
    while !used.insert(name.clone()) {
        name = format!("{base}_{n}");
        n += 1;
    }
    name
}

/// Turn a file stem into a valid module identifier: non-alphanumerics become `_`, and a leading digit
/// is prefixed so the result is a legal identifier.
fn sanitize(stem: &str) -> String {
    let mut out: String = stem
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if out.chars().next().is_none_or(|c| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch tree of `files`, each written with enough to be a plausible scene.
    fn tree(name: &str, files: &[&str]) -> Utf8PathBuf {
        let root = Utf8PathBuf::from_path_buf(env::temp_dir())
            .expect("a UTF-8 temp dir")
            .join(format!("gallery-build-{name}"));
        let _ = fs::remove_dir_all(&root);
        for file in files {
            let path = root.join(file);
            fs::create_dir_all(path.parent().expect("a parent")).expect("scratch dir");
            fs::write(&path, "// scene").expect("write scene");
        }
        root
    }

    #[test]
    fn a_scene_file_under_a_tagged_cache_is_build_output_and_stays_out_of_the_includes() {
        let root = tree(
            "cache",
            &[
                "good.scene.rs",
                "target/CACHEDIR.TAG",
                "target/debug/stale.scene.rs",
            ],
        );
        let modules = scan([format!("{root}/**/*.scene.rs")], None).modules;
        assert!(modules.contains("mod good;"), "the source scene: {modules}");
        assert!(
            !modules.contains("stale"),
            "nothing out of the build dir: {modules}"
        );
    }

    #[test]
    fn a_source_directory_named_target_keeps_its_scenes() {
        // Cargo tags its own build directory, so the name alone decides nothing
        // — a crate laid out under `targets/` or `target/` is source like any other.
        let root = tree(
            "named",
            &["crates/targets/dial.scene.rs", "target/mine.scene.rs"],
        );
        let modules = scan([format!("{root}/**/*.scene.rs")], None).modules;
        assert!(modules.contains("mod dial;"), "under `targets/`: {modules}");
        assert!(modules.contains("mod mine;"), "under `target/`: {modules}");
    }

    #[test]
    fn resolve_glob_joins_the_config_dir_and_keeps_the_wildcard_tail() {
        let resolved = resolve_glob(Utf8Path::new("cfg"), "a/b/*.scene.rs");
        assert!(resolved.contains("a/b"));
        assert!(resolved.ends_with("*.scene.rs"));
    }

    /// Cargo takes a watched directory's newest descendant, so watching a glob's static prefix
    /// watches the crate's own `target/` wherever scenes live outside the crate — and every build
    /// dirties the next. `.git` under the same prefix did it on a `git status`.
    #[test]
    fn the_watched_directories_are_the_scenes_own_not_the_tree_they_were_found_in() {
        let root = tree(
            "watched",
            &[
                "components/dial.scene.rs",
                "target/CACHEDIR.TAG",
                "target/debug/stale.scene.rs",
                ".git/HEAD",
            ],
        );
        let found = scan([format!("{root}/**/*.scene.rs")], None);

        assert!(
            found.watch.contains(&root.join("components")),
            "the directory a scene is in, so one added beside it is noticed: {:?}",
            found.watch
        );
        assert!(
            found.watch.contains(&root.join("components/dial.scene.rs")),
            "and the scene itself, so an edit to it is: {:?}",
            found.watch
        );
        for unwatched in [root.clone(), root.join("target"), root.join(".git")] {
            assert!(
                !found.watch.contains(&unwatched),
                "`{unwatched}` holds build output or churns on its own: {:?}",
                found.watch
            );
        }
    }

    /// The scaffold's own layout: scenes in the crate root, `target/` beside them.
    /// The directory holding a scene is then the one the build writes into,
    /// so it goes unwatched and only the scene files are.
    #[test]
    fn a_scene_directory_the_build_writes_into_is_not_watched() {
        let root = tree("writes-into", &["loose.scene.rs", "target/CACHEDIR.TAG"]);
        let out = root.join("target/debug/build/app-gallery-1234/out");

        let found = scan([format!("{root}/**/*.scene.rs")], Some(&out));

        assert!(
            !found.watch.contains(&root),
            "the crate root holds `out`, so watching it would dirty every next build: {:?}",
            found.watch
        );
        assert!(
            found.watch.contains(&root.join("loose.scene.rs")),
            "the scene is still watched, so editing it still rebuilds: {:?}",
            found.watch
        );
    }

    /// A bare `cargo build` has no environment from the launcher, and has to find the same scenes
    /// anyway — the config is the one place the globs are written down.
    #[test]
    fn a_config_finds_the_scenes_its_globs_name() {
        let root = tree(
            "config",
            &[
                "crate/gallery.toml",
                "scenes/dial.scene.rs",
                "loose.scene.rs",
            ],
        );
        // Relative and reaching outside the crate, as a scenes crate's own config does.
        fs::write(
            root.join("crate/gallery.toml"),
            "scene_globs = [\"../**/*.scene.rs\"]\n",
        )
        .expect("write the config");

        let config = Config::read(&root.join("crate/gallery.toml")).expect("a readable config");
        assert_eq!(
            config.title, "Gallery",
            "an omitted title has a useful default"
        );
        let modules = scan(&config.globs, None).modules;
        assert!(modules.contains("mod dial;"), "under a subdir: {modules}");
        assert!(modules.contains("mod loose;"), "beside it: {modules}");
        assert!(
            config.globs.iter().all(|glob| !glob.contains("..")),
            "resolved absolute, so `glob` never walks `..` literally: {:?}",
            config.globs
        );
        assert!(
            config.hot_watch_paths.is_empty(),
            "the escape hatch is optional"
        );
    }

    #[test]
    fn configured_hot_watch_paths_are_config_relative_canonical_and_deduped() {
        let root = tree(
            "hot-watch-paths",
            &["crate/scene.rs", "shared/assets/palette.json"],
        );
        fs::write(
            root.join("crate/gallery.toml"),
            "scene_globs = [\"scene.rs\"]\n\
             hot_watch_paths = [\"../shared/assets\", \"../shared/./assets\", \
             \"../shared/assets/palette.json\"]\n",
        )
        .expect("write the config");

        let config = Config::read(&root.join("crate/gallery.toml")).expect("a readable config");

        assert_eq!(
            config.hot_watch_paths,
            [
                root.join("shared/assets"),
                root.join("shared/assets/palette.json")
            ],
            "both directories and files are retained, with duplicate spellings collapsed"
        );
    }

    /// A config that isn't there yet finds nothing rather than failing the build,
    /// which is a crate part-way through `cargo generate`.
    #[test]
    fn a_config_that_cannot_be_read_is_not_fatal() {
        let root = tree("no-config", &["placeholder"]);
        assert!(Config::read(&root.join("gallery.toml")).is_err());
    }

    /// The generated file is `include!`d, so rewriting it with identical bytes recompiles the crate
    /// — and `discover` runs on every build, launcher or not.
    #[test]
    fn an_unchanged_discovery_leaves_the_generated_file_alone() {
        let root = tree("rewrite", &["one.scene.rs"]);
        let out = root.join("out");
        fs::create_dir_all(&out).expect("an OUT_DIR");
        // SAFETY: single-threaded test; `discover` reads `OUT_DIR` and nothing else here does.
        unsafe { env::set_var("OUT_DIR", out.as_str()) };

        let globs = [format!("{root}/**/*.scene.rs")];
        let generated = discover(&globs);
        let first = fs::metadata(&generated)
            .expect("the file")
            .modified()
            .expect("a mtime");

        // Coarse filesystem timestamps would hide a rewrite that lands in the same tick.
        std::thread::sleep(std::time::Duration::from_millis(20));
        discover(&globs);

        let second = fs::metadata(&generated)
            .expect("the file")
            .modified()
            .expect("a mtime");
        assert_eq!(first, second, "nothing changed, so nothing was written");
    }

    /// The reported build failure: cargo writes and removes temp files
    /// under `target/` as it builds, so the walk hits entries it cannot read.
    ///
    /// An unreadable directory stands in for that race,
    /// which is otherwise a matter of timing.
    #[cfg(unix)]
    #[test]
    fn a_directory_that_cannot_be_read_is_skipped_rather_than_fatal() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = tree("unreadable", &["good.scene.rs", "locked/hidden.scene.rs"]);
        let locked = root.join("locked");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).expect("lock the dir");

        // Caught rather than left to unwind: not panicking is the whole property, and the directory
        // has to be readable again either way or every later run trips over it instead.
        let walked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            scan([format!("{root}/**/*.scene.rs")], None).modules
        }));
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).expect("unlock the dir");

        let modules = walked.expect("an entry that cannot be read is skipped, not fatal");
        assert!(
            modules.contains("mod good;"),
            "the readable scene still arrives: {modules}"
        );
    }
}
