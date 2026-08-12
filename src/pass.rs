//! Offscreen wgpu rendering, the counterpart to [`offscreen`](crate::offscreen) for the glow
//! backend.
//!
//! Lets a scene draw into a colour texture and a depth buffer that gallery owns and shows inline.
//! The public entry point is [`SceneCtx::render_pass`](crate::SceneCtx::render_pass); this module
//! holds the cached [`PassTarget`], the [`ScenePass`] draw handle, and the wgpu deps the shell
//! wires in.
//!
//! It exists because egui's own render pass carries no depth attachment, so a solid drawn straight
//! into it through an [`egui_wgpu::Callback`] is sorted by submission order and nothing else. A
//! scene can record a pass of its own — that is all this does — but doing so per scene means
//! carrying a texture, a depth buffer, a blit and a resize rule, none of which is about the thing
//! being shown.

use std::collections::HashMap;

use eframe::egui_wgpu::{self, wgpu};

/// Each scene's cached wgpu render targets
/// ([`SceneCtx::render_pass`](crate::SceneCtx::render_pass)), keyed by scene identity —
/// one per call site, in the order the scene makes them.
pub(crate) type PassStore = HashMap<String, Vec<PassTarget>>;

/// A scene's cached target: the colour texture egui shows, and the depth buffer that is the whole
/// reason for having a pass of one's own. Registered with egui once and re-pointed on a resize,
/// since a `TextureId` cannot be freed.
pub(crate) struct PassTarget {
    color: wgpu::TextureView,
    depth: wgpu::TextureView,
    tex_id: egui::TextureId,
    size: [u32; 2],
}

impl PassTarget {
    /// Views alone come back: a `TextureView` holds its texture, so nothing is freed
    /// when the handle created here goes out of scope.
    fn planes(device: &wgpu::Device, size: [u32; 2]) -> (wgpu::TextureView, wgpu::TextureView) {
        let extent = wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        };
        let plane = |label, format, usage| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: extent,
                    mip_level_count: 1,
                    sample_count: ScenePass::SAMPLES,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        (
            plane(
                "gallery scene pass colour",
                ScenePass::FORMAT,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            ),
            plane(
                "gallery scene pass depth",
                ScenePass::DEPTH_FORMAT,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            ),
        )
    }

    fn new(device: &wgpu::Device, renderer: &mut egui_wgpu::Renderer, size: [u32; 2]) -> Self {
        let (color, depth) = Self::planes(device, size);
        // Nearest, so a texture shown at its own size is the pixels the scene drew
        // rather than a filtered version of them.
        let tex_id = renderer.register_native_texture(device, &color, wgpu::FilterMode::Nearest);
        Self {
            color,
            depth,
            tex_id,
            size,
        }
    }

    /// Reallocate to `size` and point the same `TextureId` at the new colour plane, so a scene
    /// that resizes its target does not leak an id egui has no way to release.
    fn resize(
        &mut self,
        device: &wgpu::Device,
        renderer: &mut egui_wgpu::Renderer,
        size: [u32; 2],
    ) {
        let (color, depth) = Self::planes(device, size);
        renderer.update_egui_texture_from_wgpu_texture(
            device,
            &color,
            wgpu::FilterMode::Nearest,
            self.tex_id,
        );
        self.color = color;
        self.depth = depth;
        self.size = size;
    }
}

/// What a scene draws with inside [`SceneCtx::render_pass`](crate::SceneCtx::render_pass):
/// a render pass gallery has already begun on the scene's own target, cleared and carrying depth.
///
/// A pipeline drawing through it is built against [`Self::FORMAT`], [`Self::SAMPLES`]
/// and [`Self::depth_state`] — stated here rather than guessed, since the target is gallery's
/// and a mismatch is a validation error rather than a wrong picture.
pub struct ScenePass<'a> {
    pass: wgpu::RenderPass<'static>,
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    size: [u32; 2],
}

