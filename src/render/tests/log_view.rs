use std::{cell::RefCell, io::Write, rc::Rc};

use super::*;
use crate::play::log::{Check, Entry, Outcome, What};

/// What a view wrote, kept where the test can read it back.
#[derive(Clone, Default)]
pub(super) struct Written(Rc<RefCell<Vec<u8>>>);

impl Write for Written {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Written {
    pub(super) fn text(&self) -> String {
        String::from_utf8(self.0.borrow().clone()).expect("UTF-8")
    }
}

pub(super) fn view(mode: LogMode, rows: usize) -> (LogView, Written) {
    let written = Written::default();
    (
        LogView::into(mode, Box::new(written.clone()), rows, 80),
        written,
    )
}

fn begun(depth: usize, start: u32, what: What) -> Entry {
    Entry {
        depth,
        frames: start..start,
        at: None,
        outcome: None,
        trace: None,
        what,
    }
}

fn ended(mut entry: Entry, end: u32, passed: bool) -> Entry {
    entry.frames.end = end;
    entry.outcome = Some(if passed {
        Outcome::Passed
    } else {
        Outcome::Failed
    });
    entry
}

fn gesture(label: &str) -> What {
    What::Gesture {
        label: label.to_owned(),
    }
}

fn exists(actual: Option<&str>) -> What {
    What::Check(Box::new(Check {
        subject: "\"Saved\"".to_owned(),
        matcher: "to_exist",
        negated: false,
        expected: "exists".to_owned(),
        actual: actual.map(str::to_owned),
        explanation: None,
        message: None,
        kept: None,
    }))
}

/// A play of a click and a check, the check passing or not.
fn play(view: &mut LogView, saved: bool, expand: bool) {
    view.begin("form", "saves", expand);
    let click = begun(0, 0, gesture("click \"Save\""));
    view.logged(0, &click);
    view.logged(0, &ended(click, 4, true));
    let check = ended(begun(0, 4, exists(None)), 10, saved);
    view.logged(1, &check);
    view.end(!saved, 10);
}

#[test]
fn linear_shows_a_passing_play_as_one_line_once_it_is_over() {
    let (mut view, written) = view(LogMode::Linear, 24);
    view.begin("form", "saves", false);
    view.logged(0, &begun(0, 0, gesture("click \"Save\"")));
    assert_eq!(written.text(), "", "nothing while the play runs");
    view.end(false, 4);
    assert_eq!(written.text(), "✓ form · saves  4 frames\n");
}

#[test]
fn linear_shows_a_failing_play_whole_with_what_its_check_found() {
    let (mut view, written) = view(LogMode::Linear, 24);
    play(&mut view, false, false);
    let text = written.text();
    assert!(
        !text.contains('\x1b'),
        "no escapes in a log file:\n{text:?}"
    );
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "form · saves", "{text}");
    assert!(lines[1].contains("✓ click \"Save\""), "{text}");
    assert!(
        lines[2].contains("× \"Saved\" exists") && lines[2].contains("got nothing matched"),
        "{text}"
    );
    assert_eq!(lines.len(), 3, "{text}");
}

#[test]
fn asked_for_its_steps_a_passing_play_is_shown_whole() {
    let (mut view, written) = view(LogMode::Linear, 24);
    play(&mut view, true, true);
    let text = written.text();
    assert_eq!(text.lines().count(), 3, "{text}");
    assert!(text.contains("✓ \"Saved\" exists"), "{text}");
}

/// What a live view left on screen: everything after its last rewind.
fn last_drawn(text: &str) -> &str {
    text.rsplit("\x1b[J").next().unwrap_or(text)
}

#[test]
fn live_redraws_in_place_and_folds_a_passed_play_to_one_line() {
    let (mut view, written) = view(LogMode::Live, 24);
    play(&mut view, true, false);
    let text = written.text();
    assert!(
        text.contains("\x1b[2F\x1b[J"),
        "went back over its rows: {text:?}"
    );
    assert_eq!(last_drawn(&text), "✓ form · saves  10 frames\n");
}

