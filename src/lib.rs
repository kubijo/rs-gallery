//! `gallery` — an egui-shelled component catalog with Storybook-style scene discovery.
//!
//! Scenes are authored next to their components with [`macro@scene`] and discovered through
//! `inventory` — no central list. Each file declares its place in the tree with [`scene_meta`]
//! (`title: "A / B"`, slashes nest); its scenes are children under it. The egui shell (tree sidebar +
//! preview) is fixed; a consumer configures where scenes live (globs, see `gallery-build`).
//!
//! [`launch!`] compiles those scenes to a dylib and loads it through [`HotDylib`] — one path, whether
//! or not `--hot` adds the watcher that swaps it live. Building the shell into a binary that already
//! links its scenes is the other way in: pass [`Linked`] to [`run`] from your own `main`.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use convert_case::{Case, Casing};

/// Re-exported so a host writes `gallery::eframe::Result` without depending on eframe itself — and so
/// both sides are the same eframe. Bumping it is a breaking change here.
pub use eframe;
pub use gallery_macros::scene;
/// Macro plumbing: `#[scene]` expands to `::gallery::inventory::submit!`. Not meant to be named.
#[doc(hidden)]
pub use inventory;

mod context;
mod fonts;
mod hot;
mod knobs;
mod launcher;
mod offscreen;
mod perf;
mod svg;
mod tree;
mod update;
pub use context::SceneCtx;
pub use hot::HotDylib;
pub use knobs::{ChoiceStyle, Knob, Pad2DSpec};
use knobs::{KnobStore, render_knobs};
pub use launcher::launch;
pub use offscreen::Offscreen;
use offscreen::{GlDeps, TargetStore};
use perf::{PERF_WINDOW_SIZE, PerfStats, perf_window_pos, render_performance};
use svg::Icons;
use tree::{TreeNode, breadcrumb, build_tree, fuzzy, node_matches, scene_key, visible_scenes};

/// Common imports for scene files: `use gallery::prelude::*;` then bare `scene_meta!` / `#[scene]`.
pub mod prelude {
    pub use crate::{Offscreen, Pad2DSpec, SceneCtx, SceneEntry, scene, scene_meta};
}

/// A discoverable component state, authored with [`macro@scene`] and joined to its group by
/// `module_path`. `default` marks the group's default scene.
#[derive(Clone, Copy)]
pub struct SceneEntry {
    pub render: fn(&mut SceneCtx<'_>),
    pub name: &'static str,
    pub module_path: &'static str,
    pub default: bool,
    /// Sort position within its group; unset (`u32::MAX`) sorts last, by name.
    pub order: u32,
    /// The scene function's source, for the Source tab (captured by [`macro@scene`]).
    pub source: &'static str,
}

inventory::collect!(SceneEntry);

/// A scene file's group metadata: its `title` (slash-separated tree path), declared with [`scene_meta`].
#[derive(Clone, Copy)]
pub struct SceneGroupMeta {
    pub module_path: &'static str,
    pub title: &'static str,
}

inventory::collect!(SceneGroupMeta);

/// Declare a scene file's group title — its place in the sidebar tree. Once per file.
///
/// ```ignore
/// scene_meta! { title: "Components / Greeting" }
/// ```
#[macro_export]
macro_rules! scene_meta {
    (title: $title:expr) => {
        // A second use in the same module is a compile error (duplicate type).
        enum _SceneGroupDeclaredOnce {}

        $crate::inventory::submit! {
            $crate::SceneGroupMeta {
                module_path: ::core::module_path!(),
                title: $title,
            }
        }
    };
}

/// A source's current scenes and their group metadata.
pub struct Manifest {
    pub scenes: Vec<SceneEntry>,
    pub groups: Vec<SceneGroupMeta>,
}

/// Where the shell's scenes come from. [`SceneSource::before_frame`] lets a source poll for a reload.
pub trait SceneSource {
    /// The scenes and group metadata to show this frame.
    fn manifest(&mut self) -> Manifest;

