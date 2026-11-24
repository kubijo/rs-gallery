//! Snapshot checks, host-side: where references live, how an image or a text
//! is compared with one, and what a check leaves behind.

use std::{
    collections::HashSet,
    sync::atomic::{AtomicU32, Ordering},
};

use camino::{Utf8Path, Utf8PathBuf};
use image::RgbaImage;
use similar::{ChangeTag, TextDiff};

use crate::{
    Renderer,
    play::{
        log::{Acceptance, Attachment, AttachmentKind, Kept},
        snapshot::{Ask, Look, Of, Unmatched, Verdict},
    },
};

use super::{QUIET_FRAMES, dashed, landed, write_png};

/// How a test run judges its snapshot checks.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct Snapshots {
    /// From 0 to 1, as `gallery.toml` has it.
    pub(crate) threshold: f32,
    pub(crate) tolerance: u32,
    pub(crate) update: Update,
    /// Where references go rather than beside each play's file.
    pub(crate) dir: Option<Utf8PathBuf>,
}

impl Default for Snapshots {
    fn default() -> Self {
        Self {
            threshold: DEFAULT_THRESHOLD,
            tolerance: 0,
            update: Update::Off,
            dir: None,
        }
    }
}

/// dify's own default.
const DEFAULT_THRESHOLD: f32 = 0.1;

impl Snapshots {
    /// The budget `gallery.toml` sets for `renderer`, its own table over the catalog's.
    pub(crate) fn new(
        config: &gallery_build::Snapshots,
        renderer: Renderer,
        update: Update,
    ) -> Self {
        let own = match renderer {
            Renderer::Wgpu => config.wgpu,
            Renderer::Glow => config.glow,
        };
        let catalog = config.budget();
        Self {
            threshold: own
                .threshold
                .or(catalog.threshold)
                .unwrap_or(DEFAULT_THRESHOLD),
            tolerance: own.tolerance.or(catalog.tolerance).unwrap_or(0),
            update,
            ..Self::default()
        }
    }
}

/// Whether a run takes what it sees as the reference, as `UPDATE_SNAPSHOTS` says:
/// the values egui_kittest takes, so one variable means one thing to both.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Update {
    Off,
    /// A check that would fail takes its frame instead.
    Failing,
    /// Every check that sees any difference takes its frame, within budget or not.
    All,
}

impl Update {
    pub(crate) fn from_env() -> Result<Self, String> {
        match std::env::var("UPDATE_SNAPSHOTS") {
            Ok(value) => Self::parse(Some(&value)),
            Err(std::env::VarError::NotPresent) => Self::parse(None),
            Err(std::env::VarError::NotUnicode(_)) => {
                Err("UPDATE_SNAPSHOTS is not UTF-8".to_owned())
            }
        }
    }

    pub(crate) fn parse(value: Option<&str>) -> Result<Self, String> {
        match value {
            None | Some("" | "0" | "false" | "no" | "off") => Ok(Self::Off),
            Some("1" | "true" | "yes" | "on") => Ok(Self::Failing),
            Some("force") => Ok(Self::All),
            Some(other) => Err(format!(
                "UPDATE_SNAPSHOTS is `{other}`; it takes 1 to take failing snapshots \
                 as references, force to take every one that differs, or 0"
            )),
        }
    }
}

/// A play's snapshot checks: the one under way,
/// and every reference used so far.
pub(super) struct Looks<'a> {
    snapshots: &'a Snapshots,
    renderer: &'static str,
    current: Option<Looking>,
    checked: HashSet<Utf8PathBuf>,
    /// Frames in a row that asked for no repaint since the check began.
    quiet: u32,
    /// What the last look measured, past the canvas.
    measured: Option<egui::Vec2>,
}

/// References this process has taken.
static TAKEN: AtomicU32 = AtomicU32::new(0);

/// The check under way: its files, its reference, and what it last saw.
struct Looking {
    reference_path: Utf8PathBuf,
    reference: Reference,
    seen: Option<Seen>,
    diff: Option<Seen>,
    verdict: Verdict,
    settled: bool,
}

