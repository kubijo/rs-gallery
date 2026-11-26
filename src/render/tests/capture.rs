use super::*;

/// The same recipe twice is the same bytes twice,
/// which is what lets an unattended run diff today's
/// captures against yesterday's and believe a difference.
///
/// A capture's frame times come from the harness's fixed `step_dt` rather
/// than from the clock, and the scene here reads them so a wall-clock
/// frame time shows up as a difference between two runs rather than
/// as anything a reader would notice in one.
#[cfg(not(target_vendor = "apple"))]
#[test]
fn two_runs_of_one_recipe_write_the_same_bytes() {
    fn clock_watching(ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
        let time = ui.input(|i| i.time);
        crate::stage!(ctx, ui, (120, 40), move |ui: &mut egui::Ui| {
            // Bars rather than the number as text: a font would round the difference away.
            let width = (time * 40.0) as f32 % 100.0;
            let (rect, _) = ui.allocate_exact_size(egui::vec2(100.0, 20.0), egui::Sense::hover());
            ui.painter().rect_filled(
                egui::Rect::from_min_size(rect.min, egui::vec2(width, 20.0)),
                0.0,
                egui::Color32::from_rgb(0x6C, 0x9C, 0xD8),
            );
        });
    }
    let manifest = Manifest {
        scenes: vec![SceneEntry {
            render: clock_watching,
            name: "clock",
            module_path: "reference",
            default: true,
            order: 0,
            source: "",
        }],
        groups: Vec::new(),
    };
    let dir = Utf8PathBuf::from_path_buf(std::env::temp_dir())
        .expect("a UTF-8 temp dir")
        .join("gallery-determinism");
    let run = |name: &str| {
        let out = dir.join(format!("{name}.png"));
        let capture = Capture {
            shots: vec![Shot {
                scene: "clock".to_owned(),
                out: Some(out.clone()),
                size: egui::vec2(200.0, 120.0),
                knobs: Overrides::default(),
                globals: Overrides::default(),
                frames: Some(6),
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
        render(&manifest, Renderer::Glow, &|_: &egui::Context| {}, &capture)
            .expect("the shot renders");
        std::fs::read(&out).expect("the capture was written")
    };

    let (once, twice) = (run("once"), run("twice"));
    assert_eq!(
        once,
        twice,
        "two runs of one recipe wrote {} and {} bytes of different PNG",
        once.len(),
        twice.len()
    );
}

/// Each shot gets its own harness, so egui's own memory cannot cross between them.
/// A scene's own cache can: it lives in the scenes dylib, which outlives every harness,
/// and one keyed by the id the scene derives is what this guards.
///
/// The bar counts frames drawn under that id — twice as long if it inherited a neighbour's.
#[test]
fn a_shot_captures_the_same_whether_or_not_another_scene_ran_first() {
    thread_local! {
        /// Frames drawn under each derived id — a render target a scene keeps, in miniature.
        static DRAWN: std::cell::RefCell<std::collections::HashMap<egui::Id, f32>> =
            std::cell::RefCell::new(std::collections::HashMap::new());
    }
    fn counts_against_its_id(ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
        let id = ui.next_auto_id();
        crate::stage!(ctx, ui, (120, 40), move |ui: &mut egui::Ui| {
            let frames = DRAWN.with_borrow_mut(|drawn| {
                let frames = drawn.entry(id).or_insert(0.0);
                *frames += 1.0;
                *frames
            });
            // Bars rather than a number: a font would round the difference away.
            let (rect, _) = ui.allocate_exact_size(egui::vec2(100.0, 20.0), egui::Sense::hover());
            let bar = frames * 8.0;
            assert!(
                bar < rect.width(),
                "a bar at {frames} frames has room to grow, so an inherited count \
                     shows longer rather than saturating to the same width"
            );
            ui.painter().rect_filled(
                egui::Rect::from_min_size(rect.min, egui::vec2(bar, 20.0)),
                0.0,
                egui::Color32::from_rgb(0x6C, 0x9C, 0xD8),
            );
        });
    }
    // One body, two identities — which is the whole of what tells the ids apart.
    let scene = |name: &'static str| SceneEntry {
        render: counts_against_its_id,
        name,
        module_path: "reference",
        default: name == "neighbour",
        order: 0,
        source: "",
    };
    let manifest = Manifest {
        scenes: vec![scene("neighbour"), scene("subject")],
        groups: Vec::new(),
    };
    let dir = Utf8PathBuf::from_path_buf(std::env::temp_dir())
        .expect("a UTF-8 temp dir")
        .join("gallery-scene-ids");
    let shot = |scene: &str, out: &Utf8PathBuf| Shot {
        scene: scene.to_owned(),
        out: Some(out.clone()),
        size: egui::vec2(200.0, 120.0),
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
    // Cleared between the runs, since the two are
    // standing in for two separate processes.
    let run = |shots: Vec<Shot>, out: &Utf8PathBuf| {
        DRAWN.with_borrow_mut(std::collections::HashMap::clear);
        let capture = Capture {
            shots,
            sheet: None,
            report: None,
        };
        render(&manifest, Renderer::Glow, &|_: &egui::Context| {}, &capture)
            .expect("the shots render");
        std::fs::read(out).expect("the capture was written")
    };

    let after = dir.join("after.png");
    let alone = dir.join("alone.png");
    let following = run(
        vec![
            shot("neighbour", &dir.join("neighbour.png")),
            shot("subject", &after),
        ],
        &after,
    );
    let by_itself = run(vec![shot("subject", &alone)], &alone);

    assert_eq!(
        following,
        by_itself,
        "the subject captured {} bytes after its neighbour and {} bytes alone",
        following.len(),
        by_itself.len()
    );
}

/// A frame count too small to apply a recipe is refused rather than quietly raised.
#[test]
fn a_shot_asking_for_fewer_frames_than_a_capture_needs_is_an_error() {
    assert!(check_frames(None).is_ok(), "unset takes the default");
    assert!(check_frames(Some(2)).is_ok(), "two is the least that works");
    let refused = check_frames(Some(1)).expect_err("one cannot apply a recipe");
    assert!(
        refused.contains("declares") && refused.contains("applies"),
        "and says why rather than just refusing: {refused}"
    );
}

/// A scene that stops asking to be redrawn is shot then, not at the recipe's frame count —
/// so a set of scenes that settle at different speeds needs one number, not one per scene.
///
/// The scene asks for `ANIMATED` frames' worth of repaints and then goes quiet,
/// well inside a generous cap, so settling and hitting the cap are far apart.
#[test]
fn a_settling_shot_stops_when_the_scene_goes_quiet_rather_than_at_the_frame_count() {
    /// Frames the scene asks to be redrawn for, counting the first.
    const ANIMATED: u32 = 3;
    /// Far enough past `ANIMATED` that stopping there cannot be the cap doing it.
    const CAP: u32 = 30;

    thread_local! {
        static DREW: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    }
    fn animates(_ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
        let drawn = DREW.get() + 1;
        DREW.set(drawn);
        if drawn < ANIMATED {
            ui.ctx().request_repaint();
        }
        ui.label("frame");
    }
    let scene = SceneEntry {
        render: animates,
        name: "animates",
        module_path: "reference",
        default: true,
        order: 0,
        source: "",
    };
    let shot = |settle: bool| Shot {
        scene: "animates".to_owned(),
        out: None,
        size: egui::vec2(80.0, 40.0),
        knobs: Overrides::default(),
        globals: Overrides::default(),
        frames: Some(CAP),
        trim: true,
        settle,
        scale: DEFAULT_SCALE,
        list: false,
        template: false,
        play: None,
        record: None,
    };

    let session = Session::open(Renderer::Wgpu).expect("headless wgpu session");
    DREW.set(0);
    let settling = draw(
        scene,
        &session,
        &|_: &egui::Context| {},
        &shot(true),
        shot(true).size,
    )
    .expect("the scene draws");
    let settling_drew = DREW.get();
    assert_eq!(
        settling.frames.motion,
        Motion::Settled,
        "the scene went quiet well before the cap"
    );
    assert!(
        settling.frames.drawn < CAP,
        "and stopped short of the ceiling: {} frames of {CAP}",
        settling.frames.drawn
    );

    DREW.set(0);
    draw(
        scene,
        &session,
        &|_: &egui::Context| {},
        &shot(false),
        shot(false).size,
    )
    .expect("the scene draws");
    let capped_drew = DREW.get();

    // Draws rather than frames: egui may run a step over more than one pass, so the counts
    // are compared against each other instead of against `CAP`.
    assert!(
        settling_drew * 2 < capped_drew,
        "settling stopped far short of the cap: {settling_drew} draws against {capped_drew}"
    );
}

/// A canvas scrolls, so a scene bigger than the size asked for used to come back cropped to it —
/// losing, at the least, the margin that shows where the stage ends.
#[cfg(not(target_vendor = "apple"))]
#[test]
fn a_scene_too_big_for_the_size_asked_for_is_captured_whole() {
    let asked = egui::vec2(120.0, 80.0);
    let out = Utf8PathBuf::from_path_buf(std::env::temp_dir())
        .expect("a UTF-8 temp dir")
        .join("gallery-overflow")
        .join("stage-forms.png");
    let capture = Capture {
        shots: vec![Shot {
            scene: "stage-forms".to_owned(),
            out: Some(out.clone()),
            size: asked,
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
    render(
        &reference_scenes(),
        Renderer::Glow,
        &|_: &egui::Context| {},
        &capture,
    )
    .expect("the shot renders");

    let png = std::fs::read(&out).expect("the capture was written");
    let image = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
        .expect("a PNG")
        .to_rgba8();
    assert!(
        image.width() > asked.x as u32 && image.height() > asked.y as u32,
        "{}×{} holds the whole scene, not the {}×{} asked for",
        image.width(),
        image.height(),
        asked.x,
        asked.y
    );
}

#[cfg(not(target_vendor = "apple"))]
#[test]
fn a_tall_capture_keeps_its_full_png_and_uploads_a_smaller_sheet_panel() {
    fn tall(ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
        ctx.stage(ui, (120.0, 2600.0), |ui| {
            ui.painter()
                .rect_filled(ui.max_rect(), 0.0, egui::Color32::RED);
            let mut bottom = ui.max_rect();
            bottom.min.y = bottom.max.y - 100.0;
            ui.painter().rect_filled(bottom, 0.0, egui::Color32::BLUE);
        });
    }
    let manifest = Manifest {
        scenes: vec![SceneEntry {
            render: tall,
            ..crate::test_support::scene("tall", "capture", true)
        }],
        groups: Vec::new(),
    };
    let temp = tempfile::tempdir().expect("capture directory");
    let dir = Utf8Path::from_path(temp.path()).expect("UTF-8 path");
    let recipe = dir.join("capture.toml");
    std::fs::write(
        &recipe,
        indoc! {r#"
            out = "renders"
            size = "200x200"
            sheet = "sheet.png"
            report = "capture.json"
            [[shot]]
            name = "one"
            scene = "tall"
            [[shot]]
            name = "two"
            scene = "tall"
        "#},
    )
    .unwrap();
    let capture =
        read_recipe(&recipe, None, &RecordDefaults::default()).expect("a tall capture recipe");
    render(
        &manifest,
        Renderer::Glow,
        &|ctx| {
            ctx.input_mut(|input| input.max_texture_side = 2048);
        },
        &capture,
    )
    .expect("tall shots and their sheet render");
    let out = dir.join("renders");
    let image = image::open(out.join("one.png"))
        .expect("full shot PNG")
        .to_rgba8();
    assert!(
        image.height() >= 2600,
        "the whole stage is captured: {}",
        image.height()
    );
    assert!(
        image
            .rows()
            .skip(2500)
            .flatten()
            .any(|pixel| pixel.0 == [0, 0, 255, 255]),
        "the bottom of the stage is painted too"
    );
    let sheet = image::open(out.join("sheet.png"))
        .expect("sheet PNG")
        .to_rgba8();
    assert!(
        sheet.height() >= image.height(),
        "the packed display size survives texture downsampling"
    );
    assert!(
        sheet
            .rows()
            .skip(2500)
            .flatten()
            .any(|pixel| pixel.0 == [0, 0, 255, 255]),
        "the sheet includes the bottom of the resized panel"
    );
    let report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out.join("capture.json")).unwrap()).unwrap();
    assert_eq!(report["complete"], true);
    assert!(report["sheet"].as_str().unwrap().ends_with("sheet.png"));
    for shot in report["shots"].as_array().unwrap() {
        assert_eq!(shot["height"], image.height());
    }
}

/// A roomy `size` keeps a scrolling canvas from cutting the scene off,
/// so the slack it leaves around the drawing is not worth writing out.
#[cfg(not(target_vendor = "apple"))]
#[test]
fn a_shot_is_cropped_to_what_it_drew_unless_trim_is_off() {
    let dir = Utf8PathBuf::from_path_buf(std::env::temp_dir())
        .expect("a UTF-8 temp dir")
        .join("gallery-trim");
    let shot = |name: &str, trim: bool| Shot {
        scene: "documented-stage".to_owned(),
        out: Some(dir.join(format!("{name}.png"))),
        size: egui::vec2(900.0, 700.0),
        knobs: Overrides::default(),
        globals: Overrides::default(),
        frames: None,
        trim,
        settle: false,
        scale: DEFAULT_SCALE,
        list: false,
        template: false,
        play: None,
        record: None,
    };
    let capture = Capture {
        shots: vec![shot("cropped", true), shot("whole", false)],
        sheet: None,
        report: None,
    };
    render(
        &reference_scenes(),
        Renderer::Glow,
        &|_: &egui::Context| {},
        &capture,
    )
    .expect("both shots render");

    let read = |name: &str| {
        let png = std::fs::read(dir.join(format!("{name}.png"))).expect("the capture was written");
        image::load_from_memory_with_format(&png, image::ImageFormat::Png)
            .expect("a PNG")
            .to_rgba8()
    };
    let (cropped, whole) = (read("cropped"), read("whole"));

    assert_eq!(
        (whole.width(), whole.height()),
        (900, 700),
        "trim off writes the size asked for"
    );
    assert!(
        cropped.width() < whole.width() && cropped.height() < whole.height(),
        "{}×{} should sit inside the {}×{} it laid out in",
        cropped.width(),
        cropped.height(),
        whole.width(),
        whole.height()
    );
    assert_eq!(
        cropped.get_pixel(0, 0),
        whole.get_pixel(0, 0),
        "the crop keeps the origin, so the drawing itself is untouched"
    );
}

/// Shoot one of the template's wgpu scenes and read the PNG back.
///
/// Through [`Linked`](crate::Linked), which reads the test binary's inventory —
/// `scaffold_scenes` has filled it with the scenes a scaffold ships.
/// The PNG is named for the scene and the scale, so two shots of one scene
/// do not overwrite each other before both have been read.
fn wgpu_shot(
    scene: &str,
    size: egui::Vec2,
    scale: f32,
    knobs: &[(&str, &str)],
) -> image::RgbaImage {
    use crate::SceneSource as _;

    let out = Utf8PathBuf::from_path_buf(std::env::temp_dir())
        .expect("a UTF-8 temp dir")
        .join("gallery-wgpu")
        .join(format!("{}-at-{scale}.png", slug(scene)));
    let capture = Capture {
        shots: vec![Shot {
            scene: scene.to_owned(),
            out: Some(out.clone()),
            size,
            scale,
            knobs: knobs
                .iter()
                .map(|(key, value)| KnobOverride {
                    key: (*key).to_owned(),
                    value: (*value).to_owned(),
                })
                .collect(),
            globals: Overrides::default(),
            frames: None,
            trim: true,
            settle: false,
            list: false,
            template: false,
            play: None,
            record: None,
        }],
        sheet: None,
        report: None,
    };
    render(
        &crate::Linked.manifest(),
        Renderer::Wgpu,
        &|_: &egui::Context| {},
        &capture,
    )
    .expect("the shot renders");

    let png = std::fs::read(&out).expect("the capture was written");
    image::load_from_memory_with_format(&png, image::ImageFormat::Png)
        .expect("a PNG")
        .to_rgba8()
}

/// A wgpu paint callback is scene content egui's shapes never carry, so a shot has to be
/// checked against it twice over: the harness must hand the render state to the scene —
/// without it a capture still writes a clean PNG, minus everything the window would have
/// drawn through wgpu — and the trim must keep the whole rect the callback drew into.
/// The last row is where a short trim would show, the callback being the only thing on it.
///
/// The template's gradient scene is the subject: its end colours reach the shader only
/// through the uniform buffer its callback uploads.
/// Finding them in the pixels is evidence the uniforms arrived, not that a pipeline ran.
#[test]
fn a_wgpu_paint_callback_lands_in_a_trimmed_capture() {
    let asked = egui::vec2(640.0, 360.0);
    let image = wgpu_shot("wgpu::gradient", asked, DEFAULT_SCALE, &[]);
    // Down the middle, where the ramp runs.
    let column = |y: u32| image.get_pixel(image.width() / 2, y).0;
    let near = |pixel: [u8; 4], color: egui::Color32| {
        pixel
            .iter()
            .zip(color.to_array())
            .all(|(drawn, wanted)| drawn.abs_diff(wanted) <= 2)
    };
    // A pixel samples the middle of its row, so the first and last are a fraction
    // of the ramp short of the colours the ends themselves hold.
    let (first, last) = (column(0), column(image.height() - 1));
    assert!(
        near(first, crate::scaffold_scenes::wgpu::TOP),
        "the ramp starts from the top knob's colour at the first row: {first:?}"
    );
    assert!(
        near(last, crate::scaffold_scenes::wgpu::BOTTOM),
        "and reaches the bottom knob's at the last, so the trim cut nothing off it: {last:?}"
    );
    // The green channel spans 173 down to 43 between the two, so it reads the ramp
    // more finely than the eye does.
    let greens: Vec<u8> = (0..image.height()).map(|y| column(y)[1]).collect();
    assert!(
        greens.windows(2).all(|pair| pair[0] >= pair[1]),
        "one ramp top to bottom, never doubling back"
    );
    // The two ends of a band gradient sit flat; these have moved a fifth of the way.
    let fifth = image.height() / 5;
    assert!(
        greens[0] - greens[fifth as usize] > 10
            && greens[(image.height() - 1 - fifth) as usize] - greens[greens.len() - 1] > 10,
        "and it is under way at both ends rather than flat there: {greens:?}"
    );
}

/// One colour by the three routes a pixel can take — egui's own fill, a callback
/// into egui's render pass, and a pass into the target
/// [`SceneCtx::render_pass`](crate::SceneCtx::render_pass) hands over.
/// A step between them means shader-drawn content cannot sit beside egui-drawn content
/// without a seam, and a backdrop behind a component is exactly that arrangement.
///
/// The third is the loose one: it goes into a texture and is sampled back out, so it rounds
/// to eight bits twice and can land a step off. A step is the tolerance here — a colour
/// space read wrong would be tens of them.
#[test]
fn one_colour_comes_out_the_same_by_every_route_a_scene_can_draw_it() {
    let tint = "#6C9CD8";
    let image = wgpu_shot(
        "wgpu::colour",
        egui::vec2(400.0, 200.0),
        DEFAULT_SCALE,
        &[("tint", tint)],
    );
    let filled = egui::Color32::from_hex(tint)
        .expect("a hex colour")
        .to_array();

    // The three bands are flush and equally wide, so the middle of each third is inside one.
    let row = image.height() / 2;
    let across: Vec<[u8; 4]> = (0..image.width())
        .map(|x| image.get_pixel(x, row).0)
        .collect();
    let opaque = |drawn: &[u8; 4]| drawn[3] == u8::MAX && drawn != &crate::PANEL_BG.to_array();
    let first = across
        .iter()
        .position(opaque)
        .expect("the bands cross the middle row");
    let last = across
        .iter()
        .rposition(opaque)
        .expect("the bands cross the middle row");
    let band = (last - first + 1) / 3;
    for (route, at) in [
        ("egui's own fill", first + band / 2),
        ("a callback", first + band + band / 2),
        ("a pass of the scene's own", first + 2 * band + band / 2),
    ] {
        let drawn = across[at];
        let off = drawn
            .iter()
            .zip(filled)
            .map(|(drawn, wanted)| drawn.abs_diff(wanted))
            .max()
            .unwrap_or(u8::MAX);
        assert!(
            off <= 1,
            "{route} came out {drawn:?} against {filled:?}, {off} steps away"
        );
    }
}

/// egui's render pass carries no depth attachment, so a solid drawn straight into it
/// is sorted by submission order alone and its far faces land on the near ones.
/// A target from [`SceneCtx::render_pass`](crate::SceneCtx::render_pass) carries one,
/// and the same shader through it comes out a cube. The template's `depth` scene draws
/// both, side by side, so one shot holds the comparison.
///
/// Counted in saturated colours, which are the cube's faces and nothing else in the
/// picture: the sorted half shows the three faces turned towards the viewer,
/// the unsorted half shows whichever were drawn last as well.
/// It is also what says the offscreen pass ran headlessly at all,
/// going as it does through an encoder and a texture nothing else here touches.
///
/// The `side` override is not idle: a shot declares knobs on its first frame and applies
/// the recipe from the second, so asking for anything but the default is what makes the
/// scene reallocate gallery's target mid-run and re-point the texture egui already holds.
#[test]
fn a_pass_of_the_scenes_own_sorts_a_solid_the_egui_pass_cannot() {
    let shot = wgpu_shot(
        "wgpu::depth",
        egui::vec2(520.0, 300.0),
        DEFAULT_SCALE,
        &[("side", "140")],
    );
    // Greys are the chrome — checkerboard, panel, captions — and every face is a tint.
    // Binned, because a face that went through gallery's texture rounds to eight bits twice
    // and comes back as two neighbouring values; counted, so the blended pixels along an
    // edge can be left out below.
    let faces = |half: std::ops::Range<u32>| {
        let mut seen = std::collections::BTreeMap::new();
        for (x, y) in half.flat_map(|x| (0..shot.height()).map(move |y| (x, y))) {
            let [r, g, b, _] = shot.get_pixel(x, y).0;
            if r.max(g).max(b) - r.min(g).min(b) > 40 {
                *seen.entry([r >> 2, g >> 2, b >> 2]).or_insert(0_u32) += 1;
            }
        }
        seen
    };
    let middle = shot.width() / 2;
    let (unsorted, sorted) = (faces(0..middle), faces(middle..shot.width()));
    assert!(
        sorted.len() < unsorted.len(),
        "the half with a depth buffer leaves fewer faces showing: {} tints sorted, \
             {} unsorted",
        sorted.len(),
        unsorted.len()
    );

    // A face is thousands of pixels; anything under that is a blend along an edge.
    let faces = |tints: &std::collections::BTreeMap<[u8; 3], u32>| {
        tints.values().filter(|count| **count > 1_000).count()
    };
    assert_eq!(
        faces(&sorted),
        3,
        "the sorted half shows the three faces turned towards the viewer: {sorted:?}"
    );
    assert!(
        faces(&unsorted) > 3,
        "and the unsorted half shows more, having kept whichever were drawn last: {unsorted:?}"
    );
}

/// A window follows its display's scale factor while a capture takes the one it is given,
/// so the two draw the same scene at different pixel counts. `scale` is what lets a shot
/// say which, and this is the property it has to keep: a layout stated in points comes out
/// the same picture at that many times the pixels, while anything a shader measured
/// in device pixels is exactly where it was.
///
/// The template's `device pixels` scene holds both halves — a stage sized in points,
/// and a ring of a fixed pixel radius drawn inside it.
/// Doubling the scale doubles the PNG and leaves the ring alone.
/// It also covers the trim, which counts in pixels off a canvas measured in points
/// and would crop to a quarter of the image if it took one for the other.
#[test]
fn a_scale_takes_the_same_layout_to_more_pixels_and_leaves_device_pixels_alone() {
    let size = egui::vec2(400.0, 300.0);
    let single = wgpu_shot("wgpu::device", size, 1.0, &[]);
    let double = wgpu_shot("wgpu::device", size, 2.0, &[]);

    // Each side is rounded up to a whole pixel, so a doubled one can land just off twice.
    let doubled = |single: u32, double: u32| double.abs_diff(single * 2) <= 2;
    assert!(
        doubled(single.width(), double.width()) && doubled(single.height(), double.height()),
        "{}×{} at 1× should come back about {}×{} at 2×, not {}×{}",
        single.width(),
        single.height(),
        single.width() * 2,
        single.height() * 2,
        double.width(),
        double.height()
    );

    // The ring is the only strongly blue thing drawn: the rules are a dimmed version of it,
    // the checkerboard and the panel are grey, and the caption is grey text.
    let ring_across = |image: &image::RgbaImage| {
        let blue = |[r, _, b, _]: [u8; 4]| b > 180 && b.saturating_sub(r) > 60;
        let lit: Vec<u32> = (0..image.width())
            .filter(|x| (0..image.height()).any(|y| blue(image.get_pixel(*x, y).0)))
            .collect();
        let (first, last) = (lit.first().copied(), lit.last().copied());
        last.zip(first).map(|(last, first)| last - first + 1)
    };
    let single_ring = ring_across(&single).expect("the ring is drawn at 1×");
    let double_ring = ring_across(&double).expect("the ring is drawn at 2×");
    assert!(
        double_ring.abs_diff(single_ring) <= 4,
        "the ring is stated in device pixels, so it spans {single_ring} at 1× and should \
             span about as many at 2×, not {double_ring}"
    );
}

/// The consumer's layout: a fixed scrolling stage holding more rows than it can show,
/// on a canvas picked to sit just over it.
///
/// Their captures came back with the checkerboard running off the bottom edge
/// — the stage had taken the canvas, so there was no canvas left under it.
///
/// The bottom row of the picture is the thing that says whether it still does.
#[cfg(not(target_vendor = "apple"))]
#[test]
fn a_capture_ends_in_canvas_under_a_fixed_scrolling_stage() {
    fn scrolling_box(ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
        ctx.stage(
            ui,
            crate::Stage::Fixed(egui::vec2(900.0, 460.0)).scrollable(),
            |ui: &mut egui::Ui| {
                for row in 0..40 {
                    ui.label(format!("row {row}"));
                }
            },
        );
    }
    let manifest = Manifest {
        scenes: vec![SceneEntry {
            render: scrolling_box,
            name: "scrolling-box",
            module_path: "reference",
            default: true,
            order: 0,
            source: "",
        }],
        groups: Vec::new(),
    };
    let out = Utf8PathBuf::from_path_buf(std::env::temp_dir())
        .expect("a UTF-8 temp dir")
        .join("gallery-margin")
        .join("scrolling-box.png");
    let capture = Capture {
        shots: vec![Shot {
            scene: "scrolling-box".to_owned(),
            out: Some(out.clone()),
            // The consumer's own numbers: a canvas picked
            // to sit just over a 900×460 stage.
            size: egui::vec2(964.0, 520.0),
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
    let bottom = image.height() - 1;
    for x in 0..image.width() {
        assert_eq!(
            image.get_pixel(x, bottom).0,
            PANEL_BG.to_array(),
            "({x}, {bottom}) of {}×{} is canvas under the stage, not the stage itself",
            image.width(),
            image.height()
        );
    }
}

/// A scene that panics fails its own shot, naming its line, and the run goes on to report it.
#[cfg(not(target_vendor = "apple"))]
#[test]
fn a_panicking_scene_fails_its_shot_naming_the_line_that_panicked() {
    fn boom(ctx: &mut crate::SceneCtx<'_>, _: &mut egui::Ui) {
        if ctx.toggle("armed", true).get() {
            panic!("boom");
        }
    }
    // What `#[scene]` registers: the scene, run where its panic can be caught.
    fn caught_boom(ctx: &mut crate::SceneCtx<'_>, ui: &mut egui::Ui) {
        crate::__run_scene(ctx, ui, "boom", boom);
    }
    let manifest = Manifest {
        scenes: vec![SceneEntry {
            render: caught_boom,
            name: "boom",
            module_path: "reference",
            default: true,
            order: 0,
            source: "",
        }],
        groups: Vec::new(),
    };
    let capture = Capture {
        shots: vec![Shot {
            scene: "boom".to_owned(),
            out: Some(
                Utf8PathBuf::from_path_buf(std::env::temp_dir())
                    .expect("a UTF-8 temp dir")
                    .join("gallery-scene-panic/boom.png"),
            ),
            size: egui::vec2(120.0, 60.0),
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
    let failure = render(&manifest, Renderer::Glow, &|_: &egui::Context| {}, &capture)
        .expect_err("the scene panicked")
        .summary();
    assert!(failure.contains("scene `boom` panicked: boom"), "{failure}");
    assert!(
        failure.contains("capture.rs:"),
        "names the scene's own line: {failure}"
    );
}
