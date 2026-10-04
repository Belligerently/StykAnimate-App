//! Library previews and CPU frame thumbnails, drawn in the app's preview worker
//! (`src/engine/engine-preview-worker.mjs`).
//!
//! The worker joins the engine's shared memory as a thread of its own and keeps its event
//! loop. Where it gets a GPU device (WebGPU in the worker, else WebGL2 on its
//! `OffscreenCanvas`) it draws library files into that canvas with the stage's renderer: a
//! file's poster, its hover animation and the samples its card's backdrop is chosen from
//! ([`styk_engine::thumbnails::library`]); after each draw the worker takes the canvas's
//! picture as an `ImageBitmap`, so nothing is read back. Where it gets none, or the
//! canvas's pictures do not reach the page, the same pictures are drawn by the CPU
//! rasterizer (styk-raster, [`Previews::create_cpu`]) and handed over as pixels.
//!
//! Frame thumbnails of the open document that the stage cannot draw (a WebGL2 stage, a
//! canvas failing the probe) are drawn here too, always on the CPU ([`Previews::thumbnail`]):
//! the document is the engine's, pinned at the interface's revision (`ExportFrames`), its
//! frames prepared on the engine's workers and its pictures decoded there once per document
//! lineage, kept across revisions. In-between poses and the camera monitor share this
//! worker's existing device/atlas (or its CPU rasterizer); even their first preparation
//! runs here, never on the interface thread.
//!
//! A library file is not the open document: its pictures go into an atlas of this device (or
//! the CPU's pictures), replaced when another file is drawn (library files hold few
//! pictures, if any).

use std::sync::Arc;

use styk_engine::export::{ExportFrames, AHEAD};
use styk_engine::thumbnails::library::{fitted, LibraryPreview, Sample, SAMPLE, SIZE};
use styk_engine::thumbnails::{card, Framing, CARD, MAX_SIDE};
use styk_engine::thumbnails::monitor::MonitorFrame;
use styk_engine::thumbnails::inbetween::{InbetweenPreview, Onions};
use styk_engine::{Engine, Generation};
use styk_raster::{Edges, Images, Raster};
use styk_render::{Renderer, ViewParams, wgpu};
use wasm_bindgen::prelude::*;

use crate::{clock, js_error};

/// What [`Previews::thumbnail`] answers.
const READY: u32 = 0;
const WAIT: u32 = 1;
const STALE: u32 = 2;

/// The worker's GPU device and the canvas it draws into.
struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    /// The library file whose pictures the atlas holds (its lineage).
    atlas: Option<(u64, u32, usize)>,
}

/// The open document's frames for thumbnails, and its pictures for the CPU.
struct DocumentThumbnails {
    frames: ExportFrames,
    images: Images,
    preview: Option<InbetweenPreview>,
}

#[wasm_bindgen]
pub struct Previews {
    gpu: Option<Gpu>,
    backend: String,
    /// The CPU's renderer: library previews without a GPU device, every frame thumbnail.
    raster: Raster,
    /// The library file whose pictures the CPU holds (its lineage), and those pictures.
    library_images: Option<(u64, Images)>,
    /// The last picture the CPU drew: its size and straight RGBA.
    picture: (u32, u32, Vec<u8>),
    /// The hover animation's file, and the samples `library_begin` evaluated.
    library: Option<LibraryPreview>,
    samples: Vec<(Sample, [f64; 4])>,
    /// The engine whose open document frame thumbnails show ([`Previews::adopt_engine`]).
    engine: Option<Engine>,
    thumbnails: Option<DocumentThumbnails>,
    monitor: Option<MonitorFrame>,
}

/// A picture drawn, as JSON for the worker (`extra` adds fields).
fn drawn(width: u32, height: u32, started: f64, evaluated: f64, extra: &str) -> String {
    format!("{{\"width\":{width},\"height\":{height},\"evaluateMs\":{:.3},\"drawMs\":{:.3}{extra}}}", evaluated - started, clock() - evaluated)
}

