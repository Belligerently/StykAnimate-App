//! Export frames drawn in a worker of their own (`src/engine/engine-export-worker.mjs`).
//!
//! The worker joins the engine's shared memory like the preparation workers, but keeps
//! its event loop: it holds a GPU device of its own (WebGPU, else WebGL2 on an
//! `OffscreenCanvas`), so nothing of an export is drawn or read on the interface thread
//! (D7). Frames come from the document of the export's revision ([`ExportFrames`]); the
//! pose the editor shows comes as keyframe JSON, drawn as the stage draws its edit frame
//! but without onion skins.

use std::sync::{Arc, Mutex};

use styk_draw::DrawList;
use styk_engine::edit::{EditOptions, EditScene};
use styk_engine::export::{ExportFrames, AHEAD};
use styk_engine::Engine;
use styk_eval::doc::Camera;
use styk_export::{GpuExport, Settings, Smoothing};
use styk_render::wgpu;
use wasm_bindgen::prelude::*;

use crate::{clock, js_error};

/// Decoded pictures uploaded per call at most, so a document with many pictures
/// reports progress instead of stalling the worker in one call.
const IMAGE_UPLOAD_BYTES: usize = 32 << 20;

#[wasm_bindgen]
pub struct ExportRenderer {
    frames: ExportFrames,
    gpu: GpuExport,
    device: wgpu::Device,
    /// WebGL2 draws through a context of this surface; WebGPU needs none.
    _surface: Option<wgpu::Surface<'static>>,
    backend: String,
    /// The pose the editor shows, when the export is of it.
    edit: EditScene,
    /// The frame being exported: its draw list and the camera it is seen through.
    current: Option<(Arc<DrawList>, Option<Camera>)>,
    /// What the device reported: lost, out of memory or a validation error.
    failure: Arc<Mutex<Option<(bool, String)>>>,
    frames_drawn: u32,
    last_evaluate_ms: f64,
    /// Starting: finding an adapter, getting its device, creating the renderer (ms).
    start_ms: [f64; 3],
}

fn adapter_options<'a, 'b>(surface: Option<&'a wgpu::Surface<'b>>) -> wgpu::RequestAdapterOptions<'a, 'b> {
    wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, compatible_surface: surface, ..Default::default() }
}

