//! Scene content drawn by wgpu rather than by egui, through the render state
//! [`SceneCtx::render_state`] hands out — and the edges of what that allows.
//!
//! Each scene builds nothing of its own: the file's pipelines and buffers go into one
//! `callback_resources` entry, built on the first draw of whichever scene is opened first.
//! Entries are keyed by type and the scenes dylib declares this one, so a `--hot` rebuild
//! looks its own [`Shaders`] up and rebuilds when it does not find it.
//!
//! `gradient` is the one to read first; the rest each stand on something the plain case
//! never meets — device pixels, a scissored rect, egui's own colours, a clock, a depth buffer.

use gallery::egui_wgpu::{self, CallbackResources, CallbackTrait, ScreenDescriptor, wgpu};
use gallery::prelude::*;

scene_meta! { title: "Wgpu" }

/// `gradient`'s colour defaults. The ramp touches each exactly once, at its own edge,
/// so they are named rather than spelled inline — the pixels there are worth asserting on.
pub const TOP: egui::Color32 = egui::Color32::from_rgb(0, 173, 181);
pub const BOTTOM: egui::Color32 = egui::Color32::from_rgb(26, 43, 109);

// --- The shaders ---

/// The vertex half every fullscreen shader here shares: one triangle reaching past every edge
/// of the viewport, which egui-wgpu has set to the callback's rect.
/// `uv` runs from 0 at that rect's top-left corner to 1 at its bottom-right.
const FULLSCREEN: &str = r"
struct Corner {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> Corner {
    var corners = array<vec2f, 3>(vec2f(-1.0, -1.0), vec2f(3.0, -1.0), vec2f(-1.0, 3.0));
    let corner = corners[index];
    var out: Corner;
    out.position = vec4f(corner, 0.0, 1.0);
    // Clip space points up and a scene reads top-down, so y flips.
    out.uv = vec2f(corner.x, -corner.y) * 0.5 + 0.5;
    return out;
}
";

/// `bias` bends the ramp rather than clipping it: `pow` leaves both ends where they are,
/// so the gradient spans the whole height whatever it is set to.
const GRADIENT: &str = r"
struct Params {
    top: vec4f,
    bottom: vec4f,
    bias: f32,
}

@group(0) @binding(0) var<uniform> params: Params;

@fragment
fn fs_main(in: Corner) -> @location(0) vec4f {
    let blend = pow(clamp(in.uv.y, 0.0, 1.0), params.bias);
    return mix(params.top, params.bottom, blend);
}
";

/// Everything here is measured in device pixels, so the ring stays round in a rect of any
/// shape and the rules stay one pixel wide however large the drawing is asked to be.
const PIXELS: &str = r"
struct Params {
    size: vec2f,
    radius: f32,
    pitch: f32,
}

@group(0) @binding(0) var<uniform> params: Params;

@fragment
fn fs_main(in: Corner) -> @location(0) vec4f {
    let at = in.uv * params.size;
    let paper = vec3f(0.09, 0.09, 0.10);
    let ink = vec3f(0.42, 0.61, 0.85);
    let rule = f32(min(at.x % params.pitch, at.y % params.pitch) < 1.0);
    let edge = abs(length(at - params.size * 0.5) - params.radius);
    let ring = 1.0 - smoothstep(0.0, 2.0, edge);
    return vec4f(mix(mix(paper, ink * 0.45, rule), ink, ring), 1.0);
}
";

/// The colour as given, so the half egui paints beside it has something to disagree with.
const SWATCH: &str = r"
struct Params {
    tint: vec4f,
}

@group(0) @binding(0) var<uniform> params: Params;

@fragment
fn fs_main(in: Corner) -> @location(0) vec4f {
    return params.tint;
}
";

/// Rings running out from the middle, placed by a clock the scene passes in.
const RIPPLE: &str = r"
struct Params {
    size: vec2f,
    seconds: f32,
    speed: f32,
}

@group(0) @binding(0) var<uniform> params: Params;

@fragment
fn fs_main(in: Corner) -> @location(0) vec4f {
    let aspect = vec2f(params.size.x / params.size.y, 1.0);
    let out_from_middle = length((in.uv - 0.5) * aspect);
    let wave = sin(out_from_middle * 34.0 - params.seconds * params.speed);
    let lit = 0.5 + 0.5 * wave * (1.0 - out_from_middle);
    return vec4f(vec3f(0.10, 0.42, 0.48) + vec3f(0.30, 0.34, 0.30) * lit, 1.0);
}
";

/// A cube, tinted by face so that a face drawn in the wrong order is unmistakable,
/// built from the vertex index alone — there is no vertex buffer to bind.
const CUBE: &str = r"
struct Params {
    spin: f32,
    aspect: f32,
}

@group(0) @binding(0) var<uniform> params: Params;

struct Face {
    @builtin(position) position: vec4f,
    @location(0) tint: vec3f,
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> Face {
    var corners = array<vec3f, 8>(
        vec3f(-1.0, -1.0, -1.0), vec3f( 1.0, -1.0, -1.0),
        vec3f( 1.0,  1.0, -1.0), vec3f(-1.0,  1.0, -1.0),
        vec3f(-1.0, -1.0,  1.0), vec3f( 1.0, -1.0,  1.0),
        vec3f( 1.0,  1.0,  1.0), vec3f(-1.0,  1.0,  1.0),
    );
    var faces = array<u32, 36>(
        0u, 1u, 2u, 0u, 2u, 3u,
        5u, 4u, 7u, 5u, 7u, 6u,
        4u, 0u, 3u, 4u, 3u, 7u,
        1u, 5u, 6u, 1u, 6u, 2u,
        3u, 2u, 6u, 3u, 6u, 7u,
        4u, 5u, 1u, 4u, 1u, 0u,
    );
    var tints = array<vec3f, 6>(
        vec3f(0.85, 0.32, 0.32), vec3f(0.30, 0.72, 0.46), vec3f(0.35, 0.55, 0.90),
        vec3f(0.90, 0.72, 0.26), vec3f(0.66, 0.42, 0.86), vec3f(0.24, 0.76, 0.78),
    );

    let at = corners[faces[index]];
    let turn = cos(params.spin);
    let swing = sin(params.spin);
    let spun = vec3f(at.x * turn + at.z * swing, at.y, at.z * turn - at.x * swing);
    // Tipped towards the viewer, so the top face shows and the cube reads as a solid.
    let tipped = vec3f(spun.x, spun.y * 0.87 - spun.z * 0.5, spun.y * 0.5 + spun.z * 0.87);
    let eye = vec3f(tipped.x, tipped.y, tipped.z + 5.0);

    var out: Face;
    // `w` is the eye distance, so the divide leaves depth inside the 0..1 a depth pass clears to.
    out.position = vec4f(
        eye.x * 2.6 / params.aspect,
        eye.y * 2.6,
        (eye.z - 1.0) * 10.0 / 9.0,
        eye.z,
    );
    out.tint = tints[index / 6u];
    return out;
}

@fragment
fn fs_main(in: Face) -> @location(0) vec4f {
    return vec4f(in.tint, 1.0);
}
";

/// Puts a rendered texture on screen, which is how anything drawn in a pass of its own
/// reaches egui's.
const BLIT: &str = r"
@group(0) @binding(0) var rendered: texture_2d<f32>;
@group(0) @binding(1) var taps: sampler;

@fragment
fn fs_main(in: Corner) -> @location(0) vec4f {
    return textureSample(rendered, taps, in.uv);
}
";

// --- What the scenes keep on the GPU ---

/// A fullscreen pipeline and the uniform buffer feeding it.
struct Program {
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    bound: wgpu::BindGroup,
}

impl Program {
    /// `words` is how many `f32`s the shader's `Params` holds, which sizes the buffer.
    fn new(state: &egui_wgpu::RenderState, label: &str, fragment: &str, words: usize) -> Self {
        let module = module(state, label, &format!("{FULLSCREEN}{fragment}"));
        let pipeline = fullscreen_pipeline(state, label, &module, None);
        let uniforms = state.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: uniform_size(words),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bound = state.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            }],
        });
        Self {
            pipeline,
            uniforms,
            bound,
        }
    }

    fn draw(&self, pass: &mut wgpu::RenderPass<'static>) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bound, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// The cube's two ways of reaching the screen, and the target the second one needs.
struct Solid {
    /// Straight into egui's render pass, which carries no depth attachment.
    inline: wgpu::RenderPipeline,
    /// Into a pass of the scene's own, which does.
    offscreen: wgpu::RenderPipeline,
    blit: wgpu::RenderPipeline,
    taps: wgpu::Sampler,
    format: wgpu::TextureFormat,
    uniforms: wgpu::Buffer,
    bound: wgpu::BindGroup,
    target: Option<Target>,
}

/// Where the offscreen pass draws: a colour texture the blit samples, and the depth buffer
/// that is the whole point of having a pass at all. Kept until the rect asks for another size.
struct Target {
    size: [u32; 2],
    color: wgpu::TextureView,
    depth: wgpu::TextureView,
    bound: wgpu::BindGroup,
}

impl Target {
    fn new(device: &wgpu::Device, solid: &Solid, size: [u32; 2]) -> Self {
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
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        let color = plane(
            "cube colour",
            solid.format,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let depth = plane(
            "cube depth",
            wgpu::TextureFormat::Depth32Float,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let bound = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cube blit"),
            layout: &solid.blit.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&color),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&solid.taps),
                },
            ],
        });
        Self {
            size,
            color,
            depth,
            bound,
        }
    }
}

