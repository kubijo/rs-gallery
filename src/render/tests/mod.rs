//! Capture tests, by topic, and the helpers and fixture scenes they share.

mod capture;
mod globals;
mod offscreen;
mod overrides;
mod recipe;
mod reference;
mod report;

use indoc::indoc;

use super::*;

/// The overrides a recipe or the command line would have produced.
fn knobs(pairs: &[(&str, &str)]) -> Overrides {
    pairs
        .iter()
        .map(|(key, value)| KnobOverride {
            key: (*key).to_owned(),
            value: (*value).to_owned(),
        })
        .collect()
}

/// Apply `pairs` to `knobs`, as a frame of [`shoot`] would; returns how many went unmatched.
fn applied(store: &mut [KnobState], pairs: &[(&str, &str)]) -> Result<usize, Diagnostic> {
    let mut ledger = Ledger::new(knobs(pairs));
    ledger.apply_knobs(store)?;
    // One apply is the last one, so a value that can't land fails here, as at a settle's end.
    let never_landed = ledger.unlanded().find_map(|over| ledger.reason(&over.key));
    if let Some(reason) = ledger.not_yet_now().or(never_landed) {
        return Err(Diagnostic::new(reason));
    }
    Ok(ledger.unlanded().count())
}

fn select(label: &str, options: &[&str]) -> KnobState {
    KnobState::Select {
        label: label.to_owned(),
        value: 0,
        options: options.iter().map(|o| (*o).to_owned()).collect(),
        style: ChoiceStyle::Buttons,
    }
}

fn slider(label: &str) -> KnobState {
    KnobState::Slider {
        label: label.to_owned(),
        value: 1.0,
        min: 0.0,
        max: 2.0,
        step: 0.1,
    }
}

/// The GL for a test's shots, as a run opens one.
#[cfg(not(target_vendor = "apple"))]
fn glow() -> Session {
    Session::open(Renderer::Glow).expect("a headless glow session")
}

/// One scene, drawn as a shot would draw it, with the harness left open to poke at.
#[cfg(not(target_vendor = "apple"))]
fn staged_harness(
    render: fn(&mut crate::SceneCtx<'_>, &mut egui::Ui),
    name: &'static str,
) -> egui_kittest::Harness<'static, Canvas> {
    let scene = SceneEntry {
        render,
        name,
        module_path: "reference",
        default: true,
        order: 0,
        source: "",
    };
    let shot = Shot {
        scene: name.to_owned(),
        out: None,
        size: egui::vec2(200.0, 200.0),
        knobs: Overrides::default(),
        globals: Overrides::default(),
        frames: None,
        trim: true,
        settle: false,
        scale: DEFAULT_SCALE,
        list: false,
        template: false,
        play: None,
    };
    draw(scene, &glow(), &|_: &egui::Context| {}, &shot, shot.size)
        .expect("the scene draws")
        .harness
}

