use super::*;

#[test]
fn global_overrides_resolve_conditional_control_chains_before_drawing() {
    #[derive(Default, serde::Deserialize, serde::Serialize)]
    struct Conditional {
        first: bool,
        second: bool,
        third: bool,
    }

    impl crate::CatalogGlobals for Conditional {
        fn controls(&mut self, controls: &mut crate::GlobalControls<'_>) {
            if self.second {
                self.third = controls.toggle("third", self.third);
            }
            if self.first {
                self.second = controls.toggle("second", self.second);
            }
            self.first = controls.toggle("first", self.first);
        }
    }

    let mut canvas = Canvas {
        scene: crate::test_support::scene("conditional", "globals", true),
        globals: Some(
            GlobalState::new(
                GlobalEntry::of::<Conditional>(),
                crate::SceneRevision::INITIAL,
            )
            .expect("global state"),
        ),
        knobs: Vec::new(),
        pinned: Vec::new(),
        bound: Bound::default(),
        gl: None,
        loader: None,
        painter: None,
        targets: Vec::new(),
        wgpu: None,
        passes: Vec::new(),
        wanted: egui::Vec2::ZERO,
        global_write_error: None,
        actions: None,
    };
    let overrides = knobs(&[("first", "true"), ("second", "true"), ("third", "true")]);
    let mut unmatched: Vec<&KnobOverride> = overrides.iter().collect();

    apply_globals(&mut canvas, &overrides, &mut unmatched).expect("global overrides apply");

    assert!(unmatched.is_empty());
    let (_, bytes) = canvas.globals.as_ref().expect("globals").parts();
    let value: Conditional = postcard::from_bytes(bytes).expect("typed state");
    assert!(value.first && value.second && value.third);
}

fn writes_finnish(ctx: &mut crate::SceneCtx<'_>, _: &mut egui::Ui) {
    use crate::scaffold_scenes::globals::{LANGUAGE, Language};

    assert!(ctx.set_global(LANGUAGE, Language::Finnish));
}

fn global_shot(globals: Overrides) -> Shot {
    Shot {
        scene: "writer".to_owned(),
        out: None,
        size: egui::vec2(320.0, 160.0),
        knobs: Overrides::default(),
        globals,
        frames: None,
        trim: true,
        settle: false,
        scale: DEFAULT_SCALE,
        list: false,
        template: false,
        play: None,
    }
}

#[test]
fn a_recipe_global_wins_over_a_scene_writing_it_every_frame() {
    use crate::scaffold_scenes::globals;

    let session = Session::open(Renderer::Wgpu).expect("headless wgpu session");
    let shot = global_shot(knobs(&[("Language", "English")]));
    let drawn = draw_with_globals(
        crate::SceneEntry {
            render: writes_finnish,
            ..crate::test_support::scene("writer", "globals", true)
        },
        Some(crate::GlobalEntry::of::<globals::Globals>()),
        &session,
        &|_: &egui::Context| {},
        &shot,
        shot.size,
    )
    .expect("the scene's writes do not fail the shot");

    let (_, bytes) = drawn
        .harness
        .state()
        .globals
        .as_ref()
        .expect("global state")
        .parts();
    let value: globals::Globals = postcard::from_bytes(bytes).expect("postcard globals");
    assert!(matches!(value.language, globals::Language::English));
}

const SCALE: crate::GlobalKey<f32> = crate::GlobalKey::new("Scale");

#[derive(Default, serde::Deserialize, serde::Serialize)]
struct Scaled {
    scale: f32,
}

impl crate::CatalogGlobals for Scaled {
    fn controls(&mut self, controls: &mut crate::GlobalControls<'_>) {
        self.scale = controls.slider(SCALE, 1.0, 0.0, 2.0, 0.0);
    }
}

fn writes_a_non_finite_scale(ctx: &mut crate::SceneCtx<'_>, _: &mut egui::Ui) {
    assert!(ctx.set_global(SCALE, f32::NAN));
}

#[test]
fn a_scene_write_that_cannot_land_fails_the_shot() {
    let session = Session::open(Renderer::Wgpu).expect("headless wgpu session");
    let shot = global_shot(Overrides::default());
    let Err(error) = draw_with_globals(
        crate::SceneEntry {
            render: writes_a_non_finite_scale,
            ..crate::test_support::scene("writer", "globals", true)
        },
        Some(crate::GlobalEntry::of::<Scaled>()),
        &session,
        &|_: &egui::Context| {},
        &shot,
        shot.size,
    ) else {
        panic!("a dropped write fails the shot");
    };

    assert!(
        error
            .plain()
            .contains("set_global(\"Scale\"): the written value is not finite"),
        "{}",
        error.plain()
    );
}

