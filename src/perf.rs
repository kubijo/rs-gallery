//! The performance window: a frame-cost ring buffer and the readout painted from it.
//!
//! The window renders in its own viewport, on its own repaint clock, so watching the numbers never
//! drives the loop being measured.

use std::time::{Duration, Instant};

use crate::{HAIRLINE, MUTED, header_bar, header_title};

const TABLE_ROW_H: f32 = 18.0;
const TABLE_LABEL_W: f32 = 96.0;
pub(crate) const PERF_WINDOW_SIZE: [f32; 2] = [380.0, 240.0];

/// Translucent threshold gridlines, then their even fainter ms labels.
const PERF_GRID: egui::Color32 = egui::Color32::from_rgba_premultiplied(0x50, 0x50, 0x50, 0x80);
const PERF_GRID_LABEL: egui::Color32 =
    egui::Color32::from_rgba_premultiplied(0x70, 0x70, 0x70, 0xA0);
/// Bar colours by frame time: green ≤ 60 fps, yellow ≤ 30 fps, red below.
const PERF_GOOD: egui::Color32 = egui::Color32::from_rgb(0x4C, 0xAF, 0x50);
const PERF_WARN: egui::Color32 = egui::Color32::from_rgb(0xE0, 0xB0, 0x30);
const PERF_BAD: egui::Color32 = egui::Color32::from_rgb(0xD9, 0x3A, 0x3A);

/// Park the perf window beside the shell rather than over it, flipping left when the monitor has no
/// room. `None` when the shell's geometry is unknown — Wayland never reports it — leaving it to the WM.
pub(crate) fn perf_window_pos(ctx: &egui::Context) -> Option<egui::Pos2> {
    let (outer, monitor) = ctx.input(|i| (i.viewport().outer_rect, i.viewport().monitor_size));
    let outer = outer?;
    let gap = 8.0;
    let right = outer.right() + gap;
    Some(
        if monitor.is_none_or(|m| right + PERF_WINDOW_SIZE[0] <= m.x) {
            egui::pos2(right, outer.top())
        } else {
            egui::pos2(
                (outer.left() - gap - PERF_WINDOW_SIZE[0]).max(0.0),
                outer.top(),
            )
        },
    )
}

/// Frame-cost ring buffer with smoothed display values for the performance window.
///
/// Samples are the cost of *building* a frame, not the interval between frames, so they mean the same
/// thing whether the shell is repainting steadily or idle. Deliberately no FPS: under a reactive loop
/// repaint frequency measures how often something asked for a frame, not how expensive one is.
pub(crate) struct PerfStats {
    /// Per-frame CPU build cost, in seconds.
    costs: [f32; 30],
    write_idx: usize,
    display_ms: f32,
    display_p95_ms: f32,
    update_at: Instant,
    /// When the last sample landed, so the window can tell "cheap" apart from "not rendering".
    last_record: Instant,
}

impl PerfStats {
    pub(crate) fn new() -> Self {
        Self {
            costs: [0.0; 30],
            write_idx: 0,
            display_ms: 0.0,
            display_p95_ms: 0.0,
            update_at: Instant::now(),
            last_record: Instant::now(),
        }
    }

    /// What the shell is doing, for the window's status row. A reactive shell nobody is touching
    /// produces no frames, so a frozen readout is correct — this says so instead of looking broken.
    fn activity(&self) -> String {
        let idle = self.last_record.elapsed();
        if idle < Duration::from_millis(400) {
            "rendering".to_owned()
        } else {
            format!("idle {:.0}s", idle.as_secs_f32())
        }
    }

