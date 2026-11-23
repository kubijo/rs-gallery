use super::*;

/// Two images in one scene are two targets: sharing one made every image show
/// the last one's pixels, stretched to whatever size that call had asked for.
///
/// Different sizes as well as different colours, since one target would also
/// have been reallocated between the two and left the first image reading
/// a texture of the wrong shape.
#[cfg(not(target_vendor = "apple"))]
#[test]
fn each_offscreen_call_in_a_scene_keeps_its_own_target() {
    fn two(ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
        fn fill(target: &crate::Offscreen, rgb: (f32, f32, f32)) {
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
        ctx.offscreen(ui, [64_u32, 32], |target| fill(target, (1.0, 0.0, 0.0)));
        ctx.offscreen(ui, [32_u32, 16], |target| fill(target, (0.0, 0.0, 1.0)));
    }
    let manifest = Manifest {
        scenes: vec![SceneEntry {
            render: two,
            name: "two-images",
            module_path: "reference",
            default: true,
            order: 0,
            source: "",
        }],
        groups: Vec::new(),
    };
    let out = Utf8PathBuf::from_path_buf(std::env::temp_dir())
        .expect("a UTF-8 temp dir")
        .join("gallery-two-images")
        .join("two.png");
    let capture = Capture {
        shots: vec![Shot {
            scene: "two-images".to_owned(),
            out: Some(out.clone()),
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
            record: None,
        }],
        sheet: None,
        report: None,
    };
    render(&manifest, Renderer::Glow, &|_: &egui::Context| {}, &capture).expect("the shot renders");

    let png = std::fs::read(&out).expect("the capture was written");
    let image = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
        .expect("a PNG")
        .to_rgba8();
    let mostly = |want: char| {
        image
            .pixels()
            .filter(|p| {
                let [r, _, b, a] = p.0;
                a > 128
                    && if want == 'r' {
                        r > 128 && b < 64
                    } else {
                        b > 128 && r < 64
                    }
            })
            .count()
    };
    assert!(
        mostly('r') > 512,
        "the first image keeps its own red, not the second's blue"
    );
    assert!(mostly('b') > 128, "and the second keeps its blue");
}

/// A context per shot leaves a scene's cached GL objects dead
/// from the second shot on — [`SharedCapture`] has the mechanism.
#[cfg(not(target_vendor = "apple"))]
#[test]
fn every_shot_in_a_run_draws_even_when_the_scene_caches_against_the_context() {
    /// Distinctive enough that no other texture in the context is this wide by chance.
    const WIDE: i32 = 37;

    thread_local! {
        // A GL object made once and reused, standing in for a cached femtovg canvas.
        static CACHED: std::cell::Cell<Option<glow::NativeTexture>> =
            const { std::cell::Cell::new(None) };
    }
    fn caches_a_texture(ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
        ctx.offscreen(ui, [40_u32, 40], |target| {
            let loader = target.gl_loader();
            // SAFETY: the capture made its context current, and `loader` resolves against it.
            let gl = unsafe { glow::Context::from_loader_function_cstr(|symbol| loader(symbol)) };
            use eframe::glow::HasContext as _;
            let texture = CACHED.get().unwrap_or_else(|| {
                // SAFETY: a live context; the name is kept, and its width is the mark read back.
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
                CACHED.set(Some(made));
                made
            });
            // A name proves nothing — a fresh context reissues them from 1, and egui's painter
            // takes them. The width is what says the object is still the one we made.
            //
            // SAFETY: querying a name a dead context issued is the failure under test;
            // the driver reports it rather than trapping.
            let ours =
                unsafe { gl.get_texture_level_parameter_i32(texture, 0, glow::TEXTURE_WIDTH) };
            // SAFETY: the target's framebuffer is bound for the duration of this closure.
            unsafe {
                let shade = if ours == WIDE { 1.0 } else { 0.0 };
                gl.clear_color(0.0, shade, 0.0, 1.0);
                gl.clear(glow::COLOR_BUFFER_BIT);
            }
        });
    }
    let manifest = Manifest {
        scenes: vec![SceneEntry {
            render: caches_a_texture,
            name: "caching",
            module_path: "reference",
            default: true,
            order: 0,
            source: "",
        }],
        groups: Vec::new(),
    };
    let dir = Utf8PathBuf::from_path_buf(std::env::temp_dir())
        .expect("a UTF-8 temp dir")
        .join("gallery-every-shot");
    let shot = |name: &str| Shot {
        scene: "caching".to_owned(),
        out: Some(dir.join(format!("{name}.png"))),
        size: egui::vec2(120.0, 120.0),
        knobs: Overrides::default(),
        globals: Overrides::default(),
        frames: None,
        trim: true,
        settle: false,
        scale: DEFAULT_SCALE,
        list: false,
        template: false,
        play: None,
        record: None,
    };
    let capture = Capture {
        shots: vec![shot("one"), shot("two"), shot("three")],
        sheet: None,
        report: None,
    };
    CACHED.set(None);
    render(&manifest, Renderer::Glow, &|_: &egui::Context| {}, &capture)
        .expect("every shot renders");

    let green = |name: &str| {
        let png = std::fs::read(dir.join(format!("{name}.png"))).expect("a capture");
        let image = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
            .expect("a PNG")
            .to_rgba8();
        image
            .pixels()
            .filter(|p| {
                // Green rather than merely bright, so the stage's pale chrome stays out of it.
                let [r, g, b, a] = p.0;
                a > 128 && g > 128 && r < 64 && b < 64
            })
            .count()
    };
    let (one, two, three) = (green("one"), green("two"), green("three"));
    assert!(one > 512, "the first shot draws: {one} green pixels");
    assert_eq!(
        (one, one),
        (two, three),
        "and every later shot draws the same, its cached texture still belonging to a live context"
    );
}

/// A texture the scene registers itself stages the same way up as one gallery owns,
/// and `showing` hides the slack rather than the content.
///
/// The marker is asymmetric top to bottom, a flip being the failure here and a symmetric
/// marker passing upside down. The texture is allocated half again as tall as it is shown,
/// so a crop taken off the wrong end shows the empty half instead.
#[cfg(not(target_vendor = "apple"))]
#[test]
fn an_adopted_texture_stages_upright_with_its_slack_hidden() {
    /// The whole texture; the bottom third is left blank as slack.
    const ALLOCATED: [u32; 2] = [32, 48];
    /// What the scene lays out and asks to be shown.
    const SHOWN: [u32; 2] = [32, 32];

    thread_local! {
        static ADOPTED: std::cell::Cell<Option<egui::TextureId>> =
            const { std::cell::Cell::new(None) };
    }
    fn adopts(ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
        let id = ADOPTED.get().unwrap_or_else(|| {
            let loader = ctx.gl_loader().expect("the glow renderer");
            // SAFETY: the capture made its context current, and `loader` resolves against it.
            let gl = unsafe { glow::Context::from_loader_function_cstr(|symbol| loader(symbol)) };
            use eframe::glow::HasContext as _;
            // Red across the scene's top row, blue across its bottom, the rest transparent —
            // so upright, flipped and mis-cropped each come out different.
            //
            // GL rows run bottom-up while a stage reads top-down, so the shown window
            // is the *last* `SHOWN` rows of this buffer: the scene's top row is the very
            // last one, and the slack sits at the start. Filling from row 0 would put
            // the content in the slack, which is the mistake the flip makes easy.
            let mut pixels = vec![0_u8; (ALLOCATED[0] * ALLOCATED[1] * 4) as usize];
            let row = (ALLOCATED[0] * 4) as usize;
            for x in 0..ALLOCATED[0] as usize {
                let top = (ALLOCATED[1] as usize - 1) * row + x * 4;
                pixels[top] = 255;
                pixels[top + 3] = 255;
                let bottom = (ALLOCATED[1] - SHOWN[1]) as usize * row + x * 4;
                pixels[bottom + 2] = 255;
                pixels[bottom + 3] = 255;
            }
            // SAFETY: a live context; the texture outlives the scene by design.
            let name = unsafe {
                let name = gl.create_texture().expect("a texture");
                gl.bind_texture(glow::TEXTURE_2D, Some(name));
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::RGBA as i32,
                    ALLOCATED[0] as i32,
                    ALLOCATED[1] as i32,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(Some(&pixels)),
                );
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MIN_FILTER,
                    glow::NEAREST as i32,
                );
                gl.tex_parameter_i32(
                    glow::TEXTURE_2D,
                    glow::TEXTURE_MAG_FILTER,
                    glow::NEAREST as i32,
                );
                gl.bind_texture(glow::TEXTURE_2D, None);
                name
            };
            let id = ctx.register_native_texture(
                std::num::NonZeroU32::new(name.0.get()).expect("a GL name"),
            );
            ADOPTED.set(Some(id));
            id
        });
        ctx.texture_stage(
            ui,
            crate::Stage::Fit,
            crate::StageTexture::new(id, ALLOCATED).showing(SHOWN),
        );
    }
    let manifest = Manifest {
        scenes: vec![SceneEntry {
            render: adopts,
            name: "adopts",
            module_path: "reference",
            default: true,
            order: 0,
            source: "",
        }],
        groups: Vec::new(),
    };
    let out = Utf8PathBuf::from_path_buf(std::env::temp_dir())
        .expect("a UTF-8 temp dir")
        .join("gallery-adopted")
        .join("adopted.png");
    let capture = Capture {
        shots: vec![Shot {
            scene: "adopts".to_owned(),
            out: Some(out.clone()),
            size: egui::vec2(120.0, 120.0),
            knobs: Overrides::default(),
            globals: Overrides::default(),
            frames: None,
            trim: true,
            settle: false,
            scale: DEFAULT_SCALE,
            list: false,
            template: false,
            play: None,
            record: None,
        }],
        sheet: None,
        report: None,
    };
    ADOPTED.set(None);
    render(&manifest, Renderer::Glow, &|_: &egui::Context| {}, &capture).expect("the shot renders");

    let png = std::fs::read(&out).expect("the capture was written");
    let image = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
        .expect("a PNG")
        .to_rgba8();
    // A hue rather than a bright channel: the stage's chrome is pale grey, which is bright
    // in every channel and would read as both markers at once.
    let topmost = |hue: usize| {
        let other = if hue == 0 { 2 } else { 0 };
        image
            .enumerate_pixels()
            .filter(|(_, _, p)| p.0[3] > 128 && p.0[hue] > 128 && p.0[other] < 64)
            .map(|(_, y, _)| y)
            .min()
            .expect("the marker is in the image")
    };
    let (red, blue) = (topmost(0), topmost(2));
    assert!(
        red < blue,
        "red marks the scene's top row and blue its bottom, so the image is upright: \
             red from y={red}, blue from y={blue}"
    );
}

