//! One shot: draw the scene, settle it under its overrides, run its play, fit and capture it.

use super::*;
use crate::style::Stream;

/// Draw `scene` at `size`, applying the shot's knobs as the frames go by.
///
/// Frame one declares the knobs at their defaults; every later frame re-applies the overrides
/// — setting one knob can be what makes the next one exist,
/// and a scene writing its own is overruled.
///
/// # Errors
/// As [`settle`].
#[cfg(test)]
pub(super) fn draw(
    scene: SceneEntry,
    session: &Session,
    setup: &impl Fn(&egui::Context),
    shot: &Shot,
    size: egui::Vec2,
) -> Result<SceneFrame, Diagnostic> {
    draw_with_globals(scene, None, session, setup, shot, size)
}

pub(super) fn draw_with_globals(
    scene: SceneEntry,
    globals: Option<GlobalEntry>,
    session: &Session,
    setup: &impl Fn(&egui::Context),
    shot: &Shot,
    size: egui::Vec2,
) -> Result<SceneFrame, Diagnostic> {
    let wgpu_session = matches!(session, Session::Wgpu(_));
    // A shot with nowhere to write records nothing either.
    let recorded = shot
        .record
        .filter(|_| shot.out.is_some())
        .map(|record| record.fps);
    let globals = globals
        .map(|entry| GlobalState::new(entry, crate::SceneRevision::INITIAL))
        .transpose()
        .map_err(Diagnostic::from)?;
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
            globals,
            knobs: Vec::new(),
            pinned: Vec::new(),
            bound: Bound::default(),
            gl: cc.gl.clone(),
            loader: cc.get_proc_address.clone(),
            painter,
            targets: Vec::new(),
            wgpu,
            passes: Vec::new(),
            wanted: egui::Vec2::ZERO,
            global_write_error: None,
            actions: None,
            clock: recorded.map(Clock::new),
            recording: None,
            scene_panic: None,
            quieten: None,
        }
    })?;
    if recorded.is_some() {
        animate(&harness.ctx);
    }
    let frames = settle(&mut harness, shot, &shot.knobs)?;
    Ok(SceneFrame { harness, frames })
}

/// A scene drawn and settled, and how it was still moving when the drawing stopped.
pub(super) struct SceneFrame {
    pub(super) harness: egui_kittest::Harness<'static, Canvas>,
    pub(super) frames: Frames,
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

/// Frames that must ask for no repaint before a `settle` shot
/// or a snapshot check calls the scene still.
///
/// More than one because the signal belongs to the egui context rather than to the scene:
/// a one-shot repaint from anything on the canvas would end the wait early on a scene
/// still moving. Two consecutive quiet frames cost a frame and take that away.
pub(super) const QUIET_FRAMES: u32 = 2;

/// Draw the frames a shot asks for, applying its overrides as they go by.
///
/// Split out so a resize can run it again on the same harness — [`shoot`] has the why.
///
/// Reports [`Motion::StillMoving`] only for a `settle` shot
/// that reached its frame count mid-animation.
///
/// # Errors
/// For a key that named no knob by the last frame, a value its knob couldn't take by then,
/// a key that landed on another label than before, or, at once, a bad value or ambiguous key.
fn settle(
    harness: &mut egui_kittest::Harness<'static, Canvas>,
    shot: &Shot,
    knobs: &Overrides,
) -> Result<Frames, Diagnostic> {
    settle_with(harness, shot, knobs, Knobs::Declared)
}

/// Whether every knob key must name a knob some frame declares.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Knobs {
    Declared,
    /// After a play: the keys were checked already, and the play's state may hide their knobs.
    Held,
}

