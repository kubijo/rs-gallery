//! Recording a shot: the clock its frames are drawn on,
//! and the frames it hands to the encoder.

use std::time::{Duration, Instant};

use super::*;
use crate::video::{Recorder, resized, too_large};

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
    /// One file per format, each fed every frame until it fails.
    sinks: Vec<Sink>,
    /// What of it was lost, and why.
    lost: Vec<String>,
    /// The size of its frames, of one that only watches the largest so far.
    size: Option<(u32, u32)>,
    fps: u16,
    /// Frames each step's end is held for.
    hold: u32,
    /// Whether a frame came since the last hold, so a step that ends
    /// with the one around it, or before any frame, adds no second pause.
    drew_since_hold: bool,
    glide: Glide,
    /// Whether the pointer came by a glide and has done nothing since, so it dwells first.
    dwelling: bool,
    input: Option<InputOverlay>,
    draw: Duration,
    capture: Duration,
    /// Set when it only watches.
    watched: Option<Watched>,
}

/// One format's file, still being written.
struct Sink {
    format: VideoFormat,
    path: Utf8PathBuf,
    recorder: Recorder,
}

/// Remove the file at `path`, which would pass for a whole recording,
/// or the link to it; why it could not be.
fn remove(path: &Utf8Path) -> Option<String> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Some(format!("`{path}` is left there: {e}"))
        }
        _ => None,
    }
}

/// [`remove`] the file at `path`, which is not
/// whole for `why`; all there is to tell of it.
fn lose(path: &Utf8Path, why: String) -> String {
    match remove(path) {
        Some(left) => format!("{why}; {left}"),
        None => why,
    }
}

/// The input at each frame of a recording that only watches,
/// to be laid over the frames once it is known that they are kept.
struct Watched {
    log: InputLog,
    shown: Vec<Shown>,
    /// Where in the recording each frame handed
    /// to it is, whatever pauses there are between.
    at: Vec<u32>,
    /// What the last frame recorded was drawn from.
    painted: Option<Painted>,
}

/// What a closed recording that only watched is of.
#[derive(Debug, Default)]
pub(super) struct Footage {
    /// What its every frame shows of the input.
    pub(super) input: Vec<Shown>,
    /// Where in it each frame handed to it is:
    /// the one it began with, then each the play drew.
    pub(super) at: Vec<u32>,
}

impl Recording {
    /// Close it, its file complete.
    ///
    /// # Errors
    /// When the file could not be written, or an error had stopped it.
    pub(super) fn close(self) -> Result<Footage, String> {
        let footage = self.watched.map(|watched| Footage {
            input: watched.shown,
            at: watched.at,
        });
        if let Some(why) = self.lost.into_iter().next() {
            return Err(why);
        }
        for sink in self.sinks {
            sink.recorder.finish()?;
        }
        Ok(footage.unwrap_or_default())
    }

    /// Add `image` to every file.
    fn push(&mut self, image: image::RgbaImage) {
        self.push_with(image, Recorder::frame);
    }

    /// Add `image`, a frame of a glide, to every file.
    fn push_glide(&mut self, image: image::RgbaImage) {
        self.push_with(image, Recorder::glide_frame);
    }

    fn push_with(
        &mut self,
        image: image::RgbaImage,
        hand: fn(&mut Recorder, image::RgbaImage) -> Result<(), String>,
    ) {
        let mut left = self.sinks.len();
        let mut image = Some(image);
        self.each(|recorder| {
            left -= 1;
            let image = if left == 0 {
                image.take()
            } else {
                image.clone()
            };
            hand(recorder, image.expect("BUG: taken for the last file"))
        });
    }

    /// Show the last frame for `frames` frames more.
    fn repeat(&mut self, frames: u32) {
        self.each(|recorder| recorder.hold(frames));
    }

    /// Show the last frame for `frames` frames more of a glide.
    fn repeat_glide(&mut self, frames: u32) {
        self.each(|recorder| recorder.glide_hold(frames));
    }

    /// `hand` every file something, losing those that fail at it.
    fn each(&mut self, mut hand: impl FnMut(&mut Recorder) -> Result<(), String>) {
        let mut reasons = Vec::new();
        let failed = self.sinks.extract_if(.., |sink| {
            let handed = hand(&mut sink.recorder);
            handed.map_err(|why| reasons.push(why)).is_err()
        });
        let failed: Vec<Sink> = failed.collect();
        for (Sink { path, recorder, .. }, why) in failed.into_iter().zip(reasons) {
            drop(recorder);
            self.lost.push(lose(&path, why));
        }
    }

