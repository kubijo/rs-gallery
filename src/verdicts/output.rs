//! The tests' output directory: the files of each result, and the report made of them all.
//! Processes and threads share it, which its lock files and the files moved into place whole are for.

use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{ErrorKind, Read as _, Write as _},
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use camino::{Utf8Path, Utf8PathBuf};
use jiff::Timestamp;

use super::{Kept, PAGE, Stored, Tested, VERSION, page};
use crate::{
    Renderer,
    diagnostic::Diagnostic,
    render::{create_output, place_at},
};

const REBUILDS: &str = ".gallery-rebuilds";
/// How many times the report was rebuilt.
const REBUILT: &str = ".gallery-rebuilt";
const WRITES: &str = ".gallery-writes";
/// What the name of a file being written begins with.
const WRITING: &str = ".gallery-writing-";
/// A catalog's [`List`], among its results.
const ORDER: &str = "order.json";
/// Of a test's part of the page.
const FRAGMENT: &str = "fragment";

/// How long a lock is waited for.
pub(super) const PATIENCE: Duration = Duration::from_secs(if cfg!(test) { 2 } else { 10 });

/// A catalog's own part of the output, and the report of all of it.
pub(super) struct Output {
    pub(super) tested: Tested,
    pub(super) list: List,
    /// A result was written that the report may not have.
    changed: AtomicBool,
    /// What writers lock together, and whoever removes a result locks alone.
    writes: Lock,
    /// What whoever rebuilds the report locks.
    rebuilds: Lock,
    warned: OnceLock<()>,
    warn: Box<dyn Fn(&Diagnostic) + Send + Sync>,
}

/// A lock file.
struct Lock {
    name: &'static str,
    /// It was waited for in vain, and not had since.
    stuck: AtomicBool,
}

impl Lock {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            stuck: AtomicBool::new(false),
        }
    }
}

/// The tests a catalog has, which every process judges its results by,
/// so that whichever makes the report makes the same one.
#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct List {
    /// When the process began whose list it is.
    pub(super) began: Timestamp,
    /// In the catalog's order.
    names: Vec<String>,
    /// Where each one's result goes, but for an extension.
    places: HashMap<String, Utf8PathBuf>,
}

impl List {
    fn new(names: Vec<String>) -> Self {
        Self {
            began: Timestamp::now(),
            places: places(&names),
            names,
        }
    }

