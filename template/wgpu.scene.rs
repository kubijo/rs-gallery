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

// --- What the scenes keep on the GPU ---

/// Which pass a pipeline is built for. A colour format, a sample count and a depth attachment
/// are all part of what a pipeline is, so the two passes are not interchangeable
/// and a shader wanted in both places is compiled twice.
#[derive(Clone, Copy)]
enum Target {
    /// egui's own render pass, reached through an [`egui_wgpu::Callback`].
    Egui,
    /// The target [`SceneCtx::render_pass`] hands over, which carries depth.
    Scene,
}

impl Target {
    fn colour(self, state: &egui_wgpu::RenderState) -> wgpu::TextureFormat {
        match self {
            Self::Egui => state.target_format,
            Self::Scene => ScenePass::FORMAT,
        }
    }

    fn samples(self) -> u32 {
        match self {
            Self::Egui => MSAA_SAMPLES,
            Self::Scene => ScenePass::SAMPLES,
        }
    }
}

/// A fullscreen pipeline and the uniform buffer feeding it.
struct Program {
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    bound: wgpu::BindGroup,
}

impl Program {
    /// `words` is how many `f32`s the shader's `Params` holds, which sizes the buffer.
    /// `target` is what it draws into — egui's own pass, or the one
    /// [`SceneCtx::render_pass`] hands over, which agrees with it on nothing
    /// and so needs a pipeline of its own.
    fn new(
        state: &egui_wgpu::RenderState,
        label: &str,
        fragment: &str,
        words: usize,
        target: Target,
    ) -> Self {
        let module = module(state, label, &format!("{FULLSCREEN}{fragment}"));
        let pipeline = fullscreen_pipeline(state, label, &module, target, None);
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

/// The cube's two ways of reaching the screen.
struct Solid {
    /// Straight into egui's render pass, which carries no depth attachment — so this one
    /// is sorted by the order its faces were submitted and nothing else.
    inline: wgpu::RenderPipeline,
    /// Into the target [`SceneCtx::render_pass`] hands over, which has depth.
    /// A separate pipeline because a depth attachment is part of what one is built against.
    sorted: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    bound: wgpu::BindGroup,
}

/// Every scene in this file, in one `callback_resources` entry.
/// The map is keyed by type and shared by the whole renderer,
/// so one entry per file leaves one thing to find and one thing to rebuild.
struct Shaders {
    gradient: Program,
    pixels: Program,
    swatch: Program,
    swatch_pass: Program,
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
        Self {
            gradient: Program::new(state, "gradient", GRADIENT, 9, Target::Egui),
            pixels: Program::new(state, "device pixels", PIXELS, 4, Target::Egui),
            swatch: Program::new(state, "swatch", SWATCH, 4, Target::Egui),
            // The same fill again for the other route, so `colour match` can show all three
            // paths to a pixel side by side.
            swatch_pass: Program::new(state, "swatch pass", SWATCH, 4, Target::Scene),
            ripple: Program::new(state, "ripple", RIPPLE, 4, Target::Egui),
            solid: Solid {
                inline: cube_pipeline(state, "cube inline", &cube, &pipelines, Target::Egui),
                sorted: cube_pipeline(state, "cube sorted", &cube, &pipelines, Target::Scene),
                uniforms,
                bound,
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
    target: Target,
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
            // A fullscreen fill covers every pixel once and sorts nothing, but gallery's pass
            // carries depth and a pipeline is matched against all of it — so it still declares
            // the state, just one that tests nothing.
            depth_stencil: matches!(target, Target::Scene).then(|| ScenePass::depth_state(false)),
            multisample: wgpu::MultisampleState {
                count: target.samples(),
                ..Default::default()
            },
            fragment: Some(wgpu::FragmentState {
                module,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target.colour(state),
                    blend,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        })
}

/// The cube is built twice, once for each `target`: a colour format, a sample count and a depth
/// attachment are all part of what a pipeline is, and the two passes agree on none of them.
fn cube_pipeline(
    state: &egui_wgpu::RenderState,
    label: &str,
    module: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    target: Target,
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
            // Sorted where there is a buffer to sort with, which is the whole comparison.
            depth_stencil: matches!(target, Target::Scene).then(|| ScenePass::depth_state(true)),
            multisample: wgpu::MultisampleState {
                count: target.samples(),
                ..Default::default()
            },
            fragment: Some(wgpu::FragmentState {
                module,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(target.colour(state).into())],
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

/// One frame's draw of the cube straight into egui's pass — the route with no depth to be had.
struct Spin {
    turned: f32,
    aspect: f32,
}

impl CallbackTrait for Spin {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let solid = &shaders(resources).solid;
        queue.write_buffer(
            &solid.uniforms,
            0,
            &uniform_bytes(&[self.turned, self.aspect]),
        );
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &CallbackResources,
    ) {
        let solid = &shaders(resources).solid;
        pass.set_pipeline(&solid.inline);
        pass.set_bind_group(0, &solid.bound, &[]);
        pass.draw(0..36, 0..1);
    }
}

fn shaders(resources: &CallbackResources) -> &Shaders {
    resources
        .get()
        .expect("the scene builds these before it stages a callback")
}

/// Take what a [`SceneCtx::render_pass`] draw needs out of `callback_resources` before the draw
/// begins, so its closure holds no borrow of the renderer.
/// Every wgpu handle is a refcount, so cloning one out costs nothing.
///
/// A callback needs none of this — it is handed the resources — but a pass of the scene's own
/// runs from the scene body, where the context is already borrowed for the drawing.
fn lifted<T>(state: &egui_wgpu::RenderState, pick: impl FnOnce(&Shaders) -> T) -> T {
    pick(shaders(&state.renderer.read().callback_resources))
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

/// The same colour by all three routes to a pixel, flush against each other:
/// egui fills the left, a callback into egui's own pass fills the middle,
/// and a pass into gallery's target fills the right. A seam is a route disagreeing about
/// colour space, which matters the moment shader-drawn content has to sit behind
/// or beside anything egui drew.
///
/// The right band is the one to watch. It goes into a texture and is sampled back out,
/// so its colour rounds to eight bits twice over and can land a step off the other two.
#[scene("colour match")]
fn colour_match(ctx: &mut SceneCtx, ui: &mut Ui) {
    const BAND: u32 = 90;

    let tint = ctx.color("tint", egui::Color32::from_rgb(0x6C, 0x9C, 0xD8));
    if !ready(ctx, ui) {
        return;
    }
    let Some(state) = ctx.render_state() else {
        return;
    };
    let (pipeline, bound, uniforms) = lifted(&state, |shaders| {
        let program = &shaders.swatch_pass;
        (
            program.pipeline.clone(),
            program.bound.clone(),
            program.uniforms.clone(),
        )
    });

    ui.horizontal(|ui| {
        // No spacing, so the three meet with nothing between them to hide a step.
        ui.spacing_mut().item_spacing.x = 0.0;
        let band = egui::vec2(BAND as f32, BAND as f32);
        let (painted, _) = ui.allocate_exact_size(band, egui::Sense::hover());
        ui.painter().rect_filled(painted, 0, tint);

        let (called, _) = ui.allocate_exact_size(band, egui::Sense::hover());
        ui.painter().add(egui_wgpu::Callback::new_paint_callback(
            called,
            Draw {
                program: |shaders| &shaders.swatch,
                uniforms: uniform_bytes(&channels(tint)),
            },
        ));

        let filled = uniform_bytes(&channels(tint));
        ctx.render_pass(ui, [BAND, BAND], |target| {
            target.queue().write_buffer(&uniforms, 0, &filled);
            let pass = target.pass();
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bound, &[]);
            pass.draw(0..3, 0..1);
        });
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

/// The same cube twice, by the two routes wgpu content can take.
///
/// On the left, an inline callback: egui's render pass carries no depth attachment,
/// so the solid is sorted by nothing but the order its faces were submitted
/// and its far side lands on top of the near one.
/// On the right, [`SceneCtx::render_pass`]: gallery hands over a target with a depth buffer,
/// and the same shader comes out a cube.
///
/// The scene keeps no texture, no depth buffer and no resize rule of its own — only the second
/// pipeline, since a depth attachment is part of what a pipeline is built against.
#[scene("depth")]
fn depth(ctx: &mut SceneCtx, ui: &mut Ui) {
    let turned = ctx.slider("turned", 0.6, 0.0, std::f32::consts::TAU, 0.01);
    // Both sides take it, so the two stay comparable — and gallery's target is reallocated
    // to follow, which is the half of it a fixed size would never ask for.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a slider bounded well inside a pixel count"
    )]
    let side = ctx.slider("side", 200.0, 80.0, 260.0, 10.0) as u32;
    if !ready(ctx, ui) {
        return;
    }
    let Some(state) = ctx.render_state() else {
        return;
    };

    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label("egui's pass — no depth");
            stage!(ctx, ui, (side, side), |ui| {
                let (rect, _) = ui.allocate_exact_size(ui.available_size(), egui::Sense::hover());
                ui.painter().add(egui_wgpu::Callback::new_paint_callback(
                    rect,
                    Spin {
                        turned,
                        aspect: aspect(rect),
                    },
                ));
            });
        });
        ui.vertical(|ui| {
            ui.label("a pass of the scene's own");
            let (pipeline, bound, uniforms) = lifted(&state, |shaders| {
                let solid = &shaders.solid;
                (
                    solid.sorted.clone(),
                    solid.bound.clone(),
                    solid.uniforms.clone(),
                )
            });
            ctx.render_pass_stage(ui, Stage::Fit, [side, side], |target| {
                // Off the target rather than off `side`, so the cube stays square
                // whatever shape the target is asked for.
                let [wide, high] = target.size().map(|side| side as f32);
                target
                    .queue()
                    .write_buffer(&uniforms, 0, &uniform_bytes(&[turned, wide / high]));
                let pass = target.pass();
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &bound, &[]);
                pass.draw(0..36, 0..1);
            });
        });
    });
}
