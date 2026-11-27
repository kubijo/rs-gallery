use super::*;

#[test]
fn a_reversed_range_takes_a_value_between_its_ends() {
    let mut store = vec![
        KnobState::Slider {
            label: "gain".to_owned(),
            value: 0.0,
            min: 1.0,
            max: 0.0,
            step: 0.0,
        },
        KnobState::Pad2D {
            label: "aim".to_owned(),
            x: 0.0,
            y: 0.0,
            min_x: 1.0,
            max_x: -1.0,
            min_y: 0.0,
            max_y: 1.0,
            invert_y: false,
        },
    ];
    applied(&mut store, &[("gain", "0.25"), ("aim", "-0.5,0.5")])
        .expect("the panel takes either way round, so the recipe does too");
    assert!(matches!(store[0], KnobState::Slider { value, .. } if value == 0.25));
    assert!(matches!(store[1], KnobState::Pad2D { x, .. } if x == -0.5));
    assert!(
        applied(&mut store, &[("gain", "2")]).is_err(),
        "past both ends is still outside"
    );
}

#[test]
fn a_value_no_range_can_take_fails_at_once_and_says_why() {
    let pad = || KnobState::Pad2D {
        label: "aim".to_owned(),
        x: 0.0,
        y: 0.0,
        min_x: -1.0,
        max_x: 1.0,
        min_y: -1.0,
        max_y: 1.0,
        invert_y: false,
    };
    let cases = [
        (pad(), "500,abc", "`abc` is not a number"),
        (pad(), "NaN,0", "`NaN` is not a number"),
        (slider("aim"), "NaN", "`NaN` is not a number"),
        (
            KnobState::Slider {
                label: "aim".to_owned(),
                value: 0.5,
                min: 0.0,
                max: 1.0,
                step: f32::INFINITY,
            },
            "0.25",
            "steps by inf",
        ),
    ];
    for (knob, value, says) in cases {
        let mut store = [knob];
        let error = Diagnostic::from(
            apply(&mut store, &knobs(&[("aim", value)])).expect_err("no later frame helps"),
        )
        .summary();
        assert!(error.contains(says), "`{value}`: {error}");
    }
}

#[test]
fn a_knob_sharing_a_label_with_a_group_or_button_is_still_reachable() {
    let mut store = vec![
        KnobState::Group {
            label: "x".to_owned(),
        },
        KnobState::Button {
            label: "x".to_owned(),
            clicked: false,
        },
        slider("x"),
    ];
    applied(&mut store, &[("x", "0.5")]).expect("the exact label reaches the slider");
    assert!(matches!(store[2], KnobState::Slider { value, .. } if value == 0.5));
    applied(&mut store, &[("^x$", "1.5")]).expect("so does a regex");
    assert!(matches!(store[2], KnobState::Slider { value, .. } if value == 1.5));
}

#[test]
fn every_knob_kind_takes_its_value() {
    let mut store = vec![
        KnobState::Text {
            label: "caption".to_owned(),
            value: String::new(),
        },
        KnobState::Toggle {
            label: "night".to_owned(),
            value: false,
        },
        slider("speed"),
        KnobState::Color {
            label: "tint".to_owned(),
            value: egui::Color32::BLACK,
        },
        select("body", &["sedan", "suv"]),
        KnobState::Pad2D {
            label: "offset".to_owned(),
            x: 0.0,
            y: 0.0,
            min_x: -1.0,
            max_x: 1.0,
            min_y: -1.0,
            max_y: 1.0,
            invert_y: false,
        },
    ];
    let left = applied(
        &mut store,
        &[
            ("caption", "hi"),
            ("night", "true"),
            ("speed", "1.5"),
            ("tint", "#FF8800"),
            ("body", "suv"),
            ("offset", "0.25,-0.5"),
        ],
    )
    .expect("every key names a declared knob");
    assert_eq!(left, 0, "nothing left pending");

    assert!(matches!(&store[0], KnobState::Text { value, .. } if value == "hi"));
    assert!(matches!(store[1], KnobState::Toggle { value: true, .. }));
    assert!(matches!(store[2], KnobState::Slider { value, .. } if value == 1.5));
    assert!(
        matches!(store[3], KnobState::Color { value, .. } if value == egui::Color32::from_rgb(0xFF, 0x88, 0x00))
    );
    assert!(matches!(store[4], KnobState::Select { value: 1, .. }));
    assert!(matches!(store[5], KnobState::Pad2D { x, y, .. } if x == 0.25 && y == -0.5));

    store.extend([
        KnobState::Group {
            label: "group".to_owned(),
        },
        KnobState::Button {
            label: "action".to_owned(),
            clicked: true,
        },
        KnobState::IconButtons {
            label: "Language".to_owned(),
            value: 1,
            options: vec!["English".to_owned(), "Finnish".to_owned()],
            icons: Vec::new(),
        },
    ]);
    let report = serde_json::to_value(control_values(&store)).unwrap();
    assert_eq!(
        report,
        serde_json::json!({
            "caption": "hi", "night": true, "speed": 1.5, "tint": "#ff8800ff",
            "body": "suv", "offset": "0.25,-0.5", "Language": "Finnish",
        })
    );
}

