//! Headless capture on the glow backend, following glutin's `egl_device` example: a GL context off an
//! EGL device, with no window and no display server.
//!
//! egui_kittest ships a wgpu renderer only, so without this a `Renderer::Glow` scene would capture
//! `SceneCtx::offscreen`'s hint instead of the component.

use std::{cell::RefCell, rc::Rc, sync::Arc};

use eframe::{
    egui_glow,
    glow::{self, HasContext as _},
};
use glutin::{
    api::egl::{device::Device, display::Display},
    config::{Api, ConfigSurfaceTypes, ConfigTemplateBuilder},
    context::{ContextApi, ContextAttributesBuilder, NotCurrentGlContext as _},
    display::GlDisplay as _,
    surface::{PbufferSurface, SurfaceAttributesBuilder},
};

use crate::diagnostic::Diagnostic;

/// A glow renderer for [`egui_kittest`], holding the GL context it paints with.
///
/// The painter is shared with the app being captured, because a scene's `ctx.offscreen(...)`
/// texture must be registered with the very painter that later draws it.
pub(crate) struct GlowCapture {
    gl: Arc<glow::Context>,
    max_texture_dimension_2d: u32,
    loader: crate::GlLoader,
    painter: Rc<RefCell<egui_glow::Painter>>,
    target: Option<Framebuffer>,
    /// Kept alive, and current, for as long as anything paints: dropping it unbinds the context.
    _context: Context,
}

/// The context and everything it borrows from, in drop order.
/// Held but never read — it is current on this thread,
/// and dropping it unbinds the GL every other field here depends on.
struct Context {
    _current: glutin::api::egl::context::PossiblyCurrentContext,
    /// `None` when the driver took a surfaceless context, which most do.
    _surface: Option<glutin::api::egl::surface::Surface<PbufferSurface>>,
    _display: Display,
}

/// The process's stderr pointed at `/dev/null` until dropped.
///
/// Listing EGL devices makes Mesa (26.0.3 here) probe every GPU, and one it has no driver for,
/// such as an NVIDIA card, logs `pci id for fd N: 10de:…, driver (null)` per string queried.
/// That goes through its loader's default logger (`QueryDeviceStringEXT` →
/// `dri_get_drm_device_info`), before any display exists to set a quieter one, so neither
/// `EGL_LOG_LEVEL` nor anything else reaches it. What else is written meanwhile waits,
/// if it is through the lock of this copy of std, or is lost too, as a panic's message is.
struct MutedStderr {
    #[cfg(unix)]
    saved: Option<std::os::fd::OwnedFd>,
    /// One at a time: a second would save the first one's `/dev/null` and bring that back.
    #[cfg(unix)]
    _alone: std::io::StderrLock<'static>,
}

impl MutedStderr {
    /// Muted where it can be; a failure leaves stderr as it was, noise and all.
    fn new() -> Self {
        #[cfg(unix)]
        {
            use std::os::fd::AsFd as _;
            let alone = std::io::stderr().lock();
            let saved = (|| {
                let null = std::fs::File::options()
                    .write(true)
                    .open("/dev/null")
                    .ok()?;
                let saved = rustix::io::dup(alone.as_fd()).ok()?;
                rustix::stdio::dup2_stderr(null.as_fd()).ok()?;
                Some(saved)
            })();
            Self {
                saved,
                _alone: alone,
            }
        }
        #[cfg(not(unix))]
        Self {}
    }
}

impl Drop for MutedStderr {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(saved) = self.saved.take() {
            // Nothing to report a failure to: stderr is what failed to come back.
            _ = rustix::stdio::dup2_stderr(&saved);
        }
    }
}

