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
    render_canvas_pinned, sheet,
    sheet::Panel,
    style::{self, paint},
    tree::{resolve_scene, scene_key},
};

mod overrides;
mod play_driver;
mod recipe;
mod report;
mod session;
mod shot;
#[cfg(test)]
mod tests;

use self::play_driver::*;
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
}

/// The play a shot runs before it is captured.
pub(crate) struct ShotPlay {
    /// A [`PlayEntry::name`] registered for the shot's scene.
    pub(crate) name: String,
    pub(crate) trace: Trace,
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
    render_with_globals(manifest, None, &[], renderer, setup, capture)
}

pub(crate) fn render_with_globals(
    manifest: &Manifest,
    globals: Option<GlobalEntry>,
    plays: &[PlayEntry],
    renderer: Renderer,
    setup: &impl Fn(&egui::Context),
    capture: &Capture,
) -> Result<(), Diagnostic> {
    // Before the loop rather than inside it — [`Session`] has why that matters.
    let session = Session::open(renderer)?;
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
        let mut failed_shot = None;
        match shoot_with_globals(
            manifest,
            globals,
            plays,
            &session,
            setup,
            shot,
            &mut failed_shot,
        ) {
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
