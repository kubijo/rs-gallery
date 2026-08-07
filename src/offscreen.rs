//! Offscreen GL rendering for the glow backend.
//!
//! Lets a scene paint non-egui content — femtovg, raw glow — into a framebuffer that gallery owns and
//! shows inline. The public entry point is [`SceneCtx::offscreen`](crate::SceneCtx::offscreen); this
//! module holds the cached [`RenderTarget`], the [`Offscreen`] draw handle, and the glow deps the shell
//! wires in.

use std::collections::HashMap;

use eframe::glow::{self, HasContext};

/// Each scene's cached offscreen render targets ([`SceneCtx::offscreen`](crate::SceneCtx::offscreen)),
/// keyed by scene identity — one per call site, in the order the scene makes them.
pub(crate) type TargetStore = HashMap<String, Vec<RenderTarget>>;

/// A scene's cached offscreen framebuffer — a colour texture plus a depth/stencil renderbuffer (femtovg
/// fills need stencil) — registered with egui once. The shell owns it so scenes needn't manage GL.
pub(crate) struct RenderTarget {
    fbo: glow::NativeFramebuffer,
    texture: glow::NativeTexture,
    rbo: glow::NativeRenderbuffer,
    tex_id: egui::TextureId,
    size: [u32; 2],
}

impl RenderTarget {
    /// # Safety
    /// `gl` must be the live glow context for the current backend.
    #[expect(
        clippy::cast_possible_wrap,
        reason = "small, non-negative pixel dimensions"
    )]
    unsafe fn create(
        gl: &glow::Context,
        register: &mut dyn FnMut(glow::NativeTexture) -> egui::TextureId,
        size: [u32; 2],
    ) -> Self {
        let (w, h) = (size[0] as i32, size[1] as i32);
        // SAFETY: `gl` is the live context (fn contract); standard offscreen-FBO setup.
        let (fbo, texture, rbo) = unsafe {
            let texture = gl.create_texture().expect("create GL texture");
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::SRGB8_ALPHA8 as i32,
                w,
                h,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::LINEAR as i32,
            );
            let fbo = gl.create_framebuffer().expect("create GL framebuffer");
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(texture),
                0,
            );
            let rbo = gl.create_renderbuffer().expect("create GL renderbuffer");
            gl.bind_renderbuffer(glow::RENDERBUFFER, Some(rbo));
            gl.renderbuffer_storage(glow::RENDERBUFFER, glow::DEPTH24_STENCIL8, w, h);
            gl.framebuffer_renderbuffer(
                glow::FRAMEBUFFER,
                glow::DEPTH_STENCIL_ATTACHMENT,
                glow::RENDERBUFFER,
                Some(rbo),
            );
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.bind_renderbuffer(glow::RENDERBUFFER, None);
            (fbo, texture, rbo)
        };
        let tex_id = register(texture);
        Self {
            fbo,
            texture,
            rbo,
            tex_id,
            size,
        }
    }

    /// Reallocate the colour texture and depth/stencil storage to `size`, keeping the same GL names — so
    /// the framebuffer and its egui `TextureId` stay valid. Reusing the FBO in place (rather than
    /// recreating it) keeps its GL name stable, so a scene's cached renderer can target it once and keep
    /// working across resizes; it also avoids leaking the un-freeable egui `TextureId` (eframe exposes no
    /// `free_native_glow_texture`).
    ///
    /// # Safety
    /// `gl` must be the live glow context.
    #[expect(
        clippy::cast_possible_wrap,
        reason = "small, non-negative pixel dimensions"
    )]
    unsafe fn resize(&mut self, gl: &glow::Context, size: [u32; 2]) {
        let (w, h) = (size[0] as i32, size[1] as i32);
        // SAFETY: `gl` is the live context (fn contract); same allocations as `create`, new dimensions.
        unsafe {
            gl.bind_texture(glow::TEXTURE_2D, Some(self.texture));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::SRGB8_ALPHA8 as i32,
                w,
                h,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
            );
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.bind_renderbuffer(glow::RENDERBUFFER, Some(self.rbo));
            gl.renderbuffer_storage(glow::RENDERBUFFER, glow::DEPTH24_STENCIL8, w, h);
            gl.bind_renderbuffer(glow::RENDERBUFFER, None);
        }
        self.size = size;
    }
}