#[test]
fn live_keeps_a_failed_play_open() {
    let (mut view, written) = view(LogMode::Live, 24);
    play(&mut view, false, false);
    let text = written.text();
    let left = last_drawn(&text);
    assert_eq!(left.lines().count(), 3, "{left:?}");
    assert!(left.contains("got nothing matched"), "{left:?}");
}

#[test]
fn the_live_area_never_grows_taller_than_the_terminal() {
    let (mut view, written) = view(LogMode::Live, 4);
    view.begin("long", "many", false);
    for index in 0..10 {
        let entry = begun(0, index, gesture(&format!("press {index}")));
        view.logged(index as usize, &entry);
        view.logged(index as usize, &ended(entry, index + 1, true));
    }
    let text = written.text();
    // Rows end in newlines; the next rewind's code trails each redraw without one.
    let tallest = text
        .split("\x1b[J")
        .map(|drawn| drawn.matches('\n').count())
        .max()
        .unwrap_or(0);
    assert!(tallest <= 4, "at most 4 rows, got {tallest}: {text:?}");
    let left = last_drawn(&text);
    assert!(
        left.starts_with("long · many") && left.contains("press 9"),
        "the header and the newest rows: {left:?}"
    );
}

#[test]
fn off_shows_nothing() {
    let (mut view, written) = view(LogMode::Off, 24);
    play(&mut view, false, true);
    assert_eq!(written.text(), "");
}

#[test]
fn a_terminal_redraws_and_anything_else_reads_a_line_at_a_time() {
    assert_eq!(
        LogMode::auto(true, false, Some("xterm-256color")),
        LogMode::Live
    );
    assert_eq!(
        LogMode::auto(false, false, Some("xterm-256color")),
        LogMode::Linear
    );
    assert_eq!(LogMode::auto(true, false, Some("dumb")), LogMode::Linear);
    assert_eq!(LogMode::auto(true, true, None), LogMode::Linear);
    assert_eq!("linear".parse(), Ok(LogMode::Linear));
    assert!("loud".parse::<LogMode>().is_err());
}

#[test]
fn the_flag_outranks_gallery_log_which_outranks_the_catalog() {
    let configured = Some(LogMode::Off);
    assert_eq!(
        LogMode::detect(Some(LogMode::Linear), configured),
        Ok(LogMode::Linear)
    );
    let unflagged = LogMode::detect(None, configured);
    match std::env::var("GALLERY_LOG") {
        Ok(raw) => assert_eq!(unflagged, raw.parse(), "GALLERY_LOG is set here"),
        Err(_) => assert_eq!(unflagged, Ok(LogMode::Off), "the catalog's"),
    }
}

#[test]
fn pacing_takes_milliseconds_and_shrugs_off_anything_else() {
    use std::time::Duration;

    use crate::render::log_view::pace;

    assert_eq!(pace(Some("250")), Duration::from_millis(250));
    assert_eq!(pace(None), Duration::ZERO);
    assert_eq!(pace(Some("")), Duration::ZERO);
    assert_eq!(
        pace(Some("slow")),
        Duration::ZERO,
        "a typo only goes unpaced"
    );
}

#[test]
fn a_live_row_wider_than_the_terminal_is_cut_by_how_wide_it_shows() {
    let written = Written::default();
    let mut view = LogView::into(LogMode::Live, Box::new(written.clone()), 24, 20);
    view.begin("form", "saves", false);
    view.logged(
        0,
        &begun(0, 0, gesture("click \"保存して閉じる、保存して閉じる\"")),
    );
    let text = written.text();
    let rows: Vec<&str> = last_drawn(&text).lines().collect();
    assert!(rows.iter().any(|row| row.ends_with('…')), "{text}");
    for row in rows {
        assert!(
            style::visible_width(row) <= 20,
            "a double-width row wraps unless cut by width: {row:?}"
        );
    }
}