    /// What `dir` has the results of, as the newest process had it;
    /// `None` when there is no list, or no whole one.
    fn of(dir: &Utf8Path) -> std::io::Result<Option<Self>> {
        match fs::read(dir.join(ORDER)) {
            Ok(json) => Ok(serde_json::from_slice(&json).ok()),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// `other` takes this one's place.
    fn gives_way_to(&self, other: &Self) -> bool {
        let is_same = self.names == other.names && self.places == other.places;
        // What began later than now did so by a clock that was ahead, and is no newer.
        let now = Timestamp::now();
        !is_same && (self.began > now || (self.began < other.began && other.began <= now))
    }

    /// `trial`'s result is filed at `path`,
    /// which may keep the case it first had.
    fn has(&self, trial: &str, path: &Utf8Path) -> bool {
        let place = self.places.get(trial);
        place.is_some_and(|place| caseless(place) == caseless(&path.with_extension("")))
    }
}

/// Rebuilds of the report, ever.
#[derive(Default, Debug, serde::Serialize, serde::Deserialize)]
pub(super) struct Rebuilt {
    /// The [`VERSION`] they were by: another's leave out this one's results.
    gallery: String,
    pub(super) begun: u64,
    /// The last that was finished.
    pub(super) done: u64,
}

/// A test in the report.
pub(super) struct Reported {
    pub(super) kept: Kept,
    /// In its catalog.
    pub(super) order: usize,
    /// What the page says of it when it is looked at.
    pub(super) body: String,
}

impl Output {
    pub(super) fn new(
        tested: Tested,
        trials: impl IntoIterator<Item = String>,
        warn: impl Fn(&Diagnostic) + Send + Sync + 'static,
    ) -> Self {
        Self {
            tested,
            list: List::new(trials.into_iter().collect()),
            changed: AtomicBool::new(false),
            writes: Lock::new(WRITES),
            rebuilds: Lock::new(REBUILDS),
            warned: OnceLock::new(),
            warn: Box::new(warn),
        }
    }

    /// `<output>/<crate>/results-<renderer>`: a dash no module's name has
    /// keeps it apart from the plays' directories.
    pub(super) fn results(&self) -> Utf8PathBuf {
        let Tested {
            output,
            krate,
            renderer,
            ..
        } = &self.tested;
        output.join(krate).join(format!("results-{renderer}"))
    }

    fn file(&self, trial: &str, extension: &str) -> Utf8PathBuf {
        let place = self.list.places.get(trial);
        let place = place.expect("BUG: a test its catalog does not have");
        self.results().join(format!("{place}.{extension}"))
    }

    /// Report the first thing that keeps the results
    /// from being written, as no test's failure.
    pub(super) fn bear(&self, done: Result<(), String>) {
        if let Err(why) = done
            && self.warned.set(()).is_ok()
        {
            (self.warn)(&Diagnostic::warning("the tests' report is incomplete").cause(why));
        }
    }

    /// Write a result: as JSON, for whatever else reads it, and as its part of the page,
    /// after a line of what the rest of the page says of it.
    pub(super) fn write(&self, stored: &Stored) -> Result<(), String> {
        let json = serde_json::to_vec_pretty(stored).map_err(|e| e.to_string())?;
        let mut fragment = serde_json::to_string(&stored.kept).map_err(|e| e.to_string())?;
        fragment.push('\n');
        fragment.push_str(&page::body(stored));
        let output = &self.tested.output;
        create_output(output, |at, text| fs::write(at, text))
            .map_err(|e| format!("create `{output}`: {e}"))?;
        // First of all that is written, which hot reload knows the output by.
        let _writing = self.locked(&self.writes, fs::File::try_lock_shared)?;
        let listed = self.list();
        put(&self.file(&stored.kept.trial, "json"), &json)?;
        put(
            &self.file(&stored.kept.trial, FRAGMENT),
            fragment.as_bytes(),
        )?;
        self.changed.store(true, Ordering::Release);
        listed
    }

    /// Drop what is left of a catalog that has no tests any more.
    pub(super) fn forget(&self) -> Result<(), String> {
        if !self.list.names.is_empty() || !self.results().is_dir() {
            return Ok(());
        }
        let writing = self.locked(&self.writes, fs::File::try_lock_shared)?;
        self.list()?;
        drop(writing);
        self.rebuild().map(drop)
    }

    /// Have the list be this process's, unless a newer one's is there.
    fn list(&self) -> Result<(), String> {
        let dir = self.results();
        if let Ok(Some(there)) = List::of(&dir)
            && !there.gives_way_to(&self.list)
        {
            return Ok(());
        }
        let list = serde_json::to_vec(&self.list).map_err(|e| e.to_string())?;
        put(&dir.join(ORDER), &list)
    }

    /// Rebuild the report if it may lack a result of this process's;
    /// how long that took, when it was this process that did.
    pub(super) fn rebuild_if_changed(&self) -> Result<Duration, String> {
        if self.changed.load(Ordering::Acquire) {
            self.rebuild()
        } else {
            Ok(Duration::ZERO)
        }
    }

    /// Have the report be of every result written by now;
    /// how long that took, when it was this process that did it.
    ///
    /// Those that come to it together need it rebuilt only once:
    /// by the first whose turn it is, for all that wait by then.
    pub(super) fn rebuild(&self) -> Result<Duration, String> {
        // Before anything is read, as another may be written meanwhile.
        self.changed.store(false, Ordering::Release);
        let rebuilt = self.rebuild_in_turn();
        if rebuilt.is_err() {
            self.changed.store(true, Ordering::Release);
        }
        rebuilt
    }

    fn rebuild_in_turn(&self) -> Result<Duration, String> {
        // One that begins after this reads what is written by now;
        // what is in the file may be of another version, and then none will do.
        let before = self.rebuilt().map_or(u64::MAX, |rebuilt| rebuilt.begun);
        let is_done = || self.rebuilt().is_some_and(|rebuilt| rebuilt.done > before);
        let locked = self.locked_unless(is_done, &self.rebuilds, fs::File::try_lock)?;
        let Some(_rebuilding) = locked else {
            return Ok(Duration::ZERO);
        };
        let began = Instant::now();
        let mut rebuilt = self.rebuilt().unwrap_or_default();
        VERSION.clone_into(&mut rebuilt.gallery);
        rebuilt.begun += 1;
        self.count(&rebuilt)?;
        let page = self.tested.output.join(PAGE);
        if let Some(theirs) = is_no_report(&page) {
            return Err(theirs);
        }
        let (results, trouble) = self.read();
        put(&page, page::render(&results).as_bytes())?;
        if let Some(trouble) = trouble {
            return Err(trouble);
        }
        rebuilt.done = rebuilt.begun;
        self.count(&rebuilt)?;
        Ok(began.elapsed())
    }

    /// `None` when what is there makes no sense, or is another version's.
    fn rebuilt(&self) -> Option<Rebuilt> {
        match fs::read(self.tested.output.join(REBUILT)) {
            Ok(json) => serde_json::from_slice(&json)
                .ok()
                .filter(|rebuilt: &Rebuilt| rebuilt.gallery == VERSION)
                .filter(|rebuilt| rebuilt.done <= rebuilt.begun)
                .filter(|rebuilt| rebuilt.begun < u64::MAX),
            Err(e) if e.kind() == ErrorKind::NotFound => Some(Rebuilt::default()),
            Err(_) => None,
        }
    }

    fn count(&self, rebuilt: &Rebuilt) -> Result<(), String> {
        let json = serde_json::to_vec(rebuilt).map_err(|e| e.to_string())?;
        put(&self.tested.output.join(REBUILT), &json)
    }

    /// `lock`'s file, locked by `how` until it is dropped or its process ends;
    /// unlocked on a file system that has no locks.
    ///
    /// # Errors
    /// When whoever has it locked, suspended perhaps, has it for [`PATIENCE`],
    /// or for a little of that once they have been waited for in vain.
    fn locked(
        &self,
        lock: &Lock,
        how: impl Fn(&fs::File) -> Result<(), fs::TryLockError>,
    ) -> Result<fs::File, String> {
        let locked = self.locked_unless(|| false, lock, how)?;
        Ok(locked.expect("BUG: it is needed"))
    }

    /// As [`locked`](Self::locked), until it is `needless`.
    fn locked_unless(
        &self,
        needless: impl Fn() -> bool,
        lock: &Lock,
        how: impl Fn(&fs::File) -> Result<(), fs::TryLockError>,
    ) -> Result<Option<fs::File>, String> {
        let patience = match lock.stuck.load(Ordering::Relaxed) {
            true => PATIENCE / 20,
            false => PATIENCE,
        };
        let path = self.tested.output.join(lock.name);
        let file = self
            .open_lock(lock.name)
            .map_err(|e| format!("open `{path}`: {e}"))?;
        let began = Instant::now();
        loop {
            let is_had = is_had(how(&file));
            if is_had {
                lock.stuck.store(false, Ordering::Relaxed);
            }
            if needless() {
                return Ok(None);
            }
            if is_had {
                return Ok(Some(file));
            }
            if began.elapsed() >= patience {
                lock.stuck.store(true, Ordering::Relaxed);
                return Err(format!("lock `{path}`: another process has it locked"));
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// To be read and written: where locks are emulated with `fcntl`'s, as on NFS,
    /// a shared one takes the former and an exclusive one the latter.
    pub(super) fn open_lock(&self, name: &str) -> std::io::Result<fs::File> {
        let path = self.tested.output.join(name);
        let opened = fs::File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path);
        match opened {
            // Another user's, to be read alone.
            Err(e) if e.kind() == ErrorKind::PermissionDenied => fs::File::open(&path),
            opened => opened,
        }
    }

    /// Every result in the output, of whichever crate and renderer, the stale ones
    /// removed unless another process is writing some; and what kept it from being
    /// every one, if anything did.
    fn read(&self) -> (Vec<Reported>, Option<String>) {
        let output = &self.tested.output;
        let mut trouble = Trouble::default();
        let mut crates = Crates::default();
        let mut results = Vec::new();
        let mut stale = Vec::new();
        let there = trouble.unless(output, inside(output));
        let dirs = there.iter().flat_map(|krate| match inside(krate) {
            // Whatever else is in the output may be no one's to read.
            Err(e) if e.kind() == ErrorKind::PermissionDenied => Vec::new(),
            inner => trouble.unless(krate, inner),
        });
        let dirs: Vec<Utf8PathBuf> = dirs.filter(|dir| holds_results(dir)).collect();
        for dir in dirs {
            let list = trouble.unless(&dir.join(ORDER), List::of(&dir));
            let names = list.iter().flat_map(|list| &list.names);
            let order: HashMap<&String, usize> = names.zip(0..).collect();
            for path in trouble.files(&dir) {
                let Some((kept, body)) = trouble.unless(&path, parsed(&path)) else {
                    sweep(&path);
                    continue;
                };
                let filed = path.strip_prefix(&dir).unwrap_or(&path);
                if list.as_ref().is_some_and(|it| !it.has(&kept.trial, filed)) {
                    stale.extend(self.is_stale(&path).then_some(path));
                } else if crates.is_there(&kept) {
                    results.push(Reported {
                        order: order.get(&kept.trial).copied().unwrap_or(usize::MAX),
                        kept,
                        body,
                    });
                }
            }
        }
        there.iter().for_each(|path| sweep(path));
        if !stale.is_empty()
            && let Ok(writes) = self.open_lock(WRITES)
            && is_had(writes.try_lock())
        {
            for path in stale {
                // Another may have been written there meanwhile.
                if self.is_stale(&path) {
                    _ = fs::remove_file(path.with_extension("json"));
                    _ = fs::remove_file(path);
                }
            }
        }
        (results, trouble.0)
    }

    /// What is at `path` is a result of this catalog's, from before this process,
    /// of a test that is no more or is filed elsewhere now.
    fn is_stale(&self, path: &Utf8Path) -> bool {
        let Ok(filed) = path.strip_prefix(self.results()) else {
            return false;
        };
        let Ok(Some((kept, _))) = parsed(path) else {
            return false;
        };
        kept.at < self.list.began && !self.list.has(&kept.trial, filed)
    }
}

/// The first thing that could not be read.
#[derive(Default)]
struct Trouble(Option<String>);

impl Trouble {
    /// What was `read` at `path`, or nothing, which is no trouble when nothing is there.
    fn unless<T: Default>(&mut self, path: &Utf8Path, read: std::io::Result<T>) -> T {
        read.unwrap_or_else(|e| {
            if !is_absent(&e) {
                self.0.get_or_insert_with(|| format!("read `{path}`: {e}"));
            }
            T::default()
        })
    }

    /// The files in `dir`, at whatever depth.
    fn files(&mut self, dir: &Utf8Path) -> Vec<Utf8PathBuf> {
        let mut files = Vec::new();
        for entry in walkdir::WalkDir::new(dir) {
            match entry {
                Ok(entry) if entry.file_type().is_file() => {
                    files.extend(Utf8PathBuf::from_path_buf(entry.into_path()).ok());
                }
                Ok(_) => {}
                Err(e) => {
                    let path = e.path().and_then(Utf8Path::from_path);
                    let path = path.unwrap_or(dir).to_owned();
                    self.unless(&path, Err::<(), _>(e.into()));
                }
            }
        }
        files
    }
}

fn is_absent(error: &std::io::Error) -> bool {
    matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory)
}

/// A file is locked by now, or is on a file system that has no locks,
/// which each has an error of its own for.
pub(super) fn is_had(locked: Result<(), fs::TryLockError>) -> bool {
    match locked {
        Ok(()) => true,
        Err(fs::TryLockError::WouldBlock) => false,
        Err(fs::TryLockError::Error(e)) => e.kind() != ErrorKind::Interrupted,
    }
}

/// A result of this version's: what it is of, and the part of the page it has.
fn parsed(path: &Utf8Path) -> std::io::Result<Option<(Kept, String)>> {
    if path.extension() != Some(FRAGMENT) {
        return Ok(None);
    }
    let Ok(mut text) = String::from_utf8(fs::read(path)?) else {
        return Ok(None);
    };
    let line = text.find('\n').unwrap_or_default();
    let kept = serde_json::from_str(&text[..line]).ok();
    let kept = kept.filter(|kept: &Kept| kept.gallery == VERSION);
    Ok(kept.map(|kept| {
        text.drain(..=line);
        (kept, text)
    }))
}

/// The names that crates' manifests give them now.
#[derive(Default)]
struct Crates(HashMap<Utf8PathBuf, Named>);

#[derive(Clone, PartialEq, Eq)]
enum Named {
    As(String),
    /// Nothing that can be read, as when it is being edited.
    Unknown,
    Gone,
}

impl Crates {
    /// The crate a result is of has not been renamed or removed.
    fn is_there(&mut self, kept: &Kept) -> bool {
        let named = self
            .0
            .entry(kept.manifest_dir.clone())
            .or_insert_with_key(|dir| named(&dir.join("Cargo.toml")));
        match named {
            Named::As(name) => *name == kept.krate,
            Named::Unknown => true,
            Named::Gone => false,
        }
    }
}

fn named(manifest: &Utf8Path) -> Named {
    #[derive(serde::Deserialize)]
    struct Manifest {
        package: Package,
    }
    #[derive(serde::Deserialize)]
    struct Package {
        name: String,
    }
    match fs::read_to_string(manifest) {
        Ok(text) => match toml::from_str::<Manifest>(&text) {
            Ok(manifest) => Named::As(manifest.package.name.replace('-', "_")),
            Err(_) => Named::Unknown,
        },
        Err(e) if e.kind() == ErrorKind::NotFound => Named::Gone,
        Err(_) => Named::Unknown,
    }
}

/// What is in `dir`, which may be a link to one.
fn inside(dir: &Utf8Path) -> std::io::Result<Vec<Utf8PathBuf>> {
    let entries = match dir.read_dir_utf8() {
        Err(e) if is_absent(&e) => return Ok(Vec::new()),
        entries => entries?,
    };
    let mut inside = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => inside.push(entry.into_path()),
            // It has a name that gallery gives nothing.
            Err(e) if e.kind() == ErrorKind::InvalidData => {}
            Err(e) => return Err(e),
        }
    }
    Ok(inside)
}

fn holds_results(dir: &Utf8Path) -> bool {
    let name = dir.file_name();
    name.is_some_and(|name| is_of_renderer("results-", name, ""))
}

/// `name` is a renderer's, between `start` and `end`.
fn is_of_renderer(start: &str, name: &str, end: &str) -> bool {
    let middle = name.strip_prefix(start);
    let middle = middle.and_then(|name| name.strip_suffix(end));
    let renderers = [Renderer::Wgpu, Renderer::Glow].map(Renderer::name);
    middle.is_some_and(|name| renderers.contains(&name))
}

/// Remove `path` if a process was writing it when it was killed.
fn sweep(path: &Utf8Path) {
    /// Longer than any is being written for.
    const DEAD: Duration = Duration::from_mins(1);
    let is_writing = |name: &str| name.starts_with(WRITING);
    if !path.file_name().is_some_and(is_writing) {
        return;
    }
    let modified = path.metadata().and_then(|file| file.modified());
    if modified.is_ok_and(|at| at.elapsed().is_ok_and(|age| age > DEAD)) {
        _ = fs::remove_file(path);
    }
}

/// Whether `path` is what tests write, to an output that `output` is if it is marked as one.
/// Sources may be there too, so each is known by its own name.
pub(crate) fn is_written(output: &Utf8Path, path: &Utf8Path) -> bool {
    let Ok(below) = path.strip_prefix(output) else {
        return false;
    };
    let parts: Vec<&str> = below.components().map(|part| part.as_str()).collect();
    let named = match &parts[..] {
        [PAGE] => is_report(path).unwrap_or(false),
        [name] => name.starts_with(".gallery-"),
        [_, results, ..] if is_of_renderer("results-", results, "") => true,
        [_, .., name] if is_of_renderer("video.", name, ".mp4") => parts.len() > 2,
        [_, inner @ ..] => inner
            .iter()
            .any(|name| is_of_renderer("snapshots-", name, "")),
        [] => false,
    };
    named && output.join(WRITES).is_file()
}

/// Where the result of each of `trials` goes, but for an extension:
/// a place for each, though two names come out alike as a file's.
pub(super) fn places(trials: &[String]) -> HashMap<String, Utf8PathBuf> {
    let mut places = HashMap::new();
    let mut taken = HashSet::new();
    for trial in trials {
        places.entry(trial.clone()).or_insert_with(|| {
            let filed = filed(trial);
            let numbered = (2..).map(|n| Utf8PathBuf::from(format!("{filed} {n}")));
            std::iter::once(filed.clone())
                .chain(numbered)
                .find(|path| taken.insert(caseless(path)))
                .expect("BUG: numbers do not run out")
        });
    }
    places
}

/// A test's name as a path, its parts as directories.
/// It is the same on every system, so none may forbid it.
pub(super) fn filed(trial: &str) -> Utf8PathBuf {
    /// Of the 255 bytes a file's name can have;
    /// the rest are for a number and an extension.
    const LONGEST: usize = 230;
    let parts = trial.split("::").map(|part| {
        let options = sanitize_filename::Options {
            windows: true,
            truncate: false,
            replacement: "_",
        };
        let part = &part[..part.floor_char_boundary(LONGEST)];
        let part = sanitize_filename::sanitize_with_options(part, options);
        let path = Utf8Path::new(&part);
        let is_anothers = path.extension().is_some_and(|extension| {
            ["json", FRAGMENT].contains(&extension.to_lowercase().as_str())
        });
        match part.is_empty() || is_anothers {
            true => format!("{part}_"),
            false => part,
        }
    });
    parts.collect()
}

/// As a file system that minds no case tells paths apart.
fn caseless(path: &Utf8Path) -> String {
    path.as_str().to_lowercase()
}

/// What is at `page`, if that is something else than a report.
fn is_no_report(page: &Utf8Path) -> Option<String> {
    match is_report(page) {
        Ok(true) => None,
        Ok(false) if page.metadata().is_ok_and(|file| file.len() == 0) => None,
        Ok(false) => Some(format!("`{page}` is there, and is no report of tests")),
        Err(e) if e.kind() == ErrorKind::NotFound => None,
        Err(e) => Some(format!("read `{page}`: {e}")),
    }
}

fn is_report(page: &Utf8Path) -> std::io::Result<bool> {
    let mut start = Vec::new();
    fs::File::open(page)?.take(4_096).read_to_end(&mut start)?;
    Ok(String::from_utf8_lossy(&start).contains(page::TITLE_ENDS))
}

/// Write `bytes` to `path`, moved into place whole.
fn put(path: &Utf8Path, bytes: &[u8]) -> Result<(), String> {
    let whole = || {
        let dir = path.parent().unwrap_or(Utf8Path::new("."));
        let mut file = tempfile::Builder::new()
            .prefix(WRITING)
            .make_in(dir, |path| fs::File::create_new(path))?;
        file.write_all(bytes)?;
        let (file, whole) = file.into_parts();
        drop(file);
        Ok(whole.persist(path)?)
    };
    place_at(path, whole).map_err(|e| format!("write `{path}`: {e}"))
}
