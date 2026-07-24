//! SVG icons as egui meshes. `usvg` parses the SVG (renderer-independent); `lyon` tessellates its
//! fills into triangles that egui draws directly. No rasterizer — the output is geometry, so it stays
//! crisp at any size and carries to any future backend that can draw a mesh.

use lyon::path::Path as LyonPath;
use lyon::tessellation::{BuffersBuilder, FillOptions, FillTessellator, FillVertex, VertexBuffers};
use usvg::tiny_skia_path::PathSegment;

/// A tessellated monochrome icon, normalized to the unit square and recoloured when painted.
pub struct Icon {
    vertices: Vec<egui::Pos2>,
    indices: Vec<u32>,
}

impl Icon {
    /// Parse an SVG and tessellate its fill geometry. Fill colours are ignored — icons are monochrome
    /// and tinted at paint time.
    ///
    /// # Panics
    /// If the bytes aren't a valid SVG (the icons are compiled-in constants, so this is a build bug).
    #[must_use]
    pub fn from_svg(bytes: &[u8]) -> Self {
        let tree = usvg::Tree::from_data(bytes, &usvg::Options::default()).expect("parse icon SVG");
        let size = tree.size();
        let (width, height) = (size.width(), size.height());

        let mut geometry: VertexBuffers<egui::Pos2, u32> = VertexBuffers::new();
        let mut tessellator = FillTessellator::new();
        let mut buffers = BuffersBuilder::new(&mut geometry, |vertex: FillVertex| {
            egui::pos2(vertex.position().x, vertex.position().y)
        });

        for path in paths(tree.root()) {
            let outline = to_lyon(&path, width, height);
            let _ = tessellator.tessellate_path(&outline, &FillOptions::default(), &mut buffers);
        }

        Self {
            vertices: geometry.vertices,
            indices: geometry.indices,
        }
    }

    /// Allocate a `size`×`size` slot in `ui` and paint the icon into it, tinted `color`.
    pub fn show(&self, ui: &mut egui::Ui, size: f32, color: egui::Color32) {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
        self.paint(ui.painter(), rect, color);
    }

    /// Paint the icon into `rect`, tinted `color`.
    pub fn paint(&self, painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
        let vertices = self
            .vertices
            .iter()
            .map(|unit| egui::epaint::Vertex {
                pos: egui::pos2(
                    rect.min.x + unit.x * rect.width(),
                    rect.min.y + unit.y * rect.height(),
                ),
                uv: egui::epaint::WHITE_UV,
                color,
            })
            .collect();
        let mesh = egui::Mesh {
            vertices,
            indices: self.indices.clone(),
            ..Default::default()
        };
        painter.add(egui::Shape::mesh(mesh));
    }
}

/// The icons the shell paints, each tessellated once from a bundled SVG.
pub struct Icons {
    pub folder: Icon,
    pub app: Icon,
    pub search: Icon,
}

impl Icons {
    #[must_use]
    pub fn load() -> Self {
        Self {
            folder: Icon::from_svg(include_bytes!("../assets/icons/folder.svg")),
            app: Icon::from_svg(include_bytes!("../assets/icons/app.svg")),
            search: Icon::from_svg(include_bytes!("../assets/icons/search.svg")),
        }
    }
}

/// Every fill path in the tree, with each node's absolute transform, flattened out of the groups.
fn paths(group: &usvg::Group) -> Vec<usvg::Path> {
    let mut out = Vec::new();
    fn walk(group: &usvg::Group, out: &mut Vec<usvg::Path>) {
        for node in group.children() {
            match node {
                usvg::Node::Group(child) => walk(child, out),
                usvg::Node::Path(path) => out.push((**path).clone()),
                _ => {}
            }
        }
    }
    walk(group, &mut out);
    out
}

/// Convert a usvg path to a lyon path: apply its absolute transform, then normalize to the unit square.
fn to_lyon(path: &usvg::Path, width: f32, height: f32) -> LyonPath {
    let t = path.abs_transform();
    let map = |x: f32, y: f32| {
        let px = t.sx * x + t.kx * y + t.tx;
        let py = t.ky * x + t.sy * y + t.ty;
        lyon::math::point(px / width, py / height)
    };

    let mut builder = LyonPath::builder();
    let mut open = false;
    for segment in path.data().segments() {
        match segment {
            PathSegment::MoveTo(p) => {
                if open {
                    builder.end(false);
                }
                builder.begin(map(p.x, p.y));
                open = true;
            }
            PathSegment::LineTo(p) => {
                builder.line_to(map(p.x, p.y));
            }
            PathSegment::QuadTo(c, p) => {
                builder.quadratic_bezier_to(map(c.x, c.y), map(p.x, p.y));
            }
            PathSegment::CubicTo(c1, c2, p) => {
                builder.cubic_bezier_to(map(c1.x, c1.y), map(c2.x, c2.y), map(p.x, p.y));
            }
            PathSegment::Close => {
                builder.end(true);
                open = false;
            }
        }
    }
    if open {
        builder.end(false);
    }
    builder.build()
}

#[cfg(test)]
mod tests {
    use super::Icons;

    #[test]
    fn bundled_icons_load_and_tessellate() {
        // A stroke-only icon (`fill="none"`) parses fine and tessellates to nothing, so check every
        // one produces geometry rather than trusting `load` not to panic.
        let icons = Icons::load();
        for (name, icon) in [
            ("folder", &icons.folder),
            ("app", &icons.app),
            ("search", &icons.search),
        ] {
            assert!(!icon.indices.is_empty(), "{name} tessellated to nothing");
        }
    }
}
