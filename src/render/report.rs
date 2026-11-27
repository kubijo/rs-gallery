//! What a run came to: the JSON report and the table printed for a person.

use super::*;

/// Write what the run came to as JSON, for something other than a person to read.
///
/// The sheet is named separately rather than listed among the shots: it has no scene behind it,
/// so `settled` and `frames` would be answers to questions nobody asked of it.
pub(super) fn write_report(report: &RunReport<'_>, out: &Utf8Path) -> Result<(), Diagnostic> {
    let json = serde_json::to_string_pretty(report)
        .map_err(|e| Diagnostic::from(format!("build the report: {e}")))?;
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| Diagnostic::from(format!("create `{parent}`: {e}")))?;
    }
    std::fs::write(out, json).map_err(|e| Diagnostic::from(format!("write `{out}`: {e}")))?;
    Ok(())
}

/// A PNG this run produced, held back so the whole set can be reported in one block.
pub(super) struct OutputFile {
    pub(super) path: Utf8PathBuf,
    pub(super) size: Size,
    /// Device pixels to the point, which `size` is already multiplied by.
    pub(super) scale: f32,
    pub(super) bytes: u64,
    pub(super) frames: Frames,
    pub(super) globals: BTreeMap<String, ControlValue>,
    pub(super) play: Option<PlayOutcome>,
    /// A recording's file in each format, or none.
    pub(super) videos: Vec<VideoOutcome>,
}

/// What a shot's play did before the capture.
pub(super) struct PlayOutcome {
    pub(super) name: String,
    pub(super) frames: u32,
    pub(super) log: Vec<Entry>,
}

/// The entries of `log` the report counts as steps.
fn steps(log: &[Entry]) -> Vec<StepReport<'_>> {
    log.iter().filter_map(StepReport::of).collect()
}

/// A run's outcome as JSON, for a loop nobody is watching.
///
/// Its own types rather than `#[derive(Serialize)]` on the ones above, so the file a consumer
/// parses is a stated format and not whatever the internals happen to be called this week.
#[derive(serde::Serialize)]
pub(super) struct RunReport<'a> {
    /// Whether every shot the recipe asked for was written. A reader that only counts `shots`
    /// cannot tell a recipe of three from one of ten that stopped at three.
    complete: bool,
    /// What stopped the run, when something did.
    #[serde(skip_serializing_if = "Option::is_none")]
    failed: Option<String>,
    /// How many shots were meant to write an image, which on a run that finished
    /// is how many `shots` holds. Listing-only shots are not counted, having none to write.
    requested: usize,
    shots: Vec<ShotReport<'a>>,
    /// Absent unless a sheet was gathered.
    /// Asking for one that was skipped leaves a warning, not a path.
    #[serde(skip_serializing_if = "Option::is_none")]
    sheet: Option<&'a Utf8Path>,
    /// What the run got on with despite, each a one-line summary of what it printed.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
    /// Each shot that failed and let the run go on, its play step by step, in recipe order.
    /// Absent unless one did.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    failed_shots: Vec<FailedShotReport<'a>>,
}

impl<'a> RunReport<'a> {
    /// `written` holds the shots alone; a gathered sheet is `sheet`.
    pub(super) fn of(
        outcome: &Result<(), Diagnostic>,
        requested: usize,
        written: &'a [OutputFile],
        sheet: Option<&'a Utf8Path>,
        skipped: Option<&Diagnostic>,
        failed_shots: &'a [FailedShot],
    ) -> Self {
        Self {
            complete: outcome.is_ok(),
            failed: outcome.as_ref().err().map(Diagnostic::summary),
            requested,
            shots: written
                .iter()
                .map(|file| ShotReport {
                    name: file.path.file_stem().unwrap_or("shot"),
                    path: &file.path,
                    width: file.size.width,
                    height: file.size.height,
                    scale: file.scale,
                    bytes: file.bytes,
                    settled: file.frames.motion == Motion::Settled,
                    frames: file.frames.drawn,
                    globals: &file.globals,
                    play: file.play.as_ref().map(|play| PlayReport {
                        name: &play.name,
                        frames: play.frames,
                        steps: steps(&play.log),
                        log: play.log.iter().map(EntryReport::of).collect(),
                    }),
                    videos: file.videos.iter().map(VideoReport::of).collect(),
                })
                .collect(),
            sheet,
            warnings: skipped.iter().map(|warning| warning.summary()).collect(),
            failed_shots: failed_shots
                .iter()
                .map(|failed| FailedShotReport {
                    name: &failed.name,
                    scene: &failed.scene,
                    play: &failed.play,
                    failed_step: failed.error.step(),
                    reason: failed.error.reason(),
                    at: failed
                        .error
                        .at()
                        .map(|at| format!("{}:{}:{}", at.file, at.line, at.column)),
                    on_screen: failed.error.on_screen(),
                    steps: steps(&failed.log),
                    check: failed.error.check().map(CheckReport::of),
                    log: failed.log.iter().map(EntryReport::of).collect(),
                    videos: &failed.videos,
                    lost_videos: &failed.lost_videos,
                })
                .collect(),
        }
    }
}

