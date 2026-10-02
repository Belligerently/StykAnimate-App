//! Effect executor: runs an effect stack on one layer of the isolation atlas,
//! pass for pass as the editor's WebGPU executor does (`VfxWebGPU.run` in
//! src/vfx-webgpu.mjs). The layer is copied into a working slot, filtered
//! through fragment passes (`passes.wgsl`) and copied back in place.
//!
//! Working slots are pooled: eight textures of one size, grown to the largest
//! layer a frame filters and kept across frames, so a steady animation creates
//! no texture. A pass uses the top-left corner of its slot.

use std::collections::HashMap;
use std::f64::consts::PI;
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use styk_eval::doc::PaintKind;
use styk_eval::vfx::registry::{BlurMode, GlowPlacement, OutlinePlacement, Params as P, ShadowPlacement};
use styk_eval::vfx::{Color, Effect};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Uniform record of one pass (`Params` in passes.wgsl), padded to the binding alignment.
const UNIFORM_SIZE: u64 = 736;
const UNIFORM_STRIDE: u64 = 768;
/// Slots: 0 input, 1–3 ping-pong, 4 saved artwork, 5 inner outline, 6 spare, 7 strength source.
const SLOTS: usize = 8;
const BLANK: u8 = u8::MAX;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniform {
    dimensions: [f32; 2],
    origin: [f32; 2],
    output_size: [f32; 2],
    paint_type: i32,
    stop_count: i32,
    paint_angle: f32,
    operation: i32,
    target: [f32; 2],
    texel: [f32; 2],
    unused: [f32; 2],
    p: [f32; 4],
    q: [f32; 4],
    stops: [[f32; 4]; 32],
    stop_offsets: [f32; 32],
}

const _: () = assert!(std::mem::size_of::<Uniform>() as u64 == UNIFORM_SIZE);

/// Where an effect's coordinates come from (`run`'s options), in layer pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    /// Effect sizes are multiplied by this (pixels per authored unit).
    pub scale: f64,
    /// Global pixel position of the layer's first texel.
    pub origin: [f64; 2],
    /// Size of the coordinate domain gradients and centres are relative to.
    pub output_size: [f64; 2],
}

/// The brush (`paint` in `run`): solid or gradient stops for the passes that use one.
#[derive(Clone, Copy)]
struct Brush {
    kind: i32,
    angle: f32,
    count: i32,
    colors: [[f32; 4]; 32],
    offsets: [f32; 32],
}

impl Default for Brush {
    fn default() -> Brush {
        Brush { kind: 0, angle: 0.0, count: 1, colors: [[0.0; 4]; 32], offsets: [0.0; 32] }
    }
}

/// One recorded step of a run.
#[derive(Clone, Copy, Debug)]
pub enum Step {
    /// A fragment pass into `target` of `size`, reading `source` (its size `source_size`).
    Pass { operation: i32, source: u8, source_size: [u32; 2], target: u8, size: [u32; 2], original: u8, mask: u8, uniform: u32 },
    /// Copies the top-left `size` of slot `from` into slot `to`.
    Copy { from: u8, to: u8, size: [u32; 2] },
}

/// A recorded run: its steps and the slot holding the result.
#[derive(Clone, Debug, Default)]
pub struct Run {
    pub steps: Vec<Step>,
    pub result: u8,
    pub size: [u32; 2],
}

/// Records runs for one frame, then encodes them.
pub struct Executor {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniforms: wgpu::Buffer,
    uniform_slots: u64,
    blank: wgpu::TextureView,
    slots: Vec<Option<(wgpu::Texture, wgpu::TextureView)>>,
    /// Size every slot is allocated at.
    pub slot_size: [u32; 2],
    binds: HashMap<(u8, u8, u8), wgpu::BindGroup>,
    /// Uniform records of the frame being recorded.
    records: Vec<Uniform>,
    /// Frames since one needed slots of the current size (shrinks after a while).
    idle: u32,
    pub stats: Stats,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub layers: u32,
    pub passes: u32,
    pub slot_bytes: u64,
}

fn round(x: f64) -> f64 {
    styk_eval::vfx::paint::js_round(x)
}