#[test]
fn a_key_may_be_a_regex_but_an_exact_label_wins() {
    let mut store = vec![slider("width"), slider("width (chars)")];
    // As a regex `width (chars)` is a group matching `width chars`, which no label is.
    assert_eq!(
        applied(&mut store, &[("width (chars)", "1.5")]).expect("the literal label"),
        0
    );
    assert!(matches!(store[1], KnobState::Slider { value, .. } if value == 1.5));
    assert!(
        matches!(store[0], KnobState::Slider { value, .. } if value == 1.0),
        "the other knob is untouched"
    );

    let mut pads = vec![slider("speed"), slider("angle")];
    assert_eq!(
        applied(&mut pads, &[("^ANG", "0.5")]).expect("case-insensitive regex"),
        0
    );
    assert!(matches!(pads[1], KnobState::Slider { value, .. } if value == 0.5));
}

#[test]
fn a_key_matching_several_knobs_is_an_error_naming_them() {
    let mut store = vec![slider("door front left"), slider("door front right")];
    let message = applied(&mut store, &[("door front", "1.0")])
        .expect_err("ambiguous")
        .plain();
    assert!(message.contains("2 knobs"), "says how many: {message}");
    assert!(message.contains("door front right"), "names them");
}

#[test]
fn a_choice_set_by_index_still_works_but_an_option_label_wins() {
    let mut numeric = vec![select("gear", &["2", "1", "0"])];
    applied(&mut numeric, &[("gear", "1")]).expect("declared");
    assert!(
        matches!(numeric[0], KnobState::Select { value: 1, .. }),
        "`1` is the option at index 1, not the index 1 read as a number"
    );

    let mut named = vec![select("body", &["sedan", "suv"])];
    applied(&mut named, &[("body", "1")]).expect("declared");
    assert!(
        matches!(named[0], KnobState::Select { value: 1, .. }),
        "no option is spelled `1`, so it falls back to an index"
    );
}

#[test]
fn a_value_the_knob_cannot_take_is_an_error_not_a_silent_default() {
    let cases: [(KnobState, &str); 8] = [
        (
            KnobState::Button {
                label: "Reset".to_owned(),
                clicked: false,
            },
            "true",
        ),
        (
            KnobState::Toggle {
                label: "night".to_owned(),
                value: false,
            },
            "yes",
        ),
        (slider("speed"), "nine"),
        (slider("speed"), "9"),
        (
            KnobState::Color {
                label: "tint".to_owned(),
                value: egui::Color32::BLACK,
            },
            "orange",
        ),
        (select("body", &["sedan", "suv"]), "coupe"),
        (select("body", &["sedan", "suv"]), "7"),
        (
            KnobState::Group {
                label: "layout".to_owned(),
            },
            "on",
        ),
    ];
    for (knob, value) in cases {
        let key = knob.label().to_owned();
        let mut store = [knob];
        assert!(
            applied(&mut store, &[(&key, value)]).is_err(),
            "`{key}={value}` should be rejected"
        );
    }
}