#[derive(serde::Serialize)]
struct FailedShotReport<'a> {
    name: &'a str,
    scene: &'a str,
    play: &'a str,
    /// The innermost step it failed in,
    /// absent when it failed outside any.
    #[serde(skip_serializing_if = "Option::is_none")]
    failed_step: Option<&'a str>,
    reason: &'a str,
    /// The play's own line that failed: `file:line:column`.
    #[serde(skip_serializing_if = "Option::is_none")]
    at: Option<String>,
    /// The labels in the tree when a widget could not be found.
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    on_screen: &'a [String],
    steps: Vec<StepReport<'a>>,
    /// The check that failed the play, when a matcher is what stopped it.
    #[serde(skip_serializing_if = "Option::is_none")]
    check: Option<CheckReport<'a>>,
    log: Vec<EntryReport<'a>>,
    /// The play's recording in each format that was written.
    #[serde(skip_serializing_if = "<[Utf8PathBuf]>::is_empty")]
    videos: &'a [Utf8PathBuf],
    /// What was lost of the recordings the shot asked for, and why.
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    lost_videos: &'a [String],
}

/// A shot that failed, and what its play had done, kept for the report.
pub(super) struct FailedShot {
    name: String,
    scene: String,
    play: String,
    error: crate::play::PlayError,
    pub(super) log: Vec<Entry>,
    videos: Vec<Utf8PathBuf>,
    pub(super) lost_videos: Vec<String>,
}

impl FailedShot {
    /// `shot`, which failed with `error`, taking the log of its `play` out of `run`.
    pub(super) fn of(
        shot: &Shot,
        scene: &SceneEntry,
        play: &ShotPlay,
        error: crate::play::PlayError,
        run: &mut crate::play::PlayRun,
        videos: Vec<Utf8PathBuf>,
        lost_videos: Vec<String>,
    ) -> Self {
        Self {
            name: shot
                .out
                .as_deref()
                .and_then(Utf8Path::file_stem)
                .unwrap_or(&shot.scene)
                .to_owned(),
            scene: scene.name.to_owned(),
            play: play.name.clone(),
            error,
            log: std::mem::take(&mut run.log),
            videos,
            lost_videos,
        }
    }

    /// Its play's `log` as the terminal shows it.
    pub(super) fn tree(&self, log: &[Entry]) -> String {
        step_list(&self.name, &self.play, log, true)
    }
}

#[derive(serde::Serialize)]
struct ShotReport<'a> {
    /// The shot's name, which is also its PNG's file stem.
    name: &'a str,
    path: &'a Utf8Path,
    width: u32,
    height: u32,
    /// Device pixels to the point, which `width` and `height` are already multiplied by.
    /// Without it a shot taken at 2× reads as one laid out twice as large.
    scale: f32,
    bytes: u64,
    /// `false` when a `settle` shot ran out of frames still animating —
    /// the image is a moment the frame count landed on, not one the scene chose.
    settled: bool,
    /// Frames drawn, which for a settled shot is where it went quiet.
    frames: u32,
    /// Resolved stateful globals, including defaults, keyed by their exact labels.
    globals: &'a BTreeMap<String, ControlValue>,
    /// Absent unless the shot ran a play.
    #[serde(skip_serializing_if = "Option::is_none")]
    play: Option<PlayReport<'a>>,
    /// The recording's file in each format, absent unless the shot kept one.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    videos: Vec<VideoReport<'a>>,
}

