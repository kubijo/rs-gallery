//! `gallery` — an egui-shelled component catalog with Storybook-style scene discovery.
//!
//! Scenes are authored next to their components with [`macro@scene`] and discovered through
//! `inventory` — no central list. Each file declares its place in the tree with [`scene_meta`]
//! (`title: "A / B"`, slashes nest); its scenes are children under it. The egui shell (tree sidebar +
//! preview) is fixed. A consumer configures where scenes live (globs, see `gallery-build`) and how
//! they reach the shell — a [`SceneSource`]: [`Linked`] (compiled in) or [`HotDylib`] (a rebuilt dylib).

use std::{
    collections::BTreeMap,
    fs,
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use camino::{Utf8Path, Utf8PathBuf};
use convert_case::{Case, Casing};
use process_wrap::std::{ChildWrapper, CommandWrap};

pub use eframe;
pub use gallery_macros::scene;
pub use inventory;

mod context;
mod fonts;
mod knobs;
mod offscreen;
mod svg;
pub use context::SceneCtx;
pub use knobs::{ChoiceStyle, Knob, Pad2DSpec};
use knobs::{KnobStore, render_knobs};
pub use offscreen::Offscreen;
use offscreen::{GlDeps, TargetStore};
use svg::Icons;

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

/// Scenes compiled into this binary, read from the `inventory` registry.
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
pub struct ShellState {
    pub selected: Option<String>,
    pub filter: String,
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
        // The `Debug` toggle drives egui's own overlay on every interactive widget.
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
                        ui.checkbox(&mut self.state.debug, "Debug");
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

/// A node in the sidebar tree: child groups plus the scenes placed directly here.
#[derive(Default)]
struct TreeNode {
    children: BTreeMap<String, TreeNode>,
    scenes: Vec<usize>,
}

/// Build the tree: group titles form the skeleton, then each scene lands under its group (longest
/// `module_path` prefix), or at the root if it declared no group.
fn build_tree(manifest: &Manifest) -> TreeNode {
    let mut tree = TreeNode::default();
    for meta in &manifest.groups {
        node_at(&mut tree, meta.title);
    }
    for (i, scene) in manifest.scenes.iter().enumerate() {
        match longest_group(&manifest.groups, scene.module_path) {
            Some(meta) => node_at(&mut tree, meta.title).scenes.push(i),
            None => tree.scenes.push(i),
        }
    }
    sort_scenes(&mut tree, &manifest.scenes);
    tree
}

/// Sort each node's scenes by `(order, name)` so the catalog is deterministic;
/// inventory registration order is otherwise arbitrary link order.
fn sort_scenes(node: &mut TreeNode, scenes: &[SceneEntry]) {
    node.scenes.sort_by(|&a, &b| {
        (scenes[a].order, scenes[a].name).cmp(&(scenes[b].order, scenes[b].name))
    });
    for child in node.children.values_mut() {
        sort_scenes(child, scenes);
    }
}

/// Walk (creating) the tree to the node named by a slash-separated title.
fn node_at<'a>(tree: &'a mut TreeNode, title: &str) -> &'a mut TreeNode {
    let mut node = tree;
    for part in title.split('/').map(str::trim) {
        node = node.children.entry(part.to_owned()).or_default();
    }
    node
}

/// The group whose `module_path` is the longest prefix of `module_path` (the scene's home group).
fn longest_group<'a>(
    groups: &'a [SceneGroupMeta],
    module_path: &str,
) -> Option<&'a SceneGroupMeta> {
    groups
        .iter()
        .filter(|meta| module_path.starts_with(meta.module_path))
        .max_by_key(|meta| meta.module_path.len())
}

/// Gold folders, blue scene markers.
const FOLDER_TINT: egui::Color32 = egui::Color32::from_rgb(0xC8, 0x9B, 0x3C);
const SCENE_TINT: egui::Color32 = egui::Color32::from_rgb(0x6C, 0x9C, 0xD8);
const ICON_SIZE: f32 = 12.0;

/// Sublime-style fuzzy match for the sidebar filter.
fn fuzzy(text: &str, filter: &str) -> bool {
    sublime_fuzzy::best_match(filter, text).is_some()
}

/// Whether a node's name or anything in its subtree matches the filter.
fn node_matches(name: &str, node: &TreeNode, scenes: &[SceneEntry], filter: &str) -> bool {
    fuzzy(name, filter)
        || node.scenes.iter().any(|&i| fuzzy(scenes[i].name, filter))
        || node
            .children
            .iter()
            .any(|(child, node)| node_matches(child, node, scenes, filter))
}

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

