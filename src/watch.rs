//! `--hot`: rebuilding the scenes dylib as its sources change, and saying where that has got to.
//!
//! Gallery watches and builds for itself: a build it started is one it can read as it happens,
//! through `--message-format=json`. `cargo watch` kept the cycle to itself and the terminal.

use std::{
    collections::VecDeque,
    io::{BufRead as _, BufReader, Read},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, RecvTimeoutError},
    },
    thread,
    time::{Duration, Instant},
};

use camino::{Utf8Path, Utf8PathBuf};
use notify::{EventKind, RecursiveMode, Watcher as _, event::CreateKind};
use process_wrap::std::{ChildWrapper, CommandWrap};

/// Where the rebuild cycle has got to.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum HotPhase {
    /// Nothing in flight.
    Watching,
    /// An edit landed; the quiet before a build has not run down.
    Changed,
    /// Cargo is building; the chip counts from `since`.
    Building { since: Instant },
    /// The build came out; the dylib it wrote is not mapped yet.
    Swapping { since: Instant },
    /// What is on screen is the edit's.
    Reloaded { at: Instant, took: Duration },
    /// The build failed, and everything it said.
    Failed(Arc<BuildFailure>),
    /// Nothing is watching any more, and why.
    Stopped { why: String },
}

/// A failed build: everything it said, and how many errors that came to.
#[derive(Debug, PartialEq)]
pub(crate) struct BuildFailure {
    /// Every message cargo rendered, escapes stripped.
    pub(crate) messages: Vec<BuildMessage>,
    /// Errors with a site of their own; zero where nothing rendered a diagnostic,
    /// as an unparseable manifest does — which is why the bar reads the count rather than states it.
    pub(crate) errors: usize,
}

/// One message rustc rendered, as the window shows it.
#[derive(Debug, PartialEq)]
pub(crate) struct BuildMessage {
    pub(crate) level: MessageLevel,
    pub(crate) text: String,
}

/// What a message weighs; anything rustc says that is neither is a note.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum MessageLevel {
    Error,
    Warning,
    Note,
}

/// The rebuild cycle as the shell reads it: one handle, cloned to each side that reports into it.
#[derive(Clone)]
pub(crate) struct HotStatus(Arc<Mutex<HotState>>);

/// What the handle shares: the phase, and the window to wake when it moves.
struct HotState {
    phase: HotPhase,
    /// Taken from the first frame, so the watcher's thread can wake a window at rest.
    ctx: Option<egui::Context>,
}

/// How long a finished cycle stays on the chip.
const LINGER: Duration = Duration::from_secs(4);

/// How long to wait for a swap. Only a build that compiled something gets here, and it can still
/// come to the same bytes — which the reloader hashes, recognises, and does not reload.
const SWAP_WAIT: Duration = Duration::from_millis(1_500);

/// The quiet before a build: an editor writes a file in several steps, and a save-all touches many.
const QUIET: Duration = Duration::from_millis(500);

/// Bounds how long the watcher can remain asleep after the UI begins an orderly shutdown.
const STOP_POLL: Duration = Duration::from_millis(50);

/// Lines of cargo's own output kept back, for a failure that renders no diagnostics.
const TAIL_LINES: usize = 20;

impl HotStatus {
    pub(crate) fn new() -> Self {
        Self(Arc::new(Mutex::new(HotState {
            phase: HotPhase::Watching,
            ctx: None,
        })))
    }

    /// The phase to draw.
    pub(crate) fn phase(&self) -> HotPhase {
        self.0.lock().expect("the hot phase").phase.clone()
    }

    /// Move to `phase` and wake the window, which is how a build starting shows up.
    fn set(&self, phase: HotPhase) {
        let mut state = self.0.lock().expect("the hot phase");
        state.phase = phase;
        if let Some(ctx) = &state.ctx {
            ctx.request_repaint();
        }
    }

    /// Hand over the window to wake. Called every frame; the first lands.
    pub(crate) fn wake_with(&self, ctx: &egui::Context) {
        let mut state = self.0.lock().expect("the hot phase");
        if state.ctx.is_none() {
            state.ctx = Some(ctx.clone());
        }
    }

    /// The dylib was swapped in — the scenes on screen are the ones just built.
    pub(crate) fn swapped(&self) {
        let mut state = self.0.lock().expect("the hot phase");
        let took = match state.phase {
            HotPhase::Swapping { since } => since.elapsed(),
            // A bare `cargo build` elsewhere rebuilt it; it reloaded all the same.
            _ => Duration::ZERO,
        };
        state.phase = HotPhase::Reloaded {
            at: Instant::now(),
            took,
        };
    }

    /// Let a finished cycle lapse back to watching, and give up on a swap that is not coming.
    pub(crate) fn settle(&self) {
        let mut state = self.0.lock().expect("the hot phase");
        let lapsed = match state.phase {
            HotPhase::Reloaded { at, .. } => at.elapsed() >= LINGER,
            HotPhase::Swapping { since } => since.elapsed() >= SWAP_WAIT,
            _ => false,
        };
        if lapsed {
            state.phase = HotPhase::Watching;
        }
    }

    /// Whether the phase moves on its own, and so has to be redrawn.
    pub(crate) fn is_moving(&self) -> bool {
        matches!(
            self.phase(),
            HotPhase::Changed
                | HotPhase::Building { .. }
                | HotPhase::Swapping { .. }
                | HotPhase::Reloaded { .. }
        )
    }
}

#[cfg(any(test, feature = "shell-scenes"))]
impl HotStatus {
    /// A cycle held at `phase` — for a test or a shell scene, neither of which has a cargo.
    pub(crate) fn posed(phase: HotPhase) -> Self {
        let hot = Self::new();
        hot.set(phase);
        hot
    }
}

#[cfg(test)]
impl HotStatus {
    /// Stopped on a failed build of `errors`, with `said` as the whole of what it said.
    pub(crate) fn failing(errors: usize, said: &str) -> Self {
        Self::posed(HotPhase::Failed(Arc::new(BuildFailure {
            messages: vec![BuildMessage {
                level: MessageLevel::Error,
                text: said.to_owned(),
            }],
            errors,
        })))
    }
}

/// How to start a rebuild: a [`Command`] cannot be run twice, so each build makes a fresh one.
pub(crate) type RebuildCommand = Box<dyn Fn() -> Command + Send>;

/// The build in flight, so that closing the window takes it down too.
type BuildInFlight = Arc<Mutex<Option<Box<dyn ChildWrapper>>>>;

/// A running watcher: the thread that builds, and the build it is running.
#[derive(Clone)]
pub(crate) struct SceneWatcher {
    building: BuildInFlight,
    stopped: Arc<AtomicBool>,
    thread: Arc<Mutex<Option<thread::JoinHandle<()>>>>,
}

