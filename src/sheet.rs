//! Every capture of a run on one image.
//!
//! The packing itself is `rectangle-pack`'s. What's here is the search for a sheet worth looking at,
//! and the drawing — which goes through egui, so a caption is set in the shell's own fonts
//! and needs no rasteriser here.

use std::{cmp::Reverse, collections::BTreeMap};

use rectangle_pack::{
    GroupedRectsToPlace, RectToInsert, TargetBin, contains_smallest_box, pack_rects,
    volume_heuristic,
};

use crate::{
    HEADER_BG, PANEL_BG,
    diagnostic::Diagnostic,
    render::{Size, open},
};

/// One capture, waiting to be placed.
pub(crate) struct Panel {
    pub(crate) name: String,
    pub(crate) image: image::RgbaImage,
}

/// Space around every panel, and the band above it the caption is drawn into.
const GUTTER: u32 = 12;
const CAPTION: u32 = 22;

/// Where one panel's cell sits, in pixels from the top left of the sheet.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Cell {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

/// A sheet with a place on it for every panel it was packed from.
pub(crate) struct Packed {
    width: u32,
    height: u32,
    /// One per panel, in the order the panels were given.
    cells: Vec<Cell>,
}

/// The room a panel takes: the caption band above it, and the gutter off its right and bottom
/// that keeps two captures from reading as one picture.
fn cell_size(panel: &Panel) -> Size {
    Size {
        width: panel.image.width() + GUTTER,
        height: panel.image.height() + CAPTION + GUTTER,
    }
}

/// The smallest sheet the packer will take these panels on.
///
/// Every candidate width is squeezed to the shortest sheet that still
/// holds everything, and the best of those is kept. The widths are tried
/// rather than searched because a narrower sheet is not a smaller one —
/// a column fits the narrowest sheet of all, and covers the least of it.
///
/// `None` if there are no panels, or if the packer takes none of the widths offered.
pub(crate) fn pack(panels: &[Panel]) -> Option<Packed> {
    let cells: Vec<Size> = panels.iter().map(cell_size).collect();
    widths(&cells)
        .into_iter()
        .filter_map(|width| shortest(&cells, width))
        .min_by(|one, two| cost(one).total_cmp(&cost(two)))
}

/// The proportions a sheet gets read at, near enough: a screen.
const TARGET: f64 = 16.0 / 10.0;

fn area(packed: &Packed) -> u64 {
    u64::from(packed.width) * u64::from(packed.height)
}

/// Smaller, and shaped like the thing it will be read on. Lower is better.
///
/// Area alone picks unreviewable extremes. A tall column and a long strip hold the same panels
/// over much the same page, and both are area-optimal — yet either one, scaled to fit a screen,
/// leaves every panel too small to read.
///
/// Weighting the area by how far the sheet sits off a screen's proportions separates them.
/// Every candidate holds the same panels, so the areas sit close together and the shape decides.
fn cost(packed: &Packed) -> f64 {
    let ratio = f64::from(packed.width) / f64::from(packed.height);
    area(packed) as f64 * (ratio / TARGET).max(TARGET / ratio)
}

/// The shortest sheet of this width that still holds every cell.
///
/// The height has to be squeezed rather than left generous: given room for a column the packer will
/// lay one out and never reach across the width, and cropping that back would make every width come
/// out the same sheet.
fn shortest(cells: &[Size], width: u32) -> Option<Packed> {
    let mut short = cells.iter().map(|cell| cell.height).max()?;
    // A column always fits, so this is a height the search can close in on rather than test.
    let mut tall = cells.iter().map(|cell| cell.height).sum();
    while short < tall {
        let between = short + (tall - short) / 2;
        match place(cells, width, between) {
            Some(_) => tall = between,
            None => short = between + 1,
        }
    }
    crop(place(cells, width, short)?)
}

/// The widths worth packing into, since a sheet's width is always some run of panels side by side.
///
/// Three sources, since no one of them covers the range.
/// Every panel and every pair, for the arrangements a couple of panels wide.
/// Every run of the widest panels, because a dozen shots read best several across
/// and no pair reaches that far.
/// And the width a flawless pack would have at the shape we want, which usually falls between runs.
///
/// All quadratic in the number of shots, so the whole search stays cheap.
fn widths(cells: &[Size]) -> Vec<u32> {
    let across: Vec<u32> = cells.iter().map(|cell| cell.width).collect();
    let widest = across.iter().copied().max().unwrap_or_default();

    let pairs = across
        .iter()
        .enumerate()
        .flat_map(|(at, w)| across[at..].iter().map(move |other| w + other));

    let mut widest_first = across.clone();
    widest_first.sort_unstable_by(|a, b| b.cmp(a));
    let runs = widest_first.iter().scan(0u32, |run, w| {
        *run += w;
        Some(*run)
    });

    let area: u64 = cells
        .iter()
        .map(|cell| u64::from(cell.width) * u64::from(cell.height))
        .sum();
    let ideal = ((area as f64) * TARGET).sqrt() as u32;

    let mut widths: Vec<u32> = across
        .iter()
        .copied()
        .chain(pairs)
        .chain(runs)
        .chain(std::iter::once(ideal))
        // Under the widest panel nothing can be laid out at all.
        .filter(|width| *width >= widest)
        .collect();
    widths.sort_unstable();
    widths.dedup();
    widths
}