/// One format's file of a recording, and what its frames cost on average.
#[derive(serde::Serialize)]
struct VideoReport<'a> {
    format: &'static str,
    path: &'a Utf8Path,
    fps: u16,
    /// Frames drawn into it.
    /// A play's frame `n`, as its steps count them, is the `n`th drawn.
    frames: u32,
    /// Frames added by pausing on each step's last frame.
    held: u32,
    /// Frames added by gliding the pointer between gestures,
    /// so the file runs `frames + held + glided` frames long.
    glided: u32,
    /// Frames the file holds, a repeat having lengthened the frame before it instead.
    written: u32,
    /// Drawing the scene, per frame, in microseconds.
    draw_us: u64,
    /// Rendering a frame and reading it back, per frame drawn, in microseconds;
    /// the glides' frames are made in this time too.
    capture_us: u64,
    encode_us: u64,
    /// The most frames waiting for the encoder at once.
    deepest_queue: usize,
}

impl<'a> VideoReport<'a> {
    fn of(video: &'a VideoOutcome) -> Self {
        let per_frame = |spent: std::time::Duration| {
            u64::try_from(spent.as_micros()).unwrap_or(u64::MAX) / u64::from(video.frames.max(1))
        };
        Self {
            format: video.format.extension(),
            path: &video.path,
            fps: video.fps,
            frames: video.frames,
            held: video.held,
            glided: video.glided,
            written: video.written,
            draw_us: per_frame(video.draw),
            capture_us: per_frame(video.capture),
            encode_us: per_frame(video.encode),
            deepest_queue: video.deepest_queue,
        }
    }
}

#[derive(serde::Serialize)]
struct PlayReport<'a> {
    name: &'a str,
    /// Frames the play drew, before the shot settled again.
    frames: u32,
    steps: Vec<StepReport<'a>>,
    log: Vec<EntryReport<'a>>,
}

/// One thing a play did: a step, a gesture or a check, in the order they began.
#[derive(serde::Serialize)]
pub(super) struct EntryReport<'a> {
    #[serde(flatten)]
    what: WhatReport<'a>,
    /// How many steps were open around it.
    depth: usize,
    /// The play's frames it drew: `[first, end)`.
    frames: [u32; 2],
    /// The play's line it ran at: `file:line:column`.
    #[serde(skip_serializing_if = "Option::is_none")]
    at: Option<String>,
    /// `passed`, `failed` or `skipped`; absent while it runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    outcome: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    trace: Option<&'a Utf8Path>,
}

#[derive(serde::Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum WhatReport<'a> {
    Step { label: &'a str, path: &'a str },
    Gesture { label: &'a str },
    Check(CheckReport<'a>),
}

#[derive(serde::Serialize)]
struct CheckReport<'a> {
    /// What was looked at: a widget as its locator reads, `actions` or `value`.
    subject: &'a str,
    /// The method that checked it: `to_have_value`.
    matcher: &'a str,
    negated: bool,
    /// What should hold: `value is equal to "Ada"`.
    expected: &'a str,
    /// What was there when it was last looked at; `null` when nothing matched.
    actual: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    explanation: Option<&'a str>,
    /// The play's own words for it.
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<&'a str>,
}

impl<'a> CheckReport<'a> {
    fn of(check: &'a Check) -> Self {
        Self {
            subject: &check.subject,
            matcher: check.matcher,
            negated: check.negated,
            expected: &check.expected,
            actual: check.actual.as_deref(),
            explanation: check.explanation.as_deref(),
            message: check.message.as_deref(),
        }
    }
}

impl<'a> EntryReport<'a> {
    pub(super) fn of(entry: &'a Entry) -> Self {
        Self {
            what: match &entry.what {
                What::Step { label, path } => WhatReport::Step { label, path },
                What::Gesture { label } => WhatReport::Gesture { label },
                What::Check(check) => WhatReport::Check(CheckReport::of(check)),
            },
            depth: entry.depth,
            frames: [entry.frames.start, entry.frames.end],
            at: entry
                .at
                .as_ref()
                .map(|at| format!("{}:{}:{}", at.file, at.line, at.column)),
            outcome: entry.outcome.map(|outcome| match outcome {
                Outcome::Passed => "passed",
                Outcome::Tolerated => "tolerated",
                Outcome::Failed => "failed",
                Outcome::Skipped => "skipped",
            }),
            trace: entry.trace.as_deref(),
        }
    }
}