impl GlowCapture {
    /// Build a GL context off the first EGL device that yields one, and a painter over it.
    ///
    /// # Errors
    /// When no device produces a usable context — no EGL at all, no device extensions,
    /// or a driver that refuses every config. The message lists what was tried.
    pub(crate) fn new() -> Result<Self, Diagnostic> {
        let devices: Vec<Device> = {
            let _muted = MutedStderr::new();
            Device::query_devices()
                .map_err(|e| {
                    Diagnostic::new(format!("no EGL devices to render on: {e}")).hint(
                        "headless glow capture needs EGL — install a mesa/EGL driver, \
                         or configure `Renderer::Wgpu`",
                    )
                })?
                .collect()
        };

        // Two machines only agree on a rendered image if they rasterise it the same way, so the
        // reference tests pin a software renderer through this (`nix/test.nix`).
        let wanted = std::env::var("GALLERY_CAPTURE_RENDERER").ok();
        let mut tried = Vec::new();
        for device in &devices {
            let name = device.name().unwrap_or("<unnamed device>").to_owned();
            match Self::on_device(device, wanted.as_deref()) {
                Ok(capture) => return Ok(capture),
                Err(reason) => tried.push(format!("{name} — {reason}")),
            }
        }
        Err(
            Diagnostic::new("no EGL device could provide a headless GL context")
                .candidates(tried)
                .hint("configure `Renderer::Wgpu`, whose capture needs no GL context"),
        )
    }

    /// The painter this capture draws through, for the app to register textures with.
    pub(crate) fn painter(&self) -> Rc<RefCell<egui_glow::Painter>> {
        self.painter.clone()
    }

    fn on_device(device: &Device, wanted: Option<&str>) -> Result<Self, String> {
        let display = Self::open(device)?;
        let config = Self::config(&display, ConfigSurfaceTypes::empty())?;
        let context = Self::context(&display, &config)?;

        // Surfaceless where the driver allows it. glutin never checks the extension,
        // so a refusal only shows up here as an error — hence the pbuffer.
        let (context, surface) = match context.make_current_surfaceless() {
            Ok(current) => (current, None),
            Err(surfaceless) => {
                let (context, surface) = Self::with_pbuffer(&display)
                    .map_err(|pbuffer| format!("surfaceless: {surfaceless}; pbuffer: {pbuffer}"))?;
                (context, Some(surface))
            }
        };

        let loader: crate::GlLoader = {
            let display = display.clone();
            Arc::new(move |symbol: &std::ffi::CStr| display.get_proc_address(symbol))
        };
        // SAFETY: the context is current on this thread, and `loader` resolves against its display.
        let gl =
            Arc::new(unsafe { glow::Context::from_loader_function_cstr(|symbol| loader(symbol)) });
        // SAFETY: the context is current on this thread.
        // This is the ceiling both uploaded panel textures
        // and the sheet framebuffer have to stay under.
        let max_texture_dimension_2d = unsafe { gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE) };
        let max_texture_dimension_2d = u32::try_from(max_texture_dimension_2d)
            .ok()
            .filter(|limit| *limit > 0)
            .ok_or_else(|| "GL_MAX_TEXTURE_SIZE did not report a positive limit".to_owned())?;
        // Before the painter, which compiles shaders a rejected device would only throw away.
        if let Some(wanted) = wanted {
            // SAFETY: the context is current on this thread.
            let renderer = unsafe { gl.get_parameter_string(glow::RENDERER) };
            if !renderer.to_lowercase().contains(&wanted.to_lowercase()) {
                return Err(format!("renders with {renderer}"));
            }
        }
        let painter = egui_glow::Painter::new(gl.clone(), "", None, false)
            .map_err(|e| format!("painter: {e}"))?;

