//! What a play expects of the whole canvas:
//! [`Canvas`], [`Screenshot`] and [`ExpectCanvas`].

use std::panic::Location;

use super::snapshot::{Asked, Of};
use super::{Play, PlayResult};

/// The whole canvas a scene draws on:
/// `p.expect(Canvas).to_match_screenshot("saved")`.
#[derive(Clone, Copy, Debug)]
pub struct Canvas;

/// A screenshot check: the reference's name, and what it overrides of `gallery.toml`'s `[snapshots]`.
/// A bare name converts, so `to_match_screenshot("saved")` keeps the catalog's budget.
#[derive(Clone, PartialEq, Debug)]
pub struct Screenshot {
    name: String,
    tolerance: Option<u32>,
    threshold: Option<f32>,
}

impl Screenshot {
    /// The reference this check compares against, named within its play.
    #[must_use]
    pub fn named(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            tolerance: None,
            threshold: None,
        }
    }

    /// How many pixels may differ.
    #[must_use]
    pub fn tolerance(mut self, pixels: u32) -> Self {
        self.tolerance = Some(pixels);
        self
    }

    /// How far apart two pixels' colours may be and still count as alike, from 0 to 1.
    /// At 0 every pixel counts, an antialiased edge's included.
    #[must_use]
    pub fn threshold(mut self, distance: f32) -> Self {
        self.threshold = Some(distance);
        self
    }
}

impl From<&str> for Screenshot {
    fn from(name: &str) -> Self {
        Self::named(name)
    }
}

impl From<String> for Screenshot {
    fn from(name: String) -> Self {
        Self::named(name)
    }
}

/// An expectation about the whole canvas, checked by
/// [`to_match_screenshot`](Self::to_match_screenshot)
/// or [`to_match_accesskit_snapshot`](Self::to_match_accesskit_snapshot).
#[must_use = "an expectation checks nothing until one of its matchers is called"]
pub struct ExpectCanvas<'p, 'd> {
    pub(super) play: &'p mut Play<'d>,
    pub(super) message: Option<String>,
    pub(super) within: Option<u32>,
}

impl ExpectCanvas<'_, '_> {
    /// Put the check in the play's own words, which lead its failure.
    pub fn because(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }

    /// Give up after `frames` frames rather than the play's wait budget.
    pub fn within(mut self, frames: u32) -> Self {
        self.within = Some(frames);
        self
    }

    /// The canvas looks as its reference does, in `__snapshots__/` beside the play's file.
    ///
    /// A canvas that already does passes without drawing a frame.
    /// Otherwise frames are drawn until the scene stops asking for them, and that one is judged.
    /// With no reference yet, or one that differs, the check fails and leaves the canvas as
    /// `<name>.new.png`, with a `.diff.png` where they differ; `UPDATE_SNAPSHOTS=1` takes it instead.
    ///
    /// What is compared is the scene's content and any popup or window over it;
    /// a canvas too small for them grows, and stays grown.
    ///
    /// Only the catalog's tests compare: in a capture the check is logged, and nothing more.
    ///
    /// # Errors
    /// When the canvas still differs from its reference once it settles or the wait budget is spent,
    /// when it has none, or when the name was already checked in this play.
    #[track_caller]
    pub fn to_match_screenshot(self, screenshot: impl Into<Screenshot>) -> PlayResult {
        let at = Location::caller();
        let Screenshot {
            name,
            tolerance,
            threshold,
        } = screenshot.into();
        let off_the_scale = threshold.filter(|it| !(0.0..=1.0).contains(it));
        let asked = Asked {
            subject: "canvas".to_owned(),
            matcher: "to_match_screenshot",
            kind: "screenshot",
            name,
            misused: off_the_scale.map(|threshold| {
                format!(
                    "threshold {threshold} is out of range; it is a colour distance from 0 to 1"
                )
            }),
            message: self.message,
            within: self.within,
        };
        let of = Of::Screenshot {
            tolerance,
            threshold,
        };
        asked.run(self.play, of, at)
    }

    /// The canvas's accessibility tree reads as its reference does,
    /// `<name>.accesskit.json` in `__snapshots__/` beside the play's file: each widget's role,
    /// label, value and states, as AccessKit names them, and the widgets inside it.
    ///
    /// Waits, fails and is taken as [`to_match_screenshot`](Self::to_match_screenshot) is.
    /// One reference does for both renderers, unless the scene draws otherwise under one.
    ///
    /// # Errors
    /// When the tree still differs from its reference once it settles or the wait budget
    /// is spent, when it has none, or when the name was already checked in this play.
    #[track_caller]
    pub fn to_match_accesskit_snapshot(self, name: impl Into<String>) -> PlayResult {
        let at = Location::caller();
        let asked = Asked {
            subject: "canvas".to_owned(),
            matcher: "to_match_accesskit_snapshot",
            kind: "AccessKit snapshot",
            name: name.into(),
            misused: None,
            message: self.message,
            within: self.within,
        };
        asked.run(self.play, Of::Tree { by: None }, at)
    }
}