/// Handle passed to the [`SceneCtx::offscreen`](crate::SceneCtx::offscreen) closure — its FBO is bound
/// and the GL viewport set. Draw into it with any GL library built from [`gl_loader`](Self::gl_loader).
///
/// # Colour space
///
/// The target is `SRGB8_ALPHA8` and its sampler decodes, so bytes written straight in come back one
/// decode darker — mid-grey `128` reads as `54`. A 2D library that hands you sRGB-encoded bytes
/// (femtovg, cairo, skia) therefore looks right in its own preview and crushed here.
///
/// Enable `FRAMEBUFFER_SRGB` around the draw and the write encodes, cancelling the decode
/// to within a least-significant bit, the encode and the decode rounding independently:
///
/// ```ignore
/// gl.enable(glow::FRAMEBUFFER_SRGB);
/// // ...draw...
/// gl.disable(glow::FRAMEBUFFER_SRGB);
/// ```
///
/// A scene computing in linear light wants it off, which is the default.
/// Both halves are pinned by `an_offscreen_decodes_what_is_written_unless_the_scene_encodes_it`.
pub struct Offscreen {
    loader: crate::GlLoader,
    size: [u32; 2],
    fbo: std::num::NonZeroU32,
}

impl Offscreen {
    /// The GL proc-address loader (see [`SceneCtx::gl_loader`](crate::SceneCtx::gl_loader)).
    #[must_use]
    pub fn gl_loader(&self) -> crate::GlLoader {
        self.loader.clone()
    }

    /// The target's pixel size, `[width, height]`.
    #[must_use]
    pub fn size(&self) -> [u32; 2] {
        self.size
    }

    /// The GL name of the framebuffer gallery bound for this draw. Most GL libraries render to the
    /// currently-bound framebuffer and need nothing more — but some rebind on flush and must be told
    /// this name (femtovg's `set_screen_target`, for one, otherwise falls back to the default
    /// framebuffer). The name stays stable across resizes (gallery reallocates in place), so a cached
    /// renderer can be pointed at it once.
    #[must_use]
    pub fn fbo(&self) -> std::num::NonZeroU32 {
        self.fbo
    }
}

/// A texture the scene owns, for [`SceneCtx::texture_stage`](crate::SceneCtx::texture_stage)
/// to put on a stage. Gallery composites it and never frees it — register once and keep
/// the [`TextureId`](egui::TextureId), since eframe exposes no way to release one.
///
/// The way in is [`SceneCtx::register_native_texture`](crate::SceneCtx::register_native_texture)
/// under the glow renderer, but any `TextureId` works, including one from [`egui::Context::load_texture`].
///
/// # Allocating loosely and showing tightly
///
/// [`showing`](Self::showing) says how much of the texture to draw, from its top-left.
/// A scene whose height depends on what it lays out can allocate generously once, render,
/// and show the height it measured — all in one frame, where sizing a gallery-owned target
/// means rendering, measuring, asking for a repaint and resizing, with a frame shown
/// at the wrong size in between.
///
/// It also keeps the texture from being reallocated, which is what would leak a `TextureId`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct StageTexture {
    texture: egui::TextureId,
    allocated: [u32; 2],
    shown: [u32; 2],
    interactive: bool,
}

impl StageTexture {
    /// A texture of `size` pixels, all of it shown.
    #[must_use]
    pub fn new(texture: egui::TextureId, size: impl Into<[u32; 2]>) -> Self {
        let allocated = size.into();
        Self {
            texture,
            allocated,
            shown: allocated,
            interactive: false,
        }
    }

    /// Draw only this much of it, measured from the top-left. Clamped to what was allocated.
    ///
    /// Pass an extent something actually measured, not whatever the layout handed back: a node
    /// that reports no extent gives zero, and a stage told to show zero pixels says so in place
    /// of the image rather than drawing a sliver that reads as a lost texture.
    #[must_use]
    pub fn showing(mut self, shown: impl Into<[u32; 2]>) -> Self {
        let shown = shown.into();
        self.shown = [
            shown[0].min(self.allocated[0]),
            shown[1].min(self.allocated[1]),
        ];
        self
    }

