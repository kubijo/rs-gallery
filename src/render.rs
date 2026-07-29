//! Headless scene → PNG, for a caller with no screen. Only the canvas is captured, through the same
//! [`render_canvas`] the shell draws with, so a picture holds still as the chrome around it changes.
//!
//! `--render` takes a scene at its defaults; setting knobs takes a [`Recipe`], which keeps labels
//! containing spaces out of argv and makes a set of states reviewable and re-runnable.
//!
//! Knobs are declarative-by-use, so an override cannot be seeded up front — hence the frame protocol
//! in [`shoot`]: declare, apply, redraw, capture.

use std::{collections::BTreeMap, fmt::Write as _};

use anstyle::{AnsiColor, Style};
use camino::{Utf8Path, Utf8PathBuf};
use indoc::formatdoc;

use eframe::{egui_glow, glow};

#[cfg(not(target_vendor = "apple"))]
use crate::glow_capture::GlowCapture;

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
    /// An exact scene key, else a case-insensitive regex; must name exactly one scene.
    pub(crate) scene: String,
    /// Where the PNG goes. `None` for a listing-only run.
    pub(crate) out: Option<Utf8PathBuf>,
    pub(crate) size: egui::Vec2,
    /// Knob key (exact label, else a case-insensitive regex) to the value it should take.
    pub(crate) knobs: Vec<(String, String)>,
    pub(crate) frames: Option<u32>,
    /// Print the scene's knobs, their kinds and their values.
    pub(crate) list: bool,
    /// Print a capture recipe for this scene, its knobs written out at the values found.
    pub(crate) template: bool,
}

/// The window's own inner size (see `run_with`),
/// so a headless canvas defaults to a shape the shell already renders at.
pub(crate) const DEFAULT_SIZE: egui::Vec2 = egui::vec2(1280.0, 720.0);

/// Frames drawn before the capture.
///
/// One declares the knobs, the second draws with them applied,
/// and the rest let egui settle — a grid or a scroll area lays itself out
/// from the previous frame's measurements, as the knob-panel test in `knobs.rs` also relies on.
const DEFAULT_FRAMES: u32 = 4;

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
    let mut written = Vec::new();
    let mut panels = Vec::new();
    let mut outcome = Ok(());
    for shot in &capture.shots {
        match shoot(manifest, renderer, setup, shot) {
            Ok(None) => {}
            Ok(Some((made, image))) => {
                // Held only for a sheet, so a run without one keeps a single capture in memory
                // rather than every capture it has taken.
                if capture.sheet.is_some() {
                    panels.push(Panel {
                        name: made.path.file_stem().unwrap_or("shot").to_owned(),
                        image,
                    });
                }
                written.push(made);
            }
            Err(failure) => {
                outcome = Err(failure);
                break;
            }
        }
    }
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
            match gather(panels, out, renderer, setup) {
                Ok(sheet) => written.push(sheet),
                Err(failure) => outcome = Err(failure),
            }
        }
    }
    // Reported before the failure is returned: the shots that did run are on disk either way,
    // and an unlisted stale PNG is one a reader will trust.
    report(&written, skipped.as_ref());
    outcome
}

/// What one invocation asks for.
pub(crate) struct Capture {
    pub(crate) shots: Vec<Shot>,
    /// Where to gather the captures. `None` unless a recipe asked for a sheet.
    pub(crate) sheet: Option<Utf8PathBuf>,
}

/// Draw the run's captures onto one sheet and write it.
fn gather(
    panels: Vec<Panel>,
    out: &Utf8Path,
    renderer: Renderer,
    setup: &impl Fn(&egui::Context),
) -> Result<Written, Diagnostic> {
    let image = sheet::compose(panels, renderer, setup)?;
    write_png(&image, out)?;
    Ok(Written {
        path: landed(out),
        size: (image.width(), image.height()),
        bytes: std::fs::metadata(out).map(|file| file.len()).unwrap_or(0),
    })
}

/// A PNG this run produced, held back so the whole set can be reported in one block.
struct Written {
    path: Utf8PathBuf,
    size: (u32, u32),
    bytes: u64,
}