#[derive(serde::Serialize)]
struct StepReport<'a> {
    label: &'a str,
    /// The play's frames this step drew: `[first, end)`.
    frames: [u32; 2],
    #[serde(skip_serializing_if = "Option::is_none")]
    trace: Option<&'a Utf8Path>,
}

impl<'a> StepReport<'a> {
    /// `entry` as the report's step, if it counts as one.
    fn of(entry: &'a Entry) -> Option<Self> {
        Some(Self {
            label: entry.step_path()?,
            frames: [entry.frames.start, entry.frames.end],
            trace: entry.trace.as_deref(),
        })
    }
}

/// Dimensions in pixels — a written PNG's, a sheet cell's.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Size {
    pub(crate) width: u32,
    pub(crate) height: u32,
}

/// Whether a scene had stopped asking to be redrawn when its shot was taken.
///
/// [`StillMoving`](Self::StillMoving) only ever comes of a `settle` shot that ran out of frames
/// mid-animation. It says the captured moment is where the frame count landed rather than one
/// the scene chose — the difference between a diff worth reading and a phantom one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Motion {
    Settled,
    StillMoving,
}

/// A shot's place in the summary:
/// an index into the images written, or into the shots that failed.
#[derive(Clone, Copy)]
pub(super) enum Slot {
    Written(usize),
    Failed(usize),
}

/// What the run came to: a table of every shot,
/// then each failed one's report, numbered.
///
/// Each block opens on a blank line and closes on none,
/// so the shell prompt lands one line under whichever block came last.
pub(super) fn report(
    run: Ran<'_>,
    json: Option<&Utf8Path>,
    skipped: Option<&Diagnostic>,
    recipe_on_stdout: bool,
) {
    let print = |text: &str| {
        if recipe_on_stdout {
            anstream::eprintln!("\n{text}");
        } else {
            anstream::println!("\n{text}");
        }
    };
    let aside = Style::new().dimmed();
    let stream = if recipe_on_stdout {
        style::Stream::Stderr
    } else {
        style::Stream::Stdout
    };
    if !run.order.is_empty() {
        let good = Style::new().bold().fg_color(Some(AnsiColor::Green.into()));
        let bad = Style::new().bold().fg_color(Some(AnsiColor::Red.into()));
        let dir = shared_dir(run.written);
        let failed = failures(run.failed.len(), run.ended)
            .map(|failures| format!(", {}", paint(bad, &failures)))
            .unwrap_or_default();
        let headline = format!(
            "{} {} image{}{}{failed}",
            paint(good, "Gallery wrote"),
            run.written.len(),
            if run.written.len() == 1 { "" } else { "s" },
            dir.map(|dir| format!(" in {}", style::path(dir, stream)))
                .unwrap_or_default()
        );
        let mut footer = human(run.written.iter().map(|w| w.bytes).sum());
        if let Some(sheet) = run.sheet {
            footer.push_str(&format!(" · sheet {}", style::path(sheet, stream)));
        }
        if let Some(json) = json {
            footer.push_str(&format!(" · report {}", style::path(json, stream)));
        }
        print(&format!(
            "{headline}\n{}\n  {}",
            table(run, stream),
            paint(aside, &footer)
        ));
    } else if let Some(json) = json {
        // A run that failed before any shot came to anything: the report says how far it got.
        print(&format!(
            "{} {}",
            paint(aside, "Report"),
            style::path(json, stream)
        ));
    }
    let count = run.findings.len();
    let titled: Vec<(String, &Diagnostic)> = run
        .findings
        .iter()
        .zip(run.failed)
        .enumerate()
        .map(|(at, (finding, failed))| {
            (format!("{} of {count} · {}", at + 1, failed.name), finding)
        })
        .collect();
    Diagnostic::report_all(&titled);
    if let Some(note) = skipped {
        note.report();
    }
}