/// What a check compares.
#[derive(PartialEq)]
pub(super) enum Seen {
    Image(RgbaImage),
    Text(String),
}

impl Seen {
    fn write(&self, path: &Utf8Path) -> Result<(), String> {
        match self {
            Self::Image(image) => write_png(image, path),
            Self::Text(text) => {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir).map_err(|e| format!("create `{dir}`: {e}"))?;
                }
                std::fs::write(path, text).map_err(|e| format!("write `{path}`: {e}"))
            }
        }
    }
}

enum Reference {
    Missing,
    Unreadable { why: String, bytes: Vec<u8> },
    Read(Seen),
}

impl<'a> Looks<'a> {
    pub(super) fn new(snapshots: &'a Snapshots, renderer: &'static str) -> Self {
        Self {
            snapshots,
            renderer,
            current: None,
            checked: HashSet::new(),
            quiet: 0,
            measured: None,
        }
    }

    /// Whether two looks in a row measured `drawn`, past the canvas, so that it may grow:
    /// an area's first frame can lie where its next will not.
    pub(super) fn measured_twice(&mut self, drawn: egui::Vec2) -> bool {
        let twice = self.measured == Some(drawn);
        self.measured = (!twice).then_some(drawn);
        twice
    }

    /// A frame was drawn; `moving` when it asked for another.
    pub(super) fn drew(&mut self, moving: bool) {
        self.quiet = if moving { 0 } else { self.quiet + 1 };
    }

    /// Start the check `ask` is the first look of.
    ///
    /// # Errors
    /// When it can't be asked as it was, its name names no file,
    /// or its reference is already another check's.
    pub(super) fn begin(&mut self, ask: &Ask<'_>) -> Result<(), String> {
        self.current = None;
        self.quiet = 0;
        self.measured = None;
        if let Some(why) = ask.misused {
            return Err(why.to_owned());
        }
        let path = reference_path(ask, self.renderer, self.snapshots.dir.as_deref())?;
        if !self.checked.insert(path.clone()) {
            return Err(format!(
                "`{}` is already checked in this play; give each snapshot a name of its own",
                path.file_name().unwrap_or_default()
            ));
        }
        self.current = Some(Looking {
            reference: match ask.of {
                Of::Screenshot { .. } => read_image(&path),
                Of::Tree { .. } => read_tree(&path),
            },
            reference_path: path,
            seen: None,
            diff: None,
            verdict: Verdict::Outgrown,
            settled: false,
        });
        Ok(())
    }

    pub(super) fn outgrown(&mut self) -> Look {
        self.unseen(Verdict::Outgrown)
    }

    /// There is no one widget to compare the tree of.
    pub(super) fn unmatched(&mut self, why: Unmatched) -> Look {
        self.unseen(Verdict::Unmatched(why))
    }

    fn unseen(&mut self, verdict: Verdict) -> Look {
        if let Some(looking) = &mut self.current {
            looking.seen = None;
        }
        Look {
            verdict,
            done: false,
        }
    }

    /// Compare `seen`, as things are now,
    /// with the reference of the check under way.
    pub(super) fn judge(&mut self, ask: &Ask<'_>, seen: Seen) -> Result<Look, String> {
        let looking = self
            .current
            .as_mut()
            .ok_or("BUG: a snapshot judged before its check began")?;
        let (threshold, allowed) = match ask.of {
            Of::Screenshot {
                tolerance,
                threshold,
            } => (
                threshold.unwrap_or(self.snapshots.threshold),
                tolerance.unwrap_or(self.snapshots.tolerance),
            ),
            Of::Tree { .. } => (0.0, 0),
        };
        self.measured = None;
        looking.settled = self.quiet >= QUIET_FRAMES && looking.seen.as_ref() == Some(&seen);
        (looking.verdict, looking.diff) = compare(&looking.reference, &seen, threshold, allowed);
        looking.seen = Some(seen);
        // A frame merely within budget may be mid-animation, which is no frame to retake.
        let passes_now = match self.snapshots.update {
            Update::All => looking.verdict == Verdict::Identical,
            Update::Off | Update::Failing => looking.verdict.passes(),
        };
        Ok(Look {
            verdict: looking.verdict.clone(),
            done: passes_now || looking.settled,
        })
    }