/// [`settle`], with `knobs` the shot's plus whatever a play set.
fn settle_with(
    harness: &mut egui_kittest::Harness<'static, Canvas>,
    shot: &Shot,
    knobs: &Overrides,
    require: Knobs,
) -> Result<Frames, Diagnostic> {
    let mut unmatched_globals: Vec<&KnobOverride> = shot.globals.iter().collect();
    apply_globals(harness.state_mut(), &shot.globals, &mut unmatched_globals)?;
    step(harness).map_err(Diagnostic::new)?;
    let mut ledger = Ledger::new(knobs.clone());
    let mut quiet = 0;
    let mut drawn = 1;
    // The floor is defence rather than adjustment: both ways in reject a smaller `frames`,
    // and a ceiling below it would run the loop no times, applying the recipe never.
    for _ in 1..shot.frames.unwrap_or(DEFAULT_FRAMES).max(MIN_FRAMES) {
        apply_globals(harness.state_mut(), &shot.globals, &mut unmatched_globals)?;
        ledger.apply(harness.state_mut())?;
        step(harness).map_err(Diagnostic::new)?;
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
    if let Some(error) = harness.state_mut().global_write_error.take() {
        return Err(Diagnostic::new(error));
    }
    // The loop ends on a draw, so a scene's own writes would outlive the last apply.
    // One more settles the store on the recipe, which is what `--list-knobs` then reports.
    apply_globals(harness.state_mut(), &shot.globals, &mut unmatched_globals)?;
    ledger.apply(harness.state_mut())?;
    if !unmatched_globals.is_empty() {
        let declared = harness
            .state()
            .globals
            .as_ref()
            .map_or(&[][..], GlobalState::knobs);
        return Err(unknown_keys(KeyKind::Global, &unmatched_globals, declared));
    }
    // Unknown keys first, so a typo is not hidden behind a reason for another run.
    let (not_yet, unknown): (Vec<&KnobOverride>, Vec<&KnobOverride>) = ledger
        .unlanded()
        .partition(|over| ledger.reason(&over.key).is_some());
    if require == Knobs::Declared && !unknown.is_empty() {
        return Err(unknown_keys(
            KeyKind::Knob,
            &unknown,
            &harness.state().knobs,
        ));
    }
    // A value its knob can't take now, or one that never landed and couldn't when last tried.
    // After a play, a key whose knob the play hid is the play's to answer for.
    let never_landed = match require {
        Knobs::Declared => not_yet.first().and_then(|over| ledger.reason(&over.key)),
        Knobs::Held => None,
    };
    if let Some(reason) = ledger.not_yet_now().or(never_landed) {
        return Err(Diagnostic::new(reason));
    }
    let motion = if !shot.settle || quiet >= QUIET_FRAMES {
        Motion::Settled
    } else {
        Motion::StillMoving
    };
    Ok(Frames { drawn, motion })
}

/// Draw one shot: resolve its scene, draw it at a size that holds it, then capture.
#[cfg(test)]
pub(super) fn shoot(
    manifest: &Manifest,
    session: &Session,
    setup: &impl Fn(&egui::Context),
    shot: &Shot,
) -> Result<Option<ShotOutput>, Diagnostic> {
    let mut view = LogView::off();
    shoot_with_globals(
        manifest,
        None,
        &[],
        session,
        setup,
        shot,
        &mut Watch::new(&mut view),
    )
}

/// What a run keeps of a shot's play besides the image.
pub(super) struct Watch<'a> {
    /// Where the play's log is shown as it goes.
    pub(super) view: &'a mut LogView,
    /// What a play that stopped the shot did, for the run's report.
    pub(super) failed: Option<FailedShot>,
    /// How to judge the play's snapshot checks. `None` in a capture, which makes pictures
    /// at any size, scale and knobs: no reference means one of those.
    pub(super) snapshots: Option<&'a Snapshots>,
    /// How to film the play.
    /// `None` in a capture, which records what its shot asks for.
    pub(super) films: Option<&'a Films>,
    /// What a play that passed did, of a shot
    /// that writes no image to report it with.
    pub(super) played: Option<PlayOutcome>,
    /// The play's video, when it is kept.
    pub(super) filmed: Option<Filmed>,
    /// Where a failure is reported.
    pub(super) stream: Stream,
    /// What is amiss that fails nothing.
    pub(super) warnings: Vec<Diagnostic>,
}

impl<'a> Watch<'a> {
    pub(super) fn new(view: &'a mut LogView) -> Self {
        Self {
            view,
            failed: None,
            snapshots: None,
            films: None,
            played: None,
            filmed: None,
            stream: Stream::Stderr,
            warnings: Vec::new(),
        }
    }

    /// Note that something is amiss with a play's video.
    fn unfilmed(&mut self, play: &str, why: String) {
        let warning = Diagnostic::warning(format!("no video of play `{play}`"));
        self.warnings.push(warning.cause(why));
    }
}

