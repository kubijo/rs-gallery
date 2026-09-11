//! SVG icon masks cached by physical display size.

use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash as _, Hasher as _},
    sync::Arc,
};

use egui::emath::GuiRounding as _;

const SUPERSAMPLE: u32 = 4;
const RASTER_MARGIN: f32 = 1.0;
const MAX_RASTER_SIDE: f32 = 512.0;

/// A monochrome SVG icon tinted when painted.
#[derive(Clone)]
pub struct Icon {
    svg: Arc<Svg>,
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
            svg: Arc::new(Svg {
                hash: hasher.finish(),
                width: size.width(),
                height: size.height(),
                paths: paths(tree.root()),
            }),
        }
    }

    /// Allocate a `size`×`size` slot and paint the icon in `color`.
    pub fn show(&self, ui: &mut egui::Ui, size: f32, color: egui::Color32) {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
        self.paint(ui.painter(), rect, color);
    }

    /// Paint the icon into `rect`, tinted `color`.
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
        let texture = self.texture(painter.ctx(), pixel_size);
        painter.with_clip_rect(rect).image(
            texture.id(),
            rect,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            color,
        );
    }

    fn texture(&self, context: &egui::Context, pixel_size: [u32; 2]) -> egui::TextureHandle {
        let cache_id = egui::Id::new(("gallery-svg-icon", self.svg.hash, pixel_size));
        if let Some(texture) = context.data(|data| data.get_temp(cache_id)) {
            return texture;
        }

        let texture = context.load_texture(
            format!(
                "gallery-svg-icon-{:016x}-{}x{}",
                self.svg.hash, pixel_size[0], pixel_size[1]
            ),
            self.svg.rasterize(pixel_size),
            egui::TextureOptions::NEAREST,
        );
        context.data_mut(|data| data.insert_temp(cache_id, texture.clone()));
        texture
    }
}

impl Svg {
    fn rasterize(&self, size: [u32; 2]) -> egui::ColorImage {
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
        for path in &self.paths {
            let transform = tiny_skia::Transform::from_row(
                path.transform.sx * scale_x,
                path.transform.ky * scale_y,
                path.transform.kx * scale_x,
                path.transform.sy * scale_y,
                path.transform.tx * scale_x + RASTER_MARGIN,
                path.transform.ty * scale_y + RASTER_MARGIN,
            );
            pixmap.fill_path(&path.data, &paint, path.rule, transform, None);
        }

        let mut pixels = Vec::with_capacity((size[0] * size[1] * 4) as usize);
        for y in 0..size[1] {
            for x in 0..size[0] {
                let mut alpha = 0_u32;
                for sample_y in 0..SUPERSAMPLE {
                    for sample_x in 0..SUPERSAMPLE {
                        let sx = x * SUPERSAMPLE + sample_x;
                        let sy = y * SUPERSAMPLE + sample_y;
                        let at = ((sy * supersampled[0] + sx) * 4 + 3) as usize;
                        alpha += u32::from(pixmap.data()[at]);
                    }
                }
                let alpha = (alpha / (SUPERSAMPLE * SUPERSAMPLE)) as u8;
                pixels.extend_from_slice(&[alpha, alpha, alpha, alpha]);
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
        let image = icon.svg.rasterize([16, 16]);
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