    /// Lose every file there still is, for `why`,
    /// after those that had failed by then for their own reason.
    fn stop(&mut self, why: String) {
        let mut stopped = None;
        for Sink { path, recorder, .. } in self.sinks.drain(..) {
            match recorder.give_up() {
                Some(own) => self.lost.push(lose(&path, own)),
                None => {
                    let why = stopped.take().unwrap_or_else(|| why.clone());
                    stopped = Some(lose(&path, why));
                }
            }
        }
        self.lost.extend(stopped);
    }

    /// Lose the MP4, if there is one, as it is [`too_large`] for `why`.
    fn outgrow(&mut self, why: &str) {
        let is_mp4 = |sink: &Sink| sink.format == VideoFormat::Mp4;
        let Some(at) = self.sinks.iter().position(is_mp4) else {
            return;
        };
        let Sink { path, recorder, .. } = self.sinks.remove(at);
        drop(recorder);
        let mut why = format!("write `{path}`: {why}");
        if self.sinks.is_empty() {
            why.push_str("; an APNG has no such limit");
        }
        self.lost.push(lose(&path, why));
    }

    /// Lay the input over it, if that could be `opened`.
    fn show_input(&mut self, opened: Result<InputOverlay, Diagnostic>) {
        match opened {
            Ok(input) => self.input = Some(input),
            Err(report) => self.stop(report.summary()),
        }
    }

    /// Make room for a frame of `size` pixels.
    ///
    /// # Errors
    /// When it can't be of that size.
    fn fit(&mut self, size: (u32, u32)) -> Result<(), String> {
        let was = self.size.unwrap_or(size);
        self.size = Some((was.0.max(size.0), was.1.max(size.1)));
        if self.watched.is_none() {
            return if was == size {
                Ok(())
            } else {
                Err(resized(was, size))
            };
        }
        self.size.and_then(too_large).map_or(Ok(()), Err)
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
    /// Frames added by gliding the pointer between a play's gestures.
    pub(super) glided: u32,
    /// Frames the file holds, a repeat having
    /// lengthened the frame before it instead.
    pub(super) written: u32,
    /// Drawing the scene, all frames together; shared by every format's file.
    pub(super) draw: Duration,
    /// Rendering each frame and reading it back, and making each glide's;
    /// shared by every format's file.
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

/// Start sending every frame drawn to a recording beside `out` in each of its formats.
/// What can't be recorded is told once it is [finished](finish).
///
/// # Errors
/// When there is no writing to a file where it goes.
pub(super) fn start(
    harness: &mut egui_kittest::Harness<'static, Canvas>,
    out: &Utf8Path,
    record: Record,
) -> Result<(), String> {
    let size = pixels(harness);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a canvas the session has drawn"
    )]
    let pixels = (size.x as u32, size.y as u32);
    harness.state_mut().recording = Some(begun(pixels, out, record)?);
    Ok(())
}

/// [`start`] for a play's recording, on a canvas at `scale`, with its input laid over it:
/// the pointer, and the panel where `input` puts it.
///
/// # Errors
/// As [`start`].
pub(super) fn start_play(
    harness: &mut egui_kittest::Harness<'static, Canvas>,
    session: &Session,
    scale: f32,
    out: &Utf8Path,
    record: Record,
) -> Result<(), String> {
    start(harness, out, record)?;
    let size = pixels(harness) / scale;
    if let Some(recording) = &mut harness.state_mut().recording
        && !recording.sinks.is_empty()
    {
        let opened = InputOverlay::open(session, size, scale, record.fps, record.input);
        recording.show_input(opened);
    }
    Ok(())
}