/// The colour-space contract [`crate::Offscreen`] states, in pixels.
///
/// Both halves are asserted because a trap is only useful stated alongside its remedy
/// — the identity case alone reads as "it just works".
#[cfg(not(target_vendor = "apple"))]
#[test]
fn an_offscreen_decodes_what_is_written_unless_the_scene_encodes_it() {
    /// Away from both ends, where a stray decode moves the value a long way.
    const WROTE: [f32; 3] = [0.2, 0.5, 0.8];

    fn flat(ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
        let fill = |target: &crate::Offscreen, encode: bool| {
            let loader = target.gl_loader();
            // SAFETY: the capture made its context current, and `loader` resolves against it.
            let gl = unsafe { glow::Context::from_loader_function_cstr(|symbol| loader(symbol)) };
            // SAFETY: the target's framebuffer is bound for the duration of this closure.
            unsafe {
                use eframe::glow::HasContext as _;
                if encode {
                    gl.enable(glow::FRAMEBUFFER_SRGB);
                }
                gl.clear_color(WROTE[0], WROTE[1], WROTE[2], 1.0);
                gl.clear(glow::COLOR_BUFFER_BIT);
                if encode {
                    gl.disable(glow::FRAMEBUFFER_SRGB);
                }
            }
        };
        ctx.offscreen(ui, [32_u32, 32], |target| fill(target, false));
        ctx.offscreen(ui, [32_u32, 32], |target| fill(target, true));
    }
    let manifest = Manifest {
        scenes: vec![SceneEntry {
            render: flat,
            name: "flat",
            module_path: "reference",
            default: true,
            order: 0,
            source: "",
        }],
        groups: Vec::new(),
    };
    let out = Utf8PathBuf::from_path_buf(std::env::temp_dir())
        .expect("a UTF-8 temp dir")
        .join("gallery-srgb")
        .join("flat.png");
    let capture = Capture {
        shots: vec![Shot {
            scene: "flat".to_owned(),
            out: Some(out.clone()),
            size: egui::vec2(80.0, 80.0),
            knobs: Overrides::default(),
            globals: Overrides::default(),
            frames: None,
            trim: true,
            settle: false,
            scale: DEFAULT_SCALE,
            list: false,
            template: false,
            play: None,
            record: None,
        }],
        sheet: None,
        report: None,
    };
    render(&manifest, Renderer::Glow, &|_: &egui::Context| {}, &capture).expect("the shot renders");

    let png = std::fs::read(&out).expect("the capture was written");
    let image = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
        .expect("a PNG")
        .to_rgba8();
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a colour channel, rounded into the byte it came from"
    )]
    let wrote: Vec<u8> = WROTE.iter().map(|c| (c * 255.0).round() as u8).collect();
    // The two images stack down the canvas; sample the middle of each, clear of the edges.
    let quarter = image.get_pixel(image.width() / 2, image.height() / 4).0;
    let three_quarters = image.get_pixel(image.width() / 2, image.height() * 3 / 4).0;

    assert!(
        quarter[..3]
            .iter()
            .zip(&wrote)
            .all(|(got, sent)| got < sent),
        "written straight in, every channel comes back decoded and darker: \
             wrote {wrote:?}, read {:?}",
        &quarter[..3]
    );
    // Within a bit rather than to the byte: the encode and the decode round independently,
    // and llvmpipe and a GPU driver disagree by one on some channels. A decode moves it by 40.
    assert!(
        three_quarters[..3]
            .iter()
            .zip(&wrote)
            .all(|(got, sent)| got.abs_diff(*sent) <= 1),
        "and written under FRAMEBUFFER_SRGB it survives: wrote {wrote:?}, read {:?}",
        &three_quarters[..3]
    );
    assert_eq!(three_quarters[3], 255, "opaque either way");
}

