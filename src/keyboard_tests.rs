//! Input regressions run through the complete Gallery, including chrome and scene selection.

use egui::{Id, Key, Modifiers};
use egui_kittest::{Harness, kittest::Queryable as _};

use crate::{
    Gallery, Manifest, Renderer, SceneCtx, SceneEntry, SceneRevision, SceneSource, Settings,
    test_support::{group, scene},
};

#[derive(Clone, Default)]
struct Observed {
    keys: Vec<(Key, bool)>,
    fields: Vec<Id>,
    first: String,
    second: String,
    code: String,
    revision: Option<SceneRevision>,
    discard: bool,
}

fn observed(ctx: &egui::Context) -> Observed {
    ctx.data_mut(|data| {
        data.get_temp_mut_or_default::<Observed>(Id::new("key-test"))
            .clone()
    })
}

fn capture(ctx: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
    content(ctx, ui, "capture", true);
}

fn replacement(ctx: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
    content(ctx, ui, "replacement", true);
}

fn passive(ctx: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
    content(ctx, ui, "capture", false);
}

fn content(ctx: &mut SceneCtx<'_>, ui: &mut egui::Ui, salt: &str, claims: bool) {
    let mut seen = observed(ui.ctx());
    seen.revision = Some(ctx.scene_revision());
    let (_, rect) = ui.allocate_space(egui::vec2(180.0, 50.0));
    let response = ui.interact(rect, ui.make_persistent_id(salt), egui::Sense::click());
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, "Capture keys"));
    if response.clicked() {
        response.request_focus();
    }
    if response.has_focus() {
        ui.memory_mut(|memory| {
            memory.set_focus_lock_filter(
                response.id,
                egui::EventFilter {
                    tab: claims,
                    escape: claims,
                    ..Default::default()
                },
            )
        });
        if claims {
            let mut released = false;
            ui.input_mut(|input| {
                input.events.retain(|event| {
                    let egui::Event::Key {
                        key,
                        pressed: true,
                        modifiers,
                        ..
                    } = event
                    else {
                        return true;
                    };
                    if !released
                        && matches!(key, Key::Tab | Key::Escape)
                        && (modifiers.is_none() || modifiers.shift_only())
                    {
                        seen.keys.push((*key, modifiers.shift));
                        released = *key == Key::Escape;
                        false
                    } else {
                        true
                    }
                })
            });
            if released {
                response.surrender_focus();
            }
        }
    }
    seen.fields.clear();
    for (label, value) in [
        ("First input", &mut seen.first),
        ("Second input", &mut seen.second),
    ] {
        let label = ui.label(label);
        seen.fields
            .push(ui.text_edit_singleline(value).labelled_by(label.id).id);
    }
    let label = ui.label("Code input");
    seen.fields.push(
        ui.add(egui::TextEdit::multiline(&mut seen.code).code_editor())
            .labelled_by(label.id)
            .id,
    );
    let _ = ctx.text("Control input", "");
    let _ = ui.button("Scene button");
    if seen.discard {
        seen.discard = false;
        ui.ctx().request_discard("exercise a second layout pass");
    }
    ui.data_mut(|data| data.insert_temp(Id::new("key-test"), seen));
}

struct Source {
    scenes: Vec<SceneEntry>,
    revision: SceneRevision,
}

impl SceneSource for Source {
    fn manifest(&mut self) -> Manifest {
        Manifest {
            scenes: self.scenes.clone(),
            groups: vec![group("keys", "Keyboard")],
        }
    }

    fn scene_revision(&self) -> SceneRevision {
        self.revision
    }
}

type Shell = Harness<'static, Gallery<Source>>;

fn shell() -> Shell {
    let scenes = ["capture", "other", "third"]
        .map(|name| SceneEntry {
            render: capture,
            ..scene(name, "keys", false)
        })
        .to_vec();
    let mut harness = Harness::builder()
        .with_size(egui::vec2(1280.0, 750.0))
        .build_eframe(|cc| {
            crate::install_context(cc, |_| {});
            Gallery::new(
                Source {
                    scenes,
                    revision: SceneRevision::INITIAL,
                },
                Settings::new(Renderer::Wgpu),
                None,
                None,
            )
        });
    harness.run();
    harness
}

fn click(harness: &mut Shell, label: &str) {
    harness.get_by_label(label).click();
    harness.run();
}

