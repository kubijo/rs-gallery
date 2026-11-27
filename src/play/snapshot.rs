//! What every snapshot check shares, of an image or of text:
//! what it asks the host, what the host answers, and the waiting in between.

use std::panic::Location;

use super::log::{Acceptance, Check, Kept, Outcome};
use super::{By, Play, PlayResult};

/// What a play asks the host to compare with a reference.
pub(crate) struct Ask<'a> {
    /// Where the play is written; its references sit beside it.
    pub(crate) file: &'a str,
    pub(crate) module_path: &'a str,
    pub(crate) play: &'a str,
    pub(crate) name: &'a str,
    pub(crate) of: Of<'a>,
    /// Why nothing can be compared, asked this way.
    pub(crate) misused: Option<&'a str>,
    /// The check's first look.
    pub(crate) first: bool,
}

#[derive(Clone, Copy)]
pub(crate) enum Of<'a> {
    Screenshot {
        tolerance: Option<u32>,
        threshold: Option<f32>,
    },
    /// The accessibility tree of the widget `by` matches, or of the canvas.
    Tree { by: Option<&'a By> },
}

/// One look at what the check compares, against its reference.
pub(crate) struct Look {
    pub(crate) verdict: Verdict,
    /// The verdict stands: more frames wouldn't change it.
    pub(crate) done: bool,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum Verdict {
    /// The run compares no snapshots.
    NotCompared,
    /// Every pixel alike, or the same tree.
    Identical,
    /// Not identical, but within the budget.
    Alike {
        differing: u32,
        allowed: u32,
    },
    Differs {
        differing: u32,
        allowed: u32,
    },
    /// So many lines of the text differ.
    Changed {
        removed: usize,
        added: usize,
    },
    Missing,
    Unreadable(String),
    Resized {
        reference: [u32; 2],
        actual: [u32; 2],
    },
    /// The content does not fit the canvas, which grows to hold it.
    Outgrown,
    Unmatched(Unmatched),
}

/// Why there is no one widget to compare the tree of.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum Unmatched {
    Nothing,
    Several(usize),
    Hidden,
    /// The tree can't be read this frame.
    Unread(String),
}

impl Verdict {
    pub(crate) fn passes(&self) -> bool {
        matches!(self, Self::Identical | Self::Alike { .. })
    }

    /// What the check found, `None` when nothing matched.
    pub(crate) fn actual(&self) -> Option<String> {
        Some(match self {
            Self::NotCompared => "not compared in a capture".to_owned(),
            Self::Identical => "identical".to_owned(),
            Self::Alike { differing: 0, .. } => "no pixel differs past the threshold".to_owned(),
            Self::Alike { differing, allowed } | Self::Differs { differing, allowed } => {
                format!("{differing} pixels differ, {allowed} allowed")
            }
            Self::Changed { removed, added } => {
                format!("{} removed, {added} added", lines(*removed))
            }
            Self::Missing => "no reference yet".to_owned(),
            Self::Unreadable(why) => format!("the reference can't be read: {why}"),
            Self::Resized {
                reference: [rw, rh],
                actual: [aw, ah],
            } => format!("{aw}×{ah}, the reference {rw}×{rh}"),
            Self::Outgrown => "the content outgrows its canvas".to_owned(),
            Self::Unmatched(Unmatched::Nothing) => return None,
            Self::Unmatched(Unmatched::Several(many)) => {
                format!("{many} widgets match; narrow it with a role or an exact label")
            }
            Self::Unmatched(Unmatched::Hidden) => "hidden".to_owned(),
            Self::Unmatched(Unmatched::Unread(why)) => why.clone(),
        })
    }
}

fn lines(count: usize) -> String {
    match count {
        1 => "1 line".to_owned(),
        _ => format!("{count} lines"),
    }
}

/// A snapshot check as a play words it.
pub(super) struct Asked {
    /// What is looked at, as the log names it.
    pub(super) subject: String,
    pub(super) matcher: &'static str,
    /// What it should match, as its failure reads: `AccessKit snapshot`.
    pub(super) kind: &'static str,
    pub(super) name: String,
    /// Why nothing can be compared, asked this way.
    pub(super) misused: Option<String>,
    pub(super) message: Option<String>,
    pub(super) within: Option<u32>,
}

impl Asked {
    /// Look until the check passes, what it looks at settles, or the budget is spent.
    pub(super) fn run(
        self,
        play: &mut Play<'_>,
        of: Of<'_>,
        at: &'static Location<'static>,
    ) -> PlayResult {
        let start = play.frame;
        let budget = self.within.unwrap_or(play.wait_frames);
        let mut waited = 0;
        let look = loop {
            let ask = Ask {
                file: play.file,
                module_path: play.module_path,
                play: play.name,
                name: &self.name,
                of,
                misused: self.misused.as_deref(),
                first: waited == 0,
            };
            match play.driver.compare(&ask) {
                Ok(look) if look.done || waited >= budget => break look,
                Ok(_) => {}
                Err(reason) => return self.refused(play, reason, start, at),
            }
            play.frame(Vec::new()).map_err(|error| error.located(at))?;
            waited += 1;
        };
        let mut actual = look.verdict.actual();
        if !look.done && !matches!(look.verdict, Verdict::Unmatched(_)) {
            actual = actual.map(|found| format!("{found}; not settled within {waited} frames"));
        }
        let kept = match play.driver.keep() {
            Ok(kept) => kept,
            Err(reason) => {
                let found = actual.unwrap_or_default();
                return self.refused(play, format!("{found}; {reason}"), start, at);
            }
        };
        let taken = kept.acceptance == Acceptance::Taken;
        if taken {
            actual = actual.map(|found| format!("{found}; taken as the reference"));
        }
        let outcome = match look.verdict {
            Verdict::NotCompared => Outcome::Skipped,
            Verdict::Alike { differing, .. } if differing > 0 && !taken => Outcome::Tolerated,
            ref verdict => (verdict.passes() || taken).into(),
        };
        let nothing_matched = look.verdict == Verdict::Unmatched(Unmatched::Nothing);
        let check = self.check(actual, kept);
        play.checked(check, start, at, outcome, nothing_matched)
    }

    /// Fail the check for `reason` before anything
    /// was compared, or with nothing kept of it.
    fn refused(
        self,
        play: &mut Play<'_>,
        reason: String,
        start: u32,
        at: &'static Location<'static>,
    ) -> PlayResult {
        let check = self.check(Some(reason), Kept::default());
        play.checked(check, start, at, false, false)
    }

    fn check(self, actual: Option<String>, kept: Kept) -> Check {
        Check {
            subject: self.subject,
            matcher: self.matcher,
            negated: false,
            expected: format!("matches {} {:?}", self.kind, self.name),
            actual,
            explanation: None,
            message: self.message,
            kept: Some(kept),
        }
    }
}
