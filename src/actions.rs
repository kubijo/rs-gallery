//! The actions log: what a scene reports happening, and the panel that shows it.
//!
//! [`action`] is a free function rather than a method on [`SceneCtx`](crate::SceneCtx),
//! so a scene can call it from inside a callback it hands a component,
//! where `SceneCtx` is already borrowed or out of reach entirely.

use std::{cell::RefCell, collections::VecDeque, fmt::Display, time::Instant};

/// How many lines the shell keeps.
const MOST: usize = 200;

thread_local! {
    /// Armed by [`collecting`] for a scene's render, and `None` the rest of the time.
    static SINK: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
}

/// Report that something happened, for the shell's Actions panel.
///
/// ```ignore
/// picker(ctx.ui, |row| gallery::action(format!("picked {}", row.name)));
/// ```
///
/// Only heard while a scene renders — from a spawned thread or a headless capture it goes nowhere.
pub fn action(what: impl Display) {
    SINK.with_borrow_mut(|sink| {
        if let Some(log) = sink.as_mut() {
            log.push(what.to_string());
        }
    });
}

/// Run `render` with [`action`] listening, and return what it reported.
pub(crate) fn collecting<R>(render: impl FnOnce() -> R) -> (R, Vec<String>) {
    // Fresh each frame, so a panicking render leaves nothing to replay into the next.
    SINK.with_borrow_mut(|sink| *sink = Some(Vec::new()));
    let rendered = render();
    let reported = SINK.with_borrow_mut(Option::take).unwrap_or_default();
    (rendered, reported)
}

/// The lines one scene has reported.
#[derive(Default)]
pub(crate) struct Log {
    lines: VecDeque<(f64, String)>,
    /// What the stamps count from, set on the first line — an idle scene shows no clock.
    since: Option<Instant>,
    /// Which scene the lines belong to; a different one starts over.
    scene: Option<String>,
}

impl Log {
    /// Take `reported` as `scene`'s, dropping what an earlier scene had said.
    pub(crate) fn extend(&mut self, scene: &str, reported: Vec<String>) {
        if self.scene.as_deref() != Some(scene) {
            self.lines.clear();
            self.since = None;
            self.scene = Some(scene.to_owned());
        }
        for line in reported {
            let since = *self.since.get_or_insert_with(Instant::now);
            self.lines.push_back((since.elapsed().as_secs_f64(), line));
            if self.lines.len() > MOST {
                self.lines.pop_front();
            }
        }
    }

    pub(crate) fn clear(&mut self) {
        self.lines.clear();
        self.since = None;
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

/// Newest at the bottom and stuck there, so a burst reads in the order it happened.
pub(crate) fn render_actions(ui: &mut egui::Ui, log: &Log) {
    if log.is_empty() {
        ui.weak("Nothing reported yet.");
        return;
    }
    egui::ScrollArea::vertical()
        .stick_to_bottom(true)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            for (at, line) in &log.lines {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(line).monospace());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.weak(egui::RichText::new(format!("{at:.2}s")).monospace());
                    });
                });
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scene_is_heard_only_while_it_renders() {
        action("before");
        let ((), heard) = collecting(|| {
            action("during");
            action(format_args!("{} too", "this"));
        });
        assert_eq!(heard, ["during", "this too"], "and nothing from outside");

        action("after");
        let ((), next) = collecting(|| {});
        assert!(next.is_empty(), "each render starts over");
    }

    #[test]
    fn a_log_starts_over_on_another_scene_and_keeps_only_the_last_lines() {
        let mut log = Log::default();
        log.extend("a::one", vec!["first".to_owned()]);
        log.extend("a::one", vec!["second".to_owned()]);
        assert_eq!(log.lines.len(), 2);

        log.extend("a::two", vec!["elsewhere".to_owned()]);
        assert_eq!(
            log.lines
                .iter()
                .map(|(_, l)| l.as_str())
                .collect::<Vec<_>>(),
            ["elsewhere"],
            "another scene's lines are not this scene's"
        );

        log.extend("a::two", (0..MOST + 10).map(|i| i.to_string()).collect());
        assert_eq!(log.lines.len(), MOST, "the oldest fall off the front");
        assert_eq!(
            log.lines.back().expect("a last line").1,
            (MOST + 9).to_string(),
            "and the newest stays"
        );
    }
}
