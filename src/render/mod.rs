//! Headless scene → PNG, for a caller with no screen. Only the canvas is captured,
//! through the same [`render_canvas`] the shell draws with, so a picture holds still
//! as the chrome around it changes.
//!
//! `--render` accepts knob and global overrides; a [`Recipe`] keeps a set of states
//! reviewable and re-runnable. Command-line globals override every recipe shot.
//!
//! Knobs are declarative-by-use, so an override cannot be seeded up front
//! — hence the frame protocol in [`draw`]: declare, apply, redraw, capture.
//! Overrides re-apply before every frame, so a recipe outranks whatever
//! a scene writes to its own knobs — capture and store both end on the recipe.

use std::{cell::RefCell, collections::BTreeMap, fmt::Write as _, panic::AssertUnwindSafe};

use anstyle::{AnsiColor, Style};
use camino::{Utf8Path, Utf8PathBuf};
use indoc::formatdoc;

use eframe::{egui_glow, glow};

#[cfg(not(target_vendor = "apple"))]
use crate::glow_capture::SharedCapture;

use crate::{
    ChoiceStyle, GlLoader, GlobalEntry, KnobState, Manifest, PANEL_BG, PlayEntry, RenderTarget,
    Renderer, SceneEntry,
    context::Pin,
    diagnostic::Diagnostic,
    globals::{GlobalState, control_shape},
    install_context,
    knobs::Span,
    offscreen::GlDeps,
    play::{
        log::{Acceptance, Check, Entry, Kept, Outcome, What},
        snapshot::{Ask, Look, Of, Unmatched, Verdict},
    },
    render_canvas_pinned, sheet,
    sheet::Panel,
    style::{self, paint},
    tree::{resolve_scene, scene_key},
};

pub(crate) use crate::video::{VideoFormat, VideoFormats};

mod log_view;
mod overlay;
mod overrides;
mod play_driver;
mod recipe;
mod record;
mod report;
mod session;
mod shot;
mod snapshot;
#[cfg(test)]
mod tests;

pub(crate) use self::log_view::LogMode;
use self::log_view::LogView;
pub(crate) use self::overlay::Corner;
use self::snapshot::{Looks, Seen};
pub(crate) use self::snapshot::{Snapshots, Update};
use self::{overlay::InputOverlay, play_driver::*, record::*};
pub(crate) use self::{overrides::*, recipe::*, report::*, session::*, shot::*};

/// One scene, in one state, at one size — what a single capture produces.
pub(crate) struct Shot {
    /// An exact scene key, else a case-insensitive regex;
    /// must name exactly one scene.
    pub(crate) scene: String,
    /// Where the PNG goes.
    /// `None` for a listing-only run.
    pub(crate) out: Option<Utf8PathBuf>,
    /// The canvas to lay out on, in points.
    pub(crate) size: egui::Vec2,
    /// Device pixels to the point, as a display's scale factor would set it.
    /// The layout is unmoved — `size` is still points — so the PNG is the same
    /// picture at `scale` times the pixels. [`DEFAULT_SCALE`] is one for one.
    pub(crate) scale: f32,
    /// Knob key (exact label, else a case-insensitive regex)
    /// to the value it should take.
    pub(crate) knobs: Overrides,
    /// Effective catalog-global overrides.
    pub(crate) globals: Overrides,
    /// How many frames to draw — and with `settle`, the most to draw.
    pub(crate) frames: Option<u32>,
    /// Crop the PNG to what the canvas drew.
    pub(crate) trim: bool,
    /// Stop as soon as the scene stops asking
    /// to be redrawn, rather than always drawing `frames`.
    pub(crate) settle: bool,
    /// Print the scene's knobs, their kinds and their values.
    pub(crate) list: bool,
    /// Print a capture recipe for this scene,
    /// its knobs written out at the values found.
    pub(crate) template: bool,
    /// A play to run once the scene settles;
    /// the shot captures where it leaves off.
    pub(crate) play: Option<ShotPlay>,
    /// An animated PNG to record beside the shot's own.
    pub(crate) record: Option<Record>,
}

