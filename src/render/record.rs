//! Recording a shot: the clock its frames are drawn on,
//! and the frames it hands to the encoder.

use std::time::{Duration, Instant};

use super::*;
use crate::video::{Recorder, too_large};

/// A recording shot's clock: frame `drawn` sits at exactly `drawn / fps`,
/// so neither render speed nor the frames before the recording can move one.
pub(super) struct Clock {
    fps: u16,
    drawn: u64,
}

impl Clock {
    pub(super) fn new(fps: u16) -> Self {
        Self { fps, drawn: 0 }
    }

    #[expect(clippy::cast_precision_loss, reason = "a frame count far below 2^52")]
    fn next(&mut self) -> f64 {
        self.drawn += 1;
        self.drawn as f64 / f64::from(self.fps)
    }
}

/// What a frame was drawn from: one drawn from the same looks the same.
/// The canvas's size is in its shapes, which begin with one that fills it.
#[derive(PartialEq)]
struct Painted {
    shapes: Vec<egui::epaint::ClippedShape>,
    tessellation: egui::epaint::TessellationOptions,
    scale: f32,
    pointer: Option<egui::Pos2>,
}

impl Painted {
    /// The frame last drawn, if that is all it was drawn from:
    /// a paint callback and a texture of the scene's own draw what they like,
    /// and a texture egui manages may just have been given another image.
    fn of(harness: &egui_kittest::Harness<'static, Canvas>) -> Option<Self> {
        let retextured = harness
            .ctx
            .with_plugin(|textures: &mut Textures| std::mem::take(&mut textures.set));
        let shapes = &harness.output().shapes;
        if retextured != Some(false) || shapes.iter().any(|shape| draws_its_own(&shape.shape)) {
            return None;
        }
        Some(Self {
            shapes: shapes.clone(),
            tessellation: harness.ctx.options(|options| options.tessellation_options),
            scale: harness.ctx.pixels_per_point(),
            pointer: harness.ctx.input(|input| input.pointer.hover_pos()),
        })
    }
}

fn draws_its_own(shape: &egui::Shape) -> bool {
    match shape {
        egui::Shape::Vec(shapes) => shapes.iter().any(draws_its_own),
        egui::Shape::Callback(_) => true,
        _ => matches!(shape.texture_id(), egui::TextureId::User(_)),
    }
}

/// Notices textures being set, which kittest takes out of the frame's output.
#[derive(Default)]
struct Textures {
    set: bool,
}

impl egui::Plugin for Textures {
    fn debug_name(&self) -> &'static str {
        "gallery-recording"
    }

    fn output_hook(&mut self, _: &egui::Context, output: &mut egui::FullOutput) {
        self.set |= !output.textures_delta.set.is_empty();
    }
}

/// A recording in progress, and what its frames have cost so far.
pub(super) struct Recording {
    /// One file per format, each fed every frame.
    sinks: Vec<(VideoFormat, Utf8PathBuf, Recorder)>,
    fps: u16,
    /// Frames each step's end is held for.
    hold: u32,
    /// Whether a frame came since the last hold, so a step that ends
    /// with the one around it, or before any frame, adds no second pause.
    drew_since_hold: bool,
    input: Option<InputOverlay>,
    draw: Duration,
    capture: Duration,
    /// Set when it only watches: an error stops it and nothing else.
    watched: Option<Watched>,
    /// The error that stopped it.
    stopped: Option<String>,
}

/// The input at each frame of a recording that only watches,
/// to be laid over the frames once it is known that they are kept.
struct Watched {
    log: InputLog,
    shown: Vec<Shown>,
    /// What the last frame recorded was drawn from.
    painted: Option<Painted>,
    /// Holds every frame so far, in pixels.
    size: (u32, u32),
}

