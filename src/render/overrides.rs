//! Knob and global overrides: parsing them, and applying them frame after frame until they land.

use super::*;

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

pub(crate) fn parse_assignment(assignment: &str) -> Result<KnobOverride, String> {
    let (key, raw) = assignment
        .split_once('=')
        .ok_or_else(|| "an override takes LABEL=VALUE".to_owned())?;
    let value = raw
        .parse::<toml::Value>()
        .unwrap_or_else(|_| toml::Value::String(raw.to_owned()));
    Ok(KnobOverride {
        key: key.to_owned(),
        value: scalar(key, &value)?,
    })
}

/// Overrides with one value per key: setting a key again replaces it and moves it last.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub(crate) struct Overrides(Vec<KnobOverride>);

impl Overrides {
    pub(crate) fn set(&mut self, over: KnobOverride) {
        self.0.retain(|before| before.key != over.key);
        self.0.push(over);
    }
}

impl std::ops::Deref for Overrides {
    type Target = [KnobOverride];

    fn deref(&self) -> &[KnobOverride] {
        &self.0
    }
}

impl Extend<KnobOverride> for Overrides {
    fn extend<I: IntoIterator<Item = KnobOverride>>(&mut self, overrides: I) {
        for over in overrides {
            self.set(over);
        }
    }
}

impl FromIterator<KnobOverride> for Overrides {
    fn from_iter<I: IntoIterator<Item = KnobOverride>>(overrides: I) -> Self {
        let mut merged = Self::default();
        merged.extend(overrides);
        merged
    }
}

pub(super) fn apply_globals<'shot>(
    canvas: &mut Canvas,
    overrides: &'shot [KnobOverride],
    unmatched: &mut Vec<&'shot KnobOverride>,
) -> Result<(), Diagnostic> {
    let Some(globals) = &mut canvas.globals else {
        return apply(&mut [], overrides)
            .map(drop)
            .map_err(Diagnostic::from);
    };
    const MAX_PASSES: usize = 256;

    let mut invalid = None;
    let mut stable_passes = 0;
    for _ in 0..MAX_PASSES {
        let pending = unmatched.len();
        let shape = control_shape(globals.knobs());
        invalid = apply_global_pass(globals.knobs_mut(), overrides, unmatched)?;
        globals.sync().map_err(Diagnostic::from)?;
        if unmatched.len() == pending && control_shape(globals.knobs()) == shape {
            stable_passes += 1;
        } else {
            stable_passes = 0;
        }
        // Declarations may precede controls they depend on, so require two stable passes.
        if stable_passes >= 2 {
            return invalid.map_or(Ok(()), Err);
        }
    }
    match invalid {
        Some(error) => Err(error),
        None if unmatched.is_empty() => Err(Diagnostic::new(
            "catalog global controls did not stabilize while applying overrides",
        )),
        None => Ok(()),
    }
}

fn apply_global_pass<'shot>(
    knobs: &mut [KnobState],
    overrides: &'shot [KnobOverride],
    unmatched: &mut Vec<&'shot KnobOverride>,
) -> Result<Option<Diagnostic>, Diagnostic> {
    let mut invalid = None;
    for over in overrides {
        if let Some(at) = find(knobs, &over.key)? {
            match set(&mut knobs[at], &over.value) {
                Ok(()) => unmatched.retain(|left| !std::ptr::eq(*left, over)),
                Err(error) if invalid.is_none() => invalid = Some(error.into()),
                Err(_) => {}
            }
        }
    }
    Ok(invalid)
}

/// Apply the overrides whose knobs exist; run each frame, so the recipe has the last word.
/// Unknown keys and values a knob can't take yet wait; bad values and ambiguous keys fail at once.
pub(super) fn apply<'shot>(
    knobs: &mut [KnobState],
    overrides: &'shot [KnobOverride],
) -> Result<Applied<'shot>, Box<KeyError>> {
    let mut applied = Applied::default();
    for over in overrides {
        let blame = |error: Diagnostic| KeyError::boxed(&over.key, error);
        if let Some(at) = find(knobs, &over.key).map_err(blame)? {
            match set(&mut knobs[at], &over.value) {
                Ok(()) => {
                    applied.pinned.push(Pin::of(knobs, at));
                    applied.landed.push((over, knobs[at].label().to_owned()));
                }
                Err(Rejected::NotYet(why)) => applied.not_yet.push((over, why)),
                Err(Rejected::Hidden(why)) => applied.hidden.push((over, why)),
                Err(invalid) => return Err(blame(invalid.into())),
            }
        }
    }
    Ok(applied)
}