/// A recording of a canvas of `size` pixels.
fn begun(size: (u32, u32), out: &Utf8Path, record: Record) -> Result<Recording, String> {
    let mut recording = Recording {
        sinks: Vec::new(),
        lost: Vec::new(),
        size: None,
        fps: record.fps,
        hold: record.hold_frames(),
        drew_since_hold: false,
        glide: record.glide,
        dwelling: false,
        input: None,
        draw: Duration::ZERO,
        capture: Duration::ZERO,
        watched: None,
    };
    // It starts before the shot's PNG, whose writing would otherwise make the directory.
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create `{dir}`: {e}"))?;
    }
    for format in record.formats.iter() {
        let path = video_path(out, format);
        match Recorder::create(&path, record.fps, format) {
            Ok(recorder) => recording.sinks.push(Sink {
                format,
                path,
                recorder,
            }),
            Err(why) => {
                let mut reasons = vec![why];
                reasons.extend(given_up(recording));
                return Err(reasons.join("; "));
            }
        }
    }
    if let Some(why) = too_large(size) {
        recording.outgrow(&why);
    }
    Ok(recording)
}

/// Record into `out` at `fps`, pausing and gliding as `hold` and `glide` say,
/// only watching: an APNG that grows with the canvas.
/// It begins with the frame there is, as a play can fail before it draws one.
///
/// # Errors
/// When the file cannot be created, or that frame recorded.
pub(super) fn watch(
    harness: &mut egui_kittest::Harness<'static, Canvas>,
    out: &Utf8Path,
    fps: u16,
    hold: Option<f32>,
    glide: Glide,
) -> Result<(), String> {
    let record = Record {
        fps,
        seconds: None,
        keep: Trace::Always,
        formats: VideoFormat::Apng.into(),
        hold,
        input: None,
        glide,
    };
    harness.ctx.add_plugin(Textures::default());
    harness.state_mut().recording = Some(Recording {
        sinks: vec![Sink {
            format: VideoFormat::Apng,
            path: out.to_owned(),
            recorder: Recorder::growing(out, fps)?,
        }],
        lost: Vec::new(),
        size: None,
        fps,
        hold: record.hold_frames(),
        drew_since_hold: false,
        glide,
        dwelling: false,
        input: None,
        draw: Duration::ZERO,
        capture: Duration::ZERO,
        watched: Some(Watched {
            log: InputLog::new(fps),
            shown: Vec::new(),
            at: Vec::new(),
            painted: None,
        }),
    });
    let first = record_frame(harness, &[], Duration::ZERO);
    if first.is_err() {
        harness.state_mut().recording = None;
    }
    first
}

/// A play's gesture began: its typing starts a new run, and an earlier glide no longer dwells.
pub(super) fn gesture_began(harness: &mut egui_kittest::Harness<'static, Canvas>) {
    let Some(recording) = harness.state_mut().recording.as_mut() else {
        return;
    };
    recording.dwelling = false;
    if let Some(input) = recording.input.as_mut() {
        input.end_run();
    }
    if let Some(watched) = &mut recording.watched {
        watched.log.end_run();
    }
}

/// Pause on the frame last drawn, at a play's step end, when recording.
pub(super) fn hold(harness: &mut egui_kittest::Harness<'static, Canvas>) {
    let Some(recording) = harness.state_mut().recording.as_mut() else {
        return;
    };
    if !std::mem::take(&mut recording.drew_since_hold) {
        return;
    }
    if let Some(input) = recording.input.as_mut() {
        input.hold(recording.hold);
    }
    let hold = recording.hold;
    recording.repeat(hold);
    if let Some(watched) = &mut recording.watched
        && let Some(last) = watched.shown.last().cloned()
    {
        watched
            .shown
            .extend(std::iter::repeat_n(last, hold as usize));
        watched.log.hold(hold);
    }
}

/// The pointer's place on each frame of a glide `from` `to`, short of `to`:
/// the gesture's own frame lands there, as the glide's last.
/// Its time follows the distance on the canvas, within `glide.time`.
pub(super) fn glide_path(
    glide: Glide,
    fps: u16,
    from: egui::Pos2,
    to: egui::Pos2,
    zoom: f32,
) -> Vec<egui::Pos2> {
    let Some(easing) = glide.easing else {
        return Vec::new();
    };
    if from == to {
        return Vec::new();
    }
    let ease: fn(f32) -> f32 = match easing {
        Easing::Linear => egui::emath::easing::linear,
        Easing::CubicInOut => egui::emath::easing::cubic_in_out,
        Easing::CubicOut => egui::emath::easing::cubic_out,
    };
    let [fewest, most] = glide.time;
    let seconds = (from.distance(to) * zoom / glide.speed).clamp(fewest, most);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "`glide_time` keeps it within 0 to 5 seconds"
    )]
    let frames = (seconds * f32::from(fps)).round() as u32;
    let way = to - from;
    let bow = way.rot90().normalized() * (way.length() * BOW).min(MOST_BOW);
    #[expect(clippy::cast_precision_loss, reason = "a few frames")]
    (1..frames)
        .map(|frame| {
            let along = ease(frame as f32 / frames as f32);
            from.lerp(to, along) + bow * (along * std::f32::consts::PI).sin()
        })
        .collect()
}