        Ok(Self {
            gl,
            max_texture_dimension_2d,
            loader,
            painter: Rc::new(RefCell::new(painter)),
            target: None,
            _context: Context {
                _current: context,
                _surface: surface,
                _display: display,
            },
        })
    }

    /// A display over one EGL device — the platform that needs no window system.
    fn open(device: &Device) -> Result<Display, String> {
        // SAFETY: `device` came from `query_devices`, which is what this call wants.
        unsafe { Display::with_device(device, None) }.map_err(|e| format!("display: {e}"))
    }

    /// A config for offscreen work, admitting the given surface types.
    ///
    /// The builder defaults to `WINDOW`, which a device display has no configs for — and EGL reports
    /// "matched nothing" as success, so that mistake reads as a bare absence rather than an error.
    fn config(
        display: &Display,
        surfaces: ConfigSurfaceTypes,
    ) -> Result<glutin::api::egl::config::Config, String> {
        let template = ConfigTemplateBuilder::new()
            .with_surface_type(surfaces)
            .with_api(Api::OPENGL | Api::GLES2)
            .with_depth_size(0)
            .with_stencil_size(0)
            .build();
        // SAFETY: the template is ours and the display is live.
        unsafe { display.find_configs(template) }
            .map_err(|e| format!("configs: {e}"))?
            .next()
            .ok_or_else(|| "no config supports offscreen rendering".to_owned())
    }

    /// eframe's own two-step (`glow_integration`): desktop GL, falling back to GLES,
    /// so a scene sees the same GL headlessly as it does in a window.
    fn context(
        display: &Display,
        config: &glutin::api::egl::config::Config,
    ) -> Result<glutin::api::egl::context::NotCurrentContext, String> {
        let attributes = ContextAttributesBuilder::new().build(None);
        let fallback = ContextAttributesBuilder::new()
            .with_context_api(ContextApi::Gles(None))
            .build(None);
        // SAFETY: both attribute sets name no window, which is what a device display supports.
        unsafe { display.create_context(config, &attributes) }
            .or_else(|_| unsafe { display.create_context(config, &fallback) })
            .map_err(|e| format!("context: {e}"))
    }

    /// A 1×1 pbuffer to hang the context on, for drivers that won't go surfaceless. Nothing is drawn
    /// into it, so its size never matters — but it needs a config of its own, the surfaceless one
    /// having been matched against no surface type at all.
    fn with_pbuffer(
        display: &Display,
    ) -> Result<
        (
            glutin::api::egl::context::PossiblyCurrentContext,
            glutin::api::egl::surface::Surface<PbufferSurface>,
        ),
        String,
    > {
        let config = Self::config(display, ConfigSurfaceTypes::PBUFFER)?;
        let one = std::num::NonZeroU32::new(1).expect("1 is not zero");
        let attributes = SurfaceAttributesBuilder::<PbufferSurface>::new().build(one, one);
        // SAFETY: the config came from this display, and the attributes name a 1×1 offscreen surface.
        let surface = unsafe { display.create_pbuffer_surface(&config, &attributes) }
            .map_err(|e| format!("surface: {e}"))?;
        let context = Self::context(display, &config)?
            .make_current(&surface)
            .map_err(|e| format!("make current: {e}"))?;
        Ok((context, surface))
    }
}

/// One [`GlowCapture`] for a whole run, handed to each shot's harness in turn.
///
/// A shot builds its own harness, and the harness owns the renderer it was given,
/// so a capture made per shot takes its GL context down with it.
/// Scenes are encouraged to cache against that context — building a femtovg canvas
/// recompiles its shaders — and that cache lives in a thread-local in the scenes dylib,
/// which outlives every harness.
///
/// Each shot after the first would then draw with objects belonging to a dead context,
/// and come back blank. Nothing a scene could soundly test for, either:
/// a fresh context reissues GL names from 1, and a freed handle's address is commonly reused.
#[derive(Clone)]
pub(crate) struct SharedCapture {
    // Declared first, so a painter of its own
    // is freed while its context is still there.
    painter: Painting,
    capture: Rc<RefCell<GlowCapture>>,
}

/// Which painter a [`SharedCapture`] draws through.
#[derive(Clone)]
enum Painting {
    /// The run's, which a scene registers
    /// its offscreen textures with.
    Shared,
    Own(Rc<OwnPainter>),
}

/// A painter of [`SharedCapture::with_own_painter`]'s,
/// freeing its GL objects as it goes.
struct OwnPainter(RefCell<egui_glow::Painter>);

impl Drop for OwnPainter {
    fn drop(&mut self) {
        self.0.get_mut().destroy();
    }
}

