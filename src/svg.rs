//! SVG icons cached by physical display size.

use std::{
    collections::hash_map::DefaultHasher,
    error::Error,
    fmt,
    hash::{Hash as _, Hasher as _},
    sync::Arc,
};

use egui::emath::GuiRounding as _;

const SUPERSAMPLE: u32 = 4;
const RASTER_MARGIN: f32 = 1.0;
const MAX_RASTER_SIDE: f32 = 512.0;

/// An SVG icon: tinted monochrome with [`Self::from_svg`], or original fill colours
/// with [`Self::from_svg_colored`]. Use [`Self::rounded`] for a circular crop.
#[derive(Clone)]
pub struct Icon {
    svg: Arc<Svg>,
    colored: bool,
    rounded: bool,
}

/// Bytes that [`Icon::try_from_svg`] or [`Icon::try_from_svg_colored`] could not parse as an SVG.
///
/// The parser's own error is available through [`Error::source`].
#[derive(Debug)]
pub struct IconError(usvg::Error);

impl fmt::Display for IconError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("invalid icon SVG")
    }
}

impl Error for IconError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.0)
    }
}

struct Svg {
    hash: u64,
    width: f32,
    height: f32,
    paths: Vec<FillPath>,
    /// The first thing it has that is not drawn: only paths filled with a colour are.
    undrawn: Option<&'static str>,
}

struct FillPath {
    data: tiny_skia::Path,
    transform: tiny_skia::Transform,
    rule: tiny_skia::FillRule,
    color: Option<usvg::Color>,
    opacity: f32,
}

impl Icon {
    /// Parse filled SVG paths. Fill colours are ignored.
    ///
    /// # Panics
    /// If the supplied bytes are not a valid SVG; [`Self::try_from_svg`] returns the error instead.
    #[must_use]
    pub fn from_svg(bytes: &[u8]) -> Self {
        Self::try_from_svg(bytes).expect("parse icon SVG")
    }

    /// Parse filled SVG paths like [`Self::from_svg`], for bytes that may not be a valid SVG.
    ///
    /// # Errors
    /// If the supplied bytes are not a valid SVG.
    pub fn try_from_svg(bytes: &[u8]) -> Result<Self, IconError> {
        let tree = usvg::Tree::from_data(bytes, &usvg::Options::default()).map_err(IconError)?;
        let size = tree.size();
        let mut hasher = DefaultHasher::new();
        bytes.hash(&mut hasher);

        Ok(Self {
            colored: false,
            rounded: false,
            svg: Arc::new(Svg {
                hash: hasher.finish(),
                width: size.width(),
                height: size.height(),
                paths: paths(tree.root()),
                undrawn: undrawn(tree.root()),
            }),
        })
    }

    /// Parse filled SVG paths with their colours and fill opacity.
    ///
    /// Gradients and patterns use the paint call's tint instead.
    ///
    /// # Panics
    /// If the supplied bytes are not a valid SVG; [`Self::try_from_svg_colored`] returns the error instead.
    #[must_use]
    pub fn from_svg_colored(bytes: &[u8]) -> Self {
        Self::try_from_svg_colored(bytes).expect("parse icon SVG")
    }

    /// Parse filled SVG paths with their colours like [`Self::from_svg_colored`],
    /// for bytes that may not be a valid SVG.
    ///
    /// # Errors
    /// If the supplied bytes are not a valid SVG.
    pub fn try_from_svg_colored(bytes: &[u8]) -> Result<Self, IconError> {
        Ok(Self {
            colored: true,
            ..Self::try_from_svg(bytes)?
        })
    }

    /// Crop to an inscribed circle, preserving the icon's colour mode.
    #[must_use]
    pub fn rounded(mut self) -> Self {
        self.rounded = true;
        self
    }

    fn texture_hash(&self, tint: egui::Color32) -> u64 {
        if !self.colored && !self.rounded {
            return self.svg.hash;
        }
        let mut hasher = DefaultHasher::new();
        (self.svg.hash, self.colored, self.rounded).hash(&mut hasher);
        if self.colored && self.svg.paths.iter().any(|path| path.color.is_none()) {
            tint.hash(&mut hasher);
        }
        hasher.finish()
    }