/// A shot's recording: the whole play on a play shot,
/// else `seconds` once the scene settles.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Record {
    pub(crate) fps: u16,
    pub(crate) seconds: Option<f32>,
    /// `RetainOnFailure` drops a play's recording once the play passes.
    pub(crate) keep: Trace,
    pub(crate) formats: VideoFormats,
    /// Seconds a play's recording shows each step's last frame for, beyond its own;
    /// [`DEFAULT_HOLD`] when unset.
    pub(crate) hold: Option<f32>,
    /// Show the play's keys, mouse buttons and pointer on the recording,
    /// with the panel in this corner.
    pub(crate) input: Option<Corner>,
}

/// The highest frame rate a recording takes.
/// A typo like `600` would otherwise render ten times
/// the frames anyone wanted before anything checked.
const MAX_FPS: u16 = 240;

/// A play runs a click in a few frames, so its recording
/// pauses at each step's end long enough to see what the step did.
pub(crate) const DEFAULT_HOLD: f32 = 0.5;

impl Record {
    /// Refuse a recording that cannot be made as asked.
    ///
    /// # Errors
    /// For a frame rate of 0 or past [`MAX_FPS`], a length
    /// that is not a positive number, a length on a play shot
    /// (which records the play), or none on a shot without a play;
    /// a hold on a shot without a play, which has no steps, or one below 0.
    pub(crate) fn check(self, has_play: bool) -> Result<(), String> {
        if self.fps == 0 || self.fps > MAX_FPS {
            return Err(format!(
                "a recording at {} fps: it takes 1 to {MAX_FPS} frames a second",
                self.fps
            ));
        }
        match (self.hold, has_play) {
            (Some(_), false) => {
                return Err(
                    "a recording without a play has no steps, so it takes no `hold`".to_owned(),
                );
            }
            (Some(hold), true) if !(hold.is_finite() && hold >= 0.0) => {
                return Err(format!(
                    "a hold of {hold} seconds: it takes 0, for none, or more"
                ));
            }
            _ => {}
        }
        if self.input.is_some() && !has_play {
            return Err(
                "a recording without a play has no input to show, so it takes no `input`"
                    .to_owned(),
            );
        }
        match (self.seconds, has_play) {
            (Some(_), true) => {
                Err("a play shot records its play, so it takes no `seconds`".to_owned())
            }
            (None, false) => {
                Err("a recording without a play needs `seconds` to know how long".to_owned())
            }
            (Some(seconds), false) if !(seconds.is_finite() && seconds > 0.0) => Err(format!(
                "a recording of {seconds} seconds: it takes a number above 0"
            )),
            _ => Ok(()),
        }
    }

    /// How many frames a recording without a play draws.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "`check` keeps `seconds` positive and finite; a frame count past u32 is not a recording"
    )]
    pub(crate) fn frames(self) -> u32 {
        let seconds = self.seconds.unwrap_or(0.0);
        ((seconds * f32::from(self.fps)).round() as u32).max(1)
    }

    /// How many frames a play's recording holds each step's last frame for.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "`check` keeps `hold` at 0 or more and finite; a hold past u32 frames is not one"
    )]
    pub(crate) fn hold_frames(self) -> u32 {
        (self.hold.unwrap_or(DEFAULT_HOLD) * f32::from(self.fps)).round() as u32
    }
}

/// `FPS` or `FPS:SECONDS`, as `--record` takes it;
/// kept whatever the play does.
///
/// # Errors
/// When either part is not a number.
pub(crate) fn parse_record(raw: &str) -> Result<Record, String> {
    let (fps, seconds) = match raw.split_once(':') {
        Some((fps, seconds)) => (fps, Some(seconds)),
        None => (raw, None),
    };
    let fps = fps
        .trim()
        .parse::<u16>()
        .map_err(|_| format!("`{raw}` is not FPS or FPS:SECONDS"))?;
    let seconds = seconds
        .map(|seconds| seconds.trim().parse::<f32>())
        .transpose()
        .map_err(|_| format!("`{raw}` is not FPS or FPS:SECONDS"))?;
    Ok(Record {
        fps,
        seconds,
        keep: Trace::On,
        formats: VideoFormats::default(),
        hold: None,
        input: None,
    })
}

/// The play a shot runs before it is captured.
pub(crate) struct ShotPlay {
    /// A [`PlayEntry::name`] registered for the shot's scene.
    pub(crate) name: String,
    pub(crate) trace: Trace,
    /// List the play's steps under the run's summary.
    pub(crate) steps: bool,
}

/// When a play's per-step screenshots are written beside the shot.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Trace {
    On,
    #[default]
    RetainOnFailure,
}