/// Watch `paths` for edits and rebuild the dylib as they land, reporting the cycle into `hot`.
pub(crate) fn spawn(
    paths: Vec<Utf8PathBuf>,
    rebuild: RebuildCommand,
    hot: &HotStatus,
) -> SceneWatcher {
    let watcher = SceneWatcher {
        building: BuildInFlight::default(),
        stopped: Arc::new(AtomicBool::new(false)),
        thread: Arc::new(Mutex::new(None)),
    };
    let running = watcher.clone();
    let reporting = hot.clone();
    let handle = thread::spawn(move || {
        if let Err(why) = watch(&paths, &rebuild, &reporting, &running) {
            reporting.set(HotPhase::Stopped { why });
        }
    });
    *watcher.thread.lock().expect("the watcher thread") = Some(handle);
    watcher
}

impl SceneWatcher {
    /// Stop watching and take down any build in flight without waiting for its thread.
    pub(crate) fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(building) = self.building.lock().expect("the build in flight").as_mut() {
            let _ = building.kill();
        }
    }

    /// Stop all watcher work and wait until neither it nor a cargo child can touch the hot dylib.
    pub(crate) fn stop_and_join(&self) {
        self.stop();
        let handle = self.thread.lock().expect("the watcher thread").take();
        if let Some(handle) = handle {
            let _ = handle.join();
        }
    }

    fn going(&self) -> bool {
        !self.stopped.load(Ordering::Acquire)
    }
}

/// Set the watches up, then run the loop over what they report.
fn watch(
    paths: &[Utf8PathBuf],
    rebuild: &RebuildCommand,
    hot: &HotStatus,
    watcher: &SceneWatcher,
) -> Result<(), String> {
    let (tx, rx) = mpsc::channel();
    let mut notify =
        notify::recommended_watcher(tx).map_err(|e| format!("no file watcher: {e}"))?;
    let roots = watch_roots(paths);
    for root in &roots {
        notify
            .watch(root.dir.as_std_path(), root.mode)
            .map_err(|e| format!("cannot watch `{}`: {e}", root.dir))?;
    }
    WatchLoop {
        events: &rx,
        notify: &mut notify,
        roots: &roots,
        watcher,
        hot,
        rebuild: &mut || build(rebuild, hot, watcher),
        quiet: QUIET,
    }
    .run();
    Ok(())
}

/// What the loop runs against. `rebuild` and `quiet` are handed in so a test can drive the loop
/// from a channel it fills by hand, rather than through a real kernel and a real cargo.
struct WatchLoop<'a, W: notify::Watcher> {
    events: &'a mpsc::Receiver<notify::Result<notify::Event>>,
    notify: &'a mut W,
    roots: &'a [WatchRoot],
    watcher: &'a SceneWatcher,
    hot: &'a HotStatus,
    rebuild: &'a mut dyn FnMut(),
    quiet: Duration,
}

impl<W: notify::Watcher> WatchLoop<'_, W> {
    /// Wait for an edit, wait for the edits to stop, rebuild, and go round again.
    ///
    /// What the build writes comes back through the same channel and is dropped there, so an edit
    /// landing mid-build is the next thing waiting — never a restart, never a killed build.
    fn run(&mut self) {
        while self.watcher.going() {
            // A watcher whose sender is gone has nothing left to say.
            let event = loop {
                match self.events.recv_timeout(STOP_POLL) {
                    Ok(event) => break event,
                    Err(RecvTimeoutError::Timeout) if self.watcher.going() => {}
                    Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => return,
                }
            };
            let mut edited = edit_landed(event, self.roots, self.notify);

            let mut quiet = Instant::now() + self.quiet;
            while let Some(wait) = quiet.checked_duration_since(Instant::now()) {
                if edited {
                    self.hot.set(HotPhase::Changed);
                }
                match self.events.recv_timeout(wait.min(STOP_POLL)) {
                    Ok(event) => {
                        if edit_landed(event, self.roots, self.notify) {
                            edited = true;
                            quiet = Instant::now() + self.quiet;
                        }
                    }
                    Err(RecvTimeoutError::Timeout) if !self.watcher.going() => return,
                    Err(RecvTimeoutError::Timeout) if Instant::now() >= quiet => break,
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }

            if edited && self.watcher.going() {
                (self.rebuild)();
            }
        }
    }
}

/// Whether one event is an edit to rebuild for, and where a directory that has just appeared under
/// a root starts being watched — [`watch_roots`] watches a root shallowly, so edits inside a new
/// one would otherwise go unheard.
fn edit_landed(
    event: notify::Result<notify::Event>,
    roots: &[WatchRoot],
    notify: &mut impl notify::Watcher,
) -> bool {
    // An overflowed queue arrives as an error; rebuilding on one would answer a flood of events
    // with the build that caused it.
    let event = match event {
        Ok(event) => event,
        Err(e) => {
            eprintln!("gallery: file watcher: {e}");
            return false;
        }
    };

    // Access events describe opens, reads and closes, not filesystem mutations.
    // Cargo opens the manifest and lockfile during every rebuild, so treating
    // those reads as edits makes each build trigger another one.
    if !matches!(
        event.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    ) {
        return false;
    }
    let edits: Vec<&Utf8Path> = event
        .paths
        .iter()
        // Everything gallery builds from is named in UTF-8, so one
        // that cannot be spelled is not one to rebuild for.
        .filter_map(|path| Utf8Path::from_path(path))
        .filter(|path| roots.iter().any(|root| root.includes(path)))
        .filter(|path| rebuilds_for(path))
        .collect();
    if matches!(
        event.kind,
        EventKind::Create(CreateKind::Folder | CreateKind::Any)
    ) {
        for edit in &edits {
            let under_a_root = roots
                .iter()
                .any(|root| root.only.is_none() && Some(root.dir.as_path()) == edit.parent());
            if under_a_root && edit.is_dir() {
                let _ = notify.watch(edit.as_std_path(), RecursiveMode::Recursive);
            }
        }
    }
    !edits.is_empty()
}

/// A directory to watch, and whether what lies under it is watched with it.
struct WatchRoot {
    dir: Utf8PathBuf,
    mode: RecursiveMode,
    only: Option<Utf8PathBuf>,
}

impl WatchRoot {
    fn includes(&self, path: &Utf8Path) -> bool {
        if let Some(only) = &self.only {
            return path == only;
        }
        path == self.dir
            || match self.mode {
                RecursiveMode::Recursive => path.starts_with(&self.dir),
                RecursiveMode::NonRecursive => path.parent() == Some(self.dir.as_path()),
            }
    }
}