#[test]
fn an_unmatched_key_stays_pending_rather_than_erroring_on_the_spot() {
    // It may name a knob that only appears once another is set,
    // so it gets every frame to show up.
    let mut store = vec![KnobState::Toggle {
        label: "night".to_owned(),
        value: false,
    }];
    let mut ledger = Ledger::new(knobs(&[("night", "true"), ("headlights", "true")]));
    ledger
        .apply_knobs(&mut store)
        .expect("an unmatched key is not yet a failure");
    let unmatched: Vec<&KnobOverride> = ledger.unlanded().collect();
    assert_eq!(unmatched.len(), 1);
    assert_eq!(unmatched[0].key, "headlights");

    let message = unknown_keys(KeyKind::Knob, &unmatched, &store).plain();
    assert!(message.contains("`headlights`"), "names what was not found");
    assert!(
        message.contains("night"),
        "lists what the scene does declare"
    );

    // A knob that took its value and then vanished stays satisfied: the recipe was honoured.
    store.clear();
    ledger
        .apply_knobs(&mut store)
        .expect("an empty store is not a failure");
    assert_eq!(
        ledger.unlanded().count(),
        1,
        "`night` landed once and does not come back"
    );
}

/// The flip side of re-applying: a regex key resolves anew each frame,
/// so a knob appearing mid-settle can turn it ambiguous — which errors.
#[test]
fn a_key_turning_ambiguous_on_a_later_frame_is_an_error() {
    let mut store = vec![slider("speed")];
    let owned = knobs(&[("spe.*", "1.5")]);
    apply(&mut store, &owned).expect("one match applies");

    store.push(slider("spectrum"));
    let failure =
        apply(&mut store, &owned).expect_err("two matches can no longer say which knob is meant");
    assert!(failure.error.plain().contains("matches 2 knobs"));
}

#[test]
fn a_recipe_value_is_reasserted_over_a_scenes_own_write() {
    let mut store = vec![slider("speed")];
    let mut ledger = Ledger::new(knobs(&[("speed", "1.5")]));
    ledger
        .apply_knobs(&mut store)
        .expect("the key names its knob");
    assert_eq!(ledger.unlanded().count(), 0);

    // Stand in for a scene writing the knob back mid-frame.
    if let KnobState::Slider { value, .. } = &mut store[0] {
        *value = 1.9;
    }
    ledger
        .apply_knobs(&mut store)
        .expect("re-applying is idempotent");
    assert!(
        matches!(store[0], KnobState::Slider { value, .. } if value == 1.5),
        "the recipe's word is the last one"
    );
}