/// Trim the sheet to the panels on it, and open it up by the one gutter the cells don't carry:
/// they hold theirs off their right and bottom, so without this the leftmost and topmost panels
/// would sit flush against the edge.
fn crop(cells: Vec<Cell>) -> Option<Packed> {
    Some(Packed {
        width: cells.iter().map(|c| c.x + c.width).max()? + GUTTER,
        height: cells.iter().map(|c| c.y + c.height).max()? + GUTTER,
        cells,
    })
}

/// Lay `cells` out on one `width` × `height` sheet, or `None` when they don't all fit.
///
/// Two cells of a size are interchangeable to a packer, which may separate them in whatever order
/// it happens to hold them. Sorting first makes that order ours rather than its, so the same panels
/// land in the same places whatever the packer does inside.
/// Reading back by our own ids then returns them in the caller's order.
fn place(cells: &[Size], width: u32, height: u32) -> Option<Vec<Cell>> {
    let mut order: Vec<usize> = (0..cells.len()).collect();
    order.sort_by_key(|at| {
        let cell = cells[*at];
        (
            Reverse(u64::from(cell.width) * u64::from(cell.height)),
            Reverse(cell.height),
            Reverse(cell.width),
            *at,
        )
    });

    let mut wanted = GroupedRectsToPlace::<usize, ()>::new();
    for (rank, at) in order.iter().enumerate() {
        let cell = cells[*at];
        wanted.push_rect(rank, None, RectToInsert::new(cell.width, cell.height, 1));
    }
    let mut sheet = BTreeMap::new();
    sheet.insert((), TargetBin::new(width, height, 1));
    let packed = pack_rects(
        &wanted,
        &mut sheet,
        &volume_heuristic,
        &contains_smallest_box,
    )
    .ok()?;

    let placements = packed.packed_locations();
    let mut placed = vec![None; cells.len()];
    for (rank, at) in order.iter().enumerate() {
        let (_, spot) = placements.get(&rank)?;
        placed[*at] = Some(Cell {
            x: spot.x(),
            y: spot.y(),
            width: spot.width(),
            height: spot.height(),
        });
    }
    placed.into_iter().collect()
}

/// Draw the panels onto one image, each captioned with the shot that produced it.
///
/// # Errors
/// If the panels can't be packed, or the sheet can't be drawn on this renderer.
pub(crate) fn compose(
    panels: Vec<Panel>,
    session: &crate::render::Session,
    setup: &impl Fn(&egui::Context),
) -> Result<image::RgbaImage, Diagnostic> {
    let packed = pack(&panels).ok_or_else(|| Diagnostic::new("these captures will not pack"))?;
    let size = egui::vec2(packed.width as f32, packed.height as f32);
    let mut harness = open(size, session, setup, |cc, _| {
        Sheet::new(&cc.egui_ctx, panels, packed)
    })?;
    harness.run_steps(1);
    harness
        .render()
        .map_err(|reason| Diagnostic::from(format!("draw the sheet: {reason}")))
}

/// The sheet as the entire app: every panel where the packer put it, and nothing else.
struct Sheet {
    panels: Vec<(String, egui::TextureHandle, Cell)>,
}

impl Sheet {
    fn new(ctx: &egui::Context, panels: Vec<Panel>, packed: Packed) -> Self {
        let panels = panels
            .into_iter()
            .zip(packed.cells)
            .map(|(panel, cell)| {
                let size = [panel.image.width() as usize, panel.image.height() as usize];
                let pixels = egui::ColorImage::from_rgba_unmultiplied(size, panel.image.as_raw());
                // Nearest, because a panel is drawn at the size it was captured: any filtering here
                // would soften pixels that a reference comparison expects back unchanged.
                let texture = ctx.load_texture(&panel.name, pixels, egui::TextureOptions::NEAREST);
                (panel.name, texture, cell)
            })
            .collect();
        Self { panels }
    }
}