/// Watching a crate root recursively takes its `target/` with it — a watch descriptor per
/// directory and an event per file a build writes. So a root is watched by itself, its children
/// recursively, minus any holding a build or a repository. One further down still costs churn,
/// its events dropped by [`rebuilds_for`] rather than never arriving.
///
/// Files are watched through their parent so atomic replacements remain observable.
fn watch_roots(paths: &[Utf8PathBuf]) -> Vec<WatchRoot> {
    let mut roots = Vec::new();
    for path in paths {
        if path.is_file() {
            let Some(parent) = path.parent() else {
                continue;
            };
            roots.push(WatchRoot {
                dir: parent.to_owned(),
                mode: RecursiveMode::NonRecursive,
                only: Some(path.clone()),
            });
            continue;
        }
        let dir = path.clone();
        let Ok(entries) = dir.read_dir_utf8() else {
            // Unreadable, or not there at all — a glob can name a directory that does not exist yet.
            continue;
        };
        for entry in entries.flatten() {
            let child = entry.into_path();
            if child.is_dir() && rebuilds_for(&child) {
                roots.push(WatchRoot {
                    dir: child,
                    mode: RecursiveMode::Recursive,
                    only: None,
                });
            }
        }
        roots.push(WatchRoot {
            dir,
            mode: RecursiveMode::NonRecursive,
            only: None,
        });
    }
    roots
}

/// Whether a changed path is one to rebuild for.
///
/// Out: what cargo wrote (a directory the build writes into is one it would rebuild for ever),
/// a repository's bookkeeping, which `git status` alone stirs,
/// and what an editor leaves beside the file it is saving.
///
/// Everything else rebuilds, `.rs` or not: a shader or an `include_bytes!` asset is a source too,
/// a spare build costs a moment, and an edit that silently does nothing is what this exists to end.
fn rebuilds_for(path: &Utf8Path) -> bool {
    !gallery_build::build_output(path) && !repository(path) && !editor_debris(path)
}

/// Whether the path is inside a version control system's own storage.
fn repository(path: &Utf8Path) -> bool {
    path.components()
        .any(|part| matches!(part.as_str(), ".git" | ".hg" | ".jj" | ".svn"))
}

/// Whether the file is what an editor writes beside the one being saved.
fn editor_debris(path: &Utf8Path) -> bool {
    let Some(name) = path.file_name() else {
        return false;
    };
    name.ends_with('~')
        || name.starts_with('#')
        || name.starts_with(".#")
        || name.contains("___jb_")
        || matches!(path.extension(), Some("swp" | "swx" | "swo" | "tmp"))
}

/// Run one build, reporting what it comes to into `hot`.
fn build(rebuild: &RebuildCommand, hot: &HotStatus, watcher: &SceneWatcher) {
    hot.set(HotPhase::Building {
        since: Instant::now(),
    });

    let mut command = rebuild();
    // Records on stdout, each diagnostic pre-rendered with its escapes: the terminal keeps
    // the output it always had, the window takes the same text through `style::plain`.
    // Colour is asked for outright, cargo dropping it for a pipe and `anstream` stripping it back.
    command.args([
        "--message-format=json-diagnostic-rendered-ansi",
        "--color=always",
    ]);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut wrapped = CommandWrap::from(command);
    #[cfg(unix)]
    wrapped.wrap(process_wrap::std::ProcessGroup::leader());
    #[cfg(windows)]
    wrapped.wrap(process_wrap::std::JobObject);

    let mut child = match wrapped.spawn() {
        Ok(child) => child,
        Err(e) => {
            hot.set(HotPhase::Stopped {
                why: format!("cargo did not start: {e}"),
            });
            return;
        }
    };
    let stdout = child.stdout().take();
    let stderr = child.stderr().take();
    *watcher.building.lock().expect("the build in flight") = Some(child);

    // Cargo writes to both, and a pipe nobody is reading fills up and blocks it.
    let tail = stderr.map(|stderr| thread::spawn(move || forward_output(stderr)));
    let report = stdout.map(BuildReport::read).unwrap_or_default();
    let tail = tail.and_then(|pump| pump.join().ok()).unwrap_or_default();

    // Taken out before it is waited on: holding the lock across the wait leaves
    // [`SceneWatcher::stop`] blocking on the child it is killing, and hangs the window on close.
    // A statement of its own, an `if let` scrutinee's guard living to the end of its body
    // — the shape `stopping_does_not_wait_on_a_build_that_has_gone_quiet` catches.
    let child = watcher.building.lock().expect("the build in flight").take();
    if let Some(mut child) = child {
        let _ = child.wait();
    }

    // A build stopped on the way out is not a failure to report, and the window is going anyway.
    if watcher.going() {
        hot.set(report.came_to(tail));
    }
}

/// What a build said, gathered as it says it.
#[derive(Default, Debug, PartialEq)]
struct BuildReport {
    /// Every message rustc rendered, escapes stripped.
    messages: Vec<BuildMessage>,
    /// How many of them are errors with a site of their own.
    errors: usize,
    /// Whether anything was actually compiled. Cargo marks what it did not rebuild as fresh,
    /// and a build of nothing but those leaves the dylib untouched.
    compiled: bool,
    outcome: Option<BuildOutcome>,
}

/// How cargo said the build ended.
#[derive(Clone, Copy, Debug, PartialEq)]
enum BuildOutcome {
    Built,
    Failed,
}

/// One line of cargo's stream. Nothing else it emits is read.
#[derive(serde::Deserialize)]
#[serde(tag = "reason")]
enum CargoRecord {
    #[serde(rename = "compiler-message")]
    CompilerMessage { message: CargoMessage },
    #[serde(rename = "compiler-artifact")]
    CompilerArtifact { fresh: bool },
    #[serde(rename = "build-finished")]
    BuildFinished { success: bool },
    #[serde(other)]
    Other,
}

/// One rustc message as cargo's record carries it, as much of it as is read.
#[derive(serde::Deserialize)]
struct CargoMessage {
    level: String,
    /// Absent for a message rustc did not render, which is nothing to show.
    rendered: Option<String>,
    /// A summary — "aborting due to 2 previous errors" — carries neither, and is no error to count.
    code: Option<serde::de::IgnoredAny>,
    #[serde(default)]
    spans: Vec<serde::de::IgnoredAny>,
}

impl BuildReport {
    /// Read cargo's records to the end, putting each rendered message on the terminal as it lands.
    fn read(stream: impl Read) -> Self {
        let mut report = Self::default();
        for line in BufReader::new(stream).lines().map_while(Result::ok) {
            if let Some(rendered) = report.take(&line) {
                anstream::eprint!("{rendered}");
            }
        }
        report
    }