/// The visible scenes in render order (for keyboard next/prev), honouring the filter.
fn visible_scenes(
    node: &TreeNode,
    scenes: &[SceneEntry],
    filter: &str,
    ancestor_matched: bool,
    out: &mut Vec<usize>,
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
        visible_scenes(child, scenes, filter, ancestor_matched || name_matches, out);
    }
    for &i in &node.scenes {
        if filtering && !ancestor_matched && !fuzzy(scenes[i].name, filter) {
            continue;
        }
        out.push(i);
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

/// The preview heading. A file's default scene IS its `scene_meta` title node, so it shows just the
/// title (e.g. "Components / Greeting"); an additional named scene hangs under it ("… / world").
fn breadcrumb(scene: &SceneEntry, groups: &[SceneGroupMeta]) -> String {
    match longest_group(groups, scene.module_path) {
        Some(group) if scene.default => group.title.to_owned(),
        Some(group) => format!("{} / {}", group.title, scene.name),
        None => scene.name.to_owned(),
    }
}

/// A scene's stable identity for keying selection and persisted knobs — survives reloads and reordering.
fn scene_key(scene: &SceneEntry) -> String {
    format!("{}::{}", scene.module_path, scene.name)
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
const HAIRLINE: egui::Color32 = egui::Color32::from_rgb(0x39, 0x39, 0x39);

/// Paint a panel's grey title bar across the top of `ui` (full width, hairline underline),
/// advance the cursor past it, and return a child `Ui` centred in the bar for the title and controls.
fn header_bar(ui: &mut egui::Ui) -> egui::Ui {
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
fn header_title(text: &str) -> egui::RichText {
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

// --- Performance footer ---

const PERF_LABEL: egui::Color32 = egui::Color32::from_rgb(0x6F, 0x6F, 0x6F);
const TABLE_ROW_H: f32 = 18.0;
const TABLE_LABEL_W: f32 = 96.0;
const PERF_WINDOW_SIZE: [f32; 2] = [380.0, 240.0];

/// Park the perf window beside the shell rather than over it, flipping left when the monitor has no
/// room. `None` when the shell's geometry is unknown — Wayland never reports it — leaving it to the WM.
fn perf_window_pos(ctx: &egui::Context) -> Option<egui::Pos2> {
    let (outer, monitor) = ctx.input(|i| (i.viewport().outer_rect, i.viewport().monitor_size));
    let outer = outer?;
    let gap = 8.0;
    let right = outer.right() + gap;
    Some(
        if monitor.is_none_or(|m| right + PERF_WINDOW_SIZE[0] <= m.x) {
            egui::pos2(right, outer.top())
        } else {
            egui::pos2(
                (outer.left() - gap - PERF_WINDOW_SIZE[0]).max(0.0),
                outer.top(),
            )
        },
    )
}
/// Translucent threshold gridlines, then their even fainter ms labels.
const PERF_GRID: egui::Color32 = egui::Color32::from_rgba_premultiplied(0x50, 0x50, 0x50, 0x80);
const PERF_GRID_LABEL: egui::Color32 =
    egui::Color32::from_rgba_premultiplied(0x70, 0x70, 0x70, 0xA0);
/// Bar colours by frame time: green ≤ 60 fps, yellow ≤ 30 fps, red below.
const PERF_GOOD: egui::Color32 = egui::Color32::from_rgb(0x4C, 0xAF, 0x50);
const PERF_WARN: egui::Color32 = egui::Color32::from_rgb(0xE0, 0xB0, 0x30);
const PERF_BAD: egui::Color32 = egui::Color32::from_rgb(0xD9, 0x3A, 0x3A);

/// Frame-cost ring buffer with smoothed display values for the performance window.
///
/// Samples are the cost of *building* a frame, not the interval between frames, so they mean the same
/// thing whether the shell is repainting steadily or idle. Deliberately no FPS: under a reactive loop
/// repaint frequency measures how often something asked for a frame, not how expensive one is.
struct PerfStats {
    /// Per-frame CPU build cost, in seconds.
    costs: [f32; 30],
    write_idx: usize,
    display_ms: f32,
    display_p95_ms: f32,
    update_at: Instant,
    /// When the last sample landed, so the window can tell "cheap" apart from "not rendering".
    last_record: Instant,
}

impl PerfStats {
    fn new() -> Self {
        Self {
            costs: [0.0; 30],
            write_idx: 0,
            display_ms: 0.0,
            display_p95_ms: 0.0,
            update_at: Instant::now(),
            last_record: Instant::now(),
        }
    }

    /// What the shell is doing, for the window's status row. A reactive shell nobody is touching
    /// produces no frames, so a frozen readout is correct — this says so instead of looking broken.
    fn activity(&self) -> String {
        let idle = self.last_record.elapsed();
        if idle < Duration::from_millis(400) {
            "rendering".to_owned()
        } else {
            format!("idle {:.0}s", idle.as_secs_f32())
        }
    }

    /// Record one frame's build cost (seconds); ~4×/sec, refresh the smoothed average and p95.
    #[expect(
        clippy::cast_precision_loss,
        reason = "averaging 30 small, non-negative costs"
    )]
    fn record(&mut self, cost: f32) {
        self.costs[self.write_idx] = cost;
        self.write_idx = (self.write_idx + 1) % self.costs.len();
        self.last_record = Instant::now();
        if self.update_at.elapsed().as_secs_f32() > 0.25 {
            self.update_at = Instant::now();
            let avg = self.costs.iter().sum::<f32>() / self.costs.len() as f32;
            self.display_ms = avg * 1_000.0;
            let mut sorted = self.costs;
            sorted.sort_by(f32::total_cmp);
            self.display_p95_ms = sorted[sorted.len() * 95 / 100] * 1_000.0;
        }
    }
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
        PERF_LABEL
    };
    ui.painter()
        .add(egui::Shape::convex_polygon(pts, color, egui::Stroke::NONE));
    resp
}

fn render_performance(ui: &mut egui::Ui, perf: &PerfStats) {
    {
        let mut header = header_bar(ui);
        header.label(header_title("Performance"));
    }

    ui.add_space(6.0);
    render_metric_table(ui, perf);

    // Taken as a rect rather than through a `horizontal`, whose cross-axis sizing caps the height.
    ui.add_space(6.0);
    let left = ui.available_rect_before_wrap();
    let plot = egui::Rect::from_min_max(
        egui::pos2(left.left() + 6.0, left.top()),
        egui::pos2(left.right() - 6.0, left.bottom() - 6.0),
    );
    ui.allocate_rect(plot, egui::Sense::hover());
    render_sparkline(ui, &perf.costs, perf.write_idx, plot);
}

/// The readings, painted rather than laid out: a `Grid` sizes columns to content, so the box would
/// breathe whenever a reading changed width, and it draws no inner rules. Fixed geometry gives both.
fn render_metric_table(ui: &mut egui::Ui, perf: &PerfStats) {
    let font = egui::FontId::monospace(11.0);
    let rows = [
        ("Frame cost", format!("{:.1} ms", perf.display_ms)),
        ("p95", format!("{:.1} ms", perf.display_p95_ms)),
        ("Shell", perf.activity()),
    ];

    let avail = ui.available_rect_before_wrap();
    let table = egui::Rect::from_min_size(
        egui::pos2(avail.left() + 6.0, avail.top()),
        egui::vec2(avail.width() - 12.0, TABLE_ROW_H * rows.len() as f32),
    );
    ui.allocate_rect(table, egui::Sense::hover());

    let painter = ui.painter();
    let rule = egui::Stroke::new(1.0, HAIRLINE);
    painter.rect_stroke(table, 0.0, rule, egui::StrokeKind::Inside);
    let divider = table.left() + TABLE_LABEL_W;
    painter.vline(divider, table.y_range(), rule);

    let mut top = table.top();
    for (i, (name, reading)) in rows.iter().enumerate() {
        if i > 0 {
            painter.hline(table.x_range(), top, rule);
        }
        let mid = top + TABLE_ROW_H / 2.0;
        painter.text(
            egui::pos2(table.left() + 8.0, mid),
            egui::Align2::LEFT_CENTER,
            name,
            font.clone(),
            PERF_LABEL,
        );
        painter.text(
            egui::pos2(divider + 8.0, mid),
            egui::Align2::LEFT_CENTER,
            reading,
            font.clone(),
            egui::Color32::WHITE,
        );
        top += TABLE_ROW_H;
    }
}

/// Paint the frame-cost sparkline into `rect`, oldest sample first. The gridlines are frame budgets —
/// 17 ms (60 fps) and 33 ms (30 fps) — so a bar above one costs more than that budget allows.
#[expect(
    clippy::cast_precision_loss,
    reason = "small bar counts cast to pixel offsets"
)]
fn render_sparkline(ui: &egui::Ui, costs: &[f32; 30], write_idx: usize, rect: egui::Rect) {
    let n = costs.len();
    // A gutter for the gridline labels; the plot takes everything else.
    let plot_left = rect.left() + 30.0;
    let bar_h_max = (rect.height() - 2.0).max(4.0);
    let bar_stride = ((rect.right() - plot_left) / n as f32).max(1.0);
    let bar_fill = (bar_stride - 1.0).max(1.0);
    let spark_left = plot_left;
    let spark_bottom = rect.bottom() - 1.0;
    let spark_top = spark_bottom - bar_h_max;
    let scale_max = 1.0 / 30.0; // 33.3 ms fills the height.

    // Border around the plot area.
    let border_rect = egui::Rect::from_min_max(
        egui::pos2(spark_left - 1.0, spark_top - 1.0),
        egui::pos2(rect.right(), spark_bottom + 1.0),
    );
    ui.painter().rect_stroke(
        border_rect,
        0.0,
        egui::Stroke::new(1.0, HAIRLINE),
        egui::StrokeKind::Outside,
    );

    // Threshold gridlines, labelled on the left
    // in milliseconds (the axis is frame time, not FPS).
    let grid_stroke = egui::Stroke::new(1.0, PERF_GRID);
    let label_font = egui::FontId::monospace(7.0);
    for (label, frac) in [("17ms", 0.5_f32), ("33ms", 1.0_f32)] {
        let y = (spark_bottom - frac * bar_h_max).floor();
        ui.painter()
            .hline(spark_left..=border_rect.right() - 1.0, y, grid_stroke);
        let galley =
            ui.painter()
                .layout_no_wrap(label.to_owned(), label_font.clone(), PERF_GRID_LABEL);
        ui.painter().galley(
            egui::pos2(
                spark_left - galley.size().x - 3.0,
                y - galley.size().y / 2.0,
            ),
            galley,
            PERF_GRID_LABEL,
        );
    }

    // Bars, oldest (write_idx) at the left.
    for i in 0..n {
        let idx = (write_idx + i) % n;
        let t = costs[idx];
        if t <= 0.0 {
            continue;
        }
        let frac = (t / scale_max).clamp(0.0, 1.0);
        let x = (spark_left + i as f32 * bar_stride).floor();
        let bar_h = frac * bar_h_max;
        if bar_h < 0.5 {
            continue;
        }
        let color = if t <= 1.0 / 60.0 {
            PERF_GOOD
        } else if t <= 1.0 / 30.0 {
            PERF_WARN
        } else {
            PERF_BAD
        };
        let bar_top = (spark_bottom - bar_h).floor();
        let bar_rect = egui::Rect::from_min_size(
            egui::pos2(x, bar_top),
            egui::vec2(bar_fill, spark_bottom - bar_top),
        );
        ui.painter().rect_filled(bar_rect, 0.0, color);
    }
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
struct RunOptions {
    frames: Option<u32>,
    scene: Option<String>,
}

fn run_with<S: SceneSource + 'static>(
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

// --- Hot-reload source ---

mod hot {
    use super::{Manifest, SceneSource};
    use hot_lib_reloader::LibReloader;
    use std::time::Duration;

    /// A [`SceneSource`](super::SceneSource) reading scenes from a reloaded dylib: the dylib exports
    /// `__gallery_manifest() -> Manifest`, hot-swapped as it is rebuilt. The dylib directory comes from
    /// the running executable, so it follows any `CARGO_TARGET_DIR`. Both sides must share one
    /// gallery/egui version — a single workspace lock guarantees it.
    pub struct HotDylib {
        reloader: LibReloader,
        watching: bool,
    }

    impl HotDylib {
        /// Load `lib<lib_name>.<dylib-ext>` from the current executable's directory — the same
        /// `<target>/<profile>/` cargo drops both the host binary and the dylib into.
        ///
        /// `watching` says whether a watcher is rebuilding that dylib (`--hot`). Only then is there
        /// anything to poll for.
        ///
        /// # Errors
        /// If the executable path can't be read, or the dylib can't be loaded from that directory.
        pub fn new(lib_name: &str, watching: bool) -> Result<Self, Box<dyn std::error::Error>> {
            let exe = std::env::current_exe()?;
            let dir = exe
                .parent()
                .ok_or("current executable has no parent directory")?;
            let dir = camino::Utf8Path::from_path(dir).ok_or("executable path is not UTF-8")?;
            let reloader = LibReloader::new(dir, lib_name, Some(Duration::from_millis(200)), None)?;
            Ok(Self { reloader, watching })
        }
    }

    impl SceneSource for HotDylib {
        fn before_frame(&mut self, ctx: &egui::Context) {
            // Polling unconditionally kept the shell repainting 5×/s forever, so it never came to rest
            // and a frame-cost reading had nothing at rest to measure.
            if !self.watching {
                return;
            }
            // Swap in a rebuilt dylib, then keep polling so edits show without user input.
            let _ = self.reloader.update();
            ctx.request_repaint_after(Duration::from_millis(200));
        }

        fn manifest(&mut self) -> Manifest {
            // SAFETY: `__gallery_manifest` is exported by the scenes dylib built against the same gallery
            // (one workspace lock), so `Manifest`/`SceneEntry` layouts match. Its `&'static str`s point
            // into the loaded library and are used only this frame (before the next `update()`).
            let entry = unsafe {
                self.reloader
                    .get_symbol::<fn() -> Manifest>(b"__gallery_manifest\0")
            };
            match entry {
                Ok(manifest) => manifest(),
                Err(_) => Manifest {
                    scenes: Vec::new(),
                    groups: Vec::new(),
                },
            }
        }
    }
}

pub use hot::HotDylib;

// --- Config-driven launcher ---

/// The consumer's entire `main`. Both arguments are required — a `setup` closure and [`Settings`], which
/// names the [`Renderer`]:
///
/// ```ignore
/// fn main() -> gallery::eframe::Result {
///     gallery::launch!(|_| {}, gallery::Settings::new(gallery::Renderer::Wgpu))
/// }
/// ```
///
/// Expands to [`launch()`] with the calling crate's name and manifest dir filled in. `setup` runs
/// against the fresh egui context (e.g. `|ctx| egui_extras::install_image_loaders(ctx)`).
#[macro_export]
macro_rules! launch {
    ($setup:expr, $settings:expr) => {
        $crate::launch(
            env!("CARGO_PKG_NAME"),
            env!("CARGO_MANIFEST_DIR"),
            $settings,
            $setup,
        )
    };
}

/// Read the config, build the scenes dylib from its globs, load it, and open the window. Prefer the
/// [`launch!`] macro, which fills `package`/`manifest_dir` from the calling crate.
///
/// Args: `--config <path>` (default `<manifest_dir>/gallery.toml`); `--hot` (rebuild + swap on edits);
/// `--frames <n>` with optional `--scene <key>` for a deterministic profiling run that renders exactly
/// `n` frames and exits.
pub fn launch(
    package: &str,
    manifest_dir: &str,
    settings: Settings,
    setup: impl FnOnce(&egui::Context) + 'static,
) -> eframe::Result {
    if std::env::args().skip(1).any(|arg| arg == "--check-updates") {
        check_updates();
        return Ok(());
    }
    let args = launch_args(manifest_dir);
    let config = read_config(&args.config);
    let base = args.config.parent().unwrap_or_else(|| Utf8Path::new("."));
    let globs: Vec<String> = config
        .scene_globs
        .iter()
        .map(|glob| resolve_glob(base, glob))
        .collect();

    build_lib(manifest_dir, &globs);
    let watcher = if args.hot {
        spawn_watcher(manifest_dir, &globs)
    } else {
        None
    };
    // The dylib is `lib<crate>.so`; the crate's lib name is the package name with dashes as underscores.
    let source = HotDylib::new(&package.replace('-', "_"), args.hot)
        .expect("load the freshly built scenes dylib");
    let result = run_with(&config.title, source, settings, setup, args.options);
    // Window closed normally: stop the watcher (the Ctrl-C/SIGTERM path is handled in spawn_watcher).
    if let Some(watcher) = &watcher {
        let _ = watcher.lock().unwrap().kill();
    }
    result
}

#[derive(serde::Deserialize)]
struct Config {
    scene_globs: Vec<String>,
    #[serde(default = "default_title")]
    title: String,
}

fn default_title() -> String {
    "gallery".to_owned()
}

struct LaunchArgs {
    config: Utf8PathBuf,
    hot: bool,
    options: RunOptions,
}

fn launch_args(manifest_dir: &str) -> LaunchArgs {
    let mut config = None;
    let mut hot = false;
    let mut options = RunOptions::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--hot" => hot = true,
            "--config" => config = Some(args.next().expect("--config needs a path")),
            "--frames" => {
                let count = args.next().expect("--frames needs a count");
                options.frames = Some(count.parse().expect("--frames needs a number"));
            }
            "--scene" => options.scene = Some(args.next().expect("--scene needs a scene key")),
            other => panic!("unknown argument: {other}"),
        }
    }
    let path = config
        .map(Utf8PathBuf::from)
        .unwrap_or_else(|| Utf8Path::new(manifest_dir).join("gallery.toml"));
    let config = path
        .canonicalize_utf8()
        .unwrap_or_else(|e| panic!("config `{path}`: {e}"));
    LaunchArgs {
        config,
        hot,
        options,
    }
}