impl eframe::App for Sheet {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(HEADER_BG))
            .show(ui, |ui| {
                let caption = ui.visuals().weak_text_color();
                let painter = ui.painter();
                for (name, texture, cell) in &self.panels {
                    let left = (cell.x + GUTTER) as f32;
                    let top = (cell.y + GUTTER) as f32;
                    painter.text(
                        egui::pos2(left, top + (CAPTION as f32) / 2.0),
                        egui::Align2::LEFT_CENTER,
                        name,
                        egui::FontId::proportional(13.0),
                        caption,
                    );
                    let panel = egui::Rect::from_min_size(
                        egui::pos2(left, top + CAPTION as f32),
                        texture.size_vec2(),
                    );
                    // The panels carry their own background, so this only guards against a capture
                    // with transparency letting the sheet show through it.
                    painter.rect_filled(panel, 0.0, PANEL_BG);
                    painter.image(
                        texture.id(),
                        panel,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Panels of `sizes`, named by their place in the list.
    fn panels(sizes: &[(u32, u32)]) -> Vec<Panel> {
        sizes
            .iter()
            .enumerate()
            .map(|(at, (w, h))| Panel {
                name: format!("panel-{at}"),
                image: image::RgbaImage::new(*w, *h),
            })
            .collect()
    }

    #[test]
    fn every_panel_gets_a_cell_and_no_two_of_them_overlap() {
        let packed = pack(&panels(&[(640, 360), (640, 360), (480, 240), (480, 480)]))
            .expect("four panels pack");
        assert_eq!(packed.cells.len(), 4);
        for (at, cell) in packed.cells.iter().enumerate() {
            assert!(
                cell.x + cell.width <= packed.width && cell.y + cell.height <= packed.height,
                "cell {at} is on the sheet: {cell:?} of {}×{}",
                packed.width,
                packed.height
            );
            for other in &packed.cells[..at] {
                let apart = cell.x + cell.width <= other.x
                    || other.x + other.width <= cell.x
                    || cell.y + cell.height <= other.y
                    || other.y + other.height <= cell.y;
                assert!(apart, "{cell:?} and {other:?} do not overlap");
            }
        }
    }

    /// How much of the sheet is panel rather than nothing, in percent.
    fn covered(packed: &Packed) -> u64 {
        let panels: u64 = packed
            .cells
            .iter()
            .map(|c| u64::from(c.width) * u64::from(c.height))
            .sum();
        panels * 100 / (u64::from(packed.width) * u64::from(packed.height))
    }

    #[test]
    fn a_mixed_capture_packs_densely_and_into_a_shape_that_can_be_looked_at() {
        // The demo's own capture. A single column of these is the smallest sheet by area,
        // and also two and a half screens tall — the shape has to count for something.
        let packed =
            pack(&panels(&[(640, 360), (640, 360), (480, 480), (480, 240)])).expect("panels pack");
        assert!(
            covered(&packed) >= 80,
            "the sheet is {}% panels at {}×{}",
            covered(&packed),
            packed.width,
            packed.height
        );
        assert!(
            packed.height < packed.width * 2,
            "{}×{} is not a strip",
            packed.width,
            packed.height
        );
    }

    /// A sheet's width over its height.
    fn ratio(packed: &Packed) -> f64 {
        f64::from(packed.width) / f64::from(packed.height)
    }

    #[test]
    fn a_dozen_tall_shots_land_near_the_shape_of_a_screen() {
        // Reported: twelve of these packed to a 1:2 column,
        // and changing one panel's height turned it into a 6.3:1 strip.
        //
        // Both are area-optimal and neither can be read once scaled to a screen,
        // so area cannot be the whole objective — and a dozen panels want several
        // across, which  no pair of widths reaches.
        let mut sizes = vec![(420, 780); 11];
        sizes.push((420, 1000));
        for sizes in [&sizes[..11], &sizes[..]] {
            let packed = pack(&panels(sizes)).expect("panels pack");
            assert!(
                (0.8..=2.6).contains(&ratio(&packed)),
                "{}×{} is {:.2}:1, near enough a screen to review",
                packed.width,
                packed.height,
                ratio(&packed)
            );
        }
    }

    #[test]
    fn panels_of_one_size_tile_into_a_block_rather_than_a_strip() {
        // Four of a size cover the same area however they are arranged, so nothing but the shape
        // separates a 2×2 from a single column — and only one of those can be looked at.
        let packed = pack(&panels(&[(300, 200); 4])).expect("panels pack");
        assert!(
            packed.width * 2 > packed.height && packed.height * 2 > packed.width,
            "{}×{} is a block, not a strip",
            packed.width,
            packed.height
        );
    }

    #[test]
    fn the_sheet_is_cropped_to_its_panels_rather_than_left_square() {
        // A single row of wide panels: a square big enough to hold them would be mostly empty.
        let packed = pack(&panels(&[(600, 80), (600, 80)])).expect("two panels pack");
        assert!(
            packed.height < packed.width,
            "{}×{} keeps no empty square below the panels",
            packed.width,
            packed.height
        );
    }

    #[test]
    fn packing_the_same_panels_twice_puts_them_in_the_same_places() {
        // Equal-sized panels tie on every heuristic the packer sorts by, so if the order it walked
        // them in were a hash order this would come back different each time.
        let sizes = [(320, 200), (320, 200), (320, 200), (320, 200)];
        let first = pack(&panels(&sizes)).expect("panels pack");
        let again = pack(&panels(&sizes)).expect("panels pack");
        assert_eq!(first.cells, again.cells);
        assert_eq!((first.width, first.height), (again.width, again.height));
    }

    #[test]
    fn a_sheet_leaves_room_for_the_caption_above_every_panel() {
        let packed = pack(&panels(&[(100, 100), (100, 100)])).expect("panels pack");
        for cell in &packed.cells {
            assert_eq!(
                cell.height,
                100 + CAPTION + GUTTER,
                "{cell:?} has a caption"
            );
        }
    }

    #[test]
    fn nothing_to_pack_is_no_sheet() {
        assert!(pack(&[]).is_none());
    }
}