    /// The SVG's own width and height, in its units.
    pub(crate) fn size(&self) -> egui::Vec2 {
        egui::vec2(self.svg.width, self.svg.height)
    }

    /// Whether it has a filled path to draw.
    pub(crate) fn draws(&self) -> bool {
        !self.svg.paths.is_empty()
    }

    /// What it has that it draws without, if anything: a stroke, a gradient, a hidden path….
    /// What usvg dropped is not seen here; [`undrawn_markup`] tells it.
    pub(crate) fn undrawn(&self) -> Option<&'static str> {
        self.svg.undrawn
    }

    /// Allocate a `size`×`size` slot. Coloured solid fills ignore `color`.
    pub fn show(&self, ui: &mut egui::Ui, size: f32, color: egui::Color32) {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
        self.paint(ui.painter(), rect, color);
    }

    /// Paint into `rect`, using `color` for monochrome paths and unsupported fills.
    pub fn paint(&self, painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
        let pixels_per_point = painter.pixels_per_point();
        let rect = rect.round_to_pixels(pixels_per_point);
        if !rect.is_finite() || !rect.is_positive() {
            return;
        }
        let pixel_size = [
            (rect.width() * pixels_per_point)
                .round()
                .clamp(1.0, MAX_RASTER_SIDE) as u32,
            (rect.height() * pixels_per_point)
                .round()
                .clamp(1.0, MAX_RASTER_SIDE) as u32,
        ];
        let texture = self.texture(painter.ctx(), pixel_size, color);
        painter.with_clip_rect(rect).image(
            texture.id(),
            rect,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            if self.colored {
                egui::Color32::WHITE
            } else {
                color
            },
        );
    }

    fn texture(
        &self,
        context: &egui::Context,
        pixel_size: [u32; 2],
        tint: egui::Color32,
    ) -> egui::TextureHandle {
        let hash = self.texture_hash(tint);
        let cache_id = egui::Id::new(("gallery-svg-icon", hash, pixel_size));
        if let Some(texture) = context.data(|data| data.get_temp(cache_id)) {
            return texture;
        }

        let texture = context.load_texture(
            format!(
                "gallery-svg-icon-{:016x}-{}x{}",
                hash, pixel_size[0], pixel_size[1]
            ),
            self.svg
                .rasterize(pixel_size, self.colored, self.rounded, tint),
            egui::TextureOptions::NEAREST,
        );
        context.data_mut(|data| data.insert_temp(cache_id, texture.clone()));
        texture
    }
}