    /// Per-frame hook before the shell draws (default: nothing).
    fn before_frame(&mut self, _ctx: &egui::Context) {}
}

/// Scenes compiled into this binary, read from the `inventory` registry — for a host that links its
/// own scenes and drives [`run`] directly. [`launch!`] takes the dylib path instead.
pub struct Linked;

impl SceneSource for Linked {
    fn manifest(&mut self) -> Manifest {
        Manifest {
            scenes: inventory::iter::<SceneEntry>().copied().collect(),
            groups: inventory::iter::<SceneGroupMeta>().copied().collect(),
        }
    }
}

/// The selected scene (by stable key), the sidebar filter, the Preview/Source and debug-overlay
/// toggles, the scenes/controls/performance panel toggles, and each scene's persisted knob values.
#[derive(Default)]
pub(crate) struct ShellState {
    pub(crate) selected: Option<String>,
    pub(crate) filter: String,
    show_source: bool,
    debug: bool,
    show_perf: bool,
    show_scenes: bool,
    show_controls: bool,
    knobs: KnobStore,
    targets: TargetStore,
}

/// The eframe backend the shell runs on — the host's explicit, required choice ([`Settings::renderer`]).
/// There's no default: a scene that renders through a raw GL context needs to know `gl` is present.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Renderer {
    /// egui's wgpu backend. Pure-egui scenes; [`SceneCtx::gl_loader`] is `None`.
    Wgpu,
    /// egui's glow (OpenGL) backend. [`SceneCtx::gl_loader`] is `Some`, so a scene can render non-egui
    /// content — femtovg into an offscreen FBO — and show it via [`SceneCtx::register_native_texture`].
    Glow,
}

/// A GL function-pointer loader (eframe's `get_proc_address`). Version-agnostic: hand it to femtovg's
/// `OpenGl::new_from_function_cstr` or `glow::Context::from_loader_function_cstr`, at whatever
/// glow/femtovg version the scene likes — gallery pins none. `Some` only under [`Renderer::Glow`]; see
/// [`SceneCtx::gl_loader`].
pub type GlLoader = Arc<dyn Fn(&std::ffi::CStr) -> *const std::ffi::c_void + Send + Sync>;

/// Launch settings the host supplies. `renderer` is required (no default); the rest default via
/// [`Settings::new`].
#[derive(Debug, Clone)]
pub struct Settings {
    /// Which eframe backend to run on.
    pub renderer: Renderer,
    /// Initial Controls-panel width; egui's default when `None`. A hand resize persists over it.
    pub controls_default_width: Option<f32>,
}

impl Settings {
    /// Settings for `renderer`, with defaults for everything else.
    #[must_use]
    pub fn new(renderer: Renderer) -> Self {
        Self {
            renderer,
            controls_default_width: None,
        }
    }

    /// Seed the Controls panel's initial width (a hand resize still persists over it).
    #[must_use]
    pub fn controls_default_width(mut self, width: f32) -> Self {
        self.controls_default_width = Some(width);
        self
    }
}

/// The fixed egui shell over a [`SceneSource`].
pub struct Gallery<S: SceneSource> {
    source: S,
    state: ShellState,
    settings: Settings,
    icons: Icons,
    perf: Arc<Mutex<PerfStats>>,
    /// Set by the perf window when its close button is hit; the shell clears `show_perf` next frame.
    perf_close: Arc<AtomicBool>,
    /// Frozen at open: recomputing it each frame would yank the window back on every drag.
    perf_pos: Option<egui::Pos2>,
    frames_left: Option<u32>,
    /// A `--scene` request, resolved on the first frame once the manifest exists. Matched on a
    /// fragment, since the real keys are `module_path::name` and nobody wants to type those.
    scene_request: Option<String>,
    /// The GL proc-address loader, `Some` under [`Renderer::Glow`] — handed to scenes as
    /// [`SceneCtx::gl_loader`].
    gl_loader: Option<GlLoader>,
    /// gallery's own glow context, `Some` under [`Renderer::Glow`] — used for [`SceneCtx::offscreen`]'s
    /// FBO bookkeeping (internal; never in the scene API).
    gl: Option<Arc<eframe::glow::Context>>,
}

impl<S: SceneSource> Gallery<S> {
    #[must_use]
    pub fn new(
        source: S,
        settings: Settings,
        gl_loader: Option<GlLoader>,
        gl: Option<Arc<eframe::glow::Context>>,
    ) -> Self {
        Self {
            source,
            state: ShellState {
                show_scenes: true,
                show_controls: true,
                ..ShellState::default()
            },
            settings,
            icons: Icons::load(),
            perf: Arc::new(Mutex::new(PerfStats::new())),
            perf_close: Arc::new(AtomicBool::new(false)),
            perf_pos: None,
            frames_left: None,
            scene_request: None,
            gl_loader,
            gl,
        }
    }
}

impl<S: SceneSource> eframe::App for Gallery<S> {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let frame_start = Instant::now();
        let gl_loader = self.gl_loader.clone();
        let gl = self.gl.clone();
        self.source.before_frame(ui.ctx());
        // egui declares `Style::debug` under `#[cfg(debug_assertions)]`, so the overlay this drives
        // does not exist in a release build — mirror its gate rather than fail to compile there.
        #[cfg(debug_assertions)]
        ui.ctx()
            .all_styles_mut(|style| style.debug.show_interactive_widgets = self.state.debug);
        if self.perf_close.swap(false, Ordering::Relaxed) {
            self.state.show_perf = false;
        }
        let manifest = self.source.manifest();
        if let Some(request) = self.scene_request.take() {
            let needle = request.to_lowercase();
            self.state.selected = manifest
                .scenes
                .iter()
                .map(scene_key)
                .find(|key| key.to_lowercase().contains(&needle));
        }
        let tree = build_tree(&manifest);

