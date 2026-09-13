use std::collections::HashSet;

use serde::{Serialize, de::DeserializeOwned};

use crate::{ChoiceStyle, Icon, Knob, Pad2D, Pad2DSpec, SceneRevision};

/// Consumer-owned state shared by a catalog's scenes.
///
/// Gallery stores postcard bytes and reconstructs the value in the scenes library.
pub trait CatalogGlobals: Default + Serialize + DeserializeOwned + 'static {
    /// Declare controls and update `self`.
    ///
    /// Gallery may call this repeatedly to settle conditional declarations. Apart from
    /// [`button`](GlobalControls::button) callbacks, avoid external side effects.
    fn controls(&mut self, controls: &mut GlobalControls<'_>);

    /// Prepare staged scene content before a scene renders.
    ///
    /// Runs in windows and captures, including for two-argument scenes. Apply themes with
    /// `Ui::set_style`; Gallery keeps that style inside stages and restores its canvas and
    /// context styles.
    fn prepare(&self, _ui: &mut egui::Ui) {}
}

/// Declares catalog controls and maps stable choice labels to typed values.
pub struct GlobalControls<'a> {
    knobs: &'a mut Vec<Knob>,
    cursor: usize,
    labels: HashSet<String>,
    error: Option<String>,
}

impl<'a> GlobalControls<'a> {
    fn new(knobs: &'a mut Vec<Knob>) -> Self {
        Self {
            knobs,
            cursor: 0,
            labels: HashSet::new(),
            error: None,
        }
    }

