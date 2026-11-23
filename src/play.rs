//! Plays: scripts that drive a scene through its accessibility tree,
//! finding widgets by label or role rather than by position.
//!
//! A play is compiled into the scenes dylib, which has its own copy of every crate.
//! AccessKit hashes its node map with foldhash, seeded per copy,
//! so only the host can search a tree the host built.
//! Queries cross the [`PlayDriver`] vtable as a [`By`] and return owned [`Found`]s.
//!
//! Each gesture is a fixed sequence of frames,
//! so a play draws the same frames on any machine.

use std::{
    cell::{Cell, RefCell},
    fmt::{self, Debug, Display},
    ops::Range,
    panic::{self, AssertUnwindSafe, Location},
    sync::Once,
};

use camino::Utf8PathBuf;
use egui::{
    Event, Key, Modifiers, MouseWheelUnit, PointerButton, Pos2, TouchPhase, Vec2,
    accesskit::{self, Role},
};
use kittest::{AccessKitNode, NodeT, Queryable};

/// The key of the scene a play drives.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SceneKey {
    pub module_path: &'static str,
    pub name: &'static str,
}

impl Display for SceneKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}::{}", self.module_path, self.name)
    }
}

/// A play, as [`macro@crate::play`] registers it.
#[derive(Clone, Copy)]
pub struct PlayEntry {
    pub scene: SceneKey,
    /// The function's name, which a recipe or `--play` asks for.
    pub name: &'static str,
    /// Where the function is, to tell apart two plays of one name.
    pub module_path: &'static str,
    pub run: fn(&mut Play<'_>) -> PlayResult,
}

inventory::collect!(PlayEntry);

/// What a play, a step or a gesture came to.
pub type PlayResult = Result<(), PlayError>;

/// Why a play stopped, where in its source, and in which step.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PlayError {
    reason: String,
    /// The innermost step it happened in.
    step: Option<String>,
    /// The play's own line that failed.
    at: Option<SourceAt>,
    /// The labels in the tree when a widget could not be found.
    on_screen: Vec<String>,
}

/// A line in a play's or a scene's source.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct SourceAt {
    pub(crate) file: String,
    pub(crate) line: u32,
    pub(crate) column: u32,
}

impl From<&Location<'_>> for SourceAt {
    fn from(at: &Location<'_>) -> Self {
        Self {
            file: at.file().to_owned(),
            line: at.line(),
            column: at.column(),
        }
    }
}

impl PlayError {
    pub fn new(reason: impl Display) -> Self {
        Self {
            reason: reason.to_string(),
            step: None,
            at: None,
            on_screen: Vec::new(),
        }
    }

    pub(crate) fn reason(&self) -> &str {
        &self.reason
    }

    pub(crate) fn step(&self) -> Option<&str> {
        self.step.as_deref()
    }

    pub(crate) fn at(&self) -> Option<&SourceAt> {
        self.at.as_ref()
    }

    pub(crate) fn on_screen(&self) -> &[String] {
        &self.on_screen
    }

    /// Blame `step`, unless a step inside it was already blamed.
    fn in_step(mut self, step: &str) -> Self {
        self.step.get_or_insert_with(|| step.to_owned());
        self
    }

    /// Point at `at`, unless a line closer to the failure already was.
    fn located(mut self, at: &Location<'_>) -> Self {
        self.at.get_or_insert_with(|| at.into());
        self
    }

    fn seeing(mut self, on_screen: Vec<String>) -> Self {
        self.on_screen = on_screen;
        self
    }
}

/// What [`Play::get`] and [`Play::query`] panic with,
/// so the report can lay the labels out.
struct Missed(PlayError);

impl Display for PlayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(step) = &self.step {
            write!(f, "step `{step}`: ")?;
        }
        f.write_str(&self.reason)?;
        if !self.on_screen.is_empty() {
            write!(f, "; on screen: {}", self.on_screen.join(", "))?;
        }
        Ok(())
    }
}

impl std::error::Error for PlayError {}

