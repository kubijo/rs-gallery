//! The renderer a capture draws with: a glow or wgpu harness, opened once per run.

use super::*;

/// The painter a glow capture draws with, which is the one a scene has to register
/// its offscreen texture on. `None` under wgpu, whose capture needs no GL context.
pub(crate) type GlPainter = std::rc::Rc<std::cell::RefCell<egui_glow::Painter>>;

/// A harness drawing `app` headlessly at `size` points, `scale` device pixels to the point,
/// on the renderer the consumer configured.
///
/// The renderer is set before the app is built: the harness runs `setup_eframe` first,
/// and that is what leaves a GL context on the `CreationContext` the closure reads.
/// The painter comes out alongside it: a scene registers its offscreen texture
/// with the very one that draws it.
///
/// # Errors
/// On a platform with no headless glow capture, when glow is what was configured.
pub(crate) fn open<A: eframe::App + 'static>(
    size: egui::Vec2,
    scale: f32,
    session: &Session,
    setup: &impl Fn(&egui::Context),
    app: impl FnOnce(&eframe::CreationContext<'_>, Option<GlPainter>) -> A,
) -> Result<egui_kittest::Harness<'static, A>, Diagnostic> {
    session.validate_target(size, scale)?;
    let builder = builder(size, scale);
    let (painter, builder) = match session {
        #[cfg(not(target_vendor = "apple"))]
        Session::Glow(capture) => (Some(capture.painter()), builder.renderer(capture.clone())),
        // Eager, unlike the default lazy renderer, which builds its wgpu state on the first
        // `render()` — by which time the app has been drawing for frames. Built up front,
        // the state reaches the `CreationContext`, so `SceneCtx::render_state` is `Some`
        // here exactly as it is in a window. Same render options either way.
        Session::Wgpu(capture) => (None, builder.renderer(capture.renderer()?)),
    };
    Ok(builder.build_eframe(|cc| {
        install_context(cc, setup);
        app(cc, painter)
    }))
}

/// [`open`] for an app drawn alongside a shot that shares nothing with it:
/// under glow, through a painter of its own, so its textures cannot replace the shot's.
/// A wgpu harness already has a renderer of its own.
///
/// # Errors
/// As [`open`], or when glow's second painter cannot be made.
pub(crate) fn open_apart<A: eframe::App + 'static>(
    size: egui::Vec2,
    scale: f32,
    session: &Session,
    app: impl FnOnce(&eframe::CreationContext<'_>) -> A,
) -> Result<egui_kittest::Harness<'static, A>, Diagnostic> {
    session.validate_target(size, scale)?;
    let builder = builder(size, scale);
    let builder = match session {
        #[cfg(not(target_vendor = "apple"))]
        Session::Glow(capture) => builder.renderer(capture.with_own_painter()?),
        Session::Wgpu(capture) => builder.renderer(capture.renderer()?),
    };
    Ok(builder.build_eframe(|cc| app(cc)))
}

fn builder<A>(size: egui::Vec2, scale: f32) -> egui_kittest::HarnessBuilder<A> {
    egui_kittest::Harness::builder()
        .with_size(size)
        .with_pixels_per_point(scale)
        // The predictable options a snapshot wants
        //  - software texture filtering
        //  - no dithering
        //
        // …with the sample count spelled from the one place the window reads it too.
        .with_render_options(wgpu_render_options())
}

/// The GL a run paints through, opened once and lent to every shot.
///
/// A type rather than a convention, because [`SharedCapture`] is only correct
/// when one of it covers the whole run — its own docs have why.
pub(crate) enum Session {
    #[cfg(not(target_vendor = "apple"))]
    Glow(SharedCapture),
    Wgpu(Box<WgpuCapture>),
}

/// One actual headless wgpu device for a run. Each harness needs a fresh egui renderer, but all of
/// them are built over this same device so the capability used by sheet packing is authoritative.
pub(crate) struct WgpuCapture {
    setup: eframe::egui_wgpu::WgpuSetup,
    first: RefCell<Option<eframe::egui_wgpu::RenderState>>,
    max_texture_dimension_2d: u32,
}