/// A scene that paints through [`crate::SceneCtx::offscreen`]
/// — the glow-only path, where a GL library draws into a framebuffer
/// gallery owns and egui shows the result inline.
fn draws_offscreen(ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
    ctx.offscreen(ui, [32_u32, 32], |target| {
        let loader = target.gl_loader();
        // SAFETY: the capture made its context current, and `loader` resolves against it.
        let gl = unsafe { glow::Context::from_loader_function_cstr(|symbol| loader(symbol)) };
        // SAFETY: the target's framebuffer is bound for the duration of this closure.
        unsafe {
            use eframe::glow::HasContext as _;
            gl.clear_color(1.0, 0.0, 1.0, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            // A second colour in one corner: a flat field would look right however it went wrong.
            gl.enable(glow::SCISSOR_TEST);
            gl.scissor(0, 0, 16, 16);
            gl.clear_color(0.0, 1.0, 1.0, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            gl.disable(glow::SCISSOR_TEST);
        }
    });
}

/// Several offscreen images at different sizes, so the reference catches one call site's target
/// standing in for another's — which reads as the same picture repeated, at the wrong shape.
/// The last is staged, which also holds the chrome a rendered frame gets.
fn draws_offscreen_slots(ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
    const SLOT_GAP: f32 = 6.0;

    fn flat(target: &crate::Offscreen, rgb: (f32, f32, f32)) {
        let loader = target.gl_loader();
        // SAFETY: the capture made its context current, and `loader` resolves against it.
        let gl = unsafe { glow::Context::from_loader_function_cstr(|symbol| loader(symbol)) };
        // SAFETY: the target's framebuffer is bound for the duration of this closure.
        unsafe {
            use eframe::glow::HasContext as _;
            gl.clear_color(rgb.0, rgb.1, rgb.2, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
        }
    }
    ui.label("wide");
    ctx.offscreen(ui, [64_u32, 24], |target| flat(target, (0.9, 0.2, 0.2)));
    ui.add_space(SLOT_GAP);
    ui.label("tall");
    ctx.offscreen(ui, [24_u32, 48], |target| flat(target, (0.2, 0.4, 0.9)));
    ui.add_space(SLOT_GAP);
    // Staged, so the reference also holds the chrome a rendered frame gets:
    // the checkerboard around it, the size caption, the collapse arrow.
    ui.label("staged");
    ctx.offscreen_stage(ui, crate::Stage::Fit, [40_u32, 40], |target| {
        flat(target, (0.2, 0.8, 0.4));
    });
}

/// A scene of the kind the shell is for: prose, and a fixed-size stage on the checkerboard.
fn a_documented_stage(ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
    ui.heading("Snapshot");
    ui.label("prose above the stage");
    crate::stage!(ctx, ui, (96, 40), |ui: &mut egui::Ui| {
        ui.label("in the stage");
    });
}

/// Each way a stage can be sized, so one reference covers the conversions rather than one of them.
fn every_stage_form(ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
    ui.label("fitted");
    crate::stage!(ctx, ui, |ui: &mut egui::Ui| {
        ui.label("hugs its content");
    });
    ui.label("pinned");
    crate::stage!(ctx, ui, (150, 36), |ui: &mut egui::Ui| {
        ui.label("150×36");
    });
    ui.label("square");
    crate::stage!(ctx, ui, 64, |ui: &mut egui::Ui| {
        ui.label("64");
    });
    // Content past its box, so the reference shows a fixed scrolling stage keeping to the size
    // it declared. The checkerboard closing underneath is the whole point of the picture.
    ui.label("scrolling");
    ctx.stage(
        ui,
        crate::Stage::Fixed(egui::vec2(150.0, 48.0)).scrollable(),
        |ui: &mut egui::Ui| {
            for row in 0..12 {
                ui.label(format!("row {row}"));
            }
        },
    );
}

/// Glyphs the default faces lack: without the bundled Noto fallbacks these are tofu,
/// which no structural assertion would notice and no reader of the reference could miss.
fn glyphs_past_the_default_faces(_ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
    ui.heading("→ ∑ ≈ ± °");
    ui.label("arrows ← ↑ ↓ →");
    ui.label("math ∀ ∈ ∞ √");
    ui.label("symbols ✓ ✗ ★ ♦");
}

/// Everything drawn here comes from a knob, so its reference is a picture of the override path.
fn dressed_by_knobs(ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
    let caption = ctx.text("caption", "the default").get();
    let tint = ctx.color("tint", egui::Color32::WHITE).get();
    let width = ctx.slider("width", 70.0, 20.0, 260.0, 1.0).get();
    crate::stage!(ctx, ui, (width, 48.0), |ui: &mut egui::Ui| {
        ui.label(egui::RichText::new(caption).color(tint).size(20.0));
    });
}

/// Keeps a GL object between frames, as a scene holding a femtovg canvas does,
/// and paints by whether that object is still the one it made.
/// The recipe takes two shots, since one cannot show a context outliving anything.
fn caches_between_shots(ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
    /// Wide enough that nothing else in the context is this shape by chance.
    const WIDE: i32 = 37;

    thread_local! {
        static KEPT: std::cell::Cell<Option<glow::NativeTexture>> =
            const { std::cell::Cell::new(None) };
    }
    ctx.offscreen(ui, [48_u32, 48], |target| {
        let loader = target.gl_loader();
        // SAFETY: the capture made its context current, and `loader` resolves against it.
        let gl = unsafe { glow::Context::from_loader_function_cstr(|symbol| loader(symbol)) };
        use eframe::glow::HasContext as _;
        let texture = KEPT.get().unwrap_or_else(|| {
            // SAFETY: a live context; the width is the mark read back on later shots.
            let made = unsafe {
                let made = gl.create_texture().expect("a texture");
                gl.bind_texture(glow::TEXTURE_2D, Some(made));
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::RGBA as i32,
                    WIDE,
                    1,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(None),
                );
                made
            };
            KEPT.set(Some(made));
            made
        });
        // A name proves nothing — a fresh context reissues them from 1. The width is the mark.
        //
        // SAFETY: querying a name a dead context issued is the failure this guards;
        // the driver reports it rather than trapping.
        let ours = unsafe { gl.get_texture_level_parameter_i32(texture, 0, glow::TEXTURE_WIDTH) };
        // SAFETY: the target's framebuffer is bound for the duration of this closure.
        unsafe {
            let green = f32::from(u8::from(ours == WIDE));
            gl.clear_color(0.0, green, 0.0, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
        }
    });
}

/// The scenes the reference recipe names, standing in for a consumer's `*.scene.rs`.
fn reference_scenes() -> Manifest {
    let scene = |render, name| SceneEntry {
        render,
        name,
        module_path: "reference",
        default: true,
        order: 0,
        source: "",
    };
    Manifest {
        scenes: vec![
            scene(a_documented_stage, "documented-stage"),
            scene(every_stage_form, "stage-forms"),
            scene(glyphs_past_the_default_faces, "glyph-fallbacks"),
            scene(dressed_by_knobs, "knobs-applied"),
            scene(draws_offscreen, "offscreen-gl"),
            scene(draws_offscreen_slots, "offscreen-slots"),
            scene(caches_between_shots, "offscreen-cached"),
            scene(a_hoverable_button, "hovered-button"),
        ],
        groups: Vec::new(),
    }
}

/// Hover is a state no knob can set, so a play's reference shows it.
fn a_hoverable_button(ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
    crate::stage!(ctx, ui, |ui: &mut egui::Ui| {
        _ = ui.button("Hover me");
    });
}

fn hover_it(p: &mut crate::Play<'_>) -> crate::PlayResult {
    p.hover("Hover me")
}

fn reference_plays() -> Vec<PlayEntry> {
    vec![PlayEntry {
        scene: crate::SceneKey {
            module_path: "reference",
            name: "hovered-button",
        },
        name: "hover_it",
        module_path: "reference",
        run: hover_it,
    }]
}