        // Keep the selected scene if it still exists (across reloads/reordering); else the first.
        let still_here = self
            .state
            .selected
            .as_deref()
            .is_some_and(|key| manifest.scenes.iter().any(|scene| scene_key(scene) == key));
        if !still_here {
            self.state.selected = manifest.scenes.first().map(scene_key);
        }

        handle_keyboard(ui.ctx(), &mut self.state, &tree, &manifest.scenes);

        // Its own viewport, on its own repaint clock: watching the numbers never drives this loop, and
        // the meter's own draw lands in its budget rather than the frame it measures.
        if self.state.show_perf {
            if self.perf_pos.is_none() {
                self.perf_pos = perf_window_pos(ui.ctx());
            }
            let mut builder = egui::ViewportBuilder::default()
                .with_title("gallery · perf")
                .with_inner_size(PERF_WINDOW_SIZE);
            if let Some(pos) = self.perf_pos {
                builder = builder.with_position(pos);
            }
            let perf = self.perf.clone();
            let close = self.perf_close.clone();
            ui.ctx().show_viewport_deferred(
                egui::ViewportId::from_hash_of("gallery-perf"),
                builder,
                move |ctx, _class| {
                    egui::CentralPanel::default()
                        .frame(egui::Frame::NONE.fill(PANEL_BG))
                        .show(ctx, |ui| {
                            render_performance(ui, &perf.lock().expect("perf stats"));
                        });
                    if ctx.input(|i| i.viewport().close_requested()) {
                        close.store(true, Ordering::Relaxed);
                        ctx.request_repaint_of(egui::ViewportId::ROOT);
                    }
                    // Fast enough to read as live, far below the render loop it reports on.
                    ctx.request_repaint_after(Duration::from_millis(100));
                },
            );
        } else {
            // Re-park on the next open, in case the shell has moved since.
            self.perf_pos = None;
        }

