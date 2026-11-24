//! Videos of catalog tests: a play recorded as it is tested,
//! for whoever reviews its failure.
//!
//! A video only watches: the test keeps its own clock
//! and animations, and no trouble with a video fails it.

use std::{
    fs,
    io::ErrorKind,
    sync::{
        OnceLock,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};

use gallery_build::Video;

use super::*;
use crate::video::to_mp4;

/// A test's frames are a quarter of a second apart,
/// which at its own speed would be slow to watch.
const FPS: u16 = 15;

/// Where a run's videos go, and which of them it keeps.
pub(crate) struct Films {
    output: Utf8PathBuf,
    /// Where they are while it isn't known whether they are kept: gallery's own,
    /// as what comes and goes in `output` could be seen by git, and by hot reload.
    recordings: Utf8PathBuf,
    video: Video,
    /// The catalog's crate.
    krate: String,
    files: OnceLock<Result<Files, String>>,
}

/// A play's video, dropped unless [kept](Self::keep).
///
/// An APNG until then, which grows with its canvas as H.264 cannot.
pub(super) struct Film<'f> {
    films: &'f Films,
    recording: Option<Recording>,
    recorded: tempfile::TempPath,
    /// Where it goes when kept.
    at: Utf8PathBuf,
}

impl Films {
    pub(crate) fn new(
        output: Utf8PathBuf,
        recordings: Utf8PathBuf,
        video: Video,
        krate: String,
    ) -> Self {
        Self {
            output,
            recordings,
            video,
            krate,
            files: OnceLock::new(),
        }
    }

    /// `<catalog>/<module>/<play>/video.<renderer>.mp4`. A play of another crate is kept
    /// apart by a dash, which no module's name has.
    fn place(&self, play: &PlayEntry, renderer: &str) -> Result<Utf8PathBuf, String> {
        let mut dir = self.output.join(component(&self.krate)?);
        let krate = play.module_path.split("::").next().unwrap_or_default();
        if krate != self.krate {
            dir.push(format!("extern-{}", component(krate)?));
        }
        dir.push(play_path(play.module_path, play.name)?);
        Ok(dir.join(format!("video.{renderer}.mp4")))
    }

    /// Remove an earlier run's video of `play`, which would pass for this one's,
    /// and the directories that leaves empty.
    ///
    /// # Errors
    /// When a video is there and cannot be removed.
    pub(super) fn clear(&self, play: &PlayEntry, renderer: &str) -> Result<(), String> {
        let Ok(at) = self.place(play, renderer) else {
            return Ok(());
        };
        match fs::remove_file(&at) {
            Ok(()) => {}
            Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
                return Ok(());
            }
            Err(e) => return Err(format!("remove the earlier `{at}`: {e}")),
        }
        let mut dir = at.parent();
        while let Some(empty) = dir
            && empty != self.output
            && fs::remove_dir(empty).is_ok()
        {
            dir = empty.parent();
        }
        Ok(())
    }

    /// Start recording `play` as the canvas is now;
    /// `None` when no video is wanted.
    ///
    /// # Errors
    /// When the recording cannot start.
    pub(super) fn begin(
        &self,
        harness: &mut egui_kittest::Harness<'static, Canvas>,
        renderer: &str,
        play: &PlayEntry,
    ) -> Result<Option<Film<'_>>, String> {
        if self.video == Video::Off {
            return Ok(None);
        }
        let at = self.place(play, renderer)?;
        let path = self.files()?.next("apng");
        let recorded = temporary(&path)?;
        watch(harness, &path, FPS)?;
        Ok(Some(Film {
            films: self,
            recording: None,
            recorded,
            at,
        }))
    }

    fn files(&self) -> Result<&Files, String> {
        self.files
            .get_or_init(|| Files::open(&self.recordings))
            .as_ref()
            .map_err(Clone::clone)
    }

    pub(super) fn keeps_passing(&self) -> bool {
        self.video == Video::Always
    }
}