    fn finish(self) -> Result<(), String> {
        self.knobs.truncate(self.cursor);
        match self.error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn reject(&mut self, error: impl Into<String>) {
        if self.error.is_none() {
            self.error = Some(error.into());
        }
    }

    fn slot(
        &mut self,
        label: &str,
        fresh: impl FnOnce() -> Knob,
        keep: impl Fn(&Knob) -> bool,
    ) -> &mut Knob {
        if !self.labels.insert(label.to_owned()) {
            self.reject(format!(
                "global control label {label:?} is declared more than once"
            ));
        }
        let at = self.cursor;
        self.cursor += 1;
        if let Some(found) = self.knobs[at..].iter().position(keep).map(|i| at + i) {
            self.knobs.swap(at, found);
        } else {
            self.knobs.insert(at, fresh());
        }
        &mut self.knobs[at]
    }

    fn check_options(&mut self, control: &str, options: &[String]) {
        if options.is_empty() {
            self.reject(format!("global control {control:?} declares no options"));
            return;
        }
        let mut labels = HashSet::new();
        if let Some(label) = options.iter().find(|label| !labels.insert(label.as_str())) {
            self.reject(format!(
                "global control {control:?} declares option label {label:?} more than once"
            ));
        }
    }

    /// A momentary action whose callback runs once per click.
    pub fn button(&mut self, label: &str, on_click: impl FnOnce()) -> bool {
        let clicked = match self.slot(
            label,
            || Knob::Button {
                label: label.to_owned(),
                clicked: false,
            },
            |knob| matches!(knob, Knob::Button { label: current, .. } if current == label),
        ) {
            Knob::Button { clicked, .. } => std::mem::take(clicked),
            _ => false,
        };
        if clicked {
            on_click();
        }
        clicked
    }

    pub fn text(&mut self, label: &str, default: &str) -> String {
        match self.slot(
            label,
            || Knob::Text {
                label: label.to_owned(),
                value: default.to_owned(),
            },
            |knob| matches!(knob, Knob::Text { label: current, .. } if current == label),
        ) {
            Knob::Text { value, .. } => value.clone(),
            _ => default.to_owned(),
        }
    }

    /// A finite slider with order-independent bounds and a non-negative step.
    pub fn slider(&mut self, label: &str, default: f32, min: f32, max: f32, step: f32) -> f32 {
        if ![default, min, max, step].into_iter().all(f32::is_finite) || step < 0.0 {
            self.reject(format!(
                "global control {label:?} needs finite slider values and a non-negative step"
            ));
            return default;
        }
        let (min, max) = ordered(min, max);
        match self.slot(
            label,
            || Knob::Slider {
                label: label.to_owned(),
                value: default,
                min,
                max,
                step,
            },
            |knob| matches!(knob, Knob::Slider { label: current, .. } if current == label),
        ) {
            Knob::Slider {
                value,
                min: current_min,
                max: current_max,
                step: current_step,
                ..
            } => {
                *current_min = min;
                *current_max = max;
                *current_step = step;
                *value = (*value).clamp(min, max);
                *value
            }
            _ => default,
        }
    }

    pub fn toggle(&mut self, label: &str, default: bool) -> bool {
        match self.slot(
            label,
            || Knob::Toggle {
                label: label.to_owned(),
                value: default,
            },
            |knob| matches!(knob, Knob::Toggle { label: current, .. } if current == label),
        ) {
            Knob::Toggle { value, .. } => *value,
            _ => default,
        }
    }

    pub fn color(&mut self, label: &str, default: egui::Color32) -> egui::Color32 {
        match self.slot(
            label,
            || Knob::Color {
                label: label.to_owned(),
                value: default,
            },
            |knob| matches!(knob, Knob::Color { label: current, .. } if current == label),
        ) {
            Knob::Color { value, .. } => *value,
            _ => default,
        }
    }

    /// A typed dropdown. Capture recipes select by label.
    pub fn select<T: Clone + PartialEq>(
        &mut self,
        label: &str,
        options: &[(&str, T)],
        default: T,
    ) -> T {
        self.choice(label, options, default, ChoiceStyle::Dropdown)
    }

    /// A typed vertical radio group.
    pub fn radio<T: Clone + PartialEq>(
        &mut self,
        label: &str,
        options: &[(&str, T)],
        default: T,
    ) -> T {
        self.choice(label, options, default, ChoiceStyle::Radio)
    }

    /// A typed inline segmented row.
    pub fn buttons<T: Clone + PartialEq>(
        &mut self,
        label: &str,
        options: &[(&str, T)],
        default: T,
    ) -> T {
        self.choice(label, options, default, ChoiceStyle::Buttons)
    }

    /// A typed SVG icon choice in the window bar.
    ///
    /// Each option is `(stable label, icon, value)`. Labels identify captures and reloads and
    /// provide tooltips and accessibility names. Reuse icons built with [`Icon::from_svg`].
    pub fn icon_buttons<T: Clone + PartialEq>(
        &mut self,
        label: &str,
        options: &[(&str, &Icon, T)],
        default: T,
    ) -> T {
        let labels: Vec<String> = options
            .iter()
            .map(|(option, _, _)| (*option).to_owned())
            .collect();
        self.check_options(label, &labels);
        let icons: Vec<Icon> = options.iter().map(|(_, icon, _)| (*icon).clone()).collect();
        let default_at = options
            .iter()
            .position(|(_, _, value)| value == &default)
            .unwrap_or(0);
        let last = labels.len().saturating_sub(1);
        let knob = self.slot(
            label,
            || Knob::IconButtons {
                label: label.to_owned(),
                value: default_at.min(last),
                options: labels.clone(),
                icons: icons.clone(),
            },
            |knob| matches!(knob, Knob::IconButtons { label: current, .. } if current == label),
        );
        let selected = match knob {
            Knob::IconButtons {
                value,
                options: current,
                icons: current_icons,
                ..
            } => {
                let former = current.get(*value).cloned();
                *current = labels;
                *current_icons = icons;
                *value = former
                    .and_then(|former| current.iter().position(|option| option == &former))
                    .unwrap_or(default_at)
                    .min(last);
                *value
            }
            _ => default_at,
        };
        options
            .get(selected)
            .map_or(default, |(_, _, value)| value.clone())
    }

    fn choice<T: Clone + PartialEq>(
        &mut self,
        label: &str,
        options: &[(&str, T)],
        default: T,
        style: ChoiceStyle,
    ) -> T {
        let labels: Vec<String> = options
            .iter()
            .map(|(option, _)| (*option).to_owned())
            .collect();
        self.check_options(label, &labels);
        let default_at = options
            .iter()
            .position(|(_, value)| value == &default)
            .unwrap_or(0);
        let last = labels.len().saturating_sub(1);
        let knob = self.slot(
            label,
            || Knob::Select {
                label: label.to_owned(),
                value: default_at.min(last),
                options: labels.clone(),
                style,
            },
            |knob| matches!(knob, Knob::Select { label: current, style: current_style, .. } if current == label && *current_style == style),
        );
        let selected = match knob {
            Knob::Select {
                value,
                options: current,
                ..
            } => {
                let former = current.get(*value).cloned();
                *current = labels;
                *value = former
                    .and_then(|former| current.iter().position(|option| option == &former))
                    .unwrap_or(default_at)
                    .min(last);
                *value
            }
            _ => default_at,
        };
        options
            .get(selected)
            .map_or(default, |(_, value)| value.clone())
    }

    pub fn group(&mut self, label: &str) {
        self.slot(
            label,
            || Knob::Group {
                label: label.to_owned(),
            },
            |knob| matches!(knob, Knob::Group { label: current } if current == label),
        );
    }

    /// A finite two-axis pad with order-independent bounds.
    pub fn pad2d(&mut self, label: &str, spec: Pad2DSpec) -> Pad2D {
        if ![
            spec.default_x,
            spec.default_y,
            spec.min_x,
            spec.max_x,
            spec.min_y,
            spec.max_y,
        ]
        .into_iter()
        .all(f32::is_finite)
        {
            self.reject(format!(
                "global control {label:?} needs finite pad defaults and bounds"
            ));
            return Pad2D {
                x: spec.default_x,
                y: spec.default_y,
            };
        }
        let (ordered_min_x, ordered_max_x) = ordered(spec.min_x, spec.max_x);
        let (ordered_min_y, ordered_max_y) = ordered(spec.min_y, spec.max_y);
        match self.slot(
            label,
            || Knob::Pad2D {
                label: label.to_owned(),
                x: spec.default_x,
                y: spec.default_y,
                min_x: spec.min_x,
                max_x: spec.max_x,
                min_y: spec.min_y,
                max_y: spec.max_y,
                invert_y: spec.invert_y,
            },
            |knob| matches!(knob, Knob::Pad2D { label: current, .. } if current == label),
        ) {
            Knob::Pad2D {
                x,
                y,
                min_x,
                max_x,
                min_y,
                max_y,
                invert_y,
                ..
            } => {
                *min_x = ordered_min_x;
                *max_x = ordered_max_x;
                *min_y = ordered_min_y;
                *max_y = ordered_max_y;
                *invert_y = spec.invert_y;
                *x = (*x).clamp(ordered_min_x, ordered_max_x);
                *y = (*y).clamp(ordered_min_y, ordered_max_y);
                Pad2D { x: *x, y: *y }
            }
            _ => Pad2D {
                x: spec.default_x,
                y: spec.default_y,
            },
        }
    }
}

fn ordered(min: f32, max: f32) -> (f32, f32) {
    (min.min(max), min.max(max))
}

pub(crate) fn control_shape(knobs: &[Knob]) -> Vec<(u8, String, Vec<String>)> {
    knobs
        .iter()
        .map(|knob| {
            let (kind, label, options) = match knob {
                Knob::Button { label, .. } => (0, label, Vec::new()),
                Knob::Text { label, .. } => (1, label, Vec::new()),
                Knob::Slider { label, .. } => (2, label, Vec::new()),
                Knob::Toggle { label, .. } => (3, label, Vec::new()),
                Knob::Color { label, .. } => (4, label, Vec::new()),
                Knob::Select {
                    label,
                    options,
                    style,
                    ..
                } => (
                    match style {
                        ChoiceStyle::Dropdown => 5,
                        ChoiceStyle::Radio => 6,
                        ChoiceStyle::Buttons => 7,
                    },
                    label,
                    options.clone(),
                ),
                Knob::IconButtons { label, options, .. } => (8, label, options.clone()),
                Knob::Pad2D { label, .. } => (9, label, Vec::new()),
                Knob::Group { label } => (10, label, Vec::new()),
            };
            (kind, label.clone(), options)
        })
        .collect()
}

/// Type-erased operations exported by a scenes library with catalog globals.
#[doc(hidden)]
#[derive(Clone, Copy)]
pub struct GlobalEntry {
    defaults: fn() -> Result<Vec<u8>, String>,
    sync: fn(&mut Vec<u8>, &mut Vec<Knob>) -> Result<(), String>,
    prepare: fn(&[u8], &mut egui::Ui) -> Result<(), String>,
}

impl GlobalEntry {
    #[doc(hidden)]
    pub const fn of<T: CatalogGlobals>() -> Self {
        Self {
            defaults: defaults::<T>,
            sync: sync::<T>,
            prepare: prepare::<T>,
        }
    }