fn press(harness: &mut Shell, key: Key, modifiers: Modifiers) {
    // No modifier-only warmup frame: a reload and a key can arrive in the same frame.
    harness
        .input_mut()
        .events
        .push(egui::Event::ModifiersChanged(modifiers));
    harness.input_mut().events.push(egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    });
    harness.step();
    harness.key_up(key);
    harness.event(egui::Event::ModifiersChanged(Modifiers::NONE));
    harness.run();
}

fn selected(harness: &Shell) -> &str {
    harness.state().state.selected.as_deref().unwrap()
}

#[test]
fn focused_scene_consumes_navigation_and_escape_releases_without_clearing_filter() {
    let mut harness = shell();
    harness.state_mut().state.filter = "Keyboard".to_owned();
    click(&mut harness, "Capture keys");
    for (key, modifiers) in [
        (Key::Tab, Modifiers::NONE),
        (Key::Tab, Modifiers::SHIFT),
        (Key::Escape, Modifiers::NONE),
    ] {
        press(&mut harness, key, modifiers);
        assert_eq!(selected(&harness), "keys::capture");
        assert_eq!(harness.state().state.filter, "Keyboard");
    }
    assert_eq!(
        observed(&harness.ctx).keys,
        [(Key::Tab, false), (Key::Tab, true), (Key::Escape, false)]
    );
    assert_eq!(harness.ctx.memory(|m| m.focused()), None);
    press(&mut harness, Key::Escape, Modifiers::NONE);
    assert!(harness.state().state.filter.is_empty());
    press(&mut harness, Key::Tab, Modifiers::NONE);
    assert_eq!(selected(&harness), "keys::other");
}

#[test]
fn a_claimed_escape_releases_later_keys_in_the_same_frame() {
    let mut harness = shell();
    harness.state_mut().state.filter = "Keyboard".to_owned();
    click(&mut harness, "Capture keys");
    for key in [Key::Escape, Key::Tab] {
        harness.input_mut().events.push(egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        });
    }
    harness.step();
    assert_eq!(observed(&harness.ctx).keys, [(Key::Escape, false)]);
    assert_eq!(harness.state().state.filter, "Keyboard");
    assert_eq!(selected(&harness), "keys::other");
    assert_eq!(harness.ctx.memory(|memory| memory.focused()), None);
}

#[test]
fn unclaimed_tabs_cycle_both_ways_even_with_a_button_focused() {
    let mut harness = shell();
    for (index, expected) in ["keys::other", "keys::third", "keys::capture"]
        .repeat(3)
        .into_iter()
        .enumerate()
    {
        press(&mut harness, Key::Tab, Modifiers::NONE);
        assert_eq!(
            selected(&harness),
            expected,
            "press {index}: focus {:?}, text {}, claimed {:?}",
            harness.ctx.memory(|memory| memory.focused()),
            harness.ctx.text_edit_focused(),
            observed(&harness.ctx).keys,
        );
    }
    press(&mut harness, Key::Tab, Modifiers::SHIFT);
    assert_eq!(selected(&harness), "keys::third");
    press(&mut harness, Key::Tab, Modifiers::NONE);
    assert_eq!(selected(&harness), "keys::capture");
    harness.get_by_label("Scene button").focus();
    harness.run();
    assert!(harness.ctx.memory(|m| m.focused()).is_some());
    press(&mut harness, Key::Tab, Modifiers::NONE);
    assert_eq!(selected(&harness), "keys::other");
    assert!(observed(&harness.ctx).keys.is_empty());

    harness.state_mut().state.filter = "other".to_owned();
    press(&mut harness, Key::Tab, Modifiers::NONE);
    assert_eq!(selected(&harness), "keys::other");
    harness.state_mut().state.filter = "no matches".to_owned();
    press(&mut harness, Key::Tab, Modifiers::SHIFT);
    assert_eq!(selected(&harness), "keys::other");
}

#[test]
fn text_fields_traverse_and_code_editor_indents_without_switching_scenes() {
    let mut harness = shell();
    click(&mut harness, "Capture keys");
    click(&mut harness, "First input");
    harness.event(egui::Event::Text("hello".to_owned()));
    harness.run();
    assert_eq!(observed(&harness.ctx).first, "hello");
    press(&mut harness, Key::Tab, Modifiers::NONE);
    let fields = observed(&harness.ctx).fields;
    assert_eq!(harness.ctx.memory(|m| m.focused()), Some(fields[1]));
    press(&mut harness, Key::Tab, Modifiers::SHIFT);
    assert_eq!(harness.ctx.memory(|m| m.focused()), Some(fields[0]));
    click(&mut harness, "Code input");
    press(&mut harness, Key::Tab, Modifiers::NONE);
    assert_eq!(observed(&harness.ctx).code, "\t");
    press(&mut harness, Key::Tab, Modifiers::SHIFT);
    assert_eq!(observed(&harness.ctx).code, "");
    assert_eq!(selected(&harness), "keys::capture");
    assert!(observed(&harness.ctx).keys.is_empty());
}

