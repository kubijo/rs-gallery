//! Assertions: matchers that retry until they hold,
//! every check left in the play's log.
//!
//! Values are compared with googletest's matchers,
//! which phrase what they expect;
//! a plain value means equality to it.

use std::{fmt::Debug, panic::Location};

use egui::accesskit::Toggled;
use googletest::matcher::{Matcher, MatcherResult};

use super::{By, Found, Play, PlayResult, caught, log::Check};

/// What a matcher compares against: a googletest matcher, or a value it must equal.
///
/// `Kind` only tells the impls apart: one for every matcher
/// beside one for `&str` would overlap, since googletest
/// could implement `Matcher` for `&str` itself.
pub trait Expected<T, Kind> {
    #[doc(hidden)]
    fn holds(&self, actual: &T) -> bool;
    /// What holds, as a verb phrase, or what doesn't when `holds` is false.
    #[doc(hidden)]
    fn describe(&self, holds: bool) -> String;
    #[doc(hidden)]
    fn explain(&self, actual: &T) -> String;
}

#[doc(hidden)]
pub struct ByValue;
#[doc(hidden)]
pub struct Text;
#[doc(hidden)]
pub struct Literal;

fn sense(holds: bool) -> MatcherResult {
    if holds {
        MatcherResult::Match
    } else {
        MatcherResult::NoMatch
    }
}

/// googletest colours a string's diff when stdout is a terminal;
/// the log keeps text, and each view styles it as it shows it.
fn text(phrase: impl std::fmt::Display) -> String {
    crate::style::plain(&phrase.to_string())
}

impl<T: Debug + Copy, M: Matcher<T>> Expected<T, ByValue> for M {
    fn holds(&self, actual: &T) -> bool {
        self.matches(*actual).is_match()
    }
    fn describe(&self, holds: bool) -> String {
        text(Matcher::describe(self, sense(holds)))
    }
    fn explain(&self, actual: &T) -> String {
        text(self.explain_match(*actual))
    }
}

impl<M: for<'a> Matcher<&'a str>> Expected<String, Text> for M {
    fn holds(&self, actual: &String) -> bool {
        self.matches(actual.as_str()).is_match()
    }
    fn describe(&self, holds: bool) -> String {
        text(Matcher::<&str>::describe(self, sense(holds)))
    }
    fn explain(&self, actual: &String) -> String {
        text(self.explain_match(actual.as_str()))
    }
}

impl Expected<String, Literal> for &str {
    fn holds(&self, actual: &String) -> bool {
        actual == self
    }
    fn describe(&self, holds: bool) -> String {
        Expected::<String, Text>::describe(&googletest::matchers::eq(*self), holds)
    }
    fn explain(&self, actual: &String) -> String {
        Expected::<String, Text>::explain(&googletest::matchers::eq(*self), actual)
    }
}

impl Expected<String, Literal> for String {
    fn holds(&self, actual: &String) -> bool {
        actual == self
    }
    fn describe(&self, holds: bool) -> String {
        self.as_str().describe(holds)
    }
    fn explain(&self, actual: &String) -> String {
        self.as_str().explain(actual)
    }
}

macro_rules! literal {
    ($($value:ty),+) => {$(
        impl Expected<$value, Literal> for $value {
            fn holds(&self, actual: &$value) -> bool {
                actual == self
            }
            fn describe(&self, holds: bool) -> String {
                Expected::<$value, ByValue>::describe(&googletest::matchers::eq(*self), holds)
            }
            fn explain(&self, actual: &$value) -> String {
                Expected::<$value, ByValue>::explain(&googletest::matchers::eq(*self), actual)
            }
        }
    )+};
}

literal!(f32, f64, usize, bool);

/// A matcher's account of a mismatch, kept only when it says
/// more than "which isn't …": a diff of two texts, say.
fn worth_keeping(explanation: String) -> Option<String> {
    explanation.contains('\n').then_some(explanation)
}

/// What one look at the widgets came to.
struct Look {
    /// `None` when it cannot be judged, which never passes, negated or not:
    /// no single widget matched, or the widget has no such state.
    verdict: Option<bool>,
    actual: Option<String>,
    explanation: Option<String>,
}

impl Look {
    fn unjudged(actual: impl Into<String>) -> Self {
        Self {
            verdict: None,
            actual: Some(actual.into()),
            explanation: None,
        }
    }
}