/// The precedence rule end to end: a scene nudging its own knob every frame
/// loses to the recipe, in the pixels and in the store a listing reads.
#[cfg(not(target_vendor = "apple"))]
#[test]
fn a_recipe_outlives_a_scene_writing_its_own_knob() {
    fn self_nudging(ctx: &mut crate::SceneCtx<'_>, _ui: &mut egui::Ui) {
        let speed = ctx.slider("speed", 1.0, 0.0, 100.0, 0.0);
        speed.set(speed.get() + 1.0);
    }
    let scene = SceneEntry {
        render: self_nudging,
        name: "self-nudging",
        module_path: "reference",
        default: true,
        order: 0,
        source: "",
    };
    let shot = Shot {
        scene: "self-nudging".to_owned(),
        out: None,
        size: egui::vec2(320.0, 200.0),
        knobs: knobs(&[("speed", "5")]),
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
    let harness = draw(scene, &glow(), &|_: &egui::Context| {}, &shot, shot.size)
        .expect("the shot draws")
        .harness;
    assert!(
        matches!(harness.state().knobs[0], KnobState::Slider { value, .. } if value == 5.0),
        "the recipe holds the knob, so the scene's writes are refused"
    );
}

/// A scene correcting its own knob every frame fights a recipe holding it elsewhere,
/// and each write would ask for a frame, so the shot would never settle.
#[cfg(not(target_vendor = "apple"))]
#[test]
fn a_scene_correcting_a_recipe_held_knob_still_settles_on_the_recipe() {
    thread_local! {
        static SEEN: std::cell::RefCell<Vec<(bool, f32)>> = const { std::cell::RefCell::new(Vec::new()) };
    }
    fn capped(ctx: &mut crate::SceneCtx<'_>, _ui: &mut egui::Ui) {
        let count = ctx.slider("count", 1.0, 0.0, 9.0, 1.0);
        let landed = count.set(count.get().min(3.0));
        SEEN.with_borrow_mut(|seen| seen.push((landed, count.get())));
    }
    let scene = SceneEntry {
        render: capped,
        name: "capped",
        module_path: "reference",
        default: true,
        order: 0,
        source: "",
    };
    let shot = Shot {
        scene: "capped".to_owned(),
        out: None,
        size: egui::vec2(320.0, 200.0),
        knobs: knobs(&[("count", "5")]),
        globals: Overrides::default(),
        frames: None,
        trim: true,
        settle: true,
        scale: DEFAULT_SCALE,
        list: false,
        template: false,
        play: None,
        record: None,
    };
    let drawn =
        draw(scene, &glow(), &|_: &egui::Context| {}, &shot, shot.size).expect("the shot draws");
    assert_eq!(drawn.frames.motion, Motion::Settled, "{:?}", drawn.frames);
    let seen = SEEN.take();
    let held = seen
        .iter()
        .position(|frame| *frame == (false, 5.0))
        .expect("some frame draws the held value");
    assert!(
        seen[held..].iter().all(|frame| *frame == (false, 5.0)),
        "once held, the write is refused and the scene draws the recipe's value: {seen:?}"
    );
}

/// A range that follows another knob is last frame's when the recipe applies,
/// so a value past it waits for the range to catch up, and fails only if it never does.
#[cfg(not(target_vendor = "apple"))]
#[test]
fn a_recipe_value_waits_for_a_range_that_follows_another_knob() {
    fn dependent(ctx: &mut crate::SceneCtx<'_>, _ui: &mut egui::Ui) {
        let max = ctx.slider("max", 5.0, 1.0, 20.0, 1.0).get();
        ctx.slider("x", 1.0, 0.0, max, 1.0);
    }
    let scene = SceneEntry {
        render: dependent,
        name: "dependent",
        module_path: "reference",
        default: true,
        order: 0,
        source: "",
    };
    let shot = |x: &str| Shot {
        scene: "dependent".to_owned(),
        out: None,
        size: egui::vec2(320.0, 200.0),
        knobs: knobs(&[("max", "10"), ("x", x)]),
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
    let reached = shot("8");
    let drawn = draw(
        scene,
        &glow(),
        &|_: &egui::Context| {},
        &reached,
        reached.size,
    )
    .expect("the range widens before the last apply");
    assert!(
        matches!(drawn.harness.state().knobs[1], KnobState::Slider { value, .. } if value == 8.0)
    );
    let never = shot("12");
    let error = draw(scene, &glow(), &|_: &egui::Context| {}, &never, never.size)
        .err()
        .expect("12 stays past the widened range");
    assert!(
        error.summary().contains("takes 0 ..= 10; 12 is outside it"),
        "{}",
        error.summary()
    );
}

/// Like a range, an option list or a knob hidden behind a same-label group
/// can follow another knob, so a value for it waits rather than failing at once.
#[test]
fn a_recipe_value_waits_for_an_option_or_knob_another_knob_brings() {
    fn gated(ctx: &mut crate::SceneCtx<'_>, _ui: &mut egui::Ui) {
        let pro = ctx.toggle("pro", false).get();
        let options: &[&str] = if pro {
            &["Basic", "Advanced"]
        } else {
            &["Basic"]
        };
        ctx.select("mode", options, 0);
        ctx.group("size");
        if pro {
            ctx.slider("size", 1.0, 0.0, 10.0, 1.0);
        }
    }
    let scene = SceneEntry {
        render: gated,
        name: "gated",
        module_path: "reference",
        default: true,
        order: 0,
        source: "",
    };
    let shot = |pairs: &[(&str, &str)]| Shot {
        scene: "gated".to_owned(),
        out: None,
        size: egui::vec2(320.0, 200.0),
        knobs: knobs(pairs),
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
    let reached = shot(&[("pro", "true"), ("mode", "Advanced"), ("size", "7")]);
    let drawn = draw(
        scene,
        &glow(),
        &|_: &egui::Context| {},
        &reached,
        reached.size,
    )
    .expect("the option and the slider arrive once `pro` lands");
    let knobs = &drawn.harness.state().knobs;
    assert!(matches!(knobs[1], KnobState::Select { value: 1, .. }));
    assert!(matches!(knobs[3], KnobState::Slider { value, .. } if value == 7.0));

    for (pairs, says) in [
        (&[("mode", "Advanced")][..], "has no option `Advanced`"),
        (&[("size", "7")][..], "`size` is a group heading"),
    ] {
        let never = shot(pairs);
        let error = draw(scene, &glow(), &|_: &egui::Context| {}, &never, never.size)
            .err()
            .expect("without `pro` it never arrives");
        assert!(error.summary().contains(says), "{}", error.summary());
    }
}

/// A value past its range that the recipe then hides must not pass for one that landed,
/// and a misspelt key in the same recipe is reported first.
#[cfg(not(target_vendor = "apple"))]
#[test]
fn a_recipe_value_that_never_lands_fails_even_once_hidden_and_after_a_typo() {
    fn shown(ctx: &mut crate::SceneCtx<'_>, _ui: &mut egui::Ui) {
        if ctx.toggle("show", true).get() {
            ctx.slider("x", 1.0, 0.0, 10.0, 1.0);
        }
    }
    let scene = SceneEntry {
        render: shown,
        name: "shown",
        module_path: "reference",
        default: true,
        order: 0,
        source: "",
    };
    let shot = |pairs: &[(&str, &str)]| Shot {
        scene: "shown".to_owned(),
        out: None,
        size: egui::vec2(320.0, 200.0),
        knobs: knobs(pairs),
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
    let failure = |shot: &Shot| {
        draw(scene, &glow(), &|_: &egui::Context| {}, shot, shot.size)
            .err()
            .expect("the shot fails")
            .summary()
    };
    let hidden = failure(&shot(&[("x", "12"), ("show", "false")]));
    assert!(hidden.contains("12 is outside it"), "{hidden}");
    let typo = failure(&shot(&[("x", "12"), ("nope", "1")]));
    assert!(typo.contains("no knob matching `nope`"), "{typo}");
}

/// While `x` is hidden, the key `x` reads as a regex that lands on `max`;
/// once `x` shows, the key moving to it fails rather than leaving `max` set as well.
#[cfg(not(target_vendor = "apple"))]
#[test]
fn a_recipe_key_that_moves_to_another_knob_fails() {
    fn shadowed(ctx: &mut crate::SceneCtx<'_>, _ui: &mut egui::Ui) {
        ctx.slider("max", 1.0, 0.0, 10.0, 1.0);
        if ctx.toggle("show", false).get() {
            ctx.slider("x", 1.0, 0.0, 10.0, 1.0);
        }
    }
    let scene = SceneEntry {
        render: shadowed,
        name: "shadowed",
        module_path: "reference",
        default: true,
        order: 0,
        source: "",
    };
    let shot = Shot {
        scene: "shadowed".to_owned(),
        out: None,
        size: egui::vec2(320.0, 200.0),
        knobs: knobs(&[("x", "5"), ("show", "true")]),
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
    let error = draw(scene, &glow(), &|_: &egui::Context| {}, &shot, shot.size)
        .err()
        .expect("the key moved")
        .summary();
    assert!(
        error.contains("knob key `x` matched `max`, then `x`"),
        "{error}"
    );
}