impl Recording {
    /// Close it: what its every frame shows of the input, its file complete.
    ///
    /// # Errors
    /// When the file could not be written, or an error had stopped it.
    pub(super) fn close(self) -> Result<Vec<Shown>, String> {
        let shown = self.watched.map(|watched| watched.shown);
        if let Some(why) = self.stopped {
            return Err(why);
        }
        for (_, _, sink) in self.sinks {
            sink.finish()?;
        }
        Ok(shown.unwrap_or_default())
    }

    /// Show the last frame for `frames` frames more.
    fn repeat(&mut self, frames: u32) -> Result<(), String> {
        self.sinks
            .iter_mut()
            .try_for_each(|(_, _, sink)| sink.hold(frames))
    }

    /// `done`, its error kept as what stopped a recording that only watches.
    fn bear(&mut self, done: Result<(), String>) -> Result<(), String> {
        match done {
            Err(why) if self.watched.is_some() => {
                self.stopped = Some(why);
                Ok(())
            }
            done => done,
        }
    }
}

/// What one of a finished recording's files came to.
#[derive(Clone, Debug)]
pub(super) struct VideoOutcome {
    pub(super) format: VideoFormat,
    pub(super) path: Utf8PathBuf,
    pub(super) fps: u16,
    /// Frames drawn into it.
    pub(super) frames: u32,
    /// Frames added by pausing at a play's step ends.
    pub(super) held: u32,
    /// Frames the file holds, a repeat having
    /// lengthened the frame before it instead.
    pub(super) written: u32,
    /// Drawing the scene, all frames together; shared by every format's file.
    pub(super) draw: Duration,
    /// Rendering each frame and reading it back; shared by every format's file.
    pub(super) capture: Duration,
    pub(super) encode: Duration,
    pub(super) deepest_queue: usize,
}

/// Where a shot's recording goes: `<stem>.apng` or `<stem>.mp4` beside its PNG.
pub(super) fn video_path(out: &Utf8Path, format: VideoFormat) -> Utf8PathBuf {
    out.with_file_name(format!(
        "{}.{}",
        out.file_stem().unwrap_or("shot"),
        format.extension()
    ))
}

/// egui's animations and cursor blink, which kittest turns off
/// so stills hold still, back on for a recording,
/// where they are part of what is shown.
pub(super) fn animate(ctx: &egui::Context) {
    let moving = egui::Style::default();
    ctx.all_styles_mut(|style| {
        style.animation_time = moving.animation_time;
        style.scroll_animation = moving.scroll_animation;
        style.visuals.text_cursor.blink = true;
    });
}

/// The canvas as last drawn, in pixels: what it renders to, at whatever zoom
/// the scene set and whatever size it asked its window for.
fn pixels(harness: &egui_kittest::Harness<'static, Canvas>) -> egui::Vec2 {
    (harness.ctx.content_rect().size() * harness.ctx.pixels_per_point()).round()
}

