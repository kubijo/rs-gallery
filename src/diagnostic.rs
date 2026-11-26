//! A failure the caller has to act on.
//!
//! The fix is usually one of a handful of names the program already knows, so a [`Diagnostic`] keeps
//! them as a list rather than flattening them into a sentence.
//! miette lays the report out, as the stream it is bound for can show it.

mod code;
pub(crate) mod diff;

use std::{
    fmt,
    io::{IsTerminal as _, Write as _},
};

use anstyle::{AnsiColor, Style};
use miette::{
    GraphicalReportHandler, GraphicalTheme, LabeledSpan, MietteHandlerOpts, NamedSource,
    ReportHandler,
};
use textwrap::{WordSeparator, WordSplitter};

pub(crate) use self::code::Highlighted;
use self::code::{Code, Frame};
#[cfg(test)]
pub(crate) use self::code::{Kind, Token};
use crate::style::{Stream, paint};

/// What went wrong, why, which names were in play, and what to do about it.
pub(crate) struct Diagnostic {
    severity: Severity,
    headline: String,
    /// Why it happened, beneath the headline, each reason a line of its own.
    causes: Vec<String>,
    /// Names a pattern matched, or the ones it could have matched instead.
    candidates: Vec<String>,
    /// The source line to show, read when the report is drawn.
    source: Option<Box<SourceLine>>,
    hints: Vec<Hint>,
}

struct Hint {
    text: String,
    /// It says where a play is recorded.
    of_recording: bool,
}