#[wasm_bindgen]
impl ExportRenderer {
    /// Starts an export of `width`×`height` frames sampled as `smoothing` (`edges`, `none`,
    /// `sprites`), in tiles of at most `tile` pixels. `address` comes from
    /// `EngineHandle::address` and is consumed. Fails when the engine does not hold the
    /// document of `revision`, or when no GPU device can be had (`canvas` gives WebGL2 a
    /// context where the worker has no WebGPU).
    #[allow(clippy::too_many_arguments)]
    pub async fn create(address: usize, revision: f64, width: u32, height: u32, smoothing: String, max_pixels: f64, tile: u32, canvas: Option<web_sys::OffscreenCanvas>) -> Result<ExportRenderer, JsError> {
        // SAFETY: `address` was produced by `EngineHandle::address` in this memory and is used once.
        let engine = unsafe { *Box::from_raw(address as *mut Engine) };
        let smoothing = Smoothing::parse(&smoothing).ok_or_else(|| js_error("choose Smooth edges, No smoothing or Smooth sprites"))?;
        let settings = Settings::new(width, height, smoothing, max_pixels as u64).map_err(js_error)?;
        let frames = engine.export_frames(revision as u64).map_err(js_error)?;
        let started = clock();
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = wgpu::Backends::BROWSER_WEBGPU | wgpu::Backends::GL;
        let instance = wgpu::util::new_instance_with_webgpu_detection(descriptor).await;
        let (adapter, surface) = match instance.request_adapter(&adapter_options(None)).await {
            Ok(adapter) => (adapter, None),
            Err(first) => {
                let canvas = canvas.ok_or_else(|| js_error(format!("no GPU adapter in this worker ({first})")))?;
                let surface = instance.create_surface(wgpu::SurfaceTarget::OffscreenCanvas(canvas)).map_err(js_error)?;
                let adapter = instance.request_adapter(&adapter_options(Some(&surface))).await.map_err(|e| js_error(format!("no GPU adapter in this worker ({e})")))?;
                (adapter, Some(surface))
            }
        };
        let info = adapter.get_info();
        let adapted = clock();
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("styk export"),
                required_limits: wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
                ..Default::default()
            })
            .await
            .map_err(js_error)?;
        let deviced = clock();
        let failure: Arc<Mutex<Option<(bool, String)>>> = Arc::default();
        let lost = failure.clone();
        device.set_device_lost_callback(move |_, message| {
            lost.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert((true, format!("the GPU device was lost: {message}")));
        });
        let errors = failure.clone();
        device.on_uncaptured_error(Arc::new(move |error: wgpu::Error| {
            let memory = matches!(error, wgpu::Error::OutOfMemory { .. });
            errors.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert((memory, error.to_string()));
        }));
        let mut gpu = GpuExport::new(&device, &queue, settings, tile);
        let generation = frames.generation().clone();
        gpu.prepare_sprites(generation.lineage, &generation.assets);
        frames.decode_images();
        Ok(ExportRenderer {
            frames,
            gpu,
            device,
            _surface: surface,
            backend: format!("{:?} · {}", info.backend, info.name),
            edit: EditScene::default(),
            current: None,
            failure,
            frames_drawn: 0,
            last_evaluate_ms: 0.0,
            start_ms: [adapted - started, deviced - adapted, clock() - deviced],
        })
    }

    /// The graphics backend and adapter, for reports.
    pub fn backend(&self) -> String {
        self.backend.clone()
    }

    /// Uploads pictures decoded so far and returns how many are still to come. A frame is
    /// drawn only once every picture is in (an export never shows a frame without one).
    pub fn upload_images(&mut self) -> u32 {
        let generation = self.frames.generation().clone();
        let images = self.frames.take_images(IMAGE_UPLOAD_BYTES);
        if !images.is_empty() {
            self.gpu.upload_images(&generation.assets, images.iter().map(|image| &**image));
        }
        self.frames.images_pending() as u32
    }

    /// Makes `tick` the frame to draw, and asks the engine's workers for the next ones up to
    /// `last`. `false` while a worker is still preparing it: wait for the engine's progress.
    /// Fails for a tick the document does not have.
    pub fn prepare(&mut self, tick: u32, last: u32) -> Result<bool, JsError> {
        if tick >= self.frames.ticks() {
            return Err(js_error(format!("Choose an export frame inside this animation (tick {tick} of {}).", self.frames.ticks())));
        }
        let started = clock();
        let Some(frame) = self.frames.take(tick) else { return Ok(false) };
        self.last_evaluate_ms = clock() - started;
        self.current = Some((frame.list.clone(), frame.camera));
        let ahead = (tick + 1..=last.min(tick + AHEAD as u32)).collect::<Vec<_>>();
        self.frames.prefetch(ahead);
        Ok(true)
    }

    /// Makes the editor's pose the frame to draw: keyframe JSON whose `objects` are poses,
    /// with the times it carries (`_vfxTimelineTick`, `_cameraMotionTick`).
    pub fn prepare_pose(&mut self, json: &str) -> Result<(), JsError> {
        let started = clock();
        let generation = self.frames.generation().clone();
        self.edit.update(&generation, json.as_bytes()).map_err(js_error)?;
        // Every earlier anchor of generated effects is evaluated: an export has no frame budget.
        let options = EditOptions { camera: true, clock: None, anchor_budget_ms: f64::INFINITY, ..Default::default() };
        let list = self.edit.draw_list(&generation, &options).map_err(js_error)?.ok_or_else(|| js_error("the pose has no frame"))?;
        self.current = Some((list, self.edit.camera()));
        self.last_evaluate_ms = clock() - started;
        Ok(())
    }

    /// Tiles of a frame.
    pub fn tiles(&self) -> u32 {
        self.gpu.tiles().len() as u32
    }

    /// Read-back slots: tiles that can wait to be read at once (the worker draws ahead into
    /// free ones while earlier tiles are mapped).
    pub fn slots(&self) -> u32 {
        self.gpu.slots() as u32
    }

    /// Where tile `index` lands in the output and its pixels there: x, y, width, height of
    /// the part inside the frame, then the width and height every tile has.
    pub fn tile_rect(&self, index: u32) -> Box<[u32]> {
        let tile = self.gpu.tiles()[index as usize];
        let f = self.gpu.settings().smoothing.factor();
        Box::new([tile.x / f, tile.y / f, tile.valid[0] / f, tile.valid[1] / f, tile.size[0] / f, tile.size[1] / f])
    }

    /// Draws tile `index` of the prepared frame into read-back `slot`; the promise resolves once
    /// its pixels can be read.
    pub fn draw_tile(&mut self, index: u32, slot: u32) -> Result<js_sys::Promise, JsError> {
        self.check()?;
        let generation = self.frames.generation().clone();
        let (list, camera) = self.current.clone().ok_or_else(|| js_error("prepare a frame before drawing it"))?;
        self.gpu.draw(&generation.prepared.doc, &list, camera.as_ref(), index as usize, slot as usize);
        let readback = self.gpu.readback(slot as usize).clone();
        let promise = js_sys::Promise::new(&mut |resolve, reject| {
            readback.slice(..).map_async(wgpu::MapMode::Read, move |result| {
                let _ = match result {
                    Ok(()) => resolve.call0(&JsValue::UNDEFINED),
                    Err(error) => reject.call1(&JsValue::UNDEFINED, &JsValue::from_str(&format!("the export frame could not be read: {error}"))),
                };
            });
        });
        // WebGL2 maps on the next poll; WebGPU through the mapping's own promise.
        let _ = self.device.poll(wgpu::PollType::Poll);
        Ok(promise)
    }

    /// Writes tile `index`, read into `slot`, into `out` (the RGBA of a tile-sized `ImageData`,
    /// rows of the tile's full width), straight alpha, over black with `matte`.
    pub fn resolve_tile(&mut self, index: u32, slot: u32, matte: bool, out: &js_sys::Uint8ClampedArray) -> Result<(), JsError> {
        let tile = self.gpu.tiles()[index as usize];
        let f = self.gpu.settings().smoothing.factor();
        let (row, full) = ((tile.valid[0] / f * 4) as usize, tile.size[0] / f * 4);
        let pixels = self.gpu.resolve(index as usize, slot as usize, matte).map_err(js_error)?;
        if out.length() < full * (tile.size[1] / f) {
            return Err(js_error("the tile's pixels do not fit"));
        }
        // SAFETY: the view is used at once, before anything here can allocate.
        let view = unsafe { js_sys::Uint8Array::view(pixels) };
        for (y, start) in (0..pixels.len()).step_by(row.max(1)).enumerate() {
            out.set(&view.subarray(start as u32, (start + row) as u32), y as u32 * full);
        }
        self.frames_drawn += (index + 1 == self.gpu.tiles().len() as u32) as u32;
        Ok(())
    }

    /// Gives back read-back `slot`, mapped and not read: a frame drawn ahead that the export
    /// did not ask for.
    pub fn discard(&mut self, slot: u32) {
        self.gpu.readback(slot as usize).unmap();
    }

    /// Fails with what the device reported since the export started (lost, out of memory).
    /// `recoverable` in the error's message prefix tells a retry with smaller tiles may help.
    pub fn check(&self) -> Result<(), JsError> {
        match self.failure.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            Some((recoverable, message)) => Err(js_error(format!("{}{message}", if *recoverable { "recoverable: " } else { "" }))),
            None => Ok(()),
        }
    }

    /// Draws later frames in tiles of at most `tile` pixels (a retry after a GPU memory failure).
    pub fn retile(&mut self, tile: u32) {
        let settings = *self.gpu.settings();
        let queue = self.gpu_queue();
        let generation = self.frames.generation().clone();
        self.gpu = GpuExport::new(&self.device, &queue, settings, tile);
        self.gpu.prepare_sprites(generation.lineage, &generation.assets);
        self.frames.forget();
        self.frames.decode_images();
    }

    fn gpu_queue(&self) -> wgpu::Queue {
        self.gpu.queue().clone()
    }

    /// The export's figures as JSON: backend, tiles, read-back slots, tile size, frames drawn, the last
    /// evaluation, and the renderer's counts of the last tile.
    pub fn stats(&self) -> String {
        let s = self.gpu.stats();
        let size = self.gpu.tiles()[0].size;
        format!(
            "{{\"backend\":{:?},\"tiles\":{},\"slots\":{},\"tileWidth\":{},\"tileHeight\":{},\"framesDrawn\":{},\"startMs\":[{:.1},{:.1},{:.1}],\"evaluateMs\":{:.3},\"prims\":{},\"passes\":{},\"rounds\":{},\"layerBytes\":{},\"effectBytes\":{},\"depthBytes\":{},\"glyphBytes\":{}}}",
            self.backend, self.gpu.tiles().len(), self.gpu.slots(), size[0], size[1], self.frames_drawn, self.start_ms[0], self.start_ms[1], self.start_ms[2], self.last_evaluate_ms, s.prims, s.passes, s.rounds, s.layer_bytes, s.effect_bytes, s.depth_bytes, s.glyph_bytes
        )
    }

    /// Releases the GPU device and everything the export holds. The worker ends afterwards.
    pub fn dispose(&mut self) {
        self.current = None;
        self.device.destroy();
    }
}