fn read_config(path: &Utf8Path) -> Config {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("read `{path}`: {e}"));
    toml::from_str(&text).unwrap_or_else(|e| panic!("parse `{path}`: {e}"))
}

/// Resolve a config-relative glob to an absolute one. Canonicalizes the directory prefix (up to the
/// first wildcard) so `..` is gone before it reaches `glob`, which walks components literally.
fn resolve_glob(config_dir: &Utf8Path, glob: &str) -> String {
    let wildcard = glob.find(['*', '?', '[']).unwrap_or(glob.len());
    let split = glob[..wildcard].rfind('/').map_or(0, |slash| slash + 1);
    let (dir, pattern) = glob.split_at(split);
    let base = config_dir.join(dir);
    let base = base.canonicalize_utf8().unwrap_or(base);
    base.join(pattern).into_string()
}

/// Build the scenes dylib once, blocking, so the loader finds a `.so` on first launch.
fn build_lib(manifest_dir: &str, globs: &[String]) {
    let built = cargo(manifest_dir, globs)
        .args(["build", "--lib"])
        .status()
        .is_ok_and(|status| status.success());
    assert!(built, "`cargo build --lib` for the scenes dylib failed");
}

/// A running hot-reload watcher, shared so both the window-close path
/// and the signal handler can kill it.
type Watcher = Arc<Mutex<Box<dyn ChildWrapper>>>;