/// The one widget a property matcher reads, or why there isn't one.
fn single(found: &[Found]) -> Result<&Found, Look> {
    match found {
        [one] => Ok(one),
        [] => Err(Look {
            verdict: None,
            actual: None,
            explanation: None,
        }),
        many => Err(Look::unjudged(format!(
            "{} widgets match; narrow it with a role or an exact label",
            many.len()
        ))),
    }
}

fn widgets(count: usize) -> String {
    match count {
        1 => "1 widget matches".to_owned(),
        _ => format!("{count} widgets match"),
    }
}

/// An expectation about the widget a [`By`] matches,
/// checked by one of its matchers.
///
/// Every matcher draws frames until it holds, within the play's wait budget,
/// so none needs a [`wait_for`](Play::wait_for) around it.
#[must_use = "an expectation checks nothing until one of its matchers is called"]
pub struct Expect<'p, 'd> {
    play: &'p mut Play<'d>,
    by: By,
    negated: bool,
    message: Option<String>,
    within: Option<u32>,
}

impl<'d> Play<'d> {
    /// Expect something of the widget `by` matches:
    /// `p.expect("Name").to_have_value("Ada")?`.
    pub fn expect(&mut self, by: impl Into<By>) -> Expect<'_, 'd> {
        Expect {
            play: self,
            by: by.into(),
            negated: false,
            message: None,
            within: None,
        }
    }

    /// Draw frames until `poll` returns a value `expected` accepts,
    /// within the wait budget:
    ///     `p.expect_value(|p| p.get("Apple").rect.top(), lt(zebra))?`.
    ///
    /// A panic in `poll`, such as a [`get`](Self::get) that missed, counts as not yet.
    ///
    /// # Errors
    /// When no value it returned was accepted; the error gives the last one.
    #[track_caller]
    pub fn expect_value<T: Debug, K>(
        &mut self,
        mut poll: impl FnMut(&Self) -> T,
        expected: impl Expected<T, K>,
    ) -> PlayResult {
        let at = Location::caller();
        let start = self.frame;
        let mut waited = 0;
        loop {
            let (holds, actual, explanation) = match caught(|| poll(&*self)) {
                Ok(value) => {
                    let holds = expected.holds(&value);
                    let explanation = (!holds)
                        .then(|| worth_keeping(expected.explain(&value)))
                        .flatten();
                    (holds, format!("{value:?}"), explanation)
                }
                Err(error) => (false, error.reason().to_owned(), None),
            };
            if holds || waited >= self.wait_frames {
                let check = Check {
                    subject: "value".to_owned(),
                    matcher: "expect_value",
                    negated: false,
                    expected: expected.describe(true),
                    actual: Some(actual),
                    explanation,
                    message: None,
                };
                return self.checked(check, start, at, holds, false);
            }
            self.frame(Vec::new()).map_err(|error| error.located(at))?;
            waited += 1;
        }
    }

    /// Draw frames until the scene has reported, through [`action`](crate::action),
    /// an action `expected` accepts: `p.expect_action("saved")?`.
    ///
    /// # Errors
    /// When it never did within the wait budget;
    /// the error lists what it did report.
    #[track_caller]
    pub fn expect_action<K>(&mut self, expected: impl Expected<String, K>) -> PlayResult {
        let at = Location::caller();
        let start = self.frame;
        let mut waited = 0;
        loop {
            let holds = self.actions.iter().any(|action| expected.holds(action));
            if holds || waited >= self.wait_frames {
                let check = Check {
                    subject: "actions".to_owned(),
                    matcher: "expect_action",
                    negated: false,
                    expected: format!("include one that {}", expected.describe(true)),
                    actual: Some(format!("{:?}", self.actions)),
                    explanation: None,
                    message: None,
                };
                return self.checked(check, start, at, holds, false);
            }
            self.frame(Vec::new()).map_err(|error| error.located(at))?;
            waited += 1;
        }
    }
}