/// A play's log as a tree: each step, gesture and check with the frames it drew,
/// a failed check saying what it found instead, a skipped one why.
/// A play that `failed` with no entry to blame says it failed outside any step.
pub(super) fn step_list(shot: &str, play: &str, log: &[Entry], failed: bool) -> String {
    let aside = Style::new().dimmed();
    let passed = paint(Style::new().fg_color(Some(AnsiColor::Green.into())), "✓");
    let broke = paint(Style::new().fg_color(Some(AnsiColor::Red.into())), "×");
    let tolerated = paint(Style::new().fg_color(Some(AnsiColor::Yellow.into())), "≈");
    let running = paint(aside, "…");
    let skipped = paint(aside, "–");
    let lines: Vec<(String, &Entry)> = log
        .iter()
        .map(|entry| {
            (
                format!("{}{}", "  ".repeat(entry.depth), line(entry)),
                entry,
            )
        })
        .collect();
    let width = lines
        .iter()
        .map(|(text, _)| style::visible_width(text))
        .max()
        .unwrap_or(0);
    let digits = lines
        .iter()
        .map(|(_, entry)| entry.frames.len().to_string().len())
        .max()
        .unwrap_or(1);
    let mut list = format!(
        "{} {}",
        paint(Style::new().bold(), shot),
        paint(aside, &format!("· {play}"))
    );
    if lines.is_empty() && !failed {
        list.push_str(&format!("\n  {}", paint(aside, "no steps")));
    }
    for (text, entry) in lines {
        let frames = entry.frames.len();
        let mark = match entry.outcome {
            Some(Outcome::Passed) => &passed,
            Some(Outcome::Tolerated) => &tolerated,
            Some(Outcome::Failed) => &broke,
            Some(Outcome::Skipped) => &skipped,
            None => &running,
        };
        list.push_str(&format!(
            "\n  {mark} {text}{}  {}",
            " ".repeat(width - style::visible_width(&text)),
            paint(
                aside,
                &format!(
                    "{frames:>digits$} frame{}",
                    if frames == 1 { "" } else { "s" }
                )
            ),
        ));
    }
    if failed && !log.iter().any(Entry::failed) {
        list.push_str(&format!(
            "\n  {broke} {}",
            paint(aside, "failed outside any step")
        ));
    }
    list
}

/// What an entry's line says, before its indent and frames.
fn line(entry: &Entry) -> String {
    let aside = Style::new().dimmed();
    match &entry.what {
        What::Step { label, .. } => label.clone(),
        What::Gesture { label } => paint(aside, label),
        What::Check(check) => {
            let mut line = format!("{} {}", check.subject, one_line(&check.expected));
            let actual = one_line(check.actual.as_deref().unwrap_or("nothing matched"));
            match entry.outcome {
                Some(Outcome::Failed) => {
                    line.push_str(&paint(aside, &format!("  got {actual}")));
                }
                Some(Outcome::Skipped | Outcome::Tolerated) => {
                    line.push_str(&paint(aside, &format!("  {actual}")));
                }
                Some(Outcome::Passed) | None => {}
            }
            line
        }
    }
}

/// What the summary's headline says of the `failed` shots,
/// and of one more that `ended` the run.
pub(super) fn failures(failed: usize, ended: bool) -> Option<String> {
    let ending = ended.then_some("ended the run");
    match (failed, ending) {
        (0, ending) => ending.map(|it| format!("1 shot {it}")),
        (1, None) => Some("1 shot failed".to_owned()),
        (1, Some(it)) => Some(format!("1 shot failed, 1 {it}")),
        (n, None) => Some(format!("{n} shots failed")),
        (n, Some(it)) => Some(format!("{n} shots failed, 1 {it}")),
    }
}

/// A phrase on one line: googletest lays
/// a compound matcher's parts out on lines of their own.
pub(crate) fn one_line(phrase: &str) -> String {
    phrase.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Every shot of a run, in recipe order, for [`report`] to lay out.
#[derive(Clone, Copy)]
pub(super) struct Ran<'a> {
    pub(super) order: &'a [Slot],
    pub(super) written: &'a [OutputFile],
    pub(super) failed: &'a [FailedShot],
    /// Whether one more shot failed, and ended the run.
    pub(super) ended: bool,
    /// Each failed shot's report, unless the run's one shot failed and its report stands alone.
    pub(super) findings: &'a [Diagnostic],
    pub(super) sheet: Option<&'a Utf8Path>,
}

/// A byte count the way a reader wants it: a figure and a unit, not eight digits to count through.
pub(super) fn human(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit + 1 < UNITS.len() {
        size /= 1024.0;
        unit += 1;
    }
    match unit {
        0 => format!("{bytes} B"),
        _ => format!("{size:.1} {}", UNITS[unit]),
    }
}

