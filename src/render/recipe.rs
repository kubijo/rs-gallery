//! Capture recipes: reading one into shots, and writing one out from a scene's knobs.

use super::*;

/// One knob as `--list-knobs` prints it: the accessor that declared it, the label quoted (they contain
/// spaces, and the caller has to copy it into a recipe), the current value, and what else it accepts.
pub(super) fn describe(knob: &KnobState) -> String {
    match knob {
        KnobState::Button { label, .. } => format!("button  {label:?}"),
        KnobState::Text { label, value } => format!("text    {label:?} = {value:?}"),
        KnobState::Toggle { label, value } => format!("toggle  {label:?} = {value}"),
        KnobState::Color { label, value } => format!("color   {label:?} = {}", value.to_hex()),
        KnobState::Group { label } => format!("group   {label:?}"),
        KnobState::Slider {
            label,
            value,
            min,
            max,
            step,
        } => format!(
            "slider  {label:?} = {value}  ({}, step {step})",
            Span::new(*min, *max)
        ),
        KnobState::Select {
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
        KnobState::IconButtons {
            label,
            value,
            options,
            ..
        } => format!(
            "buttons {label:?} = {:?}  ({})",
            options.get(*value).map_or("", String::as_str),
            options.join(" | ")
        ),
        KnobState::Pad2D {
            label,
            x,
            y,
            min_x,
            max_x,
            min_y,
            max_y,
            ..
        } => format!(
            "pad2d   {label:?} = {x},{y}  (x {}, y {})",
            Span::new(*min_x, *max_x),
            Span::new(*min_y, *max_y)
        ),
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

/// A capture recipe: the shots to render,
/// and the defaults they fall back on.
///
/// Unknown keys are rejected rather than ignored
/// — a misspelt field would otherwise render a clean
/// picture of the wrong state, which is worse than no picture.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Recipe {
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
    /// Write what the run came to as JSON here.
    /// Off unless asked for.
    report: Option<Utf8PathBuf>,
    /// When to keep a play's step screenshots: `failing` unless set.
    /// A shot can override it.
    trace: Option<Trace>,
    /// Catalog globals inherited by each shot.
    #[serde(default)]
    pub(super) globals: BTreeMap<String, toml::Value>,
    #[serde(default, rename = "shot")]
    pub(super) shots: Vec<RecipeShot>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RecipeShot {
    /// The shot's identity, and the PNG's filename.
    name: String,
    scene: String,
    size: Option<String>,
    scale: Option<f32>,
    frames: Option<u32>,
    trim: Option<bool>,
    settle: Option<bool>,
    #[serde(default)]
    pub(super) knobs: BTreeMap<String, toml::Value>,
    /// Globals overridden for this shot.
    #[serde(default)]
    globals: BTreeMap<String, toml::Value>,
    /// A play to run on the scene before it is captured.
    play: Option<String>,
    trace: Option<Trace>,
    /// A recording beside the shot's PNG.
    record: Option<RecipeRecord>,
    /// When to keep a play's recording: `on` unless set.
    video: Option<Trace>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RecipeRecord {
    fps: u16,
    /// How long a shot without a play records;
    /// a play shot records the play.
    seconds: Option<f32>,
    /// One format, or a list recorded together from the same frames.
    #[serde(default)]
    format: VideoFormats,
    /// Seconds a play's recording pauses at each step's end.
    hold: Option<f32>,
    /// The play's input on the recording: `true`,
    /// or the corner its panel sits in.
    input: Option<RecipeInput>,
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum RecipeInput {
    On(bool),
    In(Corner),
}

impl RecipeInput {
    fn corner(&self) -> Option<Corner> {
        match self {
            Self::On(true) => Some(Corner::default()),
            Self::On(false) => None,
            Self::In(corner) => Some(*corner),
        }
    }
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
            if shot.trace.is_some() && shot.play.is_none() {
                return Err(format!(
                    "shot `{}` sets `trace`, which keeps a play's step screenshots, but runs no `play`",
                    shot.name
                ));
            }
            let scale = shot.scale.or(recipe.scale).unwrap_or(DEFAULT_SCALE);
            check_scale(scale).map_err(|e| format!("shot `{}`: {e}", shot.name))?;
            if shot.video.is_some() && shot.record.is_none() {
                return Err(format!(
                    "shot `{}` sets `video`, which keeps a recording, but has no `record`",
                    shot.name
                ));
            }
            let record = shot
                .record
                .as_ref()
                .map(|record| {
                    let record = Record {
                        fps: record.fps,
                        seconds: record.seconds,
                        keep: shot.video.unwrap_or(Trace::Always),
                        formats: record.format,
                        hold: record.hold,
                        input: record.input.as_ref().and_then(RecipeInput::corner),
                    };
                    record.check(shot.play.is_some()).map(|()| record)
                })
                .transpose()
                .map_err(|e| format!("shot `{}`: {e}", shot.name))?;
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
                globals: recipe
                    .globals
                    .iter()
                    .filter(|(key, _)| !shot.globals.contains_key(*key))
                    .chain(&shot.globals)
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
                play: shot.play.clone().map(|name| ShotPlay {
                    name,
                    trace: shot.trace.or(recipe.trace).unwrap_or_default(),
                    steps: false,
                }),
                record,
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
pub(super) fn scalar(key: &str, value: &toml::Value) -> Result<String, String> {
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

#[cfg(test)]
pub(super) fn capture_template(
    scene: &SceneEntry,
    knobs: &[KnobState],
    size: egui::Vec2,
    scale: f32,
) -> String {
    capture_template_with_globals(scene, &[], knobs, size, scale)
}

pub(super) fn capture_template_with_globals(
    scene: &SceneEntry,
    globals: &[KnobState],
    knobs: &[KnobState],
    size: egui::Vec2,
    scale: f32,
) -> String {
    let (width, height) = (size.x, size.y);
    let (name, key) = (slug(scene.name), scene_key(scene));
    let magnified = if scale == DEFAULT_SCALE {
        String::new()
    } else {
        format!("scale = {scale}\n")
    };
    let mut out = formatdoc! {r#"
        # Generated by `--init-capture`: every stateful control at its current value.
        out = "renders"
        size = "{width}x{height}"
        {magnified}# Uncomment once there is a second shot: gathers limit-safe copies onto one captioned image.
        # sheet = "sheet.png"
    "#};
    write_controls(&mut out, "globals", globals);
    out.push_str("\n[[shot]]\n");
    out.push_str(
        &toml::to_string(&BTreeMap::from([("name", name), ("scene", key)]))
            .expect("scene identifiers are TOML strings"),
    );
    write_controls(&mut out, "shot.knobs", knobs);
    out
}

fn write_controls(out: &mut String, table: &str, knobs: &[KnobState]) {
    if knobs
        .iter()
        .any(|knob| !matches!(knob, KnobState::Button { .. } | KnobState::Group { .. }))
    {
        let _ = writeln!(out, "\n[{table}]");
    }
    for knob in knobs {
        if let KnobState::Group { label } = knob {
            out.push('\n');
            for line in label.lines() {
                let _ = writeln!(out, "# {}", line.escape_debug());
            }
        } else if let Some(value) = control_value(knob) {
            out.push_str(
                &toml::to_string(&BTreeMap::from([(knob.label(), value)]))
                    .expect("control values are TOML scalars"),
            );
        }
    }
}

#[derive(Debug, PartialEq, serde::Serialize)]
#[serde(untagged)]
pub(super) enum ControlValue {
    Text(String),
    Toggle(bool),
    Slider(f32),
}

pub(super) fn control_values(knobs: &[KnobState]) -> BTreeMap<String, ControlValue> {
    knobs
        .iter()
        .filter_map(|knob| control_value(knob).map(|value| (knob.label().to_owned(), value)))
        .collect()
}

pub(super) fn control_value(knob: &KnobState) -> Option<ControlValue> {
    Some(match knob {
        KnobState::Button { .. } | KnobState::Group { .. } => return None,
        KnobState::Toggle { value, .. } => ControlValue::Toggle(*value),
        KnobState::Slider { value, .. } => ControlValue::Slider(*value),
        KnobState::Text { value, .. } => ControlValue::Text(value.clone()),
        KnobState::Color { value, .. } => ControlValue::Text(value.to_hex()),
        KnobState::Select { value, options, .. }
        | KnobState::IconButtons { value, options, .. } => {
            ControlValue::Text(options.get(*value).cloned().unwrap_or_default())
        }
        KnobState::Pad2D { x, y, .. } => ControlValue::Text(format!("{x},{y}")),
    })
}

/// A scene name as a filename: the shot's `name`, which is what the PNG gets called.
pub(super) fn slug(name: &str) -> String {
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
