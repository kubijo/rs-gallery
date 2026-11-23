//! Terminal presentation for the things gallery prints.
//!
//! Nothing here decides *whether* to decorate: `anstream` strips escapes for a pipe or `NO_COLOR`.
//! A link is the exception: it asks the stream first, a hyperlink having no fallback rendering.

use anstyle::{AnsiColor, Style};
use anstyle_hyperlink::Hyperlink;
use camino::Utf8Path;

/// `content` in a rounded box drawn in `edge`, so a report stands apart from the ones around it.
/// A `title` sits in the top edge; `min_width` lets panels stacked together share one width.
pub(crate) fn panel(title: Option<&str>, content: &str, edge: Style, min_width: usize) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let title_width = title.map_or(0, |title| visible_width(title) + 2);
    let width = lines
        .iter()
        .map(|line| visible_width(line))
        .max()
        .unwrap_or(0)
        .max(title_width + 1)
        .max(min_width);
    let rule = "─".repeat(width + 2);
    let mut out = match title {
        Some(title) => format!(
            "{}{}{}",
            paint(edge, "╭─ "),
            paint(Style::new().bold(), title),
            paint(edge, &format!(" {}╮", "─".repeat(width + 1 - title_width))),
        ),
        None => paint(edge, &format!("╭{rule}╮")),
    };
    for line in lines {
        let pad = " ".repeat(width - visible_width(line));
        out.push_str(&format!(
            "\n{} {line}{pad} {}",
            paint(edge, "│"),
            paint(edge, "│")
        ));
    }
    out.push_str(&format!("\n{}", paint(edge, &format!("╰{rule}╯"))));
    out
}

/// A table's border drawn in `edge`, so it recedes behind what it holds.
/// Only for text whose content has no box-drawing characters of its own.
pub(crate) fn recede(table: &str, edge: Style) -> String {
    let mut out = String::with_capacity(table.len());
    let mut run = String::new();
    for c in table.chars() {
        if ('\u{2500}'..='\u{257f}').contains(&c) {
            run.push(c);
        } else {
            if !run.is_empty() {
                out.push_str(&paint(edge, &run));
                run.clear();
            }
            out.push(c);
        }
    }
    if !run.is_empty() {
        out.push_str(&paint(edge, &run));
    }
    out
}

/// How many columns `line` takes on screen, its escapes taking none.
pub(crate) fn visible_width(line: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(plain(line).as_str())
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

pub(crate) use supports_hyperlinks::Stream;

/// A path in output bound for `stream`: coloured as a path, and clickable where that stream does OSC 8.
pub(crate) fn path(path: &Utf8Path, stream: Stream) -> String {
    path_as(path, &relative(path), stream)
}

/// [`path`] showing `text` in place of the path, for a row beneath a heading that names the directory.
pub(crate) fn path_as(path: &Utf8Path, text: &str, stream: Stream) -> String {
    paint(PATH, &linked(path, text, stream))
}

/// [`path`] in a diagnostic, which goes to stderr.
pub(crate) fn path_on_stderr(target: &Utf8Path) -> String {
    path(target, Stream::Stderr)
}

const PATH: Style = Style::new().fg_color(Some(anstyle::Color::Ansi(AnsiColor::Cyan)));

/// `text` linking to `path` where `stream` does OSC 8, and plain everywhere else.
/// A relative path stays unlinked, as a `file://` URL has no base to resolve it against.
fn linked(path: &Utf8Path, text: &str, stream: Stream) -> String {
    if !path.is_absolute() || !supports_hyperlinks::on(stream) {
        return text.to_owned();
    }
    let link = Hyperlink::with_path(path.as_std_path());
    format!("{link}{text}{link:#}")
}

/// `path` as the reader would type it from where they ran gallery:
/// relative when it lies below, as given otherwise.
pub(crate) fn relative(path: &Utf8Path) -> String {
    let here = std::env::current_dir()
        .ok()
        .and_then(|dir| camino::Utf8PathBuf::from_path_buf(dir).ok());
    match here
        .as_deref()
        .and_then(|here| path.strip_prefix(here).ok())
    {
        Some(below) if !below.as_str().is_empty() => below.to_string(),
        _ => path.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panel_lines_up_its_right_border_whatever_its_lines_carry() {
        let link = Hyperlink::with_url("file:///tmp/a");
        let linked = format!("see {link}{}{link:#}", paint(PATH, "/tmp/a"));
        let content = format!("short\n{linked}\na much longer line");
        for title in [
            None,
            Some("1 of 5 · a shot"),
            Some("a title wider than every line"),
        ] {
            let boxed = panel(title, &content, Style::new(), 0);
            let widths: Vec<usize> = boxed.lines().map(visible_width).collect();
            assert!(
                widths.iter().all(|width| *width == widths[0]),
                "every line is as wide as the box: {widths:?}\n{}",
                plain(&boxed)
            );
        }
    }

    #[test]
    fn a_relative_path_stays_text_since_a_url_has_no_base_to_resolve_it_against() {
        assert_eq!(
            linked(Utf8Path::new("renders/a.png"), "a.png", Stream::Stdout),
            "a.png"
        );
    }
}