impl Film<'_> {
    /// Take the recording out of `harness`, the play being over.
    pub(super) fn end(&mut self, harness: &mut egui_kittest::Harness<'static, Canvas>) {
        self.recording = harness.state_mut().recording.take();
    }

    /// Write the finished video, of a canvas at `scale`, to its place: an MP4,
    /// with the play's keys, mouse buttons and pointer over it.
    ///
    /// # Errors
    /// When it cannot be written, or an error had stopped it.
    pub(super) fn keep(self, session: &Session, scale: f32) -> Result<Utf8PathBuf, String> {
        guarded("keep the video", || self.kept(session, scale))
    }

    fn kept(self, session: &Session, scale: f32) -> Result<Utf8PathBuf, String> {
        let Self {
            films,
            recording,
            recorded,
            at,
        } = self;
        let recording = recording.expect("BUG: a video kept before its play was over");
        let input = recording.close()?;
        let path = films.files()?.next("mp4");
        let file = fs::File::create_new(&path).map_err(|e| format!("create `{path}`: {e}"))?;
        let video = temporary(&path)?;
        let mut layer: Option<InputLayer> = None;
        to_mp4(utf8(&recorded)?, file, &input, |shown, frame| {
            let layer = match &mut layer {
                Some(layer) => layer,
                None => {
                    let size = egui::vec2(frame.width() as f32, frame.height() as f32) / scale;
                    let opened = InputLayer::open(session, size, scale, Corner::default());
                    layer.insert(opened.map_err(|e| e.summary())?)
                }
            };
            layer.lay(shown, frame)
        })?;
        let output = &films.output;
        create_output(output, |at, text| fs::write(at, text))
            .map_err(|e| format!("create `{output}`: {e}"))?;
        place_at(&at, || {
            moved(|| fs::rename(&video, &at), || fs::copy(&video, &at), &at)
        })
        .map_err(|e| format!("write `{at}`: {e}"))?;
        Ok(landed(&at))
    }
}

/// Have `put` put a file at `at`. Another test may be removing the directories on the way
/// there, which it finds empty, so they are made again, a few times, when they have gone.
fn place_at(at: &Utf8Path, mut put: impl FnMut() -> std::io::Result<()>) -> std::io::Result<()> {
    let mut attempt = || {
        if let Some(dir) = at.parent() {
            fs::create_dir_all(dir)?;
        }
        put()
    };
    for _ in 0..9 {
        match attempt() {
            Err(gone) if gone.kind() == ErrorKind::NotFound => {}
            done => return done,
        }
    }
    attempt()
}

/// `rename` a file to `to`, or `copy` it there when that is on another file system:
/// whole, or not at all.
fn moved(
    rename: impl FnOnce() -> std::io::Result<()>,
    copy: impl FnOnce() -> std::io::Result<u64>,
    to: &Utf8Path,
) -> std::io::Result<()> {
    match rename() {
        Err(e) if e.kind() == ErrorKind::CrossesDevices => {
            copy().map(drop).inspect_err(|_| _ = fs::remove_file(to))
        }
        done => done,
    }
}

/// This process's files among the recordings: `.run-<name>-<n>.<ext>`,
/// beside a `.run-<name>.lock` it has locked for as long as it lives.
struct Files {
    stem: Utf8PathBuf,
    made: AtomicU32,
    _locked: fs::File,
    _lock: tempfile::TempPath,
}

/// How long a run may take from creating its lock file to locking it.
const UNLOCKED_FOR: Duration = Duration::from_mins(1);

impl Files {
    fn open(recordings: &Utf8Path) -> Result<Self, String> {
        fs::create_dir_all(recordings).map_err(|e| format!("create `{recordings}`: {e}"))?;
        sweep(recordings);
        let error = |e: std::io::Error| format!("create a file in `{recordings}`: {e}");
        let (locked, lock) = tempfile::Builder::new()
            .prefix(".run-")
            .suffix(".lock")
            .make_in(recordings, |path| fs::File::create_new(path))
            .map_err(error)?
            .into_parts();
        locked
            .lock()
            .map_err(|e| format!("lock `{}`: {e}", lock.display()))?;
        Ok(Self {
            stem: utf8(&lock)?.with_extension(""),
            made: AtomicU32::new(0),
            _locked: locked,
            _lock: lock,
        })
    }

    fn next(&self, extension: &str) -> Utf8PathBuf {
        let n = self.made.fetch_add(1, Ordering::Relaxed);
        Utf8PathBuf::from(format!("{}-{n}.{extension}", self.stem))
    }
}

/// Remove the files of runs that ended without doing so, whose lock is free.
fn sweep(recordings: &Utf8Path) {
    let names: Vec<String> = match recordings.read_dir_utf8() {
        Ok(files) => files
            .flatten()
            .map(|file| file.file_name().to_owned())
            .filter(|name| name.starts_with(".run-"))
            .collect(),
        Err(_) => return,
    };
    for lock in names.iter().filter(|name| name.ends_with(".lock")) {
        let path = recordings.join(lock);
        let Ok(file) = fs::File::open(&path) else {
            continue;
        };
        let age = file.metadata().and_then(|lock| lock.modified());
        let settled = age.is_ok_and(|at| at.elapsed().is_ok_and(|age| age > UNLOCKED_FOR));
        if !settled || file.try_lock().is_err() {
            continue;
        }
        let of_run = format!("{}-", lock.trim_end_matches(".lock"));
        for left in names.iter().filter(|name| name.starts_with(&of_run)) {
            _ = fs::remove_file(recordings.join(left));
        }
        _ = fs::remove_file(&path);
    }
}

