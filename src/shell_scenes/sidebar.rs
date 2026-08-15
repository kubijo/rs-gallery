//! The scene tree: folders, leaves, what a selection looks like, and what a filter does to both.

use gallery::prelude::*;

use crate::{Collapsed, Manifest, SceneGroupMeta, Sidebar, build_tree, render_node, svg::Icons};

scene_meta! { title: "Shell / Sidebar" }

thread_local! {
    static ICONS: Icons = Icons::load();
}

/// The tree the shell builds, over scenes that are not this run's.
/// The filter knob is the sidebar's search box: a match reaches into a folded folder,
/// and a folder whose own name matches shows everything under it.
#[scene(default)]
fn tree(ctx: &mut SceneCtx, ui: &mut Ui) {
    let filter = ctx.text("filter", "");
    let fold = ctx.toggle("folders folded", false);
    let scenes = posed();
    let groups = vec![
        group("app::dial", "Components / Dial"),
        group("app::badge", "Components / Badge"),
        group("app::map", "Map"),
    ];
    let tree = build_tree(&Manifest {
        scenes: scenes.clone(),
        groups,
    });

    let selected = ui.id().with("shell-scenes-selected");
    let mut chosen = ui.data(|d| d.get_temp::<String>(selected));
    ICONS.with(|icons| {
        let sidebar = Sidebar {
            scenes: &scenes,
            icons,
            filter: &filter,
            collapsed: if fold {
                &Collapsed::Everything
            } else {
                &Collapsed::Nothing
            },
        };
        ctx.stage(ui, egui::vec2(260.0, 240.0), |ui| {
            render_node(ui, &tree, &sidebar, &mut chosen, false, true);
        });
    });
    if let Some(chosen) = chosen {
        ui.data_mut(|d| d.insert_temp(selected, chosen));
    }
}

/// Scenes to hang the tree off. Nothing draws them — the sidebar only ever reads their names.
fn posed() -> Vec<SceneEntry> {
    fn nothing(_: &mut SceneCtx, _: &mut Ui) {}
    let scene = |name, module_path, default| SceneEntry {
        render: nothing,
        name,
        module_path,
        default,
        order: u32::MAX,
        source: "",
    };
    vec![
        scene("enabled", "app::dial", true),
        scene("disabled", "app::dial", false),
        scene("mid drag", "app::dial", false),
        scene("badge", "app::badge", true),
        scene("aerial", "app::map", true),
    ]
}

fn group(module_path: &'static str, title: &'static str) -> SceneGroupMeta {
    SceneGroupMeta { module_path, title }
}