    /// The last look was the check's verdict: take it as the reference, or leave it beside one.
    /// Only a settled one is taken.
    pub(super) fn keep(&mut self) -> Result<Kept, String> {
        let looking = self
            .current
            .take()
            .ok_or("BUG: a snapshot kept before it was compared")?;
        let passed = looking.verdict.passes();
        let take = looking.settled
            && match self.snapshots.update {
                Update::Off => false,
                Update::Failing => !passed,
                Update::All => looking.verdict != Verdict::Identical,
            };
        let path = &looking.reference_path;
        let (new, diff) = match path.extension() {
            Some("json") => ("new.json", "diff.txt"),
            _ => ("new.png", "diff.png"),
        };
        let (new, diff) = (path.with_extension(new), path.with_extension(diff));
        // Leftovers at worst: a read-only checkout must not fail a check that passes.
        for stale in [&new, &diff] {
            _ = std::fs::remove_file(stale);
        }
        let mut kept = Kept::default();
        if !take && !matches!(looking.reference, Reference::Missing) {
            kept.attachments
                .push(attach(AttachmentKind::Reference, path));
        }
        if let Some(seen) = looking.seen.filter(|_| take || !passed) {
            if take {
                // Moved into place whole, so a run cut short leaves the old reference as it was,
                // from a name of its own, as another run may be taking the same one.
                let whole = path.with_extension(format!(
                    "{}-{}.new.{}",
                    std::process::id(),
                    TAKEN.fetch_add(1, Ordering::Relaxed),
                    path.extension().unwrap_or_default(),
                ));
                seen.write(&whole)
                    .and_then(|()| {
                        std::fs::rename(&whole, path).map_err(|e| format!("write `{path}`: {e}"))
                    })
                    .inspect_err(|_| _ = std::fs::remove_file(&whole))?;
                kept.attachments
                    .push(attach(AttachmentKind::Reference, path));
                kept.acceptance = Acceptance::Taken;
            } else {
                seen.write(&new)?;
                kept.attachments.push(attach(AttachmentKind::Actual, &new));
                if let Some(marked) = looking.diff {
                    marked.write(&diff)?;
                    kept.attachments.push(attach(AttachmentKind::Diff, &diff));
                    if let Seen::Text(lines) = marked {
                        kept.diff = Some(lines);
                    }
                }
                if looking.settled {
                    kept.acceptance = Acceptance::Offered;
                }
            }
        }
        Ok(kept)
    }
}

fn attach(kind: AttachmentKind, path: &Utf8Path) -> Attachment {
    Attachment {
        kind,
        path: landed(path),
    }
}

/// `__snapshots__/<module>/<play>/<name>.<renderer>.png` beside the play's file,
/// the module without its crate, since the file already places it,
/// and the name [`dashed`]. A tree, `<name>.accesskit.json`, is drawn by no renderer.
fn reference_path(
    ask: &Ask<'_>,
    renderer: &str,
    dir: Option<&Utf8Path>,
) -> Result<Utf8PathBuf, String> {
    let name = dashed(ask.name);
    if name.is_empty() {
        return Err(format!(
            "{:?} has no letter or digit to name a reference by",
            ask.name
        ));
    }
    let mut path = match dir {
        Some(dir) => dir.to_owned(),
        None => {
            let here =
                std::env::current_dir().map_err(|e| format!("find the working directory: {e}"))?;
            let here = Utf8PathBuf::from_path_buf(here)
                .map_err(|dir| format!("the working directory `{}` is not UTF-8", dir.display()))?;
            play_dir(ask.file, &here)?.join(crate::play::snapshot::DIR)
        }
    };
    for module in ask.module_path.split("::").skip(1) {
        path.push(component(module)?);
    }
    path.push(component(ask.play)?);
    path.push(match ask.of {
        Of::Screenshot { .. } => format!("{name}.{renderer}.png"),
        Of::Tree { .. } => format!("{name}.accesskit.json"),
    });
    Ok(path)
}