    fn default_bytes(self) -> Result<Vec<u8>, String> {
        (self.defaults)()
    }

    fn sync(self, bytes: &mut Vec<u8>, knobs: &mut Vec<Knob>) -> Result<(), String> {
        (self.sync)(bytes, knobs)
    }

    pub(crate) fn prepare(self, bytes: &[u8], ui: &mut egui::Ui) -> Result<(), String> {
        (self.prepare)(bytes, ui)
    }
}

inventory::collect!(GlobalEntry);

fn defaults<T: CatalogGlobals>() -> Result<Vec<u8>, String> {
    postcard::to_stdvec(&T::default())
        .map_err(|error| format!("serialize default catalog globals: {error}"))
}

fn sync<T: CatalogGlobals>(bytes: &mut Vec<u8>, knobs: &mut Vec<Knob>) -> Result<(), String> {
    let mut globals: T = postcard::from_bytes(bytes)
        .map_err(|error| format!("deserialize catalog globals: {error}"))?;
    let mut controls = GlobalControls::new(knobs);
    globals.controls(&mut controls);
    controls.finish()?;
    let encoded = postcard::to_stdvec(&globals)
        .map_err(|error| format!("serialize catalog globals: {error}"))?;
    postcard::from_bytes::<T>(&encoded)
        .map_err(|error| format!("validate serialized catalog globals: {error}"))?;
    *bytes = encoded;
    Ok(())
}

fn prepare<T: CatalogGlobals>(bytes: &[u8], ui: &mut egui::Ui) -> Result<(), String> {
    let globals: T = postcard::from_bytes(bytes)
        .map_err(|error| format!("deserialize catalog globals: {error}"))?;
    globals.prepare(ui);
    Ok(())
}

pub(crate) struct GlobalState {
    entry: GlobalEntry,
    revision: SceneRevision,
    bytes: Vec<u8>,
    knobs: Vec<Knob>,
}

impl GlobalState {
    pub(crate) fn new(entry: GlobalEntry, revision: SceneRevision) -> Result<Self, String> {
        let mut state = Self {
            entry,
            revision,
            bytes: entry.default_bytes()?,
            knobs: Vec::new(),
        };
        state.sync()?;
        Ok(state)
    }