/// The labels on screen when a play gave up,
/// wrapped into rows beneath their caption.
pub(super) fn on_screen(labels: &[String]) -> String {
    const MOST: usize = 24;
    const WIDTH: usize = 72;
    const CAPTION: &str = "on screen: ";
    let mut rows = vec![String::new()];
    for label in labels.iter().take(MOST) {
        let label = format!("`{label}`");
        let row = rows.last_mut().expect("a row to add to");
        if !row.is_empty() && style::visible_width(row) + 2 + style::visible_width(&label) > WIDTH {
            rows.push(label);
        } else {
            if !row.is_empty() {
                row.push_str("  ");
            }
            row.push_str(&label);
        }
    }
    let more = labels.len().saturating_sub(MOST);
    if more > 0 {
        rows.last_mut()
            .expect("a row to add to")
            .push_str(&format!("  and {more} more"));
    }
    let indent = " ".repeat(CAPTION.len());
    format!("{CAPTION}{}", rows.join(&format!("\n{indent}")))
}

/// The directory every image landed in, when they all landed in one.
fn shared_dir(written: &[OutputFile]) -> Option<&Utf8Path> {
    let first = written.first()?.path.parent()?;
    written
        .iter()
        .all(|w| w.path.parent() == Some(first))
        .then_some(first)
}

/// One row per shot, so a set can be scanned for the odd one out.
/// Every path is written out from where gallery ran, so it can be pasted as it reads.
pub(super) fn table(run: Ran<'_>, stream: style::Stream) -> String {
    let aside = Style::new().dimmed();
    let passed = paint(Style::new().fg_color(Some(AnsiColor::Green.into())), "✓");
    let failed = paint(Style::new().fg_color(Some(AnsiColor::Red.into())), "×");
    let linked = |path: &Utf8Path| style::path(path, stream);
    // The width is padded on its own account,
    // so the `×` holds still down the column.
    let digits = run
        .written
        .iter()
        .map(|w| w.size.width.to_string().len())
        .max()
        .unwrap_or_default();
    let screenshots_of = |log: &[Entry]| {
        log.iter()
            .find_map(|entry| entry.trace.as_deref())
            .and_then(Utf8Path::parent)
            .map(linked)
            .unwrap_or_default()
    };
    let mut rows = Vec::new();
    for slot in run.order {
        rows.push(match *slot {
            Slot::Written(at) => {
                let w = &run.written[at];
                let mut size = format!("{:>digits$}×{}", w.size.width, w.size.height);
                if w.frames.motion == Motion::StillMoving {
                    size.push_str("  still moving");
                }
                let (steps, screenshots) = match &w.play {
                    None => (String::new(), String::new()),
                    Some(play) => (
                        steps(&play.log).len().to_string(),
                        screenshots_of(&play.log),
                    ),
                };
                let recording = w
                    .videos
                    .iter()
                    .map(|video| {
                        let length = f64::from(video.frames + video.held + video.glided)
                            / f64::from(video.fps);
                        format!(
                            "{}  {}",
                            linked(&video.path),
                            paint(aside, &format!("{} frames · {length:.1} s", video.frames))
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                vec![
                    passed.clone(),
                    linked(&w.path),
                    paint(aside, &size),
                    steps,
                    screenshots,
                    recording,
                ]
            }
            Slot::Failed(at) => {
                let shot = &run.failed[at];
                vec![
                    failed.clone(),
                    shot.name.clone(),
                    paint(aside, "failed"),
                    steps(&shot.log).len().to_string(),
                    screenshots_of(&shot.log),
                    shot.videos
                        .iter()
                        .map(|path| linked(path))
                        .collect::<Vec<_>>()
                        .join("\n"),
                ]
            }
        });
    }
    let titles = ["", "shot", "size", "steps", "screenshots", "recording"];
    let filled = |column: &usize| rows.iter().any(|row| !row[*column].is_empty());
    let columns: Vec<usize> = (0..titles.len()).filter(filled).collect();
    let mut table = comfy_table::Table::new();
    table
        .load_style(comfy_table::presets::UTF8_FULL_CONDENSED.with_rounded_corners())
        .set_header(columns.iter().map(|&at| paint(aside, titles[at])));
    for row in &rows {
        table.add_row(columns.iter().map(|&at| &row[at]));
    }
    let framed = table
        .lines()
        .map(|line| format!("  {}", line.trim_end()))
        .collect::<Vec<_>>()
        .join("\n");
    style::recede(&framed, aside)
}