    /// Take one record, and hand back what the terminal should show for it.
    fn take(&mut self, line: &str) -> Option<String> {
        // Cargo's stream is one record per line; a line that is not one is not ours to show.
        let record = serde_json::from_str::<CargoRecord>(line).ok()?;
        match record {
            CargoRecord::CompilerArtifact { fresh } => {
                self.compiled |= !fresh;
                None
            }
            CargoRecord::BuildFinished { success } => {
                self.outcome = Some(match success {
                    true => BuildOutcome::Built,
                    false => BuildOutcome::Failed,
                });
                None
            }
            CargoRecord::CompilerMessage { message } => {
                let rendered = message.rendered?;
                let level = match message.level.as_str() {
                    "error" => MessageLevel::Error,
                    "warning" => MessageLevel::Warning,
                    _ => MessageLevel::Note,
                };
                // A summary restates a count rather than being another place to look.
                if level == MessageLevel::Error
                    && (message.code.is_some() || !message.spans.is_empty())
                {
                    self.errors += 1;
                }
                self.messages.push(BuildMessage {
                    level,
                    text: crate::style::plain(&rendered),
                });
                Some(rendered)
            }
            CargoRecord::Other => None,
        }
    }

    /// The phase the build leaves behind. `tail` is cargo's own last words, all there is to show
    /// when it failed before compiling anything — an unparseable manifest.
    fn came_to(self, tail: String) -> HotPhase {
        if self.outcome == Some(BuildOutcome::Built) {
            // Nothing compiled means no dylib written and no reload coming; saying otherwise
            // accounts for a rebuild that did not happen, which a directory of generated files
            // would produce all day.
            return match self.compiled {
                true => HotPhase::Swapping {
                    since: Instant::now(),
                },
                false => HotPhase::Watching,
            };
        }
        let mut messages = self.messages;
        if messages.is_empty() {
            messages.push(BuildMessage {
                level: MessageLevel::Error,
                text: tail,
            });
        }
        HotPhase::Failed(Arc::new(BuildFailure {
            messages,
            errors: self.errors,
        }))
    }
}

/// Pass cargo's own output through as it arrives, keeping the last lines in case they are all
/// it said.
fn forward_output(stream: impl Read) -> String {
    let mut tail = VecDeque::with_capacity(TAIL_LINES);
    for line in BufReader::new(stream).lines().map_while(Result::ok) {
        anstream::eprintln!("{line}");
        if tail.len() == TAIL_LINES {
            tail.pop_front();
        }
        tail.push_back(crate::style::plain(&line));
    }
    Vec::from(tail).join("\n")
}

/// Gold while something is happening, green for a cycle that came out, red for one that did not.
const BUSY: egui::Color32 = egui::Color32::from_rgb(0xC8, 0x9B, 0x3C);
const DONE: egui::Color32 = egui::Color32::from_rgb(0x6C, 0xC8, 0x7A);
const BROKEN: egui::Color32 = egui::Color32::from_rgb(0xE0, 0x6C, 0x6C);

/// What the failure bar is filled with: dark enough not to glow, loud enough not to read as chrome.
const BAR_BG: egui::Color32 = egui::Color32::from_rgb(0x4A, 0x1D, 0x1D);

/// How a phase reads on the chip.
struct HotChip {
    text: String,
    colour: egui::Color32,
    hover: String,
}

/// How far the chip sits off the corner it is hung in.
const INSET: f32 = 8.0;

/// The room around the chip's own words.
const PAD: egui::Vec2 = egui::vec2(6.0, 3.0);

/// The box the mark is drawn in, and the gap between it and the text.
const MARK: f32 = 9.0;
const GAP: f32 = 5.0;

/// The chip: where the cycle has got to, in a word.
///
/// Painted into the bottom-left of `over` rather than laid out. It sits in chrome, so it covers
/// no scene; it is painted because its width changes as it counts, which would shift any control
/// beside it; and its mark is drawn because a typed one comes from whichever fallback face has
/// the glyph, at that face's weight and baseline.
pub(crate) fn render_hot_chip(ui: &egui::Ui, over: egui::Rect, hot: &HotStatus) {
    let phase = hot.phase();
    let chip = hot_chip(&phase);
    let painter = ui.painter();
    let words = painter.layout_no_wrap(
        chip.text.clone(),
        egui::TextStyle::Body.resolve(ui.style()),
        chip.colour,
    );

    let inner = egui::vec2(MARK + GAP + words.size().x, words.size().y.max(MARK));
    let box_size = inner + PAD * 2.0;
    let chip_box = egui::Rect::from_min_size(
        egui::pos2(over.left() + INSET, over.bottom() - INSET - box_size.y),
        box_size,
    );
    // The tree can run under it, so the words need something of their own to sit on.
    painter.rect_filled(chip_box, 0.0, crate::PANEL_BG);

    let mark = egui::pos2(chip_box.left() + PAD.x + MARK / 2.0, chip_box.center().y);
    paint_mark(painter, mark, &phase, chip.colour);
    painter.galley(
        egui::pos2(
            mark.x + MARK / 2.0 + GAP,
            chip_box.center().y - words.size().y / 2.0,
        ),
        words,
        chip.colour,
    );

    // Named here because it is painted, not added: without this a reader — and a test — has only
    // pixels. The id comes off the `Ui` so several can be drawn at once, as a shell scene does.
    let hovered = ui.interact(chip_box, ui.id().with("hot-chip"), egui::Sense::hover());
    hovered.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, &chip.text));
    hovered.on_hover_text(chip.hover);
}

/// The mark beside the words: a dot at rest, a breathing one at work, a tick or a cross at the end.
fn paint_mark(painter: &egui::Painter, at: egui::Pos2, phase: &HotPhase, colour: egui::Color32) {
    let radius = MARK / 2.0;
    let stroke = |colour| egui::Stroke::new(1.6, colour);
    let line = |from: egui::Vec2, to: egui::Vec2, colour| {
        painter.line_segment([at + from * radius, at + to * radius], stroke(colour));
    };
    match phase {
        HotPhase::Watching => {
            painter.circle_filled(at, radius * 0.5, colour);
        }
        HotPhase::Changed | HotPhase::Building { .. } | HotPhase::Swapping { .. } => {
            // Breathing, because a build that is under way should not look like one at rest.
            let breath = 0.45 + 0.55 * (painter.ctx().input(|i| i.time) as f32 * 4.0).sin().abs();
            painter.circle_filled(at, radius * 0.8, colour.gamma_multiply(breath));
        }
        HotPhase::Reloaded { .. } => {
            line(egui::vec2(-0.85, 0.05), egui::vec2(-0.2, 0.6), colour);
            line(egui::vec2(-0.2, 0.6), egui::vec2(0.85, -0.65), colour);
        }
        HotPhase::Failed(_) | HotPhase::Stopped { .. } => {
            line(egui::vec2(-0.7, -0.7), egui::vec2(0.7, 0.7), colour);
            line(egui::vec2(-0.7, 0.7), egui::vec2(0.7, -0.7), colour);
        }
    }
}

