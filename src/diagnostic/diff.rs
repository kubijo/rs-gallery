//! A text diff, cut where its tokens begin and end
//! and where a changed line's words differ from the line it replaced.

use std::ops::Range;

use owo_colors::{AnsiColors, Style, XtermColors};
use similar::{ChangeTag, TextDiff};

use super::code::{Code, Kind};

/// The language of the snapshots compared as text, AccessKit trees.
pub(crate) const SNAPSHOTS: &str = "json";

/// What became of a line.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Change {
    Removed,
    Added,
    Kept,
    /// Lines between changes, left out.
    Elided,
}

pub(crate) struct Line<'a> {
    pub(crate) change: Change,
    /// What it starts with to say what became of it.
    pub(crate) sign: &'a str,
    pub(crate) pieces: Vec<Piece<'a>>,
}

#[derive(PartialEq, Debug)]
pub(crate) struct Piece<'a> {
    pub(crate) text: &'a str,
    pub(crate) kind: Option<Kind>,
    /// Words that differ from those of the line it replaced or was replaced by.
    pub(crate) changed: bool,
}

/// The lines of `diff` of a text in `language`,
/// each starting with `-`, `+` or a space, and `…` standing for lines left out.
pub(crate) fn lines<'a>(diff: &'a str, language: &str) -> Vec<Line<'a>> {
    let parsed: Vec<(Change, &str, &str)> = diff.lines().map(parsed).collect();
    // Fragments of a text, which a grammar's parser takes as one with a few errors in it.
    let joined: Vec<&str> = parsed.iter().map(|(_, _, content)| *content).collect();
    let code = Code::in_language(language, &joined.join("\n"));
    let changed = changed_words(&parsed);
    parsed
        .iter()
        .zip(changed)
        .enumerate()
        .map(|(at, ((change, sign, content), changed))| {
            let tokens = code.as_ref().map_or(&[][..], |code| code.tokens(at));
            Line {
                change: *change,
                sign,
                pieces: pieces(content, tokens, &changed),
            }
        })
        .collect()
}

/// `diff` of a text in `language` for a terminal, its tokens in their colours.
///
/// A changed line is `tinted` where the terminal shows 256 colours:
/// its text behind a tint, the words that differ behind a stronger one.
/// Without, it is red or green, so it stands out from the tokens' colours.
///
/// The tint stops where the text does: padding to a common width would be there
/// in the plain text too, and break up where the terminal wraps a line.
pub(crate) fn painted(diff: &str, language: &str, tinted: bool) -> String {
    let lines = lines(diff, language).into_iter().zip(diff.lines());
    let painted = lines.map(|(line, written)| {
        // Dark reds and greens of the 256, for the line and for the words that differ.
        let (sign, line_tint, word_tint) = match line.change {
            Change::Removed => (AnsiColors::Red, 52, 88),
            Change::Added => (AnsiColors::Green, 22, 28),
            Change::Kept | Change::Elided => {
                let pieces = line.pieces.iter().map(|piece| {
                    let style = piece.kind.map_or_else(Style::new, Kind::coloured);
                    style.style(piece.text).to_string()
                });
                return line.sign.to_owned() + &pieces.collect::<String>();
            }
        };
        if !tinted {
            return Style::new().color(sign).style(written).to_string();
        }
        let on = |tint: u8, style: Style| style.on_color(XtermColors::from(tint));
        let mut painted = on(line_tint, Style::new().color(sign))
            .style(line.sign)
            .to_string();
        for piece in &line.pieces {
            let style = piece.kind.map_or_else(Style::new, Kind::coloured);
            let tint = if piece.changed { word_tint } else { line_tint };
            painted.push_str(&on(tint, style).style(piece.text).to_string());
        }
        painted
    });
    painted.collect::<Vec<_>>().join("\n")
}

fn parsed(line: &str) -> (Change, &str, &str) {
    if line == "…" {
        return (Change::Elided, line, "");
    }
    let (sign, content) = line.split_at(line.chars().next().map_or(0, char::len_utf8));
    let change = match sign {
        "-" => Change::Removed,
        "+" => Change::Added,
        _ => Change::Kept,
    };
    (change, sign, content)
}

/// The words of each line that differ from its counterpart's.
///
/// Lines are paired as git's contrib/diff-highlight pairs them:
/// line by line, where as many were added as were removed in their place.
fn changed_words(parsed: &[(Change, &str, &str)]) -> Vec<Vec<Range<usize>>> {
    let mut changed = vec![Vec::new(); parsed.len()];
    let run = |from: usize, change: Change| {
        let rest = parsed.get(from..).unwrap_or_default();
        rest.iter().take_while(|(was, _, _)| *was == change).count()
    };
    let mut at = 0;
    while at < parsed.len() {
        let removed = run(at, Change::Removed);
        let added = run(at + removed, Change::Added);
        if removed > 0 && removed == added {
            for nth in 0..removed {
                let (old, new) = (at + nth, at + removed + nth);
                let (gone, came) = words(parsed[old].2, parsed[new].2);
                changed[old] = gone;
                changed[new] = came;
            }
        }
        at += (removed + added).max(1);
    }
    changed
}