/// Start sending every frame drawn, on a canvas at `scale`,
/// to a recording beside `out` in each of its formats.
///
/// # Errors
/// When the canvas is too large for a format, a file cannot be created,
/// or the input overlay's harness cannot be opened.
pub(super) fn start(
    harness: &mut egui_kittest::Harness<'static, Canvas>,
    session: &Session,
    scale: f32,
    out: &Utf8Path,
    record: Record,
) -> Result<(), Diagnostic> {
    let size = pixels(harness);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a canvas the session has drawn"
    )]
    let pixels = (size.x as u32, size.y as u32);
    if record
        .formats
        .iter()
        .any(|format| format == VideoFormat::Mp4)
        && let Some(why) = too_large(pixels)
    {
        return Err(Diagnostic::new(why).hint("record it with `format = \"apng\"`"));
    }
    // It starts before the shot's PNG, whose writing would otherwise make the directory.
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create `{dir}`: {e}"))?;
    }
    let sinks = record
        .formats
        .iter()
        .map(|format| {
            let path = video_path(out, format);
            Ok((
                format,
                landed(&path),
                Recorder::create(&path, record.fps, format)?,
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let input = record
        .input
        .map(|corner| InputOverlay::open(session, size / scale, scale, record.fps, corner))
        .transpose()?;
    harness.state_mut().recording = Some(Recording {
        sinks,
        fps: record.fps,
        hold: record.hold_frames(),
        drew_since_hold: false,
        input,
        draw: Duration::ZERO,
        capture: Duration::ZERO,
        watched: None,
        stopped: None,
    });
    Ok(())
}

/// Record into `out` at `fps`, only watching: an APNG that grows with the canvas.
/// It begins with the frame there is, as a play can fail before it draws one.
///
/// # Errors
/// When the file cannot be created, or that frame recorded.
pub(super) fn watch(
    harness: &mut egui_kittest::Harness<'static, Canvas>,
    out: &Utf8Path,
    fps: u16,
) -> Result<(), String> {
    let record = Record {
        fps,
        seconds: None,
        keep: Trace::Always,
        formats: VideoFormat::Apng.into(),
        hold: None,
        input: None,
    };
    harness.ctx.add_plugin(Textures::default());
    harness.state_mut().recording = Some(Recording {
        sinks: vec![(
            VideoFormat::Apng,
            out.to_owned(),
            Recorder::growing(out, fps)?,
        )],
        fps,
        hold: record.hold_frames(),
        drew_since_hold: false,
        input: None,
        draw: Duration::ZERO,
        capture: Duration::ZERO,
        watched: Some(Watched {
            log: InputLog::new(fps),
            shown: Vec::new(),
            painted: None,
            size: (0, 0),
        }),
        stopped: None,
    });
    let first = record_frame(harness, &[], Duration::ZERO);
    if first.is_err() {
        harness.state_mut().recording = None;
    }
    first
}

/// Pause on the frame last drawn, at a play's step end, when recording.
///
/// # Errors
/// When the encoder has failed.
pub(super) fn hold(harness: &mut egui_kittest::Harness<'static, Canvas>) -> Result<(), String> {
    let Some(recording) = harness.state_mut().recording.as_mut() else {
        return Ok(());
    };
    if !std::mem::take(&mut recording.drew_since_hold) || recording.stopped.is_some() {
        return Ok(());
    }
    if let Some(input) = recording.input.as_mut() {
        input.hold(recording.hold);
    }
    let hold = recording.hold;
    let held = recording.repeat(hold);
    if let Some(watched) = &mut recording.watched
        && let Some(last) = watched.shown.last().cloned()
    {
        watched
            .shown
            .extend(std::iter::repeat_n(last, hold as usize));
        watched.log.hold(hold);
    }
    recording.bear(held)
}

/// Close the recording, if one is open: the files written, one for each format,
/// and why any others, removed now, were not.
pub(super) fn finish(
    harness: &mut egui_kittest::Harness<'static, Canvas>,
) -> (Vec<VideoOutcome>, Vec<String>) {
    let (mut videos, mut lost) = (Vec::new(), Vec::new());
    let Some(recording) = harness.state_mut().recording.take() else {
        return (videos, lost);
    };
    for (format, path, sink) in recording.sinks {
        match sink.finish() {
            Ok(encoded) => videos.push(VideoOutcome {
                format,
                path,
                fps: recording.fps,
                frames: encoded.frames,
                held: encoded.held,
                written: encoded.written,
                draw: recording.draw,
                capture: recording.capture,
                encode: encoded.encode,
                deepest_queue: encoded.deepest_queue,
            }),
            Err(why) => {
                _ = std::fs::remove_file(&path);
                lost.push(why);
            }
        }
    }
    (videos, lost)
}

/// Draw one frame: on the shot's clock when it records,
/// and into the recording while one is open.
///
/// # Errors
/// When the frame cannot be rendered, or the encoder has failed.
pub(super) fn step(harness: &mut egui_kittest::Harness<'static, Canvas>) -> Result<(), String> {
    if let Some(time) = harness.state_mut().clock.as_mut().map(Clock::next) {
        harness.input_mut().time = Some(time);
    }
    // Taken before the frame consumes them.
    let shows_input = harness
        .state()
        .recording
        .as_ref()
        .is_some_and(|recording| recording.input.is_some() || recording.watched.is_some());
    let events = if shows_input {
        harness.input().events.clone()
    } else {
        Vec::new()
    };
    let started = Instant::now();
    harness.step();
    let drew = started.elapsed();
    if let Some(panic) = harness.state_mut().scene_panic.take() {
        return Err(format!(
            "scene `{}` panicked: {panic}",
            harness.state().scene.name
        ));
    }
    match &harness.state().recording {
        Some(recording) if recording.stopped.is_none() => {}
        _ => return Ok(()),
    }
    let recorded = record_frame(harness, &events, drew);
    match &mut harness.state_mut().recording {
        Some(recording) => recording.bear(recorded),
        None => recorded,
    }
}

#[cfg(test)]
thread_local! {
    /// Frames this thread has rendered for a recording.
    pub(crate) static RENDERED: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Hand the frame last drawn, in `drew`
/// and with `events`, to the recording.
fn record_frame(
    harness: &mut egui_kittest::Harness<'static, Canvas>,
    events: &[egui::Event],
    drew: Duration,
) -> Result<(), String> {
    let started = Instant::now();
    let zoom = harness.ctx.zoom_factor();
    let painted = match &harness.state().recording {
        Some(recording) if recording.watched.is_some() => Painted::of(harness),
        _ => None,
    };
    if let Some(recording) = &mut harness.state_mut().recording
        && let Some(watched) = &mut recording.watched
    {
        watched.log.frame(events);
        watched.shown.push(watched.log.shown(zoom));
        let same = painted.is_some() && painted == watched.painted;
        watched.painted = painted;
        if same {
            recording.drew_since_hold = true;
            return recording.repeat(1);
        }
    }
    #[cfg(test)]
    RENDERED.set(RENDERED.get() + 1);
    let mut image = rendered("render a recorded frame", harness)?;
    let recording = harness
        .state_mut()
        .recording
        .as_mut()
        .ok_or("BUG: a frame recorded with no recording open")?;
    if let Some(watched) = &mut recording.watched {
        let (width, height) = image.dimensions();
        watched.size = (watched.size.0.max(width), watched.size.1.max(height));
        if let Some(why) = too_large(watched.size) {
            return Err(why);
        }
    }
    if let Some(input) = recording.input.as_mut() {
        input.lay_over(events, zoom, &mut image)?;
    }
    let captured = started.elapsed();
    recording.draw += drew;
    recording.capture += captured;
    recording.drew_since_hold = true;
    let Some(((_, _, last), others)) = recording.sinks.split_last_mut() else {
        return Ok(());
    };
    for (_, _, sink) in others {
        sink.frame(image.clone())?;
    }
    last.frame(image)
}

/// Record `frames` frames of a shot without a play,
/// its overrides re-applied before each as settling does,
/// so the recipe keeps outranking the scene.
///
/// # Errors
/// When an override or a frame fails.
pub(super) fn record_frames(
    harness: &mut egui_kittest::Harness<'static, Canvas>,
    shot: &Shot,
    frames: u32,
) -> Result<(), Diagnostic> {
    let mut unmatched_globals: Vec<&KnobOverride> = shot.globals.iter().collect();
    let mut ledger = Ledger::new(shot.knobs.clone());
    for _ in 0..frames {
        apply_globals(harness.state_mut(), &shot.globals, &mut unmatched_globals)?;
        ledger.apply(harness.state_mut())?;
        step(harness).map_err(Diagnostic::new)?;
    }
    match harness.state_mut().global_write_error.take() {
        Some(error) => Err(Diagnostic::new(error)),
        None => Ok(()),
    }
}
