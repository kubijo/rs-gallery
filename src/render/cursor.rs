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

/// The cursor a process draws, set once from its catalog's `gallery.toml`.
struct Cursors {
    configured: OnceLock<gallery_build::Cursor>,
    /// Read when the first play is recorded,
    /// so a bad file fails plays' recordings and nothing else.
    drawn: OnceLock<Result<Cursor, String>>,
}

static CURSORS: Cursors = Cursors::new();

impl Cursors {
    const fn new() -> Self {
        Self {
            configured: OnceLock::new(),
            drawn: OnceLock::new(),
        }
    }

    fn configure(&self, configured: Option<&gallery_build::Cursor>) {
        if let Some(configured) = configured {
            // One catalog a process: a second call names the same cursor.
            let _ = self.configured.set(configured.clone());
        }
    }

    fn drawn(&self) -> Result<&Cursor, String> {
        self.drawn
            .get_or_init(|| resolve(self.configured.get()))
            .as_ref()
            .map_err(Clone::clone)
    }
}

/// Draw recordings with `configured`, a catalog's cursor, or with gallery's own.
pub(crate) fn use_cursor(configured: Option<&gallery_build::Cursor>) {
    CURSORS.configure(configured);
}

/// The cursor recordings draw.
///
/// # Errors
/// When a configured SVG cannot be read or parsed, naming it.
pub(super) fn cursor() -> Result<&'static Cursor, String> {
    CURSORS.drawn()
}

/// The `configured` cursor, or gallery's own.
fn resolve(configured: Option<&gallery_build::Cursor>) -> Result<Cursor, String> {
    configured.map_or_else(|| Ok(built_in()), load)
}

fn built_in() -> Cursor {
    Cursor {
        image: Icon::from_svg_colored(include_bytes!("../../assets/cursor.svg")),
        pressed: Icon::from_svg_colored(include_bytes!("../../assets/cursor-pressed.svg")),
        hotspot: egui::vec2(1.0, 1.0),
    }
}