/// The images the run wrote, and anything it has to say about one it didn't.
///
/// Each block opens on a blank line and closes on none.
/// A note under the list is then set off by one line rather than two,
/// and the shell prompt lands one line under whichever block came last.
fn report(written: &[Written], skipped: Option<&Diagnostic>) {
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
fn rows(written: &[Written]) -> Vec<String> {
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
        .map(|w| w.size.0.to_string().len())
        .max()
        .unwrap_or_default();
    written
        .iter()
        .map(|w| {
            let pad = " ".repeat(column - w.path.as_str().chars().count());
            let size = format!("{:>digits$}×{}", w.size.0, w.size.1);
            format!("{}{pad}   {}", link(&w.path), paint(aside, &size))
        })
        .collect()
}

/// The painter a glow capture draws with, which is the one a scene has to register
/// its offscreen texture on. `None` under wgpu, whose capture needs no GL context.
pub(crate) type GlPainter = std::rc::Rc<std::cell::RefCell<egui_glow::Painter>>;

/// A harness drawing `app` headlessly at `size`, on the renderer the consumer configured.
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
    renderer: Renderer,
    setup: &impl Fn(&egui::Context),
    app: impl FnOnce(&eframe::CreationContext<'_>, Option<GlPainter>) -> A,
) -> Result<egui_kittest::Harness<'static, A>, Diagnostic> {
    let builder = egui_kittest::Harness::builder()
        .with_size(size)
        // Pinned, so the PNG's pixel dimensions are the requested size rather than a scaled one.
        .with_pixels_per_point(1.0);
    #[cfg(not(target_vendor = "apple"))]
    let (painter, builder) = match renderer {
        Renderer::Glow => {
            let capture = GlowCapture::new()?;
            (Some(capture.painter()), builder.renderer(capture))
        }
        Renderer::Wgpu => (None, builder),
    };
    #[cfg(target_vendor = "apple")]
    let (painter, builder) = match renderer {
        Renderer::Glow => {
            return Err(
                Diagnostic::new("this platform has no headless glow capture")
                    .hint("configure `Renderer::Wgpu`, whose capture needs no GL context"),
            );
        }
        Renderer::Wgpu => (None, builder),
    };
    Ok(builder.build_eframe(|cc| {
        install_context(cc, setup);
        app(cc, painter)
    }))
}

/// Draw `scene` at `size`, applying the shot's knobs as the frames go by.
///
/// Frame one declares the knobs at their defaults; each later frame
/// gets another chance to apply overrides, because setting one knob
/// can be what makes the next one exist.
///
/// # Errors
/// For a key that named no knob by the last frame.
fn draw(
    scene: SceneEntry,
    renderer: Renderer,
    setup: &impl Fn(&egui::Context),
    shot: &Shot,
    size: egui::Vec2,
) -> Result<egui_kittest::Harness<'static, Canvas>, Diagnostic> {
    let mut harness = open(size, renderer, setup, |cc, painter| Canvas {
        scene,
        knobs: Vec::new(),
        gl: cc.gl.clone(),
        loader: cc.get_proc_address.clone(),
        painter,
        target: None,
        wanted: egui::Vec2::ZERO,
    })?;
    harness.run_steps(1);
    let mut pending: Vec<&(String, String)> = shot.knobs.iter().collect();
    for _ in 1..shot.frames.unwrap_or(DEFAULT_FRAMES).max(2) {
        apply(&mut harness.state_mut().knobs, &mut pending)?;
        harness.run_steps(1);
    }
    if !pending.is_empty() {
        return Err(unknown_knobs(&pending, &harness.state().knobs));
    }
    Ok(harness)
}