/// A failure caused by one knob key, which a play traces back to its `set_knob`.
#[derive(Debug)]
pub(super) struct KeyError {
    pub(super) key: String,
    pub(super) error: Diagnostic,
}

impl KeyError {
    fn boxed(key: &str, error: Diagnostic) -> Box<Self> {
        Box::new(Self {
            key: key.to_owned(),
            error,
        })
    }
}

impl From<Box<KeyError>> for Diagnostic {
    fn from(failed: Box<KeyError>) -> Self {
        failed.error
    }
}

/// One set of overrides applied frame after frame:
/// which keys have landed so far, and why the others haven't.
pub(super) struct Ledger {
    overrides: Overrides,
    seen: Seen,
}

/// What the frames a [`Ledger`] applied have shown so far.
#[derive(Default)]
struct Seen {
    landed: std::collections::HashSet<String>,
    /// Why each key's value hasn't landed: the latest refusal by its knob,
    /// else what stood in for it (a group or button) while it never landed.
    /// Cleared when the key lands or is set again; a knob hidden meanwhile leaves its entry.
    not_yet: BTreeMap<String, String>,
    /// Why the latest apply found a value its knob couldn't take, if it did.
    not_yet_now: Option<String>,
}

impl Seen {
    /// A key that lands is done for good, so a knob hidden later can't undo it.
    fn note(&mut self, applied: &Applied<'_>) {
        self.not_yet_now = applied.not_yet.first().map(|(_, why)| why.clone());
        for (over, why) in &applied.not_yet {
            self.not_yet.insert(over.key.clone(), why.clone());
        }
        // Hidden is no match: it only names what stood in for a key that never landed.
        for (over, why) in &applied.hidden {
            if !self.landed.contains(&over.key) {
                self.not_yet
                    .entry(over.key.clone())
                    .or_insert_with(|| why.clone());
            }
        }
        for (over, _) in &applied.landed {
            self.landed.insert(over.key.clone());
            self.not_yet.remove(&over.key);
        }
    }
}

impl Ledger {
    pub(super) fn new(overrides: Overrides) -> Self {
        Self {
            overrides,
            seen: Seen::default(),
        }
    }

    /// Apply to the canvas's knobs, hold the ones set against the scene's writes,
    /// and bind each key to the label it landed on.
    /// A frame that fails is not noted.
    pub(super) fn apply(&mut self, canvas: &mut Canvas) -> Result<(), Box<KeyError>> {
        let applied = apply(&mut canvas.knobs, &self.overrides)?;
        canvas.bound.note(&applied)?;
        self.seen.note(&applied);
        canvas.pinned = applied.pinned;
        Ok(())
    }

    /// [`apply`](Self::apply) to bare knobs, with no canvas to bind or pin.
    #[cfg(test)]
    pub(super) fn apply_knobs(&mut self, knobs: &mut [KnobState]) -> Result<(), Box<KeyError>> {
        let applied = apply(knobs, &self.overrides)?;
        self.seen.note(&applied);
        Ok(())
    }

    /// A new value has yet to land, and any old reason is for the old one.
    pub(super) fn set(&mut self, over: KnobOverride) {
        self.seen.landed.remove(&over.key);
        self.seen.not_yet.remove(&over.key);
        self.overrides.set(over);
    }

    pub(super) fn overrides(&self) -> &Overrides {
        &self.overrides
    }

    pub(super) fn unlanded(&self) -> impl Iterator<Item = &KnobOverride> {
        self.overrides
            .iter()
            .filter(|over| !self.seen.landed.contains(&over.key))
    }

    pub(super) fn reason(&self, key: &str) -> Option<&str> {
        self.seen.not_yet.get(key).map(String::as_str)
    }

    pub(super) fn not_yet_now(&self) -> Option<&str> {
        self.seen.not_yet_now.as_deref()
    }

    pub(super) fn not_yet_reasons(&self) -> impl Iterator<Item = &str> {
        self.seen.not_yet.values().map(String::as_str)
    }
}