    pub(crate) fn reload(
        &mut self,
        entry: GlobalEntry,
        revision: SceneRevision,
    ) -> Result<(), String> {
        let mut next = Self {
            entry,
            revision,
            bytes: entry.default_bytes()?,
            knobs: self.knobs.clone(),
        };
        next.sync()?;
        *self = next;
        Ok(())
    }

    pub(crate) fn revision(&self) -> SceneRevision {
        self.revision
    }

    pub(crate) fn sync(&mut self) -> Result<(), String> {
        const MAX_PASSES: usize = 256;

        let mut stable_passes = 0;
        for _ in 0..MAX_PASSES {
            let before = control_shape(&self.knobs);
            self.entry.sync(&mut self.bytes, &mut self.knobs)?;
            if control_shape(&self.knobs) == before {
                stable_passes += 1;
                if stable_passes == 2 {
                    return Ok(());
                }
            } else {
                stable_passes = 0;
            }
        }
        Err("catalog global controls did not stabilize".to_owned())
    }

    pub(crate) fn knobs(&self) -> &[Knob] {
        &self.knobs
    }

    pub(crate) fn knobs_mut(&mut self) -> &mut [Knob] {
        &mut self.knobs
    }

    pub(crate) fn parts(&self) -> (GlobalEntry, &[u8]) {
        (self.entry, &self.bytes)
    }
}

#[doc(hidden)]
pub fn decode<T: CatalogGlobals>(bytes: &[u8]) -> T {
    postcard::from_bytes(bytes).expect("gallery validated catalog globals before rendering a scene")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
    enum Theme {
        #[default]
        Auto,
        Dark,
        Light,
    }

    #[derive(Default, serde::Deserialize, serde::Serialize)]
    struct Globals {
        theme: Theme,
        enabled: bool,
    }

    impl CatalogGlobals for Globals {
        fn controls(&mut self, controls: &mut GlobalControls<'_>) {
            self.theme = controls.buttons(
                "theme",
                &[
                    ("auto", Theme::Auto),
                    ("dark", Theme::Dark),
                    ("light", Theme::Light),
                ],
                self.theme,
            );
            self.enabled = controls.toggle("enabled", self.enabled);
        }
    }

    #[test]
    fn typed_values_round_trip_through_the_wire_state() {
        let mut state = GlobalState::new(GlobalEntry::of::<Globals>(), SceneRevision::INITIAL)
            .expect("global state");
        if let Knob::Select { value, .. } = &mut state.knobs[0] {
            *value = 1;
        }
        state.sync().expect("selected state serializes");
        let globals: Globals = postcard::from_bytes(&state.bytes).expect("typed state");
        assert_eq!(globals.theme, Theme::Dark);
    }

    #[test]
    fn defaults_settle_through_long_reverse_declared_chains() {
        const STEPS: usize = 12;

        #[derive(Default, serde::Deserialize, serde::Serialize)]
        struct Conditional {
            steps: [bool; STEPS],
        }

        impl CatalogGlobals for Conditional {
            fn controls(&mut self, controls: &mut GlobalControls<'_>) {
                for at in (1..STEPS).rev() {
                    if self.steps[at - 1] {
                        self.steps[at] = controls.toggle(&format!("step {at}"), true);
                    }
                }
                self.steps[0] = controls.toggle("step 0", true);
            }
        }

        let state = GlobalState::new(GlobalEntry::of::<Conditional>(), SceneRevision::INITIAL)
            .expect("conditional defaults stabilize before the state is exposed");
        let value: Conditional = postcard::from_bytes(&state.bytes).expect("typed state");

        assert!(value.steps.into_iter().all(|step| step));
        assert_eq!(state.knobs.len(), STEPS);
    }

    #[test]
    fn settling_consumes_a_global_button_click_only_once() {
        #[derive(Default, serde::Deserialize, serde::Serialize)]
        struct Counter(u8);

        impl CatalogGlobals for Counter {
            fn controls(&mut self, controls: &mut GlobalControls<'_>) {
                controls.button("increment", || self.0 += 1);
            }
        }

        let mut state = GlobalState::new(GlobalEntry::of::<Counter>(), SceneRevision::INITIAL)
            .expect("global state");
        let Knob::Button { clicked, .. } = &mut state.knobs[0] else {
            panic!("a button was declared")
        };
        *clicked = true;

        state.sync().expect("the click settles");

        let value: Counter = postcard::from_bytes(&state.bytes).expect("typed state");
        assert_eq!(value.0, 1);
    }

    #[test]
    fn standard_color_and_pad_values_are_postcard_compatible() {
        #[derive(Default, serde::Deserialize, serde::Serialize)]
        struct Visuals {
            tint: egui::Color32,
            offset: crate::Pad2D,
        }

        impl CatalogGlobals for Visuals {
            fn controls(&mut self, controls: &mut GlobalControls<'_>) {
                self.tint = controls.color("tint", self.tint);
                self.offset = controls.pad2d("offset", crate::Pad2DSpec::default());
            }
        }

        GlobalState::new(GlobalEntry::of::<Visuals>(), SceneRevision::INITIAL)
            .expect("egui color and gallery pad values cross postcard");
    }

    #[test]
    fn a_reload_preserves_choices_by_label_when_their_order_changes() {
        #[derive(Default, serde::Deserialize, serde::Serialize)]
        struct Reordered {
            theme: Theme,
        }
        impl CatalogGlobals for Reordered {
            fn controls(&mut self, controls: &mut GlobalControls<'_>) {
                self.theme = controls.buttons(
                    "theme",
                    &[
                        ("light", Theme::Light),
                        ("auto", Theme::Auto),
                        ("dark", Theme::Dark),
                    ],
                    self.theme,
                );
            }
        }

        let mut state = GlobalState::new(GlobalEntry::of::<Globals>(), SceneRevision::INITIAL)
            .expect("global state");
        if let Knob::Select { value, .. } = &mut state.knobs[0] {
            *value = 1;
        }
        state.sync().expect("old state");
        state
            .reload(
                GlobalEntry::of::<Reordered>(),
                SceneRevision::INITIAL.next(),
            )
            .expect("new state");
        let globals: Reordered = postcard::from_bytes(&state.bytes).expect("typed state");
        assert_eq!(globals.theme, Theme::Dark);
    }

    #[test]
    fn a_reload_replays_controls_by_label_when_declarations_move() {
        #[derive(Default, serde::Deserialize, serde::Serialize)]
        struct Moved {
            theme: Theme,
            enabled: bool,
        }
        impl CatalogGlobals for Moved {
            fn controls(&mut self, controls: &mut GlobalControls<'_>) {
                self.enabled = controls.toggle("enabled", self.enabled);
                self.theme = controls.buttons(
                    "theme",
                    &[
                        ("auto", Theme::Auto),
                        ("dark", Theme::Dark),
                        ("light", Theme::Light),
                    ],
                    self.theme,
                );
            }
        }

        let mut state = GlobalState::new(GlobalEntry::of::<Globals>(), SceneRevision::INITIAL)
            .expect("global state");
        if let Knob::Select { value, .. } = &mut state.knobs[0] {
            *value = 1;
        }
        if let Knob::Toggle { value, .. } = &mut state.knobs[1] {
            *value = true;
        }
        state.sync().expect("old state");
        state
            .reload(GlobalEntry::of::<Moved>(), SceneRevision::INITIAL.next())
            .expect("new state");

        let globals: Moved = postcard::from_bytes(&state.bytes).expect("typed state");
        assert!(globals.enabled);
        assert_eq!(globals.theme, Theme::Dark);
    }

    #[test]
    fn a_failed_reload_never_pairs_new_code_with_old_bytes() {
        #[derive(Default, serde::Deserialize)]
        struct CannotSerialize;

        impl serde::Serialize for CannotSerialize {
            fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                Err(serde::ser::Error::custom("not postcard compatible"))
            }
        }

        impl CatalogGlobals for CannotSerialize {
            fn controls(&mut self, _controls: &mut GlobalControls<'_>) {}
        }

        let mut state = GlobalState::new(GlobalEntry::of::<Globals>(), SceneRevision::INITIAL)
            .expect("old global state");
        let old_revision = state.revision();
        let (_, old_bytes) = state.parts();
        let old_bytes = old_bytes.to_vec();

        let error = state
            .reload(
                GlobalEntry::of::<CannotSerialize>(),
                SceneRevision::INITIAL.next(),
            )
            .expect_err("the new type cannot cross postcard");

        assert!(
            error.contains("serialize default catalog globals"),
            "{error}"
        );
        assert_eq!(state.revision(), old_revision);
        let (_, bytes) = state.parts();
        assert_eq!(bytes, old_bytes);
        postcard::from_bytes::<Globals>(bytes).expect("old bytes still belong to old code");
    }

    #[test]
    fn a_value_must_decode_before_its_bytes_are_committed() {
        #[derive(Default, serde::Serialize)]
        struct OneWay(bool);

        impl<'de> serde::Deserialize<'de> for OneWay {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let value = <bool as serde::Deserialize>::deserialize(deserializer)?;
                if value {
                    Err(serde::de::Error::custom("true cannot be decoded"))
                } else {
                    Ok(Self(value))
                }
            }
        }

        impl CatalogGlobals for OneWay {
            fn controls(&mut self, controls: &mut GlobalControls<'_>) {
                self.0 = controls.toggle("one way", self.0);
            }
        }

        let mut state = GlobalState::new(GlobalEntry::of::<OneWay>(), SceneRevision::INITIAL)
            .expect("the default round trips");
        let before = state.bytes.clone();
        let Knob::Toggle { value, .. } = &mut state.knobs[0] else {
            panic!("a toggle was declared")
        };
        *value = true;

        let error = state
            .sync()
            .expect_err("the selected value cannot round trip");

        assert!(error.starts_with("validate serialized catalog globals:"));
        assert_eq!(state.bytes, before, "invalid bytes were not committed");
        postcard::from_bytes::<OneWay>(&state.bytes).expect("the previous bytes remain valid");
    }