/// Rebuild the scenes dylib on every scene change; each fresh `.so` is what `HotDylib` reloads.
/// The watcher runs as a process group (unix) / job object (windows), so killing it takes down
/// its whole tree — on window close (via the returned handle) and on Ctrl-C/SIGTERM (via the handler).
fn spawn_watcher(manifest_dir: &str, globs: &[String]) -> Option<Watcher> {
    let mut command = cargo(manifest_dir, globs);
    command.arg("watch");
    for dir in watch_dirs(manifest_dir, globs) {
        command.args(["-w", &dir]);
    }
    command.args(["-x", "build --lib"]);

    let mut wrapped = CommandWrap::from(command);
    #[cfg(unix)]
    wrapped.wrap(process_wrap::std::ProcessGroup::leader());
    #[cfg(windows)]
    wrapped.wrap(process_wrap::std::JobObject);

    let child = match wrapped.spawn() {
        Ok(child) => Arc::new(Mutex::new(child)),
        Err(e) => {
            eprintln!("gallery: `cargo watch` did not start — edits will not rebuild: {e}");
            return None;
        }
    };

    let on_signal = Arc::clone(&child);
    if let Err(e) = ctrlc::set_handler(move || {
        let _ = on_signal.lock().unwrap().kill();
        std::process::exit(130);
    }) {
        eprintln!("gallery: no signal handler — the watcher may outlive the window: {e}");
    }

    Some(child)
}

