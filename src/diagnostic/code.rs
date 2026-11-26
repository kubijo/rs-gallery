//! The source a failure points at, coloured by what each of its tokens is.

use std::ops::Range;

use anstyle_parse::{DefaultCharAccumulator, Params, Parser, Perform};
use miette::{
    SpanContents,
    highlighters::{Highlighter, HighlighterState},
};
use owo_colors::{AnsiColors, Style, Styled, XtermColors};

/// What a token is, of the kinds a frame tells apart; the others arborium knows stay plain.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
#[repr(u8)]
pub(crate) enum Kind {
    Keyword,
    Function,
    Macro,
    String,
    Comment,
    Type,
    Constant,
    Attribute,
    Label,
}

impl Kind {
    /// In the order declared, so each is at its discriminant.
    const ALL: [Self; 9] = [
        Self::Keyword,
        Self::Function,
        Self::Macro,
        Self::String,
        Self::Comment,
        Self::Type,
        Self::Constant,
        Self::Attribute,
        Self::Label,
    ];

    /// The first of the 256 terminal colours [`Frame::marked`] marks with, one per kind.
    /// The report is drawn with no colour of miette's own, so any would do.
    const FIRST_MARK: u8 = 232;

    /// The kind of arborium's theme `tag`, if it is one told apart.
    fn of(tag: &str) -> Option<Self> {
        Some(match tag {
            "k" => Self::Keyword,
            "f" => Self::Function,
            "m" => Self::Macro,
            "s" => Self::String,
            "c" => Self::Comment,
            "t" | "cr" => Self::Type,
            "n" | "co" => Self::Constant,
            "at" => Self::Attribute,
            "l" => Self::Label,
            _ => return None,
        })
    }

    /// As the report's styles name it.
    pub(crate) fn class(self) -> &'static str {
        match self {
            Self::Keyword => "keyword",
            Self::Function => "function",
            Self::Macro => "macro",
            Self::String => "string",
            Self::Comment => "comment",
            Self::Type => "type",
            Self::Constant => "constant",
            Self::Attribute => "attribute",
            Self::Label => "label",
        }
    }

    pub(super) fn coloured(self) -> Style {
        let style = Style::new();
        match self {
            Self::Keyword => style.color(AnsiColors::Magenta),
            Self::Function => style.color(AnsiColors::Blue),
            Self::Macro => style.color(AnsiColors::Cyan),
            Self::String => style.color(AnsiColors::Green),
            Self::Comment | Self::Attribute => style.dimmed(),
            Self::Type | Self::Label => style.color(AnsiColors::Yellow),
            Self::Constant => style.color(AnsiColors::Red),
        }
    }

    fn marked(self) -> Style {
        Style::new().color(XtermColors::from(Self::FIRST_MARK + self as u8))
    }

    fn marked_by(mark: u16) -> Option<Self> {
        let at = usize::from(mark.checked_sub(u16::from(Self::FIRST_MARK))?);
        Self::ALL.get(at).copied()
    }
}

/// A file's tokens, line by line.
pub(super) struct Code {
    lines: Vec<Line>,
}

struct Line {
    text: String,
    tokens: Vec<(Range<usize>, Kind)>,
}

impl Code {
    /// The tokens of `text`, the contents of `file`, or `None` where no grammar is built in for it.
    ///
    /// The whole file is read, so a line inside a comment or a string is told by what opened it.
    pub(super) fn of(file: &str, text: &str) -> Option<Self> {
        Self::in_language(arborium::detect_language(file)?, text)
    }

    /// The tokens of `text`, written in `language` as arborium names it.
    pub(super) fn in_language(language: &str, text: &str) -> Option<Self> {
        let spans = arborium::Highlighter::new()
            .highlight_spans(language, text)
            .ok()?;
        let tokens = arborium_highlight::spans_to_flat_tokens(text, spans);
        let mut lines = Vec::new();
        let mut start = 0;
        for content in text.split_inclusive('\n') {
            let line = content.trim_end_matches(['\n', '\r']);
            let end = start + line.len();
            // In order and apart, so those on the line follow the last to end before it.
            let first = tokens.partition_point(|token| token.end as usize <= start);
            let on = tokens[first..]
                .iter()
                .take_while(|token| (token.start as usize) < end);
            let on = on.filter_map(|token| {
                let kind = Kind::of(token.tag)?;
                let range = (token.start as usize).max(start) - start
                    ..(token.end as usize).min(end) - start;
                (!range.is_empty()).then_some((range, kind))
            });
            lines.push(Line {
                text: line.to_owned(),
                tokens: on.collect(),
            });
            start += content.len();
        }
        Some(Self { lines })
    }