impl Executor {
    pub fn new(device: &wgpu::Device) -> Executor {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("styk effects"), source: wgpu::ShaderSource::Wgsl(include_str!("passes.wgsl").into()) });
        let texture = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("styk effects"),
            entries: &[
                texture(0),
                texture(1),
                texture(2),
                wgpu::BindGroupLayoutEntry { binding: 3, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: true, min_binding_size: wgpu::BufferSize::new(UNIFORM_SIZE) },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("styk effects"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("styk effects"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vertex"), compilation_options: Default::default(), buffers: &[] },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fragment"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: FORMAT, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("styk effects"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let blank = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("styk effects blank"),
                size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        Executor {
            pipeline,
            layout,
            sampler,
            uniforms: device.create_buffer(&wgpu::BufferDescriptor { label: Some("styk effect passes"), size: UNIFORM_STRIDE * 64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false }),
            uniform_slots: 64,
            blank,
            slots: (0..SLOTS).map(|_| None).collect(),
            slot_size: [0, 0],
            binds: HashMap::new(),
            records: Vec::new(),
            idle: 0,
            stats: Stats::default(),
        }
    }

    /// Starts a frame's recording.
    pub fn begin(&mut self) {
        self.records.clear();
        self.stats = Stats { slot_bytes: self.stats.slot_bytes, ..Default::default() };
    }

    /// Records `stack` on a `size` layer (in slot 0). Mirrors `VfxWebGPU.run`.
    pub fn record(&mut self, stack: &[Arc<Effect>], frame: Frame, size: [u32; 2]) -> Run {
        let run = record(&mut self.records, stack, frame, size);
        self.stats.layers += 1;
        run
    }

    /// Prepares the frame's recorded runs: slots of at least `largest`, uniforms written.
    pub fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, largest: [u32; 2]) {
        if largest[0] == 0 || largest[1] == 0 {
            // No effect this frame: release the slots after a while (a quiet period).
            self.idle += 1;
            if self.idle > 240 && self.slots.iter().any(Option::is_some) {
                self.release();
            }
            return;
        }
        let bucket = |n: u32| n.div_ceil(256) * 256;
        let fits = self.slot_size[0] >= largest[0] && self.slot_size[1] >= largest[1];
        // Shrink when this frame needs less than a quarter of the area for a while.
        let small = (largest[0] as u64 * largest[1] as u64) * 4 < self.slot_size[0] as u64 * self.slot_size[1] as u64;
        self.idle = if fits && small { self.idle + 1 } else { 0 };
        if !fits || self.idle > 240 {
            let size = if fits { [bucket(largest[0]), bucket(largest[1])] } else { [bucket(largest[0]).max(self.slot_size[0]), bucket(largest[1]).max(self.slot_size[1])] };
            self.release();
            self.slot_size = size;
            self.idle = 0;
        }
        let texel = [1.0 / self.slot_size[0].max(1) as f32, 1.0 / self.slot_size[1].max(1) as f32];
        for record in &mut self.records {
            record.texel = texel;
        }
        let need = self.records.len() as u64;
        if need > self.uniform_slots {
            self.uniform_slots = need.next_power_of_two();
            self.uniforms = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("styk effect passes"),
                size: UNIFORM_STRIDE * self.uniform_slots,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.binds.clear();
        }
        if need > 0 {
            let mut bytes = vec![0u8; (UNIFORM_STRIDE * need) as usize];
            for (i, record) in self.records.iter().enumerate() {
                let o = i * UNIFORM_STRIDE as usize;
                bytes[o..o + UNIFORM_SIZE as usize].copy_from_slice(bytemuck::bytes_of(record));
            }
            queue.write_buffer(&self.uniforms, 0, &bytes);
        }
    }

    fn release(&mut self) {
        for slot in &mut self.slots {
            if let Some((texture, _)) = slot.take() {
                texture.destroy();
            }
        }
        self.binds.clear();
        self.slot_size = [0, 0];
        self.stats.slot_bytes = 0;
    }

    fn slot(&mut self, device: &wgpu::Device, index: u8) -> &wgpu::TextureView {
        let size = self.slot_size;
        if self.slots[index as usize].is_none() {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("styk effect slot"),
                size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            self.stats.slot_bytes += size[0] as u64 * size[1] as u64 * 4;
            self.slots[index as usize] = Some((texture, view));
        }
        &self.slots[index as usize].as_ref().unwrap().1
    }

    /// The texture of slot `index` (allocated on demand).
    pub fn slot_texture(&mut self, device: &wgpu::Device, index: u8) -> &wgpu::Texture {
        self.slot(device, index);
        &self.slots[index as usize].as_ref().unwrap().0
    }

    fn bind(&mut self, device: &wgpu::Device, source: u8, original: u8, mask: u8) -> wgpu::BindGroup {
        if let Some(bind) = self.binds.get(&(source, original, mask)) {
            return bind.clone();
        }
        for index in [source, original, mask] {
            if index != BLANK {
                self.slot(device, index);
            }
        }
        let view = |index: u8| if index == BLANK { &self.blank } else { &self.slots[index as usize].as_ref().unwrap().1 };
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("styk effect pass"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view(source)) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(view(original)) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(view(mask)) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &self.uniforms, offset: 0, size: wgpu::BufferSize::new(UNIFORM_SIZE) }),
                },
            ],
        });
        self.binds.insert((source, original, mask), bind.clone());
        bind
    }

    /// Encodes the steps of `run` (after [`Executor::prepare`]).
    pub fn encode(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, run: &Run) {
        for step in &run.steps {
            match *step {
                Step::Copy { from, to, size } => {
                    self.slot(device, from);
                    self.slot(device, to);
                    let (a, b) = (&self.slots[from as usize].as_ref().unwrap().0, &self.slots[to as usize].as_ref().unwrap().0);
                    encoder.copy_texture_to_texture(a.as_image_copy(), b.as_image_copy(), wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 });
                }
                Step::Pass { operation: _, source, source_size: _, target, size, original, mask, uniform } => {
                    let bind = self.bind(device, source, original, mask);
                    let view = self.slot(device, target).clone();
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("styk effect pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    pass.set_viewport(0.0, 0.0, size[0] as f32, size[1] as f32, 0.0, 1.0);
                    pass.set_scissor_rect(0, 0, size[0], size[1]);
                    pass.set_pipeline(&self.pipeline);
                    pass.set_bind_group(0, &bind, &[(uniform as u64 * UNIFORM_STRIDE) as u32]);
                    pass.draw(0..3, 0..1);
                    self.stats.passes += 1;
                }
            }
        }
    }
}

