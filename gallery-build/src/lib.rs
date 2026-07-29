//! Build-time scene discovery for gallery. A scenes `build.rs` calls [`discover_from_env`]; it globs
//! for `*.scene.rs`, writes their `#[path] mod …;` includes to `OUT_DIR` (mod named after the file
//! stem, so `module_path!()` is the tree node), and reruns when the set changes. The crate `include!`s it.

use std::{collections::HashSet, env, fmt::Write as _, fs};

use camino::{Utf8Path, Utf8PathBuf};

/// The whole of a scenes crate's `build.rs`: `gallery_build::discover_from_env()`. Discovers the globs
/// in `GALLERY_SCENE_GLOBS` (newline-separated, set by the launcher); with none set, nothing is found.
pub fn discover_from_env() {
    println!("cargo:rerun-if-env-changed=GALLERY_SCENE_GLOBS");
    let raw = env::var("GALLERY_SCENE_GLOBS").unwrap_or_default();
    let globs: Vec<&str> = raw.lines().filter(|line| !line.is_empty()).collect();
    discover(globs);
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
    let generated = Utf8PathBuf::from(out_dir).join("gallery_scenes.rs");
    fs::write(&generated, includes(globs)).expect("write generated scene includes");
    generated
}

/// The `#[path] mod …;` line for every scene file the globs match,
/// and the `rerun-if-changed` directives that go with them.
fn includes<I, S>(globs: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut modules = String::new();
    let mut used = HashSet::new();
    for pattern in globs {
        let pattern = pattern.as_ref();
        println!("cargo:rerun-if-changed={}", glob_base(pattern));

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
            println!("cargo:rerun-if-changed={abs}");
            let module = unique_module(&abs, &mut used);
            writeln!(modules, "#[path = {:?}]\nmod {module};", abs.as_str())
                .expect("write to String");
        }
    }
    modules
}

/// Whether a match came out of a build directory rather than a source tree.
///
/// A `**` glob from a crate root walks straight into `target/`, where a scene file is cargo's own
/// copy — compiling it in would declare the same scenes twice.
///
/// What marks it is the `CACHEDIR.TAG` cargo writes there, not the name:
/// a source directory called `target` keeps its scenes, and any other cache
/// that tags itself is skipped too.
///
/// The walk still descends either way, which only costs time
/// — `glob` takes no directory to prune.
fn build_output(path: &Utf8Path) -> bool {
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

/// The static directory prefix of a glob (before the first wildcard), so cargo can watch it for
/// added/removed files. Falls back to `.` when the pattern opens with a wildcard.
fn glob_base(pattern: &str) -> &str {
    let wildcard = pattern.find(['*', '?', '[']).unwrap_or(pattern.len());
    match pattern[..wildcard].rfind('/') {
        Some(slash) => &pattern[..slash],
        None => ".",
    }
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
        let modules = includes([format!("{root}/**/*.scene.rs")]);
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
        let modules = includes([format!("{root}/**/*.scene.rs")]);
        assert!(modules.contains("mod dial;"), "under `targets/`: {modules}");
        assert!(modules.contains("mod mine;"), "under `target/`: {modules}");
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
            includes([format!("{root}/**/*.scene.rs")])
        }));
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).expect("unlock the dir");

        let modules = walked.expect("an entry that cannot be read is skipped, not fatal");
        assert!(
            modules.contains("mod good;"),
            "the readable scene still arrives: {modules}"
        );
    }
}