/// A module or a play as a directory. A [`PlayEntry`](crate::PlayEntry) built by hand
/// may name anything, a way out of the references' directory included.
fn component(name: &str) -> Result<&str, String> {
    if name.is_empty() || name.contains(['/', '\\', ':', '.', '\0']) {
        return Err(format!("{name:?} is no name to keep references under"));
    }
    Ok(name)
}

/// The directory of `file`, as `file!()` gives it: absolute for a scene file,
/// relative to the workspace for a module, where a member's tests run from below.
fn play_dir(file: &str, here: &Utf8Path) -> Result<Utf8PathBuf, String> {
    here.ancestors()
        .map(|dir| dir.join(file))
        .find(|found| found.is_file())
        .and_then(|found| found.parent().map(Utf8Path::to_owned))
        .ok_or_else(|| {
            format!(
                "the play's file `{file}` is not in `{here}` or above it, to keep references beside"
            )
        })
}

fn read_image(path: &Utf8Path) -> Reference {
    read(path, |bytes| {
        image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
            .map(|image| Seen::Image(image.to_rgba8()))
            .map_err(|e| e.to_string())
    })
}

fn read_tree(path: &Utf8Path) -> Reference {
    read(path, |bytes| {
        crate::play::tree::rewritten(bytes).map(Seen::Text)
    })
}

fn read(path: &Utf8Path, decode: impl FnOnce(&[u8]) -> Result<Seen, String>) -> Reference {
    match std::fs::read(path) {
        Ok(bytes) => match decode(&bytes) {
            Ok(seen) => Reference::Read(seen),
            Err(why) => Reference::Unreadable { why, bytes },
        },
        Err(missing) if missing.kind() == std::io::ErrorKind::NotFound => Reference::Missing,
        Err(e) => Reference::Unreadable {
            why: e.to_string(),
            bytes: Vec::new(),
        },
    }
}

/// What `seen` comes to beside `reference`,
/// and what marks where they differ.
fn compare(
    reference: &Reference,
    seen: &Seen,
    threshold: f32,
    allowed: u32,
) -> (Verdict, Option<Seen>) {
    match (reference, seen) {
        (Reference::Missing, _) => (Verdict::Missing, None),
        // serde_json reads 128 levels and writes any number,
        // so a deep tree is told from its bytes or not at all.
        (Reference::Unreadable { bytes, .. }, Seen::Text(actual))
            if *bytes == actual.as_bytes() =>
        {
            (Verdict::Identical, None)
        }
        (Reference::Unreadable { why, .. }, _) => (Verdict::Unreadable(why.clone()), None),
        (Reference::Read(reference), seen) if reference == seen => (Verdict::Identical, None),
        (Reference::Read(Seen::Image(reference)), Seen::Image(actual))
            if reference.dimensions() != actual.dimensions() =>
        {
            let verdict = Verdict::Resized {
                reference: reference.dimensions().into(),
                actual: actual.dimensions().into(),
            };
            (verdict, None)
        }
        (Reference::Read(Seen::Image(reference)), Seen::Image(actual)) => {
            let (differing, diff) = differ(reference, actual, threshold);
            let verdict = if differing <= allowed {
                Verdict::Alike { differing, allowed }
            } else {
                Verdict::Differs { differing, allowed }
            };
            (verdict, Some(Seen::Image(diff)))
        }
        (Reference::Read(Seen::Text(reference)), Seen::Text(actual)) => {
            let (removed, added, diff) = changed(reference, actual);
            let verdict = Verdict::Changed { removed, added };
            (verdict, Some(Seen::Text(diff)))
        }
        (Reference::Read(_), _) => {
            unreachable!("BUG: a reference is read as what its check compares")
        }
    }
}