#[test]
fn global_overrides_are_rechecked_after_choice_options_change() {
    #[derive(serde::Deserialize, serde::Serialize)]
    struct DynamicChoice {
        expanded: bool,
        choice: String,
    }

    impl Default for DynamicChoice {
        fn default() -> Self {
            Self {
                expanded: false,
                choice: "target".to_owned(),
            }
        }
    }

    impl crate::CatalogGlobals for DynamicChoice {
        fn controls(&mut self, controls: &mut crate::GlobalControls<'_>) {
            self.choice = if self.expanded {
                controls.select("choice", &[("Other", "other")], "other")
            } else {
                controls.select("choice", &[("Target", "target")], "target")
            }
            .to_owned();
            self.expanded = controls.toggle("expanded", self.expanded);
        }
    }

    let mut canvas = Canvas {
        scene: crate::test_support::scene("dynamic", "globals", true),
        globals: Some(
            GlobalState::new(
                GlobalEntry::of::<DynamicChoice>(),
                crate::SceneRevision::INITIAL,
            )
            .expect("global state"),
        ),
        knobs: Vec::new(),
        pinned: Vec::new(),
        bound: Bound::default(),
        gl: None,
        loader: None,
        painter: None,
        targets: Vec::new(),
        wgpu: None,
        passes: Vec::new(),
        wanted: egui::Vec2::ZERO,
        global_write_error: None,
        actions: None,
    };
    let overrides = knobs(&[("choice", "Target"), ("expanded", "true")]);
    let mut unmatched: Vec<&KnobOverride> = overrides.iter().collect();

    let error = apply_globals(&mut canvas, &overrides, &mut unmatched)
        .expect_err("the final choice options reject the override");

    assert!(error.plain().contains("no option `Target`"));
}

#[test]
fn a_shot_applies_typed_globals_before_render_and_the_next_shot_starts_fresh() {
    use crate::{SceneSource as _, scaffold_scenes::globals};

    let scene = crate::Linked
        .manifest()
        .scenes
        .into_iter()
        .find(|scene| scene.module_path.ends_with("scaffold_scenes::globals"))
        .expect("the generated globals demo is compiled into the tests");
    let session = Session::open(Renderer::Wgpu).expect("headless wgpu session");
    let shot = Shot {
        scene: "globals".to_owned(),
        out: None,
        size: egui::vec2(520.0, 240.0),
        knobs: Overrides::default(),
        globals: knobs(&[("Theme", "Dark"), ("Language", "Finnish")]),
        frames: None,
        trim: true,
        settle: false,
        scale: DEFAULT_SCALE,
        list: false,
        template: false,
        play: None,
    };

    let mut dark = draw_with_globals(
        scene,
        Some(crate::GlobalEntry::of::<globals::Globals>()),
        &session,
        &|_: &egui::Context| {},
        &shot,
        shot.size,
    )
    .expect("the shipped demo draws with its global overrides");
    let dark_image = dark.harness.render().expect("render the dark story");
    let (_, bytes) = dark
        .harness
        .state()
        .globals
        .as_ref()
        .expect("global state")
        .parts();
    let value: globals::Globals = postcard::from_bytes(bytes).expect("postcard globals");
    assert!(matches!(value.theme, globals::Theme::Dark));
    assert!(matches!(value.language, globals::Language::Finnish));
    drop(dark);

    let default_shot = Shot {
        globals: Overrides::default(),
        ..shot
    };
    let mut fresh = draw_with_globals(
        scene,
        Some(crate::GlobalEntry::of::<globals::Globals>()),
        &session,
        &|_: &egui::Context| {},
        &default_shot,
        default_shot.size,
    )
    .expect("fresh shot draws");
    let light_image = fresh.harness.render().expect("render the light story");
    let (_, bytes) = fresh
        .harness
        .state()
        .globals
        .as_ref()
        .expect("global state")
        .parts();
    let value: globals::Globals = postcard::from_bytes(bytes).expect("postcard globals");
    assert!(
        matches!(value.theme, globals::Theme::Light)
            && matches!(value.language, globals::Language::English),
        "shots do not inherit the previous shot's globals"
    );
    let luminance = |image: &image::RgbaImage| {
        image
            .pixels()
            .map(|pixel| u64::from(pixel[0]) + u64::from(pixel[1]) + u64::from(pixel[2]))
            .sum::<u64>()
            / image.pixels().len() as u64
    };
    assert!(
        luminance(&dark_image) < luminance(&light_image),
        "the selected theme reaches the captured story background"
    );
}
