//! A failure the caller has to act on, and how it reaches them.
//!
//! A headless run fails on things a person then has to fix, and the fix is usually one of a handful
//! of names the program already knows. So a [`Diagnostic`] carries those names as a list rather than
//! flattening them into a sentence, and prints them framed under the headline they belong to.
//!
//! Styling goes through `anstream`, which drops it for a pipe, for `NO_COLOR`, or for a terminal that
//! cannot take escapes. The frame is not styling and always survives — it is what keeps a twenty-scene
//! listing legible in a CI log.

use std::fmt::Write as _;

use anstyle::{AnsiColor, Style};

/// What went wrong, which names were in play, and what to do about it.
pub(crate) struct Diagnostic {
    headline: String,
    /// Names a pattern matched, or the ones it could have matched instead.
    candidates: Vec<String>,
    hint: Option<String>,
}

impl Diagnostic {
    pub(crate) fn new(headline: impl Into<String>) -> Self {
        Self {
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
        let bad = Style::new().bold().fg_color(Some(AnsiColor::Red.into()));
        let frame = Style::new().dimmed();
        let help = Style::new().bold().fg_color(Some(AnsiColor::Cyan.into()));

        let mut out = format!("{}: {}", paint(bad, "error"), self.headline);
        if !self.candidates.is_empty() {
            let _ = write!(out, "\n{}", paint(frame, "  │"));
            for name in &self.candidates {
                let _ = write!(out, "\n{}   {name}", paint(frame, "  │"));
            }
        }
        if let Some(hint) = &self.hint {
            let corner = if self.candidates.is_empty() {
                "  "
            } else {
                "  ╰─ "
            };
            let _ = write!(
                out,
                "\n{}{}: {hint}",
                paint(frame, corner),
                paint(help, "help")
            );
        }
        out
    }

    /// Report to stderr, with a blank line either side: this lands under whatever cargo was last
    /// saying, and wants separating from it. Callers exit afterwards; this only writes.
    ///
    /// The margin is presentation, so it stays out of [`render`](Self::render):
    /// what tests assert on is the block itself.
    pub(crate) fn report(&self) {
        anstream::eprintln!("\n{}\n", self.render());
    }

    /// The report as a pipe or a CI log sees it — `anstream` strips the escapes there,
    /// so this is what assertions elsewhere in the crate should read.
    pub(crate) fn plain(&self) -> String {
        let rendered = self.render();
        String::from_utf8(anstream::adapter::strip_bytes(rendered.as_bytes()).into_vec())
            .expect("stripping escapes keeps it UTF-8")
    }
}

/// The report itself, so a test that unwraps one is told what went wrong rather than shown a struct.
impl std::fmt::Debug for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.plain())
    }
}

/// `text` in `style`, closed again after it.
///
/// Both halves come from the style itself: writing the opening sequence
/// and expecting the formatter's `{:#}` to close it silently re-opens instead,
/// leaving the colour to bleed down the rest of the line.
fn paint(style: Style, text: &str) -> String {
    format!("{}{text}{}", style.render(), style.render_reset())
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
                  │
                  │   one
                  │   two
                  ╰─ help: narrow the pattern"}
        );
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
            "│",
            "╰─",
        ] {
            assert!(
                diagnostic.plain().contains(word),
                "`{word}` survives stripping"
            );
        }
    }
}