/// A cargo command in the crate dir, carrying the resolved globs to the scenes `build.rs`.
fn cargo(manifest_dir: &str, globs: &[String]) -> Command {
    let mut command = Command::new("cargo");
    command
        .current_dir(manifest_dir)
        .env("GALLERY_SCENE_GLOBS", globs.join("\n"));
    command
}

/// Dirs for cargo-watch to monitor: the crate plus each glob's base dir — scene files usually live
/// outside the crate, so cargo-watch won't see edits to them without an explicit `-w`.
fn watch_dirs(manifest_dir: &str, globs: &[String]) -> Vec<String> {
    let mut dirs = vec![manifest_dir.to_owned()];
    for glob in globs {
        let end = glob.find(['*', '?', '[']).unwrap_or(glob.len());
        if let Some(slash) = glob[..end].rfind('/') {
            dirs.push(glob[..slash].to_owned());
        }
    }
    dirs.sort();
    dirs.dedup();
    dirs
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

// --- Update check ---

/// Print whether a newer `gallery` release is out, plus the CHANGELOG entries since this build's version.
/// A dev-tool convenience (`cargo run -- --check-updates` / `just update`): fetch the upstream CHANGELOG
/// and compare its released `## [x.y.z]` sections against this crate's own version.
fn check_updates() {
    let current = env!("CARGO_PKG_VERSION");
    let repo = env!("CARGO_PKG_REPOSITORY");
    let Some(url) = raw_changelog_url(repo) else {
        eprintln!("gallery: can't derive a CHANGELOG URL from repository `{repo}`");
        return;
    };
    let changelog = match fetch(&url) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("gallery: couldn't fetch the upstream CHANGELOG ({url}): {e}");
            return;
        }
    };

    let installed = semver::Version::parse(current).ok();
    let mut newer: Vec<(semver::Version, String)> = released_sections(&changelog)
        .into_iter()
        .filter(|(version, _)| installed.as_ref().is_none_or(|cur| version > cur))
        .collect();
    newer.sort_by(|a, b| b.0.cmp(&a.0));

    if newer.is_empty() {
        println!("gallery {current} is up to date — no newer release upstream.");
        return;
    }
    println!("A newer gallery is out (you're on {current}):\n");
    for (version, notes) in newer {
        println!("## {version}\n{}\n", notes.trim());
    }
}

