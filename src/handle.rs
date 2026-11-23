//! [`Knob`]: the handle a knob accessor returns.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::knobs::{KnobState, Pad2D, Span};

/// A knob declared this frame, readable and writable anywhere in it:
/// a change callback, a helper, a stage's closure.
///
/// ```ignore
/// let power = ctx.toggle("power", false);
/// Switch::new(power.get())
///     .on_change({ let power = power.clone(); move |on| { power.set(on); } })
///     .show(ui);
/// ```
///
/// Clones share one value; a [`set`](Self::set) lands in the store when the scene returns.
pub struct Knob<T: KnobValue> {
    shared: Rc<Shared<T>>,
}

impl<T: KnobValue> Clone for Knob<T> {
    fn clone(&self) -> Self {
        Self {
            shared: Rc::clone(&self.shared),
        }
    }
}

impl<T: KnobValue + std::fmt::Debug> std::fmt::Debug for Knob<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Knob").field(&self.get()).finish()
    }
}

struct Shared<T> {
    slot: usize,
    /// The declared knob plus any write since, whose ranges clamp the next write.
    knob: RefCell<KnobState>,
    written: Cell<bool>,
    open: Cell<bool>,
    value: std::marker::PhantomData<T>,
}

impl<T: KnobValue> Knob<T> {
    pub(crate) fn new(slot: usize, knob: KnobState) -> (Self, Rc<dyn Pending>) {
        let shared = Rc::new(Shared::<T> {
            slot,
            knob: RefCell::new(knob),
            written: Cell::new(false),
            open: Cell::new(true),
            value: std::marker::PhantomData,
        });
        let pending: Rc<dyn Pending> = shared.clone();
        (Self { shared }, pending)
    }

    /// This frame's value, including a [`set`](Self::set) made in it; a stale handle keeps that frame's.
    #[must_use]
    pub fn get(&self) -> T {
        T::read(&self.shared.knob.borrow()).expect("a handle's knob keeps its kind")
    }

    /// Write the knob, clamped and snapped as its control would.
    ///
    /// `false`, changing nothing, for a value it can't take (non-finite, a select without options),
    /// after the knob's frame, or while a capture's recipe or a play's `set_knob` holds it.
    pub fn set(&self, value: T) -> bool {
        if !self.shared.open.get() || !value.write(&mut self.shared.knob.borrow_mut()) {
            return false;
        }
        self.shared.written.set(true);
        true
    }
}

impl Knob<usize> {
    /// Select the option labelled exactly `option`; `false` when there is none.
    pub fn select(&self, option: &str) -> bool {
        let at = match &*self.shared.knob.borrow() {
            KnobState::Select { options, .. } => options.iter().position(|opt| opt == option),
            _ => None,
        };
        at.is_some_and(|at| self.set(at))
    }
}

/// A handle with its value type erased, for [`SceneCtx`](crate::SceneCtx) to land and close.
pub(crate) trait Pending {
    /// Land a write in the store; `true` if it changed the stored value.
    fn apply(&self, store: &mut [KnobState]) -> bool;
    fn close(&self);
}

impl<T: KnobValue> Pending for Shared<T> {
    fn apply(&self, store: &mut [KnobState]) -> bool {
        if !self.written.take() {
            return false;
        }
        let target = &mut store[self.slot];
        let before = T::read(target);
        let value = T::read(&self.knob.borrow());
        value.is_some_and(|value| value.write(target)) && T::read(target) != before
    }

    fn close(&self) {
        self.open.set(false);
    }
}

/// A value a knob holds: `f32` for a slider, `bool` for a toggle, `String` for text,
/// `Color32` for a colour, `usize` for a choice's index, and [`Pad2D`].
pub trait KnobValue: Clone + PartialEq + 'static + sealed::Sealed {
    #[doc(hidden)]
    fn read(knob: &KnobState) -> Option<Self>;
    /// Write into a knob of this value's kind; `false` for another kind or a value it rejects.
    #[doc(hidden)]
    fn write(self, knob: &mut KnobState) -> bool;
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for f32 {}
    impl Sealed for bool {}
    impl Sealed for String {}
    impl Sealed for egui::Color32 {}
    impl Sealed for usize {}
    impl Sealed for super::Pad2D {}
}

