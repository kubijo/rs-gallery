//! Headless scene → PNG, for a caller with no screen. Only the canvas is captured,
//! through the same [`render_canvas`] the shell draws with, so a picture holds still
//! as the chrome around it changes.
//!
//! `--render` takes a scene at its defaults; setting knobs takes a [`Recipe`],
//! which keeps labels containing spaces out of argv and makes a set of states
//! reviewable and re-runnable.
//!
//! Knobs are declarative-by-use, so an override cannot be seeded up front
//! — hence the frame protocol in [`draw`]: declare, apply, redraw, capture.
//! Overrides re-apply before every frame, so a recipe outranks whatever
//! a scene writes to its own knobs — capture and store both end on the recipe.

use std::{collections::BTreeMap, fmt::Write as _};

use anstyle::{AnsiColor, Style};
use camino::{Utf8Path, Utf8PathBuf};
use indoc::formatdoc;

use eframe::{egui_glow, glow};

#[cfg(not(target_vendor = "apple"))]
use crate::glow_capture::SharedCapture;

use crate::{
    ChoiceStyle, GlLoader, Knob, Manifest, PANEL_BG, RenderTarget, Renderer, SceneEntry,
    diagnostic::Diagnostic,
    install_context,
    offscreen::GlDeps,
    render_canvas, sheet,
    sheet::Panel,
    style::{frame, link, paint},
    tree::{resolve_scene, scene_key},
};

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
    pub(crate) knobs: Vec<KnobOverride>,
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
}

/// One knob a shot sets, over whatever its scene declared.
///
/// The key is an exact label, else a case-insensitive regex over the labels.
/// The value is parsed against whatever kind the knob turns out to be,
/// so nothing here is checked until a frame has declared the knob.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct KnobOverride {
    pub(crate) key: String,
    pub(crate) value: String,
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
pub(crate) fn render(
    manifest: &Manifest,
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
    for shot in &capture.shots {
        match shoot(manifest, &session, setup, shot) {
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
                written.push(taken.written);
            }
            Err(failure) => {
                outcome = Err(failure);
                break;
            }
        }
    }
    // Counted before the sheet is appended, so the report can hand back the shots alone.
    let shots = written.len();
    // Two is the fewest that gather into anything: a sheet of one panel is that panel
    // with a caption, so it is worth saying nothing was written rather than writing it.
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
    // Written whether or not the run finished: the shots that did land are on disk,
    // and this is where a reader learns the rest were meant to and didn't.
    if let Some(out) = &capture.report {
        let written_report = RunReport {
            complete: outcome.is_ok(),
            failed: outcome.as_ref().err().map(Diagnostic::plain),
            requested,
            shots: written[..shots]
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
                })
                .collect(),
            sheet: gathered.as_deref(),
            warnings: skipped.iter().map(Diagnostic::plain).collect(),
        };
        if let Err(failure) = write_report(&written_report, out)
            && outcome.is_ok()
        {
            outcome = Err(failure);
        }
    }
    // Reported before the failure is returned: the shots that did run are on disk either way,
    // and an unlisted stale PNG is one a reader will trust.
    report(&written, skipped.as_ref());
    outcome
}

/// Write what the run came to as JSON, for something other than a person to read.
///
/// The sheet is named separately rather than listed among the shots: it has no scene behind it,
/// so `settled` and `frames` would be answers to questions nobody asked of it.
fn write_report(report: &RunReport<'_>, out: &Utf8Path) -> Result<(), Diagnostic> {
    let json = serde_json::to_string_pretty(report)
        .map_err(|e| Diagnostic::from(format!("build the report: {e}")))?;
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| Diagnostic::from(format!("create `{parent}`: {e}")))?;
    }
    std::fs::write(out, json).map_err(|e| Diagnostic::from(format!("write `{out}`: {e}")))?;
    Ok(())
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
        bytes: std::fs::metadata(out).map(|file| file.len()).unwrap_or(0),
        // A sheet is drawn once from images already taken; there is nothing for it to settle.
        frames: Frames {
            drawn: 1,
            motion: Motion::Settled,
        },
    })
}

/// A PNG this run produced, held back so the whole set can be reported in one block.
struct OutputFile {
    path: Utf8PathBuf,
    size: Size,
    /// Device pixels to the point, which `size` is already multiplied by.
    scale: f32,
    bytes: u64,
    frames: Frames,
}

/// A run's outcome as JSON, for a loop nobody is watching.
///
/// Its own types rather than `#[derive(Serialize)]` on the ones above, so the file a consumer
/// parses is a stated format and not whatever the internals happen to be called this week.
#[derive(serde::Serialize)]
struct RunReport<'a> {
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
    /// Absent unless a sheet was gathered — asking for one that was skipped leaves a warning,
    /// not a path.
    #[serde(skip_serializing_if = "Option::is_none")]
    sheet: Option<&'a Utf8Path>,
    /// What the run got on with despite, in the words it printed.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
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

/// The images the run wrote, and anything it has to say about one it didn't.
///
/// Each block opens on a blank line and closes on none.
/// A note under the list is then set off by one line rather than two,
/// and the shell prompt lands one line under whichever block came last.
fn report(written: &[OutputFile], skipped: Option<&Diagnostic>) {
    if !written.is_empty() {
        let good = Style::new().bold().fg_color(Some(AnsiColor::Green.into()));
        let headline = format!(
            "{} {} image{}",
            paint(good, "Gallery wrote"),
            written.len(),
            if written.len() == 1 { "" } else { "s" }
        );
        let total = human(written.iter().map(|w| w.bytes).sum());
        anstream::println!("\n{}", frame(&headline, &rows(written), Some(&total)));
    }
    if let Some(note) = skipped {
        note.report();
    }
}

/// A byte count the way a reader wants it: a figure and a unit, not eight digits to count through.
fn human(bytes: u64) -> String {
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

/// One row per image: the path, then its size in a column of its own,
/// so a set of shots can be scanned for the odd one out rather than read line by line.
fn rows(written: &[OutputFile]) -> Vec<String> {
    let aside = Style::new().dimmed();
    let column = written
        .iter()
        .map(|w| w.path.as_str().chars().count())
        .max()
        .unwrap_or_default();
    // The width is padded on its own account, so the `×` holds still
    // instead of drifting with the number of digits ahead of it.
    let digits = written
        .iter()
        .map(|w| w.size.width.to_string().len())
        .max()
        .unwrap_or_default();
    written
        .iter()
        .map(|w| {
            let pad = " ".repeat(column - w.path.as_str().chars().count());
            let size = format!("{:>digits$}×{}", w.size.width, w.size.height);
            let still = match w.frames.motion {
                Motion::Settled => "",
                Motion::StillMoving => "  still moving",
            };
            format!(
                "{}{pad}   {}{}",
                link(&w.path),
                paint(aside, &size),
                paint(aside, still)
            )
        })
        .collect()
}

/// The painter a glow capture draws with, which is the one a scene has to register
/// its offscreen texture on. `None` under wgpu, whose capture needs no GL context.
pub(crate) type GlPainter = std::rc::Rc<std::cell::RefCell<egui_glow::Painter>>;

/// A harness drawing `app` headlessly at `size` points, `scale` device pixels to the point,
/// on the renderer the consumer configured.
///
/// The renderer is set before the app is built: the harness runs `setup_eframe` first,
/// and that is what leaves a GL context on the `CreationContext` the closure reads.
/// The painter comes out alongside it: a scene registers its offscreen texture
/// with the very one that draws it.
///
/// # Errors
/// On a platform with no headless glow capture, when glow is what was configured.
pub(crate) fn open<A: eframe::App + 'static>(
    size: egui::Vec2,
    scale: f32,
    session: &Session,
    setup: &impl Fn(&egui::Context),
    app: impl FnOnce(&eframe::CreationContext<'_>, Option<GlPainter>) -> A,
) -> Result<egui_kittest::Harness<'static, A>, Diagnostic> {
    let builder = egui_kittest::Harness::builder()
        .with_size(size)
        .with_pixels_per_point(scale)
        // The predictable options a snapshot wants — software texture filtering, no dithering —
        // with the sample count spelled from the one place the window reads it too.
        .with_render_options(eframe::egui_wgpu::RendererOptions {
            msaa_samples: crate::MSAA_SAMPLES,
            ..eframe::egui_wgpu::RendererOptions::PREDICTABLE
        });
    let (painter, builder) = match session {
        #[cfg(not(target_vendor = "apple"))]
        Session::Glow(capture) => (Some(capture.painter()), builder.renderer(capture.clone())),
        // Eager, unlike the default lazy renderer, which builds its wgpu state on the first
        // `render()` — by which time the app has been drawing for frames. Built up front,
        // the state reaches the `CreationContext`, so `SceneCtx::render_state` is `Some`
        // here exactly as it is in a window. Same render options either way.
        Session::Wgpu => (None, builder.wgpu()),
    };
    Ok(builder.build_eframe(|cc| {
        install_context(cc, setup);
        app(cc, painter)
    }))
}

/// The GL a run paints through, opened once and lent to every shot.
///
/// A type rather than a convention, because [`SharedCapture`] is only correct
/// when one of it covers the whole run — its own docs have why.
pub(crate) enum Session {
    #[cfg(not(target_vendor = "apple"))]
    Glow(SharedCapture),
    Wgpu,
}

impl Session {
    /// # Errors
    /// When glow is configured and the platform has no headless GL to give.
    pub(crate) fn open(renderer: Renderer) -> Result<Self, Diagnostic> {
        match renderer {
            #[cfg(not(target_vendor = "apple"))]
            Renderer::Glow => Ok(Self::Glow(SharedCapture::new()?)),
            #[cfg(target_vendor = "apple")]
            Renderer::Glow => Err(
                Diagnostic::new("this platform has no headless glow capture")
                    .hint("configure `Renderer::Wgpu`, whose capture needs no GL context"),
            ),
            Renderer::Wgpu => Ok(Self::Wgpu),
        }
    }
}

/// Draw `scene` at `size`, applying the shot's knobs as the frames go by.
///
/// Frame one declares the knobs at their defaults; every later frame re-applies the overrides
/// — setting one knob can be what makes the next one exist,
/// and a scene writing its own is overruled.
///
/// # Errors
/// For a key that named no knob by the last frame.
fn draw(
    scene: SceneEntry,
    session: &Session,
    setup: &impl Fn(&egui::Context),
    shot: &Shot,
    size: egui::Vec2,
) -> Result<SceneFrame, Diagnostic> {
    let wgpu_session = matches!(session, Session::Wgpu);
    let mut harness = open(size, shot.scale, session, setup, |cc, painter| {
        let wgpu = cc.wgpu_render_state.clone();
        // Loudly, because the quiet alternative is worse: a capture without the render state
        // still writes a PNG, minus every paint callback the window would have drawn.
        assert!(
            !wgpu_session || wgpu.is_some(),
            "the wgpu harness left no render state on the CreationContext"
        );
        Canvas {
            scene,
            knobs: Vec::new(),
            gl: cc.gl.clone(),
            loader: cc.get_proc_address.clone(),
            painter,
            targets: Vec::new(),
            wgpu,
            passes: Vec::new(),
            wanted: egui::Vec2::ZERO,
        }
    })?;
    let frames = settle(&mut harness, shot)?;
    Ok(SceneFrame { harness, frames })
}

/// A scene drawn and settled, and how it was still moving when the drawing stopped.
struct SceneFrame {
    harness: egui_kittest::Harness<'static, Canvas>,
    frames: Frames,
}

/// What the drawing of one shot came to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Frames {
    /// How many were drawn, counting the first.
    /// A shot too big for the size it asked for is drawn twice, and this counts the second
    /// pass alone — the one that made the captured pixels — not the two added together.
    pub(crate) drawn: u32,
    pub(crate) motion: Motion,
}

/// Frames that must ask for no repaint before a `settle` shot calls the scene still.
///
/// More than one because the signal belongs to the egui context rather than to the scene:
/// a one-shot repaint from anything on the canvas would end the wait early on a scene
/// still moving. Two consecutive quiet frames cost a frame and take that away.
const QUIET_FRAMES: u32 = 2;

/// Draw the frames a shot asks for, applying its overrides as they go by.
///
/// Split out so a resize can run it again on the same harness — [`shoot`] has the why.
///
/// Reports [`Motion::StillMoving`] only for a `settle` shot
/// that reached its frame count mid-animation.
///
/// # Errors
/// For a key that named no knob by the last frame.
fn settle(
    harness: &mut egui_kittest::Harness<'static, Canvas>,
    shot: &Shot,
) -> Result<Frames, Diagnostic> {
    harness.run_steps(1);
    let mut unmatched: Vec<&KnobOverride> = shot.knobs.iter().collect();
    let mut quiet = 0;
    let mut drawn = 1;
    // The floor is defence rather than adjustment: both ways in reject a smaller `frames`,
    // and a ceiling below it would run the loop no times, applying the recipe never.
    for _ in 1..shot.frames.unwrap_or(DEFAULT_FRAMES).max(MIN_FRAMES) {
        apply(&mut harness.state_mut().knobs, &shot.knobs, &mut unmatched)?;
        harness.run_steps(1);
        drawn += 1;
        if shot.settle {
            quiet = if harness.ctx.has_requested_repaint() {
                0
            } else {
                quiet + 1
            };
            if quiet >= QUIET_FRAMES {
                break;
            }
        }
    }
    // The loop ends on a draw, so a scene's own writes would outlive the last apply.
    // One more settles the store on the recipe, which is what `--list-knobs` then reports.
    apply(&mut harness.state_mut().knobs, &shot.knobs, &mut unmatched)?;
    if !unmatched.is_empty() {
        return Err(unknown_knobs(&unmatched, &harness.state().knobs));
    }
    let motion = if !shot.settle || quiet >= QUIET_FRAMES {
        Motion::Settled
    } else {
        Motion::StillMoving
    };
    Ok(Frames { drawn, motion })
}

/// Draw one shot: resolve its scene, draw it at a size that holds it, then capture.
fn shoot(
    manifest: &Manifest,
    session: &Session,
    setup: &impl Fn(&egui::Context),
    shot: &Shot,
) -> Result<Option<ShotOutput>, Diagnostic> {
    let scene = *resolve_scene(&manifest.scenes, &shot.scene)?;
    let SceneFrame {
        mut harness,
        mut frames,
    } = draw(scene, session, setup, shot, shot.size)?;

    // The canvas scrolls, so a scene that outgrew its size would be cropped to it.
    // A shot names the size to lay out at, not how much of the result to keep,
    // so the second drawing gets the size the first one asked for.
    let wanted = harness.state().wanted;
    let fitting = egui::vec2(
        shot.size.x.max(wanted.x.ceil()),
        shot.size.y.max(wanted.y.ceil()),
    );
    if fitting != shot.size {
        // Resized rather than drawn again on a fresh harness, which would take the GL context with it.
        // A scene that cached anything against that context — a femtovg canvas, a compiled shader —
        // would then be drawing into nothing, and the shot would come back with the egui parts alone.
        // Reuse also keeps two EGL contexts from being alive at once, which the teardown was for.
        harness.set_size(fitting);
        frames = settle(&mut harness, shot)?;
    }

    if shot.list {
        for knob in &harness.state().knobs {
            println!("{}", describe(knob));
        }
    }
    if shot.template {
        print!(
            "{}",
            capture_template(&scene, &harness.state().knobs, shot.size, shot.scale)
        );
    }
    if let Some(out) = &shot.out {
        let drawn = harness.state().wanted;
        let image = harness
            .render()
            .map_err(|reason| format!("render `{}`: {reason}", scene.name))?;
        let image = if shot.trim {
            trim(image, drawn, shot.scale)
        } else {
            image
        };
        write_png(&image, out)?;
        let written = OutputFile {
            path: landed(out),
            size: Size {
                width: image.width(),
                height: image.height(),
            },
            scale: shot.scale,
            bytes: std::fs::metadata(out).map(|file| file.len()).unwrap_or(0),
            frames,
        };
        return Ok(Some(ShotOutput { written, image }));
    }
    Ok(None)
}

