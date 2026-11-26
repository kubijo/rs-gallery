//! What a catalog's tests came to: files for each test,
//! and an HTML report of them all at the root of the tests' output.
//!
//! Under nextest every test is a process of its own,
//! so each writes its own files, and the report
//! is rebuilt from those there are.

use std::{
    fs,
    panic::AssertUnwindSafe,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use camino::{Utf8Path, Utf8PathBuf};
use jiff::Timestamp;

pub(crate) use self::output::is_written;
use self::output::{Output, PATIENCE};
use crate::{
    PlayEntry,
    diagnostic::{Diagnostic, Highlighted},
    play::log::{Acceptance, AttachmentKind, Check, Entry, Outcome, What},
    render::{Checked, Filmed, place_at, play_dir_in},
};

mod output;
mod page;
#[cfg(test)]
mod tests;

/// The report, at the root of the tests' output.
pub(crate) const PAGE: &str = "index.html";

/// Gallery's: the page shows no results of another one's,
/// whose markup its styles and script may not fit.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// How a run's results are written down.
pub(crate) struct Results {
    output: Arc<Output>,
    /// What tells this run's results from an earlier one's.
    run: String,
    ran: AtomicBool,
    /// What keeps the report up to date while tests run.
    following: Mutex<Option<Following>>,
}

/// Whose tests they are, and where their results go.
pub(crate) struct Tested {
    pub(crate) output: Utf8PathBuf,
    pub(crate) krate: String,
    pub(crate) manifest_dir: Utf8PathBuf,
    pub(crate) renderer: &'static str,
}

/// A test, as its result is filed.
#[derive(Clone, Copy)]
pub(crate) struct Trial<'a> {
    /// Its name, as a test.
    pub(crate) name: &'a str,
    /// The key of its scene.
    pub(crate) scene: &'a str,
    /// The play it runs, if it does not just draw the scene.
    pub(crate) play: Option<&'a PlayEntry>,
}

/// What every part of the report says of a test.
#[derive(Clone, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
struct Kept {
    /// The [`VERSION`] that wrote it.
    gallery: String,
    krate: String,
    manifest_dir: Utf8PathBuf,
    renderer: String,
    run: String,
    at: Timestamp,
    trial: String,
    scene: String,
    play: Option<String>,
    verdict: Verdict,
    took_ms: u64,
    warned: bool,
    /// It passed, a screenshot check of it within its budget though pixels differ.
    tolerated: bool,
}

/// A test's result, as it is kept.
#[derive(Clone, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
struct Stored {
    #[serde(flatten)]
    kept: Kept,
    failure: Option<Highlighted>,
    warnings: Vec<String>,
    log: Vec<Line>,
    /// In the output, as every path here.
    video: Option<Utf8PathBuf>,
}

/// The worst last.
#[derive(
    Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
enum Verdict {
    Passed,
    /// It began, and took its process with it, or is still going.
    Unfinished,
    Failed,
}