fn wgpu_render_options() -> eframe::egui_wgpu::RendererOptions {
    eframe::egui_wgpu::RendererOptions {
        msaa_samples: crate::MSAA_SAMPLES,
        ..eframe::egui_wgpu::RendererOptions::PREDICTABLE
    }
}

impl WgpuCapture {
    fn new() -> Result<Self, Diagnostic> {
        let initial_setup = egui_kittest::wgpu::default_wgpu_setup();
        let state = make_wgpu_state(initial_setup, "open the headless wgpu renderer")?;
        let max_texture_dimension_2d = state.device.limits().max_texture_dimension_2d;
        let setup = eframe::egui_wgpu::WgpuSetup::Existing(eframe::egui_wgpu::WgpuSetupExisting {
            instance: state.instance.clone(),
            adapter: state.adapter.clone(),
            device: state.device.clone(),
            queue: state.queue.clone(),
        });
        Ok(Self {
            setup,
            first: RefCell::new(Some(state)),
            max_texture_dimension_2d,
        })
    }

    fn renderer(&self) -> Result<egui_kittest::wgpu::WgpuTestRenderer, Diagnostic> {
        let state = match self.first.borrow_mut().take() {
            Some(state) => state,
            None => make_wgpu_state(self.setup.clone(), "open a headless wgpu capture")?,
        };
        Ok(egui_kittest::wgpu::WgpuTestRenderer::from_render_state(
            state,
        ))
    }
}

fn make_wgpu_state(
    setup: eframe::egui_wgpu::WgpuSetup,
    action: &str,
) -> Result<eframe::egui_wgpu::RenderState, Diagnostic> {
    std::panic::catch_unwind(AssertUnwindSafe(|| {
        egui_kittest::wgpu::create_render_state(setup, wgpu_render_options())
    }))
    .map_err(|panic| Diagnostic::new(format!("{action}: {}", panic_message(&panic))))
}

pub(crate) fn panic_message(panic: &Box<dyn std::any::Any + Send>) -> String {
    panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&str>().copied())
        .unwrap_or("the renderer panicked")
        .to_owned()
}

impl Session {
    /// # Errors
    /// When glow is configured and the platform has no headless GL to give.
    pub(crate) fn open(renderer: Renderer) -> Result<Self, Diagnostic> {
        match renderer {
            #[cfg(not(target_vendor = "apple"))]
            Renderer::Glow => Ok(Self::Glow(SharedCapture::new()?)),
            #[cfg(target_vendor = "apple")]
            Renderer::Glow => Err(
                Diagnostic::new("this platform has no headless glow capture")
                    .hint("configure `Renderer::Wgpu`, whose capture needs no GL context"),
            ),
            Renderer::Wgpu => Ok(Self::Wgpu(Box::new(WgpuCapture::new()?))),
        }
    }

    /// What a reference taken under this session is named for.
    pub(crate) fn renderer_name(&self) -> &'static str {
        match self {
            #[cfg(not(target_vendor = "apple"))]
            Self::Glow(_) => "glow",
            Self::Wgpu(_) => "wgpu",
        }
    }

    /// The largest 2D texture/render-target side enabled on this run's actual backend.
    pub(crate) fn max_texture_dimension_2d(&self) -> u32 {
        match self {
            #[cfg(not(target_vendor = "apple"))]
            Self::Glow(capture) => capture.max_texture_dimension_2d(),
            Self::Wgpu(capture) => capture.max_texture_dimension_2d,
        }
    }

    pub(super) fn validate_target(&self, size: egui::Vec2, scale: f32) -> Result<(), Diagnostic> {
        let pixels = size * scale;
        let width = pixels.x.round();
        let height = pixels.y.round();
        let limit = self.max_texture_dimension_2d();
        if width.is_finite()
            && height.is_finite()
            && width >= 1.0
            && height >= 1.0
            && width <= limit as f32
            && height <= limit as f32
        {
            return Ok(());
        }
        Err(Diagnostic::new(format!(
            "a {width:.0}×{height:.0} capture target exceeds this renderer's {limit}×{limit} limit"
        )))
    }
}
