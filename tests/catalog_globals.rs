use gallery::{CatalogGlobals, GlobalControls, SceneSource as _, prelude::*};

#[derive(Clone, Copy, Default, PartialEq, serde::Deserialize, serde::Serialize)]
enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Copy, Default, PartialEq, serde::Deserialize, serde::Serialize)]
enum Language {
    #[default]
    English,
    Finnish,
}

#[derive(Default, serde::Deserialize, serde::Serialize)]
struct Globals {
    theme: Theme,
    language: Language,
    tint: egui::Color32,
    offset: gallery::Pad2D,
}

impl CatalogGlobals for Globals {
    fn controls(&mut self, controls: &mut GlobalControls<'_>) {
        self.theme = controls.buttons(
            "Theme",
            &[
                ("System", Theme::System),
                ("Light", Theme::Light),
                ("Dark", Theme::Dark),
            ],
            Theme::System,
        );
        self.language = controls.select(
            "Language",
            &[
                ("English", Language::English),
                ("Finnish", Language::Finnish),
            ],
            Language::English,
        );
        self.tint = controls.color("Tint", self.tint);
        self.offset = controls.pad2d("Offset", gallery::Pad2DSpec::default());
    }

    fn prepare(&self, ui: &mut egui::Ui) {
        let theme = match self.theme {
            Theme::System => return,
            Theme::Light => egui::Theme::Light,
            Theme::Dark => egui::Theme::Dark,
        };
        ui.set_style(ui.ctx().style_of(theme));
    }
}

gallery::catalog_globals!(Globals);

#[scene(default)]
fn reads_typed_globals(ctx: &mut SceneCtx<'_>, ui: &mut Ui, globals: &Globals) {
    let language = match globals.language {
        Language::English => "English",
        Language::Finnish => "Finnish",
    };
    stage!(ctx, ui, |ui: &mut Ui| {
        ui.label(language);
    });
}

#[test]
fn linked_catalogs_discover_globals_and_three_argument_scenes() {
    let mut linked = gallery::Linked;
    assert!(linked.globals().is_some());
    let manifest = linked.manifest();
    assert_eq!(manifest.scenes.len(), 1);
    assert_eq!(manifest.scenes[0].name, "Reads Typed Globals");
}