/// How many lines of `reference` are gone from `actual`, how many
/// are new in it, and both with `-` and `+` before them among
/// the lines around, `…` standing for those left out.
fn changed(reference: &str, actual: &str) -> (usize, usize, String) {
    const AROUND: usize = 3;
    let diff = TextDiff::from_lines(reference, actual);
    let (mut removed, mut added, mut shown) = (0, 0, String::new());
    for (nth, group) in diff.grouped_ops(AROUND).iter().enumerate() {
        if nth > 0 {
            shown.push_str("…\n");
        }
        for change in group.iter().flat_map(|op| diff.iter_changes(op)) {
            let sign = match change.tag() {
                ChangeTag::Delete => {
                    removed += 1;
                    '-'
                }
                ChangeTag::Insert => {
                    added += 1;
                    '+'
                }
                ChangeTag::Equal => ' ',
            };
            shown.push(sign);
            shown.push_str(change.value().trim_end_matches('\n'));
            shown.push('\n');
        }
    }
    (removed, added, shown)
}

/// The largest squared YIQ distance between two colours, which scales a 0–1 threshold
/// into the units `dify::diff::get_results` takes, as dify's own `run` does.
const MAX_YIQ_DELTA: f32 = 35_215.0;

/// How much of the reference shows, in grey, under a diff's red:
/// pixelmatch's 0.1 washes a dark UI out to near white.
const UNCHANGED_SHOWN: f32 = 0.7;

/// How many pixels of `actual` differ from `reference`, and an image marking them.
/// Antialiased pixels count only at a threshold of 0.
fn differ(reference: &RgbaImage, actual: &RgbaImage, threshold: f32) -> (u32, RgbaImage) {
    let (differing, diff) = dify::diff::get_results(
        reference.clone(),
        actual.clone(),
        MAX_YIQ_DELTA * threshold * threshold,
        threshold > 0.0,
        Some(UNCHANGED_SHOWN),
        &None,
        &None,
    )
    .expect("BUG: dify draws a diff whenever it is told how to draw unchanged pixels");
    (u32::try_from(differing).unwrap_or(u32::MAX), diff)
}

#[cfg(test)]
mod tests {
    use image::Rgba;

    use super::*;

    fn grey(level: u8) -> RgbaImage {
        RgbaImage::from_pixel(8, 8, Rgba([level, level, level, 255]))
    }

    #[test]
    fn a_threshold_is_a_colour_distance_not_its_square() {
        let differing = |level, threshold| differ(&grey(100), &grey(level), threshold).0;
        assert_eq!(differing(125, 0.1), 0, "25 levels of grey are 0.095 apart");
        assert_eq!(differing(130, 0.1), 64, "30 levels are 0.114 apart");
        assert_eq!(differing(130, 0.2), 0);
        assert_eq!(differing(101, 0.0), 64, "at 0 any difference counts");
    }

    #[test]
    fn an_antialiased_pixel_counts_only_at_threshold_0() {
        let edge = |level: u8| {
            RgbaImage::from_fn(9, 9, |x, _| match x {
                0..4 => Rgba([0, 0, 0, 255]),
                4 => Rgba([level, level, level, 255]),
                _ => Rgba([255, 255, 255, 255]),
            })
        };
        let differing = |threshold| differ(&edge(90), &edge(190), threshold).0;
        assert_eq!(differing(0.1), 0, "a moved edge is antialiasing");
        assert_eq!(differing(0.0), 9);
    }

    #[test]
    fn a_diff_is_red_over_the_greyed_reference() {
        let mut actual = grey(100);
        actual.put_pixel(3, 3, Rgba([255, 255, 255, 255]));
        let (differing, diff) = differ(&grey(100), &actual, 0.0);
        assert_eq!(differing, 1);
        assert_eq!(*diff.get_pixel(3, 3), Rgba([255, 0, 0, 255]));
        let Rgba([r, g, b, _]) = *diff.get_pixel(0, 0);
        assert!(r == g && g == b && r > 100, "{r} {g} {b}");
    }

