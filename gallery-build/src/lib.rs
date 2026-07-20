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

    let mut modules = String::new();
    let mut used = HashSet::new();
    for pattern in globs {
        let pattern = pattern.as_ref();
        println!("cargo:rerun-if-changed={}", glob_base(pattern));

        let matches =
            glob::glob(pattern).unwrap_or_else(|e| panic!("invalid scene glob `{pattern}`: {e}"));
        for entry in matches {
            let path = entry.unwrap_or_else(|e| panic!("reading scene glob `{pattern}`: {e}"));
            let path = Utf8PathBuf::from_path_buf(path)
                .unwrap_or_else(|p| panic!("scene path is not UTF-8: {}", p.display()));
            let abs = path.canonicalize_utf8().unwrap_or(path);
            println!("cargo:rerun-if-changed={abs}");
            let module = unique_module(&abs, &mut used);
            writeln!(modules, "#[path = {:?}]\nmod {module};", abs.as_str())
                .expect("write to String");
        }
    }

    fs::write(&generated, modules).expect("write generated scene includes");
    generated
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