    /// Take the pointer that lands on the image, and report it in the image's own pixels.
    ///
    /// Off by default, since a stage taking the drag and the wheel stops the canvas behind it
    /// from scrolling — worth it for content that hit-tests itself, a cost for content that
    /// doesn't.
    #[must_use]
    pub fn interactive(mut self) -> Self {
        self.interactive = true;
        self
    }

    /// Where the shown part sits in the texture, V flipped.
    ///
    /// GL textures are bottom-left origin while a stage reads top-down, so the rect runs backwards
    /// in y, and the crop has to come off the end the flip put the content on.
    /// Derived at a call site instead, it crops an upside-down image
    /// — quietly keeping the slack and cutting the content.
    pub(crate) fn uv(self) -> egui::Rect {
        let ratio = |shown: u32, allocated: u32| match allocated {
            0 => 0.0,
            whole => f64::from(shown) / f64::from(whole),
        };
        #[expect(
            clippy::cast_possible_truncation,
            reason = "a ratio of two small pixel counts, in 0..=1"
        )]
        let (across, down) = (
            ratio(self.shown[0], self.allocated[0]) as f32,
            ratio(self.shown[1], self.allocated[1]) as f32,
        );
        egui::Rect::from_min_max(egui::pos2(0.0, 1.0), egui::pos2(across, 1.0 - down))
    }

    pub(crate) fn shown(self) -> [u32; 2] {
        self.shown
    }

    pub(crate) fn texture(self) -> egui::TextureId {
        self.texture
    }

    pub(crate) fn sense(self) -> egui::Sense {
        if self.interactive {
            egui::Sense::click_and_drag()
        } else {
            egui::Sense::hover()
        }
    }

    pub(crate) fn is_interactive(self) -> bool {
        self.interactive
    }
}

/// An image gallery drew, and the pointer that landed on it.
///
/// `pointers` is empty unless the image asked to be [`interactive`](StageTexture::interactive).
pub struct ImageInput {
    pub response: egui::Response,
    pub pointers: Vec<Pointer>,
}

/// One pointer event that landed on an offscreen image, in that image's own pixel space:
/// `(0, 0)` at its top-left, [`Offscreen::size`] as the extent.
///
/// A drag that runs off the edge keeps reporting, so coordinates can fall outside those bounds.
/// Whatever egui calls a pointer arrives here, so a touchscreen drives this as a mouse does.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Pointer {
    Down {
        x: f32,
        y: f32,
    },
    Move {
        x: f32,
        y: f32,
    },
    Up {
        x: f32,
        y: f32,
    },
    /// A wheel or trackpad scroll; `delta` is in egui's points rather than image pixels.
    Wheel {
        x: f32,
        y: f32,
        delta: egui::Vec2,
    },
}

/// The pointer input that landed on the image drawn at `rect`, mapped into its pixel space.
///
/// Coordinates come off the whole `rect`, even where the canvas scrolled part of it out of sight
/// — measuring from the visible part would shift them by however much is clipped —
/// while the visible part is what a press must land on.
///
/// A press captures until its release, so a drag off the edge keeps reporting and the release
/// always arrives; without that, content that saw the press would stay held.
/// The capture lives in egui's memory under `id`, which is what carries it between frames.
pub(crate) fn pointer(
    ui: &egui::Ui,
    id: egui::Id,
    rect: egui::Rect,
    size: [u32; 2],
) -> Vec<Pointer> {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return Vec::new();
    }
    let visible = rect.intersect(ui.clip_rect());
    let extent = egui::vec2(size[0] as f32, size[1] as f32);
    // Normalised rather than a bare offset, so the mapping holds at whatever size egui shows it.
    let to_image = |pos: egui::Pos2| {
        let at = ((pos - rect.min) / rect.size()) * extent;
        (at.x, at.y)
    };

    let mut captured = ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    let events = ui.input(|input| {
        let mut events = Vec::new();
        for event in &input.events {
            match *event {
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    ..
                } => {
                    if pressed {
                        if visible.contains(pos) {
                            let (x, y) = to_image(pos);
                            events.push(Pointer::Down { x, y });
                            captured = true;
                        }
                    } else if captured {
                        let (x, y) = to_image(pos);
                        events.push(Pointer::Up { x, y });
                        captured = false;
                    }
                }
                egui::Event::PointerMoved(pos)
                    if input.pointer.primary_down() && (captured || visible.contains(pos)) =>
                {
                    let (x, y) = to_image(pos);
                    events.push(Pointer::Move { x, y });
                }
                _ => {}
            }
        }
        let scrolled = input.smooth_scroll_delta;
        if scrolled != egui::Vec2::ZERO
            && let Some(pos) = input.pointer.latest_pos()
            && visible.contains(pos)
        {
            let (x, y) = to_image(pos);
            events.push(Pointer::Wheel {
                x,
                y,
                delta: scrolled,
            });
        }
        // Holding the pointer with nothing pressed means the release went somewhere we never saw
        // — the image stopped being drawn mid-drag, say. Let go rather than pair the next one.
        if captured && !input.pointer.any_down() {
            captured = false;
        }
        events
    });
    ui.data_mut(|d| d.insert_temp(id, captured));
    // Taken before the enclosing scroll area reads it, which it does once the canvas closes:
    // content that scrolls would otherwise scroll the canvas under it too.
    if events.iter().any(|at| matches!(at, Pointer::Wheel { .. })) {
        ui.input_mut(|input| input.smooth_scroll_delta = egui::Vec2::ZERO);
    }
    events
}