impl Svg {
    fn rasterize(
        &self,
        size: [u32; 2],
        colored: bool,
        rounded: bool,
        tint: egui::Color32,
    ) -> egui::ColorImage {
        let supersampled = [size[0] * SUPERSAMPLE, size[1] * SUPERSAMPLE];
        let mut pixmap = tiny_skia::Pixmap::new(supersampled[0], supersampled[1])
            .expect("icon raster dimensions are non-zero");
        let mut paint = tiny_skia::Paint::default();
        paint.set_color_rgba8(255, 255, 255, 255);
        paint.anti_alias = true;

        let draw_width = supersampled[0] as f32 - RASTER_MARGIN * 2.0;
        let draw_height = supersampled[1] as f32 - RASTER_MARGIN * 2.0;
        let scale_x = draw_width / self.width;
        let scale_y = draw_height / self.height;
        let mask = rounded.then(|| {
            let mut mask = tiny_skia::Mask::new(supersampled[0], supersampled[1])
                .expect("icon raster dimensions are non-zero");
            let circle = tiny_skia::PathBuilder::from_circle(
                supersampled[0] as f32 / 2.0,
                supersampled[1] as f32 / 2.0,
                draw_width.min(draw_height) / 2.0,
            )
            .expect("icon circle has a positive radius");
            mask.fill_path(
                &circle,
                tiny_skia::FillRule::Winding,
                true,
                tiny_skia::Transform::identity(),
            );
            mask
        });
        for path in &self.paths {
            if colored {
                let [r, g, b, a] = path.color.map_or_else(
                    || tint.to_srgba_unmultiplied(),
                    |color| [color.red, color.green, color.blue, 255],
                );
                paint.set_color(
                    tiny_skia::Color::from_rgba(
                        f32::from(r) / 255.0,
                        f32::from(g) / 255.0,
                        f32::from(b) / 255.0,
                        f32::from(a) / 255.0 * path.opacity,
                    )
                    .expect("SVG fill channels are normalized"),
                );
            }
            let transform = tiny_skia::Transform::from_row(
                path.transform.sx * scale_x,
                path.transform.ky * scale_y,
                path.transform.kx * scale_x,
                path.transform.sy * scale_y,
                path.transform.tx * scale_x + RASTER_MARGIN,
                path.transform.ty * scale_y + RASTER_MARGIN,
            );
            pixmap.fill_path(&path.data, &paint, path.rule, transform, mask.as_ref());
        }

        let mut pixels = Vec::with_capacity((size[0] * size[1] * 4) as usize);
        for y in 0..size[1] {
            for x in 0..size[0] {
                let mut channels = [0_u32; 4];
                for sample_y in 0..SUPERSAMPLE {
                    for sample_x in 0..SUPERSAMPLE {
                        let sx = x * SUPERSAMPLE + sample_x;
                        let sy = y * SUPERSAMPLE + sample_y;
                        let at = ((sy * supersampled[0] + sx) * 4) as usize;
                        for (channel, sum) in channels.iter_mut().enumerate() {
                            *sum +=
                                u32::from(pixmap.data()[at + if colored { channel } else { 3 }]);
                        }
                    }
                }
                pixels.extend(channels.map(|sum| (sum / (SUPERSAMPLE * SUPERSAMPLE)) as u8));
            }
        }
        egui::ColorImage::from_rgba_premultiplied([size[0] as usize, size[1] as usize], &pixels)
    }
}

/// The shell's bundled icons.
pub struct Icons {
    pub folder: Icon,
    pub app: Icon,
    pub search: Icon,
    pub window_minimize: Icon,
    pub window_maximize: Icon,
    pub window_restore: Icon,
    pub window_close: Icon,
}

impl Icons {
    #[must_use]
    pub fn load() -> Self {
        Self {
            folder: Icon::from_svg(include_bytes!("../assets/icons/folder.svg")),
            app: Icon::from_svg(include_bytes!("../assets/icons/app.svg")),
            search: Icon::from_svg(include_bytes!("../assets/icons/search.svg")),
            window_minimize: Icon::from_svg(include_bytes!("../assets/icons/window-minimize.svg")),
            window_maximize: Icon::from_svg(include_bytes!("../assets/icons/window-maximize.svg")),
            window_restore: Icon::from_svg(include_bytes!("../assets/icons/window-restore.svg")),
            window_close: Icon::from_svg(include_bytes!("../assets/icons/window-close.svg")),
        }
    }
}

fn paths(group: &usvg::Group) -> Vec<FillPath> {
    let mut out = Vec::new();
    fn walk(group: &usvg::Group, out: &mut Vec<FillPath>) {
        for node in group.children() {
            match node {
                usvg::Node::Group(child) => walk(child, out),
                usvg::Node::Path(path) if path.is_visible() => {
                    if let Some(fill) = path.fill() {
                        out.push(FillPath {
                            color: match fill.paint() {
                                usvg::Paint::Color(color) => Some(*color),
                                _ => None,
                            },
                            opacity: fill.opacity().get(),
                            data: path.data().clone(),
                            transform: path.abs_transform(),
                            rule: match fill.rule() {
                                usvg::FillRule::NonZero => tiny_skia::FillRule::Winding,
                                usvg::FillRule::EvenOdd => tiny_skia::FillRule::EvenOdd,
                            },
                        });
                    }
                }
                _ => {}
            }
        }
    }
    walk(group, &mut out);
    out
}