impl SharedCapture {
    /// # Errors
    /// As [`GlowCapture::new`] — and once per run rather than once per shot.
    pub(crate) fn new() -> Result<Self, Diagnostic> {
        Ok(Self {
            painter: Painting::Shared,
            capture: Rc::new(RefCell::new(GlowCapture::new()?)),
        })
    }

    /// The same context, through a painter of its own: for an app
    /// drawn alongside a shot, whose textures would otherwise replace
    /// the shot's under the same ids, its font atlas first among them.
    ///
    /// # Errors
    /// When the painter's shaders do not compile.
    pub(crate) fn with_own_painter(&self) -> Result<Self, Diagnostic> {
        let gl = self.capture.borrow().gl.clone();
        let painter = egui_glow::Painter::new(gl, "", None, false)
            .map_err(|e| Diagnostic::new(format!("open a second glow painter: {e}")))?;
        Ok(Self {
            painter: Painting::Own(Rc::new(OwnPainter(RefCell::new(painter)))),
            capture: self.capture.clone(),
        })
    }

    pub(crate) fn painter(&self) -> Rc<RefCell<egui_glow::Painter>> {
        self.capture.borrow().painter()
    }

    /// The current context's `GL_MAX_TEXTURE_SIZE`.
    pub(crate) fn max_texture_dimension_2d(&self) -> u32 {
        self.capture.borrow().max_texture_dimension_2d
    }

    fn with_painter<T>(&self, then: impl FnOnce(&mut egui_glow::Painter) -> T) -> T {
        match &self.painter {
            Painting::Shared => {
                let painter = self.capture.borrow().painter();
                then(&mut painter.borrow_mut())
            }
            Painting::Own(own) => then(&mut own.0.borrow_mut()),
        }
    }
}

impl egui_kittest::TestRenderer for SharedCapture {
    /// Hand the app the GL a scene draws with. Both fields are `pub` on `CreationContext`, unlike
    /// their counterparts on `Frame` — see `offscreen::RegisterTexture` for what that costs.
    fn setup_eframe(&self, cc: &mut eframe::CreationContext<'_>, _frame: &mut eframe::Frame) {
        let capture = self.capture.borrow();
        cc.gl = Some(capture.gl.clone());
        cc.get_proc_address = Some(capture.loader.clone());
    }

    /// Upload what egui added or changed, then empty the delta — epaint 0.36 asserts on drop
    /// that every entry was consumed. The frees are dropped rather than applied: this runs
    /// before `render`, whereas `Painter::paint_and_update_textures` frees after painting —
    /// the output still names them.
    fn handle_delta(&mut self, delta: &mut egui::TexturesDelta) {
        self.with_painter(|painter| {
            for (id, images) in delta.set.drain() {
                for image in images {
                    painter.set_texture(id, &image);
                }
            }
        });
        delta.free.clear();
    }

    fn render(
        &mut self,
        ctx: &egui::Context,
        output: &egui::FullOutput,
    ) -> Result<image::RgbaImage, String> {
        let capture = self.capture.clone();
        self.with_painter(|painter| capture.borrow_mut().render(painter, ctx, output))
    }
}

impl GlowCapture {
    fn render(
        &mut self,
        painter: &mut egui_glow::Painter,
        ctx: &egui::Context,
        output: &egui::FullOutput,
    ) -> Result<image::RgbaImage, String> {
        let pixels_per_point = ctx.pixels_per_point();
        let size = ctx.content_rect().size() * pixels_per_point;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a canvas is small and positive"
        )]
        let size_px = [size.x.round() as u32, size.y.round() as u32];
        let tessellated = ctx.tessellate(output.shapes.clone(), pixels_per_point);

        // SAFETY (both calls): `self.gl` is the context this renderer made current.
        if let Some(stale) = self.target.take_if(|held| held.size != size_px) {
            unsafe { stale.destroy(&self.gl) };
        }
        let framebuffer = match &self.target {
            Some(kept) => kept,
            None => self
                .target
                .insert(unsafe { Framebuffer::new(&self.gl, size_px)? }),
        };

        // SAFETY (every call here): the context is current on this thread and owns `framebuffer`.
        unsafe {
            self.gl
                .bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer.fbo));
        }
        // Transparent, matching what the wgpu renderer clears to.
        egui_glow::painter::clear(&self.gl, size_px, [0.0, 0.0, 0.0, 0.0]);
        painter.paint_primitives(size_px, pixels_per_point, &tessellated);
        // Drawing has to land before the read below can see it.
        unsafe { self.gl.finish() };

        let image = painter.read_screen_rgba(size_px);
        // Unbound again, leaving the context as the next frame expects to find it.
        unsafe { self.gl.bind_framebuffer(glow::FRAMEBUFFER, None) };

        let pixels = image
            .pixels
            .iter()
            .flat_map(|pixel| pixel.to_array())
            .collect();
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the image is the canvas, which is small"
        )]
        let (width, height) = (image.width() as u32, image.height() as u32);
        image::RgbaImage::from_raw(width, height, pixels)
            .ok_or_else(|| "the rendered pixels do not fill the image".to_owned())
    }
}