impl KnobValue for f32 {
    fn read(knob: &KnobState) -> Option<Self> {
        match knob {
            KnobState::Slider { value, .. } => Some(*value),
            _ => None,
        }
    }

    fn write(self, knob: &mut KnobState) -> bool {
        let KnobState::Slider {
            value,
            min,
            max,
            step,
            ..
        } = knob
        else {
            return false;
        };
        slider_value(self, Span::new(*min, *max), *step).is_some_and(|next| {
            *value = next;
            true
        })
    }
}

/// `value` as the panel's slider stores it, or `None` if it can't take it.
///
/// Mirrors egui 0.36's `Slider::set_value` under the panel's `step_by` and `fixed_decimals`
/// (clamp, snap from `min` in `f64`, round, no final clamp, so it may overshoot `max`);
/// any mismatch has the panel rewrite the value every frame.
pub(crate) fn slider_value(value: f32, span: Span, step: f32) -> Option<f32> {
    // `f64::clamp` panics on a NaN bound, which a scene is free to declare.
    if !value.is_finite() || span.has_nan() {
        return None;
    }
    let (min, max) = (f64::from(span.min), f64::from(span.max));
    let mut next = f64::from(value).clamp(min.min(max), min.max(max));
    if step > 0.0 {
        let step_f64 = f64::from(step);
        next = min + ((next - min) / step_f64).round() * step_f64;
        next = egui::emath::round_to_decimals(next, crate::knobs::step_decimals(step));
    }
    // An infinite step snaps to NaN, egui's panel included.
    if !next.is_finite() {
        return None;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "egui stores the panel's f32 knob the same way"
    )]
    Some(next as f32)
}

impl KnobValue for bool {
    fn read(knob: &KnobState) -> Option<Self> {
        match knob {
            KnobState::Toggle { value, .. } => Some(*value),
            _ => None,
        }
    }

    fn write(self, knob: &mut KnobState) -> bool {
        match knob {
            KnobState::Toggle { value, .. } => {
                *value = self;
                true
            }
            _ => false,
        }
    }
}

impl KnobValue for String {
    fn read(knob: &KnobState) -> Option<Self> {
        match knob {
            KnobState::Text { value, .. } => Some(value.clone()),
            _ => None,
        }
    }

    fn write(self, knob: &mut KnobState) -> bool {
        match knob {
            KnobState::Text { value, .. } => {
                *value = self;
                true
            }
            _ => false,
        }
    }
}

impl KnobValue for egui::Color32 {
    fn read(knob: &KnobState) -> Option<Self> {
        match knob {
            KnobState::Color { value, .. } => Some(*value),
            _ => None,
        }
    }

    /// Through HSVA, as egui 0.36's colour picker stores it;
    /// the panel would rewrite an invalid premultiplied colour, so a scene re-setting it never idles.
    fn write(self, knob: &mut KnobState) -> bool {
        match knob {
            KnobState::Color { value, .. } => {
                *value = Self::from(egui::ecolor::Hsva::from(self));
                true
            }
            _ => false,
        }
    }
}

impl KnobValue for usize {
    fn read(knob: &KnobState) -> Option<Self> {
        match knob {
            KnobState::Select { value, .. } => Some(*value),
            _ => None,
        }
    }

    /// Clamped to the last option, as the accessor clamps its default.
    fn write(self, knob: &mut KnobState) -> bool {
        match knob {
            KnobState::Select { value, options, .. } if !options.is_empty() => {
                *value = self.min(options.len() - 1);
                true
            }
            _ => false,
        }
    }
}

impl KnobValue for Pad2D {
    fn read(knob: &KnobState) -> Option<Self> {
        match knob {
            KnobState::Pad2D { x, y, .. } => Some(Pad2D { x: *x, y: *y }),
            _ => None,
        }
    }