/// Every scene in this file, in one `callback_resources` entry.
/// The map is keyed by type and shared by the whole renderer,
/// so one entry per file leaves one thing to find and one thing to rebuild.
struct Shaders {
    gradient: Program,
    pixels: Program,
    swatch: Program,
    ripple: Program,
    solid: Solid,
}

impl Shaders {
    fn new(state: &egui_wgpu::RenderState) -> Self {
        let cube = module(state, "cube", CUBE);
        let uniforms = state.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cube"),
            size: uniform_size(2),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // Spelled out, where a program with one pipeline lets the shader imply it:
        // an inferred layout belongs to the pipeline that inferred it,
        // so the cube's two routes would otherwise need a bind group each.
        let layout = state
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("cube"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });
        let pipelines = state
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("cube"),
                bind_group_layouts: &[Some(&layout)],
                immediate_size: 0,
            });
        let bound = state.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cube"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            }],
        });
        let blit = module(state, "cube blit", &format!("{FULLSCREEN}{BLIT}"));
        Self {
            gradient: Program::new(state, "gradient", GRADIENT, 9),
            pixels: Program::new(state, "device pixels", PIXELS, 4),
            swatch: Program::new(state, "swatch", SWATCH, 4),
            ripple: Program::new(state, "ripple", RIPPLE, 4),
            solid: Solid {
                inline: cube_pipeline(state, "cube inline", &cube, &pipelines, false),
                offscreen: cube_pipeline(state, "cube offscreen", &cube, &pipelines, true),
                // Blended, since the offscreen pass clears to nothing
                // and only the cube is meant to land on the canvas.
                blit: fullscreen_pipeline(
                    state,
                    "cube blit",
                    &blit,
                    Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                ),
                taps: state.device.create_sampler(&wgpu::SamplerDescriptor {
                    label: Some("cube blit"),
                    mag_filter: wgpu::FilterMode::Nearest,
                    min_filter: wgpu::FilterMode::Nearest,
                    ..Default::default()
                }),
                format: state.target_format,
                uniforms,
                bound,
                target: None,
            },
        }
    }
}

