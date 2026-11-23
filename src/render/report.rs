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
}

/// What a shot's play did before the capture.
pub(super) struct PlayOutcome {
    pub(super) name: String,
    pub(super) frames: u32,
    pub(super) steps: Vec<StepOutcome>,
}

pub(super) struct StepOutcome {
    pub(super) label: String,
    pub(super) frames: std::ops::Range<u32>,
    /// The step's screenshot, when the trace was kept.
    pub(super) trace: Option<Utf8PathBuf>,
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
    /// Each shot whose play failed, step by step, in recipe order.
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
                        steps: play.steps.iter().map(StepReport::of).collect(),
                    }),
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
                    steps: failed.steps.iter().map(StepReport::of).collect(),
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
}

/// What a play that failed had done, kept for the report.
pub(super) struct FailedShot {
    name: String,
    scene: String,
    play: String,
    error: crate::play::PlayError,
    steps: Vec<StepOutcome>,
}

impl FailedShot {
    /// `shot`, whose `play` failed with `error`, taking the steps out of `run`.
    pub(super) fn of(
        shot: &Shot,
        scene: &SceneEntry,
        play: &ShotPlay,
        error: crate::play::PlayError,
        run: &mut crate::play::PlayRun,
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
            steps: run
                .steps
                .drain(..)
                .map(|step| StepOutcome {
                    label: step.label,
                    frames: step.frames,
                    trace: step.trace,
                })
                .collect(),
        }
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
}

#[derive(serde::Serialize)]
struct PlayReport<'a> {
    name: &'a str,
    /// Frames the play drew, before the shot settled again.
    frames: u32,
    steps: Vec<StepReport<'a>>,
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
    fn of(step: &'a StepOutcome) -> Self {
        Self {
            label: &step.label,
            frames: [step.frames.start, step.frames.end],
            trace: step.trace.as_deref(),
        }
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
/// an index into the images written, or into the plays that failed.
#[derive(Clone, Copy)]
pub(super) enum Slot {
    Written(usize),
    Failed(usize),
}

/// What the run came to: a table of every shot,
/// then each failed play's report, numbered.
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
        let failed = match run.failed.len() {
            0 => String::new(),
            1 => format!(", {}", paint(bad, "1 play failed")),
            n => format!(", {}", paint(bad, &format!("{n} plays failed"))),
        };
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
            table(run, dir, stream),
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

/// Every shot of a run, in recipe order, for [`report`] to lay out.
#[derive(Clone, Copy)]
pub(super) struct Ran<'a> {
    pub(super) order: &'a [Slot],
    pub(super) written: &'a [OutputFile],
    pub(super) failed: &'a [FailedShot],
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
/// Beneath a heading that names their directory, a row names just the file.
pub(super) fn table(run: Ran<'_>, dir: Option<&Utf8Path>, stream: style::Stream) -> String {
    let aside = Style::new().dimmed();
    let passed = paint(Style::new().fg_color(Some(AnsiColor::Green.into())), "✓");
    let failed = paint(Style::new().fg_color(Some(AnsiColor::Red.into())), "×");
    let mut table = comfy_table::Table::new();
    table
        .load_style(comfy_table::presets::UTF8_FULL_CONDENSED.with_rounded_corners())
        .set_header(["", "shot", "size", "steps", "screenshots"].map(|title| paint(aside, title)));
    // The width is padded on its own account, so the `×` holds still down the column.
    let digits = run
        .written
        .iter()
        .map(|w| w.size.width.to_string().len())
        .max()
        .unwrap_or_default();
    let screenshots_of = |steps: &[StepOutcome]| {
        steps
            .iter()
            .find_map(|step| step.trace.as_deref())
            .and_then(Utf8Path::parent)
            .map(|trace| match (dir, trace.file_name()) {
                (Some(_), Some(folder)) => style::path_as(trace, &format!("{folder}/"), stream),
                _ => style::path(trace, stream),
            })
            .unwrap_or_default()
    };
    for slot in run.order {
        let row = match *slot {
            Slot::Written(at) => {
                let w = &run.written[at];
                let name = match dir {
                    Some(_) => style::path_as(
                        &w.path,
                        w.path.file_name().unwrap_or(w.path.as_str()),
                        stream,
                    ),
                    None => style::path(&w.path, stream),
                };
                let mut size = format!("{:>digits$}×{}", w.size.width, w.size.height);
                if w.frames.motion == Motion::StillMoving {
                    size.push_str("  still moving");
                }
                let (steps, screenshots) = match &w.play {
                    None => (String::new(), String::new()),
                    Some(play) => (play.steps.len().to_string(), screenshots_of(&play.steps)),
                };
                [
                    passed.clone(),
                    name,
                    paint(aside, &size),
                    steps,
                    screenshots,
                ]
            }
            Slot::Failed(at) => {
                let shot = &run.failed[at];
                [
                    failed.clone(),
                    shot.name.clone(),
                    paint(aside, "failed"),
                    shot.steps.len().to_string(),
                    screenshots_of(&shot.steps),
                ]
            }
        };
        table.add_row(row);
    }
    let framed = table
        .lines()
        .map(|line| format!("  {}", line.trim_end()))
        .collect::<Vec<_>>()
        .join("\n");
    style::recede(&framed, aside)
}