/// How far a glide bows off the straight line at its middle, as a share of its length.
const BOW: f32 = 0.08;

/// The most points a glide bows off the straight line, however long it is.
const MOST_BOW: f32 = 40.0;

/// Glide the pointer to where this frame's events put it,
/// over the scene as it stands.
///
/// A drag, or a pointer not yet shown, doesn't glide.
///
/// # Errors
/// When the scene or a glide frame cannot be rendered.
fn glide(harness: &mut egui_kittest::Harness<'static, Canvas>) -> Result<(), String> {
    let Some(recording) = harness.state().recording.as_ref() else {
        return Ok(());
    };
    let log = match (&recording.input, &recording.watched) {
        (Some(input), _) => input.log(),
        (None, Some(watched)) => &watched.log,
        (None, None) => return Ok(()),
    };
    let Some(from) = log.free_pointer() else {
        return Ok(());
    };
    let to = harness.input().events.iter().find_map(|event| match event {
        egui::Event::PointerMoved(at) | egui::Event::PointerButton { pos: at, .. } => Some(*at),
        _ => None,
    });
    let Some(to) = to else {
        return Ok(());
    };
    let zoom = harness.ctx.zoom_factor();
    let path = glide_path(recording.glide, recording.fps, from, to, zoom);
    if path.is_empty() {
        return Ok(());
    }
    let started = Instant::now();
    let scene = match recording.watched {
        Some(_) => None,
        None => Some(rendered("render the scene a pointer glides over", harness)?),
    };
    let recording = harness
        .state_mut()
        .recording
        .as_mut()
        .ok_or("BUG: a glide with no recording open")?;
    if let Some(watched) = &mut recording.watched {
        for at in &path {
            watched.log.glide(*at);
            watched.shown.push(watched.log.shown(zoom));
        }
        recording.repeat_glide(u32::try_from(path.len()).unwrap_or(u32::MAX));
    } else if let (Some(mut input), Some(scene)) = (recording.input.take(), scene) {
        let glided = path.into_iter().try_for_each(|at| {
            recording.push_glide(input.glide_over(at, zoom, &scene)?);
            Ok::<_, String>(())
        });
        recording.input = Some(input);
        glided?;
    }
    recording.dwelling = true;
    recording.capture += started.elapsed();
    Ok(())
}

/// Rest the pointer a glide brought, on the frame it arrived on,
/// before this frame's press, key or wheel.
fn dwell(harness: &mut egui_kittest::Harness<'static, Canvas>) {
    let acts = harness.input().events.iter().any(|event| {
        matches!(
            event,
            egui::Event::PointerButton { pressed: true, .. }
                | egui::Event::Key { pressed: true, .. }
                | egui::Event::Text(_)
                | egui::Event::MouseWheel { .. }
        )
    });
    let Some(recording) = harness.state_mut().recording.as_mut() else {
        return;
    };
    if !(acts && std::mem::take(&mut recording.dwelling)) {
        return;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "`glide_dwell` keeps it within 0 to 5 seconds"
    )]
    let frames = (recording.glide.dwell * f32::from(recording.fps)).round() as u32;
    // Frozen whole, panel and lit pointer too, as at a step's end.
    if let Some(input) = recording.input.as_mut() {
        input.hold(frames);
    }
    if let Some(watched) = &mut recording.watched {
        let Some(last) = watched.shown.last().cloned() else {
            return;
        };
        watched
            .shown
            .extend(std::iter::repeat_n(last, frames as usize));
        watched.log.hold(frames);
    }
    recording.repeat_glide(frames);
}