fn load(configured: &gallery_build::Cursor) -> Result<Cursor, String> {
    let read = |path: &camino::Utf8Path| {
        let bytes = std::fs::read(path).map_err(|e| format!("read the cursor `{path}`: {e}"))?;
        let icon = Icon::try_from_svg_colored(&bytes)
            .map_err(|e| format!("the cursor `{path}` is no SVG: {}", source_of(&e)))?;
        let undrawn = icon
            .undrawn()
            .map(str::to_owned)
            .or_else(|| crate::svg::undrawn_markup(&bytes));
        if let Some(what) = undrawn {
            return Err(format!(
                "the cursor `{path}` has {what}, which is not drawn: only paths filled with a colour are"
            ));
        }
        if !icon.draws() {
            return Err(format!(
                "the cursor `{path}` draws nothing: it has no filled path"
            ));
        }
        Ok(icon)
    };
    Ok(Cursor {
        image: read(&configured.image)?,
        pressed: read(&configured.pressed)?,
        hotspot: egui::Vec2::from(configured.hotspot),
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

    /// Where `cursor` lands painted with its hotspot on `at`, pressed or not.
    fn painted(cursor: &Cursor, at: egui::Pos2, pressed: bool) -> egui::Rect {
        let context = egui::Context::default();
        let mut output = context.run_ui(egui::RawInput::default(), |ui| {
            cursor.paint(ui.painter(), at, pressed);
        });
        output.textures_delta.clear();
        output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Mesh(mesh) if mesh.texture_id != egui::TextureId::default() => {
                    Some(mesh.calc_bounds())
                }
                _ => None,
            })
            .expect("a cursor painted")
    }

    #[test]
    fn a_cursor_lands_with_its_hotspot_on_the_pointer_at_its_svgs_size() {
        let at = egui::pos2(40.0, 30.0);
        let unset = Cursors::new();
        unset.configure(None);
        let own = unset.drawn().expect("gallery's own");
        for pressed in [false, true] {
            assert_eq!(
                painted(own, at, pressed),
                egui::Rect::from_min_size(egui::pos2(39.0, 29.0), egui::Vec2::splat(24.0)),
                "gallery's own, pressed {pressed}: its tip 1 in from its corner, 24 points"
            );
        }

        let scratch = tempfile::tempdir().expect("a directory");
        let dir = Utf8PathBuf::from_path_buf(scratch.path().to_owned()).expect("UTF-8");
        let square = |name: &str, side: u8| {
            let path = dir.join(name);
            let text = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="{side}" height="{side}"><path d="M0 0h{side}v{side}H0z" fill="red"/></svg>"#
            );
            std::fs::write(&path, text).expect("written");
            path
        };
        let configured = gallery_build::Cursor {
            image: square("cursor.svg", 12),
            pressed: square("pressed.svg", 16),
            hotspot: [7.0, 3.0],
        };
        let set = Cursors::new();
        set.configure(Some(&configured));
        let catalogs = set.drawn().expect("the catalog's");
        assert_eq!(
            painted(catalogs, at, false),
            egui::Rect::from_min_size(egui::pos2(33.0, 27.0), egui::Vec2::splat(12.0)),
            "the catalog's: its hotspot on the pointer, at its own size"
        );
        assert_eq!(
            painted(catalogs, at, true),
            egui::Rect::from_min_size(egui::pos2(33.0, 27.0), egui::Vec2::splat(16.0)),
            "the catalog's pressed one: its own SVG, the same hotspot"
        );
    }

    #[test]
    fn a_cursor_that_cannot_be_read_or_is_no_svg_is_refused_by_name() {
        let scratch = tempfile::tempdir().expect("a directory");
        let dir = Utf8PathBuf::from_path_buf(scratch.path().to_owned()).expect("UTF-8");
        let good = dir.join("cursor.svg");
        std::fs::write(&good, include_bytes!("../../assets/cursor.svg")).expect("written");
        let not_svg = dir.join("cursor.png");
        std::fs::write(&not_svg, b"\x89PNG\r\n\x1a\n").expect("written");
        let svg = |name: &str, body: &str| {
            let path = dir.join(name);
            let text = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24">{body}</svg>"#
            );
            std::fs::write(&path, text).expect("written");
            path
        };
        let outlined = svg(
            "outlined.svg",
            r#"<path d="M1 1v20l6-6h10z" fill="white" stroke="black"/>"#,
        );
        let arrow = r#"<path d="M1 1v20l6-6h10z" fill="white"/>"#;
        let gradient = svg(
            "gradient.svg",
            r##"<linearGradient id="g"><stop offset="0" stop-color="red"/><stop offset="1" stop-color="blue"/></linearGradient><path d="M1 1v20l6-6h10z" fill="url(#g)"/>"##,
        );
        let pixel = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
        let image = svg(
            "image.svg",
            &format!(
                r#"{arrow}<image width="4" height="4" href="data:image/png;base64,{pixel}"/>"#
            ),
        );
        image::RgbaImage::new(1, 1)
            .save(dir.join("pixel.png"))
            .expect("written");
        let linked = svg(
            "linked.svg",
            &format!(r#"{arrow}<image width="4" height="4" href="pixel.png"/>"#),
        );
        let foreign = svg(
            "foreign.svg",
            &format!(r#"{arrow}<foreignObject width="4" height="4"><p>Hi</p></foreignObject>"#),
        );
        let text = svg("text.svg", &format!("{arrow}<text y=\"20\">A</text>"));
        let hidden_layer = svg(
            "hidden-layer.svg",
            &format!(r#"{arrow}<g style="display:none"><text y="20">A</text></g>"#),
        );
        let hidden_stroke = svg(
            "hidden-stroke.svg",
            &format!(r#"{arrow}<path d="M0 0h4v4H0z" stroke="black" visibility="hidden"/>"#),
        );
        let undisplayed = svg(
            "undisplayed.svg",
            &format!(r#"{arrow}<path d="M0 0h4v4H0z" fill="red" display="none"/>"#),
        );
        let faded = svg("faded.svg", &format!(r#"<g opacity="0.5">{arrow}</g>"#));
        let blended = svg(
            "blended.svg",
            &format!(r#"<g style="mix-blend-mode:difference">{arrow}</g>"#),
        );
        let clipped = svg(
            "clipped.svg",
            &format!(
                r#"<clipPath id="c"><rect width="8" height="8"/></clipPath><g clip-path="url(#c)">{arrow}</g>"#
            ),
        );
        let masked = svg(
            "masked.svg",
            &format!(
                r#"<mask id="m"><rect width="8" height="8" fill="white"/></mask><g mask="url(#m)">{arrow}</g>"#
            ),
        );
        let filtered = svg(
            "filtered.svg",
            &format!(
                r#"<filter id="f"><feGaussianBlur stdDeviation="1"/></filter><g filter="url(#f)">{arrow}</g>"#
            ),
        );
        let dangling = svg(
            "dangling.svg",
            &format!(r#"{arrow}<path d="M0 0h4v4H0z" fill="red" filter="url(#gone)"/>"#),
        );
        let unknown = svg(
            "unknown.svg",
            &format!(r#"{arrow}<layer><path d="M0 0h4v4H0z" fill="red"/></layer>"#),
        );
        let switched = svg(
            "switched.svg",
            &format!(r#"<switch>{arrow}<path d="M0 0h4v4H0z" fill="red"/></switch>"#),
        );
        let styled = svg(
            "styled.svg",
            &format!(r#"<style>.h {{ display: none }}</style>{arrow}"#),
        );
        let conditional = svg(
            "conditional.svg",
            &format!(r#"{arrow}<path d="M0 0h4v4H0z" fill="red" systemLanguage="xx"/>"#),
        );
        let external = svg(
            "external.svg",
            &format!(r#"{arrow}<use href="other.svg#a"/>"#),
        );
        let decoy = |name: &str, attributes: &str| {
            svg(
                name,
                &format!(
                    r#"<g xmlns:f="urn:f">{arrow}<path d="M0 0h4v4H0z" fill="red" {attributes}/></g>"#
                ),
            )
        };
        let decoy_hidden = decoy("decoy-hidden.svg", r#"f:display="inline" display="none""#);
        let decoy_filter = decoy("decoy-filter.svg", r#"f:filter="none" filter="url(#gone)""#);
        let decoy_link = svg(
            "decoy-link.svg",
            &format!(r##"<g xmlns:f="urn:f">{arrow}<use f:href="#a" href="other.svg#x"/></g>"##),
        );
        let linked_fill = svg(
            "linked-fill.svg",
            r#"<path d="M1 1v20l6-6h10z" fill="url(other.svg#g)"/>"#,
        );
        let linked_stroke = svg(
            "linked-stroke.svg",
            &format!(r#"<g style="stroke: url(other.svg#g)">{arrow}</g>"#),
        );
        let empty = svg("empty.svg", "");
        let bare = dir.join("bare.svg");
        std::fs::write(
            &bare,
            format!(r#"<svg width="24" height="24">{arrow}</svg>"#),
        )
        .expect("written");
        let bare = gallery_build::Cursor {
            image: bare.clone(),
            pressed: bare,
            hotspot: [0.0, 0.0],
        };
        assert!(
            load(&bare).is_ok(),
            "an SVG with no namespace is drawn as usvg reads it"
        );
        let annotated = svg(
            "annotated.svg",
            &format!(
                r#"<title>A</title><desc>B</desc><metadata><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"/></metadata><defs/><g>{arrow}</g>"#
            ),
        );
        let annotated = gallery_build::Cursor {
            image: annotated.clone(),
            pressed: annotated,
            hotspot: [0.0, 0.0],
        };
        assert!(
            load(&annotated).is_ok(),
            "a title, description, metadata and groups are not drawn, and leave nothing out"
        );
        for (pressed, says) in [
            (dir.join("gone.svg"), "read the cursor"),
            (not_svg, "is no SVG"),
            (outlined, "has a stroke, which is not drawn"),
            (gradient, "has a gradient or pattern fill"),
            (image, "has an image"),
            (linked, "has an image"),
            (foreign, "has a foreign object"),
            (text, "has text"),
            (hidden_layer, "has a hidden element"),
            (hidden_stroke, "has a hidden element"),
            (undisplayed, "has a hidden element"),
            (faded, "has a clip path, mask, filter or group opacity"),
            (blended, "has a blend mode"),
            (clipped, "has a clip path, mask"),
            (masked, "has a clip path, mask"),
            (filtered, "has a clip path, mask"),
            (dangling, "has a clip path, mask"),
            (unknown, "has a `<layer>` element"),
            (switched, "has a `<switch>` element"),
            (styled, "has a `<style>` element"),
            (conditional, "has a conditional attribute"),
            (external, "has a link to another file"),
            (decoy_hidden, "has a hidden element"),
            (decoy_filter, "has a clip path, mask"),
            (decoy_link, "has a link to another file"),
            (linked_fill, "has a link to another file"),
            (linked_stroke, "has a link to another file"),
            (empty, "draws nothing"),
        ] {
            let configured = gallery_build::Cursor {
                image: good.clone(),
                pressed: pressed.clone(),
                hotspot: [0.0, 0.0],
            };
            let Err(error) = load(&configured) else {
                panic!("`{pressed}` refused");
            };
            assert!(
                error.contains(says) && error.contains(pressed.as_str()),
                "{error}"
            );
        }
    }
}