/// What one shot came to: the PNG it wrote, and the image itself for a sheet that wants it.
struct ShotOutput {
    written: OutputFile,
    image: image::RgbaImage,
}

/// Crop the background a roomy `size` leaves around the canvas.
///
/// The size stays as asked — `Fill` stages and any breakpoint lay out against it —
/// so only the image is cut. `drawn` is what the canvas came to in points,
/// which `scale` turns into the pixels the crop is measured in.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a canvas is a few thousand non-negative pixels at most"
)]
fn trim(image: image::RgbaImage, drawn: egui::Vec2, scale: f32) -> image::RgbaImage {
    let drawn = drawn * scale;
    let width = (drawn.x.ceil() as u32).clamp(1, image.width());
    let height = (drawn.y.ceil() as u32).clamp(1, image.height());
    if (width, height) == image.dimensions() {
        return image;
    }
    image::imageops::crop_imm(&image, 0, 0, width, height).to_image()
}

/// The canvas as the entire app: one panel filled like
/// the shell's, and the scene in it.
///
/// No sidebar, no controls, no header — a render stays stable
/// when chrome that isn't the component changes.
///
/// The `gl` fields are `Some` only under a glow capture,
/// and mirror what `Gallery` holds in a window.
struct Canvas {
    scene: SceneEntry,
    knobs: Vec<Knob>,
    gl: Option<std::sync::Arc<glow::Context>>,
    loader: Option<GlLoader>,
    painter: Option<GlPainter>,
    targets: Vec<RenderTarget>,
    /// `Some` only under a wgpu capture — [`SceneCtx::render_state`](crate::SceneCtx::render_state),
    /// as the window's `Frame` carries it.
    wgpu: Option<eframe::egui_wgpu::RenderState>,
    /// This scene's cached `render_pass` targets, as the shell keeps them per scene.
    passes: Vec<crate::PassTarget>,
    /// What the last frame's canvas came to — the size a shot has to be at least as big as.
    wanted: egui::Vec2,
}

impl eframe::App for Canvas {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Split so the painter and the targets can be borrowed at once.
        let Self {
            scene,
            knobs,
            gl,
            loader,
            painter,
            targets,
            wgpu,
            passes,
            wanted,
        } = self;
        let gl_deps = match (loader.clone(), gl.as_deref(), painter.as_ref()) {
            (Some(loader), Some(gl), Some(painter)) => Some(GlDeps {
                loader,
                gl,
                // Through the painter rather than the `Frame`, which a harness builds
                // without the glow hook that nothing outside eframe can supply.
                register: Box::new(|texture| painter.borrow_mut().register_native_texture(texture)),
                targets,
            }),
            // Under wgpu, `SceneCtx::offscreen` draws its "needs the glow renderer" hint, as in a window.
            _ => None,
        };
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(PANEL_BG))
            .show(ui, |ui| {
                let wgpu_deps = wgpu.clone().map(|state| crate::WgpuDeps {
                    state,
                    targets: passes,
                });
                *wanted = render_canvas(ui, scene, knobs, gl_deps, wgpu_deps);
            });
    }
}

/// Apply every override whose knob the scene has declared,
/// and cross the ones that landed off `unmatched` for good —
/// a knob another override later hides must not put its key back.
///
/// Takes the whole list each frame: re-applying is what keeps the recipe's word the last one.
/// A key matching nothing yet is not an error
/// — it may name a knob that appears only once another is set.
///
/// A key matching a knob that can't take the value fails straight away,
/// as does one matching several knobs, which no later frame can disambiguate.
fn apply<'shot>(
    knobs: &mut [Knob],
    overrides: &'shot [KnobOverride],
    unmatched: &mut Vec<&'shot KnobOverride>,
) -> Result<(), Diagnostic> {
    for over in overrides {
        if let Some(at) = find(knobs, &over.key)? {
            set(&mut knobs[at], &over.value)?;
            unmatched.retain(|left| !std::ptr::eq(*left, over));
        }
    }
    Ok(())
}

/// Which knob a key names.
///
/// An exact label wins, so one copied out of `--list-knobs` always works
/// even when it carries regex punctuation — `width (chars)` reads as a group, not as itself.
/// Otherwise the key is a case-insensitive regex searched against every label.
///
/// # Errors
/// If the key matches more than one knob, or isn't a valid regex.
fn find(knobs: &[Knob], key: &str) -> Result<Option<usize>, Diagnostic> {
    if let Some(exact) = knobs.iter().position(|knob| label(knob) == key) {
        return Ok(Some(exact));
    }
    let regex = regex::RegexBuilder::new(key)
        .case_insensitive(true)
        .build()
        .map_err(|e| Diagnostic::new(format!("knob key `{key}` is not a regex: {e}")))?;
    let matched: Vec<usize> = knobs
        .iter()
        .enumerate()
        .filter(|(_, knob)| regex.is_match(label(knob)))
        .map(|(at, _)| at)
        .collect();
    match matched.as_slice() {
        [one] => Ok(Some(*one)),
        [] => Ok(None),
        several => Err(Diagnostic::new(format!(
            "knob key `{key}` matches {} knobs",
            several.len()
        ))
        .candidates(several.iter().map(|at| format!("{:?}", label(&knobs[*at]))))
        .hint("spell one label out in full — an exact label always wins over the regex")),
    }
}

/// Set one knob from the text a recipe or command line gave for it.
///
/// # Errors
/// A message naming the knob, its kind, and what that kind accepts.
fn set(knob: &mut Knob, raw: &str) -> Result<(), String> {
    let label = label(knob).to_owned();
    let bad = |kind: &str, accepts: &str| {
        Err(format!(
            "knob `{label}` is a {kind}; `{raw}` is not {accepts}"
        ))
    };
    match knob {
        Knob::Text { value, .. } => *value = raw.to_owned(),
        Knob::Toggle { value, .. } => match raw {
            "true" => *value = true,
            "false" => *value = false,
            _ => return bad("toggle", "`true` or `false`"),
        },
        Knob::Slider {
            value, min, max, ..
        } => match raw.parse::<f32>() {
            Ok(parsed) if (*min..=*max).contains(&parsed) => *value = parsed,
            Ok(parsed) => {
                return Err(format!(
                    "knob `{label}` takes {min} ..= {max}; {parsed} is outside it"
                ));
            }
            Err(_) => return bad("slider", "a number"),
        },
        Knob::Color { value, .. } => match egui::Color32::from_hex(raw) {
            Ok(parsed) => *value = parsed,
            Err(_) => return bad("colour", "a hex colour (#RGB, #RGBA, #RRGGBB or #RRGGBBAA)"),
        },
        Knob::Select { value, options, .. } => *value = choice(options, raw, &label)?,
        Knob::Pad2D {
            x,
            y,
            min_x,
            max_x,
            min_y,
            max_y,
            ..
        } => {
            let Some((raw_x, raw_y)) = raw.split_once(',') else {
                return bad("2-axis pad", "`x,y`");
            };
            *x = axis(raw_x, *min_x, *max_x, &label, 'x')?;
            *y = axis(raw_y, *min_y, *max_y, &label, 'y')?;
        }
        Knob::Group { .. } => {
            return Err(format!("`{label}` is a group heading and carries no value"));
        }
    }
    Ok(())
}

/// Which option a choice knob should select.
///
/// By option label, which survives a reordering and reads far better than a number.
/// An index still works as a fallback, and the label wins when the options are themselves numbers.
fn choice(options: &[String], raw: &str, label: &str) -> Result<usize, String> {
    if let Some(found) = options.iter().position(|option| option == raw) {
        return Ok(found);
    }
    match raw.parse::<usize>() {
        Ok(index) if index < options.len() => Ok(index),
        _ => Err(format!(
            "knob `{label}` has no option `{raw}`. Options: {}",
            options.join(" | ")
        )),
    }
}

/// One axis of a 2-axis pad, range-checked against that axis's own bounds.
fn axis(raw: &str, min: f32, max: f32, label: &str, which: char) -> Result<f32, String> {
    match raw.trim().parse::<f32>() {
        Ok(parsed) if (min..=max).contains(&parsed) => Ok(parsed),
        Ok(parsed) => Err(format!(
            "knob `{label}` takes {min} ..= {max} on {which}; {parsed} is outside it"
        )),
        Err(_) => Err(format!(
            "knob `{label}` is a 2-axis pad; `{raw}` is not a number"
        )),
    }
}

/// Every knob carries one, and it is its identity in the store.
fn label(knob: &Knob) -> &str {
    match knob {
        Knob::Text { label, .. }
        | Knob::Slider { label, .. }
        | Knob::Toggle { label, .. }
        | Knob::Color { label, .. }
        | Knob::Select { label, .. }
        | Knob::Pad2D { label, .. }
        | Knob::Group { label } => label,
    }
}

/// One knob as `--list-knobs` prints it: the accessor that declared it, the label quoted (they contain
/// spaces, and the caller has to copy it into a recipe), the current value, and what else it accepts.
fn describe(knob: &Knob) -> String {
    match knob {
        Knob::Text { label, value } => format!("text    {label:?} = {value:?}"),
        Knob::Toggle { label, value } => format!("toggle  {label:?} = {value}"),
        Knob::Color { label, value } => format!("color   {label:?} = {}", value.to_hex()),
        Knob::Group { label } => format!("group   {label:?}"),
        Knob::Slider {
            label,
            value,
            min,
            max,
            step,
        } => format!("slider  {label:?} = {value}  ({min} ..= {max}, step {step})"),
        Knob::Select {
            label,
            value,
            options,
            style,
        } => format!(
            "{:<7} {label:?} = {:?}  ({})",
            accessor(*style),
            options.get(*value).map_or("", String::as_str),
            options.join(" | ")
        ),
        Knob::Pad2D {
            label,
            x,
            y,
            min_x,
            max_x,
            min_y,
            max_y,
            ..
        } => format!("pad2d   {label:?} = {x},{y}  (x {min_x} ..= {max_x}, y {min_y} ..= {max_y})"),
    }
}

/// The `SceneCtx` method that declares a choice in this style
/// — what the caller reads in the scene.
fn accessor(style: ChoiceStyle) -> &'static str {
    match style {
        ChoiceStyle::Dropdown => "select",
        ChoiceStyle::Radio => "radio",
        ChoiceStyle::Buttons => "buttons",
    }
}

/// Keys that named a knob no frame ever declared.
/// Listing what the scene does declare turns a typo
/// or a stale label into a one-step fix.
fn unknown_knobs(pending: &[&KnobOverride], declared: &[Knob]) -> Diagnostic {
    let named = pending
        .iter()
        .map(|over| format!("`{}`", over.key))
        .collect::<Vec<_>>()
        .join(", ");
    Diagnostic::new(format!("this scene declares no knob matching {named}"))
        .candidates(declared.iter().map(|knob| format!("{:?}", label(knob))))
        .hint("`--list-knobs` prints these with their kinds and current values")
}

/// Always PNG, whatever the path is called: the caller picked
/// a filename, not a format, and a missing or unexpected extension
/// shouldn't fail a render that already succeeded.
fn write_png(image: &image::RgbaImage, out: &Utf8Path) -> Result<(), String> {
    if let Some(parent) = out.parent().filter(|parent| !parent.as_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| format!("create `{parent}`: {e}"))?;
    }
    image
        .save_with_format(out, image::ImageFormat::Png)
        .map_err(|e| format!("write `{out}`: {e}"))
}

/// Where a written file actually is, so a recipe's `out = "../renders"` is reported
/// as the place its bytes landed rather than as the way it was spelled.
/// Only correct once the file exists, since resolving it reads the filesystem.
fn landed(out: &Utf8Path) -> Utf8PathBuf {
    out.canonicalize_utf8().unwrap_or_else(|_| out.to_owned())
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

/// A capture recipe: the shots to render,
/// and the defaults they fall back on.
///
/// Unknown keys are rejected rather than ignored
/// — a misspelt field would otherwise render a clean
/// picture of the wrong state, which is worse than no picture.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Recipe {
    /// Where the PNGs go, relative to the recipe. `--out` overrides it.
    out: Option<Utf8PathBuf>,
    /// Canvas size for any shot that doesn't state its own.
    size: Option<String>,
    /// Device pixels to the point for any shot that doesn't state its own.
    /// One for one unless asked otherwise.
    scale: Option<f32>,
    /// Gather every shot onto one image here, alongside their own PNGs.
    /// Off unless asked for.
    sheet: Option<Utf8PathBuf>,
    /// Crop each PNG to what its canvas drew.
    /// On by default, and a shot can override it.
    trim: Option<bool>,
    /// Shoot each scene as soon as it stops asking to be redrawn.
    /// Off by default, and a shot can override it.
    settle: Option<bool>,
    /// Write what the run came to as JSON here. Off unless asked for.
    report: Option<Utf8PathBuf>,
    #[serde(default, rename = "shot")]
    shots: Vec<RecipeShot>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RecipeShot {
    /// The shot's identity, and the PNG's filename.
    name: String,
    scene: String,
    size: Option<String>,
    scale: Option<f32>,
    frames: Option<u32>,
    trim: Option<bool>,
    settle: Option<bool>,
    #[serde(default)]
    knobs: BTreeMap<String, toml::Value>,
}

/// Read a capture recipe into the shots it describes,
/// with `out` overriding the recipe's own.
///
/// # Errors
/// If the file can't be read or parsed, declares no shots,
/// gives two shots the same name, leaves a shot with no size,
/// or has nowhere to write.
pub(crate) fn read_recipe(path: &Utf8Path, out: Option<&Utf8Path>) -> Result<Capture, Diagnostic> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("read `{path}`: {e}"))?;
    let recipe: Recipe = toml::from_str(&text).map_err(|e| format!("parse `{path}`: {e}"))?;
    if recipe.shots.is_empty() {
        return Err(Diagnostic::new(format!("`{path}` declares no `[[shot]]`"))
            .hint("`--init-capture` prints one, with the scene's knobs already filled in"));
    }
    // The recipe's own `out` is relative to it, so the file works
    // from any directory; `--out` is the caller's, relative
    // to wherever they ran from.
    let base = match (out, &recipe.out) {
        (Some(given), _) => given.to_owned(),
        (None, Some(own)) => path.parent().unwrap_or(Utf8Path::new(".")).join(own),
        (None, None) => {
            return Err(Diagnostic::new(format!("`{path}` sets no `out`")).hint(
                "give the recipe a root-level `out = \"<dir>\"`, or pass --out on the command line",
            ));
        }
    };
    let shots = recipe
        .shots
        .iter()
        .enumerate()
        .map(|(at, shot)| {
            if let Some(clash) = recipe.shots[..at]
                .iter()
                .find(|before| before.name == shot.name)
            {
                return Err(format!(
                    "two shots are both named `{}`, so one would overwrite the other",
                    clash.name
                ));
            }
            let size = shot.size.as_ref().or(recipe.size.as_ref()).ok_or_else(|| {
                format!(
                    "shot `{}` has no `size`, and the recipe sets no default",
                    shot.name
                )
            })?;
            check_frames(shot.frames).map_err(|e| format!("shot `{}`: {e}", shot.name))?;
            let scale = shot.scale.or(recipe.scale).unwrap_or(DEFAULT_SCALE);
            check_scale(scale).map_err(|e| format!("shot `{}`: {e}", shot.name))?;
            Ok(Shot {
                scene: shot.scene.clone(),
                out: Some(base.join(format!("{}.png", shot.name))),
                size: parse_size(size).map_err(|e| format!("shot `{}`: {e}", shot.name))?,
                scale,
                knobs: shot
                    .knobs
                    .iter()
                    .map(|(key, value)| {
                        Ok(KnobOverride {
                            key: key.clone(),
                            value: scalar(key, value)?,
                        })
                    })
                    .collect::<Result<_, String>>()?,
                frames: shot.frames,
                trim: shot.trim.or(recipe.trim).unwrap_or(true),
                settle: shot.settle.or(recipe.settle).unwrap_or(false),
                list: false,
                template: false,
            })
        })
        .collect::<Result<Vec<_>, String>>()
        .map_err(Diagnostic::from)?;
    Ok(Capture {
        shots,
        // Beside the shots it gathers, wherever those landed.
        sheet: recipe.sheet.map(|name| base.join(name)),
        report: recipe.report.map(|name| base.join(name)),
    })
}