pub(super) fn shoot_with_globals(
    manifest: &Manifest,
    globals: Option<GlobalEntry>,
    plays: &[PlayEntry],
    session: &Session,
    setup: &impl Fn(&egui::Context),
    shot: &Shot,
    watch: &mut Watch<'_>,
) -> Result<Option<ShotOutput>, Diagnostic> {
    let scene = *resolve_scene(&manifest.scenes, &shot.scene)?;
    if let Some(films) = watch.films
        && let Some(play) = &shot.play
        && let Ok(entry) = find_play(plays, &scene, &play.name)
        && let Err(why) = films.clear(entry, session.renderer_name())
    {
        watch.unfilmed(&play.name, why);
    }
    let SceneFrame {
        mut harness,
        mut frames,
    } = draw_with_globals(scene, globals, session, setup, shot, shot.size)?;
    let mut size = shot.size;
    fit(
        &mut harness,
        session,
        shot,
        &shot.knobs,
        Knobs::Declared,
        &mut size,
        &mut frames,
    )?;

    // A shot with nowhere to write records nothing either.
    let recording = shot.record.zip(shot.out.as_ref());
    let (played, mut videos) = match &shot.play {
        None => (None, Vec::new()),
        Some(play) => {
            let entry = find_play(plays, &scene, &play.name)?;
            let trace_dir = shot.out.as_ref().map(|out| steps_dir(out));
            if let Some(dir) = &trace_dir {
                clear_trace(dir)?;
            }
            // A failed play writes no image, so an earlier run's would pass for this one's.
            if let Some(out) = &shot.out {
                match std::fs::remove_file(out) {
                    Ok(()) => {}
                    Err(missing) if missing.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(format!("remove the earlier `{out}`: {e}").into()),
                }
            }
            harness.state_mut().actions = Some(Vec::new());
            if let Some((record, out)) = recording {
                start(&mut harness, session, shot.scale, out, record)
                    .map_err(|why| unrecorded(out).cause(why))?;
            }
            let mut film = watch
                .films
                .map(|films| films.begin(&mut harness, session.renderer_name(), entry))
                .unwrap_or(Ok(None));
            let mut driver = HeadlessDriver {
                harness: &mut harness,
                globals: &shot.globals,
                ledger: Ledger::new(shot.knobs.clone()),
                scale: shot.scale,
                trim: shot.trim,
                trace_dir: trace_dir.clone(),
                bad_knob: None,
                view: watch.view,
                session,
                size: &mut size,
                looks: watch
                    .snapshots
                    .map(|snapshots| Looks::new(snapshots, session.renderer_name())),
            };
            let name = shot
                .out
                .as_deref()
                .and_then(Utf8Path::file_stem)
                .unwrap_or(&shot.scene);
            driver.view.begin(name, &play.name, play.steps);
            let mut run = crate::play::run(entry, &mut driver);
            let ledger = driver.ledger;
            harness.state_mut().actions = None;
            if let Ok(Some(film)) = &mut film {
                film.end(&mut harness);
            }
            watch.view.end(run.failure.is_some(), run.frames);
            let (videos, lost) = finish(&mut harness);
            let mut video_paths: Vec<Utf8PathBuf> =
                videos.iter().map(|video| video.path.clone()).collect();
            // A failure's report, with where its play is recorded.
            let mut with_video = |mut report: Diagnostic, film: Result<Option<Film>, String>| {
                let keep = |film: Film| film.keep(session, shot.scale);
                match film.and_then(|film| film.map(keep).transpose()) {
                    Ok(kept) => {
                        video_paths.extend(kept.iter().map(|film| film.path.clone()));
                        watch.filmed = kept;
                    }
                    Err(why) => report = report.hint(format!("the play is not recorded: {why}")),
                }
                if !video_paths.is_empty() {
                    let paths: Vec<String> = video_paths
                        .iter()
                        .map(|path| crate::style::path(path, watch.stream))
                        .collect();
                    let paths = paths.join(" and ");
                    report = report.recorded(format!("the play is recorded in {paths}"));
                }
                (report, std::mem::take(&mut video_paths))
            };
            // Reported before anything else is drawn: settling again
            // could fail on the very thing the play did, and that
            // would stop the run instead of reporting the play.
            if let Some(failure) = &run.failure {
                let at = failure
                    .step()
                    .map(|step| format!(" at step `{step}`"))
                    .unwrap_or_default();
                let mut report = Diagnostic::new(format!(
                    "play `{}` on `{}` failed{at}",
                    play.name, scene.name
                ));
                report = match failure.check() {
                    Some(check) => {
                        if let Some(message) = &check.message {
                            report = report.cause(message);
                        }
                        report = report
                            .cause(format!("expected {} {}", check.subject, check.expected))
                            .cause(format!(
                                "found {}",
                                check.actual.as_deref().unwrap_or("nothing matched")
                            ));
                        if let Some(explanation) = &check.explanation {
                            report = report.cause(explanation);
                        }
                        match &check.kept {
                            Some(kept) => with_kept(report, kept, watch.stream),
                            None => report,
                        }
                    }
                    None => report.cause(failure.reason()),
                };
                for path in run.log.iter().filter_map(Entry::taken) {
                    report = report.hint(format!(
                        "taken as a reference before it failed: {}",
                        crate::style::path(path, watch.stream)
                    ));
                }
                if !failure.on_screen().is_empty() {
                    report = report.cause(on_screen(failure.on_screen()));
                }
                // Likely why: a knob is not at the value set for it.
                for reason in ledger.not_yet_reasons() {
                    report = report.hint(format!("not at the value set: {reason}"));
                }
                if let Some(at) = failure.at() {
                    let label = if failure.check().is_some() {
                        "expected here"
                    } else {
                        "failed here"
                    };
                    report = report.source_at(&at.file, at.line, at.column, label);
                }
                if let Some(dir) = &trace_dir
                    && run.log.iter().any(|entry| entry.trace.is_some())
                {
                    report = report.hint(format!(
                        "screenshots of each step are in {}",
                        crate::style::path(&landed(dir), watch.stream)
                    ));
                }
                let (report, video_paths) = with_video(with_lost(report, &lost), film);
                let kept = failure.check().and_then(|check| check.kept.as_ref());
                let report = with_offer(report, kept);
                watch.failed = Some(FailedShot::of(
                    shot,
                    &scene,
                    play,
                    failure.clone(),
                    &mut run,
                    video_paths,
                    lost,
                ));
                return Err(report);
            }
            // The recipe's knobs were checked before the play and the play's own at its end,
            // so a knob the play's state has hidden is no error here.
            let knobs = ledger.overrides();
            let settled = settle_with(&mut harness, shot, knobs, Knobs::Held).and_then(|drawn| {
                frames = drawn;
                // A play can grow the canvas — a menu it opened, a row it added.
                fit(
                    &mut harness,
                    session,
                    shot,
                    knobs,
                    Knobs::Held,
                    &mut size,
                    &mut frames,
                )
            });
            // What the play left behind is the shot's finding, not the run's end;
            // so is a recording it asked for and does not have.
            let failed = match settled {
                Err(report) => Some((
                    "the state it left fails the shot",
                    vec![report.summary()],
                    &lost[..],
                )),
                Ok(()) if !lost.is_empty() => {
                    Some(("its recording failed", lost.clone(), Default::default()))
                }
                Ok(()) => None,
            };
            if let Some((what, reasons, untold)) = failed {
                let error = crate::play::PlayError::new(reasons.join("; "));
                let headline = format!(
                    "play `{}` on `{}` passed, but {what}",
                    play.name, scene.name
                );
                let report = reasons
                    .into_iter()
                    .fold(Diagnostic::new(headline), Diagnostic::cause);
                let (report, video_paths) = with_video(with_lost(report, untold), film);
                watch.failed = Some(FailedShot::of(
                    shot,
                    &scene,
                    play,
                    error,
                    &mut run,
                    video_paths,
                    lost,
                ));
                return Err(report);
            }
            if watch.films.is_some_and(Films::keeps_passing) {
                let keep = |film: Film| film.keep(session, shot.scale);
                match film.and_then(|film| film.map(keep).transpose()) {
                    Ok(kept) => watch.filmed = kept,
                    Err(why) => watch.unfilmed(&play.name, why),
                }
            }
            // Only now has the shot passed, so only now are its step screenshots let go.
            if play.trace == Trace::Failing {
                if let Some(dir) = &trace_dir {
                    clear_trace(dir)?;
                }
                for entry in &mut run.log {
                    entry.trace = None;
                }
            }
            let videos = match recording {
                Some((record, out)) if record.keep == Trace::Failing => {
                    for video in &videos {
                        let path = video_path(out, video.format);
                        std::fs::remove_file(&path)
                            .map_err(|e| format!("remove the passing play's `{path}`: {e}"))?;
                    }
                    Vec::new()
                }
                _ => videos,
            };
            let outcome = PlayOutcome {
                name: play.name.clone(),
                frames: run.frames,
                log: run.log,
            };
            (Some(outcome), videos)
        }
    };
    if shot.play.is_none()
        && let Some((record, out)) = recording
    {
        start(&mut harness, session, shot.scale, out, record)
            .map_err(|why| unrecorded(out).cause(why))?;
        if let Err(report) = record_frames(&mut harness, shot, record.frames()) {
            return Err(discard(&mut harness, report));
        }
        let (finished, lost) = finish(&mut harness);
        if !lost.is_empty() {
            let mut report = lost.into_iter().fold(unrecorded(out), Diagnostic::cause);
            for video in &finished {
                let path = crate::style::path(&video.path, watch.stream);
                report = report.hint(format!("the shot is recorded in {path}"));
            }
            return Err(report);
        }
        videos = finished;
    }

    if shot.list {
        let print = |line: &str| {
            if shot.template {
                eprintln!("{line}");
            } else {
                println!("{line}");
            }
        };
        if let Some(global) = harness
            .state()
            .globals
            .as_ref()
            .filter(|global| !global.knobs().is_empty())
        {
            print("Global controls:");
            for knob in global.knobs() {
                print(&describe(knob));
            }
            print("Scene knobs:");
        }
        for knob in &harness.state().knobs {
            print(&describe(knob));
        }
    }
    if shot.template {
        print!(
            "{}",
            capture_template_with_globals(
                &scene,
                harness
                    .state()
                    .globals
                    .as_ref()
                    .map_or(&[][..], GlobalState::knobs),
                &harness.state().knobs,
                shot.size,
                shot.scale,
            )
        );
    }
    if let Some(out) = &shot.out {
        let drawn = drawn(&harness);
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
            globals: harness
                .state()
                .globals
                .as_ref()
                .map_or_else(BTreeMap::new, |global| control_values(global.knobs())),
            play: played,
            videos,
        };
        return Ok(Some(ShotOutput { written, image }));
    }
    watch.played = played;
    Ok(None)
}