/// Give up the recording, if one is open, of a shot that failed for `why`:
/// that report, with what of the recording was lost before.
pub(super) fn discard(
    harness: &mut egui_kittest::Harness<'static, Canvas>,
    why: Diagnostic,
) -> Diagnostic {
    let recording = harness.state_mut().recording.take();
    with_lost(why, &recording.map(given_up).unwrap_or_default())
}

/// What of a recording that is given up was lost all the same.
fn given_up(mut recording: Recording) -> Vec<String> {
    for Sink { path, recorder, .. } in recording.sinks {
        match recorder.give_up() {
            Some(own) => recording.lost.push(lose(&path, own)),
            None => recording.lost.extend(remove(&path)),
        }
    }
    recording.lost
}

/// Close the recording, if one is open: the files written whole, one for each format,
/// and why each other, which is removed, was not.
pub(super) fn finish(
    harness: &mut egui_kittest::Harness<'static, Canvas>,
) -> (Vec<VideoOutcome>, Vec<String>) {
    let recording = harness.state_mut().recording.take();
    recording.map(finished).unwrap_or_default()
}

fn finished(recording: Recording) -> (Vec<VideoOutcome>, Vec<String>) {
    let (mut videos, mut lost) = (Vec::new(), recording.lost);
    for sink in recording.sinks {
        match sink.recorder.finish() {
            Ok(encoded) => videos.push(VideoOutcome {
                format: sink.format,
                path: landed(&sink.path),
                fps: recording.fps,
                frames: encoded.frames,
                held: encoded.held,
                glided: encoded.glided,
                written: encoded.written,
                draw: recording.draw,
                capture: recording.capture,
                encode: encoded.encode,
                deepest_queue: encoded.deepest_queue,
            }),
            Err(why) => lost.push(lose(&sink.path, why)),
        }
    }
    (videos, lost)
}

/// Draw one frame: on the shot's clock when it records,
/// and into the recording while one is open.
///
/// # Errors
/// When the scene panicked.
pub(super) fn step(harness: &mut egui_kittest::Harness<'static, Canvas>) -> Result<(), String> {
    if let Some(time) = harness.state_mut().clock.as_mut().map(Clock::next) {
        harness.input_mut().time = Some(time);
    }
    if is_recording(harness)
        && let Err(why) = glide(harness)
        && let Some(recording) = &mut harness.state_mut().recording
    {
        recording.stop(why);
    }
    if is_recording(harness) {
        dwell(harness);
    }
    // Taken before the frame consumes them.
    let shows_input = harness.state().recording.as_ref().is_some_and(|recording| {
        !recording.sinks.is_empty() && (recording.input.is_some() || recording.watched.is_some())
    });
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
    if is_recording(harness)
        && let Err(why) = record_frame(harness, &events, drew)
        && let Some(recording) = &mut harness.state_mut().recording
    {
        recording.stop(why);
    }
    Ok(())
}

/// Whether the next frame would reach any file.
fn is_recording(harness: &egui_kittest::Harness<'static, Canvas>) -> bool {
    let recording = harness.state().recording.as_ref();
    recording.is_some_and(|recording| !recording.sinks.is_empty())
}