/// The window's own inner size (see `run_with`),
/// so a headless canvas defaults to a shape the shell already renders at.
pub(crate) const DEFAULT_SIZE: egui::Vec2 = egui::vec2(1280.0, 720.0);

/// One pixel to the point, so a shot's PNG comes out the size it asked to lay out at.
pub(crate) const DEFAULT_SCALE: f32 = 1.0;

/// The most a shot may magnify. Nothing needs more, and a stray `--scale 100` would
/// otherwise ask the GPU for a texture in the tens of gigabytes before anything checked.
const MAX_SCALE: f32 = 8.0;

/// Refuse a scale that would not render.
///
/// # Errors
/// For anything but a finite number above zero, up to [`MAX_SCALE`].
pub(crate) fn check_scale(scale: f32) -> Result<(), String> {
    if scale.is_finite() && scale > 0.0 && scale <= MAX_SCALE {
        return Ok(());
    }
    Err(format!(
        "`scale = {scale}` is not a magnification: it takes a number above 0, up to {MAX_SCALE}"
    ))
}

/// Frames drawn before the capture.
///
/// One declares the knobs, the second draws with them applied,
/// and the rest let egui settle — a grid or a scroll area lays itself out
/// from the previous frame's measurements, as the knob-panel test in `knobs.rs` also relies on.
const DEFAULT_FRAMES: u32 = 4;

/// The fewest a shot can ask for and still be told the truth.
const MIN_FRAMES: u32 = 2;

/// Refuse a `frames` too small to capture the state a shot asked for.
///
/// A scene's knobs do not exist until it asks for them, so the first frame declares and the second
/// applies the recipe over them. One frame captures the scene at its own defaults
/// whatever the recipe says — the wrong picture rather than a rougher one, worth stopping for.
///
/// # Errors
/// For a `frames` below [`MIN_FRAMES`].
pub(crate) fn check_frames(frames: Option<u32>) -> Result<(), String> {
    match frames {
        Some(few) if few < MIN_FRAMES => Err(format!(
            "`frames = {few}` is too few: one frame declares a scene's knobs \
             and the next applies the recipe over them, so {MIN_FRAMES} is the least that captures \
             what was asked for"
        )),
        _ => Ok(()),
    }
}

/// Render every shot, in order.
///
/// Each gets its own harness. Sharing one would be faster,
/// but a scene's image would then depend on what ran before it —
/// residual scroll offsets, animation clocks, widget memory —
/// and a capture is worth nothing if it isn't reproducible alone.
///
/// # Errors
/// The first shot that fails, having already written the ones before it.
#[cfg(test)]
pub(crate) fn render(
    manifest: &Manifest,
    renderer: Renderer,
    setup: &impl Fn(&egui::Context),
    capture: &Capture,
) -> Result<(), Diagnostic> {
    render_with_globals(manifest, None, &[], renderer, setup, capture, LogMode::Off)
}