fn module(state: &egui_wgpu::RenderState, label: &str, source: &str) -> wgpu::ShaderModule {
    state
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(label),
            source: wgpu::ShaderSource::Wgsl(source.to_owned().into()),
        })
}

fn fullscreen_pipeline(
    state: &egui_wgpu::RenderState,
    label: &str,
    module: &wgpu::ShaderModule,
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    state
        .device
        .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            // Inferred from the shader; the bind group comes off the result.
            layout: None,
            vertex: wgpu::VertexState {
                module,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: state.target_format,
                    blend,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        })
}

fn cube_pipeline(
    state: &egui_wgpu::RenderState,
    label: &str,
    module: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    depth: bool,
) -> wgpu::RenderPipeline {
    state
        .device
        .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            // Every face drawn, back ones included: culling them would hide the missing depth
            // buffer, a convex solid having nothing left to sort once its far side is gone.
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: depth.then(|| wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(state.target_format.into())],
            }),
            multiview_mask: None,
            cache: None,
        })
}

/// The bytes a uniform buffer takes for `words`, padded up to WGSL's 16-byte struct alignment.
fn uniform_bytes(words: &[f32]) -> Vec<u8> {
    let mut bytes: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
    bytes.resize(bytes.len().next_multiple_of(16), 0);
    bytes
}