impl Expect<'_, '_> {
    /// Expect the opposite: `p.expect("Saving…").not().to_exist()`.
    #[expect(
        clippy::should_implement_trait,
        reason = "read in the chain as Playwright's `.not`; `!` would have to wrap the whole expectation"
    )]
    pub fn not(mut self) -> Self {
        self.negated = !self.negated;
        self
    }

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

    /// Exactly one widget matches.
    ///
    /// # Errors
    /// When none or several still do once the wait budget is spent.
    #[track_caller]
    pub fn to_exist(self) -> PlayResult {
        let at = Location::caller();
        self.settle(at, "to_exist", ("exists", "doesn't exist"), |found| Look {
            verdict: match found.len() {
                0 => Some(false),
                1 => Some(true),
                _ => None,
            },
            actual: (!found.is_empty()).then(|| widgets(found.len())),
            explanation: None,
        })
    }

    /// No widget matches.
    ///
    /// # Errors
    /// When one still does once the wait budget is spent.
    #[track_caller]
    pub fn to_be_absent(self) -> PlayResult {
        let at = Location::caller();
        self.settle(at, "to_be_absent", ("is absent", "isn't absent"), |found| {
            Look {
                verdict: Some(found.is_empty()),
                actual: (!found.is_empty()).then(|| widgets(found.len())),
                explanation: None,
            }
        })
    }

    /// As many widgets match as `expected` accepts:
    /// `to_have_count(3)`, `to_have_count(gt(0))`.
    ///
    /// # Errors
    /// When the count is still not accepted once
    /// the wait budget is spent.
    #[track_caller]
    pub fn to_have_count<K>(self, expected: impl Expected<usize, K>) -> PlayResult {
        let at = Location::caller();
        let phrase = |holds| format!("count {}", expected.describe(holds));
        let phrases = (phrase(true), phrase(false));
        self.settle(at, "to_have_count", phrases, |found| Look {
            verdict: Some(expected.holds(&found.len())),
            actual: Some(widgets(found.len())),
            explanation: None,
        })
    }

    /// Its value, a text field's contents for one,
    /// is accepted by `expected`.
    ///
    /// # Errors
    /// When it still isn't once the wait budget is spent.
    #[track_caller]
    pub fn to_have_value<K>(self, expected: impl Expected<String, K>) -> PlayResult {
        let at = Location::caller();
        self.text(
            at,
            "to_have_value",
            "value",
            |found| found.value.clone(),
            expected,
        )
    }

    /// Its accessible name, a text label's text for one,
    /// is accepted by `expected`.
    ///
    /// # Errors
    /// When it still isn't once the wait budget is spent.
    #[track_caller]
    pub fn to_have_label<K>(self, expected: impl Expected<String, K>) -> PlayResult {
        let at = Location::caller();
        self.text(
            at,
            "to_have_label",
            "label",
            |found| found.label.clone(),
            expected,
        )
    }

    /// Its accessible description, a widget's tooltip
    /// for one, is accepted by `expected`.
    ///
    /// # Errors
    /// When it still isn't once the wait budget is spent.
    #[track_caller]
    pub fn to_have_description<K>(self, expected: impl Expected<String, K>) -> PlayResult {
        let at = Location::caller();
        self.text(
            at,
            "to_have_description",
            "description",
            |found| found.description.clone(),
            expected,
        )
    }

    /// The text an empty field shows in its place
    /// is accepted by `expected`.
    ///
    /// # Errors
    /// When it still isn't once the wait budget is spent.
    #[track_caller]
    pub fn to_have_placeholder<K>(self, expected: impl Expected<String, K>) -> PlayResult {
        let at = Location::caller();
        self.text(
            at,
            "to_have_placeholder",
            "placeholder",
            |found| found.placeholder.clone(),
            expected,
        )
    }

    /// Its number, a slider's or a drag value's,
    /// is accepted by `expected`.
    ///
    /// # Errors
    /// When it still isn't once the wait budget
    /// is spent, or the widget has no number.
    #[track_caller]
    pub fn to_have_numeric_value<K>(self, expected: impl Expected<f64, K>) -> PlayResult {
        let at = Location::caller();
        let phrase = |holds| format!("number {}", expected.describe(holds));
        let phrases = (phrase(true), phrase(false));
        self.settle(at, "to_have_numeric_value", phrases, |found| {
            let one = match single(found) {
                Ok(one) => one,
                Err(look) => return look,
            };
            let Some(number) = one.numeric.map(|numeric| numeric.value) else {
                return Look::unjudged("no number");
            };
            let holds = expected.holds(&number);
            Look {
                verdict: Some(holds),
                actual: Some(format!("{number:?}")),
                explanation: (!holds)
                    .then(|| worth_keeping(expected.explain(&number)))
                    .flatten(),
            }
        })
    }

    /// It has keyboard focus.
    ///
    /// # Errors
    /// When it still hasn't once the wait budget is spent.
    #[track_caller]
    pub fn to_be_focused(self) -> PlayResult {
        let at = Location::caller();
        self.state(
            at,
            "to_be_focused",
            ("is focused", "isn't focused"),
            |found| {
                Ok((
                    found.focused,
                    if found.focused {
                        "focused"
                    } else {
                        "not focused"
                    },
                ))
            },
        )
    }

    /// It can't be used.
    ///
    /// # Errors
    /// When it still can once the wait budget is spent.
    #[track_caller]
    pub fn to_be_disabled(self) -> PlayResult {
        let at = Location::caller();
        self.state(
            at,
            "to_be_disabled",
            ("is disabled", "isn't disabled"),
            enabled_or_not,
        )
    }

    /// It can be used.
    ///
    /// # Errors
    /// When it still can't once the wait budget is spent.
    #[track_caller]
    pub fn to_be_enabled(self) -> PlayResult {
        let at = Location::caller();
        self.state(
            at,
            "to_be_enabled",
            ("is enabled", "isn't enabled"),
            |found| enabled_or_not(found).map(|(disabled, shown)| (!disabled, shown)),
        )
    }

    /// It is a checkbox, toggle or radio that is on.
    ///
    /// # Errors
    /// When it still isn't once the wait budget
    /// is spent, or it can't be checked at all.
    #[track_caller]
    pub fn to_be_checked(self) -> PlayResult {
        let at = Location::caller();
        self.state(
            at,
            "to_be_checked",
            ("is checked", "isn't checked"),
            |found| toggle(found).map(|(state, shown)| (state == Toggled::True, shown)),
        )
    }

    /// It is a checkbox that is neither on nor off:
    /// some of what it stands for is checked.
    ///
    /// # Errors
    /// When it still isn't once the wait budget
    /// is spent, or it can't be checked at all.
    #[track_caller]
    pub fn to_be_partially_checked(self) -> PlayResult {
        let at = Location::caller();
        let phrases = ("is partially checked", "isn't partially checked");
        self.state(at, "to_be_partially_checked", phrases, |found| {
            toggle(found).map(|(state, shown)| (state == Toggled::Mixed, shown))
        })
    }

    /// It is the selected one of its kind: a tab,
    /// a list row, a selectable label.
    ///
    /// # Errors
    /// When it still isn't once the wait budget
    /// is spent, or it can't be selected at all.
    #[track_caller]
    pub fn to_be_selected(self) -> PlayResult {
        let at = Location::caller();
        self.state(
            at,
            "to_be_selected",
            ("is selected", "isn't selected"),
            |found| {
                // egui marks a selected selectable
                // label as toggled, never as selected.
                let selected = found
                    .selected
                    .or_else(|| found.toggled.map(|state| state == Toggled::True));
                match selected {
                    Some(true) => Ok((true, "selected")),
                    Some(false) => Ok((false, "not selected")),
                    None => Err("can't be selected"),
                }
            },
        )
    }

    /// It is open: a collapsing header,
    /// a combo box showing its list.
    ///
    /// # Errors
    /// When it still isn't once the wait budget
    /// is spent, or it can't be expanded at all.
    #[track_caller]
    pub fn to_be_expanded(self) -> PlayResult {
        let at = Location::caller();
        self.state(
            at,
            "to_be_expanded",
            ("is expanded", "isn't expanded"),
            |found| match found.expanded {
                Some(true) => Ok((true, "expanded")),
                Some(false) => Ok((false, "collapsed")),
                None => Err("can't be expanded"),
            },
        )
    }

    /// It is in the tree, not hidden,
    /// with a place on screen.
    ///
    /// # Errors
    /// When it still isn't once the wait budget is spent.
    #[track_caller]
    pub fn to_be_visible(self) -> PlayResult {
        let at = Location::caller();
        self.state(
            at,
            "to_be_visible",
            ("is visible", "isn't visible"),
            |found| {
                let shown = visible(found);
                Ok((shown, if shown { "visible" } else { "hidden" }))
            },
        )
    }

    /// It is hidden, has no place on screen, or isn't there at all.
    ///
    /// # Errors
    /// When it is still visible once the wait budget is spent.
    #[track_caller]
    pub fn to_be_hidden(self) -> PlayResult {
        let at = Location::caller();
        self.settle(at, "to_be_hidden", ("is hidden", "isn't hidden"), |found| {
            match single(found) {
                Ok(one) => {
                    let shown = visible(one);
                    Look {
                        verdict: Some(!shown),
                        actual: Some((if shown { "visible" } else { "hidden" }).to_owned()),
                        explanation: None,
                    }
                }
                // Gone from the tree is as hidden as it gets.
                Err(_) if found.is_empty() => Look {
                    verdict: Some(true),
                    actual: None,
                    explanation: None,
                },
                Err(look) => look,
            }
        })
    }

    /// A matcher reading one text property of the single widget matched.
    fn text<K>(
        self,
        at: &'static Location<'static>,
        matcher: &'static str,
        property: &'static str,
        read: impl Fn(&Found) -> Option<String>,
        expected: impl Expected<String, K>,
    ) -> PlayResult {
        let phrase = |holds| format!("{property} {}", expected.describe(holds));
        let phrases = (phrase(true), phrase(false));
        self.settle(at, matcher, phrases, |found| {
            let one = match single(found) {
                Ok(one) => one,
                Err(look) => return look,
            };
            let value = read(one).unwrap_or_default();
            let holds = expected.holds(&value);
            Look {
                verdict: Some(holds),
                actual: Some(format!("{value:?}")),
                explanation: (!holds)
                    .then(|| worth_keeping(expected.explain(&value)))
                    .flatten(),
            }
        })
    }

    /// A matcher reading one yes-or-no state of the single widget matched;
    /// `read` gives the state and how to show it, or why the widget has none.
    fn state<P: Into<String>>(
        self,
        at: &'static Location<'static>,
        matcher: &'static str,
        phrases: (P, P),
        read: impl Fn(&Found) -> Result<(bool, &'static str), &'static str>,
    ) -> PlayResult {
        self.settle(at, matcher, phrases, |found| match single(found) {
            Ok(one) => match read(one) {
                Ok((state, shown)) => Look {
                    verdict: Some(state),
                    actual: Some(shown.to_owned()),
                    explanation: None,
                },
                Err(why) => Look::unjudged(why),
            },
            Err(look) => look,
        })
    }

    /// Look at what `by` matches each frame until the verdict is the one expected,
    /// or the budget is spent, and log the check either way.
    fn settle<P: Into<String>>(
        self,
        at: &'static Location<'static>,
        matcher: &'static str,
        (holds, fails): (P, P),
        look: impl Fn(&[Found]) -> Look,
    ) -> PlayResult {
        let Self {
            play,
            by,
            negated,
            message,
            within,
        } = self;
        let budget = within.unwrap_or(play.wait_frames);
        let start = play.frame;
        let mut waited = 0;
        loop {
            let found = play.driver.query(&by);
            let seen = look(&found);
            let passed = seen.verdict.is_some_and(|verdict| verdict != negated);
            if passed || waited >= budget {
                let check = Check {
                    subject: by.to_string(),
                    matcher,
                    negated,
                    expected: if negated { fails.into() } else { holds.into() },
                    actual: seen.actual,
                    explanation: seen.explanation,
                    message,
                };
                return play.checked(check, start, at, passed, found.is_empty());
            }
            play.frame(Vec::new()).map_err(|error| error.located(at))?;
            waited += 1;
        }
    }
}

fn enabled_or_not(found: &Found) -> Result<(bool, &'static str), &'static str> {
    Ok((
        found.disabled,
        if found.disabled {
            "disabled"
        } else {
            "enabled"
        },
    ))
}

fn toggle(found: &Found) -> Result<(Toggled, &'static str), &'static str> {
    match found.toggled {
        Some(Toggled::True) => Ok((Toggled::True, "checked")),
        Some(Toggled::False) => Ok((Toggled::False, "unchecked")),
        Some(Toggled::Mixed) => Ok((Toggled::Mixed, "partially checked")),
        None => Err("can't be checked"),
    }
}

fn visible(found: &Found) -> bool {
    !found.hidden && found.rect.is_finite() && found.rect.is_positive()
}