#[test]
fn shell_text_fields_keep_navigation_and_global_shortcuts() {
    let mut harness = shell();
    click(&mut harness, "Capture keys");
    press(&mut harness, Key::F, Modifiers::COMMAND);
    assert_eq!(
        harness.ctx.memory(|m| m.focused()),
        Some(crate::filter_id())
    );
    harness.event(egui::Event::Text("Keyboard".to_owned()));
    harness.run();
    press(&mut harness, Key::Tab, Modifiers::NONE);
    assert_eq!(selected(&harness), "keys::capture");
    assert_ne!(
        harness.ctx.memory(|m| m.focused()),
        Some(crate::filter_id())
    );
    click(&mut harness, "Control input");
    harness.event(egui::Event::Text("control".to_owned()));
    harness.run();
    assert!(harness.ctx.text_edit_focused());
    press(&mut harness, Key::Tab, Modifiers::NONE);
    assert_eq!(selected(&harness), "keys::capture");
    press(&mut harness, Key::Escape, Modifiers::NONE);
    assert!(harness.state().state.filter.is_empty());

    for (key, field) in [(Key::L, true), (Key::R, false)] {
        press(&mut harness, key, Modifiers::COMMAND | Modifiers::SHIFT);
        assert!(
            !(if field {
                harness.state().state.show_scenes
            } else {
                harness.state().state.show_controls
            })
        );
        press(&mut harness, key, Modifiers::COMMAND | Modifiers::SHIFT);
    }
    harness.key_press_modifiers(Modifiers::COMMAND, Key::B);
    harness.step(); // The performance window deliberately keeps repainting.
    assert!(harness.state().state.show_perf);
    press(&mut harness, Key::B, Modifiers::COMMAND);
    assert!(!harness.state().state.show_perf);
}

#[test]
fn scene_and_source_view_changes_drop_keyboard_ownership() {
    let mut harness = shell();
    click(&mut harness, "Capture keys");
    click(&mut harness, "Other");
    assert_eq!(selected(&harness), "keys::other");
    press(&mut harness, Key::Tab, Modifiers::NONE);
    assert_eq!(selected(&harness), "keys::third");
    click(&mut harness, "Capture keys");
    press(&mut harness, Key::Tab, Modifiers::NONE);
    assert_eq!(selected(&harness), "keys::third");
    click(&mut harness, "Source");
    press(&mut harness, Key::Tab, Modifiers::NONE);
    assert_eq!(selected(&harness), "keys::capture");
}

#[test]
fn reload_preserves_live_focus_and_drops_removed_or_changed_claims() {
    let mut harness = shell();
    click(&mut harness, "Capture keys");
    harness.state_mut().source.revision = SceneRevision::INITIAL.next();
    press(&mut harness, Key::Tab, Modifiers::NONE);
    assert_eq!(selected(&harness), "keys::capture");
    assert_eq!(
        observed(&harness.ctx).revision,
        Some(SceneRevision::INITIAL.next())
    );

    harness.state_mut().source.scenes[0].render = passive;
    press(&mut harness, Key::Tab, Modifiers::SHIFT);
    assert_eq!(selected(&harness), "keys::third");
    click(&mut harness, "Capture");
    harness.state_mut().source.scenes[0].render = capture;
    harness.run();
    click(&mut harness, "Capture keys");
    harness.state_mut().source.scenes[0].render = replacement;
    press(&mut harness, Key::Tab, Modifiers::NONE);
    assert_eq!(selected(&harness), "keys::other");
    click(&mut harness, "Capture keys");
    harness.state_mut().source.scenes.remove(1);
    press(&mut harness, Key::Tab, Modifiers::NONE);
    assert_eq!(selected(&harness), "keys::third");
}