fn uniform_size(words: usize) -> u64 {
    (words * size_of::<f32>()).next_multiple_of(16) as u64
}

fn channels(color: egui::Color32) -> [f32; 4] {
    color.to_array().map(|channel| f32::from(channel) / 255.0)
}

// --- Drawing ---

/// One frame's draw of one fullscreen program: which one, and the uniforms it takes.
///
/// A program owns one buffer, so it takes one draw per frame — two of these on the same program
/// would leave both painting whichever uniforms were written second.
struct Draw {
    program: fn(&Shaders) -> &Program,
    uniforms: Vec<u8>,
}

impl CallbackTrait for Draw {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        queue.write_buffer(
            &(self.program)(shaders(resources)).uniforms,
            0,
            &self.uniforms,
        );
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &CallbackResources,
    ) {
        (self.program)(shaders(resources)).draw(pass);
    }
}

/// One frame's draw of the cube, either way round.
struct Spin {
    turned: f32,
    /// In points: [`prepare`](CallbackTrait::prepare) is told the size of the screen
    /// but not of the rect, so a target sized to the rect
    /// has to be measured before the callback goes out.
    rect: egui::Rect,
    depth: bool,
}

impl CallbackTrait for Spin {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen: &ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let size = self.rect.size() * screen.pixels_per_point;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a rect on a canvas is a few thousand non-negative pixels at most"
        )]
        let size = [size.x.round() as u32, size.y.round() as u32].map(|side| side.max(1));
        let solid = &mut shaders_mut(resources).solid;
        queue.write_buffer(
            &solid.uniforms,
            0,
            &uniform_bytes(&[self.turned, aspect(self.rect)]),
        );
        if !self.depth {
            return Vec::new();
        }
        if solid
            .target
            .as_ref()
            .is_none_or(|target| target.size != size)
        {
            solid.target = Some(Target::new(device, solid, size));
        }
        let target = solid.target.as_ref().expect("just made if it was missing");
        // Recorded on egui's own encoder, which submits it before the pass that paints below.
        let mut pass = encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("cube"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.color,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &target.depth,
                    depth_ops: Some(wgpu::Operations {
                        // Cleared to the far plane, so the nearest face at each pixel wins.
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            })
            .forget_lifetime();
        pass.set_pipeline(&solid.offscreen);
        pass.set_bind_group(0, &solid.bound, &[]);
        pass.draw(0..36, 0..1);
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &CallbackResources,
    ) {
        let solid = &shaders(resources).solid;
        match solid.target.as_ref().filter(|_| self.depth) {
            Some(target) => {
                pass.set_pipeline(&solid.blit);
                pass.set_bind_group(0, &target.bound, &[]);
                pass.draw(0..3, 0..1);
            }
            None => {
                pass.set_pipeline(&solid.inline);
                pass.set_bind_group(0, &solid.bound, &[]);
                pass.draw(0..36, 0..1);
            }
        }
    }
}

fn shaders(resources: &CallbackResources) -> &Shaders {
    resources
        .get()
        .expect("the scene builds these before it stages a callback")
}

fn shaders_mut(resources: &mut CallbackResources) -> &mut Shaders {
    resources
        .get_mut()
        .expect("the scene builds these before it stages a callback")
}