impl Previews {
    fn with(gpu: Option<Gpu>, backend: String) -> Previews {
        Previews { gpu, backend, raster: Raster::new(Edges::Smooth), library_images: None, picture: (0, 0, Vec::new()), library: None, samples: Vec::new(), engine: None, thumbnails: None, monitor: None }
    }
}

#[wasm_bindgen]
impl Previews {
    /// Gets a GPU device that draws into `canvas`: WebGPU in this worker where the browser
    /// has it, else WebGL2. Fails when neither gives a device ([`Previews::create_cpu`] then).
    pub async fn create(canvas: web_sys::OffscreenCanvas) -> Result<Previews, JsError> {
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = wgpu::Backends::BROWSER_WEBGPU | wgpu::Backends::GL;
        let instance = wgpu::util::new_instance_with_webgpu_detection(descriptor).await;
        let (width, height) = (canvas.width().max(1), canvas.height().max(1));
        let surface = instance.create_surface(wgpu::SurfaceTarget::OffscreenCanvas(canvas)).map_err(js_error)?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, compatible_surface: Some(&surface), ..Default::default() })
            .await
            .map_err(|e| js_error(format!("no GPU adapter in this worker ({e})")))?;
        let info = adapter.get_info();
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("styk previews"),
                required_limits: wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
                ..Default::default()
            })
            .await
            .map_err(js_error)?;
        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities.formats.iter().copied().find(|f| !f.is_srgb()).or(capabilities.formats.first().copied()).ok_or_else(|| js_error("the canvas has no surface format"))?;
        // Previews are on transparency: every WebGPU canvas takes premultiplied alpha (wgpu
        // lists only opaque for WebGPU); a WebGL2 canvas has premultiplied alpha already.
        let alpha_mode = if info.backend == wgpu::Backend::BrowserWebGpu || capabilities.alpha_modes.contains(&wgpu::CompositeAlphaMode::PreMultiplied) {
            wgpu::CompositeAlphaMode::PreMultiplied
        } else {
            capabilities.alpha_modes[0]
        };
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width,
            height,
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode,
            view_formats: vec![],
            color_space: Default::default(),
        };
        surface.configure(&device, &config);
        let renderer = Renderer::new(&device, &queue, format);
        Ok(Previews::with(Some(Gpu { device, queue, surface, config, renderer, atlas: None }), format!("{:?} · {}", info.backend, info.name)))
    }

    /// Previews drawn by the CPU rasterizer (styk-raster): every picture is handed over as
    /// pixels ([`Previews::picture`]). For a worker without a GPU device, or whose canvas
    /// gives no pixels.
    pub fn create_cpu() -> Previews {
        Previews::with(None, "CPU · styk-raster".into())
    }

    /// The graphics backend and adapter, for reports.
    pub fn backend(&self) -> String {
        self.backend.clone()
    }

    /// Whether library pictures are drawn by the CPU (taken with [`Previews::picture`]) rather
    /// than into the canvas.
    pub fn cpu(&self) -> bool {
        self.gpu.is_none()
    }

    /// Clears the canvas to the probe colour (opaque magenta): the worker checks that its
    /// pictures leave the canvas here before it takes any preview (`views::probe`). `false`
    /// without a GPU device.
    pub fn probe(&mut self) -> bool {
        self.gpu.as_ref().is_some_and(|gpu| crate::views::probe(&gpu.surface, &gpu.config, &gpu.device, &gpu.queue))
    }

    /// The poster of a library file (`extension`: `stkz`, `stykmov`…), 320 pixels a side,
    /// drawn into the canvas (or the CPU's picture). The file is opened for this call only:
    /// the hover animation's file stays open. Returns the timings as JSON.
    pub fn poster(&mut self, bytes: &[u8], extension: &str) -> Result<String, JsError> {
        let started = clock();
        let preview = LibraryPreview::open(bytes, extension).map_err(js_error)?;
        let (sample, bounds) = preview.poster(clock);
        let evaluated = clock();
        self.draw(&preview, &sample.list, bounds, SIZE)?;
        Ok(drawn(SIZE, SIZE, started, evaluated, &format!(",\"bounds\":[{},{},{},{}]", bounds[0], bounds[1], bounds[2], bounds[3])))
    }

    /// Opens a library file for its hover animation (replacing the one open). Returns
    /// `{fps, duration}` as JSON; fails for a file the engine cannot read.
    pub fn library_open(&mut self, bytes: &[u8], extension: &str) -> Result<String, JsError> {
        self.library = None;
        self.samples.clear();
        let preview = LibraryPreview::open(bytes, extension).map_err(js_error)?;
        let text = format!("{{\"fps\":{},\"duration\":{}}}", preview.fps, preview.duration);
        self.library = Some(preview);
        Ok(text)
    }

    /// Fits the open file's animation and returns how many samples have artwork; draw each
    /// with [`Previews::library_sample`].
    pub fn library_begin(&mut self) -> Result<u32, JsError> {
        let library = self.library.as_mut().ok_or_else(|| js_error("open a library file first"))?;
        self.samples = library.begin(clock);
        Ok(self.samples.len() as u32)
    }

    /// Draws sample `index` of [`Previews::library_begin`], 40 pixels a side, fitted to its
    /// own bounds.
    pub fn library_sample(&mut self, index: u32) -> Result<(), JsError> {
        let library = self.library.take().ok_or_else(|| js_error("open a library file first"))?;
        let result = match self.samples.get(index as usize) {
            Some((sample, bounds)) => {
                let (list, bounds) = (sample.list.clone(), *bounds);
                self.draw(&library, &list, bounds, SAMPLE)
            }
            None => Err(js_error(format!("there is no sample {index}"))),
        };
        self.library = Some(library);
        result
    }

    /// Draws the open file's animation frame at `elapsed` seconds, 320 pixels a side, fitted
    /// to every frame's bounds so far. Returns its tick and timings as JSON.
    pub fn library_frame(&mut self, elapsed: f64) -> Result<String, JsError> {
        let started = clock();
        let mut library = self.library.take().ok_or_else(|| js_error("open a library file first"))?;
        let (sample, bounds, tick) = library.frame(elapsed, clock);
        let evaluated = clock();
        let result = self.draw(&library, &sample.list, bounds, SIZE);
        self.library = Some(library);
        result?;
        Ok(drawn(SIZE, SIZE, started, evaluated, &format!(",\"tick\":{tick},\"bounds\":[{},{},{},{}]", bounds[0], bounds[1], bounds[2], bounds[3])))
    }

    /// Closes the open library file.
    pub fn library_close(&mut self) {
        self.library = None;
        self.samples.clear();
    }

    /// Frame thumbnails show the open document of the engine at `address` (from
    /// `EngineHandle::address`, consumed).
    pub fn adopt_engine(&mut self, address: usize) {
        // SAFETY: `address` was produced by `EngineHandle::address` in this memory and is used once.
        self.engine = Some(unsafe { *Box::from_raw(address as *mut Engine) });
    }

    /// Consumes an engine `address` no thumbnail will use (a request dropped while the
    /// worker is released).
    pub fn forget_engine(address: usize) {
        // SAFETY: as in `adopt_engine`.
        drop(unsafe { Box::from_raw(address as *mut Engine) });
    }

    /// Consumes the stage's shared-memory monitor snapshot. It holds shared pointers,
    /// never a JSON copy of the document or a pose prepared on the interface thread.
    pub fn monitor_begin(&mut self, address: usize) {
        // SAFETY: the stage made this address in this memory; this request consumes it once.
        self.monitor = Some(unsafe { *Box::from_raw(address as *mut MonitorFrame) });
    }

    /// Releases a snapshot which did not reach a running request (worker release).
    pub fn forget_monitor(address: usize) {
        // SAFETY: as in monitor_begin; a dropped message's address is consumed once.
        drop(unsafe { Box::from_raw(address as *mut MonitorFrame) });
    }

    /// Draws the stage's displayed pose through the camera, on transparency. Executed
    /// only in the preview worker, including the first camera/parallax preparation.
    /// 0: drawn; 1: pictures pending; 2: a newer document generation superseded it.
    pub fn monitor(&mut self, width: u32, height: u32, zoom: f64, pan_x: f64, pan_y: f64) -> Result<u32, JsError> {
        if width == 0 || height == 0 || width > 2048 || height > 2048 || !zoom.is_finite() || zoom <= 0.0 || !pan_x.is_finite() || !pan_y.is_finite() {
            return Err(js_error("Invalid camera monitor size or view."));
        }
        let Some(request) = self.monitor.as_ref() else { return Ok(STALE) };
        let (engine, generation) = (request.engine.clone(), request.generation.clone());
        if engine.generation().is_none_or(|g| g.id != generation.id) { return Ok(STALE) }
        self.pin(&engine, &generation)?;
        let pictures = self.thumbnails.as_mut().expect("pinned above");
        for image in pictures.frames.take_images(usize::MAX) { pictures.images.add_shared(&generation.assets, image); }
        if pictures.frames.images_pending() > 0 { return Ok(WAIT) }
        let Some((list, camera)) = self.monitor.as_mut().expect("present above").prepare().map_err(js_error)? else { return Ok(STALE) };
        let doc = &generation.prepared.doc;
        let pan = [pan_x, pan_y];
        let framing = Framing { matrix: styk_eval::view::camera_matrix(doc, camera.as_ref(), zoom, pan), artboard: [pan_x, pan_y, pan_x + doc.canvas[0] as f64 * zoom, pan_y + doc.canvas[1] as f64 * zoom], zoom, pan };
        let view = view_of(framing, [0.0; 4], true, true);
        self.draw_document(&generation, &list, &view, width, height)?;
        Ok(READY)
    }

    /// The request finished or was cancelled. Images stay with the preview worker's
    /// pinned document, reused by thumbnails and the next monitor frame.
    pub fn monitor_end(&mut self) { self.monitor = None; }

    /// Explicit in-between samples, including hidden and one-tick native intervals,
    /// prepared here using one typed sampling view of the pinned generation.
    pub fn inbetween(&mut self, revision: f64, index: u32, ratio: f64, mode: u32, count: u32, opacity: f64, ahead: bool, width: u32, height: u32) -> Result<u32, JsError> {
        if !(1..=MAX_SIDE).contains(&width) || !(1..=MAX_SIDE).contains(&height) { return Err(js_error("Invalid in-between preview dimensions.")); }
        let Some(engine) = self.engine.clone() else { return Ok(STALE) };
        let Some(generation) = engine.generation().filter(|g| g.revision == revision as u64) else { return Ok(STALE) };
        self.pin(&engine, &generation)?;
        let pictures = self.thumbnails.as_mut().expect("pinned above");
        for image in pictures.frames.take_images(usize::MAX) { pictures.images.add_shared(&generation.assets, image); }
        if pictures.frames.images_pending() > 0 { return Ok(WAIT) }
        let preview = pictures.preview.get_or_insert_with(|| InbetweenPreview::new(generation.clone()));
        let (list, camera) = preview.sample(index as usize, ratio, Onions { mode, count, opacity, ahead }, clock).map_err(js_error)?;
        let view = view_of(card(&generation.prepared.doc, camera.as_ref(), width, height), CARD, true, true);
        self.draw_document(&generation, &list, &view, width, height)?;
        Ok(READY)
    }

    /// Draws the thumbnail of `tick` of the open document at `revision` on a `width ×
    /// height` card (`styk_engine::thumbnails::card`) with the CPU rasterizer, into the CPU's
    /// picture. 0: drawn; 1: its frame or some of the document's pictures are not ready yet
    /// (wait for the engine's progress, then ask again); 2: the engine holds another revision
    /// (or no document, or no engine was adopted). Fails for a tick past the end or a size
    /// beyond 512 pixels.
    pub fn thumbnail(&mut self, revision: f64, tick: u32, width: u32, height: u32) -> Result<u32, JsError> {
        if !(1..=MAX_SIDE).contains(&width) || !(1..=MAX_SIDE).contains(&height) {
            return Err(js_error(format!("Thumbnails are 1 to {MAX_SIDE} pixels a side ({width}×{height} asked).")));
        }
        let Some(engine) = self.engine.clone() else { return Ok(STALE) };
        let Some(generation) = engine.generation().filter(|g| g.revision == revision as u64) else { return Ok(STALE) };
        self.pin(&engine, &generation)?;
        let thumbnails = self.thumbnails.as_mut().expect("pinned above");
        if tick >= thumbnails.frames.ticks() {
            return Err(js_error(format!("The document has no tick {tick} ({} ticks).", thumbnails.frames.ticks())));
        }
        // A thumbnail never shows a frame without its pictures: they are all taken first.
        let assets = thumbnails.frames.generation().assets.clone();
        for image in thumbnails.frames.take_images(usize::MAX) {
            thumbnails.images.add_shared(&assets, image);
        }
        let Some(frame) = thumbnails.frames.ready(tick) else {
            // Frames asked for and never drawn (a cancelled thumbnail) give their places back.
            if thumbnails.frames.waiting() >= AHEAD {
                thumbnails.frames.forget();
            }
            thumbnails.frames.prefetch([tick]);
            return Ok(WAIT);
        };
        if thumbnails.frames.images_pending() > 0 {
            return Ok(WAIT);
        }
        let doc = &thumbnails.frames.generation().prepared.doc;
        let framing = card(doc, frame.camera.as_ref(), width, height);
        let view = view_of(framing, CARD, true, true);
        self.picture = (width, height, self.raster.render(&frame.list, &thumbnails.images, &view, [width, height]).straight());
        // Drawn: its place among the frames prepared ahead is free again.
        let _ = thumbnails.frames.take(tick);
        Ok(READY)
    }

    /// Copies the last picture the CPU drew into `out` (the RGBA of an `ImageData` of its
    /// size): straight alpha. Fails when there is none of that size.
    pub fn picture(&self, out: &js_sys::Uint8ClampedArray) -> Result<(), JsError> {
        let (width, height, pixels) = &self.picture;
        if pixels.is_empty() || out.length() as usize != pixels.len() {
            return Err(js_error(format!("no {}×{} picture to hand over", width, height)));
        }
        out.copy_from(pixels);
        Ok(())
    }

    /// The size of the last picture the CPU drew: width, height.
    pub fn picture_size(&self) -> Box<[u32]> {
        Box::new([self.picture.0, self.picture.1])
    }

    /// Lets go of the open document's frames and pictures (thumbnails start again from the
    /// engine when next asked).
    pub fn thumbnails_release(&mut self) {
        self.thumbnails = None;
    }

    /// Releases the GPU device (if any) and everything drawn with it. The worker ends afterwards.
    pub fn dispose(&mut self) {
        self.library = None;
        self.samples.clear();
        self.thumbnails = None;
        self.engine = None;
        self.monitor = None;
        self.library_images = None;
        self.picture = (0, 0, Vec::new());
        self.raster.release();
        if let Some(gpu) = &self.gpu {
            gpu.device.destroy();
        }
    }
}