    /// The tokens of line `at`, counted from 0, by their bytes in it.
    pub(super) fn tokens(&self, at: usize) -> &[(Range<usize>, Kind)] {
        self.lines.get(at).map_or(&[], |line| &line.tokens)
    }
}

/// [`Code`] as miette draws it, each kind of token in its style.
pub(super) struct Frame {
    code: Code,
    style: fn(Kind) -> Style,
}

impl Frame {
    /// For a terminal, in the colours of its palette.
    pub(super) fn coloured(code: Code) -> Self {
        Self {
            code,
            style: Kind::coloured,
        }
    }

    /// Marked for [`Highlighted::split`] to tell each token's kind.
    pub(super) fn marked(code: Code) -> Self {
        Self {
            code,
            style: Kind::marked,
        }
    }
}

impl Highlighter for Frame {
    fn start_highlighter_state<'h>(
        &'h self,
        source: &dyn SpanContents<'_>,
    ) -> Box<dyn HighlighterState + 'h> {
        Box::new(Lines {
            frame: self,
            next: source.line(),
        })
    }
}

/// The lines of a window onto the file, given in turn from its first.
struct Lines<'h> {
    frame: &'h Frame,
    next: usize,
}

impl HighlighterState for Lines<'_> {
    fn highlight_line<'s>(&mut self, line: &'s str) -> Vec<Styled<&'s str>> {
        let known = self.frame.code.lines.get(self.next);
        self.next += 1;
        // Another file's, or the file's as it was before an edit.
        let Some(known) = known.filter(|known| known.text == line) else {
            return vec![Style::new().style(line)];
        };
        let mut pieces = Vec::with_capacity(known.tokens.len() * 2 + 1);
        let mut at = 0;
        for (range, kind) in &known.tokens {
            pieces.push(Style::new().style(&line[at..range.start]));
            pieces.push((self.frame.style)(*kind).style(&line[range.clone()]));
            at = range.end;
        }
        pieces.push(Style::new().style(&line[at..]));
        pieces.retain(|piece| !piece.inner().is_empty());
        pieces
    }
}

/// Text, and the kind of each token of code in it.
#[derive(Clone, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Highlighted {
    pub(crate) text: String,
    /// In order, none overlapping another.
    pub(crate) tokens: Vec<Token>,
}

/// Bytes of [`Highlighted::text`].
#[derive(Clone, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Token {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) kind: Kind,
}

impl Highlighted {
    pub(crate) fn plain(text: String) -> Self {
        Self {
            text,
            tokens: Vec::new(),
        }
    }

    /// `drawn` with [`Frame::marked`], its marks taken out as tokens and every other style dropped.
    pub(super) fn split(drawn: &str) -> Self {
        let mut splitter = Splitter {
            split: Self::plain(String::with_capacity(drawn.len())),
            open: None,
        };
        let mut parser = Parser::<DefaultCharAccumulator>::new();
        for byte in drawn.bytes() {
            parser.advance(&mut splitter, byte);
        }
        splitter.split
    }
}

struct Splitter {
    split: Highlighted,
    /// Where the token being read began, and its kind.
    open: Option<(usize, Kind)>,
}

impl Perform for Splitter {
    fn print(&mut self, c: char) {
        self.split.text.push(c);
    }

    fn execute(&mut self, byte: u8) {
        if matches!(byte, b'\n' | b'\t') {
            self.split.text.push(char::from(byte));
        }
    }

    /// A style begun ends the token before it; a mark begins one.
    fn csi_dispatch(&mut self, params: &Params, _: &[u8], _: bool, action: u8) {
        if action != b'm' {
            return;
        }
        let at = self.split.text.len();
        if let Some((start, kind)) = self.open.take()
            && start < at
        {
            self.split.tokens.push(Token {
                start,
                end: at,
                kind,
            });
        }
        let params: Vec<u16> = params.iter().flatten().copied().collect();
        if let [38, 5, mark] = params[..] {
            self.open = Kind::marked_by(mark).map(|kind| (at, kind));
        }
    }
}

#[cfg(test)]
mod tests {
    use indoc::indoc;
    use miette::MietteSpanContents;

    use super::*;

    fn kinds_on(code: &Code, line: usize) -> Vec<(&str, Kind)> {
        let line = &code.lines[line];
        let tokens = line.tokens.iter();
        tokens
            .map(|(range, kind)| (&line.text[range.clone()], *kind))
            .collect()
    }