#[test]
fn removing_a_focused_text_field_does_not_swallow_the_next_tab() {
    fn empty(_ctx: &mut SceneCtx<'_>, ui: &mut egui::Ui) {
        ui.label("No inputs");
    }
    for remove_scene in [false, true] {
        let mut harness = shell();
        click(&mut harness, "First input");
        harness.state_mut().source.revision = SceneRevision::INITIAL.next();
        if remove_scene {
            harness.state_mut().source.scenes.remove(0);
        } else {
            harness.state_mut().source.scenes[0].render = empty;
        }
        press(&mut harness, Key::Tab, Modifiers::NONE);
        assert_eq!(
            selected(&harness),
            if remove_scene {
                "keys::third"
            } else {
                "keys::other"
            },
        );
    }
}

#[test]
fn batched_navigation_preserves_order_and_ignores_other_modifiers_and_releases() {
    let mut harness = shell();
    harness.state_mut().state.filter = "capture".to_owned();
    for key in [Key::Escape, Key::Tab, Key::Tab] {
        harness.input_mut().events.push(egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        });
    }
    harness.run();
    assert_eq!(selected(&harness), "keys::third");
    for modifiers in [Modifiers::CTRL, Modifiers::ALT] {
        press(&mut harness, Key::Tab, modifiers);
        assert_eq!(selected(&harness), "keys::third");
    }
    harness.key_up(Key::Tab);
    harness.run();
    assert_eq!(selected(&harness), "keys::third");

    // Preserve the old consume_key(NONE, Escape) modifier matching.
    for modifiers in [Modifiers::NONE, Modifiers::SHIFT, Modifiers::ALT] {
        harness.state_mut().state.filter = "capture".to_owned();
        press(&mut harness, Key::Escape, modifiers);
        assert!(harness.state().state.filter.is_empty());
    }
    for modifiers in [Modifiers::CTRL, Modifiers::COMMAND] {
        harness.state_mut().state.filter = "capture".to_owned();
        press(&mut harness, Key::Escape, modifiers);
        assert_eq!(harness.state().state.filter, "capture");
    }
}

#[test]
fn a_discarded_layout_does_not_repeat_scene_or_gallery_key_actions() {
    let mut harness = shell();
    click(&mut harness, "Capture keys");
    harness.ctx.data_mut(|data| {
        data.get_temp_mut_or_default::<Observed>(Id::new("key-test"))
            .discard = true
    });
    press(&mut harness, Key::Tab, Modifiers::NONE);
    assert_eq!(observed(&harness.ctx).keys, [(Key::Tab, false)]);
    assert_eq!(selected(&harness), "keys::capture");
    press(&mut harness, Key::Escape, Modifiers::NONE);
    harness.ctx.data_mut(|data| {
        data.get_temp_mut_or_default::<Observed>(Id::new("key-test"))
            .discard = true
    });
    press(&mut harness, Key::Tab, Modifiers::NONE);
    assert_eq!(selected(&harness), "keys::other");
}

#[test]
fn shipped_demo_logs_each_received_press_in_the_shell_actions_panel() {
    let demo = crate::Linked
        .manifest()
        .scenes
        .into_iter()
        .find(|scene| scene.module_path.ends_with("scaffold_scenes::keyboard"))
        .expect("the generated demo is compiled into the tests");
    let mut harness = shell();
    harness.state_mut().source.scenes[0].render = demo.render;
    harness.state_mut().state.show_actions = true;
    harness.run();
    click(&mut harness, "Key capture");
    for (key, modifiers, label) in [
        (Key::Tab, Modifiers::NONE, "Key pressed: Tab"),
        (Key::Tab, Modifiers::SHIFT, "Key pressed: Shift+Tab"),
        (Key::A, Modifiers::NONE, "Key pressed: A"),
        (Key::ArrowLeft, Modifiers::NONE, "Key pressed: Left"),
        (Key::Enter, Modifiers::NONE, "Key pressed: Enter"),
        (Key::Escape, Modifiers::NONE, "Key pressed: Escape"),
    ] {
        press(&mut harness, key, modifiers);
        assert_eq!(selected(&harness), "keys::capture");
        harness.get_by_label(label);
        harness.get_by_label(label.replace("Key pressed:", "Last key:").as_str());
    }
    harness.get_by_label("Tab: 1   Shift+Tab: 1   Escape: 1");
    assert_eq!(harness.ctx.memory(|memory| memory.focused()), None);
    press(&mut harness, Key::A, Modifiers::NONE);
    assert_eq!(harness.get_all_by_label("Key pressed: A").count(), 1);
}