    #[test]
    fn duplicate_choice_labels_are_rejected() {
        #[derive(Default, serde::Deserialize, serde::Serialize)]
        struct Duplicate;

        impl CatalogGlobals for Duplicate {
            fn controls(&mut self, controls: &mut GlobalControls<'_>) {
                controls.buttons("theme", &[("Light", false), ("Light", true)], false);
            }
        }

        let error = GlobalState::new(GlobalEntry::of::<Duplicate>(), SceneRevision::INITIAL)
            .err()
            .expect("duplicate choice labels are rejected");
        assert_eq!(
            error,
            "global control \"theme\" declares option label \"Light\" more than once"
        );
    }

    #[test]
    fn empty_choices_are_rejected() {
        #[derive(Default, serde::Deserialize, serde::Serialize)]
        struct Empty;

        impl CatalogGlobals for Empty {
            fn controls(&mut self, controls: &mut GlobalControls<'_>) {
                controls.select::<bool>("theme", &[], false);
            }
        }

        let error = GlobalState::new(GlobalEntry::of::<Empty>(), SceneRevision::INITIAL)
            .err()
            .expect("an empty choice cannot produce a control");
        assert_eq!(error, "global control \"theme\" declares no options");
    }

    #[test]
    fn non_finite_numeric_controls_are_rejected_before_rendering() {
        #[derive(Default, serde::Deserialize, serde::Serialize)]
        struct NonFinite;

        impl CatalogGlobals for NonFinite {
            fn controls(&mut self, controls: &mut GlobalControls<'_>) {
                controls.slider("scale", 1.0, f32::NEG_INFINITY, 1.0, 0.1);
            }
        }

        let error = GlobalState::new(GlobalEntry::of::<NonFinite>(), SceneRevision::INITIAL)
            .err()
            .expect("an infinite linear range would produce invalid widget geometry");
        assert_eq!(
            error,
            "global control \"scale\" needs finite slider values and a non-negative step"
        );
    }

