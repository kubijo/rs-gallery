use egui::{Event, Key, Modifiers, PointerButton, pos2};

use crate::render::overlay::{InputLog, composite};

fn key(key: Key, pressed: bool, modifiers: Modifiers) -> Event {
    Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers,
    }
}

fn click(pressed: bool) -> Event {
    Event::PointerButton {
        pos: pos2(40.0, 30.0),
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    }
}

#[test]
fn a_key_lists_its_chord_and_typed_text_its_quoted_run() {
    let mut log = InputLog::new(10);
    log.frame(&[key(Key::Tab, true, Modifiers::SHIFT)]);
    log.frame(&[key(Key::Tab, false, Modifiers::SHIFT)]);
    log.frame(&[Event::Text("hi".to_owned())]);
    assert_eq!(
        log.shown().keys,
        ["Shift+Tab", "\"hi\""],
        "a release lists nothing"
    );
}

#[test]
fn a_key_leaves_the_list_once_its_time_is_up() {
    let mut log = InputLog::new(10);
    log.frame(&[key(Key::A, true, Modifiers::NONE)]);
    for _ in 1..15 {
        log.frame(&[]);
    }
    assert_eq!(log.shown().keys, ["A"], "listed for 1.5 s at 10 fps");
    log.frame(&[]);
    assert!(log.shown().keys.is_empty());
}

#[test]
fn only_the_newest_keys_stay_listed() {
    let mut log = InputLog::new(10);
    for letter in [
        Key::A,
        Key::B,
        Key::C,
        Key::D,
        Key::E,
        Key::F,
        Key::G,
        Key::H,
    ] {
        log.frame(&[key(letter, true, Modifiers::NONE)]);
    }
    assert_eq!(log.shown().keys, ["C", "D", "E", "F", "G", "H"]);
}

#[test]
fn a_click_stays_lit_past_its_release_long_enough_to_see() {
    let mut log = InputLog::new(10);
    log.frame(&[click(true)]);
    log.frame(&[click(false)]);
    let shown = log.shown();
    assert_eq!(
        shown.buttons,
        [true, false, false],
        "lit a frame after release"
    );
    assert_eq!(shown.pointer, Some(pos2(40.0, 30.0)));
    log.frame(&[]);
    log.frame(&[]);
    assert_eq!(
        log.shown().buttons,
        [false; 3],
        "dark 0.3 s after the press"
    );
}

#[test]
fn a_pause_counts_toward_a_keys_time() {
    let mut log = InputLog::new(10);
    log.frame(&[key(Key::Escape, true, Modifiers::NONE)]);
    log.hold(20);
    log.frame(&[]);
    assert!(
        log.shown().keys.is_empty(),
        "two seconds passed in the pause"
    );
}

#[test]
fn an_overlay_pixel_is_laid_on_by_its_premultiplied_alpha() {
    let mut frame = image::RgbaImage::from_pixel(2, 1, image::Rgba([200, 100, 0, 255]));
    let mut over = image::RgbaImage::new(2, 1);
    over.put_pixel(0, 0, image::Rgba([128, 128, 128, 128]));
    composite(&mut frame, &over);
    assert_eq!(
        frame.get_pixel(0, 0).0,
        [227, 177, 128, 255],
        "half white over the frame"
    );
    assert_eq!(
        frame.get_pixel(1, 0).0,
        [200, 100, 0, 255],
        "a clear pixel leaves it be"
    );
}