    /// Record one frame's build cost (seconds); ~4×/sec, refresh the smoothed average and p95.
    #[expect(
        clippy::cast_precision_loss,
        reason = "averaging 30 small, non-negative costs"
    )]
    pub(crate) fn record(&mut self, cost: f32) {
        self.costs[self.write_idx] = cost;
        self.write_idx = (self.write_idx + 1) % self.costs.len();
        self.last_record = Instant::now();
        if self.update_at.elapsed().as_secs_f32() > 0.25 {
            self.update_at = Instant::now();
            let avg = self.costs.iter().sum::<f32>() / self.costs.len() as f32;
            self.display_ms = avg * 1_000.0;
            let mut sorted = self.costs;
            sorted.sort_by(f32::total_cmp);
            self.display_p95_ms = sorted[sorted.len() * 95 / 100] * 1_000.0;
        }
    }
}

pub(crate) fn render_performance(ui: &mut egui::Ui, perf: &PerfStats) {
    {
        let mut header = header_bar(ui);
        header.label(header_title("Performance"));
    }

    ui.add_space(6.0);
    render_metric_table(ui, perf);

    // Taken as a rect rather than through a `horizontal`, whose cross-axis sizing caps the height.
    ui.add_space(6.0);
    let left = ui.available_rect_before_wrap();
    let plot = egui::Rect::from_min_max(
        egui::pos2(left.left() + 6.0, left.top()),
        egui::pos2(left.right() - 6.0, left.bottom() - 6.0),
    );
    ui.allocate_rect(plot, egui::Sense::hover());
    render_sparkline(ui, &perf.costs, perf.write_idx, plot);
}

/// The readings, painted rather than laid out: a `Grid` sizes columns to content, so the box would
/// breathe whenever a reading changed width, and it draws no inner rules. Fixed geometry gives both.
fn render_metric_table(ui: &mut egui::Ui, perf: &PerfStats) {
    let font = egui::FontId::monospace(11.0);
    let rows = [
        ("Frame cost", format!("{:.1} ms", perf.display_ms)),
        ("p95", format!("{:.1} ms", perf.display_p95_ms)),
        ("Shell", perf.activity()),
    ];

    let avail = ui.available_rect_before_wrap();
    let table = egui::Rect::from_min_size(
        egui::pos2(avail.left() + 6.0, avail.top()),
        egui::vec2(avail.width() - 12.0, TABLE_ROW_H * rows.len() as f32),
    );
    ui.allocate_rect(table, egui::Sense::hover());

    let painter = ui.painter();
    let rule = egui::Stroke::new(1.0, HAIRLINE);
    painter.rect_stroke(table, 0.0, rule, egui::StrokeKind::Inside);
    let divider = table.left() + TABLE_LABEL_W;
    painter.vline(divider, table.y_range(), rule);

    let mut top = table.top();
    for (i, (name, reading)) in rows.iter().enumerate() {
        if i > 0 {
            painter.hline(table.x_range(), top, rule);
        }
        let mid = top + TABLE_ROW_H / 2.0;
        painter.text(
            egui::pos2(table.left() + 8.0, mid),
            egui::Align2::LEFT_CENTER,
            name,
            font.clone(),
            MUTED,
        );
        painter.text(
            egui::pos2(divider + 8.0, mid),
            egui::Align2::LEFT_CENTER,
            reading,
            font.clone(),
            egui::Color32::WHITE,
        );
        top += TABLE_ROW_H;
    }
}