impl ScenePass<'_> {
    /// The colour target a pipeline drawing here writes to.
    /// egui shows a user texture through a sampler of its own,
    /// and this is the format it documents for one.
    pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

    /// The depth buffer a pipeline drawing here tests against. Depth alone, no stencil.
    pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

    /// One sample: a target this size resolves nothing, and multisampling it would need a resolve
    /// attachment the scene never asked for. Distinct from [`MSAA_SAMPLES`](crate::MSAA_SAMPLES),
    /// which is egui's own pass and no business of this one.
    pub const SAMPLES: u32 = 1;

    /// The depth state a pipeline drawing here has to declare — every one of them,
    /// including a flat fill with nothing to sort. wgpu matches a pipeline against the
    /// attachments the pass actually carries, and this pass always carries depth,
    /// so leaving it out is a validation error rather than a pipeline that ignores it.
    ///
    /// `sorted` is whether the pipeline tests and writes depth: `true` for a solid,
    /// `false` for something covering every pixel once and wanting only to be let through.
    #[must_use]
    pub fn depth_state(sorted: bool) -> wgpu::DepthStencilState {
        wgpu::DepthStencilState {
            format: Self::DEPTH_FORMAT,
            depth_write_enabled: Some(sorted),
            depth_compare: Some(if sorted {
                wgpu::CompareFunction::Less
            } else {
                wgpu::CompareFunction::Always
            }),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }
    }

    /// The pass to draw into: set a pipeline, bind, draw. Cleared to nothing,
    /// with depth at the far plane and its viewport already covering the whole target.
    pub fn pass(&mut self) -> &mut wgpu::RenderPass<'static> {
        &mut self.pass
    }

    /// For building a pipeline, a buffer, a texture — whatever the scene caches between frames.
    pub fn device(&self) -> &wgpu::Device {
        self.device
    }

    /// For writing a buffer this frame's draw reads.
    pub fn queue(&self) -> &wgpu::Queue {
        self.queue
    }

    /// The target's size in pixels, as asked for — what an aspect ratio or a pixel-sized
    /// feature is measured against.
    pub fn size(&self) -> [u32; 2] {
        self.size
    }
}

/// The wgpu-backend handles a scene needs for rendering of its own: the render state the shell was
/// handed, and this scene's cached targets. Present only under [`Renderer::Wgpu`](crate::Renderer::Wgpu).
pub(crate) struct WgpuDeps<'a> {
    pub state: egui_wgpu::RenderState,
    pub targets: &'a mut Vec<PassTarget>,
}

impl WgpuDeps<'_> {
    /// Ensure the target for the call site at `at` matches `size` — creating it,
    /// or resizing it in place — then run `draw` against a pass begun on it, submit,
    /// and return the colour texture to show.
    ///
    /// `at` counts a scene's calls in the order it makes them, so each keeps its own texture
    /// and one image is never another's pixels. The store only grows: a call a scene stops
    /// making leaves its target behind and takes it up again if it returns.
    ///
    /// The pass goes on an encoder of gallery's own, submitted before this returns rather
    /// than recorded onto egui's — a scene draws in the middle of laying out a frame,
    /// long before egui has an encoder to lend, and the texture has to hold the result
    /// by the time egui paints it.
    pub(crate) fn render(
        &mut self,
        at: usize,
        size: [u32; 2],
        draw: impl FnOnce(&mut ScenePass<'_>),
    ) -> egui::TextureId {
        let (device, queue) = (&self.state.device, &self.state.queue);
        {
            let mut renderer = self.state.renderer.write();
            while self.targets.len() <= at {
                self.targets
                    .push(PassTarget::new(device, &mut renderer, size));
            }
            if self.targets[at].size != size {
                self.targets[at].resize(device, &mut renderer, size);
            }
        }
        let target = &self.targets[at];

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gallery scene pass"),
        });
        {
            let pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("gallery scene pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &target.color,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            // Nothing, so what the scene leaves undrawn shows the canvas
                            // behind the image rather than a colour it never chose.
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                        depth_slice: None,
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &target.depth,
                        depth_ops: Some(wgpu::Operations {
                            // The far plane, so the nearest surface at each pixel wins.
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Discard,
                        }),
                        stencil_ops: None,
                    }),
                    ..Default::default()
                })
                .forget_lifetime();
            let mut scene = ScenePass {
                pass,
                device,
                queue,
                size,
            };
            draw(&mut scene);
        }
        queue.submit(std::iter::once(encoder.finish()));
        target.tex_id
    }
}