/// The view of a framing on `clear`, clipped to its artboard or not, through the scene
/// camera (frame thumbnails) or not (library previews).
fn view_of(framing: Framing, clear: [f64; 4], clip_to_artboard: bool, camera_view: bool) -> ViewParams {
    ViewParams {
        matrix: framing.matrix,
        artboard: framing.artboard,
        clip_to_artboard,
        clear,
        zoom: framing.zoom,
        pan: framing.pan,
        stretch: 1.0,
        camera_view,
        camera_limit: styk_render::CAMERA_PREVIEW_LIMIT,
    }
}

impl Previews {
    /// Pins `generation` (the engine's, at the interface's revision) for thumbnails: its
    /// frames, and the pictures it has that those already decoded for its lineage do not.
    fn pin(&mut self, engine: &Engine, generation: &Arc<Generation>) -> Result<(), JsError> {
        if self.thumbnails.as_ref().is_some_and(|t| t.frames.generation().id == generation.id) {
            return Ok(());
        }
        let frames = engine.export_frames(generation.revision).map_err(js_error)?;
        // An earlier revision of the same document whose pictures were all taken: only what it
        // did not have is decoded (its pictures keep their atlas places, `Assets::extended`).
        match self.thumbnails.take() {
            Some(earlier) if earlier.frames.generation().lineage == generation.lineage && earlier.frames.images_pending() == 0 => {
                frames.decode_images_since(Some(earlier.frames.generation()));
                self.thumbnails = Some(DocumentThumbnails { frames, images: earlier.images, preview: None });
            }
            _ => {
                frames.decode_images();
                self.thumbnails = Some(DocumentThumbnails { frames, images: Images::default(), preview: None });
            }
        }
        Ok(())
    }