        let icons = &self.icons;
        if self.state.show_scenes {
            egui::Panel::left("gallery-scenes")
                .frame(egui::Frame::NONE.fill(PANEL_BG))
                .show(ui, |ui| {
                    {
                        let mut header = header_bar(ui);
                        header.label(header_title("Scenes"));
                        // Collapse caret hugs the panel's canvas-facing (right) edge.
                        header.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                if caret(ui, Caret::Left)
                                    .on_hover_text("Collapse scenes (Cmd+Shift+L)")
                                    .clicked()
                                {
                                    self.state.show_scenes = false;
                                }
                            },
                        );
                    }
                    egui::Frame::NONE
                        .inner_margin(egui::Margin::same(8))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                icons.search.show(ui, ICON_SIZE, egui::Color32::GRAY);
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.state.filter)
                                        .id(filter_id())
                                        .hint_text("filter")
                                        .desired_width(f32::INFINITY),
                                );
                            });
                            ui.separator();
                            let filter = self.state.filter.clone();
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                render_node(
                                    ui,
                                    &tree,
                                    &manifest.scenes,
                                    &mut self.state.selected,
                                    icons,
                                    &filter,
                                    false,
                                );
                            });
                        });
                });
        } else {
            collapsed_panel(
                ui,
                "gallery-scenes-rail",
                true,
                "Show scenes (Cmd+Shift+L)",
                &mut self.state.show_scenes,
            );
        }

        // The selected scene (after this frame's clicks) + its key, driving the preview and controls.
        let key = self.state.selected.clone();
        let scene = key
            .as_deref()
            .and_then(|key| manifest.scenes.iter().find(|scene| scene_key(scene) == key));

        if self.state.show_controls {
            let mut controls =
                egui::Panel::right("gallery-controls").frame(egui::Frame::NONE.fill(PANEL_BG));
            if let Some(width) = self.settings.controls_default_width {
                controls = controls.default_size(width);
            }
            controls.show(ui, |ui| {
                {
                    let mut header = header_bar(ui);
                    // Collapse caret hugs the panel's canvas-facing (left) edge.
                    if caret(&mut header, Caret::Right)
                        .on_hover_text("Collapse controls (Cmd+Shift+R)")
                        .clicked()
                    {
                        self.state.show_controls = false;
                    }
                    header.add_space(2.0);
                    header.label(header_title("Controls"));
                }
                egui::Frame::NONE
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            // Filled when the scene renders (below): knobs appear one frame after a scene is opened.
                            match key.as_deref().and_then(|key| self.state.knobs.get_mut(key)) {
                                Some(knobs) => {
                                    render_knobs(ui, knobs);
                                }
                                None => {
                                    ui.weak("This scene has no controls.");
                                }
                            }
                        });
                    });
            });
        } else {
            collapsed_panel(
                ui,
                "gallery-controls-rail",
                false,
                "Show controls (Cmd+Shift+R)",
                &mut self.state.show_controls,
            );
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(PANEL_BG))
            .show(ui, |ui| {
                if let Some(scene) = scene {
                    // The same header bar as the side panels, so all three line up in height and style.
                    let mut header = header_bar(ui);
                    header.label(header_title(&breadcrumb(scene, &manifest.groups)));
                    // Tight padding keeps the cluster within the slim header bar.
                    header.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().button_padding = egui::vec2(4.0, 1.0);
                        ui.selectable_value(&mut self.state.show_source, true, "Source");
                        ui.selectable_value(&mut self.state.show_source, false, "Preview");
                        #[cfg(debug_assertions)]
                        ui.checkbox(&mut self.state.debug, "Debug");
                        #[cfg(not(debug_assertions))]
                        ui.add_enabled(false, egui::Checkbox::new(&mut self.state.debug, "Debug"))
                            .on_disabled_hover_text("egui's debug overlay is a dev-build feature");
                        ui.checkbox(&mut self.state.show_perf, "Perf")
                            .on_hover_text("Performance window (⌘B)");
                    });
                }

                if self.state.show_source {
                    if let Some(scene) = scene {
                        render_source_view(ui, scene.source);
                    }
                } else {
                    // A checkerboard canvas, so a component's transparency and bounds read against the
                    // shell — over the area below the header only, or it paints over the breadcrumb and
                    // view tabs above.
                    paint_checkerboard(ui, ui.available_rect_before_wrap());
                    if let (Some(scene), Some(key)) = (scene, &key) {
                        let store = self.state.knobs.entry(key.clone()).or_default();
                        let target = self.state.targets.entry(key.clone()).or_default();
                        let gl_deps = match (gl_loader.clone(), gl.as_deref()) {
                            (Some(loader), Some(gl)) => Some(GlDeps {
                                loader,
                                gl,
                                frame,
                                target,
                            }),
                            _ => None,
                        };
                        egui::ScrollArea::both().show(ui, |ui| {
                            // Pad the scene off the canvas edges.
                            let declared = egui::Frame::new()
                                .inner_margin(egui::Margin::same(16))
                                .show(ui, |ui| {
                                    let mut ctx = SceneCtx::new(ui, store, gl_deps);
                                    (scene.render)(&mut ctx);
                                    ctx.declared()
                                })
                                .inner;
                            // Drop knobs the scene stopped declaring this frame.
                            store.truncate(declared);
                        });
                    }
                }
            });

        // Timed here, not read from `frame.info().cpu_usage`: eframe reports that
        // per *viewport* redraw, so the perf window's own repaints overwrite it
        // and the meter ends up charging the shell for the instrument.
        // This is the shell's build cost; tessellate and paint sit outside it.
        self.perf
            .lock()
            .expect("perf stats")
            .record(frame_start.elapsed().as_secs_f32());

        if let Some(left) = self.frames_left.as_mut() {
            *left = left.saturating_sub(1);
            if *left == 0 {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                ui.ctx().request_repaint();
            }
        }
    }
}

/// Gold folders, blue scene markers.
const FOLDER_TINT: egui::Color32 = egui::Color32::from_rgb(0xC8, 0x9B, 0x3C);
const SCENE_TINT: egui::Color32 = egui::Color32::from_rgb(0x6C, 0x9C, 0xD8);
const ICON_SIZE: f32 = 12.0;

/// Render a tree node, honouring the filter: a group stays if its name or a descendant matches, and a
/// matched ancestor shows all its descendants. Single-scene default groups render as flat leaves;
/// others as collapsible headers (folder icon over the triangle, auto-expanded while filtering).
fn render_node(
    ui: &mut egui::Ui,
    node: &TreeNode,
    scenes: &[SceneEntry],
    selected: &mut Option<String>,
    icons: &Icons,
    filter: &str,
    ancestor_matched: bool,
) {
    let filtering = !filter.is_empty();
    for (name, child) in &node.children {
        let name_matches = filtering && fuzzy(name, filter);
        if filtering
            && !ancestor_matched
            && !name_matches
            && !node_matches(name, child, scenes, filter)
        {
            continue;
        }
        let descend = ancestor_matched || name_matches;
        let default_leaf =
            child.children.is_empty() && child.scenes.len() == 1 && scenes[child.scenes[0]].default;
        if default_leaf {
            leaf(ui, name, &scenes[child.scenes[0]], selected, icons);
        } else {
            let mut header = egui::CollapsingHeader::new(name);
            header = if filtering {
                header.open(Some(true))
            } else {
                header.default_open(true)
            };
            let resp = header.show(ui, |ui| {
                render_node(ui, child, scenes, selected, icons, filter, descend);
            });
            let hr = resp.header_response.rect;
            let rect = egui::Rect::from_center_size(
                egui::pos2(hr.left() + ICON_SIZE / 2.0, hr.center().y),
                egui::Vec2::splat(ICON_SIZE),
            );
            icons.folder.paint(ui.painter(), rect, FOLDER_TINT);
        }
    }
    for &i in &node.scenes {
        if filtering && !ancestor_matched && !fuzzy(scenes[i].name, filter) {
            continue;
        }
        // Start-cased for the menu; `from_case(Lower)` splits on spaces only, so a name like "pad2d"
        // stays one word (default boundaries would render "Pad 2 D").
        let label = scenes[i].name.from_case(Case::Lower).to_case(Case::Title);
        leaf(ui, &label, &scenes[i], selected, icons);
    }
}