impl From<String> for PlayError {
    fn from(reason: String) -> Self {
        Self::new(reason)
    }
}

impl From<&str> for PlayError {
    fn from(reason: &str) -> Self {
        Self::new(reason)
    }
}

/// Which widgets a query or gesture means; every condition set must hold.
/// A bare `&str` is a label: `p.click("Save")`.
///
/// kittest's filter without its predicate,
/// since a closure would run dylib code against the host's tree.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct By {
    label: Option<String>,
    label_contains: bool,
    role: Option<Role>,
    value: Option<String>,
}

impl By {
    pub fn new() -> Self {
        Self::default()
    }

    /// The accessible label, exactly; a text label's text counts too.
    #[must_use]
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self.label_contains = false;
        self
    }

    /// Any accessible label containing `part`.
    #[must_use]
    pub fn label_contains(mut self, part: impl Into<String>) -> Self {
        self.label = Some(part.into());
        self.label_contains = true;
        self
    }

    #[must_use]
    pub fn role(mut self, role: Role) -> Self {
        self.role = Some(role);
        self
    }

    /// The current value, exactly: a text field's contents, a slider's number.
    #[must_use]
    pub fn value(mut self, value: impl Into<String>) -> Self {
        self.value = Some(value.into());
        self
    }

    fn kittest(&self) -> kittest::By<'_> {
        let mut by = kittest::By::new();
        match (&self.label, self.label_contains) {
            (Some(label), false) => by = by.label(label),
            (Some(part), true) => by = by.label_contains(part),
            (None, _) => {}
        }
        if let Some(role) = self.role {
            by = by.role(role);
        }
        if let Some(value) = &self.value {
            by = by.value(value);
        }
        by
    }
}

impl From<&str> for By {
    fn from(label: &str) -> Self {
        Self::new().label(label)
    }
}

impl From<String> for By {
    fn from(label: String) -> Self {
        Self::new().label(label)
    }
}

/// How a step names its target: `button "Save"`, `~"Sav"`, `= "Ada"`.
impl Display for By {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts = Vec::new();
        if let Some(role) = self.role {
            parts.push(format!("{role:?}").to_lowercase());
        }
        match (&self.label, self.label_contains) {
            (Some(label), false) => parts.push(format!("{label:?}")),
            (Some(part), true) => parts.push(format!("~{part:?}")),
            (None, _) => {}
        }
        if let Some(value) = &self.value {
            parts.push(format!("= {value:?}"));
        }
        if parts.is_empty() {
            parts.push("anything".to_owned());
        }
        f.write_str(&parts.join(" "))
    }
}

/// A widget a query found, copied out of that frame's tree.
#[derive(Clone, PartialEq, Debug)]
pub struct Found {
    pub role: Role,
    pub label: Option<String>,
    pub value: Option<String>,
    pub focused: bool,
    pub disabled: bool,
    /// In points.
    pub rect: egui::Rect,
    target: Target,
}

/// Where a focus request goes.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Target {
    node: accesskit::NodeId,
    tree: accesskit::TreeId,
}

impl Found {
    fn request(&self, action: accesskit::Action) -> Event {
        Event::AccessKitActionRequest(accesskit::ActionRequest {
            action,
            target_node: self.target.node,
            target_tree: self.target.tree,
            data: None,
        })
    }
}

/// What a play runs a scene through.
///
/// Only gallery implements it, so its methods run as host code.
/// Only host code can search the tree.
pub(crate) trait PlayDriver {
    /// The widgets `by` matches in the last frame's tree.
    fn query(&self, by: &By) -> Vec<Found>;

    /// Every non-empty label in the tree, for failure messages.
    fn labels(&self) -> Vec<String>;

    /// Draw one frame with `events` as its input.
    ///
    /// # Errors
    /// When the frame cannot be drawn as asked, e.g. a bad knob value or an ambiguous key.
    fn frame(&mut self, events: Vec<Event>) -> Result<(), String>;

    /// The actions the scene reported since the last call.
    fn take_actions(&mut self) -> Vec<String>;