/// A line of a play's log.
#[derive(Clone, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
struct Line {
    depth: usize,
    kind: Kind,
    text: String,
    /// What a failed check found, or why a skipped one was.
    found: Option<String>,
    /// More of a failed check than its line says.
    notes: Vec<String>,
    outcome: LineOutcome,
    frames: u32,
    /// The play's source line.
    source: Option<String>,
    /// Where in the play's video it begins, in seconds.
    seek: Option<f64>,
    files: Vec<File>,
    diff: Option<String>,
    /// The reference was taken this run, or could be.
    acceptance: Option<Taking>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Step,
    Gesture,
    Check,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum LineOutcome {
    /// The play did not get to its end.
    Unfinished,
    Passed,
    Tolerated,
    Failed,
    Skipped,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum Taking {
    Offered,
    Taken,
}

/// What a snapshot check left to look at.
#[derive(Clone, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
struct File {
    name: String,
    path: Utf8PathBuf,
}

impl Results {
    /// Results of `trials`, in the catalog's order.
    pub(crate) fn new(
        tested: Tested,
        trials: impl IntoIterator<Item = String>,
        warn: impl Fn(&Diagnostic) + Send + Sync + 'static,
    ) -> Self {
        Self {
            output: Arc::new(Output::new(tested, trials, warn)),
            run: run_id(std::env::var("NEXTEST_RUN_ID").ok()),
            ran: AtomicBool::new(false),
            following: Mutex::new(None),
        }
    }

    /// Run `body` as `trial` and keep what it came to.
    /// Until it is over, and if it ends the process,
    /// the report has it that it did not finish.
    pub(crate) fn of(&self, trial: Trial<'_>, body: impl FnOnce() -> Checked) -> Checked {
        self.begin(trial);
        let began = Instant::now();
        let checked = match std::panic::catch_unwind(AssertUnwindSafe(body)) {
            Ok(checked) => checked,
            Err(panic) => {
                let failure = crate::render::panic_message(&panic);
                self.output.bear(self.output.write(&Stored {
                    failure: Some(Highlighted::plain(format!("panicked: {failure}"))),
                    ..self.stored(trial, Verdict::Failed, began.elapsed())
                }));
                std::panic::resume_unwind(panic);
            }
        };
        let finished = self.finished(trial, &checked, began.elapsed());
        self.output.bear(self.output.write(&finished));
        checked
    }

    fn begin(&self, trial: Trial<'_>) {
        self.ran.store(true, Ordering::Relaxed);
        let unfinished = self.stored(trial, Verdict::Unfinished, Duration::ZERO);
        self.output.bear(self.output.write(&unfinished));
        self.output.bear(self.output.rebuild().map(drop));
        let mut following = self.following();
        if following.is_none() {
            match Following::begin(&self.output) {
                Ok(begun) => *following = Some(begun),
                Err(e) => self.output.bear(Err(format!("follow the tests: {e}"))),
            }
        }
    }

    fn following(&self) -> MutexGuard<'_, Option<Following>> {
        let following = self.following.lock();
        following.unwrap_or_else(PoisonError::into_inner)
    }

    fn stored(&self, trial: Trial<'_>, verdict: Verdict, took: Duration) -> Stored {
        let tested = &self.output.tested;
        Stored {
            kept: Kept {
                gallery: VERSION.to_owned(),
                krate: tested.krate.clone(),
                manifest_dir: tested.manifest_dir.clone(),
                renderer: tested.renderer.to_owned(),
                run: self.run.clone(),
                at: Timestamp::now(),
                trial: trial.name.to_owned(),
                scene: trial.scene.to_owned(),
                play: trial.play.map(|play| play.name.to_owned()),
                verdict,
                took_ms: u64::try_from(took.as_millis()).unwrap_or(u64::MAX),
                warned: false,
                tolerated: false,
            },
            failure: None,
            warnings: Vec::new(),
            log: Vec::new(),
            video: None,
        }
    }

    fn finished(&self, trial: Trial<'_>, checked: &Checked, took: Duration) -> Stored {
        let verdict = match checked.failure {
            Some(_) => Verdict::Failed,
            None => Verdict::Passed,
        };
        let tested = &self.output.tested;
        let output = &tested.output;
        let mut trouble = Vec::new();
        let dir = trial
            .play
            .map(|play| play_dir_in(output, &tested.krate, play))
            .transpose()
            .unwrap_or_else(|why| {
                trouble.push(why);
                None
            });
        // Where it landed may be elsewhere by name, by way of a link in the output.
        let video = checked.filmed.as_ref().zip(dir.as_ref());
        let video = video.and_then(|(film, dir)| Some(dir.join(film.path.file_name()?)));
        let mut keeper = Keeper {
            output,
            dir: dir.map(|dir| dir.join(format!("snapshots-{}", tested.renderer))),
            filmed: checked.filmed.as_ref(),
            trouble,
        };
        keeper.clear();
        let log = checked.log.iter().map(|entry| keeper.line(entry)).collect();
        let warnings = checked.warnings.iter().map(Diagnostic::summary);
        let mut finished = Stored {
            failure: checked.failure.as_ref().map(Diagnostic::beside_recordings),
            warnings: warnings.chain(keeper.trouble).collect(),
            log,
            video: video.and_then(|video| video.strip_prefix(output).map(Utf8Path::to_owned).ok()),
            ..self.stored(trial, verdict, took)
        };
        finished.kept.warned = !finished.warnings.is_empty();
        let mut outcomes = finished.log.iter().map(|line| line.outcome);
        finished.kept.tolerated =
            verdict == Verdict::Passed && outcomes.any(|outcome| outcome == LineOutcome::Tolerated);
        finished
    }

    /// Rebuild the report a last time, the process's tests
    /// being over; where it is, if any ran.
    pub(crate) fn close(&self) -> Option<Utf8PathBuf> {
        if !self.ran.load(Ordering::Relaxed) {
            self.output.bear(self.output.forget());
            return None;
        }
        drop(self.following().take());
        let built = self.output.rebuild().map(drop);
        let there = built.is_ok().then(|| self.output.tested.output.join(PAGE));
        self.output.bear(built);
        there
    }
}

/// What all of a run's processes share, and no other run has.
fn run_id(nextest: Option<String>) -> String {
    nextest.unwrap_or_else(|| format!("{} of {}", std::process::id(), Timestamp::now()))
}

/// A thread that rebuilds the report as results come,
/// until this is dropped.
struct Following {
    /// Ends it when dropped.
    end: Option<mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Following {
    fn drop(&mut self) {
        drop(self.end.take());
        if let Some(thread) = self.thread.take() {
            _ = thread.join();
        }
    }
}

impl Following {
    /// The least there is between two rebuilds;
    /// slow ones and those that fail are further apart.
    const APART: Duration = Duration::from_millis(250);