/// The pixels a shot writes have to be the ones its overrides produced, for content drawn
/// through `offscreen` as much as for egui's own — otherwise a scene drawn
/// entirely in GL captures its declared defaults whatever the recipe says.
#[cfg(not(target_vendor = "apple"))]
#[test]
fn a_shot_override_reaches_the_pixels_an_offscreen_scene_draws() {
    fn tinted(ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
        let red = ctx.slider("red", 0.0, 0.0, 1.0, 0.0).get();
        // Staged, as a scene drawn entirely in GL has it — the override has to reach
        // the texture, not just the egui chrome around it.
        ctx.offscreen_stage(ui, crate::Stage::Fit, [40_u32, 40], move |target| {
            let loader = target.gl_loader();
            // SAFETY: the capture made its context current, and `loader` resolves against it.
            let gl = unsafe { glow::Context::from_loader_function_cstr(|symbol| loader(symbol)) };
            // SAFETY: the target's framebuffer is bound for the duration of this closure.
            unsafe {
                use eframe::glow::HasContext as _;
                gl.clear_color(red, 0.0, 0.0, 1.0);
                gl.clear(glow::COLOR_BUFFER_BIT);
            }
        });
    }
    let manifest = Manifest {
        scenes: vec![SceneEntry {
            render: tinted,
            name: "tinted",
            module_path: "reference",
            default: true,
            order: 0,
            source: "",
        }],
        groups: Vec::new(),
    };
    let dir = Utf8PathBuf::from_path_buf(std::env::temp_dir())
        .expect("a UTF-8 temp dir")
        .join("gallery-offscreen-override");
    let shot = |name: &str, knobs: Overrides| Shot {
        scene: "tinted".to_owned(),
        out: Some(dir.join(format!("{name}.png"))),
        size: egui::vec2(120.0, 120.0),
        knobs,
        globals: Overrides::default(),
        frames: None,
        trim: true,
        settle: false,
        scale: DEFAULT_SCALE,
        list: false,
        template: false,
        play: None,
        record: None,
    };
    // Two shots rather than one: the default and the override in a single run,
    // so the pair also says a later shot's knobs are its own.
    let capture = Capture {
        shots: vec![
            shot("default", Overrides::default()),
            // The scene declares 0.0, so anything red in this one came from here.
            shot("overridden", knobs(&[("red", "1")])),
        ],
        sheet: None,
        report: None,
    };
    render(&manifest, Renderer::Glow, &|_: &egui::Context| {}, &capture)
        .expect("both shots render");

    let reds = |name: &str| {
        let png = std::fs::read(dir.join(format!("{name}.png"))).expect("a capture");
        let image = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
            .expect("a PNG")
            .to_rgba8();
        image
            .pixels()
            .filter(|p| {
                // Red rather than merely bright: the stage's own chrome is pale grey,
                // which a red channel alone would count.
                let [r, g, b, a] = p.0;
                a > 128 && r > 128 && g < 64 && b < 64
            })
            .count()
    };
    assert_eq!(reds("default"), 0, "the declared default is not red at all");
    assert!(
        reds("overridden") > 512,
        "and the override drove the offscreen draw: {} red pixels",
        reds("overridden")
    );
}

