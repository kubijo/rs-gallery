//! Exercise Cargo's actual artifact layout and build-script freshness, not just fake fingerprints.

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{SystemTime, UNIX_EPOCH},
};

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = env::temp_dir().join(format!(
            "gallery-build-cargo-{name}-{}-{stamp}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        let fixture = Self { root };
        fixture.write(
            "widget/Cargo.toml",
            "[package]\nname = 'review-widget'\nversion = '0.1.0'\nedition = '2024'\n\
             [lib]\ncrate-type = ['cdylib', 'rlib']\n\
             [features]\nscene = []\n\
             [workspace]\n",
        );
        fixture.write(
            "widget/src/lib.rs",
            "#[cfg(feature = \"scene\")] pub fn scene() {}\npub fn ordinary() {}\n",
        );
        let helper = toml::Value::String(env!("CARGO_MANIFEST_DIR").to_owned());
        for (name, dependency, source) in [
            (
                "scenes",
                "{ path = '../widget', features = ['scene'] }",
                "pub fn scene() { review_widget::scene(); }\n",
            ),
            (
                "optional",
                "{ path = '../widget', optional = true }",
                "pub fn ordinary() {}\n",
            ),
        ] {
            fixture.write(
                &format!("{name}/Cargo.toml"),
                &format!(
                    "[package]\nname = 'review-{name}'\nversion = '0.1.0'\nedition = '2024'\n\
                     [dependencies]\nreview-widget = {dependency}\n\
                     [build-dependencies]\ngallery-build = {{ path = {helper} }}\n\
                     [workspace]\n"
                ),
            );
            fixture.write(
                &format!("{name}/build.rs"),
                "fn main() { gallery_build::discover_from_env(); }\n",
            );
            fixture.write(&format!("{name}/gallery.toml"), "scene_globs = []\n");
            fixture.write(&format!("{name}/src/lib.rs"), source);
            // Resolve against the versions already fetched for this test run, even when
            // the local registry also contains newer releases. Cargo prunes unused entries.
            fs::copy(
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../Cargo.lock"),
                fixture.root.join(name).join("Cargo.lock"),
            )
            .unwrap();
        }
        fixture
    }

    fn write(&self, path: &str, contents: &str) {
        let path = self.root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn cargo(&self, package: &str, args: &[&str]) -> Output {
        // Each fixture owns its artifacts, so nested Cargo never waits on the outer
        // test runner's target lock. Keep its toolchain, but isolate gallery inputs and
        // coverage flags from the parent process.
        Command::new(env!("CARGO"))
            .current_dir(&self.root)
            .args(args)
            .args(["--offline", "--color", "never", "-vv", "--manifest-path"])
            .arg(self.root.join(package).join("Cargo.toml"))
            .env("CARGO_TARGET_DIR", self.root.join("target"))
            .env("CARGO_BUILD_BUILD_DIR", self.root.join("target"))
            .env_remove("GALLERY_CONFIG")
            .env_remove("GALLERY_SCENE_GLOBS")
            .env_remove("RUSTFLAGS")
            .env_remove("CARGO_ENCODED_RUSTFLAGS")
            .output()
            .expect("run Cargo fixture")
    }

    fn rlib(&self) -> PathBuf {
        self.root.join("target/debug/deps/libreview_widget.rlib")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn output_text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn success(output: Output) -> String {
    let text = output_text(&output);
    assert!(output.status.success(), "{text}");
    text
}

fn assert_fresh(output: &str, package: &str) {
    assert!(
        output.contains(&format!("Fresh review-{package}")),
        "{output}"
    );
    assert!(
        !output.contains(&format!("Dirty review-{package}")),
        "{output}"
    );
}

#[test]
fn checks_and_disabled_optional_dependencies_stay_fresh_without_an_rlib() {
    let fixture = Fixture::new("missing");
    for (package, mode) in [("scenes", "check"), ("optional", "build")] {
        let first = success(fixture.cargo(package, &[mode]));
        assert!(!fixture.rlib().exists());
        assert!(
            !first.contains(&format!(
                "cargo:rerun-if-changed={}",
                fixture.rlib().display()
            )),
            "{first}"
        );
        assert!(!first.contains("cargo:warning="), "{first}");
        assert_fresh(&success(fixture.cargo(package, &[mode])), package);
    }
}

#[test]
fn a_cold_build_succeeds_without_warning() {
    let fixture = Fixture::new("cold");
    let output = success(fixture.cargo("scenes", &["build"]));
    assert!(fixture.rlib().is_file());
    assert!(!output.contains("cargo:warning="), "{output}");
}

#[test]
fn an_overwrite_warns_but_checks_and_unused_optional_dependencies_do_not() {
    let fixture = Fixture::new("overwrite");
    // Warm the scenes' dependency graph, then rerun discovery with an existing rlib.
    // Building the widget as a workspace root can leave a different fingerprint even
    // with the same features, so use the scenes' own build for the one-library baseline.
    success(fixture.cargo("scenes", &["build"]));
    success(fixture.cargo("widget", &["check", "--features", "scene"]));
    fixture.write(
        "scenes/gallery.toml",
        "scene_globs = []\n# rerun discovery\n",
    );
    let first = success(fixture.cargo("scenes", &["build"]));
    assert!(
        first.contains(&format!(
            "cargo:rerun-if-changed={}",
            fixture.rlib().display()
        )),
        "{first}"
    );
    assert!(!first.contains("cargo:warning="), "{first}");
    assert_fresh(&success(fixture.cargo("scenes", &["build"])), "scenes");

    success(fixture.cargo("widget", &["build"]));
    let overwritten = fixture.cargo("scenes", &["build"]);
    let text = output_text(&overwritten);
    assert!(!overwritten.status.success(), "{text}");
    assert!(text.contains("cannot find function `scene`"), "{text}");
    assert!(text.contains("cargo:warning=`review-widget`"), "{text}");
    assert!(text.contains("library builds that may share"), "{text}");
    assert!(
        text.contains(&fixture.rlib().display().to_string()),
        "{text}"
    );

    let optional = success(fixture.cargo("optional", &["build"]));
    assert!(!optional.contains("cargo:warning="), "{optional}");
    assert!(
        !optional.contains(&format!(
            "cargo:rerun-if-changed={}",
            fixture.rlib().display()
        )),
        "{optional}"
    );
    assert_fresh(&success(fixture.cargo("optional", &["build"])), "optional");
}
