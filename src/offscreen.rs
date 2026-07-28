//! Offscreen GL rendering for the glow backend.
//!
//! Lets a scene paint non-egui content — femtovg, raw glow — into a framebuffer that gallery owns and
//! shows inline. The public entry point is [`SceneCtx::offscreen`](crate::SceneCtx::offscreen); this
//! module holds the cached [`RenderTarget`], the [`Offscreen`] draw handle, and the glow deps the shell
//! wires in.

use std::collections::HashMap;

use eframe::glow::{self, HasContext};

/// Each scene's cached offscreen render target ([`SceneCtx::offscreen`](crate::SceneCtx::offscreen)),
/// keyed by scene identity.
pub(crate) type TargetStore = HashMap<String, Option<RenderTarget>>;

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

/// Hands a GL texture to egui and gets back an id to draw it by.
///
/// A closure rather than the `eframe::Frame` that backs it in a window, because the headless capture
/// has no usable one: `Frame::_new_kittest` leaves the hook `None`, and the field is `pub(crate)`.
/// There, `egui_glow::Painter::register_native_texture` — the same call eframe wraps — stands in.
///
/// Owned rather than borrowed, because `&'a mut (dyn FnMut + 'a)` puts `'a` in an invariant position.
/// [`GlDeps`] has to stay covariant for a caller to hand it to a shorter-lived [`crate::SceneCtx`].
pub(crate) type RegisterTexture<'a> = Box<dyn FnMut(glow::NativeTexture) -> egui::TextureId + 'a>;

/// The glow-backend handles a scene needs for non-egui rendering — the loader, gallery's own glow
/// context (for FBO bookkeeping), a way to register a texture with egui, and this scene's cached
/// target. Present only under [`Renderer::Glow`](crate::Renderer::Glow).
pub(crate) struct GlDeps<'a> {
    pub loader: crate::GlLoader,
    pub gl: &'a glow::Context,
    pub register: RegisterTexture<'a>,
    pub target: &'a mut Option<RenderTarget>,
}

impl GlDeps<'_> {
    /// Ensure the cached target matches `size` — creating it, or resizing it in place — then bind it,
    /// clear it, run `draw`, and put back the framebuffer that was bound, returning the colour texture
    /// to show. That attachment is bottom-left origin, so the caller flips V when displaying it.
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
        size: [u32; 2],
        draw: impl FnOnce(&Offscreen),
    ) -> egui::TextureId {
        // SAFETY (every block below): `self.gl` is the live glow context handed in by the shell.
        match self.target.as_mut() {
            Some(target) if target.size != size => unsafe { target.resize(self.gl, size) },
            Some(_) => {}
            None => {
                let target = unsafe { RenderTarget::create(self.gl, &mut *self.register, size) };
                *self.target = Some(target);
            }
        }
        let target = self.target.as_ref().expect("just ensured present");
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
