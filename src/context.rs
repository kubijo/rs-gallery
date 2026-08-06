//! The per-scene rendering context.
//!
//! [`SceneCtx`] is what a scene receives each frame. Its knob accessors are **declarative by use**:
//! calling `ctx.slider(...)` both registers the control and returns its current value — the first frame
//! creates it at its default, later frames return the value set in the controls panel. Values persist in
//! the host per scene, so they survive hot-reloads. Under the glow renderer it also exposes offscreen GL
//! rendering (see [`SceneCtx::offscreen`]).

use eframe::glow;

use crate::knobs::{ChoiceStyle, Knob, Pad2DSpec};
use crate::offscreen::{GlDeps, Offscreen, Pointer};

/// What a scene receives each frame: the egui [`Ui`](egui::Ui) to draw into, plus the knob accessors.
///
/// ```ignore
/// #[scene("greeting")]
/// fn greeting(ctx: &mut SceneCtx) {
///     let name = ctx.text("name", "world");
///     ctx.ui.heading(format!("Hello, {name}"));
/// }
/// ```
pub struct SceneCtx<'a> {
    pub ui: &'a mut egui::Ui,
    knobs: &'a mut Vec<Knob>,
    cursor: usize,
    stages: usize,
    /// Counts this frame's `offscreen` calls, so each keeps its own render target.
    offscreens: usize,
    gl: Option<GlDeps<'a>>,
}

/// How much room a [`stage`](SceneCtx::stage) takes.
/// `(300.0, 200.0)`, `(300, 200)` and `200` (a square) all convert.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Stage {
    Fit,
    /// For a component that behaves differently at different sizes
    /// — a scroll area, a wrapping layout, anything with a breakpoint.
    Fixed(egui::Vec2),
    Fill,
}

impl Stage {
    /// Let the stage scroll vertically when its content outgrows it.
    ///
    /// Only does something on a stage with a bound to outgrow,
    /// so pair it with [`Self::Fill`] or [`Self::Fixed`].
    /// A [`Self::Fit`] stage grows to its content and never overflows.
    ///
    /// The stage owns the scroll area rather than the scene,
    /// so the bar and the clipped edge line up with the checkerboard
    /// while the padding stays with the content inside the viewport.
    ///
    /// A scene that wants to skip the rows
    /// it cannot see reads [`egui::Ui::clip_rect`].
    #[must_use]
    pub fn scrollable(self) -> StageSpec {
        StageSpec {
            size: self,
            scroll: true,
        }
    }
}

/// A [`Stage`] and how it behaves — what [`stage`](SceneCtx::stage)
/// actually takes. Every `Stage` converts, so the flags are opt-in
/// and spelling a plain size still works.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct StageSpec {
    size: Stage,
    scroll: bool,
}

impl From<Stage> for StageSpec {
    fn from(size: Stage) -> Self {
        Self {
            size,
            scroll: false,
        }
    }
}

impl From<egui::Vec2> for Stage {
    fn from(size: egui::Vec2) -> Self {
        Self::Fixed(size)
    }
}

impl From<egui::Vec2> for StageSpec {
    fn from(size: egui::Vec2) -> Self {
        Stage::from(size).into()
    }
}

/// `(width, height)`, and a bare side length for a square, per numeric type.
macro_rules! stage_from {
    ($($number:ty),+ $(,)?) => {$(
        impl From<($number, $number)> for Stage {
            fn from((width, height): ($number, $number)) -> Self {
                Self::Fixed(egui::vec2(width as f32, height as f32))
            }
        }

        impl From<$number> for Stage {
            fn from(side: $number) -> Self {
                Self::Fixed(egui::Vec2::splat(side as f32))
            }
        }

        // Spelled out per type rather than blanketed over `Into<Stage>`,
        // which would overlap the reflexive `From<StageSpec>`.
        impl From<($number, $number)> for StageSpec {
            fn from(size: ($number, $number)) -> Self {
                Stage::from(size).into()
            }
        }

        impl From<$number> for StageSpec {
            fn from(side: $number) -> Self {
                Stage::from(side).into()
            }
        }
    )+};
}

stage_from!(f32, f64, u16, i16, u32, i32, usize, isize);

/// Breathing room between a staged component
/// and the edge of its checkerboard.
const PADDING: i8 = 16;

impl<'a> SceneCtx<'a> {
    pub(crate) fn new(
        ui: &'a mut egui::Ui,
        knobs: &'a mut Vec<Knob>,
        gl: Option<GlDeps<'a>>,
    ) -> Self {
        Self {
            ui,
            knobs,
            cursor: 0,
            stages: 0,
            offscreens: 0,
            gl,
        }
    }