/// A line of a source file, and what to say beneath it.
struct SourceLine {
    file: String,
    line: u32,
    column: u32,
    label: String,
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
            causes: Vec::new(),
            candidates: Vec::new(),
            source: None,
            hints: Vec::new(),
        }
    }

    /// Why it happened, shown beneath the headline; each call adds a line.
    pub(crate) fn cause(self, cause: impl Into<String>) -> Self {
        let cause = highlight(&cause.into());
        self.cause_as_is(cause)
    }

    /// A [`cause`](Self::cause) whose backticks are its own, not around names.
    pub(crate) fn cause_as_is(mut self, cause: String) -> Self {
        self.causes.push(cause);
        self
    }

    /// Show `file` around `line`, marking it from `column` on with `label`.
    /// Where the file cannot be read, the report names the line instead.
    pub(crate) fn source_at(
        mut self,
        file: impl Into<String>,
        line: u32,
        column: u32,
        label: impl Into<String>,
    ) -> Self {
        self.source = Some(Box::new(SourceLine {
            file: file.into(),
            line,
            column,
            label: label.into(),
        }));
        self
    }

    /// The names the caller has to choose between.
    pub(crate) fn candidates(mut self, names: impl IntoIterator<Item = String>) -> Self {
        self.candidates = names.into_iter().collect();
        self
    }

    /// What to do next, in the imperative; each call adds a line.
    pub(crate) fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hints.push(Hint {
            text: hint.into(),
            of_recording: false,
        });
        self
    }

    /// A [`hint`](Self::hint) at where a play is recorded.
    pub(crate) fn recorded(mut self, hint: impl Into<String>) -> Self {
        self.hints.push(Hint {
            text: hint.into(),
            of_recording: true,
        });
        self
    }

    /// A [`hint`](Self::hint) set apart from those before it by an empty line.
    pub(crate) fn hint_apart(self, hint: &str) -> Self {
        let gap = if self.hints.is_empty() { "" } else { "\n" };
        self.hint(format!("{gap}{hint}"))
    }

    fn render(&self, handler: impl FnOnce(Tokens<'_>) -> Box<dyn ReportHandler>) -> String {
        self.render_with(handler, |_| true)
    }

    /// Render it with the hints that are `shown`,
    /// by the `handler` made with the tokens of its source, if it reads them.
    fn render_with(
        &self,
        handler: impl FnOnce(Tokens<'_>) -> Box<dyn ReportHandler>,
        shown: fn(&Hint) -> bool,
    ) -> String {
        struct Rendered<'a>(&'a dyn ReportHandler, &'a Report);
        impl fmt::Display for Rendered<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.debug(self.1, f)
            }
        }
        let mut message = highlight(&self.headline);
        for name in &self.candidates {
            message.push_str("\n  ");
            message.push_str(&highlight(name));
        }
        let mut causes = self.causes.clone();
        let source = self.source.as_ref().and_then(|source| {
            let shown = source.read();
            if shown.is_none() {
                causes.push(format!("at {}:{}", source.file, source.line));
            }
            shown
        });
        let hints = self.hints.iter().filter(|hint| shown(hint));
        let hints: Vec<String> = hints.map(|hint| highlight(&hint.text)).collect();
        let report = Report {
            severity: match self.severity {
                Severity::Error => miette::Severity::Error,
                Severity::Warning => miette::Severity::Warning,
            },
            message,
            cause: causes
                .into_iter()
                .rev()
                .fold(None, |next, cause| Some(Cause(cause, next.map(Box::new)))),
            source,
            // The first may be set apart from one that is not shown.
            hint: (!hints.is_empty()).then(|| hints.join("\n").trim_start().to_owned()),
        };
        let tokens = || {
            let file = &self.source.as_ref()?.file;
            Code::of(file, report.source.as_ref()?.0.inner())
        };
        Rendered(&*handler(&tokens), &report).to_string()
    }

    /// Report to stderr in a panel, opening on a blank line to stand apart from what came before.
    ///
    /// Lines wrap only on a terminal, so a log or a pipe keeps each one whole for grep.
    /// They break at spaces alone, so a path or a link never splits across lines.
    pub(crate) fn report(&self) {
        // stdout goes first: anything still in its buffer would otherwise surface after the report.
        let _ = std::io::stdout().flush();
        anstream::eprintln!("\n{}", self.panel(Stream::Stderr));
    }

    /// The report in its panel, styled for `stream`.
    pub(crate) fn panel(&self, stream: Stream) -> String {
        let (text, edge) = self.for_terminal(stream);
        crate::style::panel(None, &text, edge, 0)
    }

    /// Report each of `titled` in its own panel, its title set into the top edge,
    /// the panels all as wide as the widest so they stack as one column.
    pub(crate) fn report_all(titled: &[(String, &Self)]) {
        let rendered: Vec<(String, Style)> = titled
            .iter()
            .map(|(_, diagnostic)| diagnostic.for_terminal(Stream::Stderr))
            .collect();
        let titles = titled
            .iter()
            .map(|(title, _)| crate::style::visible_width(title) + 3);
        let width = rendered
            .iter()
            .flat_map(|(text, _)| text.lines())
            .map(crate::style::visible_width)
            .chain(titles)
            .max()
            .unwrap_or(0);
        let _ = std::io::stdout().flush();
        for ((title, _), (text, edge)) in titled.iter().zip(&rendered) {
            anstream::eprintln!("\n{}", crate::style::panel(Some(title), text, *edge, width));
        }
    }

    /// The report as `stream` shows it, and the colour of the edge framing it.
    fn for_terminal(&self, stream: Stream) -> (String, Style) {
        let (terminal, size) = match stream {
            Stream::Stdout => (
                std::io::stdout().is_terminal(),
                terminal_size::terminal_size_of(std::io::stdout()),
            ),
            Stream::Stderr => (
                std::io::stderr().is_terminal(),
                terminal_size::terminal_size_of(std::io::stderr()),
            ),
        };
        let mut options = MietteHandlerOpts::new()
            .force_graphical(true)
            .terminal_links(supports_hyperlinks::on(stream))
            .wrap_lines(terminal)
            .break_words(false)
            .word_separator(WordSeparator::AsciiSpace)
            .word_splitter(WordSplitter::NoHyphenation);
        // The panel's borders and padding take four columns of the terminal's width.
        if let Some((terminal_size::Width(columns), _)) = size.filter(|_| terminal) {
            options = options.width(usize::from(columns).saturating_sub(4));
        }
        let colour = match self.severity {
            Severity::Error => AnsiColor::Red,
            Severity::Warning => AnsiColor::Yellow,
        };
        let edge = Style::new().dimmed().fg_color(Some(colour.into()));
        let colour = crate::style::coloured(stream);
        let text = self.render(|tokens| terminal_handler(options, colour, tokens));
        (text.trim_end().to_owned(), edge)
    }

    /// The report as a pipe or a CI log sees it, unwrapped,
    /// which is what assertions elsewhere in the crate should read.
    pub(crate) fn plain(&self) -> String {
        crate::style::plain(&self.unwrapped(|_| true, false))
    }

    /// As [`plain`](Self::plain), with the kind of each token of the source it shows,
    /// for a reader who is shown the recordings and may be on another machine than the paths of them.
    pub(crate) fn beside_recordings(&self) -> Highlighted {
        Highlighted::split(&self.unwrapped(|hint| !hint.of_recording, true))
    }

    /// The report unwrapped, its source `marked` by [`Frame::marked`] or plain.
    fn unwrapped(&self, shown: fn(&Hint) -> bool, marked: bool) -> String {
        let text = self.render_with(
            |tokens| {
                let handler = GraphicalReportHandler::new_themed(GraphicalTheme::unicode_nocolor())
                    .with_links(false)
                    .with_wrap_lines(false);
                Box::new(match marked.then(tokens).flatten() {
                    Some(code) => handler.with_syntax_highlighting(Frame::marked(code)),
                    None => handler.without_syntax_highlighting(),
                })
            },
            shown,
        );
        // miette sets every line in by two spaces, which is margin, not content.
        // It draws them before any style, so they are found with the styles still in.
        textwrap::dedent(&text).trim_end().to_owned()
    }

    /// The report on one line, for a JSON field or another failure's cause.
    pub(crate) fn summary(&self) -> String {
        let mut summary = self.headline.clone();
        for cause in &self.causes {
            summary.push_str(": ");
            summary.push_str(cause);
        }
        if let Some(source) = &self.source {
            summary.push_str(&format!(" (at {}:{})", source.file, source.line));
        }
        if !self.candidates.is_empty() {
            summary.push_str(&format!(" ({})", self.candidates.join(", ")));
        }
        for hint in &self.hints {
            summary.push_str("; ");
            summary.push_str(&hint.text);
        }
        // A cause laid out over several lines for a terminal is one run of words here.
        crate::style::plain(&summary)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// The tokens of the source a report shows, read from it when called.
type Tokens<'a> = &'a dyn Fn() -> Option<Code>;

/// A [`Diagnostic`] as miette reads one.
struct Report {
    severity: miette::Severity,
    /// The headline, with the candidates beneath it on lines of their own.
    message: String,
    cause: Option<Cause>,
    source: Option<(NamedSource<String>, LabeledSpan)>,
    hint: Option<String>,
}

impl SourceLine {
    /// The file, with the marked span on its line, or `None` when it cannot be read.
    fn read(&self) -> Option<(NamedSource<String>, LabeledSpan)> {
        let text = std::fs::read_to_string(&self.file).ok()?;
        let mut offset = 0;
        for (at, content) in text.split_inclusive('\n').enumerate() {
            if at + 1 == self.line as usize {
                let start = content
                    .char_indices()
                    .nth(self.column.saturating_sub(1) as usize)
                    .map_or(0, |(start, _)| start);
                let length = content.trim_end().len().saturating_sub(start).max(1);
                let span = LabeledSpan::new(Some(self.label.clone()), offset + start, length);
                let name = crate::style::relative(camino::Utf8Path::new(&self.file));
                return Some((NamedSource::new(name, text), span));
            }
            offset += content.len();
        }
        None
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl fmt::Debug for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Report {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cause
            .as_ref()
            .map(|cause| cause as &(dyn std::error::Error + 'static))
    }
}

impl miette::Diagnostic for Report {
    fn severity(&self) -> Option<miette::Severity> {
        Some(self.severity)
    }

    fn help<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
        self.hint
            .as_ref()
            .map(|hint| Box::new(hint) as Box<dyn fmt::Display>)
    }

    fn source_code(&self) -> Option<&dyn miette::SourceCode> {
        self.source
            .as_ref()
            .map(|(code, _)| code as &dyn miette::SourceCode)
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        self.source
            .as_ref()
            .map(|(_, span)| Box::new(std::iter::once(span.clone())) as Box<dyn Iterator<Item = _>>)
    }
}

/// A terminal's handler, in `colour` or not, colouring the source by its `tokens` if in colour.
fn terminal_handler(
    options: MietteHandlerOpts,
    colour: bool,
    tokens: Tokens<'_>,
) -> Box<dyn ReportHandler> {
    let options = options.color(colour);
    let options = match colour.then(tokens).flatten() {
        Some(code) => options.with_syntax_highlighting(Frame::coloured(code)),
        None => options,
    };
    Box::new(options.build())
}

/// Every `` `name` `` in `text` styled as a name: the backticks dimmed, the name in bold.
/// The backticks stay, so a pipe that drops the styling still sets the name apart.
/// A name keeps its own spaces, so it pastes back as it was typed.
fn highlight(text: &str) -> String {
    let tick = Style::new().dimmed();
    let name = Style::new().bold();
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('`') {
        let Some(close) = rest[open + 1..].find('`') else {
            break;
        };
        let inner = &rest[open + 1..open + 1 + close];
        out.push_str(&rest[..open]);
        out.push_str(&paint(tick, "`"));
        out.push_str(&paint(name, inner));
        out.push_str(&paint(tick, "`"));
        rest = &rest[open + close + 2..];
    }
    out.push_str(rest);
    out
}

/// A line beneath the headline, and the one after it,
/// given to miette as plain error sources.
///
/// A diagnostic source would draw a whole nested report instead of one `╰─▶` line.
#[derive(Debug)]
struct Cause(String, Option<Box<Cause>>);

impl fmt::Display for Cause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Cause {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.1
            .as_deref()
            .map(|next| next as &(dyn std::error::Error + 'static))
    }
}

/// The report itself, so a test that unwraps one is told what went wrong rather than shown a struct.
impl fmt::Debug for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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
        assert_eq!(Diagnostic::new("it broke").plain(), "× it broke");
    }

    #[test]
    fn candidates_sit_under_their_headline_and_the_hint_closes_the_report() {
        let diagnostic = Diagnostic::new("`knobs` matches 2 scenes")
            .candidates(["one".to_owned(), "two".to_owned()])
            .hint("narrow the pattern");
        assert_eq!(
            diagnostic.plain(),
            indoc! {"
                × `knobs` matches 2 scenes
                │   one
                │   two
                help: narrow the pattern"}
        );
    }

    #[test]
    fn a_cause_sits_between_the_headline_and_the_hint() {
        let diagnostic = Diagnostic::new("play `p` failed at step `s`")
            .cause("still waiting after 8 frames")
            .hint("a screenshot after each step is in /tmp/trace");
        assert_eq!(
            diagnostic.plain(),
            indoc! {"
                × play `p` failed at step `s`
                ╰─▶ still waiting after 8 frames
                help: a screenshot after each step is in /tmp/trace"}
        );
    }

    #[test]
    fn a_later_hint_adds_to_an_earlier_one() {
        let diagnostic = Diagnostic::new("play `p` failed")
            .hint("knob `gain` never landed")
            .hint("screenshots are in /tmp/trace");
        let plain = diagnostic.plain();
        assert!(
            plain.contains("knob `gain` never landed") && plain.contains("screenshots are in"),
            "{plain}"
        );
        assert_eq!(
            diagnostic.summary(),
            "play `p` failed; knob `gain` never landed; screenshots are in /tmp/trace"
        );
    }

    #[test]
    fn a_hint_set_apart_has_an_empty_line_above_it_unless_it_is_the_first() {
        let first = Diagnostic::new("it differs").hint_apart("`UPDATE=1` takes it");
        assert_eq!(
            first.plain(),
            indoc! {"
                × it differs
                help: `UPDATE=1` takes it"}
        );
        let later = Diagnostic::new("it differs")
            .hint("actual: a.png")
            .hint_apart("`UPDATE=1` takes it");
        let plain = later.plain();
        let lines: Vec<&str> = plain.lines().map(str::trim).collect();
        assert_eq!(
            lines,
            [
                "× it differs",
                "help: actual: a.png",
                "",
                "`UPDATE=1` takes it"
            ]
        );
        assert_eq!(
            later.summary(),
            "it differs; actual: a.png; `UPDATE=1` takes it"
        );
    }

    #[test]
    fn a_warning_is_marked_apart_from_an_error() {
        assert_eq!(
            Diagnostic::warning("it was skipped").plain(),
            "⚠ it was skipped"
        );
    }

    #[test]
    fn a_backticked_name_is_styled_and_keeps_its_backticks_without_the_styling() {
        let text = "play `p` failed at `two words`, and `half";
        let styled = highlight(text);
        assert!(styled.contains('\u{1b}'), "the name is styled: {styled:?}");
        assert_eq!(
            crate::style::plain(&styled),
            text,
            "a name stays as typed, and an unclosed backtick is left as it was"
        );
    }

    /// What miette draws for a terminal, as `for_terminal` sets it up, in colour or not.
    fn on_a_terminal(diagnostic: &Diagnostic, colour: bool) -> String {
        let options = MietteHandlerOpts::new().force_graphical(true);
        diagnostic.render(|tokens| terminal_handler(options, colour, tokens))
    }

    #[test]
    fn on_a_terminal_the_source_is_coloured_by_what_each_token_is() {
        let at = line!();
        let diagnostic = Diagnostic::new("it failed").source_at(file!(), at, 1, "here");
        let coloured = on_a_terminal(&diagnostic, true);
        assert!(coloured.contains("\u{1b}[35mlet\u{1b}[0m"), "{coloured:?}");
        let plain = on_a_terminal(&diagnostic, false);
        assert!(!plain.contains('\u{1b}'), "{plain:?}");
    }

    #[test]
    fn beside_the_recordings_the_text_is_as_plain_and_its_tokens_are_kept() {
        let at = line!();
        let failed = |recorded: bool| {
            let diagnostic = Diagnostic::new("play `p` failed")
                .cause("it was `so`")
                .source_at(file!(), at, 1, "here")
                .hint("narrow `it`");
            match recorded {
                true => diagnostic.recorded("its video is in /tmp/v.mp4"),
                false => diagnostic,
            }
        };
        let beside = failed(true).beside_recordings();
        assert_eq!(beside.text, failed(false).plain());
        let tokens: Vec<(&str, Kind)> = beside
            .tokens
            .iter()
            .map(|token| (&beside.text[token.start..token.end], token.kind))
            .collect();
        assert!(tokens.contains(&("let", Kind::Keyword)), "{tokens:?}");
        assert!(tokens.contains(&("line!", Kind::Macro)), "{tokens:?}");
    }

    #[test]
    fn beside_the_recordings_the_controls_plain_keeps_are_kept_too() {
        let diagnostic =
            Diagnostic::new("it failed").cause("50%\r100%, then\x0ca new page\tand a tab");
        assert_eq!(diagnostic.beside_recordings().text, diagnostic.plain());
    }

    #[test]
    fn a_source_that_cannot_be_read_is_named_beside_the_recordings_too() {
        let diagnostic = Diagnostic::new("it failed").source_at("gone.rs", 3, 1, "here");
        let beside = diagnostic.beside_recordings();
        assert_eq!(beside.text, diagnostic.plain());
        assert!(beside.text.contains("at gone.rs:3"), "{}", beside.text);
        assert!(beside.tokens.is_empty());
    }

    #[test]
    fn a_summary_keeps_every_part_on_one_line() {
        let diagnostic = Diagnostic::new("`knobs` matches 2 scenes")
            .cause("both start with it")
            .candidates(["one".to_owned(), "two".to_owned()])
            .hint("narrow the pattern");
        assert_eq!(
            diagnostic.summary(),
            "`knobs` matches 2 scenes: both start with it (one, two); narrow the pattern"
        );
    }
}