/// Whether this file's GPU half is in the renderer and a callback can be staged.
/// `false` under glow, where the scene has already said so on the canvas.
fn ready(ctx: &mut SceneCtx, ui: &mut Ui) -> bool {
    let Some(state) = ctx.render_state() else {
        stage!(ctx, ui, |ui| {
            ui.colored_label(
                egui::Color32::YELLOW,
                "this scene draws through a wgpu paint callback, and the glow renderer has none",
            );
        });
        return false;
    };
    // Looked up every frame and built when missing,
    // so a `--hot` rebuild that leaves the dylib's `Shaders` unfindable
    // costs one rebuild rather than a panic.
    let mut renderer = state.renderer.write();
    if renderer.callback_resources.get::<Shaders>().is_none() {
        let built = Shaders::new(&state);
        renderer.callback_resources.insert(built);
    }
    true
}

fn aspect(rect: egui::Rect) -> f32 {
    if rect.height() > 0.0 {
        rect.width() / rect.height()
    } else {
        1.0
    }
}

/// Claim `ui`'s room and put `draw` on the rect that came back.
fn callback(ui: &mut Ui, draw: impl CallbackTrait + 'static) {
    let (rect, _) = ui.allocate_exact_size(ui.available_size(), egui::Sense::hover());
    ui.painter()
        .add(egui_wgpu::Callback::new_paint_callback(rect, draw));
}

// --- The scenes ---

/// The plain case, and the one to read first: knob values reach the shader through a uniform
/// buffer the callback writes each frame, and the drawing covers the canvas edge to edge.
#[scene(default)]
fn gradient(ctx: &mut SceneCtx, ui: &mut Ui) {
    let top = ctx.color("top", TOP);
    let bottom = ctx.color("bottom", BOTTOM);
    // 1 is the straight ramp; either side of it pushes the midpoint up or down the height.
    let bias = ctx.slider("bias", 1.0, 0.25, 4.0, 0.05);
    if !ready(ctx, ui) {
        return;
    }

    // A backdrop reaches every edge, so no stage and no badge: the callback takes the whole
    // clip rect, canvas margin included. The room is still claimed, because a capture trims
    // to what the scene laid out — painted-only pixels would be cropped away.
    ui.allocate_exact_size(ui.available_size(), egui::Sense::hover());
    let mut words = Vec::from(channels(top));
    words.extend(channels(bottom));
    words.push(bias);
    ui.painter().add(egui_wgpu::Callback::new_paint_callback(
        ui.clip_rect(),
        Draw {
            program: |shaders| &shaders.gradient,
            uniforms: uniform_bytes(&words),
        },
    ));
}

/// A shader measuring in device pixels, which a window and a capture do not agree on: a capture
/// pins the scale at 1, while a window follows the display's. The ring is a fixed pixel radius
/// and the rules are one pixel wide, so a shot taken on a HiDPI screen and one taken headlessly
/// hold a different number of both — the caption reports the scale each was drawn at.
///
/// It is also the only shape here that would come out an ellipse if the rect's own aspect
/// went unread.
#[scene("device pixels")]
fn device_pixels(ctx: &mut SceneCtx, ui: &mut Ui) {
    let radius = ctx.slider("ring radius (px)", 48.0, 8.0, 120.0, 1.0);
    let pitch = ctx.slider("rule pitch (px)", 16.0, 4.0, 64.0, 1.0);
    if !ready(ctx, ui) {
        return;
    }

    let scale = ui.ctx().pixels_per_point();
    ui.label(format!("{scale} px per point"));
    stage!(ctx, ui, (260, 180), |ui| {
        let size = ui.available_size() * scale;
        callback(
            ui,
            Draw {
                program: |shaders| &shaders.pixels,
                uniforms: uniform_bytes(&[size.x, size.y, radius, pitch]),
            },
        );
    });
}