    #[test]
    fn reversed_ranges_clamp_without_panicking() {
        #[derive(serde::Deserialize, serde::Serialize)]
        struct Ranged {
            slider: f32,
            pad: Pad2D,
        }

        impl Default for Ranged {
            fn default() -> Self {
                Self {
                    slider: 5.0,
                    pad: Pad2D { x: 5.0, y: -5.0 },
                }
            }
        }

        impl CatalogGlobals for Ranged {
            fn controls(&mut self, controls: &mut GlobalControls<'_>) {
                self.slider = controls.slider("slider", self.slider, 1.0, -1.0, 0.1);
                self.pad = controls.pad2d(
                    "pad",
                    Pad2DSpec {
                        default_x: self.pad.x,
                        default_y: self.pad.y,
                        min_x: 1.0,
                        max_x: -1.0,
                        min_y: 2.0,
                        max_y: -2.0,
                        invert_y: false,
                    },
                );
            }
        }

        let state = GlobalState::new(GlobalEntry::of::<Ranged>(), SceneRevision::INITIAL)
            .expect("reversed ranges are valid");
        let value: Ranged = postcard::from_bytes(&state.bytes).expect("typed state");
        assert_eq!(value.slider, 1.0);
        assert_eq!(value.pad, Pad2D { x: 1.0, y: -2.0 });
        assert!(matches!(
            state.knobs[0],
            Knob::Slider {
                min: -1.0,
                max: 1.0,
                ..
            }
        ));
        assert!(matches!(
            state.knobs[1],
            Knob::Pad2D {
                min_x: -1.0,
                max_x: 1.0,
                min_y: -2.0,
                max_y: 2.0,
                ..
            }
        ));
    }
}