/// The egui id of the sidebar filter box, so Cmd+F can focus it.
fn filter_id() -> egui::Id {
    egui::Id::new("gallery-filter")
}

/// Keyboard: Tab / Shift+Tab cycle scenes (filtered order), Escape clears the filter,
/// Cmd+F focuses it. Cmd+B, Cmd+Shift+L, and Cmd+Shift+R collapse/expand the performance
/// footer, the scenes sidebar, and the controls panel.
fn handle_keyboard(
    ctx: &egui::Context,
    state: &mut ShellState,
    tree: &TreeNode,
    scenes: &[SceneEntry],
) {
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
        state.filter.clear();
    }
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::F)) {
        ctx.memory_mut(|m| m.request_focus(filter_id()));
    }
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::B)) {
        state.show_perf = !state.show_perf;
    }
    let cmd_shift = egui::Modifiers::COMMAND | egui::Modifiers::SHIFT;
    if ctx.input_mut(|i| i.consume_key(cmd_shift, egui::Key::L)) {
        state.show_scenes = !state.show_scenes;
    }
    if ctx.input_mut(|i| i.consume_key(cmd_shift, egui::Key::R)) {
        state.show_controls = !state.show_controls;
    }
    let next = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Tab));
    let prev = ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab));
    if !(next || prev) {
        return;
    }
    let mut order = Vec::new();
    visible_scenes(tree, scenes, &state.filter, false, &mut order);
    let keys: Vec<String> = order.iter().map(|&i| scene_key(&scenes[i])).collect();
    if keys.is_empty() {
        return;
    }
    let current = state
        .selected
        .as_deref()
        .and_then(|key| keys.iter().position(|k| k == key));
    let idx = match current {
        Some(pos) if next => (pos + 1) % keys.len(),
        Some(pos) => (pos + keys.len() - 1) % keys.len(),
        None => 0,
    };
    state.selected = Some(keys[idx].clone());
}

/// A selectable scene leaf with its component icon. `label` is the display text — the group title for
/// a file's default scene, the start-cased scene name for additional (named) scenes.
fn leaf(
    ui: &mut egui::Ui,
    label: &str,
    scene: &SceneEntry,
    selected: &mut Option<String>,
    icons: &Icons,
) {
    let key = scene_key(scene);
    let is_selected = selected.as_deref() == Some(key.as_str());
    let clicked = ui
        .horizontal(|ui| {
            // Snug the icon ↔ label gap: trim the item spacing and the label's own left inset. Leave
            // `button_padding.y` alone so the row height (vertical spacing) is unchanged.
            let spacing = ui.spacing_mut();
            spacing.item_spacing.x = 4.0;
            spacing.button_padding.x = 2.0;
            icons.app.show(ui, ICON_SIZE, SCENE_TINT);
            ui.selectable_label(is_selected, label)
        })
        .inner
        .clicked();
    if clicked {
        *selected = Some(key);
    }
}

/// Render a scene's captured source with Rust syntax highlighting (egui_extras' built-in highlighter).
fn render_source_view(ui: &mut egui::Ui, source: &str) {
    let theme = egui_extras::syntax_highlighting::CodeTheme::dark(12.0);
    let job =
        egui_extras::syntax_highlighting::highlight(ui.ctx(), ui.style(), &theme, source, "rs");
    egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(16, 12))
        .show(ui, |ui| {
            egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
                ui.add(
                    egui::Label::new(job)
                        .wrap_mode(egui::TextWrapMode::Extend)
                        .selectable(true),
                );
            });
        });
}