    /// Put a component on the checkerboard, captioned with its size and collapsible.
    /// The canvas is plain, so headings and prose drawn onto `ui` read as headings and prose.
    ///
    /// [`stage!`](crate::stage) spells the common cases shorter.
    pub fn stage(&mut self, size: impl Into<StageSpec>, add: impl FnOnce(&mut egui::Ui)) {
        let StageSpec { size, scroll } = size.into();
        let id = self.ui.id().with(("gallery-stage", self.stages));
        self.stages += 1;
        let mut open = self.ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(true);

        // The badge sits above the stage but reports a size `Fit` only knows
        // once drawn, so keep the position and paint the text in afterwards.
        let badge_at = self
            .ui
            .horizontal(|ui| {
                let arrow = if open { "▾" } else { "▸" };
                if ui
                    .add(egui::Button::new(arrow).small().frame(false))
                    .on_hover_text(if open { "Collapse" } else { "Expand" })
                    .clicked()
                {
                    open = !open;
                    ui.data_mut(|d| d.insert_temp(id, open));
                }
                ui.cursor().min
            })
            .inner;

        // `fill` is passed in rather than read off `ui`: inside a scroll area the available width
        // shrinks the moment the bar appears, and a stage that resized to it would take the bar
        // away again, flipping every frame.
        let sized = |ui: &mut egui::Ui, fill: egui::Vec2| {
            ui.scope(|ui| match size {
                Stage::Fit => add(ui),
                Stage::Fixed(wanted) => {
                    ui.allocate_ui(wanted, |ui| {
                        ui.set_min_size(wanted);
                        add(ui);
                    });
                }
                Stage::Fill => {
                    ui.allocate_ui(fill, |ui| {
                        ui.set_min_size(fill);
                        add(ui);
                    });
                }
            })
            .response
            .rect
        };
        let padding = egui::Margin::same(PADDING);

        // Reserved before the content, so the checkerboard lands beneath it.
        let backdrop = self.ui.painter().add(egui::Shape::Noop);
        let framed = egui::Frame::new()
            // A scrolling stage pads inside its viewport instead, so the bar and the clipped edge
            // reach the checkerboard rather than stopping a margin short of it.
            .inner_margin(if scroll { egui::Margin::ZERO } else { padding })
            .show(self.ui, |ui| {
                if !open {
                    return ui.min_rect();
                }
                let available = ui.available_size();
                if !scroll {
                    return sized(ui, available);
                }
                // egui fades a scrollable edge to `ui.stack().bg_color()` — the nearest opaque frame
                // fill above it. A stage's frame is transparent, so that finds the panel behind
                // and the fade paints the panel's colour over the checkerboard.
                ui.style_mut().spacing.scroll.fade.strength = 0.0;
                // Clamped: a stage squeezed thinner than its own padding would otherwise
                // ask for a negative box, which egui rejects outright.
                let inside = (available - padding.sum()).max(egui::Vec2::ZERO);
                let scrolling = |ui: &mut egui::Ui| {
                    egui::ScrollArea::vertical()
                        .show(ui, |ui| {
                            egui::Frame::new()
                                .inner_margin(padding)
                                .show(ui, |ui| sized(ui, inside))
                                .inner
                        })
                        // The badge reports the stage itself, not the run of content it scrolls
                        // over — a viewport of 200 rows is 200 rows tall, which says nothing.
                        .inner_rect
                };
                match size {
                    // A fixed stage claims its box first and scrolls inside it. A scroll area sizes
                    // itself to what is available, so left to ask for its own room it takes only
                    // what the canvas had left — reporting that as the stage, and leaving nothing
                    // underneath for the checkerboard. Claiming it first also lets the box outgrow
                    // the canvas, which is what tells a capture to come back bigger.
                    Stage::Fixed(wanted) => {
                        let boxed = wanted + padding.sum();
                        let claimed = ui
                            .allocate_ui(boxed, |ui| {
                                ui.set_min_size(boxed);
                                scrolling(ui);
                            })
                            .response
                            .rect;
                        claimed.shrink2(padding.sum() / 2.0)
                    }
                    Stage::Fit | Stage::Fill => scrolling(ui),
                }
            });

        if open {
            let backdrop_rect = framed.response.rect;
            self.ui.painter().set(
                backdrop,
                egui::Shape::Vec(crate::checkerboard(backdrop_rect)),
            );
        }
        // The component's own size, not the padded box around it.
        let content = framed.inner;
        self.ui.painter().text(
            badge_at,
            egui::Align2::LEFT_TOP,
            format!("{:.0}×{:.0}", content.width(), content.height()),
            egui::FontId::proportional(10.0),
            self.ui.visuals().weak_text_color(),
        );
    }

    /// Whether the stage this call is about to make is open — the id [`stage`](Self::stage)
    /// will derive, read before it does.
    fn staging_open(&self) -> bool {
        let id = self.ui.id().with(("gallery-stage", self.stages));
        self.ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(true)
    }

    /// [`offscreen`](Self::offscreen) on a stage: the checkerboard, size caption and collapse toggle
    /// egui content gets, so a rendered frame reads as the same kind of thing as a widget.
    /// `None` while the stage is folded, which is also when `draw` never runs — a fold costs no GL.
    ///
    /// `stage` sizes the box as it does for any content, so [`Stage::Fit`] is the image itself;
    /// `size` is the texture's own, in pixels.
    pub fn offscreen_stage(
        &mut self,
        stage: impl Into<StageSpec>,
        size: impl Into<[u32; 2]>,
        draw: impl FnOnce(&Offscreen),
    ) -> Option<egui::Response> {
        self.staged(stage.into(), size.into(), draw, egui::Sense::hover())
    }

    /// [`offscreen_input`](Self::offscreen_input) on a stage — see
    /// [`offscreen_stage`](Self::offscreen_stage) for the chrome, and [`Pointer`] for the events.
    /// `None` while the stage is collapsed: nothing is drawn, so nothing can be pointed at.
    pub fn offscreen_input_stage(
        &mut self,
        stage: impl Into<StageSpec>,
        size: impl Into<[u32; 2]>,
        draw: impl FnOnce(&Offscreen),
    ) -> Option<(egui::Response, Vec<Pointer>)> {
        let size = size.into();
        let response = self.staged(stage.into(), size, draw, egui::Sense::click_and_drag())?;
        let events = crate::offscreen::pointer(self.ui, response.id, response.rect, size);
        Some((response, events))
    }