/// What one [`apply`] came to.
#[derive(Default, Debug)]
pub(super) struct Applied<'shot> {
    /// The knobs it set, which the next frame holds against the scene's writes.
    pinned: Vec<Pin>,
    /// The overrides their knob can't take as last declared, each with why.
    not_yet: Vec<(&'shot KnobOverride, String)>,
    /// The overrides that found only a group or button of their label, each with what it was.
    hidden: Vec<(&'shot KnobOverride, String)>,
    /// The overrides that landed, each with the label of the knob it landed on.
    landed: Vec<(&'shot KnobOverride, String)>,
}

/// The label each key has landed on, so a key can't move to another knob between frames:
/// a regex key whose own knob is hidden could otherwise land on one whose label contains it.
#[derive(Default)]
pub(super) struct Bound(std::collections::HashMap<String, String>);

impl Bound {
    /// # Errors
    /// For a key that landed on another label than before.
    fn note(&mut self, applied: &Applied<'_>) -> Result<(), Box<KeyError>> {
        for (over, label) in &applied.landed {
            let was = self
                .0
                .entry(over.key.clone())
                .or_insert_with(|| label.clone());
            if was != label {
                let error = Diagnostic::new(format!(
                    "knob key `{}` matched `{was}`, then `{label}`",
                    over.key
                ))
                .hint(
                    "while its own knob is hidden a key reads as a regex, so anchor it as `^…$` \
                     and escape its punctuation, even for an exact label",
                );
                return Err(KeyError::boxed(&over.key, error));
            }
        }
        Ok(())
    }

    pub(super) fn label(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }
}

/// Which knob a key names.
///
/// An exact label wins, so one copied out of `--list-knobs` always works
/// even when it carries regex punctuation — `width (chars)` reads as a group, not as itself.
/// Otherwise the key is a case-insensitive regex searched against every label.
/// Either way a knob holding a value wins over a group or button of the same label.
///
/// # Errors
/// If the key matches more than one knob, or isn't a valid regex.
fn find(knobs: &[KnobState], key: &str) -> Result<Option<usize>, Diagnostic> {
    let holds_value =
        |knob: &KnobState| !matches!(knob, KnobState::Group { .. } | KnobState::Button { .. });
    let exact = |knob: &KnobState| knob.label() == key;
    if let Some(at) = knobs
        .iter()
        .position(|knob| exact(knob) && holds_value(knob))
        .or_else(|| knobs.iter().position(exact))
    {
        return Ok(Some(at));
    }
    let regex = regex::RegexBuilder::new(key)
        .case_insensitive(true)
        .build()
        .map_err(|e| Diagnostic::new(format!("knob key `{key}` is not a regex: {e}")))?;
    let mut matched: Vec<usize> = knobs
        .iter()
        .enumerate()
        .filter(|(_, knob)| regex.is_match(knob.label()))
        .map(|(at, _)| at)
        .collect();
    if matched.iter().any(|at| holds_value(&knobs[*at])) {
        matched.retain(|at| holds_value(&knobs[*at]));
    }
    match matched.as_slice() {
        [one] => Ok(Some(*one)),
        [] => Ok(None),
        several => Err(Diagnostic::new(format!(
            "knob key `{key}` matches {} knobs",
            several.len()
        ))
        .candidates(several.iter().map(|at| format!("{:?}", knobs[*at].label())))
        .hint("spell one label out in full — an exact label always wins over the regex")),
    }
}

/// Set one knob from the text a recipe or command line gave for it.
///
/// # Errors
/// A message naming the knob, its kind, and what that kind accepts.
fn set(knob: &mut KnobState, raw: &str) -> Result<(), Rejected> {
    let label = knob.label().to_owned();
    let bad = |kind: &str, accepts: &str| -> Result<(), Rejected> {
        Err(Rejected::Invalid(format!(
            "knob `{label}` is a {kind}; `{raw}` is not {accepts}"
        )))
    };
    match knob {
        KnobState::Button { .. } => {
            return Err(Rejected::Hidden(format!(
                "`{label}` is a button action and carries no value"
            )));
        }
        KnobState::Text { value, .. } => *value = raw.to_owned(),
        KnobState::Toggle { value, .. } => match raw {
            "true" => *value = true,
            "false" => *value = false,
            _ => return bad("toggle", "`true` or `false`"),
        },
        KnobState::Slider {
            value,
            min,
            max,
            step,
            ..
        } => match (raw.parse::<f32>(), Span::new(*min, *max)) {
            (Ok(parsed), _) if parsed.is_nan() => return bad("slider", "a number"),
            // Snapped as the panel's slider would.
            (Ok(parsed), span) if span.contains(parsed) => {
                match crate::handle::slider_value(parsed, span, *step) {
                    Some(snapped) => *value = snapped,
                    None if !step.is_finite() => {
                        return Err(format!(
                            "knob `{label}` steps by {step}, which no value snaps onto"
                        )
                        .into());
                    }
                    None => return bad("slider", "a finite number"),
                }
            }
            (Ok(parsed), span) => {
                return Err(Rejected::NotYet(format!(
                    "knob `{label}` takes {span}; {parsed} is outside it"
                )));
            }
            (Err(_), _) => return bad("slider", "a number"),
        },
        KnobState::Color { value, .. } => match egui::Color32::from_hex(raw) {
            Ok(parsed) => *value = parsed,
            Err(_) => return bad("colour", "a hex colour (#RGB, #RGBA, #RRGGBB or #RRGGBBAA)"),
        },
        KnobState::Select { value, options, .. }
        | KnobState::IconButtons { value, options, .. } => {
            *value = choice(options, raw, &label).map_err(Rejected::NotYet)?;
        }
        KnobState::Pad2D {
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
            // Both parsed before either is range-checked, so a bad axis fails at once.
            let (at_x, at_y) = (axis(raw_x, &label)?, axis(raw_y, &label)?);
            (*x, *y) = (
                on_axis(at_x, Span::new(*min_x, *max_x), &label, 'x')?,
                on_axis(at_y, Span::new(*min_y, *max_y), &label, 'y')?,
            );
        }
        KnobState::Group { .. } => {
            return Err(Rejected::Hidden(format!(
                "`{label}` is a group heading and carries no value"
            )));
        }
    }
    Ok(())
}

/// Why [`set`] turned a value down.
enum Rejected {
    /// Not takeable as the knob was last declared, which a later frame may change:
    /// past its range, or an option it doesn't offer yet.
    NotYet(String),
    /// A group or button holds the label, so a knob of that label, if any, is hidden.
    /// As good as no match; the reason only says what was found instead.
    Hidden(String),
    Invalid(String),
}

impl From<String> for Rejected {
    fn from(message: String) -> Self {
        Self::Invalid(message)
    }
}

impl From<Rejected> for Diagnostic {
    fn from(rejected: Rejected) -> Self {
        match rejected {
            Rejected::NotYet(message) | Rejected::Hidden(message) | Rejected::Invalid(message) => {
                Self::new(message)
            }
        }
    }
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
            "knob `{label}` has no option `{raw}`. Options: {}. Use --list-knobs to list the controls",
            options.join(" | ")
        )),
    }
}

