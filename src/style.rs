//! Terminal presentation for the things gallery prints.
//!
//! Nothing here decides *whether* to decorate: `anstream` strips escapes for a pipe or `NO_COLOR`.
//! [`link`] is the exception — it asks first, a hyperlink having no fallback rendering.

use std::fmt::Write as _;

use anstyle::Style;
use camino::Utf8Path;
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};

/// A headline with the rows beneath it branching off a gutter.
///
/// A `footer` closes the block on a line of its own — a hint, or a note about the rows above it.
/// Without one the last row closes it instead.
pub(crate) fn frame(headline: &str, rows: &[String], footer: Option<&str>) -> String {
    let edge = Style::new().dimmed();
    let mut out = headline.to_owned();
    for (nth, row) in rows.iter().enumerate() {
        let closes = footer.is_none() && nth + 1 == rows.len();
        let stem = if closes { "  ╰─  " } else { "  ├─  " };
        let _ = write!(out, "\n{}{row}", paint(edge, stem));
    }
    if let Some(footer) = footer {
        // Nothing to hang off with no rows above it, so the footer brings its own indent.
        let corner = if rows.is_empty() { "" } else { "  ╰─  " };
        let _ = write!(out, "\n{}{footer}", paint(edge, corner));
    }
    out
}

/// `styled` as a pipe or a CI log sees it — what an assertion about wording or width should read.
pub(crate) fn plain(styled: &str) -> String {
    String::from_utf8(anstream::adapter::strip_bytes(styled.as_bytes()).into_vec())
        .expect("stripping escapes keeps it UTF-8")
}

/// `text` in `style`, closed again after it.
///
/// Both halves come from the style itself: writing the opening sequence and expecting `{:#}`
/// to close it silently re-opens instead, bleeding the colour down the rest of the line.
pub(crate) fn paint(style: Style, text: &str) -> String {
    format!("{}{text}{}", style.render(), style.render_reset())
}

/// `path`, clickable where the terminal does OSC 8 and plain text everywhere else.
///
/// A relative path is left alone — a `file://` URL has no base to resolve it against.
pub(crate) fn link(path: &Utf8Path) -> String {
    if !path.is_absolute() || !supports_hyperlinks::on(supports_hyperlinks::Stream::Stdout) {
        return path.to_string();
    }
    format!("\u{1b}]8;;{}\u{1b}\\{path}\u{1b}]8;;\u{1b}\\", url(path))
}

fn url(target: &Utf8Path) -> String {
    format!("file://{}", utf8_percent_encode(target.as_str(), URL))
}

/// Everything a `file://` path may not carry literally. `/` stays, being the path's own punctuation;
/// the rest either end the URL early or are reserved by RFC 3986.
const URL: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'%')
    .add(b'#')
    .add(b'?')
    .add(b'"')
    .add(b'<')
    .add(b'>')
    .add(b'`')
    .add(b'{')
    .add(b'}')
    .add(b'|')
    .add(b'\\')
    .add(b'^')
    .add(b'[')
    .add(b']');

#[cfg(test)]
mod tests {
    use indoc::indoc;

    use super::*;

    #[test]
    fn a_block_with_no_footer_closes_on_its_last_row_without_stepping_out_of_the_column() {
        let rows = ["one".to_owned(), "two".to_owned()];
        assert_eq!(
            plain(&frame("headline", &rows, None)),
            indoc! {"
                headline
                  ├─  one
                  ╰─  two"}
        );
    }

    #[test]
    fn a_relative_path_stays_text_since_a_url_has_no_base_to_resolve_it_against() {
        assert_eq!(link(Utf8Path::new("renders/a.png")), "renders/a.png");
    }

    #[test]
    fn a_linked_path_is_escaped_so_a_space_cannot_end_the_url_early() {
        assert_eq!(
            url(Utf8Path::new("/tmp/my renders/a #1.png")),
            "file:///tmp/my%20renders/a%20%231.png"
        );
    }
}