    /// Hold a knob at `value`, written as `--knob` takes it, from the next frame on.
    fn set_knob(&mut self, key: &str, value: String);

    /// The knob keys that have not landed on a knob since they were last set.
    fn unlanded(&self) -> Vec<Unlanded>;

    /// The knob key whose value failed the last frame, when one did.
    fn bad_knob(&self) -> Option<String>;

    /// Screenshot the canvas for step `index` and return where it was written,
    /// or `None` when this run keeps no trace.
    /// Written per step, so a long play holds no images in memory.
    ///
    /// # Errors
    /// When the screenshot cannot be taken or written.
    fn trace(&mut self, index: usize, label: &str) -> Result<Option<Utf8PathBuf>, String>;

    /// A step has ended: a recording pauses on it, so the step can be followed.
    ///
    /// # Errors
    /// When the recording has failed.
    fn end_step(&mut self) -> Result<(), String>;
}

/// [`PlayDriver::query`] over a tree; host code only.
pub(crate) fn query_tree(root: AccessKitNode<'_>, pixels_per_point: f32, by: &By) -> Vec<Found> {
    let root = TreeNode { node: root };
    root.query_all(by.kittest())
        .map(|found| {
            let node = found.node;
            let (node_id, tree) = node.locate();
            Found {
                role: node.role(),
                label: label(&node),
                value: node.value(),
                focused: node.is_focused(),
                disabled: node.is_disabled(),
                rect: points(node.bounding_box(), pixels_per_point),
                target: Target {
                    node: node_id,
                    tree,
                },
            }
        })
        .collect()
}

/// [`PlayDriver::labels`] over a tree; host code only.
pub(crate) fn tree_labels(root: AccessKitNode<'_>) -> Vec<String> {
    let root = TreeNode { node: root };
    let mut labels: Vec<String> = root
        .query_all(kittest::By::new().predicate(|node| label(node).is_some_and(|l| !l.is_empty())))
        .filter_map(|found| label(&found.node))
        .collect();
    let mut seen = std::collections::HashSet::new();
    labels.retain(|label| seen.insert(label.clone()));
    labels
}

/// A node's label as kittest's `By::label` reads it.
/// AccessKit keeps a text label's text in its value.
fn label(node: &AccessKitNode<'_>) -> Option<String> {
    if node.role() == Role::Label {
        node.value()
    } else {
        node.label()
    }
}

/// A node for kittest's queries to walk.
#[derive(Clone)]
struct TreeNode<'tree> {
    node: AccessKitNode<'tree>,
}

impl Debug for TreeNode<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        kittest::debug_fmt_node(self, f)
    }
}

impl<'tree> NodeT<'tree> for TreeNode<'tree> {
    fn accesskit_node(&self) -> AccessKitNode<'tree> {
        self.node
    }

    fn new_related(&self, node: AccessKitNode<'tree>) -> Self {
        Self { node }
    }
}

#[expect(clippy::cast_possible_truncation, reason = "screen coordinates")]
fn points(bounds: Option<accesskit::Rect>, pixels_per_point: f32) -> egui::Rect {
    let Some(bounds) = bounds else {
        return egui::Rect::NOTHING;
    };
    let ppp = f64::from(pixels_per_point);
    egui::Rect::from_min_max(
        Pos2::new((bounds.x0 / ppp) as f32, (bounds.y0 / ppp) as f32),
        Pos2::new((bounds.x1 / ppp) as f32, (bounds.y1 / ppp) as f32),
    )
}

/// The handle a play gets.
///
/// Gestures take a [`By`] and wait, a frame at a time,
/// until exactly one widget matches.
///
/// [`query`](Self::query), [`get`](Self::get) and their kin read the tree.
pub struct Play<'d> {
    driver: &'d mut dyn PlayDriver,
    frame: u32,
    actions: Vec<String>,
    steps: Vec<StepRecord>,
    /// Labels of the steps now running, outermost first.
    open: Vec<String>,
    wait_frames: u32,
    knobs_set: KnobsSet,
}