impl Drop for GlowCapture {
    fn drop(&mut self) {
        // The painter's GL objects belong to a context that is about to go, and it wants telling.
        self.painter.borrow_mut().destroy();
        if let Some(target) = self.target.take() {
            // SAFETY: the context is still current — `_context` drops after this.
            unsafe { target.destroy(&self.gl) };
        }
    }
}

/// The framebuffer a capture paints into. Colour only: egui needs no depth
/// or stencil, and a scene's own [`crate::Offscreen`] target carries its own.
struct Framebuffer {
    fbo: glow::NativeFramebuffer,
    texture: glow::NativeTexture,
    size: [u32; 2],
}

impl Framebuffer {
    /// # Safety
    /// `gl` must be the live, current context.
    #[expect(
        clippy::cast_possible_wrap,
        reason = "small, non-negative pixel dimensions"
    )]
    unsafe fn new(gl: &glow::Context, size: [u32; 2]) -> Result<Self, String> {
        // SAFETY: standard offscreen-FBO setup against the live context (fn contract).
        unsafe {
            let texture = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                size[0] as i32,
                size[1] as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
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
            let fbo = gl.create_framebuffer()?;
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(texture),
                0,
            );
            let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.bind_texture(glow::TEXTURE_2D, None);
            if status != glow::FRAMEBUFFER_COMPLETE {
                return Err(format!(
                    "a {}×{} capture framebuffer is not complete (GL status {status:#x})",
                    size[0], size[1]
                ));
            }
            Ok(Self { fbo, texture, size })
        }
    }

    /// # Safety
    /// `gl` must be the live, current context that made these.
    unsafe fn destroy(&self, gl: &glow::Context) {
        // SAFETY: both names came from this context (fn contract).
        unsafe {
            gl.delete_framebuffer(self.fbo);
            gl.delete_texture(self.texture);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn threads_that_mute_stderr_together_leave_it_as_it_was() {
        use std::{
            os::{fd::AsFd as _, unix::fs::MetadataExt as _},
            sync::Barrier,
        };
        // Locked, as another test may have it muted.
        let copy = || std::io::stderr().lock().as_fd().try_clone_to_owned();
        let identity = || {
            let stderr = std::fs::File::from(copy().expect("a copy"));
            let stderr = stderr.metadata().expect("its metadata");
            (stderr.dev(), stderr.ino())
        };
        let saved = copy().expect("a copy");
        let before = identity();
        let barrier = Barrier::new(8);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    for _ in 0..200 {
                        barrier.wait();
                        drop(MutedStderr::new());
                    }
                });
            }
        });
        let _alone = std::io::stderr().lock();
        let after = identity();
        rustix::stdio::dup2_stderr(&saved).expect("restored for the other tests");
        assert_eq!(after, before);
    }

    #[cfg(unix)]
    #[test]
    fn what_another_thread_writes_waits_until_stderr_is_back() {
        use std::{io::Write as _, sync::mpsc, time::Duration};
        let muted = MutedStderr::new();
        let (wrote, written) = mpsc::channel();
        std::thread::spawn(move || {
            _ = std::io::stderr().write_all(b"");
            wrote.send(()).expect("heard");
        });
        let early = written.recv_timeout(Duration::from_millis(200));
        drop(muted);
        assert!(early.is_err(), "it would be lost");
        written.recv().expect("it is written then");
    }

    /// Clear the bound framebuffer to a known colour and read one pixel back
    /// — proof that the context under test can actually draw,
    /// not merely that it was created.
    ///
    /// # Safety
    /// `gl` must be the live, current context.
    unsafe fn draws(gl: &glow::Context) -> [u8; 4] {
        let framebuffer = unsafe { Framebuffer::new(gl, [4, 4]) }.expect("a 4×4 framebuffer");
        let mut pixel = [0_u8; 4];
        // SAFETY: the framebuffer is complete and belongs to this context.
        unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer.fbo));
            gl.viewport(0, 0, 4, 4);
            gl.clear_color(0.25, 0.5, 0.75, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            gl.finish();
            gl.read_pixels(
                0,
                0,
                1,
                1,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelPackData::Slice(Some(&mut pixel)),
            );
            framebuffer.destroy(gl);
        }
        pixel
    }

    /// `0.25/0.5/0.75` in eight bits, give or take the rounding a driver chooses.
    fn is_the_cleared_colour(pixel: [u8; 4]) -> bool {
        let near = |got: u8, want: u8| got.abs_diff(want) <= 1;
        near(pixel[0], 64) && near(pixel[1], 128) && near(pixel[2], 191) && pixel[3] == 255
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn stderr_is_muted_while_devices_are_listed_and_given_back_after() {
        use std::os::unix::fs::MetadataExt as _;
        // Throughout, as another test may mute it meanwhile; a thread may lock it again.
        let _alone = std::io::stderr().lock();
        let target = || {
            let meta = std::fs::metadata("/proc/self/fd/2").expect("fd 2 is open");
            (meta.dev(), meta.ino())
        };
        let null = std::fs::metadata("/dev/null").expect("/dev/null");
        let before = target();
        {
            let _muted = MutedStderr::new();
            assert_eq!(target(), (null.dev(), null.ino()), "muted onto /dev/null");
        }
        assert_eq!(target(), before, "the same stderr as before");
    }

    #[test]
    fn a_headless_context_comes_off_an_egl_device_and_draws() {
        let capture = GlowCapture::new().expect("an EGL device offers a headless GL context");
        assert_eq!(
            capture.max_texture_dimension_2d,
            // SAFETY: `new` left its context current on this thread.
            u32::try_from(unsafe { capture.gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE) })
                .expect("a positive GL texture limit"),
            "the capture carries this context's GL_MAX_TEXTURE_SIZE"
        );
        // SAFETY: `new` left its context current on this thread.
        assert!(
            is_the_cleared_colour(unsafe { draws(&capture.gl) }),
            "the context renders and reads back"
        );
    }

    /// The fallback for drivers that refuse a surfaceless context.
    /// No driver here refuses, so the binding is exercised directly
    /// — otherwise the branch would only ever run on someone else's
    /// machine, which is where a rotted one would be found.
    #[test]
    fn a_pbuffer_binds_a_context_for_drivers_that_refuse_surfaceless() {
        let devices: Vec<Device> = Device::query_devices().expect("EGL devices").collect();
        let mut tried = Vec::new();
        for device in &devices {
            let Ok(display) = GlowCapture::open(device) else {
                continue;
            };
            match GlowCapture::with_pbuffer(&display) {
                Ok((_context, _surface)) => {
                    let loader = |symbol: &std::ffi::CStr| display.get_proc_address(symbol);
                    // SAFETY: `with_pbuffer` made the context current on this thread.
                    let gl = unsafe { glow::Context::from_loader_function_cstr(loader) };
                    assert!(
                        is_the_cleared_colour(unsafe { draws(&gl) }),
                        "a pbuffer-bound context renders and reads back"
                    );
                    return;
                }
                Err(reason) => tried.push(reason),
            }
        }
        panic!("no EGL device could back a pbuffer: {}", tried.join("; "));
    }
}