/// `https://github.com/owner/repo(.git)` → the raw CHANGELOG on the `main` branch; `None` for a
/// non-GitHub repository.
fn raw_changelog_url(repo: &str) -> Option<String> {
    let path = repo.strip_prefix("https://github.com/")?;
    let path = path.strip_suffix('/').unwrap_or(path);
    let path = path.strip_suffix(".git").unwrap_or(path);
    Some(format!(
        "https://raw.githubusercontent.com/{path}/main/CHANGELOG.md"
    ))
}

/// Fetch `url` over HTTPS in-process (`ureq`) — portable, with no reliance on a system `curl`.
fn fetch(url: &str) -> Result<String, String> {
    ureq::get(url)
        .call()
        .map_err(|e| e.to_string())?
        .into_string()
        .map_err(|e| e.to_string())
}

/// The released `## [x.y.z]` sections of a Keep-a-Changelog document, as `(version, notes)` — skipping
/// `## [Unreleased]` and any heading whose bracketed name isn't a semver version.
fn released_sections(changelog: &str) -> Vec<(semver::Version, String)> {
    let mut sections = Vec::new();
    let mut current: Option<(semver::Version, String)> = None;
    for line in changelog.lines() {
        if let Some(heading) = line.strip_prefix("## ") {
            if let Some(section) = current.take() {
                sections.push(section);
            }
            // `[x.y.z] - 2026-01-01` → `x.y.z`
            let name = heading
                .trim()
                .trim_start_matches('[')
                .split(']')
                .next()
                .unwrap_or_default()
                .trim();
            if let Ok(version) = semver::Version::parse(name) {
                current = Some((version, String::new()));
            }
        } else if let Some((_, notes)) = current.as_mut() {
            notes.push_str(line);
            notes.push('\n');
        }
    }
    if let Some(section) = current.take() {
        sections.push(section);
    }
    sections
}