/// Paint the frame-cost sparkline into `rect`, oldest sample first. The gridlines are frame budgets —
/// 17 ms (60 fps) and 33 ms (30 fps) — so a bar above one costs more than that budget allows.
#[expect(
    clippy::cast_precision_loss,
    reason = "small bar counts cast to pixel offsets"
)]
fn render_sparkline(ui: &egui::Ui, costs: &[f32; 30], write_idx: usize, rect: egui::Rect) {
    let n = costs.len();
    // A gutter for the gridline labels; the plot takes everything else.
    let plot_left = rect.left() + 30.0;
    let bar_h_max = (rect.height() - 2.0).max(4.0);
    let bar_stride = ((rect.right() - plot_left) / n as f32).max(1.0);
    let bar_fill = (bar_stride - 1.0).max(1.0);
    let spark_left = plot_left;
    let spark_bottom = rect.bottom() - 1.0;
    let spark_top = spark_bottom - bar_h_max;
    let scale_max = 1.0 / 30.0; // 33.3 ms fills the height.

    // Border around the plot area.
    let border_rect = egui::Rect::from_min_max(
        egui::pos2(spark_left - 1.0, spark_top - 1.0),
        egui::pos2(rect.right(), spark_bottom + 1.0),
    );
    ui.painter().rect_stroke(
        border_rect,
        0.0,
        egui::Stroke::new(1.0, HAIRLINE),
        egui::StrokeKind::Outside,
    );

    // Threshold gridlines, labelled on the left
    // in milliseconds (the axis is frame time, not FPS).
    let grid_stroke = egui::Stroke::new(1.0, PERF_GRID);
    let label_font = egui::FontId::monospace(7.0);
    for (label, frac) in [("17ms", 0.5_f32), ("33ms", 1.0_f32)] {
        let y = (spark_bottom - frac * bar_h_max).floor();
        ui.painter()
            .hline(spark_left..=border_rect.right() - 1.0, y, grid_stroke);
        let galley =
            ui.painter()
                .layout_no_wrap(label.to_owned(), label_font.clone(), PERF_GRID_LABEL);
        ui.painter().galley(
            egui::pos2(
                spark_left - galley.size().x - 3.0,
                y - galley.size().y / 2.0,
            ),
            galley,
            PERF_GRID_LABEL,
        );
    }

    // Bars, oldest (write_idx) at the left.
    for i in 0..n {
        let idx = (write_idx + i) % n;
        let t = costs[idx];
        if t <= 0.0 {
            continue;
        }
        let frac = (t / scale_max).clamp(0.0, 1.0);
        let x = (spark_left + i as f32 * bar_stride).floor();
        let bar_h = frac * bar_h_max;
        if bar_h < 0.5 {
            continue;
        }
        let color = if t <= 1.0 / 60.0 {
            PERF_GOOD
        } else if t <= 1.0 / 30.0 {
            PERF_WARN
        } else {
            PERF_BAD
        };
        let bar_top = (spark_bottom - bar_h).floor();
        let bar_rect = egui::Rect::from_min_size(
            egui::pos2(x, bar_top),
            egui::vec2(bar_fill, spark_bottom - bar_top),
        );
        ui.painter().rect_filled(bar_rect, 0.0, color);
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable as _;

    use super::*;

    #[test]
    fn perf_stats_starts_zeroed() {
        let perf = PerfStats::new();
        assert_eq!(perf.display_ms, 0.0);
        assert_eq!(perf.display_p95_ms, 0.0);
    }

    #[test]
    fn perf_stats_ring_buffer_wraps() {
        let mut perf = PerfStats::new();
        let cap = perf.costs.len();
        for _ in 0..cap + 2 {
            perf.record(0.016);
        }
        assert_eq!(perf.write_idx, 2);
    }

    #[test]
    fn perf_stats_smooths_over_the_window() {
        let mut perf = PerfStats::new();
        for _ in 0..perf.costs.len() {
            perf.record(1.0 / 60.0);
        }
        // Reopen the ~4×/sec smoothing window without waiting on the wall clock.
        perf.update_at -= Duration::from_millis(300);
        perf.record(1.0 / 60.0);
        // Every sample is the same cost, so the average and the p95 land on it.
        assert!(
            (perf.display_ms - 16.67).abs() < 0.1,
            "avg {}",
            perf.display_ms
        );
        assert!(
            (perf.display_p95_ms - 16.67).abs() < 0.1,
            "p95 {}",
            perf.display_p95_ms
        );
    }

    #[test]
    fn performance_window_renders_with_its_title() {
        let mut perf = PerfStats::new();
        for _ in 0..perf.costs.len() {
            perf.record(1.0 / 60.0);
        }
        let mut harness = egui_kittest::Harness::new_ui(move |ui| {
            render_performance(ui, &perf);
        });
        harness.run();
        assert!(harness.query_by_label("Performance").is_some());
    }
}
