//! SVG icons cached by physical display size.

use std::{
    collections::hash_map::DefaultHasher,
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

struct Svg {
    hash: u64,
    width: f32,
    height: f32,
    paths: Vec<FillPath>,
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
    /// If the supplied bytes are not a valid SVG.
    #[must_use]
    pub fn from_svg(bytes: &[u8]) -> Self {
        let tree = usvg::Tree::from_data(bytes, &usvg::Options::default()).expect("parse icon SVG");
        let size = tree.size();
        let mut hasher = DefaultHasher::new();
        bytes.hash(&mut hasher);

        Self {
            colored: false,
            rounded: false,
            svg: Arc::new(Svg {
                hash: hasher.finish(),
                width: size.width(),
                height: size.height(),
                paths: paths(tree.root()),
            }),
        }
    }

    /// Parse filled SVG paths with their colours and fill opacity.
    ///
    /// Gradients and patterns use the paint call's tint instead.
    ///
    /// # Panics
    /// If the supplied bytes are not a valid SVG.
    #[must_use]
    pub fn from_svg_colored(bytes: &[u8]) -> Self {
        Self {
            colored: true,
            ..Self::from_svg(bytes)
        }
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
                usvg::Node::Path(path) => {
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