#[cfg(test)]
thread_local! {
    /// Frames this thread has rendered for a recording.
    pub(crate) static RENDERED: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Hand the frame last drawn, in `drew`
/// and with `events`, to the recording.
///
/// # Errors
/// When it can't be rendered, or is of a size the recording can't take.
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
        watched
            .at
            .push(u32::try_from(watched.shown.len()).unwrap_or(u32::MAX));
        watched.shown.push(watched.log.shown(zoom));
        let same = painted.is_some() && painted == watched.painted;
        watched.painted = painted;
        if same {
            recording.drew_since_hold = true;
            recording.repeat(1);
            return Ok(());
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
    recording.fit(image.dimensions())?;
    if let Some(input) = recording.input.as_mut() {
        input.lay_over(events, zoom, &mut image)?;
    }
    let captured = started.elapsed();
    recording.draw += drew;
    recording.capture += captured;
    recording.drew_since_hold = true;
    recording.push(image);
    Ok(())
}

/// Record `frames` frames of a shot without a play, or as many as can be,
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
        if !is_recording(harness) {
            break;
        }
        apply_globals(harness.state_mut(), &shot.globals, &mut unmatched_globals)?;
        ledger.apply(harness.state_mut())?;
        step(harness).map_err(Diagnostic::new)?;
    }
    match harness.state_mut().global_write_error.take() {
        Some(error) => Err(Diagnostic::new(error)),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_glide_takes_its_time_from_the_distance_within_its_bounds_and_stops_short_of_the_target() {
        let eased = |easing| Glide {
            easing: Some(easing),
            ..Glide::default()
        };
        let linear = eased(Easing::Linear);
        let (from, at) = (egui::Pos2::ZERO, |x: f32| egui::pos2(x, 0.0));
        let xs = |path: Vec<egui::Pos2>| path.iter().map(|at| at.x).collect::<Vec<_>>();

        assert!(glide_path(Glide::default(), 20, from, at(120.0), 1.0).is_empty());
        assert!(glide_path(linear, 20, from, from, 1.0).is_empty());
        assert_eq!(
            xs(glide_path(linear, 20, from, at(120.0), 1.0)),
            [40.0, 80.0],
            "0.1 s at 1200 points a second, held to the fewest 0.15 s: \
             3 frames at 20 fps, the gesture's own the last"
        );
        assert_eq!(
            glide_path(linear, 20, from, at(2400.0), 1.0).len(),
            11,
            "2 s, cut to the most 0.6 s"
        );
        assert_eq!(glide_path(linear, 20, from, at(360.0), 1.0).len(), 5);
        assert_eq!(
            glide_path(linear, 20, from, at(360.0), 2.0).len(),
            11,
            "zoomed in twice, it goes twice as far on the canvas"
        );
        let instant = Glide {
            time: [0.0, 0.6],
            ..linear
        };
        assert!(
            glide_path(instant, 20, from, at(12.0), 1.0).is_empty(),
            "a hop too short for a frame of its own is the gesture's alone"
        );

        let cubic = xs(glide_path(
            eased(Easing::CubicInOut),
            20,
            from,
            at(360.0),
            1.0,
        ));
        assert!(cubic[0] < 60.0, "it sets off slowly: {cubic:?}");
        assert!(
            (cubic[2] - 180.0).abs() < 0.01,
            "and is halfway at half time: {cubic:?}"
        );
        assert!(
            cubic.last().is_some_and(|&x| x > 340.0 && x < 360.0),
            "and stops short: {cubic:?}"
        );
        let bowed: Vec<f32> = glide_path(linear, 20, from, at(360.0), 1.0)
            .iter()
            .map(|at| at.y.abs())
            .collect();
        assert!(
            (bowed[2] - 360.0 * BOW).abs() < 0.01,
            "it bows off the line most at its middle: {bowed:?}"
        );
        assert!(
            bowed[0] < bowed[2] && bowed[4] < bowed[2],
            "and least at its ends: {bowed:?}"
        );
        let out = xs(glide_path(
            eased(Easing::CubicOut),
            20,
            from,
            at(360.0),
            1.0,
        ));
        assert!(out[0] > 60.0, "cubic-out sets off fast: {out:?}");
    }

    /// A recording in both formats into `dir`, of a canvas of 64×64 pixels.
    fn both(dir: &tempfile::TempDir) -> Result<Recording, String> {
        both_of((64, 64), dir)
    }

    fn both_of(size: (u32, u32), dir: &tempfile::TempDir) -> Result<Recording, String> {
        let dir = Utf8Path::from_path(dir.path()).expect("UTF-8");
        let record = Record {
            fps: 10,
            seconds: None,
            keep: Trace::Always,
            formats: VideoFormats::of([VideoFormat::Apng, VideoFormat::Mp4]).expect("two"),
            hold: None,
            input: None,
            glide: Glide::default(),
        };
        begun(size, &dir.join("shot.png"), record)
    }

    fn names(dir: &tempfile::TempDir) -> Vec<String> {
        let files = dir.path().read_dir().expect("a directory");
        files
            .map(|file| file.expect("a file").file_name().into_string())
            .collect::<Result<_, _>>()
            .expect("UTF-8")
    }

    /// Frame `n`, which nothing compresses.
    fn noise(n: u32) -> image::RgbaImage {
        let mut seed = n + 1;
        image::RgbaImage::from_fn(64, 64, |_, _| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let [r, g, b, _] = seed.to_be_bytes();
            image::Rgba([r, g, b, 255])
        })
    }

    /// A recording whose APNG has failed for want of room, and how many frames it was handed.
    #[cfg(target_os = "linux")]
    fn losing_the_apng(dir: &tempfile::TempDir) -> (Recording, u32) {
        let mut recording = both(dir).expect("begun");
        let full = Utf8Path::new("/dev/full");
        recording.sinks[0].recorder = Recorder::create(full, 10, VideoFormat::Apng).expect("begun");
        for pushed in 0..200 {
            if recording.sinks.len() < 2 {
                return (recording, pushed);
            }
            recording.push(noise(pushed));
        }
        panic!("the APNG takes whatever it is handed");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_format_that_fails_is_lost_at_once_and_the_others_go_on() {
        let dir = tempfile::tempdir().expect("a directory");
        let (mut recording, pushed) = losing_the_apng(&dir);
        assert_eq!(names(&dir), ["shot.mp4"]);
        recording.repeat(5);
        recording.push(noise(pushed));
        let (videos, lost) = finished(recording);
        let [mp4] = &videos[..] else {
            panic!("{videos:?}");
        };
        assert_eq!(mp4.format, VideoFormat::Mp4);
        assert_eq!(mp4.frames, pushed + 1, "none is missing");
        assert_eq!(mp4.held, 5);
        assert!(
            matches!(&lost[..], [full] if full.contains("No space left")),
            "{lost:?}"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_hold_that_fails_loses_its_format_and_reaches_the_others() {
        let dir = tempfile::tempdir().expect("a directory");
        let mut recording = both(&dir).expect("begun");
        let full = Utf8Path::new("/dev/full");
        recording.sinks[0].recorder = Recorder::create(full, 10, VideoFormat::Apng).expect("begun");
        recording.push(noise(0));
        let failing = &mut recording.sinks[0].recorder;
        assert!((1..200).any(|n| failing.frame(noise(n)).is_err()));
        recording.repeat(3);
        assert_eq!(recording.sinks.len(), 1);
        let (videos, lost) = finished(recording);
        assert_eq!(videos[0].held, 3);
        assert_eq!(lost.len(), 1, "{lost:?}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_recording_stopped_tells_first_what_it_had_already_lost() {
        for watching in [false, true] {
            let dir = tempfile::tempdir().expect("a directory");
            let (mut recording, _) = losing_the_apng(&dir);
            recording.stop("the canvas changed".to_owned());
            assert_eq!(names(&dir), Vec::<String>::new());
            if watching {
                let why = recording.close().expect_err("it is lost");
                assert!(why.contains("No space left"), "{why}");
                continue;
            }
            let (videos, lost) = finished(recording);
            assert!(videos.is_empty(), "{videos:?}");
            assert!(
                matches!(&lost[..], [full, stopped]
                    if full.contains("No space left") && stopped == "the canvas changed"),
                "{lost:?}"
            );
        }
    }

    /// A recording of a frame smaller than OpenH264 takes, which nothing has noticed yet.
    fn failing_unnoticed(dir: &tempfile::TempDir) -> Recording {
        let mut recording = both(dir).expect("begun");
        recording.push(image::RgbaImage::new(8, 8));
        recording
    }

    #[test]
    fn what_fails_unnoticed_is_told_however_a_recording_ends() {
        let own = |lost: &str| lost.contains("shot.mp4`: ") && !lost.contains("canvas");
        let dir = tempfile::tempdir().expect("a directory");

        let mut stopped = failing_unnoticed(&dir);
        stopped.stop("the canvas changed".to_owned());
        assert!(
            matches!(&stopped.lost[..], [mp4, apng] if own(mp4) && apng == "the canvas changed"),
            "{:?}",
            stopped.lost
        );
        assert_eq!(names(&dir), Vec::<String>::new());

        let lost = given_up(failing_unnoticed(&dir));
        assert!(matches!(&lost[..], [mp4] if own(mp4)), "{lost:?}");
        assert_eq!(names(&dir), Vec::<String>::new());

        let (videos, lost) = finished(failing_unnoticed(&dir));
        assert!(matches!(&lost[..], [mp4] if own(mp4)), "{lost:?}");
        assert_eq!(videos.len(), 1);
    }

    #[test]
    fn a_recording_given_up_before_its_first_frame_lost_nothing() {
        let dir = tempfile::tempdir().expect("a directory");
        let recording = both(&dir).expect("begun");
        assert_eq!(given_up(recording), Vec::<String>::new());
        let mut recording = both(&dir).expect("begun");
        recording.stop("no frame".to_owned());
        assert_eq!(recording.lost, ["no frame"]);
    }

    #[test]
    fn a_recording_whose_input_cannot_be_shown_is_lost() {
        let dir = tempfile::tempdir().expect("a directory");
        let mut recording = both(&dir).expect("begun");
        recording.show_input(Err(Diagnostic::new("no overlay")));
        assert!(recording.sinks.is_empty());
        assert_eq!(finished(recording).1, ["no overlay"]);
        assert_eq!(names(&dir), Vec::<String>::new());
    }

    #[cfg(unix)]
    #[test]
    fn a_recording_that_cannot_be_written_over_is_left_alone_and_nothing_made_beside_it() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("a directory");
        let mp4 = dir.path().join("shot.mp4");
        std::fs::write(&mp4, "an earlier run's").expect("written");
        std::fs::set_permissions(&mp4, PermissionsExt::from_mode(0o444)).expect("read-only");
        if std::fs::File::options().write(true).open(&mp4).is_ok() {
            return; // A superuser's.
        }
        for size in [(64, 64), (4_000, 64)] {
            let why = both_of(size, &dir)
                .err()
                .expect("there is no writing to it");
            assert!(why.contains("create `"), "{why}");
            assert_eq!(names(&dir), ["shot.mp4"]);
            let kept = std::fs::read_to_string(&mp4).expect("read");
            assert_eq!(kept, "an earlier run's");
        }
    }

    #[test]
    fn an_mp4_too_large_is_lost_with_what_could_be_recorded_instead() {
        let dir = tempfile::tempdir().expect("a directory");
        let mut recording = both_of((4_000, 64), &dir).expect("begun");
        assert_eq!(names(&dir), ["shot.apng"]);
        let [mp4] = &recording.lost[..] else {
            panic!("{:?}", recording.lost);
        };
        assert!(!mp4.contains("an APNG"), "there is one: {mp4}");

        recording = both(&dir).expect("begun");
        recording.sinks.remove(0);
        recording.outgrow("too large");
        let mp4 = &recording.lost[0];
        assert!(
            mp4.ends_with("too large; an APNG has no such limit"),
            "{mp4}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_lost_recording_that_cannot_be_removed_is_told_of() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("a directory");
        let mode = |mode| {
            let mode = PermissionsExt::from_mode(mode);
            std::fs::set_permissions(dir.path(), mode).expect("set");
        };
        let left = |lost: &[String]| lost.join("\n").matches("is left there").count();
        let locked = |recording: &dyn Fn() -> Recording, end: &dyn Fn(Recording) -> Vec<String>| {
            mode(0o755);
            let recording = recording();
            mode(0o555);
            let lost = end(recording);
            mode(0o755);
            left(&lost)
        };
        mode(0o555);
        if std::fs::write(dir.path().join("probe"), "").is_ok() {
            return; // A superuser's.
        }
        let failing = || failing_unnoticed(&dir);
        let pushed = || {
            let mut recording = both(&dir).expect("begun");
            recording.push(noise(0));
            recording
        };
        let stop = |mut recording: Recording| {
            recording.stop("the canvas changed".to_owned());
            recording.lost
        };
        assert_eq!(locked(&pushed, &stop), 2, "both, stopped");
        assert_eq!(locked(&failing, &stop), 2, "one, and one failing by itself");
        assert_eq!(locked(&pushed, &given_up), 2, "both, given up");
        assert_eq!(locked(&failing, &given_up), 2);
        let finish = |recording| finished(recording).1;
        assert_eq!(locked(&failing, &finish), 1, "the MP4, at its end");
        let each = |mut recording: Recording| {
            recording.each(|_| Err("no".to_owned()));
            recording.lost
        };
        assert_eq!(locked(&pushed, &each), 2, "both, as they fail");
        let outgrow = |mut recording: Recording| {
            recording.outgrow("too large");
            recording.lost
        };
        assert_eq!(locked(&pushed, &outgrow), 1, "the MP4, too large");
    }
}