/// `report`, bound for `stream`, with what a failed snapshot check left to look at.
pub(super) fn with_kept(mut report: Diagnostic, kept: &Kept, stream: Stream) -> Diagnostic {
    if let Some(diff) = &kept.diff {
        let stream_colours = supports_color::on_cached(match stream {
            Stream::Stdout => supports_color::Stream::Stdout,
            Stream::Stderr => supports_color::Stream::Stderr,
        });
        let tinted = stream_colours.is_some_and(|colours| colours.has_256);
        report = report.cause_as_is(diff_lines(diff, tinted));
    }
    for attachment in &kept.attachments {
        report = report.hint(format!(
            "{}: {}",
            attachment.kind.name(),
            crate::style::path(&attachment.path, stream)
        ));
    }
    report
}

/// That the shot written to `out` is not recorded, which ends the run.
fn unrecorded(out: &Utf8Path) -> Diagnostic {
    let name = out.file_stem().unwrap_or("shot");
    Diagnostic::new(format!("the recording of shot `{name}` failed"))
}

/// `report` with why each recording `lost` was.
pub(super) fn with_lost(report: Diagnostic, lost: &[String]) -> Diagnostic {
    lost.iter().fold(report, |report, why| {
        report.hint(format!("not recorded: {why}"))
    })
}