/// A staged image is a call site like any other: its own slot, in the order the scene makes it,
/// leaving the bare call after it on the slot it already had. The caption reports the image,
/// which for a fitted stage is the response's own rect.
#[cfg(not(target_vendor = "apple"))]
#[test]
fn a_staged_image_takes_a_slot_of_its_own_and_is_captioned_by_its_size() {
    thread_local! {
        static STAGED: std::cell::Cell<Option<egui::Rect>> =
            const { std::cell::Cell::new(None) };
    }
    fn staged_and_bare(ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
        let blank = |_: &crate::Offscreen| {};
        let shown = ctx.offscreen_stage(ui, crate::Stage::Fit, [48_u32, 24], blank);
        STAGED.set(shown.map(|response| response.rect));
        ctx.offscreen(ui, [16_u32, 16], blank);
    }
    let harness = staged_harness(staged_and_bare, "staged-and-bare");
    assert_eq!(
        harness.state().targets.len(),
        2,
        "the staged image and the bare one each keep their own target"
    );
    let rect = STAGED.get().expect("an open stage shows its image");
    assert_eq!(
        (rect.width(), rect.height()),
        (48.0, 24.0),
        "a fitted stage is the image, and its size is what the caption reads"
    );
}

/// Folding a stage away has to skip the GL, not draw it somewhere nobody looks — and the target
/// stays put, so unfolding does not rebuild it or shift the call sites after it.
#[cfg(not(target_vendor = "apple"))]
#[test]
fn a_folded_stage_costs_no_gl_and_keeps_its_target() {
    use egui_kittest::kittest::Queryable as _;

    thread_local! {
        static DREW: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    }
    fn counts(ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
        ctx.offscreen_stage(ui, crate::Stage::Fit, [48_u32, 24], |_| {
            DREW.set(DREW.get() + 1);
        });
    }
    let mut harness = staged_harness(counts, "folds");
    assert!(DREW.get() > 0, "an open stage draws");
    assert_eq!(harness.state().targets.len(), 1);

    harness.get_by_label("Collapse stage").click();
    harness.run_steps(2);
    let folded = DREW.get();
    harness.run_steps(2);
    assert_eq!(DREW.get(), folded, "a folded stage runs no GL at all");
    assert_eq!(
        harness.state().targets.len(),
        1,
        "and holds its target for when it opens again"
    );
}

