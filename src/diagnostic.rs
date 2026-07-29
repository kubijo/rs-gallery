//! A failure the caller has to act on.
//!
//! The fix is usually one of a handful of names the program already knows, so a [`Diagnostic`] keeps
//! them as a list rather than flattening them into a sentence. Styling goes through `anstream` and so
//! disappears for a pipe or `NO_COLOR`; the frame is not styling and stays, which is what keeps a
//! twenty-scene listing legible in a CI log.

use std::io::Write as _;

use anstyle::{AnsiColor, Style};

use crate::style::{frame, paint};

/// What went wrong, which names were in play, and what to do about it.
pub(crate) struct Diagnostic {
    severity: Severity,
    headline: String,
    /// Names a pattern matched, or the ones it could have matched instead.
    candidates: Vec<String>,
    hint: Option<String>,
}

/// Whether the run stops here. A warning reports and carries on,
/// so it says what it did instead of what it wanted.
enum Severity {
    Error,
    Warning,
}

impl Diagnostic {
    pub(crate) fn new(headline: impl Into<String>) -> Self {
        Self::at(Severity::Error, headline)
    }

    pub(crate) fn warning(headline: impl Into<String>) -> Self {
        Self::at(Severity::Warning, headline)
    }

    fn at(severity: Severity, headline: impl Into<String>) -> Self {
        Self {
            severity,
            headline: headline.into(),
            candidates: Vec::new(),
            hint: None,
        }
    }

    /// The names the caller has to choose between.
    pub(crate) fn candidates(mut self, names: impl IntoIterator<Item = String>) -> Self {
        self.candidates = names.into_iter().collect();
        self
    }

    /// What to do next, in the imperative — it closes the frame, so a listing should have one.
    pub(crate) fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// The whole report, escapes and all, for `anstream` to strip as the environment demands.
    fn render(&self) -> String {
        let (word, colour) = match self.severity {
            Severity::Error => ("error", AnsiColor::Red),
            Severity::Warning => ("warning", AnsiColor::Yellow),
        };
        let bad = Style::new().bold().fg_color(Some(colour.into()));
        let help = Style::new().bold().fg_color(Some(AnsiColor::Cyan.into()));

        let headline = format!("{}: {}", paint(bad, word), self.headline);
        // With no candidates between them the two labels sit one above the other,
        // so they line up on their colons — near-alignment reads as a missed indent.
        let lead = match self.candidates.is_empty() {
            true => " ".repeat(word.len().saturating_sub("help".len())),
            false => String::new(),
        };
        let hint = self
            .hint
            .as_ref()
            .map(|hint| format!("{lead}{}: {hint}", paint(help, "help")));
        frame(&headline, &self.candidates, hint.as_deref())
    }

    /// Report to stderr, opening on a blank line: this lands under whatever cargo was last saying,
    /// and wants separating from it. Nothing closes it — whatever comes next brings its own space.
    /// Callers exit afterwards; this only writes.
    ///
    /// The margin is presentation, so it stays out of [`render`](Self::render):
    /// what tests assert on is the block itself.
    pub(crate) fn report(&self) {
        // stdout goes first: this lands on stderr, and anything still held in stdout's buffer would
        // otherwise surface after the report that refers to it.
        let _ = std::io::stdout().flush();
        anstream::eprintln!("\n{}", self.render());
    }

    /// The report as a pipe or a CI log sees it — `anstream` strips the escapes there,
    /// so this is what assertions elsewhere in the crate should read.
    pub(crate) fn plain(&self) -> String {
        crate::style::plain(&self.render())
    }
}

/// The report itself, so a test that unwraps one is told what went wrong rather than shown a struct.
impl std::fmt::Debug for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.plain())
    }
}

/// So a failure with nothing to list stays a one-liner at the call site.
impl From<String> for Diagnostic {
    fn from(headline: String) -> Self {
        Self::new(headline)
    }
}

#[cfg(test)]
mod tests {
    use indoc::indoc;

    use super::*;

    #[test]
    fn a_bare_diagnostic_is_one_line() {
        assert_eq!(Diagnostic::new("it broke").plain(), "error: it broke");
    }

    #[test]
    fn candidates_are_framed_under_their_headline_and_closed_by_the_hint() {
        let diagnostic = Diagnostic::new("`knobs` matches 2 scenes")
            .candidates(["one".to_owned(), "two".to_owned()])
            .hint("narrow the pattern");
        assert_eq!(
            diagnostic.plain(),
            indoc! {"
                error: `knobs` matches 2 scenes
                  ├─  one
                  ├─  two
                  ╰─  help: narrow the pattern"}
        );
    }

    #[test]
    fn a_hint_with_nothing_between_it_and_the_headline_lines_up_on_the_colon() {
        let colon = |line: &str| line.find(':').expect("a labelled line");
        for report in [
            Diagnostic::new("it broke").hint("try the other one"),
            Diagnostic::warning("it was skipped").hint("try the other one"),
        ] {
            let plain = report.plain();
            let (headline, hint) = plain.split_once('\n').expect("a headline and a hint");
            assert_eq!(colon(headline), colon(hint), "the labels line up:\n{plain}");
        }
    }

    #[test]
    fn styling_is_only_ever_escapes_so_stripping_it_loses_no_words() {
        let diagnostic = Diagnostic::new("headline")
            .candidates(["name".to_owned()])
            .hint("do the thing");
        let styled = diagnostic.render();
        assert!(styled.contains('\u{1b}'), "the styled form has escapes");
        assert!(
            styled.contains("\u{1b}[0m"),
            "every style is closed again — without a reset the colour bleeds down the line"
        );
        for word in [
            "error",
            "headline",
            "name",
            "help",
            "do the thing",
            "├─",
            "╰─",
        ] {
            assert!(
                diagnostic.plain().contains(word),
                "`{word}` survives stripping"
            );
        }
    }
}