    /// Render the target for this call site, then show it inside a stage.
    ///
    /// Two steps rather than one closure: `stage` lends a `Ui` and nothing more, while the GL half
    /// needs the whole context. Drawing the texture first leaves only an image, which a `Ui` can stage.
    #[expect(
        clippy::cast_precision_loss,
        reason = "small, non-negative pixel dimensions"
    )]
    fn staged(
        &mut self,
        stage: StageSpec,
        size: [u32; 2],
        draw: impl FnOnce(&Offscreen),
        sense: egui::Sense,
    ) -> Option<egui::Response> {
        // Claimed whether or not the GL runs, so folding one stage away doesn't move the targets
        // of the call sites after it.
        let at = self.offscreens;
        self.offscreens += 1;
        let drawn = self
            .staging_open()
            .then(|| self.gl.as_mut().map(|deps| deps.render(at, size, draw)));

        let mut shown = None;
        self.stage(stage, |ui| {
            shown = Some(match drawn.flatten() {
                Some(tex_id) => {
                    // GL textures are bottom-left origin; flip V so the image reads upright.
                    let sized = egui::load::SizedTexture::new(
                        tex_id,
                        egui::vec2(size[0] as f32, size[1] as f32),
                    );
                    ui.add(
                        egui::Image::new(sized)
                            .uv(egui::Rect::from_min_max(
                                egui::pos2(0.0, 1.0),
                                egui::pos2(1.0, 0.0),
                            ))
                            .sense(sense),
                    )
                }
                None => {
                    ui.colored_label(egui::Color32::YELLOW, "offscreen() needs the glow renderer")
                }
            });
        });
        shown
    }

    /// The GL proc-address loader — `Some` only under [`Renderer::Glow`](crate::Renderer::Glow). Build a
    /// femtovg renderer (`OpenGl::new_from_function_cstr`) or your own `glow::Context` from it at any
    /// glow/femtovg version — gallery pins none. The low-level floor under [`offscreen`](Self::offscreen).
    #[must_use]
    pub fn gl_loader(&self) -> Option<crate::GlLoader> {
        self.gl.as_ref().map(|deps| deps.loader.clone())
    }

    /// Register a raw GL texture name (an offscreen FBO's colour attachment) with egui and return a
    /// [`TextureId`](egui::TextureId) to draw via `ui.image(...)`. You pass the GL name, not a typed
    /// `glow::Texture`, so it stays glow-version-agnostic. Glow renderer only — panics under wgpu. The
    /// escape hatch behind [`offscreen`](Self::offscreen).
    pub fn register_native_texture(&mut self, gl_name: std::num::NonZeroU32) -> egui::TextureId {
        let deps = self
            .gl
            .as_mut()
            .expect("register_native_texture requires the glow renderer");
        (deps.register)(glow::NativeTexture(gl_name))
    }

    /// Render non-egui content into an offscreen texture of `size` pixels and show it inline. gallery
    /// owns the framebuffer + texture (one per call site, kept across frames, resized in place,
    /// registered once), binds it around `draw`, and returns the shown image's
    /// [`Response`](egui::Response). Inside `draw`, build a GL library (femtovg, raw glow, …)
    /// from [`Offscreen::gl_loader`] and paint into the bound FBO — at any glow/femtovg version.
    /// Glow renderer only; under wgpu it shows a hint instead.
    ///
    /// Call sites are told apart by the order the scene makes them, as knobs are, so a scene staging
    /// several images gives each its own; one behind a toggle shifts the ones after it to new targets.
    pub fn offscreen(
        &mut self,
        size: impl Into<[u32; 2]>,
        draw: impl FnOnce(&Offscreen),
    ) -> egui::Response {
        self.shown(size.into(), draw, egui::Sense::hover())
    }

    /// Like [`offscreen`](Self::offscreen), and reports the pointer that landed on the image
    /// — press, move, release and wheel, in its own pixel space — for content that hit-tests itself.
    /// [`Pointer`] has the coordinate and capture rules.
    ///
    /// The image takes the drag and the wheel, so neither also moves the canvas behind it;
    /// a scene that would rather leave the canvas scrolling calls [`offscreen`](Self::offscreen).
    pub fn offscreen_input(
        &mut self,
        size: impl Into<[u32; 2]>,
        draw: impl FnOnce(&Offscreen),
    ) -> (egui::Response, Vec<Pointer>) {
        let size = size.into();
        let response = self.shown(size, draw, egui::Sense::click_and_drag());
        if self.gl.is_none() {
            return (response, Vec::new());
        }
        let events = crate::offscreen::pointer(self.ui, response.id, response.rect, size);
        (response, events)
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "small, non-negative pixel dimensions"
    )]
    fn shown(
        &mut self,
        size: [u32; 2],
        draw: impl FnOnce(&Offscreen),
        sense: egui::Sense,
    ) -> egui::Response {
        // Taken before the deps are borrowed, and counted even under wgpu,
        // so a call site keeps the same slot whichever renderer the scene meets.
        let at = self.offscreens;
        self.offscreens += 1;
        let Some(deps) = self.gl.as_mut() else {
            return self
                .ui
                .colored_label(egui::Color32::YELLOW, "offscreen() needs the glow renderer");
        };
        let tex_id = deps.render(at, size, draw);
        // GL textures are bottom-left origin; flip V so the image reads upright in egui.
        let sized =
            egui::load::SizedTexture::new(tex_id, egui::vec2(size[0] as f32, size[1] as f32));
        self.ui.add(
            egui::Image::new(sized)
                .uv(egui::Rect::from_min_max(
                    egui::pos2(0.0, 1.0),
                    egui::pos2(1.0, 0.0),
                ))
                .sense(sense),
        )
    }

    /// How many knobs the scene declared this frame — the shell truncates the store to this, dropping
    /// controls the scene stopped declaring (e.g. one hidden behind a now-off toggle).
    pub(crate) fn declared(&self) -> usize {
        self.cursor
    }

    /// The knob at the current cursor, created (or replaced, if its kind/label changed) from `fresh`.
    fn slot(
        &mut self,
        fresh: impl FnOnce() -> Knob,
        keep: impl FnOnce(&Knob) -> bool,
    ) -> &mut Knob {
        let i = self.cursor;
        self.cursor += 1;
        if i >= self.knobs.len() {
            self.knobs.push(fresh());
        } else if !keep(&self.knobs[i]) {
            self.knobs[i] = fresh();
        }
        &mut self.knobs[i]
    }

    pub fn text(&mut self, label: &str, default: &str) -> String {
        match self.slot(
            || Knob::Text {
                label: label.to_owned(),
                value: default.to_owned(),
            },
            |k| matches!(k, Knob::Text { label: l, .. } if l == label),
        ) {
            Knob::Text { value, .. } => value.clone(),
            _ => default.to_owned(),
        }
    }

    /// A numeric slider over `min..=max`.
    /// `step` is the snap increment and sets the readout's decimals
    /// (`0.1` → one, `0.01` → two); pass `0.0` for a smooth slider.
    pub fn slider(&mut self, label: &str, default: f32, min: f32, max: f32, step: f32) -> f32 {
        match self.slot(
            || Knob::Slider {
                label: label.to_owned(),
                value: default,
                min,
                max,
                step,
            },
            |k| matches!(k, Knob::Slider { label: l, .. } if l == label),
        ) {
            Knob::Slider { value, .. } => *value,
            _ => default,
        }
    }

    pub fn toggle(&mut self, label: &str, default: bool) -> bool {
        match self.slot(
            || Knob::Toggle {
                label: label.to_owned(),
                value: default,
            },
            |k| matches!(k, Knob::Toggle { label: l, .. } if l == label),
        ) {
            Knob::Toggle { value, .. } => *value,
            _ => default,
        }
    }

    pub fn color(&mut self, label: &str, default: egui::Color32) -> egui::Color32 {
        match self.slot(
            || Knob::Color {
                label: label.to_owned(),
                value: default,
            },
            |k| matches!(k, Knob::Color { label: l, .. } if l == label),
        ) {
            Knob::Color { value, .. } => *value,
            _ => default,
        }
    }

    /// A dropdown of `options`; returns the selected index.
    pub fn select(&mut self, label: &str, options: &[&str], default: usize) -> usize {
        self.choice(label, options, default, ChoiceStyle::Dropdown)
    }

    /// Like [`select`](Self::select), rendered as a vertical stack of radio buttons.
    pub fn radio(&mut self, label: &str, options: &[&str], default: usize) -> usize {
        self.choice(label, options, default, ChoiceStyle::Radio)
    }

    /// Like [`select`](Self::select), rendered as an inline segmented row of buttons;
    /// long option sets wrap onto further rows.
    pub fn buttons(&mut self, label: &str, options: &[&str], default: usize) -> usize {
        self.choice(label, options, default, ChoiceStyle::Buttons)
    }

    fn choice(
        &mut self,
        label: &str,
        options: &[&str],
        default: usize,
        style: ChoiceStyle,
    ) -> usize {
        let options: Vec<String> = options.iter().map(|opt| (*opt).to_owned()).collect();
        let last = options.len().saturating_sub(1);
        let knob = self.slot(
            {
                let options = options.clone();
                move || Knob::Select {
                    label: label.to_owned(),
                    value: default.min(last),
                    options,
                    style,
                }
            },
            |k| matches!(k, Knob::Select { label: l, style: s, .. } if l == label && *s == style),
        );
        match knob {
            Knob::Select {
                value,
                options: current,
                ..
            } => {
                *current = options; // options can change between frames; keep them fresh
                (*value).min(last)
            }
            _ => default,
        }
    }

    /// A labelled separator grouping the following knobs.
    pub fn group(&mut self, label: &str) {
        self.slot(
            || Knob::Group {
                label: label.to_owned(),
            },
            |k| matches!(k, Knob::Group { label: l } if l == label),
        );
    }

    /// A 2-axis pad; returns the current `(x, y)`.
    pub fn pad2d(&mut self, label: &str, spec: Pad2DSpec) -> (f32, f32) {
        match self.slot(
            || Knob::Pad2D {
                label: label.to_owned(),
                x: spec.default_x,
                y: spec.default_y,
                min_x: spec.min_x,
                max_x: spec.max_x,
                min_y: spec.min_y,
                max_y: spec.max_y,
                invert_y: spec.invert_y,
            },
            |k| matches!(k, Knob::Pad2D { label: l, .. } if l == label),
        ) {
            Knob::Pad2D { x, y, .. } => (*x, *y),
            _ => (spec.default_x, spec.default_y),
        }
    }

    /// Write a slider's stored value, clamped to its range and snapped to its `step`.
    ///
    /// The `set_*` family is the write half of the accessors:
    /// content that does its own hit-testing reads the knob, observes the event, then writes it back.
    /// Keep that order — a write takes the first knob of its kind labelled exactly `label`,
    /// which before the declaring accessor may still be last frame's.
    /// A miss changes nothing and returns `false`; landing is not surviving,
    /// since a knob the scene stops declaring is truncated away, write and all.
    pub fn set_slider(&mut self, label: &str, value: f32) -> bool {
        if !value.is_finite() {
            return false;
        }
        let found = self.knobs.iter_mut().find_map(|knob| match knob {
            Knob::Slider {
                label: l,
                value,
                min,
                max,
                step,
            } if l == label => Some((value, *min, *max, *step)),
            _ => None,
        });
        let Some((stored, min, max, step)) = found else {
            return false;
        };
        // `f32::clamp` panics on a NaN bound, which a scene is free to declare.
        if min.is_nan() || max.is_nan() {
            return false;
        }
        // Clamp then snap from the range's start, as egui's `Slider::set_value` does — so a write
        // and a drag agree. Clamped again after, because a step that doesn't divide the range
        // rounds its last stop past the end, and a knob holding what its own slider would reject
        // is worse than the hair of disagreement.
        let (lo, hi) = (min.min(max), min.max(max));
        let mut value = value.clamp(lo, hi);
        if step > 0.0 && min.is_finite() {
            value = (min + ((value - min) / step).round() * step).clamp(lo, hi);
        }
        *stored = value;
        true
    }

    pub fn set_toggle(&mut self, label: &str, value: bool) -> bool {
        match self.knobs.iter_mut().find_map(|knob| match knob {
            Knob::Toggle { label: l, value } if l == label => Some(value),
            _ => None,
        }) {
            Some(stored) => {
                *stored = value;
                true
            }
            None => false,
        }
    }

    pub fn set_text(&mut self, label: &str, value: impl Into<String>) -> bool {
        match self.knobs.iter_mut().find_map(|knob| match knob {
            Knob::Text { label: l, value } if l == label => Some(value),
            _ => None,
        }) {
            Some(stored) => {
                *stored = value.into();
                true
            }
            None => false,
        }
    }

    pub fn set_color(&mut self, label: &str, value: egui::Color32) -> bool {
        match self.knobs.iter_mut().find_map(|knob| match knob {
            Knob::Color { label: l, value } if l == label => Some(value),
            _ => None,
        }) {
            Some(stored) => {
                *stored = value;
                true
            }
            None => false,
        }
    }

    /// Write a choice knob's selection by option label, exactly matched.
    /// An unknown option is an identity miss and drops the write;
    /// an index merely out of range clamps ([`set_select_index`](Self::set_select_index)).
    pub fn set_select(&mut self, label: &str, option: &str) -> bool {
        let Some((stored, options)) = self.select_mut(label) else {
            return false;
        };
        match options.iter().position(|opt| opt == option) {
            Some(at) => {
                *stored = at;
                true
            }
            None => false,
        }
    }

    /// Write a choice knob's selection by index, clamped to the last option.
    pub fn set_select_index(&mut self, label: &str, index: usize) -> bool {
        let Some((stored, options)) = self.select_mut(label) else {
            return false;
        };
        if options.is_empty() {
            return false;
        }
        *stored = index.min(options.len() - 1);
        true
    }

    fn select_mut(&mut self, label: &str) -> Option<(&mut usize, &[String])> {
        self.knobs.iter_mut().find_map(|knob| match knob {
            Knob::Select {
                label: l,
                value,
                options,
                ..
            } if l == label => Some((value, options.as_slice())),
            _ => None,
        })
    }

    /// Write a 2-axis pad's stored position, each axis clamped to its own range.
    pub fn set_pad2d(&mut self, label: &str, x: f32, y: f32) -> bool {
        if !x.is_finite() || !y.is_finite() {
            return false;
        }
        let found = self.knobs.iter_mut().find_map(|knob| match knob {
            Knob::Pad2D {
                label: l,
                x,
                y,
                min_x,
                max_x,
                min_y,
                max_y,
                ..
            } if l == label => Some((x, y, *min_x, *max_x, *min_y, *max_y)),
            _ => None,
        });
        let Some((sx, sy, min_x, max_x, min_y, max_y)) = found else {
            return false;
        };
        // `f32::clamp` panics on a NaN bound, which a scene is free to declare.
        if [min_x, max_x, min_y, max_y]
            .iter()
            .any(|bound| bound.is_nan())
        {
            return false;
        }
        *sx = x.clamp(min_x.min(max_x), min_x.max(max_x));
        *sy = y.clamp(min_y.min(max_y), min_y.max(max_y));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The accessors don't draw; a throwaway `Ui` (egui_kittest) just builds the `SceneCtx`.

    #[test]
    fn slider_declares_at_its_default_then_returns_the_stored_value() {
        let mut harness = egui_kittest::Harness::new_ui(|ui| {
            let mut knobs = Vec::new();
            assert_eq!(
                SceneCtx::new(ui, &mut knobs, None).slider("amt", 0.5, 0.0, 1.0, 0.1),
                0.5
            );
            assert_eq!(knobs.len(), 1);
            if let Knob::Slider { value, .. } = &mut knobs[0] {
                *value = 0.8;
            }
            assert_eq!(
                SceneCtx::new(ui, &mut knobs, None).slider("amt", 0.5, 0.0, 1.0, 0.1),
                0.8
            );
            assert_eq!(knobs.len(), 1);
        });
        harness.run();
    }

    #[test]
    fn a_knob_is_recreated_when_its_label_changes() {
        let mut harness = egui_kittest::Harness::new_ui(|ui| {
            let mut knobs = Vec::new();
            SceneCtx::new(ui, &mut knobs, None).slider("a", 0.5, 0.0, 1.0, 0.1);
            if let Knob::Slider { value, .. } = &mut knobs[0] {
                *value = 0.9;
            }
            assert_eq!(
                SceneCtx::new(ui, &mut knobs, None).slider("b", 0.2, 0.0, 1.0, 0.1),
                0.2
            );
        });
        harness.run();
    }

    #[test]
    fn declared_counts_the_knobs_used_this_frame() {
        let mut harness = egui_kittest::Harness::new_ui(|ui| {
            let mut knobs = Vec::new();
            let mut ctx = SceneCtx::new(ui, &mut knobs, None);
            ctx.slider("a", 0.0, 0.0, 1.0, 0.1);
            ctx.toggle("b", false);
            assert_eq!(ctx.declared(), 2);
        });
        harness.run();
    }

    #[test]
    fn select_clamps_an_out_of_range_default_to_the_last_option() {
        let mut harness = egui_kittest::Harness::new_ui(|ui| {
            let mut knobs = Vec::new();
            assert_eq!(
                SceneCtx::new(ui, &mut knobs, None).select("s", &["x", "y"], 9),
                1
            );
        });
        harness.run();
    }

    #[test]
    fn buttons_declares_a_select_knob_in_the_buttons_style() {
        let mut harness = egui_kittest::Harness::new_ui(|ui| {
            let mut knobs = Vec::new();
            assert_eq!(
                SceneCtx::new(ui, &mut knobs, None).buttons("mode", &["a", "b", "c"], 1),
                1
            );
            assert!(matches!(
                &knobs[0],
                Knob::Select {
                    style: ChoiceStyle::Buttons,
                    value: 1,
                    ..
                }
            ));
        });
        harness.run();
    }

    #[test]
    fn changing_a_choice_style_at_the_same_label_recreates_the_knob() {
        let mut harness = egui_kittest::Harness::new_ui(|ui| {
            let mut knobs = Vec::new();
            SceneCtx::new(ui, &mut knobs, None).radio("m", &["a", "b"], 0);
            if let Knob::Select { value, .. } = &mut knobs[0] {
                *value = 1;
            }
            assert_eq!(
                SceneCtx::new(ui, &mut knobs, None).buttons("m", &["a", "b"], 0),
                0,
                "switching style at the same label drops the stored value"
            );
        });
        harness.run();
    }

    #[test]
    fn every_knob_kind_takes_a_scene_write_and_reads_it_back() {
        let mut harness = egui_kittest::Harness::new_ui(|ui| {
            let mut knobs = Vec::new();
            {
                let mut ctx = SceneCtx::new(ui, &mut knobs, None);
                ctx.slider("amt", 0.5, 0.0, 1.0, 0.0);
                ctx.toggle("on", false);
                ctx.text("name", "before");
                ctx.color("tint", egui::Color32::BLACK);
                ctx.select("gear", &["p", "d", "r"], 0);
                ctx.pad2d("aim", Pad2DSpec::default());
            }
            {
                let mut ctx = SceneCtx::new(ui, &mut knobs, None);
                assert!(ctx.set_slider("amt", 0.75));
                assert!(ctx.set_toggle("on", true));
                assert!(ctx.set_text("name", "after"));
                assert!(ctx.set_color("tint", egui::Color32::from_rgb(0x11, 0x22, 0x33)));
                assert!(ctx.set_select("gear", "d"));
                assert!(ctx.set_pad2d("aim", 0.25, -0.5));
            }
            let mut ctx = SceneCtx::new(ui, &mut knobs, None);
            assert_eq!(ctx.slider("amt", 0.5, 0.0, 1.0, 0.0), 0.75);
            assert!(ctx.toggle("on", false));
            assert_eq!(ctx.text("name", "before"), "after");
            assert_eq!(
                ctx.color("tint", egui::Color32::BLACK),
                egui::Color32::from_rgb(0x11, 0x22, 0x33)
            );
            assert_eq!(ctx.select("gear", &["p", "d", "r"], 0), 1);
            assert_eq!(ctx.pad2d("aim", Pad2DSpec::default()), (0.25, -0.5));
        });
        harness.run();
    }

    #[test]
    fn a_written_value_clamps_to_the_knob_and_snaps_to_its_step() {
        let mut harness = egui_kittest::Harness::new_ui(|ui| {
            let mut knobs = Vec::new();
            {
                let mut ctx = SceneCtx::new(ui, &mut knobs, None);
                ctx.slider("amt", 0.5, 0.0, 1.0, 0.25);
                // A step the range is not a multiple of, so its last stop overshoots the end.
                ctx.slider("odd", 0.0, 0.0, 1.0, 0.6);
                ctx.select("gear", &["p", "d", "r"], 0);
                ctx.pad2d("aim", Pad2DSpec::default());
            }
            {
                let mut ctx = SceneCtx::new(ui, &mut knobs, None);
                assert!(ctx.set_slider("amt", 7.0), "an overshoot still lands");
                assert_eq!(
                    ctx.slider("amt", 0.5, 0.0, 1.0, 0.25),
                    1.0,
                    "clamped to the range"
                );
                assert!(ctx.set_slider("amt", 0.6));
                assert!(ctx.set_slider("odd", 1.0));
                assert!(ctx.set_select_index("gear", 9));
                assert!(ctx.set_pad2d("aim", 5.0, -5.0));
            }
            let mut ctx = SceneCtx::new(ui, &mut knobs, None);
            assert_eq!(
                ctx.slider("amt", 0.5, 0.0, 1.0, 0.25),
                0.5,
                "0.6 snaps to the 0.25 grid, as the panel widget would"
            );
            assert_eq!(
                ctx.slider("odd", 0.0, 0.0, 1.0, 0.6),
                1.0,
                "snapping never rounds past the end it was just clamped to"
            );
            assert_eq!(
                ctx.select("gear", &["p", "d", "r"], 0),
                2,
                "an index past the end clamps to the last option"
            );
            assert_eq!(
                ctx.pad2d("aim", Pad2DSpec::default()),
                (1.0, -1.0),
                "each axis clamps to its own range"
            );
        });
        harness.run();
    }

    #[test]
    fn a_select_write_takes_an_option_label_but_not_a_stranger() {
        let mut harness = egui_kittest::Harness::new_ui(|ui| {
            let mut knobs = Vec::new();
            {
                let mut ctx = SceneCtx::new(ui, &mut knobs, None);
                ctx.select("gear", &["p", "d", "r"], 0);
                ctx.select("empty", &[], 0);
            }
            {
                let mut ctx = SceneCtx::new(ui, &mut knobs, None);
                assert!(ctx.set_select("gear", "r"));
                assert!(
                    !ctx.set_select("gear", "x"),
                    "an option the knob doesn't carry is dropped"
                );
                assert!(
                    !ctx.set_select_index("empty", 3),
                    "no options means nothing to clamp to"
                );
            }
            assert_eq!(
                SceneCtx::new(ui, &mut knobs, None).select("gear", &["p", "d", "r"], 0),
                2,
                "the dropped writes left the landed one alone"
            );
        });
        harness.run();
    }

    #[test]
    fn a_write_to_a_missing_knob_is_dropped_without_a_phantom() {
        let mut harness = egui_kittest::Harness::new_ui(|ui| {
            let mut knobs = Vec::new();
            assert!(
                !SceneCtx::new(ui, &mut knobs, None).set_slider("amt", 1.0),
                "nothing is declared yet on the first frame"
            );
            assert!(knobs.is_empty(), "a miss creates no control");

            SceneCtx::new(ui, &mut knobs, None).toggle("amt", false);
            assert!(
                !SceneCtx::new(ui, &mut knobs, None).set_slider("amt", 1.0),
                "the label exists, but on another kind"
            );
            assert_eq!(knobs.len(), 1);
        });
        harness.run();
    }

    #[test]
    fn a_non_finite_write_is_dropped_rather_than_stored() {
        let mut harness = egui_kittest::Harness::new_ui(|ui| {
            let mut knobs = Vec::new();
            {
                let mut ctx = SceneCtx::new(ui, &mut knobs, None);
                ctx.slider("amt", 0.5, 0.0, 1.0, 0.0);
                ctx.pad2d("aim", Pad2DSpec::default());
            }
            {
                let mut ctx = SceneCtx::new(ui, &mut knobs, None);
                assert!(!ctx.set_slider("amt", f32::NAN));
                assert!(!ctx.set_slider("amt", f32::INFINITY));
                assert!(!ctx.set_pad2d("aim", f32::NAN, 0.0));
            }
            let mut ctx = SceneCtx::new(ui, &mut knobs, None);
            assert_eq!(ctx.slider("amt", 0.5, 0.0, 1.0, 0.0), 0.5);
            assert_eq!(ctx.pad2d("aim", Pad2DSpec::default()), (0.0, 0.0));
        });
        harness.run();
    }

    #[test]
    fn a_write_lands_before_an_accessor_later_the_same_frame() {
        let mut harness = egui_kittest::Harness::new_ui(|ui| {
            let mut knobs = Vec::new();
            SceneCtx::new(ui, &mut knobs, None).slider("amt", 0.5, 0.0, 1.0, 0.0);
            // It holds last frame's slot, so a write before this frame's declaration finds it.
            let mut ctx = SceneCtx::new(ui, &mut knobs, None);
            assert!(ctx.set_slider("amt", 0.9));
            assert_eq!(ctx.slider("amt", 0.5, 0.0, 1.0, 0.0), 0.9);
        });
        harness.run();
    }

    #[test]
    fn a_written_value_survives_exactly_as_long_as_its_knob_stays_declared() {
        let mut harness = egui_kittest::Harness::new_ui(|ui| {
            let mut knobs = Vec::new();
            {
                let mut ctx = SceneCtx::new(ui, &mut knobs, None);
                ctx.slider("a", 0.1, 0.0, 1.0, 0.0);
                ctx.toggle("b", false);
                assert!(ctx.set_slider("a", 0.7));
                assert!(ctx.set_toggle("b", true), "landing is not surviving");
            }
            // The scene stops declaring `b`; the shell truncates as `render_canvas` does.
            let declared = {
                let mut ctx = SceneCtx::new(ui, &mut knobs, None);
                ctx.slider("a", 0.1, 0.0, 1.0, 0.0);
                ctx.declared()
            };
            knobs.truncate(declared);

            let mut ctx = SceneCtx::new(ui, &mut knobs, None);
            assert_eq!(
                ctx.slider("a", 0.1, 0.0, 1.0, 0.0),
                0.7,
                "a still-declared knob keeps its written value"
            );
            assert!(
                !ctx.toggle("b", false),
                "a truncated knob comes back at its default, write and all"
            );
        });
        harness.run();
    }

    #[test]
    fn any_size_shape_converts_to_a_fixed_stage() {
        assert_eq!(
            Stage::from((300.0, 200.0)),
            Stage::Fixed(egui::vec2(300.0, 200.0))
        );
        assert_eq!(
            Stage::from((300, 200)),
            Stage::Fixed(egui::vec2(300.0, 200.0)),
            "integer pairs convert, so a call site need not spell floats"
        );
        assert_eq!(
            Stage::from(64),
            Stage::Fixed(egui::Vec2::splat(64.0)),
            "a bare number is a square"
        );
        assert_eq!(Stage::from(64.0_f32), Stage::Fixed(egui::Vec2::splat(64.0)));
        assert_eq!(
            Stage::from(egui::vec2(4.0, 5.0)),
            Stage::Fixed(egui::vec2(4.0, 5.0))
        );
    }

    /// Guards the macro's arm order: `fit`/`fill` would otherwise parse
    /// as a size, and a lone closure has to fall through to the fitted arm.
    #[test]
    fn the_macro_accepts_every_stage_form() {
        let mut harness = egui_kittest::Harness::new_ui(|ui| {
            let mut knobs = Vec::new();
            let mut ctx = SceneCtx::new(ui, &mut knobs, None);
            let mut drawn = 0;

            crate::stage!(ctx, |ui| {
                ui.label("implicitly fitted");
                drawn += 1;
            });
            crate::stage!(ctx, fit, |ui| {
                ui.label("explicitly fitted");
                drawn += 1;
            });
            crate::stage!(ctx, fill, |ui| {
                ui.label("filled");
                drawn += 1;
            });
            crate::stage!(ctx, (120.0, 80.0), |ui| {
                ui.label("float pair");
                drawn += 1;
            });
            crate::stage!(ctx, (120, 80), |ui| {
                ui.label("integer pair");
                drawn += 1;
            });
            crate::stage!(ctx, 64, |ui| {
                ui.label("square");
                drawn += 1;
            });
            crate::stage!(ctx, scroll, |ui| {
                ui.label("scrolling");
                drawn += 1;
            });
            ctx.stage(Stage::Fixed(egui::vec2(120.0, 80.0)).scrollable(), |ui| {
                ui.label("a scrolling stage of its own size");
                drawn += 1;
            });

            assert_eq!(drawn, 8, "every form ran its body");
        });
        // Stepped rather than run to quiescence: scroll area fades its bar in
        // over time and so keeps asking for frames, which `run` treats as a runaway UI.
        harness.run_steps(2);
    }

    /// A fixed stage states its box, and `.scrollable()` says
    /// the content may run past it — not that the box gives way.
    ///
    /// Taking the whole canvas instead reports the canvas as the stage's size,
    /// and leaves no room beneath it for the checkerboard —
    /// which is how a capture loses its bottom edge.
    #[test]
    fn a_fixed_scrolling_stage_keeps_to_its_own_box_rather_than_the_canvas() {
        let took = std::cell::Cell::new(0.0);
        let room = std::cell::Cell::new(0.0);
        let mut harness = egui_kittest::Harness::new_ui(|ui| {
            let mut knobs = Vec::new();
            let mut ctx = SceneCtx::new(ui, &mut knobs, None);
            room.set(ctx.ui.available_height());
            let before = ctx.ui.min_rect().height();
            ctx.stage(Stage::Fixed(egui::vec2(120.0, 80.0)).scrollable(), |ui| {
                for row in 0..200 {
                    ui.label(format!("Row {row}"));
                }
            });
            took.set(ctx.ui.min_rect().height() - before);
        });
        harness.run_steps(2);

        // The box and its padding, plus the badge above it — nowhere near the canvas it sits on.
        let box_and_padding = 80.0 + 2.0 * f32::from(PADDING);
        assert!(
            took.get() < room.get() && took.get() < box_and_padding * 2.0,
            "a 120×80 stage took {} of {} available",
            took.get(),
            room.get()
        );
    }

    /// The canvas a scene draws on is itself a scroll area, so a scrolling stage that grew to its
    /// content would scroll the canvas too and leave the viewer two bars around one list.
    #[test]
    fn a_scrolling_stage_keeps_to_the_canvas_instead_of_growing_to_its_content() {
        // Cells, because the closure holds its captures for as long as the harness lives.
        let room = std::cell::Cell::new(0.0);
        let took = std::cell::Cell::new(0.0);
        let mut harness = egui_kittest::Harness::new_ui(|ui| {
            egui::ScrollArea::both().show(ui, |ui| {
                let mut knobs = Vec::new();
                let mut ctx = SceneCtx::new(ui, &mut knobs, None);
                room.set(ctx.ui.available_height());
                let before = ctx.ui.min_rect().height();
                ctx.stage(Stage::Fill.scrollable(), |ui| {
                    for row in 0..200 {
                        ui.label(format!("Row {row}"));
                    }
                });
                took.set(ctx.ui.min_rect().height() - before);
            });
        });
        harness.run_steps(2);

        assert!(
            took.get() <= room.get(),
            "200 rows tower over the canvas, yet the stage took {} of {}",
            took.get(),
            room.get()
        );
    }
}
