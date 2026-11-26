//! The pointer a recording draws: gallery's own, or the one a catalog's `gallery.toml` names.

use std::sync::OnceLock;

use crate::svg::Icon;

/// A pointer drawn from SVGs, so it is sharp at any scale.
pub(crate) struct Cursor {
    image: Icon,
    /// While a mouse button is held.
    pressed: Icon,
    /// The point of both that sits on the pointer, in points.
    hotspot: egui::Vec2,
}

/// The process's, set once from its catalog's `gallery.toml`.
static CURSOR: OnceLock<Cursor> = OnceLock::new();

/// Draw recordings with `configured`, a catalog's cursor, or with gallery's own.
///
/// # Errors
/// When a configured SVG cannot be read or parsed, naming it.
pub(crate) fn use_cursor(configured: Option<&gallery_build::Cursor>) -> Result<(), String> {
    let Some(configured) = configured else {
        return Ok(());
    };
    let read = |path: &camino::Utf8Path| {
        let bytes = std::fs::read(path).map_err(|e| format!("read the cursor `{path}`: {e}"))?;
        Icon::try_from_svg_colored(&bytes)
            .map_err(|e| format!("the cursor `{path}` is no SVG: {}", source_of(&e)))
    };
    let cursor = Cursor {
        image: read(&configured.image)?,
        pressed: read(&configured.pressed)?,
        hotspot: egui::Vec2::from(configured.hotspot),
    };
    // One catalog a process: a second call names the same cursor.
    let _ = CURSOR.set(cursor);
    Ok(())
}

/// The cursor recordings draw.
pub(super) fn cursor() -> &'static Cursor {
    CURSOR.get_or_init(|| Cursor {
        image: Icon::from_svg_colored(include_bytes!("../../assets/cursor.svg")),
        pressed: Icon::from_svg_colored(include_bytes!("../../assets/cursor-pressed.svg")),
        hotspot: egui::vec2(1.0, 1.0),
    })
}

impl Cursor {
    /// Paint it with its hotspot on `at`, in points, as it looks `pressed` or not.
    /// At its SVG's own size: a pointer keeps its size whatever a scene's zoom.
    pub(super) fn paint(&self, painter: &egui::Painter, at: egui::Pos2, pressed: bool) {
        let icon = if pressed { &self.pressed } else { &self.image };
        let rect = egui::Rect::from_min_size(at - self.hotspot, icon.size());
        icon.paint(painter, rect, egui::Color32::WHITE);
    }
}

fn source_of(error: &crate::svg::IconError) -> String {
    std::error::Error::source(error).map_or_else(|| error.to_string(), ToString::to_string)
}

#[cfg(test)]
mod tests {
    use camino::Utf8PathBuf;

    use super::*;

    #[test]
    fn a_cursor_that_cannot_be_read_or_is_no_svg_is_refused_by_name() {
        let scratch = tempfile::tempdir().expect("a directory");
        let dir = Utf8PathBuf::from_path_buf(scratch.path().to_owned()).expect("UTF-8");
        let good = dir.join("cursor.svg");
        std::fs::write(&good, include_bytes!("../../assets/cursor.svg")).expect("written");
        let not_svg = dir.join("cursor.png");
        std::fs::write(&not_svg, b"\x89PNG\r\n\x1a\n").expect("written");
        for (pressed, says) in [
            (dir.join("gone.svg"), "read the cursor"),
            (not_svg, "is no SVG"),
        ] {
            let configured = gallery_build::Cursor {
                image: good.clone(),
                pressed: pressed.clone(),
                hotspot: [0.0, 0.0],
            };
            let error = use_cursor(Some(&configured)).expect_err("refused");
            assert!(
                error.contains(says) && error.contains(pressed.as_str()),
                "{error}"
            );
        }
    }
}
