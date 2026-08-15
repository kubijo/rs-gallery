//! The `--hot` cycle, posed: the chip in every phase, and the bar and report a failed build puts
//! over the canvas. A run passes these one at a time, so this is the one place to see them at once.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use gallery::prelude::*;

use crate::watch::{
    BuildFailure, BuildMessage, HotPhase, HotStatus, MessageLevel, render_build_bar,
    render_build_report, render_hot_chip,
};

scene_meta! { title: "Shell / Hot reload" }

/// A panel corner the shape of the one it hangs in: the scenes panel is narrow, and the chip has
/// to fit it at every phase.
const CORNER: egui::Vec2 = egui::vec2(200.0, 64.0);

/// A moment `ago` in the past, or now on a machine that has not been up that long.
fn ago(seconds: f32) -> Instant {
    let now = Instant::now();
    now.checked_sub(Duration::from_secs_f32(seconds))
        .unwrap_or(now)
}

/// Every phase the chip can be in, in the order a good run passes through them.
#[scene(default)]
fn chip(ctx: &mut SceneCtx, ui: &mut Ui) {
    let building = ctx.slider("building for", 1.4, 0.0, 90.0, 0.1);
    let took = ctx.slider("reload took", 1.8, 0.0, 30.0, 0.1);
    let errors = ctx.slider("errors", 2.0, 0.0, 9.0, 1.0) as usize;
    // The working phases breathe, which needs frames to keep coming.
    ui.ctx().request_repaint();

    let phases = [
        HotPhase::Watching,
        HotPhase::Changed,
        HotPhase::Building {
            since: ago(building),
        },
        HotPhase::Swapping {
            since: Instant::now(),
        },
        HotPhase::Reloaded {
            at: Instant::now(),
            took: Duration::from_secs_f32(took),
        },
        HotPhase::Failed(Arc::new(failure(errors))),
        HotPhase::Stopped {
            why: "no file watcher: inotify watch limit reached".to_owned(),
        },
    ];

    let sizes = vec![CORNER; phases.len()];
    ctx.matrix(ui, &sizes, |ui, at| {
        // Each in a scope of its own: the chip takes its id from the `Ui`, and seven sharing one
        // would be seven claims on the same hover.
        ui.push_id(at, |ui| {
            render_hot_chip(ui, ui.max_rect(), &HotStatus::posed(phases[at].clone()));
        });
    });
}

/// The bar a failed build puts over the canvas, at each count it can report.
///
/// Clicking one opens the report it promises, which is the modal the next scene holds open.
#[scene("failure bar", order = 1)]
fn failure_bar(ctx: &mut SceneCtx, ui: &mut Ui) {
    ui.label(
        "A build that failed says so over the canvas, and the scenes underneath stay as they \
              were — the last ones that built.",
    );
    ui.add_space(8.0);

    for errors in [0, 1, 3] {
        let hot = HotStatus::posed(HotPhase::Failed(Arc::new(failure(errors))));
        let open_id = ui.id().with(("shell-scenes-report", errors));
        let mut open = ui.data(|d| d.get_temp::<bool>(open_id).unwrap_or(false));
        ctx.stage(ui, egui::vec2(430.0, 26.0), |ui| {
            render_build_bar(ui, &hot, &mut open);
        });
        ui.data_mut(|d| d.insert_temp(open_id, open));
    }
}

/// What the bar opens: everything cargo said, each message in the colour of what it weighs.
#[scene("build report", order = 2)]
fn build_report(ctx: &mut SceneCtx, ui: &mut Ui) {
    ctx.stage(ui, egui::vec2(560.0, 260.0), |ui| {
        render_build_report(ui, &failure(1));
    });
}

/// A failure of `errors`, with the messages a build of that shape would have rendered.
fn failure(errors: usize) -> BuildFailure {
    let mut messages = vec![BuildMessage {
        level: MessageLevel::Warning,
        text: "warning: unused variable: `count`\n  --> src/dial.rs:31:9\n   |\n31 |     let count = ticks.len();\n   |         ^^^^^ help: if this is intentional, prefix it with an underscore".to_owned(),
    }];
    for nth in 0..errors {
        messages.push(BuildMessage {
            level: MessageLevel::Error,
            text: format!(
                "error[E0308]: mismatched types\n  --> src/dial.rs:{}:22\n   |\n   |     let width: f32 = \"wide\";\n   |                ---   ^^^^^^ expected `f32`, found `&str`",
                40 + nth
            ),
        });
    }
    messages.push(BuildMessage {
        level: MessageLevel::Note,
        text: "note: this error originates in the macro `stage`".to_owned(),
    });
    // What cargo itself could not do, which is all there is to show where nothing rendered.
    if errors == 0 {
        messages = vec![BuildMessage {
            level: MessageLevel::Error,
            text: "error: failed to parse manifest at `/work/app-gallery/Cargo.toml`".to_owned(),
        }];
    }
    BuildFailure { messages, errors }
}