#[cfg(test)]
mod tests {
    use egui_kittest::kittest::Queryable;

    use super::*;

    fn noop(_: &mut SceneCtx) {}

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
    fn raw_changelog_url_maps_github_to_raw_and_rejects_others() {
        let expected = "https://raw.githubusercontent.com/kubijo/rs-gallery/main/CHANGELOG.md";
        assert_eq!(
            raw_changelog_url("https://github.com/kubijo/rs-gallery.git").unwrap(),
            expected
        );
        assert_eq!(
            raw_changelog_url("https://github.com/kubijo/rs-gallery").unwrap(),
            expected
        );
        assert!(raw_changelog_url("https://gitlab.com/x/y").is_none());
    }

    #[test]
    fn released_sections_skips_unreleased_and_keeps_versioned_notes() {
        let changelog = "\
## [Unreleased]

- wip

## [0.2.0] - 2026-01-02

- added b

## [0.1.0] - 2026-01-01

- added a
";
        let sections = released_sections(changelog);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].0, semver::Version::new(0, 2, 0));
        assert!(sections[0].1.contains("added b"));
        assert_eq!(sections[1].0, semver::Version::new(0, 1, 0));
        assert!(sections[1].1.contains("added a"));
    }

    fn scene(name: &'static str, module_path: &'static str, default: bool) -> SceneEntry {
        SceneEntry {
            render: noop,
            name,
            module_path,
            default,
            order: u32::MAX,
            source: "",
        }
    }

    fn ordered(name: &'static str, order: u32) -> SceneEntry {
        SceneEntry {
            order,
            ..scene(name, "m", false)
        }
    }

    fn group(module_path: &'static str, title: &'static str) -> SceneGroupMeta {
        SceneGroupMeta { module_path, title }
    }

    #[test]
    fn longest_group_picks_the_deepest_matching_prefix() {
        let groups = [group("a", "A"), group("a::b", "A / B")];
        assert_eq!(longest_group(&groups, "a::b::s").unwrap().title, "A / B");
        assert_eq!(longest_group(&groups, "a::x").unwrap().title, "A");
        assert!(longest_group(&groups, "z").is_none());
    }

    #[test]
    fn breadcrumb_is_the_bare_title_for_a_default_scene_and_appends_the_name_otherwise() {
        let groups = [group("m", "MyCar / Map")];
        assert_eq!(
            breadcrumb(&scene("view", "m", true), &groups),
            "MyCar / Map"
        );
        assert_eq!(
            breadcrumb(&scene("aerial", "m", false), &groups),
            "MyCar / Map / aerial"
        );
        assert_eq!(breadcrumb(&scene("loose", "x", false), &[]), "loose");
    }

    #[test]
    fn scene_key_joins_module_path_and_name() {
        assert_eq!(scene_key(&scene("map", "app::map", true)), "app::map::map");
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

    #[test]
    fn build_tree_nests_each_scene_under_its_title_path() {
        let manifest = Manifest {
            scenes: vec![scene("view", "m", true), scene("dash", "d", true)],
            groups: vec![group("m", "MyCar / Map"), group("d", "MyCar / Dashboard")],
        };
        let tree = build_tree(&manifest);
        let mycar = &tree.children["MyCar"];
        assert_eq!(mycar.children["Map"].scenes, vec![0]);
        assert_eq!(mycar.children["Dashboard"].scenes, vec![1]);
    }

    #[test]
    fn an_ungrouped_scene_lands_at_the_root() {
        let manifest = Manifest {
            scenes: vec![scene("loose", "x", true)],
            groups: vec![],
        };
        let tree = build_tree(&manifest);
        assert_eq!(tree.scenes, vec![0]);
        assert!(tree.children.is_empty());
    }

    #[test]
    fn build_tree_sorts_scenes_by_order_then_name() {
        // Registration (link) order is deliberately not the wanted order.
        let manifest = Manifest {
            scenes: vec![
                ordered("beta", 10),
                ordered("alpha", 10),
                ordered("first", 0),
            ],
            groups: vec![group("m", "Group")],
        };
        let node = &build_tree(&manifest).children["Group"];
        // order 0 leads; the order-10 tie breaks by name (alpha before beta).
        assert_eq!(node.scenes, vec![2, 1, 0]);
    }

    #[test]
    fn perf_stats_starts_zeroed() {
        let perf = PerfStats::new();
        assert_eq!(perf.display_ms, 0.0);
        assert_eq!(perf.display_p95_ms, 0.0);
    }

    #[test]
    fn perf_stats_ring_buffer_wraps() {
        let mut perf = PerfStats::new();
        let cap = perf.costs.len();
        for _ in 0..cap + 2 {
            perf.record(0.016);
        }
        assert_eq!(perf.write_idx, 2);
    }

    #[test]
    fn perf_stats_smooths_over_the_window() {
        let mut perf = PerfStats::new();
        for _ in 0..perf.costs.len() {
            perf.record(1.0 / 60.0);
        }
        // Reopen the ~4×/sec smoothing window without waiting on the wall clock.
        perf.update_at -= Duration::from_millis(300);
        perf.record(1.0 / 60.0);
        // Every sample is the same cost, so the average and the p95 land on it.
        assert!(
            (perf.display_ms - 16.67).abs() < 0.1,
            "avg {}",
            perf.display_ms
        );
        assert!(
            (perf.display_p95_ms - 16.67).abs() < 0.1,
            "p95 {}",
            perf.display_p95_ms
        );
    }

    #[test]
    fn performance_window_renders_with_its_title() {
        let mut perf = PerfStats::new();
        for _ in 0..perf.costs.len() {
            perf.record(1.0 / 60.0);
        }
        let mut harness = egui_kittest::Harness::new_ui(move |ui| {
            render_performance(ui, &perf);
        });
        harness.run();
        assert!(harness.query_by_label("Performance").is_some());
    }

    #[test]
    fn fuzzy_matches_subsequences_only() {
        assert!(fuzzy("Dashboard", "Dashboard"));
        assert!(fuzzy("Dashboard", "Dash"));
        assert!(!fuzzy("Dashboard", "zzz"));
    }

    #[test]
    fn node_matches_own_name_scenes_and_descendants() {
        let manifest = Manifest {
            scenes: vec![scene("view", "m", true)],
            groups: vec![group("m", "MyCar / Map")],
        };
        let tree = build_tree(&manifest);
        let mycar = &tree.children["MyCar"];
        assert!(node_matches("MyCar", mycar, &manifest.scenes, "Map"));
        assert!(node_matches("MyCar", mycar, &manifest.scenes, "Car"));
        assert!(!node_matches("MyCar", mycar, &manifest.scenes, "zzz"));
    }

    #[test]
    fn visible_scenes_lists_them_all_when_unfiltered() {
        let manifest = Manifest {
            scenes: vec![scene("view", "m", true), scene("dash", "d", true)],
            groups: vec![group("m", "MyCar / Map"), group("d", "MyCar / Dashboard")],
        };
        let tree = build_tree(&manifest);
        let mut out = Vec::new();
        visible_scenes(&tree, &manifest.scenes, "", false, &mut out);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn resolve_glob_joins_the_config_dir_and_keeps_the_wildcard_tail() {
        let resolved = resolve_glob(Utf8Path::new("cfg"), "a/b/*.scene.rs");
        assert!(resolved.contains("a/b"));
        assert!(resolved.ends_with("*.scene.rs"));
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