fn undrawn(group: &usvg::Group) -> Option<&'static str> {
    group.children().iter().find_map(|node| match node {
        usvg::Node::Group(child)
            if child.clip_path().is_some()
                || child.mask().is_some()
                || !child.filters().is_empty()
                || child.opacity() != usvg::Opacity::ONE =>
        {
            Some("a clip path, mask, filter or group opacity")
        }
        usvg::Node::Group(child) if child.blend_mode() != usvg::BlendMode::Normal => {
            Some("a blend mode")
        }
        usvg::Node::Group(child) => undrawn(child),
        usvg::Node::Path(path) if !path.is_visible() => Some("a hidden element"),
        usvg::Node::Path(path) if path.stroke().is_some() => Some("a stroke"),
        usvg::Node::Path(path)
            if path
                .fill()
                .is_some_and(|fill| !matches!(fill.paint(), usvg::Paint::Color(_))) =>
        {
            Some("a gradient or pattern fill")
        }
        usvg::Node::Path(_) => None,
        usvg::Node::Image(_) => Some("an image"),
        usvg::Node::Text(_) => Some("text"),
    })
}

const SVG: &str = "http://www.w3.org/2000/svg";
const XLINK: &str = "http://www.w3.org/1999/xlink";
const XML: &str = "http://www.w3.org/XML/1998/namespace";

/// Elements that draw paths, or hold or place them.
const DRAWING: [&str; 12] = [
    "svg", "g", "path", "rect", "circle", "ellipse", "line", "polyline", "polygon", "defs", "use",
    "symbol",
];

/// Elements never drawn, whatever they hold.
const ANNOTATING: [&str; 3] = ["title", "desc", "metadata"];

/// The first thing the SVG `bytes` has beyond paths filled with a colour,
/// if any, read from its markup, as usvg drops some of it before its tree:
/// an element that is not a shape or a group, anything hidden, a clip,
/// mask or filter (even one that links to nothing), a conditional attribute,
/// or a link to another file.
///
/// Markup that cannot be read is refused too: it cannot be checked.
pub(crate) fn undrawn_markup(bytes: &[u8]) -> Option<String> {
    let options = usvg::roxmltree::ParsingOptions {
        allow_dtd: true,
        ..Default::default()
    };
    let Some(document) = std::str::from_utf8(bytes)
        .ok()
        .and_then(|text| usvg::roxmltree::Document::parse_with_options(text, options).ok())
    else {
        return Some("markup that cannot be checked".to_owned());
    };
    let annotation = |node: usvg::roxmltree::Node<'_, '_>| {
        node.is_element() && is_svg(node) && ANNOTATING.contains(&node.tag_name().name())
    };
    document
        .descendants()
        .filter(|node| node.is_element() && !node.ancestors().skip(1).any(annotation))
        .find_map(|node| undrawn_element(node, annotation(node)))
}

/// Whether usvg reads `node` as SVG: in its namespace, or in none.
fn is_svg(node: usvg::roxmltree::Node<'_, '_>) -> bool {
    matches!(node.tag_name().namespace(), None | Some(SVG))
}

fn undrawn_element(node: usvg::roxmltree::Node<'_, '_>, annotation: bool) -> Option<String> {
    let name = node.tag_name().name();
    if !annotation && !(is_svg(node) && DRAWING.contains(&name)) {
        return Some(match name {
            "image" => "an image".to_owned(),
            "foreignObject" => "a foreign object".to_owned(),
            "text" => "text".to_owned(),
            _ => format!("a `<{name}>` element"),
        });
    }
    properties(node)
        .find_map(|(name, value)| match name {
            "display" if value == "none" => Some("a hidden element"),
            "visibility" if matches!(value, "hidden" | "collapse") => Some("a hidden element"),
            "clip-path" | "mask" | "filter" if value != "none" => {
                Some("a clip path, mask or filter")
            }
            "systemLanguage" | "requiredExtensions" | "requiredFeatures" => {
                Some("a conditional attribute")
            }
            "href" if !value.starts_with('#') => Some("a link to another file"),
            // A paint svgtypes cannot read, usvg draws as the default; a link it can is in this file.
            "fill" | "stroke"
                if value.starts_with("url(") && svgtypes::Paint::from_str(value).is_err() =>
            {
                Some("a link to another file")
            }
            _ => None,
        })
        .map(str::to_owned)
}