/// A scene may make fewer calls on a later frame — one behind a toggle — and the targets
/// it stops asking for are kept for when it asks again, rather than freed under
/// a `TextureId` eframe gives no way to release.
#[cfg(not(target_vendor = "apple"))]
#[test]
fn a_scene_that_stops_staging_an_image_keeps_the_target_for_its_return() {
    thread_local! {
        static BOTH: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
    }
    fn sometimes_two(ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
        let blank = |_: &crate::Offscreen| {};
        ctx.offscreen(ui, [32_u32, 32], blank);
        if BOTH.get() {
            ctx.offscreen(ui, [16_u32, 16], blank);
        }
    }
    let manifest = Manifest {
        scenes: vec![SceneEntry {
            render: sometimes_two,
            name: "sometimes-two",
            module_path: "reference",
            default: true,
            order: 0,
            source: "",
        }],
        groups: Vec::new(),
    };
    let shot = Shot {
        scene: "sometimes-two".to_owned(),
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
        record: None,
    };
    let mut harness = draw(
        manifest.scenes[0],
        &glow(),
        &|_: &egui::Context| {},
        &shot,
        shot.size,
    )
    .expect("both images draw")
    .harness;
    assert_eq!(harness.state().targets.len(), 2, "one target per call site");

    BOTH.set(false);
    harness.run_steps(2);
    assert_eq!(
        harness.state().targets.len(),
        2,
        "the one it stopped staging is held, not dropped"
    );

    BOTH.set(true);
    harness.run_steps(2);
    assert_eq!(
        harness.state().targets.len(),
        2,
        "and taken up again rather than made afresh"
    );
}