/// Draw one shot: resolve its scene, draw it at a size that holds it, then capture.
fn shoot(
    manifest: &Manifest,
    renderer: Renderer,
    setup: &impl Fn(&egui::Context),
    shot: &Shot,
) -> Result<Option<(Written, image::RgbaImage)>, Diagnostic> {
    let scene = *resolve_scene(&manifest.scenes, &shot.scene)?;
    let mut harness = draw(scene, renderer, setup, shot, shot.size)?;

    // The canvas scrolls, so a scene that outgrew its size would be cropped to it.
    // A shot names the size to lay out at, not how much of the result to keep,
    // so the second drawing gets the size the first one asked for.
    let wanted = harness.state().wanted;
    let fitting = egui::vec2(
        shot.size.x.max(wanted.x.ceil()),
        shot.size.y.max(wanted.y.ceil()),
    );
    if fitting != shot.size {
        // Let go of the first before asking for the second:
        // a glow capture holds an EGL context, and two
        // of them alive at once takes the process down.
        drop(harness);
        harness = draw(scene, renderer, setup, shot, fitting)?;
    }

    if shot.list {
        for knob in &harness.state().knobs {
            println!("{}", describe(knob));
        }
    }
    if shot.template {
        print!(
            "{}",
            capture_template(&scene, &harness.state().knobs, shot.size)
        );
    }
    if let Some(out) = &shot.out {
        let image = harness
            .render()
            .map_err(|reason| format!("render `{}`: {reason}", scene.name))?;
        write_png(&image, out)?;
        let written = Written {
            path: landed(out),
            size: (image.width(), image.height()),
            bytes: std::fs::metadata(out).map(|file| file.len()).unwrap_or(0),
        };
        return Ok(Some((written, image)));
    }
    Ok(None)
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
    target: Option<RenderTarget>,
    /// What the last frame's canvas came to — the size a shot has to be at least as big as.
    wanted: egui::Vec2,
}

impl eframe::App for Canvas {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Split so the painter and the target can be borrowed at once.
        let Self {
            scene,
            knobs,
            gl,
            loader,
            painter,
            target,
            wanted,
        } = self;
        let gl_deps = match (loader.clone(), gl.as_deref(), painter.as_ref()) {
            (Some(loader), Some(gl), Some(painter)) => Some(GlDeps {
                loader,
                gl,
                // Through the painter rather than the `Frame`, which a harness builds
                // without the glow hook that nothing outside eframe can supply.
                register: Box::new(|texture| painter.borrow_mut().register_native_texture(texture)),
                target,
            }),
            // Under wgpu, `SceneCtx::offscreen` draws its "needs the glow renderer" hint, as in a window.
            _ => None,
        };
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(PANEL_BG))
            .show(ui, |ui| {
                *wanted = render_canvas(ui, scene, knobs, gl_deps);
            });
    }
}