/// `node`'s properties as usvg reads them, by local name:
/// its attributes in the namespaces usvg reads, then its inline style's declarations.
fn properties<'a>(node: usvg::roxmltree::Node<'a, '_>) -> impl Iterator<Item = (&'a str, &'a str)> {
    let attributes = node
        .attributes()
        .filter(|attribute| matches!(attribute.namespace(), None | Some(SVG | XLINK | XML)))
        .map(|attribute| (attribute.name(), attribute.value().trim()));
    let style = node
        .attributes()
        .find(|attribute| attribute.namespace().is_none() && attribute.name() == "style")
        .map(|attribute| attribute.value())
        .into_iter()
        .flat_map(simplecss::DeclarationTokenizer::from)
        .map(|declaration| (declaration.name, declaration.value.trim()));
    attributes.chain(style)
}

#[cfg(test)]
mod tests {
    use egui::emath::GuiRounding as _;

    use super::{Icon, Icons};

    const TWO_COLOUR: &[u8] = include_bytes!("../tests/fixtures/two-colour.svg");

    #[test]
    fn coloured_icons_preserve_fills_and_round_only_when_requested() {
        let square = Icon::from_svg_colored(TWO_COLOUR);
        let rounded = square.clone().rounded();
        for size in [[24, 24], [32, 16], [1, 1]] {
            let image = square
                .svg
                .rasterize(size, true, false, egui::Color32::GREEN);
            let crop = rounded
                .svg
                .rasterize(size, true, true, egui::Color32::GREEN);
            if size[0] > 1 {
                assert_eq!(
                    image[(size[0] as usize / 4, size[1] as usize / 2)],
                    egui::Color32::RED
                );
                assert_eq!(
                    image[(size[0] as usize * 3 / 4, size[1] as usize / 2)],
                    egui::Color32::BLUE
                );
                assert!(image[(0, 0)].a() > 0);
                assert_eq!(crop[(0, 0)].a(), 0);
                assert_eq!(crop[(size[0] as usize / 2, size[1] as usize / 2)].a(), 255);
                assert!(
                    crop.pixels
                        .iter()
                        .any(|pixel| pixel.a() > 0 && pixel.a() < 255)
                );
            }
        }
        assert_eq!(
            rounded.texture_hash(egui::Color32::RED),
            rounded.clone().rounded().texture_hash(egui::Color32::RED)
        );
        assert_ne!(
            square.texture_hash(egui::Color32::RED),
            rounded.texture_hash(egui::Color32::RED)
        );
    }

    #[test]
    fn fallible_constructors_match_the_panicking_ones() {
        let tint = egui::Color32::GREEN;
        for (fallible, panicking, colored) in [
            (
                Icon::try_from_svg(TWO_COLOUR),
                Icon::from_svg(TWO_COLOUR),
                false,
            ),
            (
                Icon::try_from_svg_colored(TWO_COLOUR),
                Icon::from_svg_colored(TWO_COLOUR),
                true,
            ),
        ] {
            let fallible = fallible.expect("valid SVG parses");
            assert_eq!(fallible.colored, colored);
            assert_eq!(fallible.texture_hash(tint), panicking.texture_hash(tint));
            assert_eq!(
                fallible.svg.rasterize([24, 24], colored, false, tint),
                panicking.svg.rasterize([24, 24], colored, false, tint)
            );
        }
    }