fn hot_chip(phase: &HotPhase) -> HotChip {
    let (text, colour, hover) = match phase {
        HotPhase::Watching => (
            "Watching".to_owned(),
            crate::MUTED,
            "Editing a scene or what it draws rebuilds it".to_owned(),
        ),
        HotPhase::Changed => (
            "Changed".to_owned(),
            BUSY,
            "An edit landed; building once it stops".to_owned(),
        ),
        HotPhase::Building { since } => (
            format!("Building {:.1}s", since.elapsed().as_secs_f32()),
            BUSY,
            "Cargo is rebuilding the scenes".to_owned(),
        ),
        HotPhase::Swapping { .. } => (
            "Swapping".to_owned(),
            BUSY,
            "Built; loading it over the scenes on screen".to_owned(),
        ),
        HotPhase::Reloaded { took, .. } => (
            format!("Reloaded · {:.1}s", took.as_secs_f32()),
            DONE,
            "What is on screen is the edit's".to_owned(),
        ),
        HotPhase::Failed(_) => (
            "Build failed".to_owned(),
            BROKEN,
            "The scenes on screen are the last ones that built".to_owned(),
        ),
        HotPhase::Stopped { why } => ("Not watching".to_owned(), BROKEN, why.clone()),
    };
    HotChip {
        text,
        colour,
        hover,
    }
}

/// The bar over the canvas while a build is failing, and the report a click on it opens.
/// `open` is the shell's, so the report survives a frame; a build that comes out shuts it.
pub(crate) fn render_build_bar(ui: &mut egui::Ui, hot: &HotStatus, open: &mut bool) {
    let HotPhase::Failed(failure) = hot.phase() else {
        *open = false;
        return;
    };

    let bar = egui::Frame::NONE
        .fill(BAR_BG)
        .inner_margin(egui::Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(headline(&failure))
                        .color(BROKEN)
                        .strong(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new("Click for what cargo said").color(crate::MUTED));
                });
            });
        })
        .response
        .interact(egui::Sense::click());
    if bar.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if bar.clicked() {
        *open = !*open;
    }

    if *open {
        let report = egui::Modal::new(egui::Id::new("gallery-build-report")).show(ui.ctx(), |ui| {
            // The modal's bound, not the report's, so a scene can pose one at any size.
            ui.set_max_size(ui.ctx().content_rect().size() * 0.8);
            render_build_report(ui, &failure);
        });
        if report.should_close() {
            *open = false;
        }
    }
}

/// What the bar says: the count where the messages carry one, and the fact where they do not.
fn headline(failure: &BuildFailure) -> String {
    match failure.errors {
        0 => "Build failed".to_owned(),
        1 => "Build failed — 1 error".to_owned(),
        errors => format!("Build failed — {errors} errors"),
    }
}

