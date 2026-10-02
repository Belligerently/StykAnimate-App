//! wgpu renderer for StykAnimate draw lists.
//!
//! * One instanced draw per pass, whatever the primitive mix. Primitive records are
//!   instance attributes and the tables shaders look up (gradients, isolation group
//!   placements, polyline vertices) are float textures, so the same pipeline runs on
//!   WebGPU, Vulkan/Metal/DX12 and WebGL2, which has no storage buffers.
//! * Draw lists are view independent: pan/zoom only rewrites a 96-byte uniform.
//! * Translucent figures are isolated in an atlas: one pass per nesting depth
//!   (ping-ponging between two atlases), not one pass per figure.
//! * Sprites live in a texture array filled from images decoded on worker threads.
//! * Output is drawn into an offscreen `Rgba8Unorm` frame (gamma-space blending,
//!   like Canvas 2D) and copied into the host's surface.

use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use styk_eval::assets::{Assets, PAGE};
use styk_eval::doc::Document;
use styk_eval::drawlist::{DrawList, Prim};

pub use wgpu;

const SAMPLES: u32 = 4;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const MAX_LAYER_PAGES: u32 = 8;
const UNIFORM_STRIDE: u64 = 256;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuPrim {
    kind: u32,
    group: u32,
    color: u32,
    paint: u32,
    a: [f32; 4],
    b: [f32; 4],
    c: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct GpuGroup {
    origin: [f32; 2],
    scale: f32,
    page: f32,
    offset: [f32; 2],
    size: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct GpuView {
    m: [f32; 4],
    t: [f32; 4],
    inv: [f32; 4],
    inv_t: [f32; 4],
    target: [f32; 4],
    artboard: [f32; 4],
}

/// How the scene maps onto the output, in output pixels.
#[derive(Clone, Copy, Debug)]
pub struct ViewParams {
    /// World → output pixels: x' = a*x + c*y + e, y' = b*x + d*y + f.
    pub matrix: [f64; 6],
    pub artboard: [f64; 4],
    pub clip_to_artboard: bool,
    pub clear: [f64; 4],
}

impl ViewParams {
    fn gpu(&self, target: [f32; 4]) -> GpuView {
        let [a, b, c, d, e, f] = self.matrix;
        let det = a * d - b * c;
        let (ia, ib, ic, id) = (d / det, -b / det, -c / det, a / det);
        let ie = -(ia * e + ic * f);
        let i_f = -(ib * e + id * f);
        GpuView {
            m: [a as f32, b as f32, c as f32, d as f32],
            t: [e as f32, f as f32, det.abs().sqrt() as f32, 0.0],
            inv: [ia as f32, ib as f32, ic as f32, id as f32],
            inv_t: [ie as f32, i_f as f32, 0.0, 0.0],
            target,
            artboard: self.artboard.map(|v| v as f32),
        }
    }

    fn to_screen(&self, x: f64, y: f64) -> [f64; 2] {
        let [a, b, c, d, e, f] = self.matrix;
        [a * x + c * y + e, b * x + d * y + f]
    }
}

pub use styk_eval::assets::DecodedImage;

/// Decodes every image of a document into atlas-ready pixels (tools; the engine
/// decodes in the background instead).
pub fn decode_images(doc: &Document, assets: &Assets) -> Vec<DecodedImage> {
    (0..doc.images.len()).filter_map(|index| styk_eval::assets::decode_image(doc, assets, index)).collect()
}

struct LayerAtlas {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: [u32; 2],
    pages: u32,
}

pub struct Stats {
    pub prims: u32,
    pub groups: u32,
    pub passes: u32,
    pub downscaled: bool,
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    blit: wgpu::RenderPipeline,
    blit_layout: wgpu::BindGroupLayout,
    layout0: wgpu::BindGroupLayout,
    layout1: wgpu::BindGroupLayout,
    layout2: wgpu::BindGroupLayout,
    uniforms: wgpu::Buffer,
    uniform_slots: u64,
    prims: wgpu::Buffer,
    paints: Table,
    groups: Table,
    points: Table,
    bind0: Option<wgpu::BindGroup>,
    sprites: wgpu::Texture,
    sprite_bind: wgpu::BindGroup,
    nearest: wgpu::Sampler,
    linear: wgpu::Sampler,
    layers: [LayerAtlas; 2],
    layer_binds: [wgpu::BindGroup; 2],
    msaa_main: Option<(wgpu::Texture, [u32; 2])>,
    msaa_layer: Option<(wgpu::Texture, [u32; 2])>,
    output: Option<(wgpu::Texture, wgpu::TextureView, wgpu::BindGroup, [u32; 2])>,
    list: Option<Arc<DrawList>>,
    list_prims: u32,
    pub stats: Stats,
}

/// Texels per row of a lookup table (`TABLE_WIDTH` in prim.wgsl).
const TABLE_WIDTH: u32 = 1024;

fn table_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
        count: None,
    }
}

