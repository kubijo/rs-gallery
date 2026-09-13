//! Controls shared by every scene.

use std::sync::LazyLock;

use gallery::prelude::*;

struct GlobalIcons {
    light: Icon,
    dark: Icon,
    english: Icon,
    finnish: Icon,
}

static ICONS: LazyLock<GlobalIcons> = LazyLock::new(|| GlobalIcons {
    light: Icon::from_svg(include_bytes!("assets/theme-light.svg")),
    dark: Icon::from_svg(include_bytes!("assets/theme-dark.svg")),
    english: Icon::from_svg(include_bytes!("assets/language-english.svg")),
    finnish: Icon::from_svg(include_bytes!("assets/language-finnish.svg")),
});

#[derive(Clone, Copy, Default, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) enum Theme {
    #[default]
    Light,
    Dark,
}

#[derive(Clone, Copy, Default, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) enum Language {
    #[default]
    English,
    Finnish,
}

#[derive(Default, serde::Deserialize, serde::Serialize)]
pub(crate) struct Globals {
    pub(crate) theme: Theme,
    pub(crate) language: Language,
}

impl Globals {
    fn checkerboard(&self) -> Checkerboard {
        match self.theme {
            Theme::Light => Checkerboard::Light,
            Theme::Dark => Checkerboard::Dark,
        }
    }
}

impl CatalogGlobals for Globals {
    fn controls(&mut self, controls: &mut GlobalControls<'_>) {
        self.theme = controls.icon_buttons(
            "Theme",
            &[
                ("Light", &ICONS.light, Theme::Light),
                ("Dark", &ICONS.dark, Theme::Dark),
            ],
            Theme::Light,
        );
        self.language = controls.icon_buttons(
            "Language",
            &[
                ("English", &ICONS.english, Language::English),
                ("Finnish", &ICONS.finnish, Language::Finnish),
            ],
            Language::English,
        );
    }

    fn prepare(&self, ui: &mut egui::Ui) {
        let style = ui.ctx().style_of(match self.theme {
            Theme::Light => egui::Theme::Light,
            Theme::Dark => egui::Theme::Dark,
        });
        ui.set_style(style);
    }
}

scene_meta! { title: "Globals / Theme and language" }

/// Theme is applied by `prepare`; this scene also reads the selected language.
#[scene(default)]
fn theme_and_language(ctx: &mut SceneCtx, ui: &mut Ui, globals: &Globals) {
    let (heading, description) = match globals.language {
        Language::English => (
            "Hello from the whole catalog",
            "Change either global control: theme prepares every scene, while language is read here.",
        ),
        Language::Finnish => (
            "Terve koko galleriasta",
            "Vaihda yhteisiä asetuksia: teema koskee kaikkia näkymiä, ja tämä näkymä lukee kielen.",
        ),
    };

    ctx.stage(
        ui,
        Stage::Fixed(egui::vec2(440.0, 150.0)).checkerboard(globals.checkerboard()),
        |ui| {
            ui.vertical_centered(|ui| {
                ui.heading(heading);
                ui.add_space(8.0);
                ui.label(description);
            });
        },
    );
}