/// A shot smaller than its scene lays out is drawn twice, and the scene must see the same GL
/// both times: `scripts/offscreen.scene.rs` tells scenes to build a femtovg canvas once
/// and keep it, and one built against a context since torn down draws nothing.
///
/// The loader is an `Arc` per capture, so its identity is what says whether the context held.
#[cfg(not(target_vendor = "apple"))]
#[test]
fn a_regrown_capture_keeps_the_gl_the_scene_cached_against() {
    thread_local! {
        static FIRST: std::cell::RefCell<Option<crate::GlLoader>> =
            const { std::cell::RefCell::new(None) };
        static SWAPPED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    fn caches_gl(ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
        ctx.offscreen(ui, [64_u32, 64], |target| {
            let loader = target.gl_loader();
            FIRST.with_borrow_mut(|first| match first {
                Some(seen) if !std::sync::Arc::ptr_eq(seen, &loader) => SWAPPED.set(true),
                Some(_) => {}
                None => *first = Some(loader),
            });
        });
    }
    let manifest = Manifest {
        scenes: vec![SceneEntry {
            render: caches_gl,
            name: "caches-gl",
            module_path: "reference",
            default: true,
            order: 0,
            source: "",
        }],
        groups: Vec::new(),
    };
    let shot = Shot {
        scene: "caches-gl".to_owned(),
        out: None,
        // Well under the 64×64 image plus its padding, so the fitting size differs.
        size: egui::vec2(24.0, 24.0),
        knobs: Overrides::default(),
        globals: Overrides::default(),
        frames: None,
        trim: true,
        settle: false,
        scale: DEFAULT_SCALE,
        list: false,
        template: false,
        play: None,
        record: None,
    };
    let harness = draw(
        manifest.scenes[0],
        &glow(),
        &|_: &egui::Context| {},
        &shot,
        shot.size,
    )
    .expect("the first pass draws")
    .harness;
    let wanted = harness.state().wanted;
    assert!(
        wanted.x > shot.size.x || wanted.y > shot.size.y,
        "the scene has to outgrow the shot for this to test anything"
    );
    drop(harness);
    // That measuring pass had a capture of its own; only the shot below is under test.
    FIRST.with_borrow_mut(|first| *first = None);
    SWAPPED.set(false);

    shoot(&manifest, &glow(), &|_: &egui::Context| {}, &shot).expect("the shot renders");
    assert!(
        !SWAPPED.get(),
        "the scene saw one GL context for the whole shot"
    );
}