    /// Monitor and in-between frames share this worker's existing atlas/device and
    /// the pinned document's decoded pictures; library files reuse the same device.
    fn draw_document(&mut self, generation: &Generation, list: &Arc<styk_draw::DrawList>, view: &ViewParams, width: u32, height: u32) -> Result<(), JsError> {
        let pictures = self.thumbnails.as_ref().expect("a document is pinned");
        if let Some(gpu) = self.gpu.as_mut() {
            let layout = (generation.lineage, generation.assets.pages, pictures.images.len());
            if gpu.atlas != Some(layout) {
                gpu.renderer.prepare_sprites(&generation.assets);
                gpu.renderer.upload_images(&generation.assets, pictures.images.decoded());
                gpu.atlas = Some(layout);
            }
            gpu.draw(list, view, width, height)?;
        } else { self.picture = (width, height, self.raster.render(list, &pictures.images, view, [width, height]).straight()); }
        Ok(())
    }

    /// Draws `list` of `preview`'s file fitted to `bounds`, `size` a side, on transparency:
    /// into the canvas with the GPU, else into the CPU's picture.
    fn draw(&mut self, preview: &LibraryPreview, list: &Arc<styk_draw::DrawList>, bounds: [f64; 4], size: u32) -> Result<(), JsError> {
        let view = view_of(fitted(bounds, size), [0.0; 4], false, false);
        let Some(gpu) = self.gpu.as_mut() else {
            if self.library_images.as_ref().is_none_or(|(lineage, _)| *lineage != preview.lineage) {
                let mut images = Images::default();
                for image in preview.images() {
                    images.add_shared(preview.assets(), Arc::new(image));
                }
                self.library_images = Some((preview.lineage, images));
            }
            let images = &self.library_images.as_ref().expect("set above").1;
            self.picture = (size, size, self.raster.render(list, images, &view, [size, size]).straight());
            return Ok(());
        };
        if gpu.atlas.is_none_or(|(lineage, _, _)| lineage != preview.lineage) {
            gpu.renderer.prepare_sprites(preview.assets());
            let images = preview.images();
            gpu.renderer.upload_images(preview.assets(), images.iter());
            gpu.atlas = Some((preview.lineage, preview.assets().pages, images.len()));
        }
        gpu.draw(list, &view, size, size)
    }
}

impl Gpu {
    /// One renderer and atlas serve library and camera pictures on the existing worker
    /// device. A switch replaces the atlas; no second renderer or device is allocated.
    fn draw(&mut self, list: &Arc<styk_draw::DrawList>, view: &ViewParams, width: u32, height: u32) -> Result<(), JsError> {
        if (width, height) != (self.config.width, self.config.height) {
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(&self.device, &self.config);
        }
        let target = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture) | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => texture,
            _ => {
                self.surface.configure(&self.device, &self.config);
                return Err(js_error("the preview canvas was lost; try again"));
            }
        };
        self.renderer.set_list(list);
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("styk auxiliary view") });
        self.renderer.render(&mut encoder, view, [width, height]);
        {
            let target_view = target.texture.create_view(&Default::default());
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("styk auxiliary view present"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.renderer.present(&mut pass);
        }
        self.queue.submit([encoder.finish()]);
        self.queue.present(target);
        Ok(())
    }
}
