//! Library previews, drawn in the app's preview worker (`src/engine/engine-preview-worker.mjs`).
//!
//! The worker joins the engine's shared memory as a thread of its own and keeps its event
//! loop. It holds a GPU device of its own (WebGPU in the worker, else WebGL2 on the
//! worker's `OffscreenCanvas`) and draws into that canvas with the stage's renderer: a
//! library file's poster, its hover animation and the samples its card's backdrop is chosen
//! from ([`styk_engine::thumbnails::library`]). After each draw the worker takes the canvas's
//! picture as an `ImageBitmap` (`transferToImageBitmap`), so nothing is read back.
//!
//! A library file is not the open document: its pictures go into an atlas of this device,
//! replaced when another file is drawn (library files hold few pictures, if any).

use std::sync::Arc;

use styk_engine::thumbnails::library::{fitted, LibraryPreview, Sample, SAMPLE, SIZE};
use styk_render::{Renderer, ViewParams, wgpu};
use wasm_bindgen::prelude::*;

use crate::{clock, js_error};

#[wasm_bindgen]
pub struct Previews {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    backend: String,
    /// The library file whose pictures the atlas holds (its lineage).
    atlas: Option<u64>,
    /// The hover animation's file, and the samples `library_begin` evaluated.
    library: Option<LibraryPreview>,
    samples: Vec<(Sample, [f64; 4])>,
}

/// A picture drawn, as JSON for the worker (`extra` adds fields).
fn drawn(width: u32, height: u32, started: f64, evaluated: f64, extra: &str) -> String {
    format!("{{\"width\":{width},\"height\":{height},\"evaluateMs\":{:.3},\"drawMs\":{:.3}{extra}}}", evaluated - started, clock() - evaluated)
}

#[wasm_bindgen]
impl Previews {
    /// Gets a GPU device that draws into `canvas`: WebGPU in this worker where the browser
    /// has it, else WebGL2. Fails when neither gives a device.
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
        Ok(Previews { device, queue, surface, config, renderer, backend: format!("{:?} · {}", info.backend, info.name), atlas: None, library: None, samples: Vec::new() })
    }

    /// The graphics backend and adapter, for reports.
    pub fn backend(&self) -> String {
        self.backend.clone()
    }

    /// The poster of a library file (`extension`: `stkz`, `stykmov`…), 320 pixels a side,
    /// drawn into the canvas. The file is opened for this call only: the hover animation's
    /// file stays open. Returns the timings as JSON.
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

    /// Draws sample `index` of [`Previews::library_begin`] into the canvas, 40 pixels a side,
    /// fitted to its own bounds.
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

    /// Draws the open file's animation frame at `elapsed` seconds into the canvas, 320 pixels
    /// a side, fitted to every frame's bounds so far. Returns its tick and timings as JSON.
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

    /// Releases the GPU device and everything drawn with it. The worker ends afterwards.
    pub fn dispose(&mut self) {
        self.library = None;
        self.samples.clear();
        self.device.destroy();
    }
}

impl Previews {
    /// Draws `list` of `preview`'s file fitted to `bounds` into the canvas, `size` a side,
    /// on transparency.
    fn draw(&mut self, preview: &LibraryPreview, list: &Arc<styk_draw::DrawList>, bounds: [f64; 4], size: u32) -> Result<(), JsError> {
        if self.atlas != Some(preview.lineage) {
            self.renderer.prepare_sprites(preview.assets());
            let images = preview.images();
            self.renderer.upload_images(preview.assets(), images.iter());
            self.atlas = Some(preview.lineage);
        }
        if (size, size) != (self.config.width, self.config.height) {
            self.config.width = size;
            self.config.height = size;
            self.surface.configure(&self.device, &self.config);
        }
        let framing = fitted(bounds, size);
        let view = ViewParams {
            matrix: framing.matrix,
            artboard: framing.artboard,
            clip_to_artboard: false,
            clear: [0.0; 4],
            zoom: framing.zoom,
            pan: framing.pan,
            stretch: 1.0,
            camera_view: false,
            camera_limit: styk_render::CAMERA_PREVIEW_LIMIT,
        };
        let target = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture) | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => texture,
            _ => {
                self.surface.configure(&self.device, &self.config);
                return Err(js_error("the preview canvas was lost; try again"));
            }
        };
        self.renderer.set_list(list);
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("styk library preview") });
        self.renderer.render(&mut encoder, &view, [size, size]);
        {
            let target_view = target.texture.create_view(&Default::default());
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("styk library preview present"),
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