/// Paint a transparency checkerboard across `rect` (the preview canvas background).
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "tile counts are small, non-negative screen dimensions"
)]
fn paint_checkerboard(ui: &egui::Ui, rect: egui::Rect) {
    const SIZE: f32 = 12.0;
    const DARK: egui::Color32 = egui::Color32::from_rgb(0x25, 0x25, 0x25);
    const LIGHT: egui::Color32 = egui::Color32::from_rgb(0x35, 0x35, 0x35);

    let painter = ui.painter_at(rect);
    let cols = (rect.width() / SIZE + 1.0) as usize;
    let rows = (rect.height() / SIZE + 1.0) as usize;
    for row in 0..rows {
        for col in 0..cols {
            let corner = egui::pos2(
                rect.min.x + col as f32 * SIZE,
                rect.min.y + row as f32 * SIZE,
            );
            let tile = egui::Rect::from_min_size(corner, egui::Vec2::splat(SIZE));
            let color = if (row + col) % 2 == 0 { DARK } else { LIGHT };
            painter.rect_filled(tile, 0.0, color);
        }
    }
}

// --- Panel chrome ---

/// Shared panel chrome: header-bar height, near-black panel fill,
/// a lighter header bar, and a hairline border.
const HEADER_H: f32 = 20.0;
/// A collapsed side panel shrinks to this width — a rail just wide enough for the expand caret.
const RAIL_W: f32 = 30.0;
const PANEL_BG: egui::Color32 = egui::Color32::from_rgb(0x1A, 0x1A, 0x1A);
const HEADER_BG: egui::Color32 = egui::Color32::from_rgb(0x26, 0x26, 0x26);
pub(crate) const HAIRLINE: egui::Color32 = egui::Color32::from_rgb(0x39, 0x39, 0x39);
/// Dimmed foreground, for what should recede until looked at.
pub(crate) const MUTED: egui::Color32 = egui::Color32::from_rgb(0x6F, 0x6F, 0x6F);

/// Paint a panel's grey title bar across the top of `ui` (full width, hairline underline),
/// advance the cursor past it, and return a child `Ui` centred in the bar for the title and controls.
pub(crate) fn header_bar(ui: &mut egui::Ui) -> egui::Ui {
    let area = ui.max_rect();
    let bar = egui::Rect::from_min_size(area.min, egui::vec2(area.width(), HEADER_H));
    ui.painter().rect_filled(bar, 0.0, HEADER_BG);
    ui.painter().hline(
        area.x_range(),
        bar.bottom(),
        egui::Stroke::new(1.0, HAIRLINE),
    );
    ui.advance_cursor_after_rect(bar);
    ui.new_child(
        egui::UiBuilder::new()
            .max_rect(bar.shrink2(egui::vec2(8.0, 0.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    )
}

/// A panel title styled for [`header_bar`].
pub(crate) fn header_title(text: &str) -> egui::RichText {
    egui::RichText::new(text)
        .color(egui::Color32::WHITE)
        .size(11.0)
}

/// A collapsed side panel: a thin full-height rail whose header cap holds only the expand
/// caret, pointing back toward the canvas. Clicking it flips `open` on. `on_left` picks the
/// side (and thus which edge the caret hugs); the caller keeps its own always-shown counterpart.
///
/// `id` must differ from the expanded panel's: egui persists panel size per id, so a shared id
/// would let this rail's pinned `RAIL_W` overwrite the expanded panel's remembered width.
fn collapsed_panel(
    ui: &mut egui::Ui,
    id: &'static str,
    on_left: bool,
    tooltip: &str,
    open: &mut bool,
) {
    let panel = if on_left {
        egui::Panel::left(id)
    } else {
        egui::Panel::right(id)
    };
    let (dir, layout) = if on_left {
        (
            Caret::Right,
            egui::Layout::right_to_left(egui::Align::Center),
        )
    } else {
        (
            Caret::Left,
            egui::Layout::left_to_right(egui::Align::Center),
        )
    };
    panel
        .resizable(false)
        .exact_size(RAIL_W)
        .frame(egui::Frame::NONE.fill(PANEL_BG))
        .show(ui, |ui| {
            header_bar(ui).with_layout(layout, |ui| {
                if caret(ui, dir).on_hover_text(tooltip).clicked() {
                    *open = true;
                }
            });
        });
}

/// Which way a collapse [`caret`] points — toward where a click sends the panel.
#[derive(Clone, Copy)]
enum Caret {
    Left,
    Right,
}

/// A small collapse/expand caret pointing in `dir`, muted until hovered.
/// Returns its click response so the caller owns the toggle.
fn caret(ui: &mut egui::Ui, dir: Caret) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::click());
    let c = rect.center();
    let pts = match dir {
        Caret::Right => vec![
            egui::pos2(c.x - 2.0, c.y - 3.0),
            egui::pos2(c.x - 2.0, c.y + 3.0),
            egui::pos2(c.x + 3.0, c.y),
        ],
        Caret::Left => vec![
            egui::pos2(c.x + 2.0, c.y - 3.0),
            egui::pos2(c.x + 2.0, c.y + 3.0),
            egui::pos2(c.x - 3.0, c.y),
        ],
    };
    let color = if resp.hovered() {
        egui::Color32::WHITE
    } else {
        MUTED
    };
    ui.painter()
        .add(egui::Shape::convex_polygon(pts, color, egui::Stroke::NONE));
    resp
}

/// Apply the gallery's style tweaks onto `style`, in place: square (un-rounded) widgets and roomier
/// button padding. It never touches colours, so each theme keeps its own palette.
///
/// Applied to every theme before the host's `setup` runs; [`run`] documents the extend / replace /
/// drop levels that ordering buys.
pub fn apply_default_style(style: &mut egui::Style) {
    style.spacing.button_padding = egui::vec2(8.0, 4.0);
    for widget in [
        &mut style.visuals.widgets.noninteractive,
        &mut style.visuals.widgets.inactive,
        &mut style.visuals.widgets.hovered,
        &mut style.visuals.widgets.active,
        &mut style.visuals.widgets.open,
    ] {
        widget.corner_radius = egui::CornerRadius::ZERO;
    }
}

/// Run the gallery as a native eframe window over the given scene source. `setup` runs once with the
/// freshly created egui context, after [`apply_default_style`] has run over every theme — so a host
/// can:
///
/// - **keep** the gallery look — touch nothing (just register asset loaders, e.g.
///   `egui_extras::install_image_loaders`);
/// - **extend** it — `ctx.all_styles_mut(|style| ...)`, e.g. recolour `visuals.selection.bg_fill`;
/// - **replace** it — `ctx.all_styles_mut(|style| *style = my_style)`;
/// - **drop** it — `ctx.all_styles_mut(|style| *style = egui::Style::default())` for plain egui.
pub fn run<S: SceneSource + 'static>(
    title: &str,
    source: S,
    settings: Settings,
    setup: impl FnOnce(&egui::Context) + 'static,
) -> eframe::Result {
    run_with(title, source, settings, setup, RunOptions::default())
}

