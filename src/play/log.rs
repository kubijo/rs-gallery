//! A play's log: its steps, gestures and checks,
//! in the order they began.
//!
//! Plain data only, so a play's log can cross
//! from the scenes dylib to the host that reports it.

use std::ops::Range;

use camino::{Utf8Path, Utf8PathBuf};

use super::SourceAt;

/// One thing a play did.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct Entry {
    /// How many steps were open around it.
    pub(crate) depth: usize,
    /// The play's frames it drew: `[first, end)`.
    pub(crate) frames: Range<u32>,
    pub(crate) at: Option<SourceAt>,
    /// `None` while it runs.
    pub(crate) outcome: Option<Outcome>,
    /// The screenshot taken as a step ended, when the trace was kept.
    pub(crate) trace: Option<Utf8PathBuf>,
    pub(crate) what: What,
}

#[derive(Clone, PartialEq, Debug)]
pub(crate) enum What {
    /// `path` is the label with every step around it: `outer › inner`.
    Step {
        label: String,
        path: String,
    },
    Gesture {
        label: String,
    },
    Check(Box<Check>),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Outcome {
    Passed,
    Failed,
    /// A snapshot check in a capture, which compares none.
    Skipped,
}

impl From<bool> for Outcome {
    fn from(passed: bool) -> Self {
        if passed { Self::Passed } else { Self::Failed }
    }
}

/// A check's verdict, phrased as googletest's matchers phrase theirs.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Check {
    /// What was looked at: a widget as its [`By`](super::By) reads, `actions` or `value`.
    pub(crate) subject: String,
    /// The method that checked it: `to_have_value`.
    pub(crate) matcher: &'static str,
    pub(crate) negated: bool,
    /// What should hold, as a verb phrase: `value is equal to "Ada"`.
    pub(crate) expected: String,
    /// What was there when it was last looked at; `None` when nothing matched.
    pub(crate) actual: Option<String>,
    /// A matcher's account of the difference, when it says more than `expected` does.
    pub(crate) explanation: Option<String>,
    /// The play's own words for it, from `because`.
    pub(crate) message: Option<String>,
    /// What a snapshot check left behind.
    pub(crate) kept: Option<Kept>,
}

/// What a snapshot check left behind.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub(crate) struct Kept {
    pub(crate) acceptance: Acceptance,
    /// Files to look at: the reference, what was seen and where they differ.
    pub(crate) attachments: Vec<Attachment>,
    /// The lines that differ, of a text.
    pub(crate) diff: Option<String>,
}

/// What became of what a snapshot check saw, as a reference to be.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub(crate) enum Acceptance {
    #[default]
    None,
    /// An update run would take it.
    Offered,
    /// This run took it.
    Taken,
}

/// A file a check left behind, written host-side.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Attachment {
    pub(crate) kind: AttachmentKind,
    pub(crate) path: Utf8PathBuf,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum AttachmentKind {
    /// What the check compared against.
    Reference,
    /// What the check saw.
    Actual,
    /// Where the two differ.
    Diff,
}

impl AttachmentKind {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Reference => "reference",
            Self::Actual => "actual",
            Self::Diff => "diff",
        }
    }
}

impl Entry {
    /// The label a step goes by in the report and its screenshot's name;
    /// `None` unless the report counts it as a step: a step, or a gesture run outside one.
    pub(crate) fn step_path(&self) -> Option<&str> {
        match &self.what {
            What::Step { path, .. } => Some(path),
            What::Gesture { label } if self.depth == 0 => Some(label),
            _ => None,
        }
    }

    /// The reference its check took this run.
    pub(crate) fn taken(&self) -> Option<&Utf8Path> {
        let What::Check(check) = &self.what else {
            return None;
        };
        let kept = check.kept.as_ref()?;
        if kept.acceptance != Acceptance::Taken {
            return None;
        }
        kept.attachments
            .iter()
            .find(|attachment| attachment.kind == AttachmentKind::Reference)
            .map(|attachment| attachment.path.as_path())
    }

    pub(crate) fn failed(&self) -> bool {
        self.outcome == Some(Outcome::Failed)
    }
}

impl Check {
    /// The check in one line: what should have held, and what was there instead.
    pub(crate) fn summary(&self) -> String {
        if let Some(message) = &self.message {
            return message.clone();
        }
        let actual = self.actual.as_deref().unwrap_or("nothing matched");
        format!("expected {} {}, got {actual}", self.subject, self.expected)
    }
}