    #[test]
    fn fallible_constructors_reject_invalid_svg() {
        fn assert_error<E: std::error::Error + Send + Sync + 'static>(_: &E) {}
        for bytes in [b"\x00\xffnot an svg <<".as_slice(), b""] {
            for result in [Icon::try_from_svg(bytes), Icon::try_from_svg_colored(bytes)] {
                let Err(error) = result else {
                    panic!("{bytes:?} parsed as an icon");
                };
                assert_error(&error);
                assert_eq!(error.to_string(), "invalid icon SVG");
                assert!(std::error::Error::source(&error).is_some());
            }
        }
    }

    #[test]
    fn markup_that_cannot_be_read_is_not_taken_on_trust() {
        for bytes in [&b"\x1f\x8b\x08\x00\xff"[..], b"<svg"] {
            assert_eq!(
                super::undrawn_markup(bytes).as_deref(),
                Some("markup that cannot be checked"),
                "{bytes:?}"
            );
        }
    }

    #[test]
    fn a_hidden_path_is_not_drawn() {
        let icon = Icon::from_svg_colored(
            br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24">
            <path fill="#0000ff" d="M0 0h12v24H0z"/>
            <path fill="#ff0000" visibility="hidden" d="M0 0h24v24H0z"/>
        </svg>"##,
        );
        let image = icon
            .svg
            .rasterize([24, 24], true, false, egui::Color32::GREEN);
        assert_eq!(image[(6, 12)].to_array(), [0, 0, 255, 255]);
        assert_eq!(image[(18, 12)].to_array(), [0, 0, 0, 0]);
    }

    #[test]
    fn coloured_fills_composite_with_their_opacity() {
        let icon = Icon::from_svg_colored(
            br##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24">
            <path fill="#ff0000" fill-opacity="0.5" d="M0 0h24v24H0z"/>
            <path fill="#0000ff" fill-opacity="0.5" d="M12 0h12v24H12z"/>
        </svg>"##,
        );
        let image = icon
            .svg
            .rasterize([24, 24], true, false, egui::Color32::GREEN);
        let left = image[(6, 12)].to_array();
        let right = image[(18, 12)].to_array();
        assert_eq!(left, [128, 0, 0, 128]);
        assert_eq!(right, [64, 0, 128, 192]);
    }

    #[test]
    fn unsupported_fills_use_tint_without_tinting_solid_fills() {
        for definition in [
            r##"<linearGradient id="paint"><stop stop-color="red"/><stop offset="1" stop-color="blue"/></linearGradient>"##,
            r##"<pattern id="paint" width="1" height="1"><rect width="24" height="24" fill="red"/></pattern>"##,
        ] {
            let bytes = format!(
                r##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24"><defs>{definition}</defs>
                <path fill="url(#paint)" fill-opacity="0.5" d="M0 0h12v24H0z"/>
                <path fill="#0000ff" d="M12 0h12v24H12z"/></svg>"##
            );
            let icon = Icon::from_svg_colored(bytes.as_bytes());
            let image = icon
                .svg
                .rasterize([24, 24], true, false, egui::Color32::GREEN);
            assert_eq!(image[(6, 12)].to_array(), [0, 128, 0, 128]);
            assert_eq!(image[(18, 12)], egui::Color32::BLUE);
            assert_ne!(
                icon.texture_hash(egui::Color32::GREEN),
                icon.texture_hash(egui::Color32::RED)
            );
        }
    }

    #[test]
    fn colour_modes_and_crops_have_separate_textures() {
        let context = egui::Context::default();
        let mono = Icon::from_svg(TWO_COLOUR);
        let colour = Icon::from_svg_colored(TWO_COLOUR);
        let tint = egui::Color32::GREEN;
        let ids = [
            mono.texture(&context, [24, 24], tint).id(),
            colour.texture(&context, [24, 24], tint).id(),
            colour
                .clone()
                .rounded()
                .texture(&context, [24, 24], tint)
                .id(),
            mono.clone()
                .rounded()
                .texture(&context, [24, 24], tint)
                .id(),
        ];
        for (at, id) in ids.iter().enumerate() {
            assert!(!ids[..at].contains(id));
        }
        assert_eq!(
            colour.texture(&context, [24, 24], egui::Color32::RED).id(),
            ids[1]
        );
        assert_eq!(
            mono.texture(&context, [24, 24], egui::Color32::RED).id(),
            ids[0]
        );
    }