/// A knob key whose value never landed.
pub(crate) struct Unlanded {
    pub(crate) key: String,
    /// When the driver knows: the knob couldn't take the value (past its range,
    /// an option it doesn't offer), only a group or button held the label,
    /// or the knob the key matched before is not declared now.
    pub(crate) why: Option<String>,
}

impl Unlanded {
    fn into_error(self) -> PlayError {
        PlayError::new(
            self.why
                .unwrap_or_else(|| format!("no knob matches `{}`", self.key)),
        )
    }
}

/// Each knob key the play set, and the line that last set it.
#[derive(Default)]
struct KnobsSet(Vec<(String, &'static Location<'static>)>);

impl KnobsSet {
    fn set(&mut self, key: &str, at: &'static Location<'static>) {
        self.0.retain(|(set, _)| set != key);
        self.0.push((key.to_owned(), at));
    }

    fn at(&self, key: &str) -> Option<&'static Location<'static>> {
        self.0.iter().find(|(set, _)| set == key).map(|(_, at)| *at)
    }
}

/// A finished step, for the report and the trace.
pub(crate) struct StepRecord {
    pub(crate) label: String,
    pub(crate) frames: Range<u32>,
    pub(crate) trace: Option<Utf8PathBuf>,
}

/// What a play run came to.
pub(crate) struct PlayRun {
    pub(crate) steps: Vec<StepRecord>,
    pub(crate) frames: u32,
    pub(crate) failure: Option<PlayError>,
}

/// How long gestures and [`Play::wait_for`] wait, in frames.
pub const DEFAULT_WAIT_FRAMES: u32 = 60;
/// Pointer moves between a drag's press and release.
pub const DRAG_FRAMES: u32 = 10;
/// Wheel events a scroll is split into.
pub const SCROLL_FRAMES: u32 = 5;
/// Most frames a play's last knob keys get to land in once it is over.
const TAIL_FRAMES: u32 = 8;

/// Run `play`, its panic caught by the `std` this copy of gallery links,
/// which is the only one that can catch it.
#[doc(hidden)]
pub fn __run_play(play: &mut Play<'_>, run: fn(&mut Play<'_>) -> PlayResult) -> PlayResult {
    caught(|| run(play)).and_then(|outcome| outcome)
}

pub(crate) fn run(entry: &PlayEntry, driver: &mut dyn PlayDriver) -> PlayRun {
    let mut play = Play {
        driver,
        frame: 0,
        actions: Vec::new(),
        steps: Vec::new(),
        open: Vec::new(),
        wait_frames: DEFAULT_WAIT_FRAMES,
        knobs_set: KnobsSet::default(),
    };
    // A `#[play]` catches its own panics, in the copy of gallery that raised them.
    // This catches one from an entry built by hand, and leaves the hook loud:
    // a panic from gallery's own code under a scenes dylib aborts, and should say why first.
    let mut failure = match panic::catch_unwind(AssertUnwindSafe(|| (entry.run)(&mut play))) {
        Ok(Ok(())) => None,
        Ok(Err(error)) => Some(error),
        Err(panic) => Some(panicked(panic)),
    };
    // A knob may come and go as the scene changes, so a key is held to account
    // only once the play is over, and only if its last value never landed.
    if failure.is_none() {
        // Only the play's own keys: the shot's were checked before it began.
        let unseen = |play: &Play<'_>| {
            let mut unseen: Vec<_> = play
                .driver
                .unlanded()
                .into_iter()
                .filter_map(|left| Some((play.knobs_set.at(&left.key)?, left)))
                .collect();
            // An unknown key first, so a typo is not hidden behind a known reason.
            unseen.sort_by_key(|(_, left)| left.why.is_some());
            unseen
        };
        // Keys set after the play's last frame get frames to land in while they keep landing:
        // one may declare another's knob, or widen its range.
        // A frame judges what earlier frames declared, so two must land nothing before giving up.
        let mut left = unseen(&play);
        let mut idle = 0;
        for _ in 0..TAIL_FRAMES {
            if left.is_empty() || idle == 2 {
                break;
            }
            if let Err(error) = play.frame(Vec::new()) {
                failure = Some(error);
                break;
            }
            let now = unseen(&play);
            idle = if now.len() < left.len() { 0 } else { idle + 1 };
            left = now;
        }
        if failure.is_none() {
            failure = left
                .into_iter()
                .next()
                .map(|(at, left)| left.into_error().located(at));
        }
    }
    PlayRun {
        steps: play.steps,
        frames: play.frame,
        failure,
    }
}