/// A table shaders index with `textureLoad`: `TABLE_WIDTH` texels per row, one record
/// (or half a record) per texel.
struct Table {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    rows: u32,
    format: wgpu::TextureFormat,
    texel_bytes: u32,
    label: &'static str,
    scratch: Vec<u8>,
}

impl Table {
    fn new(device: &wgpu::Device, label: &'static str, format: wgpu::TextureFormat, texel_bytes: u32) -> Table {
        let (texture, view) = Table::allocate(device, label, format, 1);
        Table { texture, view, rows: 1, format, texel_bytes, label, scratch: Vec::new() }
    }

    fn allocate(device: &wgpu::Device, label: &str, format: wgpu::TextureFormat, rows: u32) -> (wgpu::Texture, wgpu::TextureView) {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width: TABLE_WIDTH, height: rows, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        (texture, view)
    }

    /// Uploads `data` (whole texels). Returns whether the texture was replaced by a
    /// larger one, in which case bind groups that hold its view must be rebuilt.
    fn write(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, data: &[u8]) -> bool {
        let row_bytes = (TABLE_WIDTH * self.texel_bytes) as usize;
        let rows = data.len().div_ceil(row_bytes).max(1) as u32;
        let mut replaced = false;
        if rows > self.rows {
            self.rows = rows.next_power_of_two();
            (self.texture, self.view) = Table::allocate(device, self.label, self.format, self.rows);
            replaced = true;
        }
        if data.is_empty() {
            return replaced;
        }
        // The last row is padded: a texture write covers whole rows.
        let whole = data.len() / row_bytes * row_bytes;
        let copy = |bytes: &[u8], y: u32, height: u32| {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &self.texture, mip_level: 0, origin: wgpu::Origin3d { x: 0, y, z: 0 }, aspect: wgpu::TextureAspect::All },
                bytes,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row_bytes as u32), rows_per_image: Some(height) },
                wgpu::Extent3d { width: TABLE_WIDTH, height, depth_or_array_layers: 1 },
            );
        };
        if whole > 0 {
            copy(&data[..whole], 0, (whole / row_bytes) as u32);
        }
        if whole < data.len() {
            self.scratch.clear();
            self.scratch.extend_from_slice(&data[whole..]);
            self.scratch.resize(row_bytes, 0);
            copy(&self.scratch, (whole / row_bytes) as u32, 1);
        }
        replaced
    }
}

fn texture_entry(binding: u32, dimension: wgpu::TextureViewDimension) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: dimension, multisampled: false },
        count: None,
    }
}

fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

fn array_texture(device: &wgpu::Device, label: &str, size: [u32; 2], layers: u32, attachment: bool) -> (wgpu::Texture, wgpu::TextureView) {
    let mut usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST;
    if attachment {
        usage |= wgpu::TextureUsages::RENDER_ATTACHMENT;
    }
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: size[0].max(1), height: size[1].max(1), depth_or_array_layers: layers.max(1) },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });
    (texture, view)
}

fn buffer(device: &wgpu::Device, label: &str, size: u64, usage: wgpu::BufferUsages) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size: size.max(256), usage, mapped_at_creation: false })
}

/// Shelf packer for isolation layers inside fixed-size pages.
struct Shelf {
    size: [u32; 2],
    page: u32,
    x: u32,
    y: u32,
    row: u32,
}

impl Shelf {
    fn place(&mut self, w: u32, h: u32, max_pages: u32) -> Option<(u32, u32, u32)> {
        if w > self.size[0] || h > self.size[1] {
            return None;
        }
        if self.x + w > self.size[0] {
            self.x = 0;
            self.y += self.row;
            self.row = 0;
        }
        if self.y + h > self.size[1] {
            self.page += 1;
            self.x = 0;
            self.y = 0;
            self.row = 0;
        }
        if self.page >= max_pages {
            return None;
        }
        let at = (self.page, self.x, self.y);
        self.x += w;
        self.row = self.row.max(h);
        Some(at)
    }
}

