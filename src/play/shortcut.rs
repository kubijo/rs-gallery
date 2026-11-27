//! A key press written as a person would: `"Enter"`, `"Cmd+A"`, `"Shift+Tab"`.

use egui::{Key, Modifiers};

/// What the Ctrl key sends on Linux and Windows, as egui-winit reports it.
/// Plays press keys as those systems do, which is also how egui
/// names a shortcut in a step label, so `Cmd` and `Ctrl` are one key.
const CTRL: Modifiers = Modifiers::CTRL.plus(Modifiers::COMMAND);

const MODIFIERS: &[(&str, Modifiers)] = &[
    ("Cmd", CTRL),
    ("Command", CTRL),
    ("Ctrl", CTRL),
    ("Control", CTRL),
    ("Alt", Modifiers::ALT),
    ("Shift", Modifiers::SHIFT),
];

/// `shortcut`'s modifiers and key; names are matched regardless of case.
///
/// # Errors
/// What is wrong with it, suggesting the name a typo most likely meant.
pub(crate) fn parse(shortcut: &str) -> Result<(Modifiers, Key), String> {
    let pieces: Vec<&str> = shortcut.split('+').map(str::trim).collect();
    let (&last, held) = pieces
        .split_last()
        .expect("splitting yields at least one piece");

    let mut modifiers = Modifiers::NONE;
    for &piece in held {
        if piece.is_empty() {
            return Err("an empty name between `+`s; `+` itself is `Plus`".to_owned());
        }
        match modifier(piece) {
            Some(it) => modifiers = modifiers.plus(it),
            None if key(piece).is_some() => {
                return Err(format!(
                    "one key per press, after its modifiers: `{piece}` and `{last}` are both keys"
                ));
            }
            None => {
                return Err(
                    match closest(piece, MODIFIERS.iter().map(|(name, _)| *name)) {
                        Some(name) => format!("`{piece}` is no modifier; did you mean `{name}`?"),
                        None => format!(
                            "`{piece}` is no modifier; modifiers are `Cmd`, `Ctrl`, `Alt` and `Shift`"
                        ),
                    },
                );
            }
        }
    }

    if last.is_empty() {
        return Err(match held.last() {
            Some(before) => format!("nothing to press after `{before}`; `+` itself is `Plus`"),
            None => "nothing to press".to_owned(),
        });
    }
    if modifier(last).is_some() {
        return Err(format!(
            "`{shortcut}` is only modifiers; a press needs a key, e.g. `Shift+Tab`"
        ));
    }
    let pressed =
        key(last).ok_or_else(
            || match closest(last, Key::ALL.iter().map(|key| key.name())) {
                Some(name) => format!("`{last}` is no key; did you mean `{name}`?"),
                None => format!("`{last}` is no key"),
            },
        )?;
    Ok((modifiers, pressed))
}

fn modifier(name: &str) -> Option<Modifiers> {
    MODIFIERS
        .iter()
        .find(|(known, _)| known.eq_ignore_ascii_case(name))
        .map(|(_, modifiers)| *modifiers)
}

/// egui's names and aliases. Its parser is case-sensitive, so an alias
/// is tried capitalised too (`esc` as `Esc`), and every name regardless of case.
fn key(name: &str) -> Option<Key> {
    let mut letters = name.chars();
    let capitalised: String = letters
        .next()
        .map(|first| {
            first
                .to_uppercase()
                .chain(letters.flat_map(char::to_lowercase))
                .collect()
        })
        .unwrap_or_default();
    Key::from_name(name)
        .or_else(|| Key::from_name(&capitalised))
        .or_else(|| {
            Key::ALL
                .iter()
                .copied()
                .find(|key| key.name().eq_ignore_ascii_case(name))
        })
}

/// The one of `names` a typo of `typed` most likely meant, if any is that close.
fn closest<'a>(typed: &str, names: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let typed = typed.to_lowercase();
    names
        .map(|name| {
            (
                strsim::damerau_levenshtein(&typed, &name.to_lowercase()),
                name,
            )
        })
        .filter(|&(distance, name)| distance <= 2 && distance < name.len())
        .min_by_key(|&(distance, _)| distance)
        .map(|(_, name)| name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_alone_or_after_its_modifiers_regardless_of_case() {
        assert_eq!(parse("Enter"), Ok((Modifiers::NONE, Key::Enter)));
        assert_eq!(parse("esc"), Ok((Modifiers::NONE, Key::Escape)));
        assert_eq!(parse("Shift+Tab"), Ok((Modifiers::SHIFT, Key::Tab)));
        assert_eq!(parse("cmd + a"), Ok((CTRL, Key::A)));
        assert_eq!(
            parse("Ctrl+Shift+Z"),
            Ok((CTRL.plus(Modifiers::SHIFT), Key::Z))
        );
        assert_eq!(parse("Cmd+Plus"), Ok((CTRL, Key::Plus)));
    }

    #[test]
    fn cmd_and_ctrl_press_the_same_and_are_labelled_as_egui_labels_them() {
        let (cmd, _) = parse("Cmd+B").expect("parses");
        let (ctrl, _) = parse("Ctrl+B").expect("parses");
        assert_eq!(cmd, ctrl);
        let label =
            egui::KeyboardShortcut::new(cmd, Key::B).format(&egui::ModifierNames::NAMES, false);
        assert_eq!(label, "Ctrl+B");
    }

    #[test]
    fn a_typo_says_what_it_most_likely_meant() {
        assert_eq!(
            parse("Cmd+Entr"),
            Err("`Entr` is no key; did you mean `Enter`?".to_owned())
        );
        assert_eq!(
            parse("Cmnd+A"),
            Err("`Cmnd` is no modifier; did you mean `Cmd`?".to_owned())
        );
        assert_eq!(
            parse("Super+A"),
            Err(
                "`Super` is no modifier; modifiers are `Cmd`, `Ctrl`, `Alt` and `Shift`".to_owned()
            ),
            "with nothing close, the modifiers there are"
        );
        assert_eq!(
            parse("Hyperdrive"),
            Err("`Hyperdrive` is no key".to_owned()),
            "nothing close enough is suggested"
        );
    }

    #[test]
    fn a_press_is_modifiers_then_one_key() {
        assert_eq!(
            parse("Cmd+A+B"),
            Err("one key per press, after its modifiers: `A` and `B` are both keys".to_owned())
        );
        assert_eq!(
            parse("Cmd+"),
            Err("nothing to press after `Cmd`; `+` itself is `Plus`".to_owned())
        );
        assert_eq!(
            parse("Shift+Cmd"),
            Err("`Shift+Cmd` is only modifiers; a press needs a key, e.g. `Shift+Tab`".to_owned())
        );
        assert_eq!(parse(""), Err("nothing to press".to_owned()));
    }
}