    fn begin(output: &Arc<Output>) -> std::io::Result<Self> {
        let (end, ended) = mpsc::channel();
        let output = Arc::clone(output);
        let thread = std::thread::Builder::new().spawn(move || {
            let mut apart = Self::APART;
            while let Err(mpsc::RecvTimeoutError::Timeout) = ended.recv_timeout(apart) {
                let rebuilt = output.rebuild_if_changed();
                apart = Self::apart(apart, &rebuilt);
                output.bear(rebuilt.map(drop));
            }
        })?;
        Ok(Self {
            end: Some(end),
            thread: Some(thread),
        })
    }

    /// How long after a rebuild the next one is, `apart` from which that one was.
    fn apart<E>(apart: Duration, rebuilt: &Result<Duration, E>) -> Duration {
        match rebuilt {
            Ok(took) => Self::APART.max(4 * *took),
            Err(_) => (2 * apart).min(PATIENCE),
        }
    }
}

/// What turns a play's log into lines,
/// copying the files it names into the output.
struct Keeper<'a> {
    output: &'a Utf8Path,
    /// Where a play's files go.
    dir: Option<Utf8PathBuf>,
    filmed: Option<&'a Filmed>,
    /// Why files are missing from the report.
    trouble: Vec<String>,
}

impl Keeper<'_> {
    /// Remove the snapshots an earlier run left, which would pass for this one's,
    /// and nothing that a run could not have.
    fn clear(&self) {
        let Some(dir) = &self.dir else {
            return;
        };
        if !dir.symlink_metadata().is_ok_and(|dir| dir.is_dir()) {
            return;
        }
        for file in dir.read_dir_utf8().into_iter().flatten().flatten() {
            if matches!(file.path().extension(), Some("png" | "json")) {
                _ = fs::remove_file(file.path());
            }
        }
        _ = fs::remove_dir(dir);
    }

    fn line(&mut self, entry: &Entry) -> Line {
        let (kind, text) = match &entry.what {
            What::Step { label, .. } => (Kind::Step, label.clone()),
            What::Gesture { label } => (Kind::Gesture, label.clone()),
            What::Check(check) => {
                let expected = crate::render::one_line(&check.expected);
                (Kind::Check, format!("{} {expected}", check.subject))
            }
        };
        let mut line = Line {
            depth: entry.depth,
            kind,
            text,
            found: None,
            notes: Vec::new(),
            outcome: match entry.outcome {
                None => LineOutcome::Unfinished,
                Some(Outcome::Passed) => LineOutcome::Passed,
                Some(Outcome::Tolerated) => LineOutcome::Tolerated,
                Some(Outcome::Failed) => LineOutcome::Failed,
                Some(Outcome::Skipped) => LineOutcome::Skipped,
            },
            frames: u32::try_from(entry.frames.len()).unwrap_or(u32::MAX),
            source: entry
                .at
                .as_ref()
                .map(|at| format!("{}:{}", crate::style::relative(at.file.as_ref()), at.line)),
            seek: self.filmed.and_then(|film| film.before(entry.frames.start)),
            files: Vec::new(),
            diff: None,
            acceptance: None,
        };
        if let What::Check(check) = &entry.what {
            self.check(check, &mut line);
        }
        line
    }

    fn check(&mut self, check: &Check, line: &mut Line) {
        line.notes.extend(check.message.clone());
        if matches!(
            line.outcome,
            LineOutcome::Failed | LineOutcome::Tolerated | LineOutcome::Skipped
        ) {
            let found = check.actual.as_deref().unwrap_or("nothing matched");
            line.found = Some(crate::render::one_line(found));
            line.notes.extend(check.explanation.clone());
        }
        let Some(kept) = &check.kept else {
            return;
        };
        line.diff.clone_from(&kept.diff);
        line.acceptance = match kept.acceptance {
            Acceptance::None => None,
            Acceptance::Offered => Some(Taking::Offered),
            Acceptance::Taken => Some(Taking::Taken),
        };
        for attachment in &kept.attachments {
            // The check's own diff shows these lines already.
            if attachment.kind == AttachmentKind::Diff && kept.diff.is_some() {
                continue;
            }
            match self.copy(&attachment.path) {
                Ok(path) => line.files.extend(path.map(|path| File {
                    name: attachment.kind.name().to_owned(),
                    path,
                })),
                Err(why) => self.trouble.push(why),
            }
        }
    }

    /// Copy `file` into the output;
    /// where it is in there.
    fn copy(&self, file: &Utf8Path) -> Result<Option<Utf8PathBuf>, String> {
        let Some(dir) = &self.dir else {
            return Ok(None);
        };
        let copy = dir.join(file.file_name().unwrap_or_default());
        place_at(&copy, || fs::copy(file, &copy).map(drop))
            .map_err(|e| format!("copy `{file}` to `{copy}`: {e}"))?;
        let path = copy.strip_prefix(self.output).map(Utf8Path::to_owned);
        Ok(path.ok())
    }
}