/// Run every shot of `capture`, its plays shown as they go as `log` says.
pub(crate) fn render_with_globals(
    manifest: &Manifest,
    globals: Option<GlobalEntry>,
    plays: &[PlayEntry],
    renderer: Renderer,
    setup: &impl Fn(&egui::Context),
    capture: &Capture,
    log: LogMode,
) -> Result<(), Diagnostic> {
    // Before the loop rather than inside it — [`Session`] has why that matters.
    let session = Session::open(renderer)?;
    let mut view = LogView::new(log);
    let mut written = Vec::new();
    let mut panels = Vec::new();
    let mut outcome = Ok(());
    // The sheet's own path once it lands, so the report can name it apart from the shots.
    let mut gathered = None;
    // Only the shots meant to write something: a listing-only shot has nothing to account for.
    let requested = capture
        .shots
        .iter()
        .filter(|shot| shot.out.is_some())
        .count();
    if let Some(out) = &capture.report {
        // Cleared before the first shot rather than overwritten after the last: a run that dies
        // — a failed write, a panic, a kill — then leaves no report at all, which reads as none
        // rather than as the last run's, which a loop would take for this one's.
        _ = std::fs::remove_file(out);
    }
    // A play that fails is a finding, and the run goes on to the next shot.
    // Anything else wrong is the run's own failure, and it stops.
    let mut failed_shots: Vec<FailedShot> = Vec::new();
    // Each failed shot's report, in the same order.
    let mut findings: Vec<Diagnostic> = Vec::new();
    // Every shot in recipe order, written or failed, for the summary table.
    let mut order = Vec::new();
    for shot in &capture.shots {
        let mut watch = Watch::new(&mut view);
        let shot_outcome =
            shoot_with_globals(manifest, globals, plays, &session, setup, shot, &mut watch);
        let failed_shot = watch.failed;
        match shot_outcome {
            Ok(None) => {}
            Ok(Some(taken)) => {
                // Held only for a sheet, so a run without one keeps a single capture in memory
                // rather than every capture it has taken.
                if capture.sheet.is_some() {
                    panels.push(Panel {
                        name: taken.written.path.file_stem().unwrap_or("shot").to_owned(),
                        image: taken.image,
                    });
                }
                order.push(Slot::Written(written.len()));
                written.push(taken.written);
            }
            Err(failure) => match failed_shot {
                Some(failed) => {
                    order.push(Slot::Failed(failed_shots.len()));
                    failed_shots.push(failed);
                    findings.push(failure);
                }
                None => {
                    outcome = Err(failure);
                    break;
                }
            },
        }
    }
    // The plays that ran: those that failed, and those whose shot was written.
    let played = failed_shots.len() + written.iter().filter(|w| w.play.is_some()).count();
    // One shot's failure is reported as it stands;
    // several get a verdict over the lot.
    let single = capture.shots.len() == 1;
    // Counted before the sheet is appended,
    // so the report can hand back the shots alone.
    let shots = written.len();
    // Two is the fewest that gather into anything:
    // a sheet of one panel is that panel with a caption,
    // so it is worth saying nothing was written rather than writing it.
    let mut skipped = None;
    if let (Ok(()), Some(out)) = (&outcome, &capture.sheet) {
        if panels.len() < 2 {
            skipped = Some(
                Diagnostic::warning(format!(
                    "a sheet gathers a run's captures, and this run made {}",
                    panels.len()
                ))
                .hint(format!(
                    "`{out}` was not written; drop `sheet` or add another `[[shot]]`"
                )),
            );
        } else {
            match gather(panels, out, &session, setup) {
                Ok(sheet) => {
                    // Kept out of `shots`: a sheet has no scene behind it, so `settled`
                    // and `frames` would answer questions nobody asked of it.
                    gathered = Some(sheet.path.clone());
                    written.push(sheet);
                }
                Err(failure) => outcome = Err(failure),
            }
        }
    }
    // After the sheet, which still gathers the shots that passed.
    let verdict = outcome.is_ok() && !findings.is_empty() && !single;
    if outcome.is_ok() && !findings.is_empty() {
        outcome = Err(if single {
            findings.pop().expect("a failure to report")
        } else {
            Diagnostic::new(format!(
                "{} of {played} play{} failed",
                failed_shots.len(),
                if played == 1 { "" } else { "s" }
            ))
        });
    }
    // Written whether or not the run finished: the shots that did land are on disk,
    // and this is where a reader learns the rest were meant to and didn't.
    if let Some(out) = &capture.report {
        let written_report = RunReport::of(
            &outcome,
            requested,
            &written[..shots],
            gathered.as_deref(),
            skipped.as_ref(),
            &failed_shots,
        );
        match write_report(&written_report, out) {
            Err(failure) if outcome.is_ok() => outcome = Err(failure),
            Err(_) => {}
            // Pointed at only once it exists,
            // so the path is where it landed.
            Ok(()) if verdict => {
                outcome = outcome.map_err(|verdict| {
                    verdict.hint(format!(
                        "the report lists each failure step by step: {}",
                        style::path_on_stderr(&landed(out))
                    ))
                });
            }
            Ok(()) => {}
        }
    }
    // Reported before the failure is returned: the shots that did run are on disk either way,
    // and an unlisted stale PNG is one a reader will trust.
    report(
        Ran {
            order: &order,
            written: &written[..shots],
            failed: &failed_shots,
            findings: &findings,
            sheet: gathered.as_deref(),
        },
        capture.report.as_deref().filter(|out| out.exists()),
        skipped.as_ref(),
        capture.shots.iter().any(|shot| shot.template),
    );
    outcome
}