/// A knob value as TOML holds it, flattened to the text the per-kind
/// parser reads — so a recipe can write `night = true`
/// and `speed = 1.5` rather than quoting everything.
///
/// # Errors
/// For a value no knob could take: an array, a table, a date.
fn scalar(key: &str, value: &toml::Value) -> Result<String, String> {
    match value {
        toml::Value::String(text) => Ok(text.clone()),
        toml::Value::Integer(number) => Ok(number.to_string()),
        toml::Value::Float(number) => Ok(number.to_string()),
        toml::Value::Boolean(flag) => Ok(flag.to_string()),
        other => Err(format!(
            "knob `{key}` takes a string, number or boolean, not {}",
            other.type_str()
        )),
    }
}

/// A capture recipe for one scene, with every knob written out at the value it currently holds.
///
/// Starting from the real values is the point.
/// The generated file renders exactly what `--render` would,
/// so the first edit is the state you were after — nothing has to be looked up.
/// Labels arrive already quoted the way TOML needs them.
fn capture_template(scene: &SceneEntry, knobs: &[Knob], size: egui::Vec2, scale: f32) -> String {
    let (width, height) = (size.x, size.y);
    let (name, key) = (slug(scene.name), scene_key(scene));
    // Stated only when it was asked for, so the ordinary recipe carries no line
    // about a magnification nobody wants — and a `--scale` run still generates
    // what it just rendered.
    let magnified = if scale == DEFAULT_SCALE {
        String::new()
    } else {
        format!("scale = {scale}\n")
    };
    let mut out = formatdoc! {r#"
        # Generated by `--init-capture`: every knob at the value its scene declared.
        # Renders what `--render` alone would — change a value and it renders something else.
        out = "renders"
        size = "{width}x{height}"
        {magnified}# Uncomment once there is a second shot: gathers them onto one captioned image.
        # sheet = "sheet.png"

        [[shot]]
        name = {name:?}
        scene = {key:?}
    "#};
    // A `[shot.knobs]` table rather than an inline one,
    // so a long list stays one knob per line and any single knob can be commented out.
    // Groups are headings carrying no value, so they don't count towards needing the table at all.
    if knobs.iter().any(|knob| !matches!(knob, Knob::Group { .. })) {
        out.push_str("\n[shot.knobs]\n");
    }
    for knob in knobs {
        match knob {
            // Kept as a comment: it groups what follows in the panel, and does so here too.
            Knob::Group { label } => {
                let _ = writeln!(out, "\n# {label}");
            }
            _ => {
                let _ = writeln!(out, "{} = {}", toml_key(label(knob)), toml_value(knob));
            }
        }
    }
    out
}

/// A knob's current value as TOML, in the spelling [`set`] reads back.
fn toml_value(knob: &Knob) -> String {
    match knob {
        Knob::Text { value, .. } => format!("{value:?}"),
        Knob::Toggle { value, .. } => value.to_string(),
        Knob::Slider { value, .. } => value.to_string(),
        Knob::Color { value, .. } => format!("{:?}", value.to_hex()),
        // By option label, not by index — the same reason `set` prefers one.
        Knob::Select { value, options, .. } => {
            format!("{:?}", options.get(*value).map_or("", String::as_str))
        }
        Knob::Pad2D { x, y, .. } => format!("{:?}", format!("{x},{y}")),
        Knob::Group { label } => format!("{label:?}"),
    }
}

/// A TOML key: bare where that is legal, quoted otherwise. Most labels carry spaces, and some carry
/// punctuation (`width (chars)`, `centered, -1..1`).
fn toml_key(label: &str) -> String {
    let bare = !label.is_empty()
        && label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if bare {
        label.to_owned()
    } else {
        format!("{label:?}")
    }
}

/// A scene name as a filename: the shot's `name`, which is what the PNG gets called.
fn slug(name: &str) -> String {
    let slug: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    slug.trim_matches('-').to_owned()
}

#[cfg(test)]
mod tests {
    use indoc::indoc;

    use super::*;

    #[test]
    fn a_byte_count_is_stated_in_the_largest_unit_it_fills() {
        assert_eq!(human(0), "0 B");
        assert_eq!(human(1023), "1023 B");
        assert_eq!(human(1024), "1.0 KiB");
        assert_eq!(human(1_572_864), "1.5 MiB");
    }

    #[test]
    fn a_report_lines_the_sizes_up_however_long_the_paths_before_them_are() {
        let written = [
            OutputFile {
                path: "/tmp/a.png".into(),
                size: Size {
                    width: 640,
                    height: 360,
                },
                scale: DEFAULT_SCALE,
                bytes: 1,
                frames: Frames {
                    drawn: 1,
                    motion: Motion::Settled,
                },
            },
            OutputFile {
                path: "/tmp/a-much-longer-name.png".into(),
                size: Size {
                    width: 1280,
                    height: 720,
                },
                scale: DEFAULT_SCALE,
                bytes: 1,
                frames: Frames {
                    drawn: 1,
                    motion: Motion::Settled,
                },
            },
        ];
        let columns: Vec<_> = rows(&written)
            .iter()
            .map(|row| {
                crate::style::plain(row)
                    .chars()
                    .take_while(|c| *c != '×')
                    .count()
            })
            .collect();
        assert_eq!(columns[0], columns[1], "the sizes start in the same column");
    }

    #[test]
    fn a_written_path_is_reported_absolute_with_its_detours_resolved_away() {
        let landed = landed(Utf8Path::new("src/../Cargo.toml"));
        assert!(landed.is_absolute(), "`{landed}` is absolute");
        assert!(!landed.as_str().contains(".."), "`{landed}` kept no detour");
        assert!(landed.ends_with("Cargo.toml"), "`{landed}` names the file");
    }

    /// The overrides a recipe or the command line would have produced.
    fn knobs(pairs: &[(&str, &str)]) -> Vec<KnobOverride> {
        pairs
            .iter()
            .map(|(key, value)| KnobOverride {
                key: (*key).to_owned(),
                value: (*value).to_owned(),
            })
            .collect()
    }

    /// Apply `pairs` to `knobs`, as a frame of [`shoot`] would; returns how many went unmatched.
    fn applied(store: &mut [Knob], pairs: &[(&str, &str)]) -> Result<usize, Diagnostic> {
        let owned = knobs(pairs);
        let mut unmatched: Vec<&KnobOverride> = owned.iter().collect();
        apply(store, &owned, &mut unmatched)?;
        Ok(unmatched.len())
    }

    fn select(label: &str, options: &[&str]) -> Knob {
        Knob::Select {
            label: label.to_owned(),
            value: 0,
            options: options.iter().map(|o| (*o).to_owned()).collect(),
            style: ChoiceStyle::Buttons,
        }
    }

    fn slider(label: &str) -> Knob {
        Knob::Slider {
            label: label.to_owned(),
            value: 1.0,
            min: 0.0,
            max: 2.0,
            step: 0.1,
        }
    }

    #[test]
    fn every_knob_kind_takes_its_value() {
        let mut store = vec![
            Knob::Text {
                label: "caption".to_owned(),
                value: String::new(),
            },
            Knob::Toggle {
                label: "night".to_owned(),
                value: false,
            },
            slider("speed"),
            Knob::Color {
                label: "tint".to_owned(),
                value: egui::Color32::BLACK,
            },
            select("body", &["sedan", "suv"]),
            Knob::Pad2D {
                label: "offset".to_owned(),
                x: 0.0,
                y: 0.0,
                min_x: -1.0,
                max_x: 1.0,
                min_y: -1.0,
                max_y: 1.0,
                invert_y: false,
            },
        ];
        let left = applied(
            &mut store,
            &[
                ("caption", "hi"),
                ("night", "true"),
                ("speed", "1.5"),
                ("tint", "#FF8800"),
                ("body", "suv"),
                ("offset", "0.25,-0.5"),
            ],
        )
        .expect("every key names a declared knob");
        assert_eq!(left, 0, "nothing left pending");

        assert!(matches!(&store[0], Knob::Text { value, .. } if value == "hi"));
        assert!(matches!(store[1], Knob::Toggle { value: true, .. }));
        assert!(matches!(store[2], Knob::Slider { value, .. } if value == 1.5));
        assert!(
            matches!(store[3], Knob::Color { value, .. } if value == egui::Color32::from_rgb(0xFF, 0x88, 0x00))
        );
        assert!(matches!(store[4], Knob::Select { value: 1, .. }));
        assert!(matches!(store[5], Knob::Pad2D { x, y, .. } if x == 0.25 && y == -0.5));
    }

    #[test]
    fn a_key_may_be_a_regex_but_an_exact_label_wins() {
        let mut store = vec![slider("width"), slider("width (chars)")];
        // As a regex `width (chars)` is a group matching `width chars`, which no label is.
        assert_eq!(
            applied(&mut store, &[("width (chars)", "1.5")]).expect("the literal label"),
            0
        );
        assert!(matches!(store[1], Knob::Slider { value, .. } if value == 1.5));
        assert!(
            matches!(store[0], Knob::Slider { value, .. } if value == 1.0),
            "the other knob is untouched"
        );

        let mut pads = vec![slider("speed"), slider("angle")];
        assert_eq!(
            applied(&mut pads, &[("^ANG", "0.5")]).expect("case-insensitive regex"),
            0
        );
        assert!(matches!(pads[1], Knob::Slider { value, .. } if value == 0.5));
    }

    #[test]
    fn a_key_matching_several_knobs_is_an_error_naming_them() {
        let mut store = vec![slider("door front left"), slider("door front right")];
        let message = applied(&mut store, &[("door front", "1.0")])
            .expect_err("ambiguous")
            .plain();
        assert!(message.contains("2 knobs"), "says how many: {message}");
        assert!(message.contains("door front right"), "names them");
    }

    #[test]
    fn a_choice_set_by_index_still_works_but_an_option_label_wins() {
        let mut numeric = vec![select("gear", &["2", "1", "0"])];
        applied(&mut numeric, &[("gear", "1")]).expect("declared");
        assert!(
            matches!(numeric[0], Knob::Select { value: 1, .. }),
            "`1` is the option at index 1, not the index 1 read as a number"
        );

        let mut named = vec![select("body", &["sedan", "suv"])];
        applied(&mut named, &[("body", "1")]).expect("declared");
        assert!(
            matches!(named[0], Knob::Select { value: 1, .. }),
            "no option is spelled `1`, so it falls back to an index"
        );
    }

    #[test]
    fn a_value_the_knob_cannot_take_is_an_error_not_a_silent_default() {
        let cases: [(Knob, &str); 7] = [
            (
                Knob::Toggle {
                    label: "night".to_owned(),
                    value: false,
                },
                "yes",
            ),
            (slider("speed"), "nine"),
            (slider("speed"), "9"),
            (
                Knob::Color {
                    label: "tint".to_owned(),
                    value: egui::Color32::BLACK,
                },
                "orange",
            ),
            (select("body", &["sedan", "suv"]), "coupe"),
            (select("body", &["sedan", "suv"]), "7"),
            (
                Knob::Group {
                    label: "layout".to_owned(),
                },
                "on",
            ),
        ];
        for (knob, value) in cases {
            let key = label(&knob).to_owned();
            let mut store = [knob];
            assert!(
                applied(&mut store, &[(&key, value)]).is_err(),
                "`{key}={value}` should be rejected"
            );
        }
    }

    #[test]
    fn an_unmatched_key_stays_pending_rather_than_erroring_on_the_spot() {
        // It may name a knob that only appears once another is set,
        // so it gets every frame to show up.
        let mut store = vec![Knob::Toggle {
            label: "night".to_owned(),
            value: false,
        }];
        let owned = knobs(&[("night", "true"), ("headlights", "true")]);
        let mut unmatched: Vec<&KnobOverride> = owned.iter().collect();
        apply(&mut store, &owned, &mut unmatched).expect("an unmatched key is not yet a failure");
        assert_eq!(unmatched.len(), 1);
        assert_eq!(unmatched[0].key, "headlights");

        let message = unknown_knobs(&unmatched, &store).plain();
        assert!(message.contains("`headlights`"), "names what was not found");
        assert!(
            message.contains("night"),
            "lists what the scene does declare"
        );

        // A knob that took its value and then vanished stays satisfied: the recipe was honoured.
        store.clear();
        apply(&mut store, &owned, &mut unmatched).expect("an empty store is not a failure");
        assert_eq!(
            unmatched.len(),
            1,
            "`night` landed once and does not come back"
        );
    }

    /// The flip side of re-applying: a regex key resolves anew each frame,
    /// so a knob appearing mid-settle can turn it ambiguous — which errors.
    #[test]
    fn a_key_turning_ambiguous_on_a_later_frame_is_an_error() {
        let mut store = vec![slider("speed")];
        let owned = knobs(&[("spe.*", "1.5")]);
        let mut unmatched: Vec<&KnobOverride> = owned.iter().collect();
        apply(&mut store, &owned, &mut unmatched).expect("one match applies");

        store.push(slider("spectrum"));
        let failure = apply(&mut store, &owned, &mut unmatched)
            .expect_err("two matches can no longer say which knob is meant");
        assert!(failure.plain().contains("matches 2 knobs"));
    }

    #[test]
    fn a_recipe_value_is_reasserted_over_a_scenes_own_write() {
        let mut store = vec![slider("speed")];
        let owned = knobs(&[("speed", "1.5")]);
        let mut unmatched: Vec<&KnobOverride> = owned.iter().collect();
        apply(&mut store, &owned, &mut unmatched).expect("the key names its knob");
        assert!(unmatched.is_empty());

        // Stand in for a scene writing the knob back mid-frame.
        if let Knob::Slider { value, .. } = &mut store[0] {
            *value = 1.9;
        }
        apply(&mut store, &owned, &mut unmatched).expect("re-applying is idempotent");
        assert!(
            matches!(store[0], Knob::Slider { value, .. } if value == 1.5),
            "the recipe's word is the last one"
        );
    }

    /// Two images in one scene are two targets: sharing one made every image show
    /// the last one's pixels, stretched to whatever size that call had asked for.
    ///
    /// Different sizes as well as different colours, since one target would also
    /// have been reallocated between the two and left the first image reading
    /// a texture of the wrong shape.
    #[cfg(not(target_vendor = "apple"))]
    #[test]
    fn each_offscreen_call_in_a_scene_keeps_its_own_target() {
        fn two(ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
            fn fill(target: &crate::Offscreen, rgb: (f32, f32, f32)) {
                let loader = target.gl_loader();
                // SAFETY: the capture made its context current, and `loader` resolves against it.
                let gl =
                    unsafe { glow::Context::from_loader_function_cstr(|symbol| loader(symbol)) };
                // SAFETY: the target's framebuffer is bound for the duration of this closure.
                unsafe {
                    use eframe::glow::HasContext as _;
                    gl.clear_color(rgb.0, rgb.1, rgb.2, 1.0);
                    gl.clear(glow::COLOR_BUFFER_BIT);
                }
            }
            ctx.offscreen(ui, [64_u32, 32], |target| fill(target, (1.0, 0.0, 0.0)));
            ctx.offscreen(ui, [32_u32, 16], |target| fill(target, (0.0, 0.0, 1.0)));
        }
        let manifest = Manifest {
            scenes: vec![SceneEntry {
                render: two,
                name: "two-images",
                module_path: "reference",
                default: true,
                order: 0,
                source: "",
            }],
            groups: Vec::new(),
        };
        let out = Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .expect("a UTF-8 temp dir")
            .join("gallery-two-images")
            .join("two.png");
        let capture = Capture {
            shots: vec![Shot {
                scene: "two-images".to_owned(),
                out: Some(out.clone()),
                size: egui::vec2(200.0, 200.0),
                knobs: Vec::new(),
                frames: None,
                trim: true,
                settle: false,
                scale: DEFAULT_SCALE,
                list: false,
                template: false,
            }],
            sheet: None,
            report: None,
        };
        render(&manifest, Renderer::Glow, &|_: &egui::Context| {}, &capture)
            .expect("the shot renders");

        let png = std::fs::read(&out).expect("the capture was written");
        let image = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
            .expect("a PNG")
            .to_rgba8();
        let mostly = |want: char| {
            image
                .pixels()
                .filter(|p| {
                    let [r, _, b, a] = p.0;
                    a > 128
                        && if want == 'r' {
                            r > 128 && b < 64
                        } else {
                            b > 128 && r < 64
                        }
                })
                .count()
        };
        assert!(
            mostly('r') > 512,
            "the first image keeps its own red, not the second's blue"
        );
        assert!(mostly('b') > 128, "and the second keeps its blue");
    }

    /// A context per shot leaves a scene's cached GL objects dead
    /// from the second shot on — [`SharedCapture`] has the mechanism.
    #[cfg(not(target_vendor = "apple"))]
    #[test]
    fn every_shot_in_a_run_draws_even_when_the_scene_caches_against_the_context() {
        /// Distinctive enough that no other texture in the context is this wide by chance.
        const WIDE: i32 = 37;

        thread_local! {
            // A GL object made once and reused, standing in for a cached femtovg canvas.
            static CACHED: std::cell::Cell<Option<glow::NativeTexture>> =
                const { std::cell::Cell::new(None) };
        }
        fn caches_a_texture(ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
            ctx.offscreen(ui, [40_u32, 40], |target| {
                let loader = target.gl_loader();
                // SAFETY: the capture made its context current, and `loader` resolves against it.
                let gl =
                    unsafe { glow::Context::from_loader_function_cstr(|symbol| loader(symbol)) };
                use eframe::glow::HasContext as _;
                let texture = CACHED.get().unwrap_or_else(|| {
                    // SAFETY: a live context; the name is kept, and its width is the mark read back.
                    let made = unsafe {
                        let made = gl.create_texture().expect("a texture");
                        gl.bind_texture(glow::TEXTURE_2D, Some(made));
                        gl.tex_image_2d(
                            glow::TEXTURE_2D,
                            0,
                            glow::RGBA as i32,
                            WIDE,
                            1,
                            0,
                            glow::RGBA,
                            glow::UNSIGNED_BYTE,
                            glow::PixelUnpackData::Slice(None),
                        );
                        made
                    };
                    CACHED.set(Some(made));
                    made
                });
                // A name proves nothing — a fresh context reissues them from 1, and egui's painter
                // takes them. The width is what says the object is still the one we made.
                //
                // SAFETY: querying a name a dead context issued is the failure under test;
                // the driver reports it rather than trapping.
                let ours =
                    unsafe { gl.get_texture_level_parameter_i32(texture, 0, glow::TEXTURE_WIDTH) };
                // SAFETY: the target's framebuffer is bound for the duration of this closure.
                unsafe {
                    let shade = if ours == WIDE { 1.0 } else { 0.0 };
                    gl.clear_color(0.0, shade, 0.0, 1.0);
                    gl.clear(glow::COLOR_BUFFER_BIT);
                }
            });
        }
        let manifest = Manifest {
            scenes: vec![SceneEntry {
                render: caches_a_texture,
                name: "caching",
                module_path: "reference",
                default: true,
                order: 0,
                source: "",
            }],
            groups: Vec::new(),
        };
        let dir = Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .expect("a UTF-8 temp dir")
            .join("gallery-every-shot");
        let shot = |name: &str| Shot {
            scene: "caching".to_owned(),
            out: Some(dir.join(format!("{name}.png"))),
            size: egui::vec2(120.0, 120.0),
            knobs: Vec::new(),
            frames: None,
            trim: true,
            settle: false,
            scale: DEFAULT_SCALE,
            list: false,
            template: false,
        };
        let capture = Capture {
            shots: vec![shot("one"), shot("two"), shot("three")],
            sheet: None,
            report: None,
        };
        CACHED.set(None);
        render(&manifest, Renderer::Glow, &|_: &egui::Context| {}, &capture)
            .expect("every shot renders");

        let green = |name: &str| {
            let png = std::fs::read(dir.join(format!("{name}.png"))).expect("a capture");
            let image = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
                .expect("a PNG")
                .to_rgba8();
            image
                .pixels()
                .filter(|p| {
                    // Green rather than merely bright, so the stage's pale chrome stays out of it.
                    let [r, g, b, a] = p.0;
                    a > 128 && g > 128 && r < 64 && b < 64
                })
                .count()
        };
        let (one, two, three) = (green("one"), green("two"), green("three"));
        assert!(one > 512, "the first shot draws: {one} green pixels");
        assert_eq!(
            (one, one),
            (two, three),
            "and every later shot draws the same, its cached texture still belonging to a live context"
        );
    }

    /// The pixels a shot writes have to be the ones its overrides produced, for content drawn
    /// The same recipe twice is the same bytes twice, which is what lets an unattended run diff
    /// today's captures against yesterday's and believe a difference.
    ///
    /// A capture's frame times come from the harness's fixed `step_dt` rather than from the clock,
    /// and the scene here reads them — so a wall-clock frame time shows up as a difference between
    /// two runs rather than as anything a reader would notice in one.
    #[cfg(not(target_vendor = "apple"))]
    #[test]
    fn two_runs_of_one_recipe_write_the_same_bytes() {
        fn clock_watching(ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
            let time = ui.input(|i| i.time);
            crate::stage!(ctx, ui, (120, 40), move |ui: &mut egui::Ui| {
                // Bars rather than the number as text: a font would round the difference away.
                let width = (time * 40.0) as f32 % 100.0;
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(100.0, 20.0), egui::Sense::hover());
                ui.painter().rect_filled(
                    egui::Rect::from_min_size(rect.min, egui::vec2(width, 20.0)),
                    0.0,
                    egui::Color32::from_rgb(0x6C, 0x9C, 0xD8),
                );
            });
        }
        let manifest = Manifest {
            scenes: vec![SceneEntry {
                render: clock_watching,
                name: "clock",
                module_path: "reference",
                default: true,
                order: 0,
                source: "",
            }],
            groups: Vec::new(),
        };
        let dir = Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .expect("a UTF-8 temp dir")
            .join("gallery-determinism");
        let run = |name: &str| {
            let out = dir.join(format!("{name}.png"));
            let capture = Capture {
                shots: vec![Shot {
                    scene: "clock".to_owned(),
                    out: Some(out.clone()),
                    size: egui::vec2(200.0, 120.0),
                    knobs: Vec::new(),
                    frames: Some(6),
                    trim: true,
                    settle: false,
                    scale: DEFAULT_SCALE,
                    list: false,
                    template: false,
                }],
                sheet: None,
                report: None,
            };
            render(&manifest, Renderer::Glow, &|_: &egui::Context| {}, &capture)
                .expect("the shot renders");
            std::fs::read(&out).expect("the capture was written")
        };

        let (once, twice) = (run("once"), run("twice"));
        assert_eq!(
            once,
            twice,
            "two runs of one recipe wrote {} and {} bytes of different PNG",
            once.len(),
            twice.len()
        );
    }

    /// Each shot gets its own harness, so egui's own memory cannot cross between them.
    /// A scene's own cache can: it lives in the scenes dylib, which outlives every harness,
    /// and one keyed by the id the scene derives is what this guards.
    ///
    /// The bar counts frames drawn under that id — twice as long if it inherited a neighbour's.
    #[test]
    fn a_shot_captures_the_same_whether_or_not_another_scene_ran_first() {
        thread_local! {
            /// Frames drawn under each derived id — a render target a scene keeps, in miniature.
            static DRAWN: std::cell::RefCell<std::collections::HashMap<egui::Id, f32>> =
                std::cell::RefCell::new(std::collections::HashMap::new());
        }
        fn counts_against_its_id(ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
            let id = ui.next_auto_id();
            crate::stage!(ctx, ui, (120, 40), move |ui: &mut egui::Ui| {
                let frames = DRAWN.with_borrow_mut(|drawn| {
                    let frames = drawn.entry(id).or_insert(0.0);
                    *frames += 1.0;
                    *frames
                });
                // Bars rather than a number: a font would round the difference away.
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(100.0, 20.0), egui::Sense::hover());
                let bar = frames * 8.0;
                assert!(
                    bar < rect.width(),
                    "a bar at {frames} frames has room to grow, so an inherited count \
                     shows longer rather than saturating to the same width"
                );
                ui.painter().rect_filled(
                    egui::Rect::from_min_size(rect.min, egui::vec2(bar, 20.0)),
                    0.0,
                    egui::Color32::from_rgb(0x6C, 0x9C, 0xD8),
                );
            });
        }
        // One body, two identities — which is the whole of what tells the ids apart.
        let scene = |name: &'static str| SceneEntry {
            render: counts_against_its_id,
            name,
            module_path: "reference",
            default: name == "neighbour",
            order: 0,
            source: "",
        };
        let manifest = Manifest {
            scenes: vec![scene("neighbour"), scene("subject")],
            groups: Vec::new(),
        };
        let dir = Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .expect("a UTF-8 temp dir")
            .join("gallery-scene-ids");
        let shot = |scene: &str, out: &Utf8PathBuf| Shot {
            scene: scene.to_owned(),
            out: Some(out.clone()),
            size: egui::vec2(200.0, 120.0),
            knobs: Vec::new(),
            frames: None,
            trim: true,
            settle: false,
            scale: DEFAULT_SCALE,
            list: false,
            template: false,
        };
        // Cleared between the runs, since the two are standing in for two separate processes.
        let run = |shots: Vec<Shot>, out: &Utf8PathBuf| {
            DRAWN.with_borrow_mut(std::collections::HashMap::clear);
            let capture = Capture {
                shots,
                sheet: None,
                report: None,
            };
            render(&manifest, Renderer::Glow, &|_: &egui::Context| {}, &capture)
                .expect("the shots render");
            std::fs::read(out).expect("the capture was written")
        };

        let after = dir.join("after.png");
        let alone = dir.join("alone.png");
        let following = run(
            vec![
                shot("neighbour", &dir.join("neighbour.png")),
                shot("subject", &after),
            ],
            &after,
        );
        let by_itself = run(vec![shot("subject", &alone)], &alone);

        assert_eq!(
            following,
            by_itself,
            "the subject captured {} bytes after its neighbour and {} bytes alone",
            following.len(),
            by_itself.len()
        );
    }

    /// A run hands back what it did as JSON, so an unattended loop reads a file
    /// rather than scraping the text meant for a person.
    ///
    /// The scene never settles, which is the case that matters: the shot is still written,
    /// and `settled: false` is what stops the loop diffing a moment the frame ceiling
    /// landed on against one the scene chose.
    #[test]
    fn a_run_reports_what_it_wrote_and_whether_each_shot_settled() {
        fn restless(_ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
            ui.ctx().request_repaint();
            ui.label("never still");
        }
        let manifest = Manifest {
            scenes: vec![SceneEntry {
                render: restless,
                name: "restless",
                module_path: "reference",
                default: true,
                order: 0,
                source: "",
            }],
            groups: Vec::new(),
        };
        let dir = Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .expect("a UTF-8 temp dir")
            .join("gallery-report");
        let report = dir.join("capture.json");
        let capture = Capture {
            shots: vec![Shot {
                scene: "restless".to_owned(),
                out: Some(dir.join("restless.png")),
                size: egui::vec2(80.0, 40.0),
                knobs: Vec::new(),
                frames: Some(5),
                trim: true,
                settle: true,
                scale: DEFAULT_SCALE,
                list: false,
                template: false,
            }],
            sheet: None,
            report: Some(report.clone()),
        };
        _ = std::fs::remove_file(&report);
        render(&manifest, Renderer::Wgpu, &|_: &egui::Context| {}, &capture)
            .expect("the shot renders");

        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&report).expect("a report"))
                .expect("valid JSON");
        let shot = &json["shots"][0];
        assert_eq!(shot["name"], "restless", "named by its file stem");
        assert_eq!(
            shot["settled"], false,
            "a scene asking to be redrawn forever never settles"
        );
        assert_eq!(shot["frames"], 5, "so it drew every frame it was allowed");
        assert!(
            shot["path"].as_str().is_some_and(|p| p.ends_with(".png")),
            "and points at the image it wrote: {shot}"
        );
        assert_eq!(json["complete"], true, "every shot asked for was written");
        assert_eq!(json["requested"], 1, "and one was asked for");
        assert!(
            json.get("sheet").is_none(),
            "no sheet was asked for, so none is named"
        );
        assert!(
            json.get("failed").is_none() && json.get("warnings").is_none(),
            "nothing went wrong, so nothing is said to have: {json}"
        );
    }

    /// A run that stops partway still reports, and says so — a loop counting the records alone
    /// cannot tell a recipe of one from a recipe of three that failed on the second.
    ///
    /// The sheet is asked for and skipped, which is a warning rather than a failure — it leaves
    /// no path, and the reason belongs in the file, not only in the text a person reads.
    #[test]
    fn a_run_that_fails_partway_reports_what_it_managed_and_what_stopped_it() {
        fn plain(_ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
            ui.label("fine");
        }
        let manifest = Manifest {
            scenes: vec![SceneEntry {
                render: plain,
                name: "plain",
                module_path: "reference",
                default: true,
                order: 0,
                source: "",
            }],
            groups: Vec::new(),
        };
        let dir = Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .expect("a UTF-8 temp dir")
            .join("gallery-partial");
        let report = dir.join("capture.json");
        let shot = |name: &str, knobs: Vec<KnobOverride>| Shot {
            scene: "plain".to_owned(),
            out: Some(dir.join(format!("{name}.png"))),
            size: egui::vec2(60.0, 40.0),
            knobs,
            frames: None,
            trim: true,
            settle: false,
            scale: DEFAULT_SCALE,
            list: false,
            template: false,
        };
        let capture = Capture {
            shots: vec![
                shot("first", Vec::new()),
                // Names a knob the scene never declares, which stops the run on the second shot.
                shot("second", knobs(&[("headlights", "true")])),
                shot("third", Vec::new()),
            ],
            sheet: Some(dir.join("sheet.png")),
            report: Some(report.clone()),
        };
        _ = std::fs::remove_file(&report);
        render(&manifest, Renderer::Wgpu, &|_: &egui::Context| {}, &capture)
            .expect_err("the second shot names no knob");

        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&report).expect("a report even so"))
                .expect("valid JSON");
        assert_eq!(json["complete"], false, "the run did not finish");
        assert_eq!(json["requested"], 3, "three were asked for");
        assert_eq!(
            json["shots"].as_array().map(Vec::len),
            Some(1),
            "one landed"
        );
        assert!(
            json["failed"]
                .as_str()
                .is_some_and(|why| why.contains("headlights")),
            "and the reason names the knob: {json}"
        );
        assert!(
            json.get("sheet").is_none(),
            "a run that stopped gathered no sheet"
        );
    }

    /// The sheet a run does gather is named, so a loop can find it without guessing the filename.
    #[test]
    fn a_report_names_the_sheet_it_gathered() {
        fn plain(_ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
            ui.label("fine");
        }
        let manifest = Manifest {
            scenes: vec![SceneEntry {
                render: plain,
                name: "plain",
                module_path: "reference",
                default: true,
                order: 0,
                source: "",
            }],
            groups: Vec::new(),
        };
        let dir = Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .expect("a UTF-8 temp dir")
            .join("gallery-sheet-report");
        let report = dir.join("capture.json");
        let shot = |name: &str| Shot {
            scene: "plain".to_owned(),
            out: Some(dir.join(format!("{name}.png"))),
            size: egui::vec2(60.0, 40.0),
            knobs: Vec::new(),
            frames: None,
            trim: true,
            settle: false,
            scale: DEFAULT_SCALE,
            list: false,
            template: false,
        };
        let capture = Capture {
            // Two, since a sheet of one panel is that panel and is skipped.
            shots: vec![shot("one"), shot("two")],
            sheet: Some(dir.join("sheet.png")),
            report: Some(report.clone()),
        };
        _ = std::fs::remove_file(&report);
        render(&manifest, Renderer::Wgpu, &|_: &egui::Context| {}, &capture)
            .expect("both shots render");

        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&report).expect("a report"))
                .expect("valid JSON");
        assert!(
            json["sheet"]
                .as_str()
                .is_some_and(|path| path.ends_with("sheet.png")),
            "the gathered sheet is named: {json}"
        );
        assert_eq!(
            json["shots"].as_array().map(Vec::len),
            Some(2),
            "and it is not counted among the shots, having no scene behind it"
        );
    }

    /// A frame count too small to apply a recipe is refused rather than quietly raised.
    #[test]
    fn a_shot_asking_for_fewer_frames_than_a_capture_needs_is_an_error() {
        assert!(check_frames(None).is_ok(), "unset takes the default");
        assert!(check_frames(Some(2)).is_ok(), "two is the least that works");
        let refused = check_frames(Some(1)).expect_err("one cannot apply a recipe");
        assert!(
            refused.contains("declares") && refused.contains("applies"),
            "and says why rather than just refusing: {refused}"
        );
    }

    /// A scene that stops asking to be redrawn is shot then, not at the recipe's frame count —
    /// so a set of scenes that settle at different speeds needs one number, not one per scene.
    ///
    /// The scene asks for `ANIMATED` frames' worth of repaints and then goes quiet,
    /// well inside a generous cap, so settling and hitting the cap are far apart.
    #[test]
    fn a_settling_shot_stops_when_the_scene_goes_quiet_rather_than_at_the_frame_count() {
        /// Frames the scene asks to be redrawn for, counting the first.
        const ANIMATED: u32 = 3;
        /// Far enough past `ANIMATED` that stopping there cannot be the cap doing it.
        const CAP: u32 = 30;

        thread_local! {
            static DREW: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
        }
        fn animates(_ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
            let drawn = DREW.get() + 1;
            DREW.set(drawn);
            if drawn < ANIMATED {
                ui.ctx().request_repaint();
            }
            ui.label("frame");
        }
        let scene = SceneEntry {
            render: animates,
            name: "animates",
            module_path: "reference",
            default: true,
            order: 0,
            source: "",
        };
        let shot = |settle: bool| Shot {
            scene: "animates".to_owned(),
            out: None,
            size: egui::vec2(80.0, 40.0),
            knobs: Vec::new(),
            frames: Some(CAP),
            trim: true,
            settle,
            scale: DEFAULT_SCALE,
            list: false,
            template: false,
        };

        DREW.set(0);
        let settling = draw(
            scene,
            &Session::Wgpu,
            &|_: &egui::Context| {},
            &shot(true),
            shot(true).size,
        )
        .expect("the scene draws");
        let settling_drew = DREW.get();
        assert_eq!(
            settling.frames.motion,
            Motion::Settled,
            "the scene went quiet well before the cap"
        );
        assert!(
            settling.frames.drawn < CAP,
            "and stopped short of the ceiling: {} frames of {CAP}",
            settling.frames.drawn
        );

        DREW.set(0);
        draw(
            scene,
            &Session::Wgpu,
            &|_: &egui::Context| {},
            &shot(false),
            shot(false).size,
        )
        .expect("the scene draws");
        let capped_drew = DREW.get();

        // Draws rather than frames: egui may run a step over more than one pass, so the counts
        // are compared against each other instead of against `CAP`.
        assert!(
            settling_drew * 2 < capped_drew,
            "settling stopped far short of the cap: {settling_drew} draws against {capped_drew}"
        );
    }

    /// A texture the scene registers itself stages the same way up as one gallery owns,
    /// and `showing` hides the slack rather than the content.
    ///
    /// The marker is asymmetric top to bottom, a flip being the failure here and a symmetric
    /// marker passing upside down. The texture is allocated half again as tall as it is shown,
    /// so a crop taken off the wrong end shows the empty half instead.
    #[cfg(not(target_vendor = "apple"))]
    #[test]
    fn an_adopted_texture_stages_upright_with_its_slack_hidden() {
        /// The whole texture; the bottom third is left blank as slack.
        const ALLOCATED: [u32; 2] = [32, 48];
        /// What the scene lays out and asks to be shown.
        const SHOWN: [u32; 2] = [32, 32];

        thread_local! {
            static ADOPTED: std::cell::Cell<Option<egui::TextureId>> =
                const { std::cell::Cell::new(None) };
        }
        fn adopts(ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
            let id = ADOPTED.get().unwrap_or_else(|| {
                let loader = ctx.gl_loader().expect("the glow renderer");
                // SAFETY: the capture made its context current, and `loader` resolves against it.
                let gl =
                    unsafe { glow::Context::from_loader_function_cstr(|symbol| loader(symbol)) };
                use eframe::glow::HasContext as _;
                // Red across the scene's top row, blue across its bottom, the rest transparent —
                // so upright, flipped and mis-cropped each come out different.
                //
                // GL rows run bottom-up while a stage reads top-down, so the shown window
                // is the *last* `SHOWN` rows of this buffer: the scene's top row is the very
                // last one, and the slack sits at the start. Filling from row 0 would put
                // the content in the slack, which is the mistake the flip makes easy.
                let mut pixels = vec![0_u8; (ALLOCATED[0] * ALLOCATED[1] * 4) as usize];
                let row = (ALLOCATED[0] * 4) as usize;
                for x in 0..ALLOCATED[0] as usize {
                    let top = (ALLOCATED[1] as usize - 1) * row + x * 4;
                    pixels[top] = 255;
                    pixels[top + 3] = 255;
                    let bottom = (ALLOCATED[1] - SHOWN[1]) as usize * row + x * 4;
                    pixels[bottom + 2] = 255;
                    pixels[bottom + 3] = 255;
                }
                // SAFETY: a live context; the texture outlives the scene by design.
                let name = unsafe {
                    let name = gl.create_texture().expect("a texture");
                    gl.bind_texture(glow::TEXTURE_2D, Some(name));
                    gl.tex_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        glow::RGBA as i32,
                        ALLOCATED[0] as i32,
                        ALLOCATED[1] as i32,
                        0,
                        glow::RGBA,
                        glow::UNSIGNED_BYTE,
                        glow::PixelUnpackData::Slice(Some(&pixels)),
                    );
                    gl.tex_parameter_i32(
                        glow::TEXTURE_2D,
                        glow::TEXTURE_MIN_FILTER,
                        glow::NEAREST as i32,
                    );
                    gl.tex_parameter_i32(
                        glow::TEXTURE_2D,
                        glow::TEXTURE_MAG_FILTER,
                        glow::NEAREST as i32,
                    );
                    gl.bind_texture(glow::TEXTURE_2D, None);
                    name
                };
                let id = ctx.register_native_texture(
                    std::num::NonZeroU32::new(name.0.get()).expect("a GL name"),
                );
                ADOPTED.set(Some(id));
                id
            });
            ctx.texture_stage(
                ui,
                crate::Stage::Fit,
                crate::StageTexture::new(id, ALLOCATED).showing(SHOWN),
            );
        }
        let manifest = Manifest {
            scenes: vec![SceneEntry {
                render: adopts,
                name: "adopts",
                module_path: "reference",
                default: true,
                order: 0,
                source: "",
            }],
            groups: Vec::new(),
        };
        let out = Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .expect("a UTF-8 temp dir")
            .join("gallery-adopted")
            .join("adopted.png");
        let capture = Capture {
            shots: vec![Shot {
                scene: "adopts".to_owned(),
                out: Some(out.clone()),
                size: egui::vec2(120.0, 120.0),
                knobs: Vec::new(),
                frames: None,
                trim: true,
                settle: false,
                scale: DEFAULT_SCALE,
                list: false,
                template: false,
            }],
            sheet: None,
            report: None,
        };
        ADOPTED.set(None);
        render(&manifest, Renderer::Glow, &|_: &egui::Context| {}, &capture)
            .expect("the shot renders");

        let png = std::fs::read(&out).expect("the capture was written");
        let image = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
            .expect("a PNG")
            .to_rgba8();
        // A hue rather than a bright channel: the stage's chrome is pale grey, which is bright
        // in every channel and would read as both markers at once.
        let topmost = |hue: usize| {
            let other = if hue == 0 { 2 } else { 0 };
            image
                .enumerate_pixels()
                .filter(|(_, _, p)| p.0[3] > 128 && p.0[hue] > 128 && p.0[other] < 64)
                .map(|(_, y, _)| y)
                .min()
                .expect("the marker is in the image")
        };
        let (red, blue) = (topmost(0), topmost(2));
        assert!(
            red < blue,
            "red marks the scene's top row and blue its bottom, so the image is upright: \
             red from y={red}, blue from y={blue}"
        );
    }

    /// The colour-space contract [`crate::Offscreen`] states, in pixels.
    ///
    /// Both halves are asserted because a trap is only useful stated alongside its remedy
    /// — the identity case alone reads as "it just works".
    #[cfg(not(target_vendor = "apple"))]
    #[test]
    fn an_offscreen_decodes_what_is_written_unless_the_scene_encodes_it() {
        /// Away from both ends, where a stray decode moves the value a long way.
        const WROTE: [f32; 3] = [0.2, 0.5, 0.8];

        fn flat(ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
            let fill = |target: &crate::Offscreen, encode: bool| {
                let loader = target.gl_loader();
                // SAFETY: the capture made its context current, and `loader` resolves against it.
                let gl =
                    unsafe { glow::Context::from_loader_function_cstr(|symbol| loader(symbol)) };
                // SAFETY: the target's framebuffer is bound for the duration of this closure.
                unsafe {
                    use eframe::glow::HasContext as _;
                    if encode {
                        gl.enable(glow::FRAMEBUFFER_SRGB);
                    }
                    gl.clear_color(WROTE[0], WROTE[1], WROTE[2], 1.0);
                    gl.clear(glow::COLOR_BUFFER_BIT);
                    if encode {
                        gl.disable(glow::FRAMEBUFFER_SRGB);
                    }
                }
            };
            ctx.offscreen(ui, [32_u32, 32], |target| fill(target, false));
            ctx.offscreen(ui, [32_u32, 32], |target| fill(target, true));
        }
        let manifest = Manifest {
            scenes: vec![SceneEntry {
                render: flat,
                name: "flat",
                module_path: "reference",
                default: true,
                order: 0,
                source: "",
            }],
            groups: Vec::new(),
        };
        let out = Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .expect("a UTF-8 temp dir")
            .join("gallery-srgb")
            .join("flat.png");
        let capture = Capture {
            shots: vec![Shot {
                scene: "flat".to_owned(),
                out: Some(out.clone()),
                size: egui::vec2(80.0, 80.0),
                knobs: Vec::new(),
                frames: None,
                trim: true,
                settle: false,
                scale: DEFAULT_SCALE,
                list: false,
                template: false,
            }],
            sheet: None,
            report: None,
        };
        render(&manifest, Renderer::Glow, &|_: &egui::Context| {}, &capture)
            .expect("the shot renders");

        let png = std::fs::read(&out).expect("the capture was written");
        let image = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
            .expect("a PNG")
            .to_rgba8();
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a colour channel, rounded into the byte it came from"
        )]
        let wrote: Vec<u8> = WROTE.iter().map(|c| (c * 255.0).round() as u8).collect();
        // The two images stack down the canvas; sample the middle of each, clear of the edges.
        let quarter = image.get_pixel(image.width() / 2, image.height() / 4).0;
        let three_quarters = image.get_pixel(image.width() / 2, image.height() * 3 / 4).0;

        assert!(
            quarter[..3]
                .iter()
                .zip(&wrote)
                .all(|(got, sent)| got < sent),
            "written straight in, every channel comes back decoded and darker: \
             wrote {wrote:?}, read {:?}",
            &quarter[..3]
        );
        // Within a bit rather than to the byte: the encode and the decode round independently,
        // and llvmpipe and a GPU driver disagree by one on some channels. A decode moves it by 40.
        assert!(
            three_quarters[..3]
                .iter()
                .zip(&wrote)
                .all(|(got, sent)| got.abs_diff(*sent) <= 1),
            "and written under FRAMEBUFFER_SRGB it survives: wrote {wrote:?}, read {:?}",
            &three_quarters[..3]
        );
        assert_eq!(three_quarters[3], 255, "opaque either way");
    }

    /// The pixels a shot writes have to be the ones its overrides produced, for content drawn
    /// through `offscreen` as much as for egui's own — otherwise a scene drawn
    /// entirely in GL captures its declared defaults whatever the recipe says.
    #[cfg(not(target_vendor = "apple"))]
    #[test]
    fn a_shot_override_reaches_the_pixels_an_offscreen_scene_draws() {
        fn tinted(ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
            let red = ctx.slider("red", 0.0, 0.0, 1.0, 0.0);
            // Staged, as a scene drawn entirely in GL has it — the override has to reach
            // the texture, not just the egui chrome around it.
            ctx.offscreen_stage(ui, crate::Stage::Fit, [40_u32, 40], move |target| {
                let loader = target.gl_loader();
                // SAFETY: the capture made its context current, and `loader` resolves against it.
                let gl =
                    unsafe { glow::Context::from_loader_function_cstr(|symbol| loader(symbol)) };
                // SAFETY: the target's framebuffer is bound for the duration of this closure.
                unsafe {
                    use eframe::glow::HasContext as _;
                    gl.clear_color(red, 0.0, 0.0, 1.0);
                    gl.clear(glow::COLOR_BUFFER_BIT);
                }
            });
        }
        let manifest = Manifest {
            scenes: vec![SceneEntry {
                render: tinted,
                name: "tinted",
                module_path: "reference",
                default: true,
                order: 0,
                source: "",
            }],
            groups: Vec::new(),
        };
        let dir = Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .expect("a UTF-8 temp dir")
            .join("gallery-offscreen-override");
        let shot = |name: &str, knobs: Vec<KnobOverride>| Shot {
            scene: "tinted".to_owned(),
            out: Some(dir.join(format!("{name}.png"))),
            size: egui::vec2(120.0, 120.0),
            knobs,
            frames: None,
            trim: true,
            settle: false,
            scale: DEFAULT_SCALE,
            list: false,
            template: false,
        };
        // Two shots rather than one: the default and the override in a single run,
        // so the pair also says a later shot's knobs are its own.
        let capture = Capture {
            shots: vec![
                shot("default", Vec::new()),
                // The scene declares 0.0, so anything red in this one came from here.
                shot("overridden", knobs(&[("red", "1")])),
            ],
            sheet: None,
            report: None,
        };
        render(&manifest, Renderer::Glow, &|_: &egui::Context| {}, &capture)
            .expect("both shots render");

        let reds = |name: &str| {
            let png = std::fs::read(dir.join(format!("{name}.png"))).expect("a capture");
            let image = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
                .expect("a PNG")
                .to_rgba8();
            image
                .pixels()
                .filter(|p| {
                    // Red rather than merely bright: the stage's own chrome is pale grey,
                    // which a red channel alone would count.
                    let [r, g, b, a] = p.0;
                    a > 128 && r > 128 && g < 64 && b < 64
                })
                .count()
        };
        assert_eq!(reds("default"), 0, "the declared default is not red at all");
        assert!(
            reds("overridden") > 512,
            "and the override drove the offscreen draw: {} red pixels",
            reds("overridden")
        );
    }

    /// A staged image is a call site like any other: its own slot, in the order the scene makes it,
    /// leaving the bare call after it on the slot it already had. The caption reports the image,
    /// which for a fitted stage is the response's own rect.
    #[cfg(not(target_vendor = "apple"))]
    #[test]
    fn a_staged_image_takes_a_slot_of_its_own_and_is_captioned_by_its_size() {
        thread_local! {
            static STAGED: std::cell::Cell<Option<egui::Rect>> =
                const { std::cell::Cell::new(None) };
        }
        fn staged_and_bare(ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
            let blank = |_: &crate::Offscreen| {};
            let shown = ctx.offscreen_stage(ui, crate::Stage::Fit, [48_u32, 24], blank);
            STAGED.set(shown.map(|response| response.rect));
            ctx.offscreen(ui, [16_u32, 16], blank);
        }
        let harness = staged_harness(staged_and_bare, "staged-and-bare");
        assert_eq!(
            harness.state().targets.len(),
            2,
            "the staged image and the bare one each keep their own target"
        );
        let rect = STAGED.get().expect("an open stage shows its image");
        assert_eq!(
            (rect.width(), rect.height()),
            (48.0, 24.0),
            "a fitted stage is the image, and its size is what the caption reads"
        );
    }

    /// Folding a stage away has to skip the GL, not draw it somewhere nobody looks — and the target
    /// stays put, so unfolding does not rebuild it or shift the call sites after it.
    #[cfg(not(target_vendor = "apple"))]
    #[test]
    fn a_folded_stage_costs_no_gl_and_keeps_its_target() {
        use egui_kittest::kittest::Queryable as _;

        thread_local! {
            static DREW: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
        }
        fn counts(ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
            ctx.offscreen_stage(ui, crate::Stage::Fit, [48_u32, 24], |_| {
                DREW.set(DREW.get() + 1);
            });
        }
        let mut harness = staged_harness(counts, "folds");
        assert!(DREW.get() > 0, "an open stage draws");
        assert_eq!(harness.state().targets.len(), 1);

        harness.get_by_label("▾").click();
        harness.run_steps(2);
        let folded = DREW.get();
        harness.run_steps(2);
        assert_eq!(DREW.get(), folded, "a folded stage runs no GL at all");
        assert_eq!(
            harness.state().targets.len(),
            1,
            "and holds its target for when it opens again"
        );
    }

    /// The GL for a test's shots, as a run opens one.
    #[cfg(not(target_vendor = "apple"))]
    fn glow() -> Session {
        Session::open(Renderer::Glow).expect("a headless glow session")
    }

    /// One scene, drawn as a shot would draw it, with the harness left open to poke at.
    #[cfg(not(target_vendor = "apple"))]
    fn staged_harness(
        render: fn(&mut crate::SceneCtx<'_>, &mut egui::Ui),
        name: &'static str,
    ) -> egui_kittest::Harness<'static, Canvas> {
        let scene = SceneEntry {
            render,
            name,
            module_path: "reference",
            default: true,
            order: 0,
            source: "",
        };
        let shot = Shot {
            scene: name.to_owned(),
            out: None,
            size: egui::vec2(200.0, 200.0),
            knobs: Vec::new(),
            frames: None,
            trim: true,
            settle: false,
            scale: DEFAULT_SCALE,
            list: false,
            template: false,
        };
        draw(scene, &glow(), &|_: &egui::Context| {}, &shot, shot.size)
            .expect("the scene draws")
            .harness
    }

    /// A scene may make fewer calls on a later frame — one behind a toggle — and the targets
    /// it stops asking for are kept for when it asks again, rather than freed under
    /// a `TextureId` eframe gives no way to release.
    #[cfg(not(target_vendor = "apple"))]
    #[test]
    fn a_scene_that_stops_staging_an_image_keeps_the_target_for_its_return() {
        thread_local! {
            static BOTH: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
        }
        fn sometimes_two(ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
            let blank = |_: &crate::Offscreen| {};
            ctx.offscreen(ui, [32_u32, 32], blank);
            if BOTH.get() {
                ctx.offscreen(ui, [16_u32, 16], blank);
            }
        }
        let manifest = Manifest {
            scenes: vec![SceneEntry {
                render: sometimes_two,
                name: "sometimes-two",
                module_path: "reference",
                default: true,
                order: 0,
                source: "",
            }],
            groups: Vec::new(),
        };
        let shot = Shot {
            scene: "sometimes-two".to_owned(),
            out: None,
            size: egui::vec2(200.0, 200.0),
            knobs: Vec::new(),
            frames: None,
            trim: true,
            settle: false,
            scale: DEFAULT_SCALE,
            list: false,
            template: false,
        };
        let mut harness = draw(
            manifest.scenes[0],
            &glow(),
            &|_: &egui::Context| {},
            &shot,
            shot.size,
        )
        .expect("both images draw")
        .harness;
        assert_eq!(harness.state().targets.len(), 2, "one target per call site");

        BOTH.set(false);
        harness.run_steps(2);
        assert_eq!(
            harness.state().targets.len(),
            2,
            "the one it stopped staging is held, not dropped"
        );

        BOTH.set(true);
        harness.run_steps(2);
        assert_eq!(
            harness.state().targets.len(),
            2,
            "and taken up again rather than made afresh"
        );
    }

    /// A shot smaller than its scene lays out is drawn twice, and the scene must see the same GL
    /// both times: `scripts/offscreen.scene.rs` tells scenes to build a femtovg canvas once
    /// and keep it, and one built against a context since torn down draws nothing.
    ///
    /// The loader is an `Arc` per capture, so its identity is what says whether the context held.
    #[cfg(not(target_vendor = "apple"))]
    #[test]
    fn a_regrown_capture_keeps_the_gl_the_scene_cached_against() {
        thread_local! {
            static FIRST: std::cell::RefCell<Option<crate::GlLoader>> =
                const { std::cell::RefCell::new(None) };
            static SWAPPED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        }
        fn caches_gl(ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
            ctx.offscreen(ui, [64_u32, 64], |target| {
                let loader = target.gl_loader();
                FIRST.with_borrow_mut(|first| match first {
                    Some(seen) if !std::sync::Arc::ptr_eq(seen, &loader) => SWAPPED.set(true),
                    Some(_) => {}
                    None => *first = Some(loader),
                });
            });
        }
        let manifest = Manifest {
            scenes: vec![SceneEntry {
                render: caches_gl,
                name: "caches-gl",
                module_path: "reference",
                default: true,
                order: 0,
                source: "",
            }],
            groups: Vec::new(),
        };
        let shot = Shot {
            scene: "caches-gl".to_owned(),
            out: None,
            // Well under the 64×64 image plus its padding, so the fitting size differs.
            size: egui::vec2(24.0, 24.0),
            knobs: Vec::new(),
            frames: None,
            trim: true,
            settle: false,
            scale: DEFAULT_SCALE,
            list: false,
            template: false,
        };
        let harness = draw(
            manifest.scenes[0],
            &glow(),
            &|_: &egui::Context| {},
            &shot,
            shot.size,
        )
        .expect("the first pass draws")
        .harness;
        let wanted = harness.state().wanted;
        assert!(
            wanted.x > shot.size.x || wanted.y > shot.size.y,
            "the scene has to outgrow the shot for this to test anything"
        );
        drop(harness);
        // That measuring pass had a capture of its own; only the shot below is under test.
        FIRST.with_borrow_mut(|first| *first = None);
        SWAPPED.set(false);

        shoot(&manifest, &glow(), &|_: &egui::Context| {}, &shot).expect("the shot renders");
        assert!(
            !SWAPPED.get(),
            "the scene saw one GL context for the whole shot"
        );
    }

    /// The precedence rule end to end: a scene nudging its own knob every frame
    /// loses to the recipe, in the pixels and in the store a listing reads.
    #[cfg(not(target_vendor = "apple"))]
    #[test]
    fn a_recipe_outlives_a_scene_writing_its_own_knob() {
        fn self_nudging(ctx: &mut crate::SceneCtx<'_>, _ui: &mut egui::Ui) {
            let speed = ctx.slider("speed", 1.0, 0.0, 100.0, 0.0);
            ctx.set_slider("speed", speed + 1.0);
        }
        let scene = SceneEntry {
            render: self_nudging,
            name: "self-nudging",
            module_path: "reference",
            default: true,
            order: 0,
            source: "",
        };
        let shot = Shot {
            scene: "self-nudging".to_owned(),
            out: None,
            size: egui::vec2(320.0, 200.0),
            knobs: knobs(&[("speed", "5")]),
            frames: None,
            trim: true,
            settle: false,
            scale: DEFAULT_SCALE,
            list: false,
            template: false,
        };
        let harness = draw(scene, &glow(), &|_: &egui::Context| {}, &shot, shot.size)
            .expect("the shot draws")
            .harness;
        assert!(
            matches!(harness.state().knobs[0], Knob::Slider { value, .. } if value == 5.0),
            "every settle frame re-applies the recipe, and one more after the last draw"
        );
    }

    #[test]
    fn a_generated_recipe_states_every_knob_at_the_value_it_holds() {
        let scene = crate::test_support::scene("orbit", "demo::animation", true);
        let knobs = vec![
            Knob::Group {
                label: "motion".to_owned(),
            },
            Knob::Toggle {
                label: "animate".to_owned(),
                value: true,
            },
            Knob::Slider {
                label: "dot size".to_owned(),
                value: 5.0,
                min: 1.0,
                max: 20.0,
                step: 0.5,
            },
            select("body style", &["sedan", "suv"]),
            Knob::Color {
                label: "accent".to_owned(),
                value: egui::Color32::from_rgb(0x4C, 0xAF, 0x50),
            },
        ];
        assert_eq!(
            capture_template(&scene, &knobs, egui::vec2(480.0, 480.0), DEFAULT_SCALE),
            indoc! {r##"
                # Generated by `--init-capture`: every knob at the value its scene declared.
                # Renders what `--render` alone would — change a value and it renders something else.
                out = "renders"
                size = "480x480"
                # Uncomment once there is a second shot: gathers them onto one captioned image.
                # sheet = "sheet.png"

                [[shot]]
                name = "orbit"
                scene = "demo::animation::orbit"

                [shot.knobs]

                # motion
                animate = true
                "dot size" = 5
                "body style" = "sedan"
                accent = "#4caf50ff"
            "##}
        );
    }

    #[test]
    fn a_generated_recipe_reads_back_as_the_state_it_was_generated_from() {
        // The point of generating one: its values are already the spelling the parser takes,
        // so the first edit is the only thing that changes what renders.
        let scene = crate::test_support::scene("knobs", "demo", true);
        let declared = vec![
            Knob::Text {
                label: "caption, long".to_owned(),
                value: "hi there".to_owned(),
            },
            Knob::Toggle {
                label: "night".to_owned(),
                value: true,
            },
            Knob::Slider {
                label: "width (chars)".to_owned(),
                value: 1.5,
                min: 0.0,
                max: 2.0,
                step: 0.1,
            },
            Knob::Color {
                label: "tint".to_owned(),
                value: egui::Color32::from_rgb(0xE8, 0xA3, 0x3D),
            },
            select("body", &["sedan", "suv"]),
            Knob::Pad2D {
                label: "offset".to_owned(),
                x: 0.25,
                y: -0.5,
                min_x: -1.0,
                max_x: 1.0,
                min_y: -1.0,
                max_y: 1.0,
                invert_y: false,
            },
        ];
        let generated = capture_template(&scene, &declared, egui::vec2(320.0, 200.0), 2.0);

        let dir = Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .expect("a UTF-8 temp dir")
            .join("gallery-recipe-round-trip");
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("capture.toml");
        std::fs::write(&path, &generated).expect("write recipe");
        let shots = read_recipe(&path, None)
            .expect("the generated recipe parses")
            .shots;

        // Applied to knobs sitting at other values,
        // every one comes back to where it started.
        let mut store = declared.clone();
        for knob in &mut store {
            match knob {
                Knob::Text { value, .. } => value.clear(),
                Knob::Toggle { value, .. } => *value = false,
                Knob::Slider { value, .. } => *value = 0.0,
                Knob::Color { value, .. } => *value = egui::Color32::BLACK,
                Knob::Select { value, .. } => *value = 0,
                Knob::Pad2D { x, y, .. } => (*x, *y) = (0.0, 0.0),
                Knob::Group { .. } => {}
            }
        }
        assert_eq!(
            shots[0].scale, 2.0,
            "a run that magnified generates a recipe that magnifies, or it renders \
             something other than what it was generated from"
        );
        let mut unmatched: Vec<&KnobOverride> = shots[0].knobs.iter().collect();
        apply(&mut store, &shots[0].knobs, &mut unmatched)
            .expect("every generated key names its knob");
        assert!(unmatched.is_empty(), "no key went unmatched");
        for (before, after) in declared.iter().zip(&store) {
            assert_eq!(
                toml_value(before),
                toml_value(after),
                "knob {:?} round-tripped",
                label(before)
            );
        }
    }

    #[test]
    fn size_parses_both_sides_and_rejects_the_rest() {
        assert_eq!(
            parse_size("1280x720").expect("parses"),
            egui::vec2(1280.0, 720.0)
        );
        assert_eq!(
            parse_size("800X600").expect("parses"),
            egui::vec2(800.0, 600.0)
        );
        assert!(parse_size("1280").is_err());
        assert!(parse_size("0x720").is_err());
        assert!(parse_size("widexhigh").is_err());
    }

    /// A scene that paints through [`crate::SceneCtx::offscreen`]
    /// — the glow-only path, where a GL library draws into a framebuffer
    /// gallery owns and egui shows the result inline.
    fn draws_offscreen(ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
        ctx.offscreen(ui, [32_u32, 32], |target| {
            let loader = target.gl_loader();
            // SAFETY: the capture made its context current, and `loader` resolves against it.
            let gl = unsafe { glow::Context::from_loader_function_cstr(|symbol| loader(symbol)) };
            // SAFETY: the target's framebuffer is bound for the duration of this closure.
            unsafe {
                use eframe::glow::HasContext as _;
                gl.clear_color(1.0, 0.0, 1.0, 1.0);
                gl.clear(glow::COLOR_BUFFER_BIT);
                // A second colour in one corner: a flat field would look right however it went wrong.
                gl.enable(glow::SCISSOR_TEST);
                gl.scissor(0, 0, 16, 16);
                gl.clear_color(0.0, 1.0, 1.0, 1.0);
                gl.clear(glow::COLOR_BUFFER_BIT);
                gl.disable(glow::SCISSOR_TEST);
            }
        });
    }

    /// Several offscreen images at different sizes, so the reference catches one call site's target
    /// standing in for another's — which reads as the same picture repeated, at the wrong shape.
    /// The last is staged, which also holds the chrome a rendered frame gets.
    fn draws_offscreen_slots(ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
        fn flat(target: &crate::Offscreen, rgb: (f32, f32, f32)) {
            let loader = target.gl_loader();
            // SAFETY: the capture made its context current, and `loader` resolves against it.
            let gl = unsafe { glow::Context::from_loader_function_cstr(|symbol| loader(symbol)) };
            // SAFETY: the target's framebuffer is bound for the duration of this closure.
            unsafe {
                use eframe::glow::HasContext as _;
                gl.clear_color(rgb.0, rgb.1, rgb.2, 1.0);
                gl.clear(glow::COLOR_BUFFER_BIT);
            }
        }
        ui.label("wide");
        ctx.offscreen(ui, [64_u32, 24], |target| flat(target, (0.9, 0.2, 0.2)));
        ui.label("tall");
        ctx.offscreen(ui, [24_u32, 48], |target| flat(target, (0.2, 0.4, 0.9)));
        // Staged, so the reference also holds the chrome a rendered frame gets:
        // the checkerboard around it, the size caption, the collapse arrow.
        ui.label("staged");
        ctx.offscreen_stage(ui, crate::Stage::Fit, [40_u32, 40], |target| {
            flat(target, (0.2, 0.8, 0.4));
        });
    }

    /// A scene of the kind the shell is for: prose, and a fixed-size stage on the checkerboard.
    fn a_documented_stage(ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
        ui.heading("Snapshot");
        ui.label("prose above the stage");
        crate::stage!(ctx, ui, (96, 40), |ui: &mut egui::Ui| {
            ui.label("in the stage");
        });
    }

    /// Each way a stage can be sized, so one reference covers the conversions rather than one of them.
    fn every_stage_form(ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
        ui.label("fitted");
        crate::stage!(ctx, ui, |ui: &mut egui::Ui| {
            ui.label("hugs its content");
        });
        ui.label("pinned");
        crate::stage!(ctx, ui, (150, 36), |ui: &mut egui::Ui| {
            ui.label("150×36");
        });
        ui.label("square");
        crate::stage!(ctx, ui, 64, |ui: &mut egui::Ui| {
            ui.label("64");
        });
        // Content past its box, so the reference shows a fixed scrolling stage keeping to the size
        // it declared. The checkerboard closing underneath is the whole point of the picture.
        ui.label("scrolling");
        ctx.stage(
            ui,
            crate::Stage::Fixed(egui::vec2(150.0, 48.0)).scrollable(),
            |ui: &mut egui::Ui| {
                for row in 0..12 {
                    ui.label(format!("row {row}"));
                }
            },
        );
    }

    /// Glyphs the default faces lack: without the bundled Noto fallbacks these are tofu,
    /// which no structural assertion would notice and no reader of the reference could miss.
    fn glyphs_past_the_default_faces(_ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
        ui.heading("→ ∑ ≈ ± °");
        ui.label("arrows ← ↑ ↓ →");
        ui.label("math ∀ ∈ ∞ √");
        ui.label("symbols ✓ ✗ ★ ♦");
    }

    /// Everything drawn here comes from a knob, so its reference is a picture of the override path.
    fn dressed_by_knobs(ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
        let caption = ctx.text("caption", "the default");
        let tint = ctx.color("tint", egui::Color32::WHITE);
        let width = ctx.slider("width", 70.0, 20.0, 260.0, 1.0);
        crate::stage!(ctx, ui, (width, 48.0), |ui: &mut egui::Ui| {
            ui.label(egui::RichText::new(caption).color(tint).size(20.0));
        });
    }

    /// Keeps a GL object between frames, as a scene holding a femtovg canvas does,
    /// and paints by whether that object is still the one it made.
    /// The recipe takes two shots, since one cannot show a context outliving anything.
    fn caches_between_shots(ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
        /// Wide enough that nothing else in the context is this shape by chance.
        const WIDE: i32 = 37;

        thread_local! {
            static KEPT: std::cell::Cell<Option<glow::NativeTexture>> =
                const { std::cell::Cell::new(None) };
        }
        ctx.offscreen(ui, [48_u32, 48], |target| {
            let loader = target.gl_loader();
            // SAFETY: the capture made its context current, and `loader` resolves against it.
            let gl = unsafe { glow::Context::from_loader_function_cstr(|symbol| loader(symbol)) };
            use eframe::glow::HasContext as _;
            let texture = KEPT.get().unwrap_or_else(|| {
                // SAFETY: a live context; the width is the mark read back on later shots.
                let made = unsafe {
                    let made = gl.create_texture().expect("a texture");
                    gl.bind_texture(glow::TEXTURE_2D, Some(made));
                    gl.tex_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        glow::RGBA as i32,
                        WIDE,
                        1,
                        0,
                        glow::RGBA,
                        glow::UNSIGNED_BYTE,
                        glow::PixelUnpackData::Slice(None),
                    );
                    made
                };
                KEPT.set(Some(made));
                made
            });
            // A name proves nothing — a fresh context reissues them from 1. The width is the mark.
            //
            // SAFETY: querying a name a dead context issued is the failure this guards;
            // the driver reports it rather than trapping.
            let ours =
                unsafe { gl.get_texture_level_parameter_i32(texture, 0, glow::TEXTURE_WIDTH) };
            // SAFETY: the target's framebuffer is bound for the duration of this closure.
            unsafe {
                let green = f32::from(u8::from(ours == WIDE));
                gl.clear_color(0.0, green, 0.0, 1.0);
                gl.clear(glow::COLOR_BUFFER_BIT);
            }
        });
    }

    /// The scenes the reference recipe names, standing in for a consumer's `*.scene.rs`.
    fn reference_scenes() -> Manifest {
        let scene = |render, name| SceneEntry {
            render,
            name,
            module_path: "reference",
            default: true,
            order: 0,
            source: "",
        };
        Manifest {
            scenes: vec![
                scene(a_documented_stage, "documented-stage"),
                scene(every_stage_form, "stage-forms"),
                scene(glyphs_past_the_default_faces, "glyph-fallbacks"),
                scene(dressed_by_knobs, "knobs-applied"),
                scene(draws_offscreen, "offscreen-gl"),
                scene(draws_offscreen_slots, "offscreen-slots"),
                scene(caches_between_shots, "offscreen-cached"),
            ],
            groups: Vec::new(),
        }
    }

    /// The one test that asserts what a capture *looks like*;
    /// the rest check that a value arrived.
    ///
    /// Through [`read_recipe`] rather than hand-built [`Shot`]s,
    /// so it covers what someone writes.
    ///
    /// Comparable between machines only because the tests pin
    /// a software rasteriser.
    ///
    /// `UPDATE_SNAPSHOTS=1` takes an intended change.
    #[cfg(not(target_vendor = "apple"))]
    #[test]
    fn the_reference_images_match_what_the_recipe_renders() {
        let out = Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .expect("a UTF-8 temp dir")
            .join("gallery-reference");
        let recipe = Utf8Path::new("tests/reference.toml");
        let capture = read_recipe(recipe, Some(&out)).expect("the reference recipe");
        render(
            &reference_scenes(),
            Renderer::Glow,
            &|_: &egui::Context| {},
            &capture,
        )
        .expect("every reference shot renders");

        // Collected, not asserted one at a time: a layout change moves several images, and seeing all
        // of them beats fixing them one run apiece.
        let mut results = egui_kittest::SnapshotResults::new();
        for path in capture
            .shots
            .iter()
            .filter_map(|shot| shot.out.as_ref())
            .chain(capture.sheet.as_ref())
        {
            let name = path.file_stem().expect("a PNG filename");
            let png = std::fs::read(path).expect("the capture was written");
            let image = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
                .expect("a PNG")
                .to_rgba8();
            results.add(egui_kittest::try_image_snapshot(&image, name));
        }
        results.unwrap();
    }

    /// A canvas scrolls, so a scene bigger than the size asked for used to come back cropped to it —
    /// losing, at the least, the margin that shows where the stage ends.
    #[cfg(not(target_vendor = "apple"))]
    #[test]
    fn a_scene_too_big_for_the_size_asked_for_is_captured_whole() {
        let asked = egui::vec2(120.0, 80.0);
        let out = Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .expect("a UTF-8 temp dir")
            .join("gallery-overflow")
            .join("stage-forms.png");
        let capture = Capture {
            shots: vec![Shot {
                scene: "stage-forms".to_owned(),
                out: Some(out.clone()),
                size: asked,
                knobs: Vec::new(),
                frames: None,
                trim: true,
                settle: false,
                scale: DEFAULT_SCALE,
                list: false,
                template: false,
            }],
            sheet: None,
            report: None,
        };
        render(
            &reference_scenes(),
            Renderer::Glow,
            &|_: &egui::Context| {},
            &capture,
        )
        .expect("the shot renders");

        let png = std::fs::read(&out).expect("the capture was written");
        let image = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
            .expect("a PNG")
            .to_rgba8();
        assert!(
            image.width() > asked.x as u32 && image.height() > asked.y as u32,
            "{}×{} holds the whole scene, not the {}×{} asked for",
            image.width(),
            image.height(),
            asked.x,
            asked.y
        );
    }

    /// A roomy `size` keeps a scrolling canvas from cutting the scene off,
    /// so the slack it leaves around the drawing is not worth writing out.
    #[cfg(not(target_vendor = "apple"))]
    #[test]
    fn a_shot_is_cropped_to_what_it_drew_unless_trim_is_off() {
        let dir = Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .expect("a UTF-8 temp dir")
            .join("gallery-trim");
        let shot = |name: &str, trim: bool| Shot {
            scene: "documented-stage".to_owned(),
            out: Some(dir.join(format!("{name}.png"))),
            size: egui::vec2(900.0, 700.0),
            knobs: Vec::new(),
            frames: None,
            trim,
            settle: false,
            scale: DEFAULT_SCALE,
            list: false,
            template: false,
        };
        let capture = Capture {
            shots: vec![shot("cropped", true), shot("whole", false)],
            sheet: None,
            report: None,
        };
        render(
            &reference_scenes(),
            Renderer::Glow,
            &|_: &egui::Context| {},
            &capture,
        )
        .expect("both shots render");

        let read = |name: &str| {
            let png =
                std::fs::read(dir.join(format!("{name}.png"))).expect("the capture was written");
            image::load_from_memory_with_format(&png, image::ImageFormat::Png)
                .expect("a PNG")
                .to_rgba8()
        };
        let (cropped, whole) = (read("cropped"), read("whole"));

        assert_eq!(
            (whole.width(), whole.height()),
            (900, 700),
            "trim off writes the size asked for"
        );
        assert!(
            cropped.width() < whole.width() && cropped.height() < whole.height(),
            "{}×{} should sit inside the {}×{} it laid out in",
            cropped.width(),
            cropped.height(),
            whole.width(),
            whole.height()
        );
        assert_eq!(
            cropped.get_pixel(0, 0),
            whole.get_pixel(0, 0),
            "the crop keeps the origin, so the drawing itself is untouched"
        );
    }

    /// Shoot one of the template's wgpu scenes and read the PNG back.
    ///
    /// Through [`Linked`](crate::Linked), which reads the test binary's inventory —
    /// `scaffold_scenes` has filled it with the scenes a scaffold ships.
    /// The PNG is named for the scene and the scale, so two shots of one scene
    /// do not overwrite each other before both have been read.
    fn wgpu_shot(
        scene: &str,
        size: egui::Vec2,
        scale: f32,
        knobs: &[(&str, &str)],
    ) -> image::RgbaImage {
        use crate::SceneSource as _;

        let out = Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .expect("a UTF-8 temp dir")
            .join("gallery-wgpu")
            .join(format!("{}-at-{scale}.png", slug(scene)));
        let capture = Capture {
            shots: vec![Shot {
                scene: scene.to_owned(),
                out: Some(out.clone()),
                size,
                scale,
                knobs: knobs
                    .iter()
                    .map(|(key, value)| KnobOverride {
                        key: (*key).to_owned(),
                        value: (*value).to_owned(),
                    })
                    .collect(),
                frames: None,
                trim: true,
                settle: false,
                list: false,
                template: false,
            }],
            sheet: None,
            report: None,
        };
        render(
            &crate::Linked.manifest(),
            Renderer::Wgpu,
            &|_: &egui::Context| {},
            &capture,
        )
        .expect("the shot renders");

        let png = std::fs::read(&out).expect("the capture was written");
        image::load_from_memory_with_format(&png, image::ImageFormat::Png)
            .expect("a PNG")
            .to_rgba8()
    }

    /// A wgpu paint callback is scene content egui's shapes never carry, so a shot has to be
    /// checked against it twice over: the harness must hand the render state to the scene —
    /// without it a capture still writes a clean PNG, minus everything the window would have
    /// drawn through wgpu — and the trim must keep the whole rect the callback drew into.
    /// The last row is where a short trim would show, the callback being the only thing on it.
    ///
    /// The template's gradient scene is the subject: its end colours reach the shader only
    /// through the uniform buffer its callback uploads.
    /// Finding them in the pixels is evidence the uniforms arrived, not that a pipeline ran.
    #[test]
    fn a_wgpu_paint_callback_lands_in_a_trimmed_capture() {
        let asked = egui::vec2(640.0, 360.0);
        let image = wgpu_shot("wgpu::gradient", asked, DEFAULT_SCALE, &[]);
        // Down the middle, where the ramp runs.
        let column = |y: u32| image.get_pixel(image.width() / 2, y).0;
        let near = |pixel: [u8; 4], color: egui::Color32| {
            pixel
                .iter()
                .zip(color.to_array())
                .all(|(drawn, wanted)| drawn.abs_diff(wanted) <= 2)
        };
        // A pixel samples the middle of its row, so the first and last are a fraction
        // of the ramp short of the colours the ends themselves hold.
        let (first, last) = (column(0), column(image.height() - 1));
        assert!(
            near(first, crate::scaffold_scenes::wgpu::TOP),
            "the ramp starts from the top knob's colour at the first row: {first:?}"
        );
        assert!(
            near(last, crate::scaffold_scenes::wgpu::BOTTOM),
            "and reaches the bottom knob's at the last, so the trim cut nothing off it: {last:?}"
        );
        // The green channel spans 173 down to 43 between the two, so it reads the ramp
        // more finely than the eye does.
        let greens: Vec<u8> = (0..image.height()).map(|y| column(y)[1]).collect();
        assert!(
            greens.windows(2).all(|pair| pair[0] >= pair[1]),
            "one ramp top to bottom, never doubling back"
        );
        // The two ends of a band gradient sit flat; these have moved a fifth of the way.
        let fifth = image.height() / 5;
        assert!(
            greens[0] - greens[fifth as usize] > 10
                && greens[(image.height() - 1 - fifth) as usize] - greens[greens.len() - 1] > 10,
            "and it is under way at both ends rather than flat there: {greens:?}"
        );
    }

    /// One colour by the three routes a pixel can take — egui's own fill, a callback
    /// into egui's render pass, and a pass into the target
    /// [`SceneCtx::render_pass`](crate::SceneCtx::render_pass) hands over.
    /// A step between them means shader-drawn content cannot sit beside egui-drawn content
    /// without a seam, and a backdrop behind a component is exactly that arrangement.
    ///
    /// The third is the loose one: it goes into a texture and is sampled back out, so it rounds
    /// to eight bits twice and can land a step off. A step is the tolerance here — a colour
    /// space read wrong would be tens of them.
    #[test]
    fn one_colour_comes_out_the_same_by_every_route_a_scene_can_draw_it() {
        let tint = "#6C9CD8";
        let image = wgpu_shot(
            "wgpu::colour",
            egui::vec2(400.0, 200.0),
            DEFAULT_SCALE,
            &[("tint", tint)],
        );
        let filled = egui::Color32::from_hex(tint)
            .expect("a hex colour")
            .to_array();

        // The three bands are flush and equally wide, so the middle of each third is inside one.
        let row = image.height() / 2;
        let across: Vec<[u8; 4]> = (0..image.width())
            .map(|x| image.get_pixel(x, row).0)
            .collect();
        let opaque = |drawn: &[u8; 4]| drawn[3] == u8::MAX && drawn != &crate::PANEL_BG.to_array();
        let first = across
            .iter()
            .position(opaque)
            .expect("the bands cross the middle row");
        let last = across
            .iter()
            .rposition(opaque)
            .expect("the bands cross the middle row");
        let band = (last - first + 1) / 3;
        for (route, at) in [
            ("egui's own fill", first + band / 2),
            ("a callback", first + band + band / 2),
            ("a pass of the scene's own", first + 2 * band + band / 2),
        ] {
            let drawn = across[at];
            let off = drawn
                .iter()
                .zip(filled)
                .map(|(drawn, wanted)| drawn.abs_diff(wanted))
                .max()
                .unwrap_or(u8::MAX);
            assert!(
                off <= 1,
                "{route} came out {drawn:?} against {filled:?}, {off} steps away"
            );
        }
    }

    /// egui's render pass carries no depth attachment, so a solid drawn straight into it
    /// is sorted by submission order alone and its far faces land on the near ones.
    /// A target from [`SceneCtx::render_pass`](crate::SceneCtx::render_pass) carries one,
    /// and the same shader through it comes out a cube. The template's `depth` scene draws
    /// both, side by side, so one shot holds the comparison.
    ///
    /// Counted in saturated colours, which are the cube's faces and nothing else in the
    /// picture: the sorted half shows the three faces turned towards the viewer,
    /// the unsorted half shows whichever were drawn last as well.
    /// It is also what says the offscreen pass ran headlessly at all,
    /// going as it does through an encoder and a texture nothing else here touches.
    ///
    /// The `side` override is not idle: a shot declares knobs on its first frame and applies
    /// the recipe from the second, so asking for anything but the default is what makes the
    /// scene reallocate gallery's target mid-run and re-point the texture egui already holds.
    #[test]
    fn a_pass_of_the_scenes_own_sorts_a_solid_the_egui_pass_cannot() {
        let shot = wgpu_shot(
            "wgpu::depth",
            egui::vec2(520.0, 300.0),
            DEFAULT_SCALE,
            &[("side", "140")],
        );
        // Greys are the chrome — checkerboard, panel, captions — and every face is a tint.
        // Binned, because a face that went through gallery's texture rounds to eight bits twice
        // and comes back as two neighbouring values; counted, so the blended pixels along an
        // edge can be left out below.
        let faces = |half: std::ops::Range<u32>| {
            let mut seen = std::collections::BTreeMap::new();
            for (x, y) in half.flat_map(|x| (0..shot.height()).map(move |y| (x, y))) {
                let [r, g, b, _] = shot.get_pixel(x, y).0;
                if r.max(g).max(b) - r.min(g).min(b) > 40 {
                    *seen.entry([r >> 2, g >> 2, b >> 2]).or_insert(0_u32) += 1;
                }
            }
            seen
        };
        let middle = shot.width() / 2;
        let (unsorted, sorted) = (faces(0..middle), faces(middle..shot.width()));
        assert!(
            sorted.len() < unsorted.len(),
            "the half with a depth buffer leaves fewer faces showing: {} tints sorted, \
             {} unsorted",
            sorted.len(),
            unsorted.len()
        );

        // A face is thousands of pixels; anything under that is a blend along an edge.
        let faces = |tints: &std::collections::BTreeMap<[u8; 3], u32>| {
            tints.values().filter(|count| **count > 1_000).count()
        };
        assert_eq!(
            faces(&sorted),
            3,
            "the sorted half shows the three faces turned towards the viewer: {sorted:?}"
        );
        assert!(
            faces(&unsorted) > 3,
            "and the unsorted half shows more, having kept whichever were drawn last: {unsorted:?}"
        );
    }

    /// A window follows its display's scale factor while a capture takes the one it is given,
    /// so the two draw the same scene at different pixel counts. `scale` is what lets a shot
    /// say which, and this is the property it has to keep: a layout stated in points comes out
    /// the same picture at that many times the pixels, while anything a shader measured
    /// in device pixels is exactly where it was.
    ///
    /// The template's `device pixels` scene holds both halves — a stage sized in points,
    /// and a ring of a fixed pixel radius drawn inside it.
    /// Doubling the scale doubles the PNG and leaves the ring alone.
    /// It also covers the trim, which counts in pixels off a canvas measured in points
    /// and would crop to a quarter of the image if it took one for the other.
    #[test]
    fn a_scale_takes_the_same_layout_to_more_pixels_and_leaves_device_pixels_alone() {
        let size = egui::vec2(400.0, 300.0);
        let single = wgpu_shot("wgpu::device", size, 1.0, &[]);
        let double = wgpu_shot("wgpu::device", size, 2.0, &[]);

        // Each side is rounded up to a whole pixel, so a doubled one can land just off twice.
        let doubled = |single: u32, double: u32| double.abs_diff(single * 2) <= 2;
        assert!(
            doubled(single.width(), double.width()) && doubled(single.height(), double.height()),
            "{}×{} at 1× should come back about {}×{} at 2×, not {}×{}",
            single.width(),
            single.height(),
            single.width() * 2,
            single.height() * 2,
            double.width(),
            double.height()
        );

        // The ring is the only strongly blue thing drawn: the rules are a dimmed version of it,
        // the checkerboard and the panel are grey, and the caption is grey text.
        let ring_across = |image: &image::RgbaImage| {
            let blue = |[r, _, b, _]: [u8; 4]| b > 180 && b.saturating_sub(r) > 60;
            let lit: Vec<u32> = (0..image.width())
                .filter(|x| (0..image.height()).any(|y| blue(image.get_pixel(*x, y).0)))
                .collect();
            let (first, last) = (lit.first().copied(), lit.last().copied());
            last.zip(first).map(|(last, first)| last - first + 1)
        };
        let single_ring = ring_across(&single).expect("the ring is drawn at 1×");
        let double_ring = ring_across(&double).expect("the ring is drawn at 2×");
        assert!(
            double_ring.abs_diff(single_ring) <= 4,
            "the ring is stated in device pixels, so it spans {single_ring} at 1× and should \
             span about as many at 2×, not {double_ring}"
        );
    }

    /// The consumer's layout: a fixed scrolling stage holding more rows than it can show,
    /// on a canvas picked to sit just over it.
    ///
    /// Their captures came back with the checkerboard running off the bottom edge
    /// — the stage had taken the canvas, so there was no canvas left under it.
    ///
    /// The bottom row of the picture is the thing that says whether it still does.
    #[cfg(not(target_vendor = "apple"))]
    #[test]
    fn a_capture_ends_in_canvas_under_a_fixed_scrolling_stage() {
        fn scrolling_box(ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
            ctx.stage(
                ui,
                crate::Stage::Fixed(egui::vec2(900.0, 460.0)).scrollable(),
                |ui: &mut egui::Ui| {
                    for row in 0..40 {
                        ui.label(format!("row {row}"));
                    }
                },
            );
        }
        let manifest = Manifest {
            scenes: vec![SceneEntry {
                render: scrolling_box,
                name: "scrolling-box",
                module_path: "reference",
                default: true,
                order: 0,
                source: "",
            }],
            groups: Vec::new(),
        };
        let out = Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .expect("a UTF-8 temp dir")
            .join("gallery-margin")
            .join("scrolling-box.png");
        let capture = Capture {
            shots: vec![Shot {
                scene: "scrolling-box".to_owned(),
                out: Some(out.clone()),
                // The consumer's own numbers: a canvas picked
                // to sit just over a 900×460 stage.
                size: egui::vec2(964.0, 520.0),
                knobs: Vec::new(),
                frames: None,
                trim: true,
                settle: false,
                scale: DEFAULT_SCALE,
                list: false,
                template: false,
            }],
            sheet: None,
            report: None,
        };
        render(&manifest, Renderer::Glow, &|_: &egui::Context| {}, &capture)
            .expect("the shot renders");

        let png = std::fs::read(&out).expect("the capture was written");
        let image = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
            .expect("a PNG")
            .to_rgba8();
        let bottom = image.height() - 1;
        for x in 0..image.width() {
            assert_eq!(
                image.get_pixel(x, bottom).0,
                PANEL_BG.to_array(),
                "({x}, {bottom}) of {}×{} is canvas under the stage, not the stage itself",
                image.width(),
                image.height()
            );
        }
    }

    #[test]
    fn a_listing_quotes_the_label_and_names_the_accessor_that_declared_it() {
        assert_eq!(
            describe(&select("body style", &["sedan", "suv"])),
            r#"buttons "body style" = "sedan"  (sedan | suv)"#
        );
    }

    /// Write `text` to a recipe in its own scratch directory,
    /// and read it back. Its own, because a recipe's `out`
    /// resolves against the file, so the directory is part
    /// of what's under test.
    fn recipe(name: &str, text: &str, out: Option<&str>) -> Result<Capture, Diagnostic> {
        let dir = Utf8PathBuf::from_path_buf(std::env::temp_dir())
            .expect("a UTF-8 temp dir")
            .join(format!("gallery-recipe-{name}"));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("capture.toml");
        std::fs::write(&path, text).expect("write recipe");
        read_recipe(&path, out.map(Utf8Path::new))
    }

    /// The recipe a scaffold ships reads back, since `deny_unknown_fields` turns a key that drifts
    /// out of the `Recipe` struct into an error for every instance rather than a stale comment.
    ///
    /// Nothing else parses it: `just validate` compiles the crate, not the files it hands out.
    #[test]
    fn the_scaffolded_recipe_still_parses() {
        let shipped = Utf8Path::new("template/capture.toml");
        let capture = read_recipe(shipped, None).expect("the scaffold's own recipe parses");
        assert!(
            !capture.shots.is_empty(),
            "and describes the shots it documents"
        );
        assert!(
            capture.report.is_some() && capture.sheet.is_some(),
            "including the options it exists to demonstrate"
        );
    }

    #[test]
    fn a_recipe_expands_to_a_shot_each_with_the_root_defaults_filled_in() {
        let shots = recipe(
            "defaults",
            r#"
            out = "renders"
            size = "1280x720"

            [[shot]]
            name = "vehicle-night"
            scene = "vehicle"
            knobs = { night = true, speed = 1.5, "body style" = "SUV" }

            [[shot]]
            name = "map"
            scene = "map"
            size = "800x600"
            "#,
            None,
        )
        .expect("valid recipe")
        .shots;

        assert_eq!(shots.len(), 2);
        assert!(
            shots[0]
                .out
                .as_ref()
                .expect("an output path")
                .ends_with("renders/vehicle-night.png"),
            "the name becomes the filename under the recipe's own out"
        );
        assert_eq!(shots[0].size, egui::vec2(1280.0, 720.0), "root default");
        assert_eq!(shots[1].size, egui::vec2(800.0, 600.0), "own size wins");
        assert!(
            shots[0].trim,
            "a recipe that never mentions trim still gets it"
        );
        assert_eq!(
            shots[0].knobs,
            knobs(&[("body style", "SUV"), ("night", "true"), ("speed", "1.5")]),
            "TOML scalars flatten to the text the per-kind parser reads"
        );
    }

    /// `trim` follows `size`: a root default that a shot can override.
    #[test]
    fn trim_is_on_unless_the_recipe_or_the_shot_turns_it_off() {
        let shots = recipe(
            "trim",
            r#"
            out = "renders"
            size = "1280x720"
            trim = false

            [[shot]]
            name = "whole"
            scene = "vehicle"

            [[shot]]
            name = "cropped"
            scene = "map"
            trim = true
            "#,
            None,
        )
        .expect("valid recipe")
        .shots;

        assert!(!shots[0].trim, "the recipe's own default");
        assert!(shots[1].trim, "the shot's own wins");
    }

    #[test]
    fn out_on_the_command_line_overrides_the_recipes_own() {
        let shots = recipe(
            "out-override",
            r#"
            out = "renders"
            size = "10x10"
            [[shot]]
            name = "one"
            scene = "a"
            "#,
            Some("/tmp/elsewhere"),
        )
        .expect("valid recipe")
        .shots;
        assert_eq!(
            shots[0].out.as_deref(),
            Some(Utf8Path::new("/tmp/elsewhere/one.png"))
        );
    }

    #[test]
    fn a_recipe_that_could_not_do_what_it_says_is_rejected() {
        // Each name says what is wrong with the recipe beside it, and names its scratch directory.
        let cases = [
            (
                "no-shots-at-all",
                r#"
                out = "r"
                "#,
            ),
            (
                "no-size-anywhere",
                r#"
                out = "r"
                [[shot]]
                name = "a"
                scene = "s"
                "#,
            ),
            (
                "nowhere-to-write",
                r#"
                size = "1x1"
                [[shot]]
                name = "a"
                scene = "s"
                "#,
            ),
            (
                "two-shots-with-one-name",
                r#"
                out = "r"
                size = "1x1"
                [[shot]]
                name = "a"
                scene = "s"
                [[shot]]
                name = "a"
                scene = "t"
                "#,
            ),
            (
                "a-misspelt-field",
                r#"
                out = "r"
                size = "1x1"
                [[shot]]
                name = "a"
                sceen = "s"
                "#,
            ),
            (
                "a-knob-value-no-knob-could-take",
                r#"
                out = "r"
                size = "1x1"
                [[shot]]
                name = "a"
                scene = "s"
                knobs = { at = [1, 2] }
                "#,
            ),
        ];
        for (why, text) in cases {
            assert!(recipe(why, text, None).is_err(), "`{why}` is not a recipe");
        }
    }
}