/// The whole of what cargo said, each message in the colour of what it weighs.
/// Titled by what the bar promised rather than restating it — the bar is right behind, dimmed.
pub(crate) fn render_build_report(ui: &mut egui::Ui, failure: &BuildFailure) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("What cargo said")
                .color(BROKEN)
                .strong(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Close").clicked() {
                ui.close();
            }
        });
    });
    ui.separator();
    egui::ScrollArea::both().show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        for message in &failure.messages {
            let colour = match message.level {
                MessageLevel::Error => BROKEN,
                MessageLevel::Warning => BUSY,
                MessageLevel::Note => ui.visuals().text_color(),
            };
            ui.label(
                egui::RichText::new(message.text.trim_end())
                    .monospace()
                    .color(colour),
            );
        }
    });
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    /// A scratch tree of `files`, as `gallery-build`'s own tests build one.
    fn tree(name: &str, files: &[&str]) -> Utf8PathBuf {
        let root = Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .expect("a UTF-8 temp dir")
            .join(format!("gallery-watch-{name}"));
        let _ = fs::remove_dir_all(&root);
        for file in files {
            let path = root.join(file);
            fs::create_dir_all(path.parent().expect("a parent")).expect("scratch dir");
            fs::write(&path, "// source").expect("write file");
        }
        root
    }

    #[test]
    fn what_the_build_and_the_repository_write_is_not_an_edit_to_rebuild_for() {
        let root = tree(
            "filter",
            &[
                "src/button.rs",
                "shaders/tint.wgsl",
                "target/CACHEDIR.TAG",
                "target/debug/libapp.so",
                ".git/index",
            ],
        );
        for source in ["src/button.rs", "shaders/tint.wgsl"] {
            assert!(
                rebuilds_for(&root.join(source)),
                "`{source}` is a source, whatever its extension"
            );
        }
        for written in ["target/debug/libapp.so", ".git/index"] {
            assert!(
                !rebuilds_for(&root.join(written)),
                "`{written}` is written by the build or the repository, and rebuilding for it \
                 would rebuild for itself"
            );
        }
    }

    #[test]
    fn what_an_editor_leaves_beside_a_file_is_not_the_file() {
        for left in [
            "src/button.rs~",
            "src/.#button.rs",
            "src/#button.rs#",
            "src/.button.rs.swp",
            "src/button.rs___jb_tmp___",
        ] {
            assert!(!rebuilds_for(Utf8Path::new(left)), "`{left}` is debris");
        }
        assert!(rebuilds_for(Utf8Path::new("src/button.rs")), "the file is");
    }

    #[test]
    fn a_root_is_watched_by_itself_and_its_children_deeply_minus_the_build_directory() {
        let root = tree(
            "roots",
            &[
                "gallery.toml",
                "src/button.rs",
                "target/CACHEDIR.TAG",
                ".git/HEAD",
            ],
        );
        let watched = watch_roots(std::slice::from_ref(&root));
        let at = |dir: &Utf8Path| {
            watched
                .iter()
                .find(|root| root.dir == dir)
                .map(|root| root.mode)
        };

        assert_eq!(
            at(&root),
            Some(RecursiveMode::NonRecursive),
            "the root itself, so a file sitting in it is watched without its `target/` coming too"
        );
        assert_eq!(
            at(&root.join("src")),
            Some(RecursiveMode::Recursive),
            "and everything under a source directory"
        );
        for skipped in ["target", ".git"] {
            assert_eq!(at(&root.join(skipped)), None, "`{skipped}` is not watched");
        }

        let file = root.join("gallery.toml");
        let watched = watch_roots(std::slice::from_ref(&file));
        assert_eq!(watched.len(), 1);
        assert_eq!(
            watched[0].dir, root,
            "an explicitly enrolled file watches its parent so replacing the file keeps working"
        );
        assert_eq!(watched[0].mode, RecursiveMode::NonRecursive);
        assert_eq!(watched[0].only.as_ref(), Some(&file));
    }

    /// A watcher that records what it was asked to watch,
    /// so the loop can be driven without a kernel to raise events.
    #[derive(Default)]
    struct Recording(Vec<Utf8PathBuf>);

    impl notify::Watcher for Recording {
        fn new<F: notify::EventHandler>(_: F, _: notify::Config) -> notify::Result<Self> {
            Ok(Self::default())
        }

        fn watch(&mut self, path: &std::path::Path, _: RecursiveMode) -> notify::Result<()> {
            self.0
                .push(Utf8PathBuf::from_path_buf(path.to_owned()).expect("a UTF-8 path"));
            Ok(())
        }

        fn unwatch(&mut self, _: &std::path::Path) -> notify::Result<()> {
            Ok(())
        }

        fn kind() -> notify::WatcherKind {
            notify::WatcherKind::NullWatcher
        }
    }

    /// A watcher with nothing building, as one is between rebuilds.
    fn idle() -> SceneWatcher {
        SceneWatcher {
            building: BuildInFlight::default(),
            stopped: Arc::new(AtomicBool::new(false)),
            thread: Arc::new(Mutex::new(None)),
        }
    }

    fn event(kind: EventKind, path: &Utf8Path) -> notify::Result<notify::Event> {
        Ok(notify::Event::new(kind).add_path(path.as_std_path().to_owned()))
    }

    fn edited(path: &Utf8Path) -> notify::Result<notify::Event> {
        event(
            EventKind::Modify(notify::event::ModifyKind::Data(
                notify::event::DataChange::Content,
            )),
            path,
        )
    }

    fn created(path: &Utf8Path) -> notify::Result<notify::Event> {
        event(EventKind::Create(CreateKind::Folder), path)
    }

    #[test]
    fn an_explicit_file_rebuilds_without_enrolling_its_siblings() {
        let root = tree("explicit-file", &["palette.json", "notes.txt"]);
        let palette = root.join("palette.json");
        let roots = watch_roots(std::slice::from_ref(&palette));
        let mut notify = Recording::default();

        assert!(edit_landed(edited(&palette), &roots, &mut notify));
        assert!(!edit_landed(
            edited(&root.join("notes.txt")),
            &roots,
            &mut notify
        ));

        let replacement = notify::Event::new(EventKind::Modify(notify::event::ModifyKind::Name(
            notify::event::RenameMode::Both,
        )))
        .add_path(root.join("palette.json.tmp").into_std_path_buf())
        .add_path(palette.into_std_path_buf());
        assert!(
            edit_landed(Ok(replacement), &roots, &mut notify),
            "atomically replacing the enrolled file rebuilds while its parent remains watched"
        );
    }

    #[test]
    fn only_filesystem_mutations_are_edits() {
        let root = tree(
            "landed",
            &[
                "src/button.rs",
                "src/switch.rs",
                "target/CACHEDIR.TAG",
                "target/debug/libapp.so",
            ],
        );
        let roots = watch_roots(std::slice::from_ref(&root));
        let mut notify = Recording::default();
        let source = root.join("src/button.rs");

        for kind in [
            EventKind::Create(CreateKind::File),
            EventKind::Modify(notify::event::ModifyKind::Data(
                notify::event::DataChange::Content,
            )),
            EventKind::Remove(notify::event::RemoveKind::File),
        ] {
            assert!(
                edit_landed(event(kind, &source), &roots, &mut notify),
                "{kind:?} changes a source"
            );
        }

        let renamed = notify::Event::new(EventKind::Modify(notify::event::ModifyKind::Name(
            notify::event::RenameMode::Both,
        )))
        .add_path(source.as_std_path().to_owned())
        .add_path(root.join("src/switch.rs").into_std_path_buf());
        assert!(
            edit_landed(Ok(renamed), &roots, &mut notify),
            "renaming a source changes the source tree"
        );

        for kind in [
            EventKind::Access(notify::event::AccessKind::Open(
                notify::event::AccessMode::Any,
            )),
            EventKind::Access(notify::event::AccessKind::Read),
            EventKind::Any,
            EventKind::Other,
        ] {
            assert!(
                !edit_landed(event(kind, &source), &roots, &mut notify),
                "{kind:?} does not describe a filesystem mutation"
            );
        }

        assert!(
            !edit_landed(
                edited(&root.join("target/debug/libapp.so")),
                &roots,
                &mut notify
            ),
            "the build's own writing is not an edit, or it would rebuild for itself forever"
        );
    }

    #[test]
    fn a_watcher_error_is_reported_rather_than_answered_with_a_build() {
        // An overflowed queue arrives as one of these, and it arrives because a great many files
        // moved at once — the last thing to answer that with is the build that caused it.
        let overflowed = Err(notify::Error::new(notify::ErrorKind::MaxFilesWatch));

        assert!(!edit_landed(overflowed, &[], &mut Recording::default()));
    }

    #[test]
    fn a_directory_appearing_beside_the_sources_starts_being_watched() {
        let root = tree("appeared", &["src/button.rs"]);
        let roots = watch_roots(std::slice::from_ref(&root));
        let added = root.join("parts");
        fs::create_dir(&added).expect("a directory appearing while the gallery runs");
        let mut notify = Recording::default();

        assert!(edit_landed(created(&added), &roots, &mut notify));

        assert_eq!(
            notify.0,
            [added],
            "the root is watched shallowly, so a directory added under it is watched by itself \
             or edits inside it are never heard"
        );
    }

    /// Drive the loop over `events` until whatever `rebuild` does stops the watcher.
    fn drive(
        events: &mpsc::Receiver<notify::Result<notify::Event>>,
        roots: &[WatchRoot],
        watcher: &SceneWatcher,
        rebuild: &mut dyn FnMut(),
    ) {
        WatchLoop {
            events,
            notify: &mut Recording::default(),
            roots,
            watcher,
            hot: &HotStatus::new(),
            rebuild,
            // Long enough to gather what is already waiting, short enough to test.
            quiet: Duration::from_millis(20),
        }
        .run();
    }

    #[test]
    fn edits_arriving_together_come_to_one_build() {
        let root = tree("coalesce", &["src/button.rs"]);
        let roots = watch_roots(std::slice::from_ref(&root));
        let (tx, rx) = mpsc::channel();
        for _ in 0..3 {
            tx.send(edited(&root.join("src/button.rs")))
                .expect("the loop has not started reading yet");
        }

        let watcher = idle();
        let mut builds = 0;
        let stopping = watcher.clone();
        drive(&rx, &roots, &watcher, &mut || {
            builds += 1;
            stopping.stop();
        });

        assert_eq!(builds, 1, "three edits in one quiet period are one rebuild");
        assert!(
            rx.try_recv().is_err(),
            "and all three were taken in that pass — building per event would have left the last \
             two waiting when the first build stopped the loop"
        );
    }

    #[test]
    fn changing_a_watched_local_dependency_triggers_one_rebuild() {
        let dependency = tree("local-dependency", &["src/widget.rs"]);
        let roots = watch_roots(std::slice::from_ref(&dependency));
        let (tx, rx) = mpsc::channel();
        tx.send(edited(&dependency.join("src/widget.rs")))
            .expect("the loop has not started reading yet");

        let watcher = idle();
        let stopping = watcher.clone();
        let mut builds = 0;
        drive(&rx, &roots, &watcher, &mut || {
            builds += 1;
            stopping.stop();
        });

        assert_eq!(builds, 1, "one dependency edit is one rebuild");
    }

    #[test]
    fn an_edit_landing_during_a_build_is_built_after_it_rather_than_restarting_it() {
        let root = tree("queued", &["src/button.rs"]);
        let roots = watch_roots(std::slice::from_ref(&root));
        let file = root.join("src/button.rs");
        let (tx, rx) = mpsc::channel();
        tx.send(edited(&file))
            .expect("the loop has not started reading yet");

        let watcher = idle();
        let mut builds = 0;
        let stopping = watcher.clone();
        drive(&rx, &roots, &watcher, &mut || {
            builds += 1;
            match builds {
                1 => tx.send(edited(&file)).expect("the loop is still reading"),
                _ => stopping.stop(),
            }
        });

        assert_eq!(
            builds, 2,
            "the edit that landed mid-build is built once that one is done"
        );
    }

    /// A build that is not cargo: a shell saying what cargo would, and exiting as it would.
    ///
    /// `build` appends arguments of its own, which land as the script's
    /// positional parameters and go unread.
    #[cfg(unix)]
    fn stand_in(script: &str) -> RebuildCommand {
        let script = script.to_owned();
        Box::new(move || {
            let mut command = Command::new("sh");
            command.args(["-c", &script]);
            command
        })
    }

    #[cfg(unix)]
    #[test]
    fn a_build_comes_to_what_cargos_stream_said_it_did() {
        let hot = HotStatus::new();
        let said = record("error", true, "error[E0308]: mismatched types\n");
        build(
            &stand_in(&format!("printf '%s\\n' '{said}' '{}'", finished(false))),
            &hot,
            &idle(),
        );

        let failure = failure(hot.phase());
        assert_eq!(headline(&failure), "Build failed — 1 error");
        assert!(failure.messages[0].text.contains("mismatched types"));

        let hot = HotStatus::new();
        build(
            &stand_in(&format!(
                "printf '%s\\n' '{}' '{}'",
                artifact(false),
                finished(true)
            )),
            &hot,
            &idle(),
        );
        assert!(
            matches!(hot.phase(), HotPhase::Swapping { .. }),
            "a build that came out waits for the dylib to be mapped"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_build_that_rendered_nothing_is_shown_as_what_it_said_on_its_way_out() {
        let hot = HotStatus::new();
        build(
            &stand_in("printf '%s\\n' 'error: failed to parse manifest' >&2; exit 101"),
            &hot,
            &idle(),
        );

        let failure = failure(hot.phase());
        assert!(
            failure.messages[0]
                .text
                .contains("failed to parse manifest"),
            "cargo's own last words, which are all there is: {:?}",
            failure.messages
        );
    }

    #[cfg(unix)]
    #[test]
    fn stopping_takes_a_build_down_rather_than_waiting_it_out() {
        let hot = HotStatus::new();
        let watcher = idle();
        let (done, ended) = mpsc::channel();
        let (building, reporting) = (watcher.clone(), hot.clone());
        thread::spawn(move || {
            build(&stand_in("sleep 30"), &reporting, &building);
            let _ = done.send(());
        });
        under_way(&watcher);

        // On a thread of its own: a stop that blocked on the child it is killing
        // would hang this test rather than fail it.
        thread::spawn(move || watcher.stop());

        ended.recv_timeout(Duration::from_secs(10)).expect(
            "a stopped build ends with the process it started, rather than being waited out",
        );
        assert!(
            !matches!(hot.phase(), HotPhase::Failed(_)),
            "a build stopped on the way out is not a failure to report"
        );
    }

    /// Wait for the build to have a child of its own to stop, and give up rather than spin forever.
    fn under_way(watcher: &SceneWatcher) {
        for _ in 0..500 {
            if watcher
                .building
                .lock()
                .expect("the build in flight")
                .is_some()
            {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("the stand-in build never started");
    }

    /// A build that has said everything it is going to say and is still running
    /// is the shape that used to hang a window on close: the reading was over,
    /// the wait on the process held the lock, and stopping wanted that same lock.
    #[cfg(unix)]
    #[test]
    fn stopping_does_not_wait_on_a_build_that_has_gone_quiet() {
        let watcher = idle();
        let (building, reporting) = (watcher.clone(), HotStatus::new());
        thread::spawn(move || build(&stand_in("exec >&- 2>&-; sleep 5"), &reporting, &building));
        // Waited out rather than polled for: the reading is over as soon as it starts,
        // so the build holds its child for a moment too short to catch, and what this needs
        // is to be past that — inside the wait, the one point at which the two could contend.
        thread::sleep(Duration::from_millis(300));

        let (done, stopped) = mpsc::channel();
        thread::spawn(move || {
            watcher.stop();
            let _ = done.send(());
        });

        stopped.recv_timeout(Duration::from_secs(2)).expect(
            "stopping returns while the build is still running, or closing a window would \
                     wait out whatever cargo had left to do",
        );
    }

    #[test]
    fn orderly_shutdown_joins_an_idle_watcher() {
        let watcher = spawn(Vec::new(), stand_in("exit 0"), &HotStatus::new());

        watcher.stop_and_join();

        assert!(!watcher.going());
        assert!(watcher.thread.lock().expect("the watcher thread").is_none());
    }

    /// One of cargo's records, as it writes them.
    fn record(level: &str, sited: bool, rendered: &str) -> String {
        serde_json::json!({
            "reason": "compiler-message",
            "message": {
                "level": level,
                "code": sited.then(|| serde_json::json!({ "code": "E0308" })),
                "spans": match sited {
                    true => vec![serde_json::json!({ "line_start": 3 })],
                    false => Vec::new(),
                },
                "rendered": rendered,
            }
        })
        .to_string()
    }

    fn finished(success: bool) -> String {
        serde_json::json!({ "reason": "build-finished", "success": success }).to_string()
    }

    /// A unit cargo built, or found it did not have to.
    fn artifact(fresh: bool) -> String {
        serde_json::json!({ "reason": "compiler-artifact", "fresh": fresh }).to_string()
    }

    /// The failure a phase came to, or a panic naming what it was instead.
    fn failure(phase: HotPhase) -> Arc<BuildFailure> {
        match phase {
            HotPhase::Failed(failure) => failure,
            other => panic!("a build that did not come out is a failure, not {other:?}"),
        }
    }

    #[test]
    fn a_failed_build_is_its_messages_and_a_count_of_the_ones_with_a_site() {
        let mut report = BuildReport::default();
        let said = [
            record("error", true, "error[E0308]: mismatched types\n"),
            record("warning", true, "warning: unused variable\n"),
            // rustc closes with a summary, which restates the count rather than being another
            // place to look: no code, no span.
            record("error", false, "error: aborting due to 1 previous error\n"),
        ];
        for line in &said {
            assert!(report.take(line).is_some(), "each is shown as it lands");
        }
        report.take(&finished(false));
        assert_eq!(report.errors, 1, "the summary is not another error");

        let failure = failure(report.came_to(String::new()));
        let mentions = |text: &str| failure.messages.iter().any(|m| m.text.contains(text));

        assert!(mentions("mismatched types"), "the error");
        assert!(mentions("unused variable"), "and what rustc said around it");
        assert_eq!(headline(&failure), "Build failed — 1 error");
    }

    #[test]
    fn a_message_reaches_the_terminal_with_its_escapes_and_the_window_without_them() {
        let mut report = BuildReport::default();
        let coloured = record("warning", false, "\u{1b}[33mwarning\u{1b}[0m: unused\n");

        let shown = report.take(&coloured).expect("a rendered message is shown");

        assert!(shown.contains('\u{1b}'), "the terminal keeps the colour");
        let kept = &report.messages.first().expect("the message").text;
        assert!(
            !kept.contains('\u{1b}'),
            "the window takes the words: {kept:?}"
        );
        assert!(kept.contains("warning: unused"));
    }

    #[test]
    fn a_line_that_is_not_a_record_is_not_shown_and_says_nothing() {
        let mut report = BuildReport::default();
        for line in ["", "not json", r#"{"reason":"compiler-artifact"}"#] {
            assert!(report.take(line).is_none(), "`{line}` is not ours to show");
        }
        assert_eq!(report, BuildReport::default(), "and left nothing behind");
    }

    #[test]
    fn a_build_that_failed_before_it_could_compile_shows_what_cargo_itself_said() {
        let mut report = BuildReport::default();
        report.take(&finished(false));

        let failure = failure(report.came_to("error: failed to parse manifest".to_owned()));

        assert!(
            failure.messages[0]
                .text
                .contains("failed to parse manifest")
        );
        assert_eq!(
            headline(&failure),
            "Build failed",
            "nothing rendered a diagnostic, so there is no count to state"
        );
    }

    #[test]
    fn a_build_cargo_never_finished_is_a_failure_rather_than_a_swap_that_never_comes() {
        let failure = failure(BuildReport::default().came_to("killed".to_owned()));
        assert_eq!(failure.errors, 0);
    }

    #[test]
    fn a_build_that_compiled_something_waits_for_the_swap_it_wrote() {
        let mut report = BuildReport::default();
        report.take(&artifact(false));
        report.take(&finished(true));

        assert!(matches!(
            report.came_to(String::new()),
            HotPhase::Swapping { .. }
        ));
    }

    #[test]
    fn a_build_that_compiled_nothing_says_nothing_rather_than_waiting_on_a_swap() {
        let mut report = BuildReport::default();
        report.take(&artifact(true));
        report.take(&finished(true));

        assert_eq!(
            report.came_to(String::new()),
            HotPhase::Watching,
            "cargo found everything fresh, so the dylib is the one already mapped — a rebuild \
             nobody asked for is a rebuild nobody should be shown"
        );
    }

    #[test]
    fn a_swap_that_never_comes_is_given_up_on() {
        // Something did compile and the dylib was rewritten, but it came to the same bytes:
        // the reloader hashes it, finds what it already has, and no reload follows.
        let hot = HotStatus::new();
        hot.set(HotPhase::Swapping {
            since: Instant::now() - SWAP_WAIT,
        });

        hot.settle();

        assert_eq!(
            hot.phase(),
            HotPhase::Watching,
            "rather than waited on forever"
        );
    }

    #[test]
    fn a_reload_is_timed_from_the_build_that_produced_it_and_lapses_on_its_own() {
        let hot = HotStatus::new();
        hot.set(HotPhase::Swapping {
            since: Instant::now() - Duration::from_millis(200),
        });
        hot.swapped();

        let HotPhase::Reloaded { took, .. } = hot.phase() else {
            panic!("swapping in the dylib is what reloaded means")
        };
        assert!(took >= Duration::from_millis(200), "timed from the build");

        hot.settle();
        assert!(hot.is_moving(), "shown for a while first");

        hot.set(HotPhase::Reloaded {
            at: Instant::now() - LINGER,
            took,
        });
        hot.settle();
        assert_eq!(
            hot.phase(),
            HotPhase::Watching,
            "and lapses once it is read"
        );
    }

    #[test]
    fn a_swap_gallery_did_not_build_is_still_a_reload() {
        // A bare `cargo build` in another terminal writes the same dylib.
        let hot = HotStatus::new();
        hot.swapped();
        assert!(matches!(hot.phase(), HotPhase::Reloaded { .. }));
    }

    #[test]
    fn every_phase_says_what_it_is_on_the_chip() {
        let phases = [
            HotPhase::Watching,
            HotPhase::Changed,
            HotPhase::Building {
                since: Instant::now(),
            },
            HotPhase::Swapping {
                since: Instant::now(),
            },
            HotPhase::Reloaded {
                at: Instant::now(),
                took: Duration::from_millis(1_800),
            },
            HotPhase::Failed(Arc::new(BuildFailure {
                messages: Vec::new(),
                errors: 2,
            })),
            HotPhase::Stopped {
                why: "no file watcher".to_owned(),
            },
        ];
        for phase in phases {
            let chip = hot_chip(&phase);
            assert!(!chip.hover.is_empty(), "{phase:?} says more on hover");
            assert!(
                chip.text.starts_with(char::is_uppercase),
                "{phase:?} is start cased, as the controls beside it are: {:?}",
                chip.text
            );
            // Geometric Shapes and Dingbats — where `●◌◐✓✕` live, and where the bundled symbol
            // fallback takes over from the face everything else is set in.
            assert!(
                !chip
                    .text
                    .chars()
                    .any(|c| matches!(c, '\u{25A0}'..='\u{25FF}' | '\u{2700}'..='\u{27BF}')),
                "{phase:?} leaves its mark to the painter: {:?}",
                chip.text
            );
        }
        assert_eq!(
            hot_chip(&HotPhase::Reloaded {
                at: Instant::now(),
                took: Duration::from_millis(1_800),
            })
            .text,
            "Reloaded · 1.8s",
            "a reload says how long it took, not when it was"
        );
    }
}