impl Renderer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, present_format: wgpu::TextureFormat) -> Renderer {
        let device = device.clone();
        let queue = queue.clone();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("styk prims"),
            source: wgpu::ShaderSource::Wgsl(include_str!("prim.wgsl").into()),
        });
        let blit_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("styk blit"),
            source: wgpu::ShaderSource::Wgsl(include_str!("blit.wgsl").into()),
        });
        let layout0 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("styk scene"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<GpuView>() as u64),
                    },
                    count: None,
                },
                table_entry(1),
                table_entry(2),
                table_entry(3),
            ],
        });
        let layout1 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("styk sprites"),
            entries: &[texture_entry(0, wgpu::TextureViewDimension::D2Array), sampler_entry(1)],
        });
        let layout2 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("styk layers"),
            entries: &[texture_entry(0, wgpu::TextureViewDimension::D2Array)],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("styk prims"),
            bind_group_layouts: &[Some(&layout0), Some(&layout1), Some(&layout2)],
            immediate_size: 0,
        });
        let premultiplied = wgpu::BlendState {
            color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add },
            alpha: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add },
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("styk prims"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<GpuPrim>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![0 => Uint32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4],
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState { count: SAMPLES, mask: !0, alpha_to_coverage_enabled: false },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: FORMAT, blend: Some(premultiplied), write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let blit_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("styk blit"),
            entries: &[texture_entry(0, wgpu::TextureViewDimension::D2), sampler_entry(1)],
        });
        let blit_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("styk blit"),
            bind_group_layouts: &[Some(&blit_layout)],
            immediate_size: 0,
        });
        let blit = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("styk blit"),
            layout: Some(&blit_pipeline_layout),
            vertex: wgpu::VertexState { module: &blit_shader, entry_point: Some("vs_blit"), compilation_options: Default::default(), buffers: &[] },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &blit_shader,
                entry_point: Some(if present_format.is_srgb() { "fs_blit_srgb" } else { "fs_blit" }),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: present_format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let nearest = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("styk nearest"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let linear = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("styk linear"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let (sprites, sprite_view) = array_texture(&device, "styk sprites", [1, 1], 1, false);
        let sprite_bind = Self::sprite_bind(&device, &layout1, &sprite_view, &linear);
        let make_layers = |label: &str| {
            let (texture, view) = array_texture(&device, label, [1, 1], 1, true);
            LayerAtlas { texture, view, size: [1, 1], pages: 1 }
        };
        let layers = [make_layers("styk layers 0"), make_layers("styk layers 1")];
        let layer_binds = [Self::layer_bind(&device, &layout2, &layers[0].view), Self::layer_bind(&device, &layout2, &layers[1].view)];
        Renderer {
            uniforms: buffer(&device, "styk views", UNIFORM_STRIDE * 8, wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST),
            uniform_slots: 8,
            prims: buffer(&device, "styk prims", 64 * 1024, wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST),
            paints: Table::new(&device, "styk paints", wgpu::TextureFormat::Rgba32Float, 16),
            groups: Table::new(&device, "styk groups", wgpu::TextureFormat::Rgba32Float, 16),
            points: Table::new(&device, "styk polyline points", wgpu::TextureFormat::Rg32Float, 8),
            bind0: None,
            device,
            queue,
            pipeline,
            blit,
            blit_layout,
            layout0,
            layout1,
            layout2,
            sprites,
            sprite_bind,
            nearest,
            linear,
            layers,
            layer_binds,
            msaa_main: None,
            msaa_layer: None,
            output: None,
            list: None,
            list_prims: 0,
            stats: Stats { prims: 0, groups: 0, passes: 0, downscaled: false },
        }
    }

    fn sprite_bind(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, view: &wgpu::TextureView, linear: &wgpu::Sampler) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("styk sprites"),
            layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(linear) },
            ],
        })
    }

    fn layer_bind(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, view: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("styk layers"),
            layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) }],
        })
    }

    /// Allocates the sprite atlas for a document (pages from its asset layout).
    pub fn prepare_sprites(&mut self, assets: &Assets) {
        let pages = assets.pages.max(1);
        let size = if assets.pages == 0 { [1, 1] } else { [PAGE, PAGE] };
        let (texture, view) = array_texture(&self.device, "styk sprites", size, pages, false);
        self.sprites = texture;
        self.sprite_bind = Self::sprite_bind(&self.device, &self.layout1, &view, &self.linear);
    }

    /// Uploads decoded images into the atlas prepared by [`prepare_sprites`].
    pub fn upload_images<'a>(&mut self, assets: &Assets, images: impl IntoIterator<Item = &'a DecodedImage>) {
        if assets.pages == 0 {
            return;
        }
        for img in images {
            let Some(e) = assets.atlas.get(img.index as usize) else { continue };
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.sprites,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: e.x - 1, y: e.y - 1, z: e.layer },
                    aspect: wgpu::TextureAspect::All,
                },
                &img.rgba,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(img.width * 4), rows_per_image: Some(img.height) },
                wgpu::Extent3d { width: img.width, height: img.height, depth_or_array_layers: 1 },
            );
        }
    }

    /// Uploads a draw list unless it is already resident.
    pub fn set_list(&mut self, list: &Arc<DrawList>) {
        if self.list.as_ref().is_some_and(|l| Arc::ptr_eq(l, list)) {
            return;
        }
        let prims: &[GpuPrim] = bytemuck::cast_slice(prims_bytes(&list.prims));
        let need = std::mem::size_of_val(prims) as u64;
        if need > self.prims.size() {
            self.prims = buffer(&self.device, "styk prims", need.next_power_of_two(), wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST);
        }
        if !prims.is_empty() {
            self.queue.write_buffer(&self.prims, 0, bytemuck::cast_slice(prims));
        }
        let mut rebind = self.bind0.is_none();
        rebind |= self.paints.write(&self.device, &self.queue, bytemuck::cast_slice(&list.paints));
        rebind |= self.points.write(&self.device, &self.queue, bytemuck::cast_slice(&list.points));
        if rebind {
            self.rebind();
        }
        self.list_prims = list.prims.len() as u32;
        self.list = Some(list.clone());
    }

    fn rebind(&mut self) {
        self.bind0 = Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("styk scene"),
            layout: &self.layout0,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.uniforms,
                        offset: 0,
                        size: wgpu::BufferSize::new(std::mem::size_of::<GpuView>() as u64),
                    }),
                },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&self.paints.view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&self.groups.view) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&self.points.view) },
            ],
        }));
    }

    fn ensure_output(&mut self, size: [u32; 2]) {
        if self.output.as_ref().is_some_and(|o| o.3 == size) {
            return;
        }
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("styk frame"),
            size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("styk blit"),
            layout: &self.blit_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.nearest) },
            ],
        });
        self.output = Some((texture, view, bind, size));
    }

    fn msaa(device: &wgpu::Device, slot: &mut Option<(wgpu::Texture, [u32; 2])>, size: [u32; 2]) -> wgpu::TextureView {
        if slot.as_ref().is_none_or(|s| s.1 != size) {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("styk msaa"),
                size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: SAMPLES,
                dimension: wgpu::TextureDimension::D2,
                format: FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            *slot = Some((texture, size));
        }
        slot.as_ref().unwrap().0.create_view(&Default::default())
    }

    fn ensure_layers(&mut self, parity: usize, size: [u32; 2], pages: u32) {
        let atlas = &self.layers[parity];
        if atlas.size == size && atlas.pages >= pages {
            return;
        }
        let pages = pages.max(atlas.pages).min(MAX_LAYER_PAGES);
        let (texture, view) = array_texture(&self.device, "styk layers", size, pages, true);
        self.layer_binds[parity] = Self::layer_bind(&self.device, &self.layout2, &view);
        self.layers[parity] = LayerAtlas { texture, view, size, pages };
    }

    /// Renders the current draw list into the offscreen frame.
    pub fn render(&mut self, encoder: &mut wgpu::CommandEncoder, view: &ViewParams, size: [u32; 2]) {
        let size = [size[0].max(1), size[1].max(1)];
        self.ensure_output(size);
        let Some(list) = self.list.clone() else {
            self.clear_only(encoder, view);
            return;
        };
        if self.bind0.is_none() {
            self.rebind();
        }
        // Place every isolation group: screen rect (clipped to the frame) → atlas rect.
        let mut placed = vec![GpuGroup::default(); list.groups.len()];
        let max_depth = list.groups.iter().map(|g| g.depth).max().unwrap_or(0);
        let mut pages_per_depth = vec![0u32; max_depth as usize + 1];
        let mut downscaled = false;
        for depth in 1..=max_depth {
            let members: Vec<usize> = (1..list.groups.len()).filter(|&g| list.groups[g].depth == depth).collect();
            let mut rects: Vec<(usize, [f64; 4])> = Vec::with_capacity(members.len());
            for &g in &members {
                let b = list.groups[g].bounds;
                let corners = [
                    view.to_screen(b[0] as f64, b[1] as f64),
                    view.to_screen(b[2] as f64, b[1] as f64),
                    view.to_screen(b[0] as f64, b[3] as f64),
                    view.to_screen(b[2] as f64, b[3] as f64),
                ];
                let x0 = corners.iter().map(|c| c[0]).fold(f64::INFINITY, f64::min).floor() - 2.0;
                let y0 = corners.iter().map(|c| c[1]).fold(f64::INFINITY, f64::min).floor() - 2.0;
                let x1 = corners.iter().map(|c| c[0]).fold(f64::NEG_INFINITY, f64::max).ceil() + 2.0;
                let y1 = corners.iter().map(|c| c[1]).fold(f64::NEG_INFINITY, f64::max).ceil() + 2.0;
                let r = [x0.max(0.0), y0.max(0.0), x1.min(size[0] as f64), y1.min(size[1] as f64)];
                rects.push((g, r));
            }
            let mut scale = 1.0f64;
            loop {
                let mut shelf = Shelf { size, page: 0, x: 0, y: 0, row: 0 };
                let mut order: Vec<usize> = (0..rects.len()).collect();
                order.sort_by(|&a, &b| (rects[b].1[3] - rects[b].1[1]).total_cmp(&(rects[a].1[3] - rects[a].1[1])));
                let mut ok = true;
                for k in order {
                    let (g, r) = rects[k];
                    if r[2] <= r[0] || r[3] <= r[1] {
                        placed[g] = GpuGroup { page: -1.0, ..Default::default() };
                        continue;
                    }
                    let w = ((r[2] - r[0]) * scale).ceil().max(1.0) as u32;
                    let h = ((r[3] - r[1]) * scale).ceil().max(1.0) as u32;
                    match shelf.place(w, h, MAX_LAYER_PAGES) {
                        Some((page, x, y)) => {
                            placed[g] = GpuGroup {
                                origin: [r[0] as f32, r[1] as f32],
                                scale: scale as f32,
                                page: page as f32,
                                offset: [x as f32, y as f32],
                                size: [w as f32, h as f32],
                            };
                        }
                        None => {
                            ok = false;
                            break;
                        }
                    }
                }
                if ok {
                    pages_per_depth[depth as usize] = shelf.page + 1;
                    break;
                }
                scale *= 0.5;
                downscaled = true;
            }
        }
        if self.groups.write(&self.device, &self.queue, bytemuck::cast_slice(&placed)) {
            self.rebind();
        }
        // One uniform slot per pass: every (depth, page) and the frame.
        let mut passes: Vec<(u32, u32, u32, u32)> = Vec::new(); // depth, page, start, end
        for pass in &list.passes {
            if pass.depth == 0 {
                continue;
            }
            for page in 0..pages_per_depth[pass.depth as usize] {
                passes.push((pass.depth, page, pass.start, pass.end));
            }
        }
        let root = list.passes.iter().find(|p| p.depth == 0).map(|p| (p.start, p.end)).unwrap_or((0, 0));
        let slots = passes.len() as u64 + 1;
        if slots > self.uniform_slots {
            self.uniform_slots = slots.next_power_of_two();
            self.uniforms = buffer(&self.device, "styk views", UNIFORM_STRIDE * self.uniform_slots, wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST);
            self.rebind();
        }
        let mut uniform_data = vec![0u8; (UNIFORM_STRIDE * slots) as usize];
        for (i, &(depth, page, _, _)) in passes.iter().enumerate() {
            let v = view.gpu([size[0] as f32, size[1] as f32, page as f32, depth as f32]);
            let o = i * UNIFORM_STRIDE as usize;
            uniform_data[o..o + std::mem::size_of::<GpuView>()].copy_from_slice(bytemuck::bytes_of(&v));
        }
        let v = view.gpu([size[0] as f32, size[1] as f32, 0.0, 0.0]);
        let o = passes.len() * UNIFORM_STRIDE as usize;
        uniform_data[o..o + std::mem::size_of::<GpuView>()].copy_from_slice(bytemuck::bytes_of(&v));
        self.queue.write_buffer(&self.uniforms, 0, &uniform_data);

        for depth in 1..=max_depth {
            let pages = pages_per_depth[depth as usize];
            if pages > 0 {
                self.ensure_layers(depth as usize % 2, size, pages);
            }
        }
        let bind0 = self.bind0.clone().unwrap();
        for (i, &(depth, page, start, end)) in passes.iter().enumerate() {
            if start == end {
                continue;
            }
            let parity = depth as usize % 2;
            let target = self.layers[parity].texture.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2),
                base_array_layer: page,
                array_layer_count: Some(1),
                ..Default::default()
            });
            let msaa = Self::msaa(&self.device, &mut self.msaa_layer, size);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("styk layer pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &msaa,
                    depth_slice: None,
                    resolve_target: Some(&target),
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Discard },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind0, &[(i as u64 * UNIFORM_STRIDE) as u32]);
            pass.set_bind_group(1, &self.sprite_bind, &[]);
            pass.set_bind_group(2, &self.layer_binds[(depth as usize + 1) % 2], &[]);
            pass.set_vertex_buffer(0, self.prims.slice(..));
            pass.draw(0..6, start..end);
        }
        // The frame.
        let msaa = Self::msaa(&self.device, &mut self.msaa_main, size);
        let output = &self.output.as_ref().unwrap().1;
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("styk frame pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &msaa,
                depth_slice: None,
                resolve_target: Some(output),
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color { r: view.clear[0], g: view.clear[1], b: view.clear[2], a: view.clear[3] }),
                    store: wgpu::StoreOp::Discard,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bind0, &[(passes.len() as u64 * UNIFORM_STRIDE) as u32]);
        pass.set_bind_group(1, &self.sprite_bind, &[]);
        pass.set_bind_group(2, &self.layer_binds[1], &[]);
        pass.set_vertex_buffer(0, self.prims.slice(..));
        let bg = list.background_prims.min(root.1 - root.0);
        if bg > 0 {
            pass.draw(0..6, root.0..root.0 + bg);
        }
        if view.clip_to_artboard {
            let a = view.artboard;
            let x0 = a[0].clamp(0.0, size[0] as f64).floor() as u32;
            let y0 = a[1].clamp(0.0, size[1] as f64).floor() as u32;
            let x1 = a[2].clamp(0.0, size[0] as f64).ceil() as u32;
            let y1 = a[3].clamp(0.0, size[1] as f64).ceil() as u32;
            if x1 > x0 && y1 > y0 {
                pass.set_scissor_rect(x0, y0, x1 - x0, y1 - y0);
                pass.draw(0..6, root.0 + bg..root.1);
            }
        } else {
            pass.draw(0..6, root.0 + bg..root.1);
        }
        drop(pass);
        self.stats = Stats { prims: list.prims.len() as u32, groups: list.groups.len() as u32 - 1, passes: passes.len() as u32 + 1, downscaled };
    }

    fn clear_only(&mut self, encoder: &mut wgpu::CommandEncoder, view: &ViewParams) {
        let output = &self.output.as_ref().unwrap().1;
        let _ = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("styk clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color { r: view.clear[0], g: view.clear[1], b: view.clear[2], a: view.clear[3] }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }

    /// Draws the rendered frame into the host's render pass (viewport already set).
    pub fn present(&self, pass: &mut wgpu::RenderPass<'_>) {
        let Some((_, _, bind, _)) = &self.output else { return };
        pass.set_pipeline(&self.blit);
        pass.set_bind_group(0, bind, &[]);
        pass.draw(0..3, 0..1);
    }

    /// The offscreen frame (for screenshots/tests).
    pub fn frame_texture(&self) -> Option<&wgpu::Texture> {
        self.output.as_ref().map(|o| &o.0)
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }
}

fn prims_bytes(prims: &[Prim]) -> &[u8] {
    // `Prim` and `GpuPrim` share one #[repr(C)] layout of 16 four-byte fields.
    const _: () = assert!(std::mem::size_of::<Prim>() == std::mem::size_of::<GpuPrim>());
    unsafe { std::slice::from_raw_parts(prims.as_ptr() as *const u8, std::mem::size_of_val(prims)) }
}

/// Reads back the offscreen frame as RGBA8 rows (tests and screenshots).
pub fn read_frame(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) -> (u32, u32, Vec<u8>) {
    let (w, h) = (texture.width(), texture.height());
    let row = (w * 4).div_ceil(256) * 256;
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("styk readback"),
        size: (row * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo { buffer: &staging, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) } },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    queue.submit([encoder.finish()]);
    let slice = staging.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    let data = slice.get_mapped_range().expect("mapped readback");
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        out.extend_from_slice(&data[(y * row) as usize..(y * row + w * 4) as usize]);
    }
    drop(data);
    staging.unmap();
    (w, h, out)
}