/// Hands a GL texture to egui and gets back an id to draw it by.
///
/// A closure rather than the `eframe::Frame` a window uses: `Frame::_new_kittest` leaves the hook
/// `None` and the field is `pub(crate)`, so a capture registers through `egui_glow`'s painter instead.
///
/// Owned rather than borrowed, because `&'a mut (dyn FnMut + 'a)` puts `'a` in an invariant position
/// and [`GlDeps`] must stay covariant to reach a shorter-lived [`crate::SceneCtx`].
pub(crate) type RegisterTexture<'a> = Box<dyn FnMut(glow::NativeTexture) -> egui::TextureId + 'a>;

/// The glow-backend handles a scene needs for non-egui rendering — the loader, gallery's own glow
/// context (for FBO bookkeeping), a way to register a texture with egui, and this scene's cached
/// targets. Present only under [`Renderer::Glow`](crate::Renderer::Glow).
pub(crate) struct GlDeps<'a> {
    pub loader: crate::GlLoader,
    pub gl: &'a glow::Context,
    pub register: RegisterTexture<'a>,
    pub targets: &'a mut Vec<RenderTarget>,
}

impl GlDeps<'_> {
    /// Ensure the target for the call site at `at` matches `size` — creating it,
    /// or resizing it in place — then bind it, clear it, run `draw`, and put back what was bound,
    /// returning the colour texture to show. That attachment is bottom-left origin, so the caller
    /// flips V when displaying it.
    ///
    /// `at` counts a scene's `offscreen` calls in the order it makes them, so each keeps its own
    /// texture and one image is never another's pixels. The store only grows: a call a scene stops
    /// making leaves its target behind — eframe exposes no way to release a registered `TextureId`,
    /// the same reason [`RenderTarget::resize`] reallocates — and takes it up again if it returns.
    ///
    /// Putting back what was bound, rather than framebuffer 0: a window's egui paints to 0,
    /// but a headless capture paints into an FBO of its own, and everything the scene draws
    /// after this call would otherwise land somewhere the capture never reads.
    #[expect(
        clippy::cast_possible_wrap,
        reason = "small, non-negative pixel dimensions"
    )]
    pub(crate) fn render(
        &mut self,
        at: usize,
        size: [u32; 2],
        draw: impl FnOnce(&Offscreen),
    ) -> egui::TextureId {
        // SAFETY (every block below): `self.gl` is the live glow context handed in by the shell.
        while self.targets.len() <= at {
            let target = unsafe { RenderTarget::create(self.gl, &mut *self.register, size) };
            self.targets.push(target);
        }
        let target = &mut self.targets[at];
        if target.size != size {
            unsafe { target.resize(self.gl, size) };
        }
        let target = &self.targets[at];
        let (tex_id, fbo) = (target.tex_id, target.fbo);
        // SAFETY: read the binding to put back, since it is not always framebuffer 0.
        let previous = unsafe { self.gl.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING) };
        // SAFETY: bind the scene's FBO for `draw`; the previous binding goes back below.
        unsafe {
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            self.gl.viewport(0, 0, size[0] as i32, size[1] as i32);
            self.gl.clear_color(0.0, 0.0, 0.0, 0.0);
            self.gl
                .clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT | glow::STENCIL_BUFFER_BIT);
        }
        draw(&Offscreen {
            loader: self.loader.clone(),
            size,
            fbo: fbo.0,
        });
        // SAFETY: `previous` came from GL itself a moment ago, so it names a live framebuffer or 0.
        unsafe {
            let restore =
                std::num::NonZeroU32::new(previous.unsigned_abs()).map(glow::NativeFramebuffer);
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, restore);
        }
        tex_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A whole texture is drawn flipped in y and not cropped at all: `v` runs 1 → 0.
    ///
    /// The flip is what makes cropping easy to get backwards. Keeping the half of a texture
    /// that holds content means taking `v` from 1 down to 0.5; taking 0 to 0.5 keeps the slack
    /// and cuts the content, which is what deriving this from an upside-down image does.
    #[test]
    fn showing_part_of_a_texture_crops_the_end_the_flip_put_the_content_on() {
        let whole = StageTexture::new(egui::TextureId::User(0), [100_u32, 100]).uv();
        assert_eq!(
            whole.min,
            egui::pos2(0.0, 1.0),
            "top-left of a flipped image"
        );
        assert_eq!(whole.max, egui::pos2(1.0, 0.0), "and its bottom-right");

        let half = StageTexture::new(egui::TextureId::User(0), [100_u32, 100])
            .showing([100_u32, 50])
            .uv();
        assert_eq!(half.min, egui::pos2(0.0, 1.0), "the top edge is unmoved");
        assert_eq!(
            half.max,
            egui::pos2(1.0, 0.5),
            "and the crop comes off the bottom, leaving the content"
        );
    }

    #[test]
    fn a_texture_shows_no_more_than_was_allocated_for_it() {
        let over =
            StageTexture::new(egui::TextureId::User(0), [64_u32, 64]).showing([999_u32, 999]);
        assert_eq!(over.shown(), [64, 64], "clamped to the allocation");
        assert_eq!(over.uv().max, egui::pos2(1.0, 0.0), "so nothing is cropped");
    }

    /// A 200×100 image at (10, 20) showing a 400×200 target, so a point maps to two pixels
    /// and a mistaken offset can't pass for a mistaken scale.
    const RECT: egui::Rect = egui::Rect {
        min: egui::pos2(10.0, 20.0),
        max: egui::pos2(210.0, 120.0),
    };
    const SIZE: [u32; 2] = [400, 200];

    /// Feed `events` to [`pointer`] a frame at a time and collect what each frame reported.
    fn seen(rect: egui::Rect, clip: egui::Rect, events: &[egui::Event]) -> Vec<Vec<Pointer>> {
        let id = egui::Id::new("offscreen-under-test");
        let out = std::cell::RefCell::new(Vec::new());
        {
            let mut harness = egui_kittest::Harness::new_ui(|ui| {
                let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(clip));
                ui.set_clip_rect(clip);
                out.borrow_mut().push(pointer(&ui, id, rect, SIZE));
            });
            for event in events {
                harness.input_mut().events.push(event.clone());
                harness.step();
            }
        }
        out.into_inner()
    }

    fn press(x: f32, y: f32, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos: egui::pos2(x, y),
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn a_press_maps_to_the_images_own_pixels() {
        let frames = seen(RECT, RECT, &[press(110.0, 70.0, true)]);
        // The rect's centre, so the target's centre whatever the two sizes are.
        assert_eq!(
            frames.last().expect("a frame ran"),
            &[Pointer::Down { x: 200.0, y: 100.0 }]
        );
    }

    #[test]
    fn a_scrolled_off_image_keeps_its_coordinates() {
        // The top half is clipped away, as a canvas scrolled halfway down it would.
        let clip = egui::Rect::from_min_max(egui::pos2(10.0, 70.0), egui::pos2(210.0, 120.0));
        let frames = seen(RECT, clip, &[press(110.0, 95.0, true)]);
        assert_eq!(
            frames.last().expect("a frame ran"),
            &[Pointer::Down { x: 200.0, y: 150.0 }],
            "measured from the whole image, not from where it starts being visible"
        );

        let above = seen(RECT, clip, &[press(110.0, 40.0, true)]);
        assert!(
            above.last().expect("a frame ran").is_empty(),
            "a press on the scrolled-off part is not a press on the image"
        );
    }

    #[test]
    fn a_drag_past_the_edge_keeps_reporting_and_still_releases() {
        let frames = seen(
            RECT,
            RECT,
            &[
                press(110.0, 70.0, true),
                egui::Event::PointerMoved(egui::pos2(400.0, 70.0)),
                press(400.0, 70.0, false),
            ],
        );
        let all: Vec<Pointer> = frames.into_iter().flatten().collect();
        assert!(
            matches!(all[0], Pointer::Down { .. }),
            "the press starts the capture"
        );
        assert_eq!(
            all[1],
            Pointer::Move { x: 780.0, y: 100.0 },
            "a move past the edge still reports, and reads past the extent"
        );
        assert!(
            matches!(all[2], Pointer::Up { .. }),
            "the release arrives even though it happened outside"
        );
    }

    /// A drag that began elsewhere crosses the image without owning it: its moves are reported,
    /// but press and release belong to whoever took them, so content is never left held.
    #[test]
    fn a_release_belongs_to_whoever_took_the_press() {
        let all: Vec<Pointer> = seen(
            RECT,
            RECT,
            &[
                press(400.0, 70.0, true),
                egui::Event::PointerMoved(egui::pos2(110.0, 70.0)),
                press(110.0, 70.0, false),
            ],
        )
        .into_iter()
        .flatten()
        .collect();

        assert!(
            !all.iter().any(|at| matches!(at, Pointer::Down { .. })),
            "the press landed elsewhere"
        );
        assert!(
            !all.iter().any(|at| matches!(at, Pointer::Up { .. })),
            "so its release is not ours to deliver"
        );
        assert_eq!(
            all,
            [Pointer::Move { x: 200.0, y: 100.0 }],
            "only the move, which did cross the image"
        );
    }

    /// Report `pointer`'s events and what it left of the scroll delta, one frame at a time.
    fn wheeled(rect: egui::Rect, at: egui::Pos2) -> Vec<(Vec<Pointer>, egui::Vec2)> {
        let id = egui::Id::new("offscreen-under-test");
        let out = std::cell::RefCell::new(Vec::new());
        {
            let mut harness = egui_kittest::Harness::new_ui(|ui| {
                let events = pointer(ui, id, rect, SIZE);
                let left = ui.input(|input| input.smooth_scroll_delta);
                out.borrow_mut().push((events, left));
            });
            for event in [
                egui::Event::PointerMoved(at),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, -12.0),
                    phase: egui::TouchPhase::Move,
                    modifiers: egui::Modifiers::NONE,
                },
            ] {
                harness.input_mut().events.push(event);
                harness.step();
            }
        }
        out.into_inner()
    }

    /// The double-scroll fix: the canvas must not also pan when content under the pointer scrolls.
    #[test]
    fn a_wheel_the_image_takes_is_left_for_no_one_else() {
        let taken = wheeled(RECT, egui::pos2(110.0, 70.0));
        assert!(
            taken.iter().any(|(events, _)| !events.is_empty()),
            "the wheel reached the image"
        );
        assert!(
            taken.iter().all(|(_, left)| left.y == 0.0),
            "and nothing of it was left for the canvas behind"
        );

        let elsewhere = wheeled(RECT, egui::pos2(400.0, 70.0));
        assert!(
            elsewhere.iter().all(|(events, _)| events.is_empty()),
            "a wheel away from the image is not the image's to take"
        );
        assert!(
            elsewhere.iter().any(|(_, left)| left.y != 0.0),
            "so the canvas still gets to scroll on it"
        );
    }

    #[test]
    fn a_wheel_over_the_image_is_reported_where_it_happened() {
        let frames = seen(
            RECT,
            RECT,
            &[
                egui::Event::PointerMoved(egui::pos2(110.0, 70.0)),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, -12.0),
                    phase: egui::TouchPhase::Move,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        let wheel = frames
            .into_iter()
            .flatten()
            .find(|at| matches!(at, Pointer::Wheel { .. }))
            .expect("the wheel reached the image");
        assert!(
            matches!(wheel, Pointer::Wheel { x, y, delta } if x == 200.0 && y == 100.0 && delta.y != 0.0)
        );
    }
}
