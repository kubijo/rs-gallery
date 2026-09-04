//! Every capture of a run on one image.
//!
//! The packing itself is `binpack2d`'s. What's here is the search for a sheet worth looking at,
//! and the drawing — which goes through egui, so a caption is set in the shell's own fonts
//! and needs no rasteriser here.

use binpack2d::{
    Dimension,
    maxrects::{Heuristic, MaxRectsBin},
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
#[derive(Debug)]
pub(crate) struct Packed {
    width: u32,
    height: u32,
    /// One per panel, in the order the panels were given.
    cells: Vec<Cell>,
}

/// A valid sheet and the one uniform scale applied to its panel copies.
#[derive(Debug)]
struct Layout {
    packed: Packed,
    panel_sizes: Vec<Size>,
    scale: f64,
}

/// The room a panel takes: the caption band above it, and the gutter off its right and bottom
/// that keeps two captures from reading as one picture.
fn panel_sizes(panels: &[Panel]) -> Vec<Size> {
    panels
        .iter()
        .map(|panel| Size {
            width: panel.image.width(),
            height: panel.image.height(),
        })
        .collect()
}

fn cell_sizes(panels: &[Size]) -> Option<Vec<Size>> {
    panels
        .iter()
        .map(|panel| {
            Some(Size {
                width: panel.width.checked_add(GUTTER)?,
                height: panel.height.checked_add(CAPTION)?.checked_add(GUTTER)?,
            })
        })
        .collect()
}

/// The smallest sheet the packer will take these panels on.
///
/// Every candidate width is squeezed to the shortest sheet that still
/// holds everything, and the best of those is kept. The widths are tried
/// rather than searched because a narrower sheet is not a smaller one —
/// a column fits the narrowest sheet of all, and covers the least of it.
///
/// If the unscaled panels do not fit, only their copies on the sheet are uniformly reduced. The
/// individual captures are already written by then and remain at their requested dimensions.
///
/// # Errors
/// If even one-pixel panel copies and their unscaled captions/gutters cannot fit within `limit`.
fn layout(panels: &[Size], limit: u32) -> Result<Layout, Diagnostic> {
    if panels.is_empty() {
        return Err(Diagnostic::new("there are no captures to pack"));
    }
    if let Some(packed) = pack_sizes(panels, limit) {
        return Ok(Layout {
            packed,
            panel_sizes: panels.to_vec(),
            scale: 1.0,
        });
    }

    // Find any fit first, then keep the largest known fit. Dimensions are rounded at every probe,
    // so what is proved to fit here is exactly what will be resized and uploaded later.
    let mut upper = 1.0;
    let mut scale = 0.5;
    let (mut lower, mut scaled, mut packed) = loop {
        let scaled = scale_sizes(panels, scale);
        if let Some(packed) = pack_sizes(&scaled, limit) {
            break (scale, scaled, packed);
        }
        if scaled
            .iter()
            .all(|size| size.width == 1 && size.height == 1)
        {
            return Err(Diagnostic::new(format!(
                "{} captures cannot fit on a {limit}×{limit} sheet even after scaling",
                panels.len()
            ))
            .hint("write the captures without `sheet`, or gather fewer shots per recipe"));
        }
        upper = scale;
        scale /= 2.0;
    };

    for _ in 0..20 {
        let probe = (lower + upper) / 2.0;
        let probe_sizes = scale_sizes(panels, probe);
        if let Some(probe_packed) = pack_sizes(&probe_sizes, limit) {
            lower = probe;
            scaled = probe_sizes;
            packed = probe_packed;
        } else {
            upper = probe;
        }
    }
    Ok(Layout {
        packed,
        panel_sizes: scaled,
        scale: lower,
    })
}

fn scale_sizes(panels: &[Size], scale: f64) -> Vec<Size> {
    panels
        .iter()
        .map(|panel| Size {
            width: (f64::from(panel.width) * scale).round().max(1.0) as u32,
            height: (f64::from(panel.height) * scale).round().max(1.0) as u32,
        })
        .collect()
}

/// Search only bins whose cropped result can fit on `limit` along both axes.
fn pack_sizes(panels: &[Size], limit: u32) -> Option<Packed> {
    // `crop` adds the leading/top gutter after the bin has been packed.
    let bin_limit = limit.checked_sub(GUTTER)?;
    let cells = cell_sizes(panels)?;
    let widths = widths(&cells, bin_limit);
    RULES
        .iter()
        .flat_map(|rule| widths.iter().map(move |width| (*rule, *width)))
        .filter_map(|(rule, width)| shortest(&cells, width, bin_limit, rule))
        .filter(|packed| packed.width <= limit && packed.height <= limit)
        .min_by(|one, two| cost(one).total_cmp(&cost(two)))
}

/// Every rule the packer can seat a panel by.
///
/// None of them dominates — a rule that seats a tall panel well leaves a wide one stranded —
/// and they are cheap enough to run all of and score after.
const RULES: [Heuristic; 5] = [
    Heuristic::BestShortSideFit,
    Heuristic::BestLongSideFit,
    Heuristic::BestAreaFit,
    Heuristic::BottomLeftRule,
    Heuristic::ContactPointRule,
];

/// The proportions a sheet gets read at, near enough: a screen.
const TARGET: f64 = 16.0 / 10.0;

fn area(packed: &Packed) -> u64 {
    u64::from(packed.width) * u64::from(packed.height)
}

/// Smaller, shaped like the thing it will be read on, and covered in panels. Lower is better.
///
/// Area alone picks unreviewable extremes. A tall column and a long strip hold the same panels
/// over much the same page, and both are area-optimal — yet either one, scaled to fit a screen,
/// leaves every panel too small to read.
///
/// Weighting the area by how far the sheet sits off a screen's proportions separates them.
/// That alone cannot see a hole, though: `area × (TARGET / ratio)` is `TARGET × height²`, so a sheet
/// taller than a screen cancels its own width and the slack beside its panels is free.
/// Dividing by the share its panels cover puts that cancelled side back.
fn cost(packed: &Packed) -> f64 {
    let ratio = f64::from(packed.width) / f64::from(packed.height);
    let shape = (ratio / TARGET).max(TARGET / ratio);
    area(packed) as f64 * shape / covered(packed)
}

/// The share of the sheet its panels cover, from nothing to all of it.
fn covered(packed: &Packed) -> f64 {
    let panels: u64 = packed
        .cells
        .iter()
        .map(|cell| u64::from(cell.width) * u64::from(cell.height))
        .sum();
    panels as f64 / area(packed) as f64
}

/// The shortest sheet of this width that still holds every cell.
///
/// The height has to be squeezed rather than left generous: given room for a column the packer will
/// lay one out and never reach across the width, and cropping that back would make every width come
/// out the same sheet.
fn shortest(cells: &[Size], width: u32, max_height: u32, rule: Heuristic) -> Option<Packed> {
    let mut short = cells.iter().map(|cell| cell.height).max()?;
    if short > max_height {
        return None;
    }
    // A column always fits, so this is a height the search can close in on rather than test.
    let mut tall = cells
        .iter()
        .map(|cell| u64::from(cell.height))
        .sum::<u64>()
        .min(u64::from(max_height)) as u32;
    // The packer is greedy, so a height it packs is no promise that every taller one does too.
    // Keeping the last that placed stops the search from dropping a width that demonstrably works.
    let mut fits = None;
    while short < tall {
        let between = short + (tall - short) / 2;
        match place(cells, width, between, rule) {
            Some(placed) => {
                fits = Some(placed);
                tall = between;
            }
            None => short = between + 1,
        }
    }
    crop(place(cells, width, short, rule).or(fits)?)
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
fn widths(cells: &[Size], max_width: u32) -> Vec<u32> {
    let across: Vec<u32> = cells.iter().map(|cell| cell.width).collect();
    let widest = across.iter().copied().max().unwrap_or_default();

    let pairs = across.iter().enumerate().flat_map(|(at, w)| {
        across[at..]
            .iter()
            .filter_map(move |other| w.checked_add(*other))
    });

    let mut widest_first = across.clone();
    widest_first.sort_unstable_by(|a, b| b.cmp(a));
    let runs = widest_first.iter().scan(0u64, |run, w| {
        *run += u64::from(*w);
        u32::try_from(*run).ok()
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
        // Trying the backend ceiling itself keeps an otherwise-valid layout from being missed
        // merely because no panel-width sum happened to land near it.
        .chain(std::iter::once(max_width))
        // Under the widest panel nothing can be laid out at all.
        .filter(|width| *width >= widest && *width <= max_width)
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
        width: cells
            .iter()
            .filter_map(|c| c.x.checked_add(c.width))
            .max()?
            .checked_add(GUTTER)?,
        height: cells
            .iter()
            .filter_map(|c| c.y.checked_add(c.height))
            .max()?
            .checked_add(GUTTER)?,
        cells,
    })
}

/// Lay `cells` out on one `width` × `height` sheet, or `None` when they don't all fit.
///
/// Each cell carries its place in `cells` as the id the packer hands back,
/// so the placements return in the caller's order however it reordered them to seat them.
fn place(cells: &[Size], width: u32, height: u32, rule: Heuristic) -> Option<Vec<Cell>> {
    let wanted: Vec<Dimension> = cells
        .iter()
        .enumerate()
        .map(|(at, cell)| {
            Some(Dimension::with_id(
                isize::try_from(at).ok()?,
                i32::try_from(cell.width).ok()?,
                i32::try_from(cell.height).ok()?,
                0,
            ))
        })
        .collect::<Option<_>>()?;

    let mut sheet = MaxRectsBin::new(i32::try_from(width).ok()?, i32::try_from(height).ok()?);
    let (placements, rejected) = sheet.insert_list(&wanted, rule);
    if !rejected.is_empty() {
        return None;
    }

    let mut placed = vec![None; cells.len()];
    for spot in placements {
        let at = usize::try_from(spot.id()).ok()?;
        *placed.get_mut(at)? = Some(Cell {
            x: u32::try_from(spot.x()).ok()?,
            y: u32::try_from(spot.y()).ok()?,
            width: u32::try_from(spot.width()).ok()?,
            height: u32::try_from(spot.height()).ok()?,
        });
    }
    placed.into_iter().collect()
}

/// Draw the panels onto one image, each captioned with the shot that produced it.
///
/// # Errors
/// If the panels can't be packed, or the sheet can't be drawn on this renderer.
pub(crate) fn compose(
    mut panels: Vec<Panel>,
    session: &crate::render::Session,
    setup: &impl Fn(&egui::Context),
) -> Result<image::RgbaImage, Diagnostic> {
    let limit = session.max_texture_dimension_2d();
    let Layout {
        packed,
        panel_sizes,
        scale,
    } = layout(&panel_sizes(&panels), limit)?;
    if scale < 1.0 {
        for (panel, size) in panels.iter_mut().zip(panel_sizes) {
            panel.image = image::imageops::resize(
                &panel.image,
                size.width,
                size.height,
                image::imageops::FilterType::Lanczos3,
            );
        }
    }
    let packed_size = Size {
        width: packed.width,
        height: packed.height,
    };
    let size = egui::vec2(packed.width as f32, packed.height as f32);
    // One for one: the packer measured the panels in pixels, so the sheet lays out
    // in as many points and each capture lands on it at the size it was taken.
    let mut harness = open(size, 1.0, session, setup, |cc, _| {
        Sheet::new(cc, panels, packed)
    })?;
    let wgpu = harness.state().wgpu.clone();
    render_with_backend_errors(wgpu.as_ref(), packed_size, limit, || {
        harness.run_steps(1);
        harness
            .render()
            .map_err(|reason| Diagnostic::from(format!("draw the sheet: {reason}")))
    })
}

/// Turn renderer failures into the capture command's structured error path. Wgpu error scopes keep
/// its uncaptured-error handler from panicking first; the unwind guard covers renderer-side panics.
fn render_with_backend_errors<T>(
    state: Option<&eframe::egui_wgpu::RenderState>,
    size: Size,
    limit: u32,
    render: impl FnOnce() -> Result<T, Diagnostic>,
) -> Result<T, Diagnostic> {
    let scopes = state.map(|state| {
        use eframe::egui_wgpu::wgpu::ErrorFilter;
        // Error scopes are a stack and must be popped in reverse order.
        [
            state.device.push_error_scope(ErrorFilter::OutOfMemory),
            state.device.push_error_scope(ErrorFilter::Internal),
            state.device.push_error_scope(ErrorFilter::Validation),
        ]
    });
    let rendered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(render))
        .map_err(|panic| {
            Diagnostic::new(format!(
                "the sheet renderer panicked: {}",
                crate::render::panic_message(&panic)
            ))
        })
        .and_then(std::convert::identity);
    let mut error = None;
    if let Some(scopes) = scopes {
        for scope in scopes.into_iter().rev() {
            if let Some(found) = pollster::block_on(scope.pop()) {
                error.get_or_insert(found);
            }
        }
    }
    if let Some(error) = error {
        return Err(Diagnostic::new(format!(
            "wgpu rejected the {}×{} sheet within its {limit}×{limit} limit: {error}",
            size.width, size.height
        )));
    }
    rendered
}

/// The sheet as the entire app: every panel where the packer put it, and nothing else.
struct Sheet {
    panels: Vec<(String, egui::TextureHandle, Cell)>,
    wgpu: Option<eframe::egui_wgpu::RenderState>,
}

impl Sheet {
    fn new(cc: &eframe::CreationContext<'_>, panels: Vec<Panel>, packed: Packed) -> Self {
        let panels = panels
            .into_iter()
            .zip(packed.cells)
            .map(|(panel, cell)| {
                let size = [panel.image.width() as usize, panel.image.height() as usize];
                let pixels = egui::ColorImage::from_rgba_unmultiplied(size, panel.image.as_raw());
                // Scaling, when needed, happened once in image-space above. Drawing the resulting
                // copy one-for-one keeps the renderer from applying another filter.
                let texture =
                    cc.egui_ctx
                        .load_texture(&panel.name, pixels, egui::TextureOptions::NEAREST);
                (panel.name, texture, cell)
            })
            .collect();
        Self {
            panels,
            wgpu: cc.wgpu_render_state.clone(),
        }
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

    /// The old unconstrained packing assertions use a ceiling large enough for a row or column of
    /// everything they pass. Limit-specific behavior is covered separately below.
    fn pack(panels: &[Panel]) -> Option<Packed> {
        let sizes = panel_sizes(panels);
        let row = sizes
            .iter()
            .map(|size| u64::from(size.width) + u64::from(GUTTER))
            .sum::<u64>()
            + u64::from(GUTTER);
        let column = sizes
            .iter()
            .map(|size| u64::from(size.height) + u64::from(CAPTION + GUTTER))
            .sum::<u64>()
            + u64::from(GUTTER);
        let limit = u32::try_from(row.max(column)).ok()?;
        layout(&sizes, limit).ok().map(|layout| layout.packed)
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

    #[test]
    fn a_mixed_capture_packs_densely_and_into_a_shape_that_can_be_looked_at() {
        // The demo's own capture. A single column of these is the smallest sheet by area,
        // and also two and a half screens tall — the shape has to count for something.
        let packed =
            pack(&panels(&[(640, 360), (640, 360), (480, 480), (480, 240)])).expect("panels pack");
        let filled = covered(&packed) * 100.0;
        let (width, height) = (packed.width, packed.height);
        assert!(
            filled >= 80.0,
            "the sheet is {filled:.0}% panels at {width}×{height}"
        );
        assert!(height < width * 2, "{width}×{height} is not a strip");
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
        assert!(layout(&[], 1024).is_err());
    }

    #[test]
    fn every_returned_layout_stays_within_its_supplied_limit() {
        let sizes = [
            Size {
                width: 1200,
                height: 700,
            },
            Size {
                width: 900,
                height: 1400,
            },
            Size {
                width: 1700,
                height: 500,
            },
            Size {
                width: 600,
                height: 600,
            },
        ];
        for limit in [4096, 2048, 1024, 512] {
            let packed = layout(&sizes, limit)
                .expect("the sheet can scale to the limit")
                .packed;
            assert!(
                packed.width <= limit && packed.height <= limit,
                "{}×{} exceeds {limit}×{limit}",
                packed.width,
                packed.height
            );
        }
    }

    #[test]
    fn twenty_seven_full_hd_panels_pack_unscaled_below_an_8192_limit() {
        let sizes = vec![
            Size {
                width: 1920,
                height: 1080,
            };
            27
        ];
        let layout = layout(&sizes, 8192).expect("27 full-HD panels fit unscaled");
        assert_eq!(layout.scale, 1.0, "the sheet should not need scaling");
        assert!(layout.packed.width <= 8192 && layout.packed.height <= 8192);
        assert!(
            layout.packed.width >= 4 * (1920 + GUTTER),
            "{}px is approximately four full-HD panels across",
            layout.packed.width
        );
    }

    #[test]
    fn an_unscaled_set_that_cannot_fit_uses_uniform_sheet_only_scaling() {
        let sizes = vec![
            Size {
                width: 3000,
                height: 2000,
            };
            4
        ];
        let layout = layout(&sizes, 2048).expect("scaled panel copies fit");
        assert!(layout.scale < 1.0);
        assert!(layout.packed.width <= 2048 && layout.packed.height <= 2048);
        for (source, scaled) in sizes.iter().zip(&layout.panel_sizes) {
            let width_scale = f64::from(scaled.width) / f64::from(source.width);
            let height_scale = f64::from(scaled.height) / f64::from(source.height);
            assert!(
                (width_scale - height_scale).abs() < 0.001,
                "rounding keeps the uniform scale: {scaled:?}"
            );
        }
    }

    #[test]
    fn decorations_that_cannot_fit_even_one_pixel_copies_are_a_diagnostic() {
        let failure = layout(
            &[
                Size {
                    width: 100,
                    height: 100,
                },
                Size {
                    width: 100,
                    height: 100,
                },
            ],
            GUTTER,
        )
        .expect_err("the outer gutter alone consumes the limit");
        assert!(failure.plain().contains("cannot fit"), "{failure:?}");
    }

    #[test]
    fn a_sheet_renderer_panic_becomes_a_capture_diagnostic() {
        let failure = render_with_backend_errors(
            None,
            Size {
                width: 100,
                height: 100,
            },
            1024,
            || -> Result<(), Diagnostic> { panic!("synthetic renderer failure") },
        )
        .expect_err("the capture command must receive an error");
        let message = failure.plain();
        assert!(message.contains("sheet renderer panicked"), "{message}");
        assert!(message.contains("synthetic renderer failure"), "{message}");
    }

    #[test]
    fn a_real_headless_wgpu_sheet_uses_its_limit_and_oversize_is_a_diagnostic() {
        struct Empty;
        impl eframe::App for Empty {
            fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {}
        }

        let session = crate::render::Session::open(crate::Renderer::Wgpu)
            .expect("a real headless wgpu session");
        let limit = session.max_texture_dimension_2d();
        let oversized = crate::render::open(
            egui::vec2(limit as f32 + 1.0, 1.0),
            1.0,
            &session,
            &|_: &egui::Context| {},
            |_, _| Empty,
        );
        let failure = match oversized {
            Ok(_) => panic!("an oversized target reached the renderer"),
            Err(failure) => failure,
        };
        assert!(failure.plain().contains("exceeds"), "{failure:?}");

        let image = compose(
            panels(&[(96, 54), (96, 54), (64, 96)]),
            &session,
            &|_: &egui::Context| {},
        )
        .expect("the sheet renders through wgpu");
        assert!(image.width() <= limit && image.height() <= limit);
    }

    #[test]
    fn panels_of_spread_aspect_stack_instead_of_lining_up_in_one_row() {
        // Reported: one shot of a scene at each of its breakpoints, so the panels run
        // from a column two and a half times the height of the rest to a banner twice their width.
        // All of them went into a single row: the shortest sheet there is, and a third of it empty.
        let packed = pack(&panels(&[
            (385, 1698),
            (664, 918),
            (964, 658),
            (1164, 658),
            (964, 605),
        ]))
        .expect("panels pack");
        let (filled, ratio) = (covered(&packed) * 100.0, ratio(&packed));
        let (width, height) = (packed.width, packed.height);
        assert!(
            filled >= 80.0,
            "the sheet is {filled:.0}% panels at {width}×{height}"
        );
        assert!(
            (0.8..=2.6).contains(&ratio),
            "{width}×{height} is {ratio:.2}:1, near enough a screen to review"
        );
    }
}