    #[test]
    fn monochrome_coverage_baseline() {
        // Captured from the unchanged v0.11.0 rasterizer; PNG stores premultiplied coverage bytes.
        let icon = Icon::from_svg(include_bytes!("../assets/icons/window-close.svg"));
        let image = icon
            .svg
            .rasterize([16, 16], false, false, egui::Color32::WHITE);
        let bytes: Vec<u8> = image
            .pixels
            .iter()
            .flat_map(|pixel| pixel.to_array())
            .collect();
        assert_eq!(
            bytes.as_slice(),
            image::load_from_memory(include_bytes!("../tests/snapshots/window-close-16.png"))
                .unwrap()
                .into_rgba8()
                .as_raw()
        );
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        use std::hash::{Hash as _, Hasher as _};
        include_bytes!("../assets/icons/window-close.svg")
            .as_slice()
            .hash(&mut hasher);
        assert_eq!(icon.texture_hash(egui::Color32::RED), hasher.finish());
    }

    #[test]
    fn bundled_icons_load_with_filled_paths() {
        let icons = Icons::load();
        for (name, icon) in [
            ("folder", &icons.folder),
            ("app", &icons.app),
            ("search", &icons.search),
            ("window-minimize", &icons.window_minimize),
            ("window-maximize", &icons.window_maximize),
            ("window-restore", &icons.window_restore),
            ("window-close", &icons.window_close),
        ] {
            assert!(
                !icon.svg.paths.is_empty(),
                "{name} contains no filled paths"
            );
        }
    }

    #[test]
    fn consumer_supplied_demo_icons_follow_the_same_svg_path() {
        for (name, bytes) in [
            (
                "theme-light",
                include_bytes!("../template/assets/theme-light.svg").as_slice(),
            ),
            (
                "theme-dark",
                include_bytes!("../template/assets/theme-dark.svg").as_slice(),
            ),
            (
                "language-english",
                include_bytes!("../template/assets/language-english.svg").as_slice(),
            ),
            (
                "language-finnish",
                include_bytes!("../template/assets/language-finnish.svg").as_slice(),
            ),
        ] {
            let icon = Icon::from_svg(bytes);
            assert!(
                !icon.svg.paths.is_empty(),
                "consumer icon {name} contains no filled paths"
            );
        }
    }

    #[test]
    fn icon_rasterization_is_antialiased_and_painting_stays_inside_its_rect() {
        let icon = Icon::from_svg(include_bytes!("../assets/icons/window-close.svg"));
        let image = icon
            .svg
            .rasterize([16, 16], false, false, egui::Color32::WHITE);
        assert!(
            image
                .pixels
                .iter()
                .any(|pixel| 0 < pixel.a() && pixel.a() < 255),
            "supersampling should leave partially covered edge pixels"
        );

        let context = egui::Context::default();
        let rect = egui::Rect::from_min_size(egui::pos2(12.25, 7.25), egui::Vec2::splat(16.0));
        let mut output = context.run_ui(egui::RawInput::default(), |ui| {
            icon.paint(ui.painter(), rect, egui::Color32::WHITE);
        });

        let rounded = rect.round_to_pixels(context.pixels_per_point());
        let painted = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Mesh(mesh) if mesh.texture_id != egui::TextureId::default() => {
                    Some(mesh)
                }
                _ => None,
            })
            .expect("icon paints one textured quad");
        assert!(
            painted
                .vertices
                .iter()
                .all(|vertex| rounded.contains(vertex.pos)),
            "icon texture vertices cannot escape their paint rect: {:?}",
            painted.vertices
        );
        assert_eq!(
            output
                .shapes
                .iter()
                .filter(|shape| matches!(&shape.shape, egui::Shape::Mesh(mesh) if mesh.texture_id == painted.texture_id))
                .count(),
            1,
            "one bounded image replaces triangle-by-triangle feathering"
        );
        output.textures_delta.clear();
    }
}
