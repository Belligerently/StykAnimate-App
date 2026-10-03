//! Export frames drawn by the GPU renderer and read back, tile by tile.
//!
//! The host owns the device (a worker's WebGPU or WebGL2 device in the app, a native
//! adapter in tools) and waits for each read-back its own way: `map_async` and the
//! event loop in a browser, [`GpuExport::render_blocking`] natively.

use std::sync::Arc;

use styk_draw::DrawList;
use styk_eval::assets::{Assets, DecodedImage};
use styk_eval::doc::{Camera, Document};
use styk_render::{wgpu, Renderer};

use crate::pixels::resolve;
use crate::settings::Settings;
use crate::tiles::{tiles, Tile};
use crate::view::tile_view;

pub struct GpuExport {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: Renderer,
    settings: Settings,
    tiles: Vec<Tile>,
    /// Where a drawn tile is copied for reading: `stride` bytes a row (256-byte aligned).
    readback: wgpu::Buffer,
    stride: u32,
    /// The document lineage and page count the sprite atlas is laid out for.
    atlas: Option<(u64, u32)>,
    /// The last resolved tile, straight RGBA.
    resolved: Vec<u8>,
}

impl GpuExport {
    /// An exporter of `settings` frames drawing tiles of at most `max_tile` pixels a side
    /// (also within the device's texture limit).
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, settings: Settings, max_tile: u32) -> GpuExport {
        let side = max_tile.min(device.limits().max_texture_dimension_2d).max(2);
        let tiles = tiles(settings.render_size(), side, settings.smoothing.factor());
        let size = tiles[0].size;
        let stride = (size[0] * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("styk export read-back"),
            size: stride as u64 * size[1] as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let renderer = Renderer::with_edges(device, queue, wgpu::TextureFormat::Rgba8Unorm, settings.smoothing.edges());
        GpuExport { device: device.clone(), queue: queue.clone(), renderer, settings, tiles, readback, stride, atlas: None, resolved: Vec::new() }
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn tiles(&self) -> &[Tile] {
        &self.tiles
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// Lays the sprite atlas out for a document's `assets` (`lineage`: the document's
    /// identity across revisions). Returns whether the atlas was allocated anew, in which case
    /// every image must be uploaded again.
    pub fn prepare_sprites(&mut self, lineage: u64, assets: &Assets) -> bool {
        let layout = (lineage, assets.pages);
        if self.atlas == Some(layout) {
            return false;
        }
        self.renderer.prepare_sprites(assets);
        self.atlas = Some(layout);
        true
    }

    pub fn upload_images<'a>(&mut self, assets: &Assets, images: impl IntoIterator<Item = &'a DecodedImage>) {
        self.renderer.upload_images(assets, images);
    }

    /// Draws tile `index` of `list` seen through `camera`, and copies it for reading.
    pub fn draw(&mut self, doc: &Document, list: &Arc<DrawList>, camera: Option<&Camera>, index: usize) {
        let tile = self.tiles[index];
        let view = tile_view(doc, camera, &self.settings, &tile);
        self.renderer.set_list(list);
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("styk export tile") });
        self.renderer.render(&mut encoder, &view, tile.size);
        let texture = self.renderer.frame_texture().expect("render() allocates the frame");
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo { buffer: &self.readback, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(self.stride), rows_per_image: Some(tile.size[1]) } },
            wgpu::Extent3d { width: tile.size[0], height: tile.size[1], depth_or_array_layers: 1 },
        );
        self.queue.submit([encoder.finish()]);
    }

    /// The buffer a drawn tile is copied into: map it for reading, then [`GpuExport::resolve`].
    pub fn readback(&self) -> &wgpu::Buffer {
        &self.readback
    }

    /// Converts the mapped read-back of tile `index` into output pixels (straight RGBA,
    /// `tile.valid / factor` pixels, rows packed; over black with `matte`) and unmaps it.
    pub fn resolve(&mut self, index: usize, matte: bool) -> Result<&[u8], String> {
        let tile = self.tiles[index];
        {
            let data = self.readback.slice(..).get_mapped_range().map_err(|e| format!("the export read-back is not mapped: {e:?}"))?;
            resolve(&data, self.stride as usize, tile.valid, self.settings.smoothing.factor(), matte, &mut self.resolved);
        }
        self.readback.unmap();
        Ok(&self.resolved)
    }

    /// Draws, waits for the GPU and resolves tile `index`. Native hosts only: a browser's
    /// device delivers the mapping through its event loop.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn render_blocking(&mut self, doc: &Document, list: &Arc<DrawList>, camera: Option<&Camera>, index: usize, matte: bool) -> Result<&[u8], String> {
        self.draw(doc, list, camera, index);
        let (sender, receiver) = std::sync::mpsc::channel();
        self.readback.slice(..).map_async(wgpu::MapMode::Read, move |result| {
            let _ = sender.send(result);
        });
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        receiver.recv().map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
        self.resolve(index, matte)
    }

    /// Renders a whole frame natively into straight RGBA of the output size.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn frame_blocking(&mut self, doc: &Document, list: &Arc<DrawList>, camera: Option<&Camera>, matte: bool) -> Result<Vec<u8>, String> {
        let (width, height) = (self.settings.width as usize, self.settings.height as usize);
        let factor = self.settings.smoothing.factor();
        let mut out = vec![0u8; width * height * 4];
        for index in 0..self.tiles.len() {
            let tile = self.tiles[index];
            let pixels = self.render_blocking(doc, list, camera, index, matte)?;
            place(&mut out, width, &tile, factor, pixels);
        }
        Ok(out)
    }

    /// The renderer's statistics for the last tile drawn.
    pub fn stats(&self) -> &styk_render::Stats {
        &self.renderer.stats
    }
}

/// Copies a resolved tile (`tile.valid / factor` pixels, rows packed) into the output frame
/// `out` (`width` pixels a row).
pub fn place(out: &mut [u8], width: usize, tile: &Tile, factor: u32, pixels: &[u8]) {
    let f = factor.max(1) as usize;
    let (x, y) = (tile.x as usize / f, tile.y as usize / f);
    let w = tile.valid[0] as usize / f;
    for (row, line) in pixels.chunks_exact(w * 4).enumerate() {
        let at = ((y + row) * width + x) * 4;
        out[at..at + w * 4].copy_from_slice(line);
    }
}