/// `report` with how to take what a failed snapshot check saw, if that is on offer:
/// last, set apart from the files before it.
pub(super) fn with_offer(report: Diagnostic, kept: Option<&Kept>) -> Diagnostic {
    match kept {
        Some(kept) if kept.acceptance == Acceptance::Offered => {
            report.hint_apart("`UPDATE_SNAPSHOTS=1` takes what it saw as the reference")
        }
        _ => report,
    }
}

/// A snapshot's diff in its colours, changed lines `tinted`
/// or not as [`painted`](crate::diagnostic::diff::painted)
/// has them, cut where it would fill the screen.
///
/// It is painted whole before the cut, so its lines
/// pair up as they do in the report.
pub(super) fn diff_lines(diff: &str, tinted: bool) -> String {
    const MOST: usize = 40;
    let shown_lines: Vec<String> = diff.lines().map(shown).collect();
    let painted = crate::diagnostic::diff::painted(
        &shown_lines.join("\n"),
        crate::diagnostic::diff::SNAPSHOTS,
        tinted,
    );
    let mut lines: Vec<String> = painted.lines().take(MOST).map(str::to_owned).collect();
    match diff.lines().count().saturating_sub(MOST) {
        0 => {}
        1 => lines.push("… and 1 more line, in the diff".to_owned()),
        left => lines.push(format!("… and {left} more lines, in the diff")),
    }
    lines.join("\n")
}