/// One shot drawn for a test, its play's snapshots judged as `snapshots` has it:
/// nothing printed, nothing reported, only whether it failed.
///
/// # Errors
/// What a capture would report for the shot, a failed play or a panicking scene included;
/// a failed play's log follows its report. In colour when stdout takes it.
pub(crate) fn check_shot(
    manifest: &Manifest,
    globals: Option<GlobalEntry>,
    plays: &[PlayEntry],
    session: &Session,
    setup: &impl Fn(&egui::Context),
    shot: &Shot,
    snapshots: &Snapshots,
) -> Result<(), String> {
    let mut view = LogView::off();
    let mut watch = Watch::new(&mut view);
    watch.snapshots = Some(snapshots);
    watch.stream = style::Stream::Stdout;
    let outcome = shoot_with_globals(manifest, globals, plays, session, setup, shot, &mut watch);
    outcome.map(drop).map_err(|failure| {
        let log = watch.failed.as_ref().map(FailedShot::tree);
        let (report, log) = if style::coloured(watch.stream) {
            (failure.panel(watch.stream), log)
        } else {
            (failure.plain(), log.map(|log| style::plain(&log)))
        };
        match log {
            Some(log) => format!("{report}\n\n{log}"),
            None => report,
        }
    })
}

/// [`check_shot`], with the play's log: what a test of its checks reads.
/// The shot must have an `out`, which a passing play's log comes back with.
#[cfg(all(test, not(target_vendor = "apple")))]
pub(crate) fn judged_shot(
    manifest: &Manifest,
    plays: &[PlayEntry],
    renderer: Renderer,
    shot: &Shot,
    snapshots: &Snapshots,
) -> (Result<(), Diagnostic>, Vec<Entry>) {
    let session = Session::open(renderer).expect("a session");
    let mut view = LogView::off();
    let mut watch = Watch::new(&mut view);
    watch.snapshots = Some(snapshots);
    let setup = |_: &egui::Context| {};
    match shoot_with_globals(manifest, None, plays, &session, &setup, shot, &mut watch) {
        Ok(taken) => {
            let play = taken.and_then(|taken| taken.written.play);
            (Ok(()), play.expect("a play").log)
        }
        Err(failure) => {
            let log = watch.failed.map(|failed| failed.log);
            (Err(failure), log.unwrap_or_default())
        }
    }
}

/// What one invocation asks for.
pub(crate) struct Capture {
    pub(crate) shots: Vec<Shot>,
    /// Where to gather the captures. `None` unless a recipe asked for a sheet.
    pub(crate) sheet: Option<Utf8PathBuf>,
    /// Where to write the run's JSON report. `None` unless a recipe asked for one.
    pub(crate) report: Option<Utf8PathBuf>,
}

/// Draw the run's captures onto one sheet and write it.
fn gather(
    panels: Vec<Panel>,
    out: &Utf8Path,
    session: &Session,
    setup: &impl Fn(&egui::Context),
) -> Result<OutputFile, Diagnostic> {
    let image = sheet::compose(panels, session, setup)?;
    write_png(&image, out)?;
    Ok(OutputFile {
        path: landed(out),
        size: Size {
            width: image.width(),
            height: image.height(),
        },
        // The panels land at the size they were taken, whatever scale took them.
        scale: DEFAULT_SCALE,
        globals: BTreeMap::new(),
        play: None,
        videos: Vec::new(),
        bytes: std::fs::metadata(out).map(|file| file.len()).unwrap_or(0),
        // A sheet is drawn once from images already taken;
        // there is nothing for it to settle.
        frames: Frames {
            drawn: 1,
            motion: Motion::Settled,
        },
    })
}

/// Parse a `WxH` size, in points — which at the pinned scale are the PNG's pixels.
///
/// # Errors
/// If it isn't two positive numbers around an `x`.
pub(crate) fn parse_size(raw: &str) -> Result<egui::Vec2, String> {
    let malformed = || format!("size `{raw}` is not `WxH` (e.g. `1280x720`)");
    let (w, h) = raw.split_once(['x', 'X']).ok_or_else(malformed)?;
    let (w, h) = (
        w.trim().parse::<f32>().map_err(|_| malformed())?,
        h.trim().parse::<f32>().map_err(|_| malformed())?,
    );
    if w <= 0.0 || h <= 0.0 {
        return Err(format!("size `{raw}` needs both sides above zero"));
    }
    Ok(egui::vec2(w, h))
}