/// One axis of a 2-axis pad, parsed; [`on_axis`] checks it against that axis's bounds.
fn axis(raw: &str, label: &str) -> Result<f32, Rejected> {
    match raw.trim().parse::<f32>() {
        Ok(parsed) if !parsed.is_nan() => Ok(parsed),
        _ => Err(Rejected::Invalid(format!(
            "knob `{label}` is a 2-axis pad; `{raw}` is not a number"
        ))),
    }
}

fn on_axis(value: f32, span: Span, label: &str, which: char) -> Result<f32, Rejected> {
    if span.contains(value) {
        Ok(value)
    } else {
        Err(Rejected::NotYet(format!(
            "knob `{label}` takes {span} on {which}; {value} is outside it"
        )))
    }
}

/// Which overrides went unmatched: a scene's knobs, or the catalog's globals.
#[derive(Clone, Copy)]
pub(super) enum KeyKind {
    Knob,
    Global,
}

/// Keys that named a control no frame ever declared.
/// Listing what is declared turns a typo or a stale label into a one-step fix.
pub(super) fn unknown_keys(
    kind: KeyKind,
    pending: &[&KnobOverride],
    declared: &[KnobState],
) -> Diagnostic {
    let (declares_no, hint) = match kind {
        KeyKind::Knob => (
            "this scene declares no knob",
            "`--list-knobs` prints these with their kinds and current values",
        ),
        KeyKind::Global => (
            "this catalog declares no catalog global control",
            "`--list-knobs` prints the global controls with their kinds and current values",
        ),
    };
    let named = pending
        .iter()
        .map(|over| format!("`{}`", over.key))
        .collect::<Vec<_>>()
        .join(", ");
    Diagnostic::new(format!("{declares_no} matching {named}"))
        .candidates(declared.iter().map(|knob| format!("{:?}", knob.label())))
        .hint(hint)
}