/// `text` with what a terminal would act on rather than show spelled out:
/// control characters, and those that turn the direction of what follows.
fn shown(text: &str) -> String {
    let mut shown = String::with_capacity(text.len());
    for c in text.chars() {
        let turns = matches!(
            c,
            '\u{61c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
        );
        if c.is_control() || turns {
            shown.extend(c.escape_unicode());
        } else {
            shown.push(c);
        }
    }
    shown
}

/// What one shot came to: the PNG it wrote,
/// and the image itself for a sheet that wants it.
pub(super) struct ShotOutput {
    pub(super) written: OutputFile,
    pub(super) image: image::RgbaImage,
}

/// How far from the canvas's top left corner the scene drew, in points: its content,
/// and the popups and windows over it, which egui keeps out of the content's size.
/// One parked off the canvas, its edge on the canvas's own, is meant to be out of sight.
pub(super) fn drawn(harness: &egui_kittest::Harness<'static, Canvas>) -> egui::Vec2 {
    let content = harness.state().wanted;
    let canvas = harness.ctx.content_rect();
    harness.ctx.memory(|memory| {
        memory
            .areas()
            .visible_layer_ids()
            .into_iter()
            .filter(|layer| layer.order != egui::Order::Background)
            .filter_map(|layer| memory.area_rect(layer.id))
            .filter(|area| area.intersect(canvas).is_positive())
            .fold(content, |drawn, area| drawn.max(area.max.to_vec2()))
    })
}

/// Grow the canvas to fit what the scene drew, then settle again.
/// The canvas scrolls, so a scene that outgrew its size would be cropped.
/// A shot names the size to lay out at, not how much to keep.
fn fit(
    harness: &mut egui_kittest::Harness<'static, Canvas>,
    session: &Session,
    shot: &Shot,
    knobs: &Overrides,
    require: Knobs,
    size: &mut egui::Vec2,
    frames: &mut Frames,
) -> Result<(), Diagnostic> {
    let fitting = size.max(drawn(harness).ceil());
    if fitting != *size {
        session.validate_target(fitting, shot.scale)?;
        // Resized, not redrawn on a fresh harness: that would drop the GL context a scene cached
        // against (a femtovg canvas, a shader), leaving only the egui parts in the shot.
        // Reuse also keeps two EGL contexts from being alive at once.
        harness.set_size(fitting);
        *size = fitting;
        *frames = settle_with(harness, shot, knobs, require)?;
    }
    Ok(())
}

/// The play named `name` among those registered for `scene`.
fn find_play<'p>(
    plays: &'p [PlayEntry],
    scene: &SceneEntry,
    name: &str,
) -> Result<&'p PlayEntry, Diagnostic> {
    let key = scene_key(scene);
    let own: Vec<&PlayEntry> = plays
        .iter()
        .filter(|play| play.scene.to_string() == key)
        .collect();
    let named: Vec<&PlayEntry> = own
        .iter()
        .copied()
        .filter(|play| play.name == name)
        .collect();
    match named.as_slice() {
        [one] => return Ok(one),
        [] => {}
        several => {
            return Err(Diagnostic::new(format!(
                "scene `{}` has {} plays named `{name}`",
                scene.name,
                several.len()
            ))
            .candidates(
                several
                    .iter()
                    .map(|play| format!("{}::{name}", play.module_path)),
            )
            .hint("rename all but one, so a recipe can say which it means"));
        }
    }
    let failed = Diagnostic::new(format!("scene `{}` has no play `{name}`", scene.name));
    Err(if own.is_empty() {
        failed.hint("register one with `#[play(scene_fn)]` beside the scene")
    } else {
        failed.hint(format!(
            "its plays: {}",
            own.iter()
                .map(|play| format!("`{}`", play.name))
                .collect::<Vec<_>>()
                .join(", ")
        ))
    })
}