    /// The pieces miette is given for the lines from `first` on, as `(text, styled)`.
    fn drawn(frame: &Frame, first: usize, lines: &[&str]) -> Vec<Vec<(String, bool)>> {
        let data = lines.join("\n");
        let source = MietteSpanContents::new(data.as_bytes(), (0, data.len()).into(), first, 0, 1);
        let mut state = frame.start_highlighter_state(&source);
        let lines = lines.iter().map(|line| state.highlight_line(line));
        lines
            .map(|pieces| {
                let pieces = pieces.iter();
                pieces
                    .map(|piece| {
                        let text = piece.to_string();
                        let plain = crate::style::plain(&text);
                        let styled = text != plain;
                        (plain, styled)
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn a_token_is_told_by_what_it_is() {
        let code = Code::of(
            "src/play.rs",
            indoc! {r#"
                #[test]
                fn saves() -> Result<(), u8> {
                    let name: &'static str = r"Ada";
                    assert_eq!(name.len(), 3); // three
                    Ok(())
                }
            "#},
        )
        .expect("a Rust file has a grammar");
        assert_eq!(
            kinds_on(&code, 0),
            [("#", Kind::Attribute), ("test", Kind::Attribute)],
            "its brackets are punctuation, which stays plain"
        );
        assert_eq!(
            kinds_on(&code, 1),
            [
                ("fn", Kind::Keyword),
                ("saves", Kind::Function),
                ("Result", Kind::Type),
                ("u8", Kind::Type)
            ]
        );
        let third = kinds_on(&code, 2);
        assert!(third.contains(&("let", Kind::Keyword)), "{third:?}");
        assert!(third.contains(&("static", Kind::Label)), "{third:?}");
        assert!(third.contains(&("r\"Ada\"", Kind::String)), "{third:?}");
        let fourth = kinds_on(&code, 3);
        assert!(fourth.contains(&("assert_eq!", Kind::Macro)), "{fourth:?}");
        assert!(
            fourth.contains(&("len", Kind::Function)),
            "inside a macro's arguments too: {fourth:?}"
        );
        assert!(fourth.contains(&("3", Kind::Constant)), "{fourth:?}");
        assert!(fourth.contains(&("// three", Kind::Comment)), "{fourth:?}");
    }

    #[test]
    fn a_line_is_told_by_what_opened_before_it() {
        let code = Code::of("a.rs", "/* one\n two */ let x = 1;\n").expect("a grammar");
        let second = kinds_on(&code, 1);
        assert_eq!(
            second.first(),
            Some(&(" two */", Kind::Comment)),
            "{second:?}"
        );
        assert!(second.contains(&("let", Kind::Keyword)), "{second:?}");
    }

    #[test]
    fn a_file_with_no_grammar_built_in_has_no_tokens() {
        assert!(Code::of("notes.txt", "fn main() {}").is_none());
    }

    #[test]
    fn miette_is_given_each_line_of_its_window_in_pieces_that_make_it_up() {
        let code = Code::of("a.rs", "/* one\n two */ let x = 1;\nlet y = 2;\n").expect("a grammar");
        let pieces = drawn(
            &Frame::coloured(code),
            1,
            &[" two */ let x = 1;", "let y = 2;"],
        );
        let first = &pieces[0];
        assert_eq!(first[0], (" two */".to_owned(), true), "{first:?}");
        let joined: Vec<String> = pieces
            .iter()
            .map(|line| line.iter().map(|(text, _)| text.as_str()).collect())
            .collect();
        assert_eq!(joined, [" two */ let x = 1;", "let y = 2;"]);
    }

    #[test]
    fn a_line_unlike_the_files_is_given_back_plain() {
        let code = Code::of("a.rs", "let x = 1;\n").expect("a grammar");
        let pieces = drawn(&Frame::coloured(code), 0, &["let x = 2; // edited since"]);
        assert_eq!(pieces, [[("let x = 2; // edited since".to_owned(), false)]]);
    }

    #[test]
    fn marks_come_out_as_tokens_and_other_styles_are_dropped() {
        let code = Code::of("a.rs", "let ü = \"é\";\n").expect("a grammar");
        let frame = Frame::marked(code);
        let source = MietteSpanContents::new(b"", (0, 0).into(), 0, 0, 1);
        let mut state = frame.start_highlighter_state(&source);
        let line: String = state
            .highlight_line("let ü = \"é\";")
            .iter()
            .map(ToString::to_string)
            .collect();
        let bold = anstyle::Style::new().bold();
        let drawn = format!("{bold}× it{bold:#}\n  1 │ {line}\n");
        let split = Highlighted::split(&drawn);
        assert_eq!(split.text, "× it\n  1 │ let ü = \"é\";\n");
        let tokens: Vec<(&str, Kind)> = split
            .tokens
            .iter()
            .map(|token| (&split.text[token.start..token.end], token.kind))
            .collect();
        assert_eq!(tokens, [("let", Kind::Keyword), ("\"é\"", Kind::String)]);
    }
}
