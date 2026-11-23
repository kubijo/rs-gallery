//! Recording a shot: the clock its frames are drawn on,
//! and the frames it hands to the encoder.

use std::time::{Duration, Instant};

use super::*;
use crate::video::Recorder;

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

/// Start sending every frame drawn, on a canvas of `size` points
/// at `scale`, to a recording beside `out` in each of its formats.
///
/// # Errors
/// When a file cannot be created, or the input overlay's harness cannot be opened.
pub(super) fn start(
    harness: &mut egui_kittest::Harness<'static, Canvas>,
    session: &Session,
    (size, scale): (egui::Vec2, f32),
    out: &Utf8Path,
    record: Record,
) -> Result<(), Diagnostic> {
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
        .map(|corner| InputOverlay::open(session, size, scale, record.fps, corner))
        .transpose()?;
    harness.state_mut().recording = Some(Recording {
        sinks,
        fps: record.fps,
        hold: record.hold_frames(),
        drew_since_hold: false,
        input,
        draw: Duration::ZERO,
        capture: Duration::ZERO,
    });
    Ok(())
}

/// Pause on the frame last drawn, at a play's step end, when recording.
///
/// # Errors
/// When the encoder has failed.
pub(super) fn hold(harness: &mut egui_kittest::Harness<'static, Canvas>) -> Result<(), String> {
    let Some(recording) = harness.state_mut().recording.as_mut() else {
        return Ok(());
    };
    if !std::mem::take(&mut recording.drew_since_hold) {
        return Ok(());
    }
    if let Some(input) = recording.input.as_mut() {
        input.hold(recording.hold);
    }
    for (_, _, sink) in &mut recording.sinks {
        sink.hold(recording.hold)?;
    }
    Ok(())
}

/// Close the recording, if one is open: a file per format,
/// none when no recording was.
///
/// # Errors
/// When nothing was recorded,
/// or a file could not be written.
pub(super) fn finish(
    harness: &mut egui_kittest::Harness<'static, Canvas>,
) -> Result<Vec<VideoOutcome>, String> {
    let Some(recording) = harness.state_mut().recording.take() else {
        return Ok(Vec::new());
    };
    recording
        .sinks
        .into_iter()
        .map(|(format, path, sink)| {
            let encoded = sink.finish()?;
            Ok(VideoOutcome {
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
            })
        })
        .collect()
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
        .is_some_and(|recording| recording.input.is_some());
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
    if harness.state().recording.is_none() {
        return Ok(());
    }
    let started = Instant::now();
    // Caught here, by the `std` that raised it: unwinding
    // into a scenes dylib's frames, a panic would meet
    // the dylib's own `std`, which aborts on it.
    let mut image = std::panic::catch_unwind(AssertUnwindSafe(|| harness.render()))
        .map_err(|panic| format!("render a recorded frame: {}", panic_message(&panic)))??;
    let recording = harness
        .state_mut()
        .recording
        .as_mut()
        .expect("BUG: the recording checked open above closed while rendering");
    if let Some(input) = recording.input.as_mut() {
        input.lay_over(&events, &mut image)?;
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