/// A callback inside a stage that scrolls, so egui scissors it: the rect the callback is given
/// is the whole drawing, while only the slice inside the stage reaches the screen.
/// The ring is centred in that whole drawing rather than in what shows,
/// so raising `drawing height` sinks it below the fold
/// and leaves the rules running off an edge nowhere near the bottom of what was drawn.
/// Scroll the stage to reach it.
///
/// The stage chrome the backdrop scenes skip is here too — the checkerboard behind the drawing,
/// and the badge, which reports the stage rather than the taller thing inside it.
///
/// A drawing taller than the window is another matter: egui clamps a callback's viewport
/// to the screen (`ViewportInPixels::from_points`), leaving the shader to measure a rect
/// that stops at the window's bottom rather than the one it was handed.
/// Hence the modest ceiling here.
#[scene("clipped")]
fn clipped(ctx: &mut SceneCtx, ui: &mut Ui) {
    let tall = ctx.slider("drawing height", 320.0, 180.0, 480.0, 10.0);
    if !ready(ctx, ui) {
        return;
    }

    let scale = ui.ctx().pixels_per_point();
    ctx.stage(
        ui,
        Stage::Fixed(egui::vec2(260.0, 180.0)).scrollable(),
        |ui| {
            let (rect, _) = ui
                .allocate_exact_size(egui::vec2(ui.available_width(), tall), egui::Sense::hover());
            let size = rect.size() * scale;
            ui.painter().add(egui_wgpu::Callback::new_paint_callback(
                rect,
                Draw {
                    program: |shaders| &shaders.pixels,
                    uniforms: uniform_bytes(&[size.x, size.y, 56.0, 24.0]),
                },
            ));
        },
    );
}

/// The same colour twice: egui fills the left half, the shader returns it on the right.
/// A seam down the middle is the two paths disagreeing about colour space — which matters
/// the moment shader-drawn content has to sit behind or beside anything egui drew.
#[scene("colour match")]
fn colour_match(ctx: &mut SceneCtx, ui: &mut Ui) {
    let tint = ctx.color("tint", egui::Color32::from_rgb(0x6C, 0x9C, 0xD8));
    if !ready(ctx, ui) {
        return;
    }

    stage!(ctx, ui, (260, 120), |ui| {
        let (rect, _) = ui.allocate_exact_size(ui.available_size(), egui::Sense::hover());
        let (egui_half, shader_half) = rect.split_left_right_at_fraction(0.5);
        ui.painter().rect_filled(egui_half, 0, tint);
        ui.painter().add(egui_wgpu::Callback::new_paint_callback(
            shader_half,
            Draw {
                program: |shaders| &shaders.swatch,
                uniforms: uniform_bytes(&channels(tint)),
            },
        ));
    });
}

/// A shader on a clock, which never stops asking to be redrawn.
/// A `settle` capture of this one runs out of frames rather than going quiet,
/// and is reported as still moving — the picture being the moment the count landed on.
#[scene("animated")]
fn animated(ctx: &mut SceneCtx, ui: &mut Ui) {
    let speed = ctx.slider("speed", 3.0, 0.0, 12.0, 0.1);
    if !ready(ctx, ui) {
        return;
    }

    #[expect(
        clippy::cast_possible_truncation,
        reason = "a shader wants the phase, not the epoch"
    )]
    let seconds = ui.input(|i| i.time) as f32;
    stage!(ctx, ui, (260, 180), |ui| {
        let size = ui.available_size();
        callback(
            ui,
            Draw {
                program: |shaders| &shaders.ripple,
                uniforms: uniform_bytes(&[size.x, size.y, seconds, speed]),
            },
        );
    });
    ui.ctx().request_repaint();
}

/// Where an inline callback runs out: egui's render pass carries no depth attachment.
/// A solid drawn straight into it is sorted by nothing but the order its faces were submitted,
/// so the far side of the cube lands on top of the near one.
///
/// Turning `depth buffer` on takes the other route, the one anything three-dimensional ends up
/// on: the callback records a pass of its own in `prepare`, with a colour texture and a depth
/// buffer it owns, and `paint` puts the result on screen as a sampled texture.
#[scene("depth")]
fn depth(ctx: &mut SceneCtx, ui: &mut Ui) {
    let turned = ctx.slider("turned", 0.6, 0.0, std::f32::consts::TAU, 0.01);
    let depth = ctx.toggle("depth buffer", true);
    if !ready(ctx, ui) {
        return;
    }

    stage!(ctx, ui, (260, 200), |ui| {
        let (rect, _) = ui.allocate_exact_size(ui.available_size(), egui::Sense::hover());
        ui.painter().add(egui_wgpu::Callback::new_paint_callback(
            rect,
            Spin {
                turned,
                rect,
                depth,
            },
        ));
    });
}