/// Create the tests' output directory, unless it is there: the topmost directory made for it
/// is tagged as a cache, which keeps the hot-reload watcher off it, and ignored by git.
fn create_output(
    output: &Utf8Path,
    write: impl FnMut(&Utf8Path, &str) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let missing = output.ancestors().take_while(|dir| !dir.exists()).last();
    if let Some(top) = missing {
        match fs::create_dir(top) {
            Ok(()) => tag(top, write).inspect_err(|_| _ = fs::remove_dir(top))?,
            // Another test made it meanwhile, which tags it.
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    fs::create_dir_all(output)
}

/// Tag `dir` as a cache, or leave nothing in it.
fn tag(
    dir: &Utf8Path,
    mut write: impl FnMut(&Utf8Path, &str) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let [cache, git] = ["CACHEDIR.TAG", ".gitignore"].map(|name| dir.join(name));
    write(&cache, CACHE_TAG)
        .and_then(|()| write(&git, "*\n"))
        .inspect_err(|_| {
            _ = fs::remove_file(&git);
            _ = fs::remove_file(&cache);
        })
}

/// The signature <https://bford.info/cachedir/> asks for.
const CACHE_TAG: &str = "Signature: 8a477f597d28d172789f06886806bc55\n";

/// `path`, removed once this is dropped.
fn temporary(path: &Utf8Path) -> Result<tempfile::TempPath, String> {
    tempfile::TempPath::try_from_path(path).map_err(|e| format!("find `{path}`: {e}"))
}

fn utf8(path: &std::path::Path) -> Result<&Utf8Path, String> {
    Utf8Path::from_path(path).ok_or_else(|| format!("`{}` is not UTF-8", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What is in `dir`, by name.
    fn names(dir: &Utf8Path) -> Vec<String> {
        let mut names: Vec<String> = dir
            .read_dir_utf8()
            .expect("a directory")
            .map(|file| file.expect("a file").file_name().to_owned())
            .collect();
        names.sort();
        names
    }

    /// Make `path` as old as a run that has had time to lock its file.
    fn age(path: &Utf8Path) {
        let file = fs::File::options().write(true).open(path).expect("opened");
        let then = std::time::SystemTime::now() - 2 * UNLOCKED_FOR;
        file.set_modified(then).expect("aged");
    }

    #[test]
    fn a_run_removes_the_files_of_those_that_ended_and_no_others() {
        let scratch = tempfile::tempdir().expect("a directory");
        let output = utf8(scratch.path()).expect("UTF-8").join("recordings");
        let living = Files::open(&output).expect("a run");
        age(&living.stem.with_extension("lock"));
        fs::write(living.next("apng"), "").expect("written");
        for file in [
            ".run-dead.lock",
            ".run-dead-0.apng",
            ".run-dead-1.mp4",
            ".run-young.lock",
            ".run-young-0.apng",
            "notes.txt",
        ] {
            fs::write(output.join(file), "").expect("written");
        }
        age(&output.join(".run-dead.lock"));
        drop(Files::open(&output).expect("another run"));
        let left = names(&output);
        assert!(!left.iter().any(|name| name.contains("dead")), "{left:?}");
        let young = left.iter().filter(|name| name.contains("young"));
        assert_eq!(young.count(), 2, "{left:?}");
        assert!(left.contains(&"notes.txt".to_owned()), "{left:?}");
        let name = living.stem.file_name().expect("a name").to_owned();
        let own = left.iter().filter(|left| left.starts_with(&name));
        assert_eq!(
            own.count(),
            2,
            "a living run keeps its lock and file: {left:?}"
        );
        assert_eq!(left.len(), 5, "a run that ends leaves nothing: {left:?}");
    }

    #[test]
    fn a_runs_files_each_have_a_name_of_their_own() {
        let scratch = tempfile::tempdir().expect("a directory");
        let files = Files::open(utf8(scratch.path()).expect("UTF-8")).expect("a run");
        assert_ne!(files.next("apng"), files.next("apng"));
    }

    #[test]
    fn a_video_is_placed_again_a_few_times_when_its_directory_has_gone() {
        let scratch = tempfile::tempdir().expect("a directory");
        let at = utf8(scratch.path())
            .expect("UTF-8")
            .join("plays/greets/video.mp4");
        let dir = at.parent().expect("a directory");
        let tried = |gone_for: u32, kind: ErrorKind| {
            let mut tries = 0;
            let outcome = place_at(&at, || {
                assert!(dir.is_dir(), "it is made again");
                tries += 1;
                if tries > gone_for {
                    return Ok(());
                }
                fs::remove_dir(dir).expect("removed");
                Err(kind.into())
            });
            (outcome.map_err(|e| e.kind()), tries)
        };
        assert_eq!(tried(0, ErrorKind::NotFound), (Ok(()), 1));
        assert_eq!(tried(9, ErrorKind::NotFound), (Ok(()), 10));
        let gone = Err(ErrorKind::NotFound);
        assert_eq!(tried(10, ErrorKind::NotFound), (gone, 10));
        let denied = Err(ErrorKind::PermissionDenied);
        assert_eq!(tried(1, ErrorKind::PermissionDenied), (denied, 1));
    }

    #[test]
    fn a_video_is_copied_to_another_file_system_whole_or_not_at_all() {
        let scratch = tempfile::tempdir().expect("a directory");
        let at = utf8(scratch.path()).expect("UTF-8").join("video.mp4");
        let written = |rename: Option<ErrorKind>, copy: Option<ErrorKind>| {
            _ = fs::remove_file(&at);
            let moved = moved(
                || match rename {
                    Some(kind) => Err(kind.into()),
                    None => fs::write(&at, "renamed"),
                },
                || {
                    fs::write(&at, "copied")?;
                    copy.map_or(Ok(6), |kind| Err(kind.into()))
                },
                &at,
            );
            (moved.map_err(|e| e.kind()), fs::read_to_string(&at).ok())
        };
        let crosses = Some(ErrorKind::CrossesDevices);
        let full = ErrorKind::StorageFull;
        assert_eq!(written(None, None), (Ok(()), Some("renamed".to_owned())));
        assert_eq!(written(crosses, None), (Ok(()), Some("copied".to_owned())));
        assert_eq!(written(crosses, Some(full)), (Err(full), None));
        assert_eq!(written(Some(full), None), (Err(full), None));
    }

    #[test]
    fn a_panic_while_a_video_is_kept_is_an_error() {
        let session = Session::open(crate::Renderer::Wgpu).expect("a session");
        let scratch = tempfile::tempdir().expect("a directory");
        let root = utf8(scratch.path()).expect("UTF-8");
        let films = Films::new(
            root.join("out"),
            root.to_owned(),
            Video::Always,
            "plays".to_owned(),
        );
        let recorded = root.join("recorded.apng");
        let film = Film {
            films: &films,
            recording: None,
            recorded: temporary(&recorded).expect("a path"),
            at: root.join("out/video.mp4"),
        };
        let error = film.keep(&session, 1.0).expect_err("it never ended");
        assert!(error.starts_with("keep the video: BUG:"), "{error}");
    }

    #[test]
    fn tests_making_one_output_directory_at_once_all_find_it() {
        let scratch = tempfile::tempdir().expect("a directory");
        let root = utf8(scratch.path()).expect("UTF-8");
        let barrier = std::sync::Barrier::new(8);
        for round in 0..50 {
            let output = root.join(format!("{round}/tests"));
            std::thread::scope(|scope| {
                for _ in 0..8 {
                    scope.spawn(|| {
                        barrier.wait();
                        create_output(&output, write).expect("created");
                        assert!(output.is_dir());
                    });
                }
            });
        }
    }

    fn write(at: &Utf8Path, text: &str) -> std::io::Result<()> {
        fs::write(at, text)
    }

    #[test]
    fn a_directory_that_cannot_be_tagged_is_not_made() {
        let scratch = tempfile::tempdir().expect("a directory");
        let root = utf8(scratch.path()).expect("UTF-8");
        for full_at in ["CACHEDIR.TAG", ".gitignore"] {
            let made = create_output(&root.join("target/tests"), |at, text| {
                if at.file_name() == Some(full_at) {
                    write(at, "half")?;
                    return Err(ErrorKind::StorageFull.into());
                }
                write(at, text)
            });
            assert_eq!(made.map_err(|e| e.kind()), Err(ErrorKind::StorageFull));
            assert_eq!(names(root), Vec::<String>::new(), "{full_at}");
        }
    }

    #[test]
    fn the_topmost_directory_made_is_tagged_and_one_that_was_there_is_not() {
        let scratch = tempfile::tempdir().expect("a directory");
        let root = utf8(scratch.path()).expect("UTF-8");
        create_output(&root.join("target/tests"), write).expect("created");
        assert_eq!(
            names(&root.join("target")),
            [".gitignore", "CACHEDIR.TAG", "tests"]
        );
        assert_eq!(names(&root.join("target/tests")), Vec::<String>::new());

        fs::create_dir(root.join("there")).expect("made");
        create_output(&root.join("there"), write).expect("created");
        assert_eq!(names(&root.join("there")), Vec::<String>::new());
        assert_eq!(names(root), ["target", "there"], "nothing is left beside");
    }
}
