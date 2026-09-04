//! What the side panels draw: knob widgets, the actions log, the frame-cost meter, the source view.

use gallery::prelude::*;

use crate::{
    actions::{Log, render_actions},
    knobs::{ChoiceStyle, Knob, render_knobs},
    perf::{PERF_WINDOW_SIZE, PerfStats, render_performance},
    render_source_view,
};

scene_meta! { title: "Shell / Panels" }

/// Every knob kind at once, live: the Controls panel only shows what a scene declared,
/// so this is the only place the set can be compared.
/// Values are kept against the `Ui`, not the shell's store.
#[scene(default)]
fn knobs(ctx: &mut SceneCtx, ui: &mut Ui) {
    let posed = ui.id().with("shell-scenes-knobs");
    let mut knobs = ui.data_mut(|d| d.get_temp_mut_or_insert_with(posed, every_kind).clone());

    ctx.stage(ui, egui::vec2(330.0, 260.0), |ui| {
        render_knobs(ui, &mut knobs);
    });

    ui.data_mut(|d| d.insert_temp(posed, knobs));
}

/// What a scene with nothing to control gets: a sentence rather than an empty grid.
#[scene("no knobs", order = 1)]
fn no_knobs(ctx: &mut SceneCtx, ui: &mut Ui) {
    ctx.stage(ui, egui::vec2(330.0, 40.0), |ui| {
        ui.weak("This scene has no controls.");
    });
}

/// The log a scene writes with `action`, and what it says before one has.
#[scene("actions", order = 2)]
fn actions(ctx: &mut SceneCtx, ui: &mut Ui) {
    ctx.stage(ui, egui::vec2(360.0, 40.0), |ui| {
        render_actions(ui, &Log::default());
    });
    let reported = Log::posed(
        "shell_scenes::panels::actions",
        &[
            "picked Firmware 4.2.1",
            "Install · wide",
            "saved to disk",
            "dismissed",
        ],
    );
    ctx.stage(ui, egui::vec2(360.0, 96.0), |ui| {
        render_actions(ui, &reported);
    });
}

/// The frame-cost meter over readings that never happened: at rest, steady, spiking. It usually
/// draws in a viewport of its own — `Gallery::ui`'s `show_perf` branch says why.
#[scene("performance", order = 3)]
fn performance(ctx: &mut SceneCtx, ui: &mut Ui) {
    for (name, costs) in [
        ("at rest", &[0.0_f32; 12][..]),
        (
            "steady",
            &[3.1, 3.4, 3.2, 3.3, 3.5, 3.2, 3.4, 3.3, 3.2, 3.4, 3.3, 3.5][..],
        ),
        (
            "spiking",
            &[
                3.2, 3.4, 12.8, 3.3, 3.5, 21.4, 3.2, 3.4, 9.6, 3.3, 3.2, 16.1,
            ][..],
        ),
    ] {
        let mut stats = PerfStats::new();
        for cost in costs {
            stats.record(*cost / 1_000.0);
        }
        ui.label(name);
        ctx.stage(ui, egui::Vec2::from(PERF_WINDOW_SIZE), |ui| {
            render_performance(ui, &stats);
        });
    }
}

/// The Source tab: a scene's own text, as `#[scene]` captured it.
#[scene("source view", order = 4)]
fn source_view(ctx: &mut SceneCtx, ui: &mut Ui) {
    const SOURCE: &str = "fn plain(ctx: &mut SceneCtx, ui: &mut Ui) {\n    stage!(ctx, ui, |ui| {\n        ui.heading(\"Hello, world\");\n    });\n}\n";
    ctx.stage(ui, egui::vec2(460.0, 120.0), |ui| {
        render_source_view(ui, SOURCE);
    });
}

/// One of each, at values that show what the widget does with them.
fn every_kind() -> Vec<Knob> {
    vec![
        Knob::Button {
            label: "Run action".to_owned(),
            clicked: false,
        },
        Knob::Text {
            label: "name".to_owned(),
            value: "world".to_owned(),
        },
        Knob::Slider {
            label: "size".to_owned(),
            value: 24.0,
            min: 12.0,
            max: 64.0,
            step: 1.0,
        },
        Knob::Slider {
            label: "smooth".to_owned(),
            value: 0.4,
            min: 0.0,
            max: 1.0,
            step: 0.0,
        },
        Knob::Toggle {
            label: "uppercase".to_owned(),
            value: false,
        },
        Knob::Color {
            label: "accent".to_owned(),
            value: crate::SCENE_TINT,
        },
        Knob::Select {
            label: "weight".to_owned(),
            value: 1,
            options: ["light", "regular", "bold"].map(str::to_owned).to_vec(),
            style: ChoiceStyle::Buttons,
        },
        Knob::Select {
            label: "align".to_owned(),
            value: 0,
            options: ["start", "centre", "end"].map(str::to_owned).to_vec(),
            style: ChoiceStyle::Radio,
        },
        Knob::Select {
            label: "step".to_owned(),
            // Long enough to wrap onto a second row, which is the case that reserves its height.
            value: 3,
            options: [
                "idle",
                "connecting",
                "authenticating",
                "downloading",
                "verifying",
                "installing",
            ]
            .map(str::to_owned)
            .to_vec(),
            style: ChoiceStyle::Dropdown,
        },
    ]
}