impl Play<'_> {
    /// A named part of the script, logged and traced as one.
    /// A panic inside it is reported as the step's failure, naming the step.
    ///
    /// # Errors
    /// Whatever stopped the step, prefixed with its label.
    #[track_caller]
    pub fn step(
        &mut self,
        label: impl Into<String>,
        body: impl FnOnce(&mut Self) -> PlayResult,
    ) -> PlayResult {
        self.step_at(label.into(), Location::caller(), body)
    }

    /// [`step`](Self::step), pointing at `at` when nothing inside it points closer.
    fn step_at(
        &mut self,
        label: String,
        at: &Location<'_>,
        body: impl FnOnce(&mut Self) -> PlayResult,
    ) -> PlayResult {
        self.open.push(label);
        let start = self.frame;
        let mut outcome = caught(|| body(self)).and_then(|outcome| outcome);
        let label = self.open.join(" › ");
        self.open.pop();
        let trace = match self.driver.trace(self.steps.len(), &label) {
            Ok(trace) => trace,
            Err(failed) => {
                outcome = outcome.and(Err(PlayError::new(failed)));
                None
            }
        };
        if let Err(failed) = self.driver.end_step() {
            outcome = outcome.and(Err(PlayError::new(failed)));
        }
        self.steps.push(StepRecord {
            label: label.clone(),
            frames: start..self.frame,
            trace,
        });
        outcome.map_err(|error| error.located(at).in_step(&label))
    }

    /// Every widget `by` matches.
    pub fn query_all(&self, by: impl Into<By>) -> Vec<Found> {
        self.driver.query(&by.into())
    }

    /// The widget `by` matches, if any.
    ///
    /// # Panics
    /// When it matches several; inside a step, that fails the step.
    #[track_caller]
    pub fn query(&self, by: impl Into<By>) -> Option<Found> {
        let by = by.into();
        let mut found = self.driver.query(&by);
        if found.len() > 1 {
            panic::panic_any(Missed(PlayError::new(format!(
                "{} widgets match {by}; narrow it with a role or an exact label",
                found.len()
            ))));
        }
        found.pop()
    }

    /// The one widget `by` matches.
    ///
    /// # Panics
    /// When it matches none or several; inside a step, that fails the step.
    #[track_caller]
    pub fn get(&self, by: impl Into<By>) -> Found {
        let by = by.into();
        match self.query(by.clone()) {
            Some(found) => found,
            None => panic::panic_any(Missed(
                PlayError::new(format!("nothing matches {by}")).seeing(self.driver.labels()),
            )),
        }
    }

    /// [`query`](Self::query) by exact label.
    #[track_caller]
    pub fn query_by_label(&self, label: &str) -> Option<Found> {
        self.query(label)
    }

    /// [`get`](Self::get) by exact label.
    #[track_caller]
    pub fn get_by_label(&self, label: &str) -> Found {
        self.get(label)
    }

    /// Draw `frames` frames with no input.
    ///
    /// # Errors
    /// When a frame cannot be drawn.
    #[track_caller]
    pub fn idle(&mut self, frames: u32) -> PlayResult {
        let at = Location::caller();
        for _ in 0..frames {
            self.frame(Vec::new()).map_err(|error| error.located(at))?;
        }
        Ok(())
    }

    /// Draw frames until `ready` holds, within the wait budget.
    ///
    /// # Errors
    /// When it never holds; the error lists what was on screen.
    #[track_caller]
    pub fn wait_for(&mut self, mut ready: impl FnMut(&Self) -> bool) -> PlayResult {
        let at = Location::caller();
        for _ in 0..self.wait_frames {
            if ready(self) {
                return Ok(());
            }
            self.frame(Vec::new()).map_err(|error| error.located(at))?;
        }
        if ready(self) {
            return Ok(());
        }
        Err(
            PlayError::new(format!("still waiting after {} frames", self.wait_frames))
                .seeing(self.driver.labels())
                .located(at),
        )
    }

    /// How many frames gestures and [`wait_for`](Self::wait_for) wait.
    pub fn set_wait_frames(&mut self, frames: u32) {
        self.wait_frames = frames;
    }

    /// Move the pointer onto the widget.
    ///
    /// # Errors
    /// When no single widget matches `by` in time.
    #[track_caller]
    pub fn hover(&mut self, by: impl Into<By>) -> PlayResult {
        self.gesture("hover", by.into(), Location::caller(), |p, at| {
            p.frame(vec![Event::PointerMoved(aim(&at)?)])?;
            p.frame(Vec::new())
        })
    }

    /// Click the widget's centre with the primary button.
    ///
    /// # Errors
    /// When no single widget matches `by` in time.
    #[track_caller]
    pub fn click(&mut self, by: impl Into<By>) -> PlayResult {
        self.gesture("click", by.into(), Location::caller(), |p, at| {
            let center = aim(&at)?;
            p.frame(vec![Event::PointerMoved(center)])?;
            p.frame(vec![button(center, true)])?;
            p.frame(vec![button(center, false)])?;
            p.frame(Vec::new())
        })
    }

    /// Give the widget keyboard focus, as assistive technology would.
    ///
    /// # Errors
    /// When no single widget matches `by` in time.
    #[track_caller]
    pub fn focus(&mut self, by: impl Into<By>) -> PlayResult {
        self.gesture("focus", by.into(), Location::caller(), |p, at| {
            p.frame(vec![at.request(accesskit::Action::Focus)])?;
            p.frame(Vec::new())
        })
    }

    /// Focus the widget and replace what it holds with `text`.
    ///
    /// # Errors
    /// When no single widget matches `by` in time.
    #[track_caller]
    pub fn fill(&mut self, by: impl Into<By>, text: &str) -> PlayResult {
        self.gesture("fill", by.into(), Location::caller(), |p, at| {
            p.frame(vec![at.request(accesskit::Action::Focus)])?;
            // Select everything first, so the text replaces it rather than joining it.
            p.frame(vec![
                Event::ModifiersChanged(Modifiers::COMMAND),
                keyboard(Key::A, true, Modifiers::COMMAND),
            ])?;
            p.frame(vec![
                keyboard(Key::A, false, Modifiers::COMMAND),
                Event::ModifiersChanged(Modifiers::NONE),
            ])?;
            if text.is_empty() {
                // egui drops empty text input, so the selection is deleted instead.
                p.frame(vec![keyboard(Key::Backspace, true, Modifiers::NONE)])?;
                p.frame(vec![keyboard(Key::Backspace, false, Modifiers::NONE)])?;
            } else {
                p.frame(vec![Event::Text(text.to_owned())])?;
            }
            p.frame(Vec::new())
        })
    }

    /// Type `text` into whatever has focus.
    ///
    /// # Errors
    /// When a frame cannot be drawn.
    #[track_caller]
    pub fn type_text(&mut self, text: &str) -> PlayResult {
        self.unnamed(format!("type {text:?}"), Location::caller(), |p| {
            p.frame(vec![Event::Text(text.to_owned())])?;
            p.frame(Vec::new())
        })
    }

    /// Press and release `key`.
    ///
    /// # Errors
    /// When a frame cannot be drawn.
    #[track_caller]
    pub fn press(&mut self, key: Key) -> PlayResult {
        self.press_with(Modifiers::NONE, key)
    }

    /// Press and release `key` with `modifiers` held.
    ///
    /// # Errors
    /// When a frame cannot be drawn.
    #[track_caller]
    pub fn press_with(&mut self, modifiers: Modifiers, key: Key) -> PlayResult {
        let shortcut = egui::KeyboardShortcut::new(modifiers, key)
            // One spelling on every machine, so a play's step labels and trace names do not differ.
            .format(&egui::ModifierNames::NAMES, false);
        self.unnamed(format!("press {shortcut}"), Location::caller(), |p| {
            p.frame(vec![
                Event::ModifiersChanged(modifiers),
                keyboard(key, true, modifiers),
            ])?;
            p.frame(vec![
                keyboard(key, false, modifiers),
                Event::ModifiersChanged(Modifiers::NONE),
            ])?;
            p.frame(Vec::new())
        })
    }

    /// Press on the widget's centre, move by `delta` points, and let go.
    ///
    /// # Errors
    /// When no single widget matches `by` in time.
    #[track_caller]
    pub fn drag(&mut self, by: impl Into<By>, delta: Vec2) -> PlayResult {
        self.gesture("drag", by.into(), Location::caller(), |p, at| {
            let center = aim(&at)?;
            p.frame(vec![Event::PointerMoved(center)])?;
            p.frame(vec![button(center, true)])?;
            for step in 1..=DRAG_FRAMES {
                #[expect(clippy::cast_precision_loss, reason = "a handful of frames")]
                let along = step as f32 / DRAG_FRAMES as f32;
                p.frame(vec![Event::PointerMoved(center + delta * along)])?;
            }
            p.frame(vec![button(center + delta, false)])?;
            p.frame(Vec::new())
        })
    }

    /// Scroll the content under the widget by `delta` points.
    ///
    /// # Errors
    /// When no single widget matches `by` in time.
    #[track_caller]
    pub fn scroll(&mut self, by: impl Into<By>, delta: Vec2) -> PlayResult {
        self.gesture("scroll", by.into(), Location::caller(), |p, at| {
            p.frame(vec![Event::PointerMoved(aim(&at)?)])?;
            #[expect(clippy::cast_precision_loss, reason = "a handful of frames")]
            let each = delta / SCROLL_FRAMES as f32;
            for _ in 0..SCROLL_FRAMES {
                p.frame(vec![Event::MouseWheel {
                    unit: MouseWheelUnit::Point,
                    delta: each,
                    phase: TouchPhase::Move,
                    modifiers: Modifiers::NONE,
                }])?;
            }
            p.frame(Vec::new())
        })
    }

    /// Hold a knob at `value`, written as `--knob` takes it, from the next frame on.
    /// A value the knob can't take yet, such as one past its range, waits for a frame that can.
    /// The play fails once over if the key's last value never landed: no knob matched it,
    /// or the knob never could take it.
    #[track_caller]
    pub fn set_knob(&mut self, key: &str, value: impl Display) {
        self.knobs_set.set(key, Location::caller());
        self.driver.set_knob(key, value.to_string());
    }

    /// What the scene has reported through [`action`](crate::action) during the play.
    pub fn actions(&self) -> &[String] {
        &self.actions
    }

    /// Check the scene reported `expected` through [`action`](crate::action).
    ///
    /// # Errors
    /// When it didn't; the error lists what it did report.
    #[track_caller]
    pub fn expect_action(&self, expected: &str) -> PlayResult {
        if self.actions.iter().any(|line| line == expected) {
            return Ok(());
        }
        Err(PlayError::new(format!(
            "no action `{expected}`; reported: {:?}",
            self.actions
        ))
        .located(Location::caller()))
    }

    fn frame(&mut self, events: Vec<Event>) -> PlayResult {
        if let Err(error) = self.driver.frame(events) {
            let error = PlayError::new(error);
            // A bad knob value is the `set_knob` call's fault,
            // not the gesture's that drew the frame.
            let set_at = self
                .driver
                .bad_knob()
                .and_then(|key| self.knobs_set.at(&key));
            return Err(match set_at {
                Some(at) => error.located(at),
                None => error,
            });
        }
        self.frame += 1;
        self.actions.extend(self.driver.take_actions());
        Ok(())
    }

    /// Wait for `by` to match one widget, then run `frames` on it,
    /// logged as its own step unless one is already open.
    fn gesture(
        &mut self,
        verb: &str,
        by: By,
        at: &Location<'_>,
        frames: impl FnOnce(&mut Self, Found) -> PlayResult,
    ) -> PlayResult {
        self.unnamed(format!("{verb} {by}"), at, |p| {
            let target = p.locate(&by)?;
            frames(p, target)
        })
    }

    /// `body` as a step of its own unless one is already open,
    /// pointing at `at` on failure.
    fn unnamed(
        &mut self,
        label: String,
        at: &Location<'_>,
        body: impl FnOnce(&mut Self) -> PlayResult,
    ) -> PlayResult {
        if self.open.is_empty() {
            self.step_at(label, at, body)
        } else {
            body(self).map_err(|error| error.located(at))
        }
    }

    /// The one widget `by` matches,
    /// looked for on this frame and each of the next `wait_frames`.
    fn locate(&mut self, by: &By) -> Result<Found, PlayError> {
        let mut waited = 0;
        loop {
            let mut found = self.driver.query(by);
            // None and several are both waited out: a list redrawing can show two for a frame.
            match found.len() {
                1 => return Ok(found.remove(0)),
                _ if waited < self.wait_frames => {
                    self.frame(Vec::new())?;
                    waited += 1;
                }
                0 => {
                    return Err(PlayError::new(format!(
                        "nothing matched {by} within {waited} frames"
                    ))
                    .seeing(self.driver.labels()));
                }
                many => {
                    return Err(PlayError::new(format!(
                        "{many} widgets still match {by} after {waited} frames; \
                         narrow it with a role or an exact label"
                    )));
                }
            }
        }
    }
}