    /// Each axis clamped to its own range.
    fn write(self, knob: &mut KnobState) -> bool {
        let KnobState::Pad2D {
            x,
            y,
            min_x,
            max_x,
            min_y,
            max_y,
            ..
        } = knob
        else {
            return false;
        };
        let (Some(at_x), Some(at_y)) = (
            Span::new(*min_x, *max_x).clamp(self.x),
            Span::new(*min_y, *max_y).clamp(self.y),
        ) else {
            return false;
        };
        (*x, *y) = (at_x, at_y);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each slider's value after the panel draws it once, untouched.
    fn after_panel(values: &[f32], min: f32, max: f32, step: f32) -> Vec<f32> {
        let knobs: Vec<KnobState> = values
            .iter()
            .enumerate()
            .map(|(at, value)| KnobState::Slider {
                label: format!("v{at}"),
                value: *value,
                min,
                max,
                step,
            })
            .collect();
        let mut harness = egui_kittest::Harness::new_ui_state(
            |ui, knobs: &mut Vec<KnobState>| {
                crate::knobs::render_knobs(ui, knobs);
            },
            knobs,
        );
        harness.run();
        harness
            .state()
            .iter()
            .map(|knob| f32::read(knob).expect("still a slider"))
            .collect()
    }

    #[test]
    fn a_colour_write_stores_what_the_panel_picker_leaves_alone() {
        let additive = egui::Color32::from_rgba_premultiplied(255, 0, 0, 0);
        let invalid = egui::Color32::from_rgba_premultiplied(255, 0, 0, 10);
        let colours = [additive, invalid, egui::Color32::from_rgb(10, 200, 30)];
        let written: Vec<KnobState> = colours
            .iter()
            .enumerate()
            .map(|(at, colour)| {
                let mut knob = KnobState::Color {
                    label: format!("c{at}"),
                    value: egui::Color32::BLACK,
                };
                assert!(colour.write(&mut knob));
                knob
            })
            .collect();
        let stored = |knobs: &[KnobState]| -> Vec<egui::Color32> {
            knobs
                .iter()
                .map(|knob| egui::Color32::read(knob).expect("still a colour"))
                .collect()
        };
        let before = stored(&written);
        assert_ne!(
            before[1], invalid,
            "a colour the picker rewrites is the case under test"
        );
        let mut harness = egui_kittest::Harness::new_ui_state(
            |ui, knobs: &mut Vec<KnobState>| {
                crate::knobs::render_knobs(ui, knobs);
            },
            written,
        );
        harness.run();
        assert_eq!(
            stored(harness.state()),
            before,
            "the picker would rewrite these every frame it draws"
        );
    }

    #[test]
    fn a_slider_write_stores_exactly_what_the_panel_slider_would() {
        assert_ne!(
            after_panel(&[0.85], 0.0, 1.0, 0.1)[0].to_bits(),
            0.85_f32.to_bits(),
            "the panel snaps an off-grid value as it draws, which is what this test leans on"
        );
        let ranges = [
            (0.0, 1.0, 0.1),
            (-1.0, 1.0, 0.1),
            (0.0, 1.0, 0.01),
            (0.0, 10.0, 0.3),
            (0.0, 1.0, 0.4),
            (0.0, 1.0, 0.6),
            (12.0, 64.0, 1.0),
            (0.0, 1.0, 0.0),
        ];
        for (min, max, step) in ranges {
            let span = max - min;
            let raw: Vec<f32> = (0..=400)
                .map(|i| min - span / 2.0 + span * 2.0 * i as f32 / 400.0)
                .collect();
            let written: Vec<f32> = raw
                .iter()
                .map(|value| slider_value(*value, Span::new(min, max), step).expect("finite"))
                .collect();
            let dragged = after_panel(&raw, min, max, step);
            let redrawn = after_panel(&written, min, max, step);
            for at in 0..raw.len() {
                let (value, handle) = (raw[at], written[at]);
                assert_eq!(
                    handle.to_bits(),
                    dragged[at].to_bits(),
                    "{value} on {min}..={max} step {step}: the handle and the panel disagree"
                );
                assert_eq!(
                    handle.to_bits(),
                    redrawn[at].to_bits(),
                    "{value} on {min}..={max} step {step}: the panel would rewrite it every frame"
                );
            }
        }
    }
}