/// The bytes of `old` that `new` has not, and those of `new` that `old` has not, word by word.
fn words(old: &str, new: &str) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    let (mut gone, mut came) = (Vec::new(), Vec::new());
    let (mut in_old, mut in_new) = (0, 0);
    for change in TextDiff::from_words(old, new).iter_all_changes() {
        let length = change.value().len();
        match change.tag() {
            ChangeTag::Delete => {
                gone.push(in_old..in_old + length);
                in_old += length;
            }
            ChangeTag::Insert => {
                came.push(in_new..in_new + length);
                in_new += length;
            }
            ChangeTag::Equal => {
                in_old += length;
                in_new += length;
            }
        }
    }
    (gone, came)
}

/// `content` cut wherever a token or a change begins or ends.
fn pieces<'a>(
    content: &'a str,
    tokens: &[(Range<usize>, Kind)],
    changed: &[Range<usize>],
) -> Vec<Piece<'a>> {
    let ranges = tokens.iter().map(|(range, _)| range).chain(changed);
    let mut cuts: Vec<usize> = ranges.flat_map(|range| [range.start, range.end]).collect();
    cuts.extend([0, content.len()]);
    cuts.sort_unstable();
    cuts.dedup();
    cuts.windows(2)
        .map(|window| {
            let (start, end) = (window[0], window[1]);
            let token = tokens.iter().find(|(range, _)| range.contains(&start));
            Piece {
                text: &content[start..end],
                kind: token.map(|(_, kind)| *kind),
                changed: changed.iter().any(|range| range.contains(&start)),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use indoc::indoc;

    use super::*;

    /// A line's change, its sign, and the pieces of it that say something, as `(text, kind, changed)`.
    type Told<'a> = (Change, &'a str, Vec<(&'a str, Option<Kind>, bool)>);

    fn told(diff: &str) -> Vec<Told<'_>> {
        let lines = lines(diff, SNAPSHOTS).into_iter();
        lines
            .map(|line| {
                let pieces = line.pieces.iter();
                let said = pieces.filter(|piece| piece.kind.is_some() || piece.changed);
                let said = said.map(|piece| (piece.text, piece.kind, piece.changed));
                (line.change, line.sign, said.collect())
            })
            .collect()
    }

    #[test]
    fn a_line_changed_in_place_marks_the_words_that_differ() {
        let told = told(indoc! {r#"
             {
            -  "label": "Clear finished"
            +  "label": "Clear done"
             },
        "#});
        let string = Some(Kind::String);
        assert_eq!(told[1].0, Change::Removed);
        assert_eq!(
            told[1].2,
            [
                ("\"label\"", string, false),
                ("\"Clear ", string, false),
                ("finished\"", string, true)
            ]
        );
        assert_eq!(told[2].2[2], ("done\"", string, true));
        assert_eq!((told[0].0, told[0].1), (Change::Kept, " "));
    }

    #[test]
    fn lines_removed_and_added_in_unlike_numbers_are_changed_whole() {
        let told = told(indoc! {r#"
            -  "count": 1,
            -  "on": true
            +  "count": 2
        "#});
        let changed = told.iter().flat_map(|(_, _, pieces)| pieces);
        assert!(!changed.clone().any(|(_, _, changed)| *changed), "{told:?}");
        let kinds: Vec<_> = changed.map(|(text, kind, _)| (*text, *kind)).collect();
        assert!(kinds.contains(&("1", Some(Kind::Constant))), "{kinds:?}");
        assert!(kinds.contains(&("true", Some(Kind::Constant))), "{kinds:?}");
    }

    #[test]
    fn lines_left_out_are_one_sign_and_nothing_else() {
        let lines = lines(" a\n…\n b\n", SNAPSHOTS);
        assert_eq!(lines[1].change, Change::Elided);
        assert_eq!(lines[1].sign, "…");
        assert!(lines[1].pieces.is_empty());
    }

    #[test]
    fn a_changed_line_is_tinted_its_changed_words_more_and_its_text_stays_as_written() {
        let diff = " {\n-  \"on\": true\n+  \"on\": false\n";
        let tinted = painted(diff, SNAPSHOTS, true);
        let lines: Vec<&str> = tinted.lines().collect();
        assert!(
            lines[1].starts_with("\u{1b}[31;48;5;52m-"),
            "{:?}",
            lines[1]
        );
        assert!(lines[1].contains("48;5;88mtrue"), "{:?}", lines[1]);
        assert!(lines[2].contains("48;5;28mfalse"), "{:?}", lines[2]);
        assert_eq!(
            crate::style::plain(&tinted),
            diff.trim_end(),
            "nothing added to the text, so a pipe reads it as written"
        );
    }

    #[test]
    fn without_256_colours_a_changed_line_is_red_or_green_whole() {
        let diff = "-  \"on\": true\n+  \"on\": false\n";
        let shown = painted(diff, SNAPSHOTS, false);
        assert_eq!(
            shown,
            "\u{1b}[31m-  \"on\": true\u{1b}[0m\n\u{1b}[32m+  \"on\": false\u{1b}[0m"
        );
    }

    #[test]
    fn the_pieces_make_up_the_line() {
        let diff = "-  \"label\": \"Clear finished\", é\n+  \"label\": \"Clear done\", é\n";
        for (line, written) in lines(diff, SNAPSHOTS).iter().zip(diff.lines()) {
            let pieces: String = line.pieces.iter().map(|piece| piece.text).collect();
            assert_eq!(format!("{}{pieces}", line.sign), written);
        }
    }
}