/// Where a pointer gesture aims on `at`:
/// its centre, if it has a place on screen at all.
fn aim(at: &Found) -> Result<Pos2, PlayError> {
    let center = at.rect.center();
    if center.is_finite() {
        Ok(center)
    } else {
        Err(PlayError::new(format!(
            "{} matched, but has no place on screen to point at",
            at.label
                .as_deref()
                .map_or_else(|| format!("{:?}", at.role), |label| format!("`{label}`"))
        )))
    }
}

fn button(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

fn keyboard(key: Key, pressed: bool, modifiers: Modifiers) -> Event {
    Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers,
    }
}

thread_local! {
    /// Set while a play runs, so its panics reach the report rather than the terminal.
    static QUIET: Cell<bool> = const { Cell::new(false) };
    /// Where the last quiet panic was raised.
    static RAISED_AT: RefCell<Option<SourceAt>> = const { RefCell::new(None) };
}

/// Run `body`, turning a panic into a [`PlayError`]
/// that points at the line that raised it.
///
/// Generic, so it is compiled into whichever copy of gallery calls it.
/// A scenes dylib can carry its own `std`, with its own panic hook,
/// and the hook has to be armed in the copy whose code panics.
pub(crate) fn caught<T>(body: impl FnOnce() -> T) -> Result<T, PlayError> {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            if QUIET.get() {
                RAISED_AT.set(info.location().map(SourceAt::from));
            } else {
                previous(info);
            }
        }));
    });
    // A panic some other catch handled can have left its line behind.
    RAISED_AT.take();
    let was = QUIET.replace(true);
    let outcome = panic::catch_unwind(AssertUnwindSafe(body));
    QUIET.set(was);
    outcome.map_err(|panic| {
        let mut error = panicked(panic);
        if error.at.is_none() {
            error.at = RAISED_AT.take();
        }
        error
    })
}

/// A panic's payload as the failure it reports:
/// what [`Play::get`] missed, or its message.
fn panicked(panic: Box<dyn std::any::Any + Send>) -> PlayError {
    match panic.downcast::<Missed>() {
        Ok(missed) => missed.0,
        Err(panic) => PlayError::new(
            panic
                .downcast_ref::<&str>()
                .map(|message| (*message).to_owned())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panicked".to_owned()),
        ),
    }
}