    #[test]
    fn a_text_diff_counts_its_lines_and_leaves_out_those_far_from_one() {
        let reference: String = (0..20).map(|n| format!("{n}\n")).collect();
        let actual = reference
            .replace("2\n3\n", "2\nthree\n")
            .replace("17\n", "");
        let (removed, added, diff) = changed(&reference, &actual);
        assert_eq!((removed, added), (2, 1));
        assert_eq!(
            diff,
            " 0\n 1\n 2\n-3\n+three\n 4\n 5\n 6\n…\n 14\n 15\n 16\n-17\n 18\n 19\n"
        );
    }

    #[test]
    fn a_plays_file_is_looked_for_from_the_working_directory_up() {
        let workspace = Utf8Path::new(env!("CARGO_MANIFEST_DIR"));
        let dir = workspace.join("src/render");
        for here in [workspace.to_owned(), workspace.join("src/render/tests")] {
            assert_eq!(play_dir(file!(), &here).as_deref(), Ok(&*dir), "{here}");
        }
        let absolute = dir.join("snapshot.rs");
        assert_eq!(
            play_dir(absolute.as_str(), Utf8Path::new("/")).as_deref(),
            Ok(&*dir)
        );
        let lost = play_dir("nowhere/lost.scene.rs", workspace).expect_err("no such file");
        assert!(lost.contains("nowhere/lost.scene.rs"), "{lost}");
    }

    fn path_of(module_path: &str, play: &str) -> Result<String, String> {
        let ask = Ask {
            file: "",
            module_path,
            play,
            name: "saved",
            of: Of::Tree { by: None },
            misused: None,
            first: true,
        };
        reference_path(&ask, "wgpu", Some(Utf8Path::new("refs"))).map(|path| path.to_string())
    }

    #[test]
    fn a_reference_is_kept_under_its_plays_module_path() {
        for (module, play, kept) in [
            ("app::form_1", "saves", "refs/form_1/saves"),
            ("app::widgets::form", "saves", "refs/widgets/form/saves"),
            ("app::form", "नमस्ते", "refs/form/नमस्ते"),
            ("app", "saves", "refs/saves"),
        ] {
            assert_eq!(
                path_of(module, play),
                Ok(format!("{kept}/saved.accesskit.json"))
            );
        }
    }

    #[test]
    fn a_play_built_by_hand_can_name_no_way_out_of_its_references() {
        for name in ["", "..", "../../escaped", "/tmp/x", "a\\b", "C:x"] {
            assert!(path_of("app::form", name).is_err(), "{name:?}");
            assert!(
                path_of(&format!("app::{name}"), "saves").is_err(),
                "{name:?}"
            );
        }
    }

    #[test]
    fn runs_taking_one_reference_at_once_all_take_it() {
        let scratch = tempfile::tempdir().expect("a directory");
        let dir = Utf8PathBuf::from_path_buf(scratch.path().to_owned()).expect("UTF-8");
        let snapshots = Snapshots {
            update: Update::Failing,
            dir: Some(dir.clone()),
            ..Snapshots::default()
        };
        let take = |run: u32| {
            let mut looks = Looks::new(&snapshots, "wgpu");
            let ask = |first| Ask {
                file: "",
                module_path: "app",
                play: "saves",
                name: "saved",
                of: Of::Tree { by: None },
                misused: None,
                first,
            };
            for round in 0..40 {
                // No other's, so every round takes.
                let seen = || Seen::Text(format!("run {run}, round {round}\n"));
                looks.checked.clear();
                looks.begin(&ask(true)).expect("begun");
                for _ in 0..=QUIET_FRAMES {
                    looks.judge(&ask(false), seen()).expect("judged");
                    looks.drew(false);
                }
                let kept = looks.keep().expect("kept");
                assert_eq!(kept.acceptance, Acceptance::Taken);
            }
        };
        std::thread::scope(|scope| {
            for run in 0..4 {
                scope.spawn(move || take(run));
            }
        });
        let left: Vec<_> = std::fs::read_dir(dir.join("saves"))
            .expect("the references")
            .flatten()
            .map(|file| file.file_name())
            .collect();
        assert_eq!(left, ["saved.accesskit.json"]);
    }
}