/// Crop the background a roomy `size` leaves around the canvas.
///
/// The size stays as asked — `Fill` stages and any breakpoint lay out against it —
/// so only the image is cut. `drawn` is in points,
/// which `scale` turns into the pixels the crop is measured in.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a canvas is a few thousand non-negative pixels at most"
)]
pub(super) fn trim(image: image::RgbaImage, drawn: egui::Vec2, scale: f32) -> image::RgbaImage {
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
pub(super) struct Canvas {
    pub(super) scene: SceneEntry,
    pub(super) globals: Option<GlobalState>,
    pub(super) knobs: Vec<KnobState>,
    /// The knobs the shot's overrides set before this frame; the scene can't write them.
    pub(super) pinned: Vec<Pin>,
    /// The label each knob key has landed on, across every settle and play on this harness.
    pub(super) bound: Bound,
    pub(super) gl: Option<std::sync::Arc<glow::Context>>,
    pub(super) loader: Option<GlLoader>,
    pub(super) painter: Option<GlPainter>,
    pub(super) targets: Vec<RenderTarget>,
    /// `Some` only under a wgpu capture — [`SceneCtx::render_state`](crate::SceneCtx::render_state),
    /// as the window's `Frame` carries it.
    pub(super) wgpu: Option<eframe::egui_wgpu::RenderState>,
    /// This scene's cached `render_pass` targets, as the shell keeps them per scene.
    pub(super) passes: Vec<crate::PassTarget>,
    /// What the last frame's content came to, the popups and windows over it apart: see [`drawn`].
    pub(super) wanted: egui::Vec2,
    /// The first scene write to a catalog global that could not land, failing the shot.
    pub(super) global_write_error: Option<String>,
    /// Actions reported since a play last took them;
    /// `None` while no play listens.
    pub(super) actions: Option<Vec<String>>,
    /// The clock every frame is drawn on,
    /// while the shot records.
    pub(super) clock: Option<Clock>,
    /// Where frames go while a recording is open.
    pub(super) recording: Option<Recording>,
    /// The first panic the scene raised, failing the shot.
    pub(super) scene_panic: Option<crate::context::ScenePanic>,
    /// What keeps the panics of its play off the terminal.
    pub(super) quieten: Option<crate::play::Quieten>,
}

impl eframe::App for Canvas {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Split so the painter and the targets can be borrowed at once.
        let Self {
            scene,
            globals,
            knobs,
            pinned,
            bound: _,
            gl,
            loader,
            painter,
            targets,
            wgpu,
            passes,
            wanted,
            global_write_error,
            actions,
            clock: _,
            recording: _,
            scene_panic,
            quieten: _,
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
                let mut rendered = None;
                let reported = crate::actions::collecting(|| {
                    rendered = Some(render_canvas_pinned(
                        ui,
                        scene,
                        knobs,
                        pinned,
                        gl_deps,
                        wgpu_deps,
                        globals.as_ref(),
                    ));
                });
                if let Some(log) = actions {
                    log.extend(reported);
                }
                let rendered = rendered.expect("`collecting` runs the render it is handed");
                *wanted = rendered.size;
                if scene_panic.is_none() {
                    *scene_panic = rendered.panicked;
                }
                if let Some(globals) = globals
                    && !rendered.global_writes.is_empty()
                {
                    let failed = match globals.write(rendered.global_writes) {
                        Ok(written) => {
                            if written.changed {
                                ui.ctx().request_repaint();
                            }
                            written.missed.into_iter().next()
                        }
                        Err(error) => Some(error),
                    };
                    if global_write_error.is_none() {
                        *global_write_error = failed;
                    }
                }
            });
    }
}

/// Always PNG, whatever the path is called: the caller picked
/// a filename, not a format, and a missing or unexpected extension
/// shouldn't fail a render that already succeeded.
pub(super) fn write_png(image: &image::RgbaImage, out: &Utf8Path) -> Result<(), String> {
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
pub(super) fn landed(out: &Utf8Path) -> Utf8PathBuf {
    out.canonicalize_utf8().unwrap_or_else(|_| out.to_owned())
}