/// Apply each pending override whose knob the scene has now declared,
/// and leave the rest pending.
///
/// A key matching nothing yet is not an error
/// — it may name a knob that appears only once another is set.
///
/// A key matching a knob that can't take the value fails straight away,
/// as does one matching several knobs, which no later frame can disambiguate.
fn apply(knobs: &mut [Knob], pending: &mut Vec<&(String, String)>) -> Result<(), Diagnostic> {
    let mut deferred = Vec::new();
    for over in std::mem::take(pending) {
        let (key, value) = over;
        match find(knobs, key)? {
            Some(at) => set(&mut knobs[at], value)?,
            None => deferred.push(over),
        }
    }
    *pending = deferred;
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
fn unknown_knobs(pending: &[&(String, String)], declared: &[Knob]) -> Diagnostic {
    let named = pending
        .iter()
        .map(|(key, _)| format!("`{key}`"))
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
    /// Gather every shot onto one image here, alongside their own PNGs. Off unless asked for.
    sheet: Option<Utf8PathBuf>,
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
    frames: Option<u32>,
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
            Ok(Shot {
                scene: shot.scene.clone(),
                out: Some(base.join(format!("{}.png", shot.name))),
                size: parse_size(size).map_err(|e| format!("shot `{}`: {e}", shot.name))?,
                knobs: shot
                    .knobs
                    .iter()
                    .map(|(key, value)| Ok((key.clone(), scalar(key, value)?)))
                    .collect::<Result<_, String>>()?,
                frames: shot.frames,
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
fn capture_template(scene: &SceneEntry, knobs: &[Knob], size: egui::Vec2) -> String {
    let (width, height) = (size.x, size.y);
    let (name, key) = (slug(scene.name), scene_key(scene));
    let mut out = formatdoc! {r#"
        # Generated by `--init-capture`: every knob at the value its scene declared.
        # Renders what `--render` alone would — change a value and it renders something else.
        out = "renders"
        size = "{width}x{height}"
        # Uncomment once there is a second shot: gathers them onto one captioned image.
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
            Written {
                path: "/tmp/a.png".into(),
                size: (640, 360),
                bytes: 1,
            },
            Written {
                path: "/tmp/a-much-longer-name.png".into(),
                size: (1280, 720),
                bytes: 1,
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

    /// The `(key, value)` pairs a recipe or the CLI would have produced.
    fn knobs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    /// Apply `pairs` to `knobs`, as a frame of [`shoot`] would.
    fn applied(store: &mut [Knob], pairs: &[(&str, &str)]) -> Result<usize, Diagnostic> {
        let owned = knobs(pairs);
        let mut pending: Vec<&(String, String)> = owned.iter().collect();
        apply(store, &mut pending)?;
        Ok(pending.len())
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
        let mut pending: Vec<&(String, String)> = owned.iter().collect();
        apply(&mut store, &mut pending).expect("an unmatched key is not yet a failure");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].0, "headlights");

        let message = unknown_knobs(&pending, &store).plain();
        assert!(message.contains("`headlights`"), "names what was not found");
        assert!(
            message.contains("night"),
            "lists what the scene does declare"
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
            capture_template(&scene, &knobs, egui::vec2(480.0, 480.0)),
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
        let generated = capture_template(&scene, &declared, egui::vec2(320.0, 200.0));

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
        let mut pending: Vec<&(String, String)> = shots[0].knobs.iter().collect();
        apply(&mut store, &mut pending).expect("every generated key names its knob");
        assert!(pending.is_empty(), "no key went unmatched");
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
    fn draws_offscreen(ctx: &mut crate::SceneCtx<'_>) {
        ctx.offscreen([32_u32, 32], |target| {
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

    /// A scene of the kind the shell is for: prose, and a fixed-size stage on the checkerboard.
    fn a_documented_stage(ctx: &mut crate::SceneCtx<'_>) {
        ctx.ui.heading("Snapshot");
        ctx.ui.label("prose above the stage");
        crate::stage!(ctx, (96, 40), |ui: &mut egui::Ui| {
            ui.label("in the stage");
        });
    }

    /// Each way a stage can be sized, so one reference covers the conversions rather than one of them.
    fn every_stage_form(ctx: &mut crate::SceneCtx<'_>) {
        ctx.ui.label("fitted");
        crate::stage!(ctx, |ui: &mut egui::Ui| {
            ui.label("hugs its content");
        });
        ctx.ui.label("pinned");
        crate::stage!(ctx, (150, 36), |ui: &mut egui::Ui| {
            ui.label("150×36");
        });
        ctx.ui.label("square");
        crate::stage!(ctx, 64, |ui: &mut egui::Ui| {
            ui.label("64");
        });
        // Content past its box, so the reference shows a fixed scrolling stage keeping to the size
        // it declared. The checkerboard closing underneath is the whole point of the picture.
        ctx.ui.label("scrolling");
        ctx.stage(
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
    fn glyphs_past_the_default_faces(ctx: &mut crate::SceneCtx<'_>) {
        ctx.ui.heading("→ ∑ ≈ ± °");
        ctx.ui.label("arrows ← ↑ ↓ →");
        ctx.ui.label("math ∀ ∈ ∞ √");
        ctx.ui.label("symbols ✓ ✗ ★ ♦");
    }

    /// Everything drawn here comes from a knob, so its reference is a picture of the override path.
    fn dressed_by_knobs(ctx: &mut crate::SceneCtx<'_>) {
        let caption = ctx.text("caption", "the default");
        let tint = ctx.color("tint", egui::Color32::WHITE);
        let width = ctx.slider("width", 70.0, 20.0, 260.0, 1.0);
        crate::stage!(ctx, (width, 48.0), |ui: &mut egui::Ui| {
            ui.label(egui::RichText::new(caption).color(tint).size(20.0));
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
                list: false,
                template: false,
            }],
            sheet: None,
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
        fn scrolling_box(ctx: &mut crate::SceneCtx<'_>) {
            ctx.stage(
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
                list: false,
                template: false,
            }],
            sheet: None,
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
        assert_eq!(
            shots[0].knobs,
            knobs(&[("body style", "SUV"), ("night", "true"), ("speed", "1.5")]),
            "TOML scalars flatten to the text the per-kind parser reads"
        );
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