/// Records `stack` on a `size` layer held in slot 0, appending its pass uniforms to `records`.
fn record(records: &mut Vec<Uniform>, stack: &[Arc<Effect>], frame: Frame, size: [u32; 2]) -> Run {
    let mut r = Recorder { records, steps: Vec::new(), sizes: [[0, 0]; SLOTS], current: 0, brush: Brush::default(), frame, size };
    r.sizes[0] = size;
    for effect in stack.iter().filter(|e| styk_eval::vfx::is_active(e)) {
        r.effect(effect);
    }
    Run { steps: r.steps, result: r.current, size }
}

/// `run`'s local state while recording one stack.
struct Recorder<'a> {
    records: &'a mut Vec<Uniform>,
    steps: Vec<Step>,
    /// Size each slot holds in this run.
    sizes: [[u32; 2]; SLOTS],
    current: u8,
    brush: Brush,
    frame: Frame,
    /// The layer's size (`W`, `H`).
    size: [u32; 2],
}

impl Recorder<'_> {
    /// `pass(operation, p, q, original, width, height, mask)`.
    fn pass_with(&mut self, operation: i32, p: [f64; 4], q: [f64; 4], original: u8, size: [u32; 2], mask: u8) {
        let next = [1u8, 2, 3].into_iter().find(|&x| x != self.current && x != original && x != mask).unwrap_or(6);
        let source_size = self.sizes[self.current as usize];
        let f = |v: [f64; 4]| [v[0] as f32, v[1] as f32, v[2] as f32, v[3] as f32];
        let record = Uniform {
            dimensions: [source_size[0] as f32, source_size[1] as f32],
            origin: [self.frame.origin[0] as f32, self.frame.origin[1] as f32],
            output_size: [self.frame.output_size[0] as f32, self.frame.output_size[1] as f32],
            paint_type: self.brush.kind,
            stop_count: self.brush.count,
            paint_angle: self.brush.angle,
            operation,
            target: [size[0] as f32, size[1] as f32],
            texel: [0.0; 2],
            unused: [0.0; 2],
            p: f(p),
            q: f(q),
            stops: self.brush.colors,
            stop_offsets: self.brush.offsets,
        };
        let uniform = self.records.len() as u32;
        self.records.push(record);
        self.steps.push(Step::Pass { operation, source: self.current, source_size, target: next, size, original, mask, uniform });
        self.sizes[next as usize] = size;
        self.current = next;
    }

    fn pass(&mut self, operation: i32, p: [f64; 4], q: [f64; 4]) {
        let (current, size) = (self.current, self.size);
        self.pass_with(operation, p, q, current, size, BLANK);
    }

    fn pass_from(&mut self, operation: i32, p: [f64; 4], original: u8) {
        let size = self.size;
        self.pass_with(operation, p, [0.0; 4], original, size, BLANK);
    }

    /// `save(index)`: a copy of the current layer.
    fn save(&mut self, index: u8) -> u8 {
        self.steps.push(Step::Copy { from: self.current, to: index, size: self.size });
        self.sizes[index as usize] = self.size;
        index
    }

    /// `blur(radius)`: three box passes per axis, downsampled on a stable grid past 20 pixels.
    fn blur(&mut self, radius: f64) {
        if radius <= 0.0 {
            return;
        }
        let [w, h] = [self.size[0] as f64, self.size[1] as f64];
        let factor = (radius / 20.0).ceil().max(1.0);
        let modulo = |v: f64| ((v % factor) + factor) % factor;
        let (ox, oy) = if factor > 1.0 { (modulo(self.frame.origin[0]), modulo(self.frame.origin[1])) } else { (0.0, 0.0) };
        let bw = ((w + ox) / factor).ceil().max(1.0);
        let bh = ((h + oy) / factor).ceil().max(1.0);
        let small = [bw as u32, bh as u32];
        let current = self.current;
        if factor > 1.0 {
            self.pass_with(11, [factor, ox, oy, bh], [0.0; 4], current, small, BLANK);
        }
        let r = round(radius / factor * 0.57735).clamp(1.0, 128.0);
        for _ in 0..3 {
            let c = self.current;
            self.pass_with(3, [r, 0.0, 0.0, 0.0], [0.0; 4], c, small, BLANK);
            let c = self.current;
            self.pass_with(4, [r, 0.0, 0.0, 0.0], [0.0; 4], c, small, BLANK);
        }
        if factor > 1.0 {
            self.pass(12, [factor, ox, oy, h], [0.0; 4]);
        }
    }

    /// `morph(radius, rounded)`: alpha dilation (rounded in 16 directions, doubling).
    fn morph(&mut self, radius: f64, rounded: bool) {
        let mut radius = radius.ceil();
        let mut step = 1.0f64;
        while radius > 0.0 {
            let amount = (if rounded { step } else { 128.0 }).min(radius);
            if rounded {
                self.pass(14, [amount, 0.0, 0.0, 0.0], [0.0; 4]);
            } else {
                self.pass(5, [amount, 0.0, 0.0, 0.0], [0.0; 4]);
                self.pass(6, [amount, 0.0, 0.0, 0.0], [0.0; 4]);
            }
            radius -= amount;
            step *= 2.0;
        }
    }

    /// `paint(value)`: the brush of the passes that follow.
    fn paint(&mut self, color: &Color) {
        let b = &mut self.brush;
        match color {
            Color::Rgb(rgb) => {
                b.kind = 0;
                b.angle = 0.0;
                b.count = 1;
                b.colors[0] = [(rgb >> 16 & 255) as f32 / 255.0, (rgb >> 8 & 255) as f32 / 255.0, (rgb & 255) as f32 / 255.0, 1.0];
                b.offsets[0] = 0.0;
            }
            Color::Paint(paint) => {
                b.kind = match paint.kind {
                    PaintKind::Solid => 0,
                    PaintKind::Linear => 1,
                    PaintKind::Radial => 2,
                    PaintKind::Angular => 3,
                    PaintKind::Diamond => 4,
                };
                b.angle = (paint.angle * PI / 180.0) as f32;
                b.count = paint.stops.len() as i32;
                for (i, s) in paint.stops.iter().take(32).enumerate() {
                    b.colors[i] = [s.rgb[0] as f32 / 255.0, s.rgb[1] as f32 / 255.0, s.rgb[2] as f32 / 255.0, s.opacity as f32];
                    b.offsets[i] = s.offset as f32;
                }
            }
        }
    }

    fn effect(&mut self, effect: &Effect) {
        let params = &effect.params;
        let scale = self.frame.scale;
        let strength = params.effect_strength();
        let source = (strength < 1.0).then(|| self.save(7));
        let radians = |deg: f64| deg * PI / 180.0;
        match params {
            P::Tint(p) => {
                self.paint(&p.color);
                self.pass(1, [p.mix, 0.0, 0.0, 0.0], [0.0; 4]);
            }
            P::Grade(p) => self.pass(2, [p.exposure, p.contrast, p.saturation, p.temperature], [p.tint, p.shadows, p.highlights, p.gamma]),
            P::Blur(p) => {
                if p.mode == BlurMode::Gaussian {
                    self.blur(p.radius * scale);
                } else {
                    self.pass(16, [p.radius * scale, p.mode.index() as f64, radians(p.angle), 0.0], [p.center_x, p.center_y, 0.0, 0.0]);
                }
            }
            P::Glow(p) => {
                self.paint(&p.color);
                if p.core_mix != 0.0 {
                    self.paint(&p.core_color);
                    self.pass(1, [p.core_mix, 0.0, 0.0, 0.0], [0.0; 4]);
                    self.paint(&p.color);
                }
                let modes: &[bool] = match p.mode {
                    GlowPlacement::Both => &[true, false],
                    GlowPlacement::Inner => &[true],
                    GlowPlacement::Outer => &[false],
                };
                for &inner in modes {
                    let saved = self.save(4);
                    if inner {
                        self.pass(13, [0.0; 4], [0.0; 4]);
                    }
                    self.morph(p.spread * scale, true);
                    self.blur(p.radius * scale);
                    let size = self.size;
                    self.pass_with(7, [p.intensity, p.falloff, inner as u8 as f64, p.blend.index() as f64], [0.0; 4], saved, size, BLANK);
                }
            }
            P::Shadow(p) => {
                self.paint(&p.color);
                for &inner in if p.mode == ShadowPlacement::Inner { &[true][..] } else { &[false][..] } {
                    let saved = self.save(4);
                    if inner {
                        self.pass(13, [0.0; 4], [0.0; 4]);
                    }
                    self.morph(p.spread * scale, true);
                    self.blur(p.radius * scale);
                    let size = self.size;
                    self.pass_with(7, [p.opacity, 1.0, inner as u8 as f64, 0.0], [p.offset_x * scale, p.offset_y * scale, 0.0, 0.0], saved, size, BLANK);
                }
            }
            P::Outline(p) => {
                self.paint(&p.color);
                let saved = self.save(4);
                let center = p.mode == OutlinePlacement::Center;
                let width = p.width * scale * if center { 0.5 } else { 1.0 };
                if center {
                    self.pass(13, [0.0; 4], [0.0; 4]);
                    self.morph(width, p.rounded);
                    self.pass_from(15, [p.opacity, 1.0, 1.0, 0.0], saved);
                    let inner = self.save(5);
                    self.current = saved;
                    self.morph(width, p.rounded);
                    let size = self.size;
                    self.pass_with(25, [p.opacity, p.only as u8 as f64, 0.0, 0.0], [0.0; 4], saved, size, inner);
                } else {
                    let inside = p.mode == OutlinePlacement::Inside;
                    if inside {
                        self.pass(13, [0.0; 4], [0.0; 4]);
                    }
                    self.morph(width, p.rounded);
                    self.pass_from(15, [p.opacity, inside as u8 as f64, p.only as u8 as f64, 0.0], saved);
                }
            }
            P::Pixelate(p) => {
                let y = if p.linked { p.size } else { p.size_y };
                self.pass(8, [(p.size * scale).max(1.0), (y * scale).max(1.0), 0.0, 0.0], [p.offset_x * scale, p.offset_y * scale, 0.0, 0.0]);
            }
            P::Noise(p) => self.pass(
                9,
                [p.amount, (p.size * scale).max(1.0), p.seed as f64, (p.phase * 1024.0).floor()],
                [p.colored as u8 as f64, p.contrast, p.softness, p.pattern.index() as f64],
            ),
            P::Vignette(p) => {
                self.paint(&p.color);
                self.pass(10, [p.amount, p.softness, p.radius, 0.0], [p.center_x, p.center_y, p.aspect, p.roundness]);
            }
            P::Dissolve(p) => {
                self.paint(&p.color);
                self.pass(17, [p.progress, p.size * scale, p.softness, p.seed as f64], [p.edge, 0.0, 0.0, 0.0]);
            }
            P::RgbSplit(p) => self.pass(18, [p.amount * scale, radians(p.angle), 0.0, 0.0], [p.red, p.green, p.blue, 0.0]),
            P::Turbulence(p) => self.pass(27, [p.amount * scale, p.size * scale, p.evolution, p.seed as f64], [p.detail as f64, 0.0, 0.0, 0.0]),
            P::Wave(p) => self.pass(19, [p.strength * scale, p.wavelength * scale, radians(p.angle), radians(p.phase)], [0.0; 4]),
            P::Shockwave(p) => self.pass(20, [p.radius * scale, p.thickness * scale, p.strength * scale, p.falloff], [p.center_x, p.center_y, 0.0, 0.0]),
            P::Bloom(p) => {
                self.paint(&p.color);
                let saved = self.save(4);
                self.pass(21, [p.threshold, p.softness, 0.0, 0.0], [0.0; 4]);
                self.blur(p.radius * scale);
                self.pass_from(22, [p.intensity, 0.0, 0.0, 0.0], saved);
            }
            P::LightSweep(p) => {
                self.paint(&p.color);
                self.pass(23, [p.position, radians(p.angle), p.width, p.softness], [p.intensity, 0.0, 0.0, 0.0]);
            }
            P::Crt(p) => {
                self.paint(&p.color);
                self.pass(24, [p.amount, p.spacing * scale, p.curvature, p.chromatic * scale], [p.phase * scale, 0.0, 0.0, 0.0]);
            }
            // Generated effects draw artwork of their own; they never reach a filter stack.
            P::Particles(_) | P::Aura(_) | P::Beam(_) | P::Smoke(_) | P::Explosion(_) => {}
        }
        if let Some(source) = source {
            self.pass_from(26, [strength, 0.0, 0.0, 0.0], source);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passes(run: &Run) -> Vec<i32> {
        run.steps.iter().filter_map(|s| if let Step::Pass { operation, .. } = s { Some(*operation) } else { None }).collect()
    }

    /// A glow at half strength with a blur past 20 pixels, recorded pass for
    /// pass as `VfxWebGPU.run` records it.
    #[test]
    fn records_like_the_editor() {
        let glow = Effect { id: Arc::from("g"), enabled: true, params: P::Glow(styk_eval::vfx::registry::Glow { radius: 30.0, effect_strength: 0.5, ..Default::default() }) };
        let mut records = Vec::new();
        let run = record(&mut records, &[Arc::new(glow)], Frame { scale: 1.0, origin: [-7.0, 3.0], output_size: [100.0, 80.0] }, [120, 90]);
        // downsample, three box passes per axis, upsample, composite over the saved artwork, strength mix
        assert_eq!(passes(&run), vec![11, 3, 4, 3, 4, 3, 4, 12, 7, 26]);
        assert_eq!(run.steps.iter().filter(|s| matches!(s, Step::Copy { .. })).count(), 2, "the strength source and the saved artwork");
        assert_eq!(records.len(), 10);
        // The downsampled grid is anchored to the global origin: (120 + (-7 mod 2)) / 2.
        let Some(Step::Pass { size, .. }) = run.steps.iter().find(|s| matches!(s, Step::Pass { operation: 11, .. })) else { unreachable!() };
        assert_eq!(*size, [61, 46]);
        assert_eq!(run.size, [120, 90]);
    }

    /// Inactive effects record nothing; a centred outline keeps its inner ring in slot 5.
    #[test]
    fn centred_outline_uses_a_mask() {
        let outline = Effect { id: Arc::from("o"), enabled: true, params: P::Outline(styk_eval::vfx::registry::Outline { width: 4.0, mode: OutlinePlacement::Center, ..Default::default() }) };
        let off = Effect { id: Arc::from("x"), enabled: false, params: P::Tint(Default::default()) };
        let mut records = Vec::new();
        let run = record(&mut records, &[Arc::new(off), Arc::new(outline)], Frame { scale: 2.0, origin: [0.0; 2], output_size: [10.0; 2] }, [40, 40]);
        let ops = passes(&run);
        assert_eq!(ops.first(), Some(&13));
        assert_eq!(ops.last(), Some(&25));
        let Some(Step::Pass { mask, original, .. }) = run.steps.last() else { unreachable!() };
        assert_eq!((*mask, *original), (5, 4));
    }
}