/// Overrides for a scripted run; an ordinary session sets none. `frames` renders exactly that many
/// and exits, which is what makes two profiles comparable; `scene` picks the one to measure.
#[derive(Default)]
pub(crate) struct RunOptions {
    pub(crate) frames: Option<u32>,
    pub(crate) scene: Option<String>,
}

pub(crate) fn run_with<S: SceneSource + 'static>(
    title: &str,
    source: S,
    settings: Settings,
    setup: impl FnOnce(&egui::Context) + 'static,
    options: RunOptions,
) -> eframe::Result {
    let renderer = match settings.renderer {
        Renderer::Wgpu => eframe::Renderer::Wgpu,
        Renderer::Glow => eframe::Renderer::Glow,
    };
    eframe::run_native(
        title,
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default().with_inner_size([1280.0, 720.0]),
            renderer,
            ..Default::default()
        },
        Box::new(|cc| {
            // Bundled Noto fallbacks first (so glyphs the default faces lack resolve), then gallery
            // style, then `setup` — which can layer over, replace, or drop either (levels in the fn docs).
            fonts::install(&cc.egui_ctx);
            cc.egui_ctx.all_styles_mut(apply_default_style);
            setup(&cc.egui_ctx);
            // Surface format for wgpu paint-callback scenes — egui-wgpu won't hand it to them.
            if let Some(rs) = cc.wgpu_render_state.as_ref() {
                rs.renderer
                    .write()
                    .callback_resources
                    .insert(rs.target_format);
            }
            // `cc.get_proc_address` is `Some` under glow — the version-agnostic GL loader that reaches
            // scenes as `SceneCtx::gl_loader`. `cc.gl` is gallery's own context for `offscreen` FBOs.
            let mut gallery =
                Gallery::new(source, settings, cc.get_proc_address.clone(), cc.gl.clone());
            gallery.frames_left = options.frames;
            gallery.scene_request = options.scene;
            Ok(Box::new(gallery))
        }),
    )
}

/// The scenes dylib's entire `lib.rs`: `gallery::scenes_dylib!();`. Pulls in the discovered
/// `*.scene.rs` (from the `build.rs` discovery) and exports the manifest the loader reads.
#[macro_export]
macro_rules! scenes_dylib {
    () => {
        include!(concat!(env!("OUT_DIR"), "/gallery_scenes.rs"));

        #[unsafe(no_mangle)]
        pub fn __gallery_manifest() -> $crate::Manifest {
            $crate::Manifest {
                scenes: $crate::inventory::iter::<$crate::SceneEntry>()
                    .copied()
                    .collect(),
                groups: $crate::inventory::iter::<$crate::SceneGroupMeta>()
                    .copied()
                    .collect(),
            }
        }
    };
}

/// Scene fixtures shared by the tests here and in [`tree`].
#[cfg(test)]
pub(crate) mod test_support {
    use crate::{SceneCtx, SceneEntry, SceneGroupMeta};

    fn noop(_: &mut SceneCtx) {}

    pub(crate) fn scene(
        name: &'static str,
        module_path: &'static str,
        default: bool,
    ) -> SceneEntry {
        SceneEntry {
            render: noop,
            name,
            module_path,
            default,
            order: u32::MAX,
            source: "",
        }
    }

    pub(crate) fn ordered(name: &'static str, order: u32) -> SceneEntry {
        SceneEntry {
            order,
            ..scene(name, "m", false)
        }
    }

    pub(crate) fn group(module_path: &'static str, title: &'static str) -> SceneGroupMeta {
        SceneGroupMeta { module_path, title }
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable;

    use super::*;
    use crate::test_support::{group, scene};

    /// Renderer independence lets a scene's GL library keep its own glow version;
    /// only the raw proc-address loader crosses to eframe's glow.
    ///
    /// The femtovg demo leans on that — femtovg 0.20.4 links glow 0.16
    /// against eframe's glow 0.17 — and a test-only `glow = "0.16"` dev-dependency
    /// (the version femtovg pulls) drops it into our lockfile.
    ///
    /// Guard that the two stay distinct: were a bump to align them, cargo would
    /// collapse to a single glow and the demo would quietly stop proving anything.
    #[test]
    fn femtovg_demo_pins_a_glow_version_distinct_from_eframe() {
        #[derive(serde::Deserialize)]
        struct Lock {
            package: Vec<Package>,
        }
        #[derive(serde::Deserialize)]
        struct Package {
            name: String,
            version: String,
        }

        let lock: Lock = toml::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/Cargo.lock"
        )))
        .expect("Cargo.lock is valid TOML");
        let glow_versions: std::collections::BTreeSet<&str> = lock
            .package
            .iter()
            .filter(|pkg| pkg.name == "glow")
            .map(|pkg| pkg.version.as_str())
            .collect();
        assert!(
            glow_versions.len() >= 2,
            "femtovg and eframe should resolve distinct glow versions, but the lockfile has only \
             {glow_versions:?} — a bump may have aligned them, so the femtovg demo no longer \
             demonstrates glow-version independence"
        );
    }

    #[test]
    fn apply_default_style_squares_widgets_and_pads_buttons_past_egui_defaults() {
        let mut style = egui::Style::default();
        let egui_default = style.spacing.button_padding;
        apply_default_style(&mut style);
        assert_eq!(
            style.visuals.widgets.inactive.corner_radius,
            egui::CornerRadius::ZERO
        );
        assert_eq!(
            style.visuals.widgets.active.corner_radius,
            egui::CornerRadius::ZERO
        );
        let ours = style.spacing.button_padding;
        assert!(
            ours.x > egui_default.x && ours.y > egui_default.y,
            "button padding {ours:?} should exceed egui's default {egui_default:?}"
        );
    }

    // Structural (egui_kittest + AccessKit): the rendered sidebar, queried by label.

    #[test]
    fn sidebar_labels_a_default_scene_by_its_title_not_its_fn_name() {
        let scenes = vec![scene("view", "m", true)];
        let tree = build_tree(&Manifest {
            scenes: scenes.clone(),
            groups: vec![group("m", "MyCar / Map")],
        });
        let icons = crate::svg::Icons::load();
        let mut selected = None;
        let mut harness = egui_kittest::Harness::new_ui(move |ui| {
            render_node(ui, &tree, &scenes, &mut selected, &icons, "", false);
        });
        harness.run();
        assert!(
            harness.query_by_label("Map").is_some(),
            "the title node labels the entry"
        );
        assert!(
            harness.query_by_label("view").is_none(),
            "the fn name is not shown"
        );
    }

    #[test]
    fn sidebar_shows_additional_named_scenes_under_the_title_folder() {
        // Distinct from the "Map" title node, so the leaf queries below stay unambiguous.
        let scenes = vec![scene("grid", "m", true), scene("aerial", "m", false)];
        let tree = build_tree(&Manifest {
            scenes: scenes.clone(),
            groups: vec![group("m", "MyCar / Map")],
        });
        let icons = crate::svg::Icons::load();
        let mut selected = None;
        let mut harness = egui_kittest::Harness::new_ui(move |ui| {
            render_node(ui, &tree, &scenes, &mut selected, &icons, "", false);
        });
        harness.run();
        assert!(harness.query_by_label("Grid").is_some());
        assert!(harness.query_by_label("Aerial").is_some());
    }
}
