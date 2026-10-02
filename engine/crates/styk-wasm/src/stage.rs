//! One canvas: the GPU surface and the renderer that draws prepared frames into it.

use std::sync::Arc;

use styk_engine::{Engine, Generation};
use styk_render::{Renderer, ViewParams, wgpu};
use wasm_bindgen::prelude::*;

use crate::engine::EngineHandle;
use crate::js_error;

/// Pixels of decoded images uploaded per frame at most, so a document with many
/// sprites fills its atlas over a few frames instead of stalling one.
const IMAGE_UPLOAD_BYTES: usize = 8 << 20;

#[wasm_bindgen]
pub struct Stage {
    engine: Engine,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: Renderer,
    backend: String,
    /// The document whose sprite atlas is allocated.
    atlas_of: Option<Arc<Generation>>,
}

#[wasm_bindgen]
impl Stage {
    /// Creates the surface of `canvas`: WebGPU where the browser has it, WebGL2 otherwise.
    pub async fn create(canvas: web_sys::HtmlCanvasElement, engine: &EngineHandle) -> Result<Stage, JsError> {
        let (width, height) = (canvas.width().max(1), canvas.height().max(1));
        let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
        descriptor.backends = wgpu::Backends::BROWSER_WEBGPU | wgpu::Backends::GL;
        let instance = wgpu::util::new_instance_with_webgpu_detection(descriptor).await;
        let surface = instance.create_surface(wgpu::SurfaceTarget::Canvas(canvas)).map_err(js_error)?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, compatible_surface: Some(&surface), ..Default::default() })
            .await
            .map_err(js_error)?;
        let info = adapter.get_info();
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("styk stage"),
                required_limits: wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
                ..Default::default()
            })
            .await
            .map_err(js_error)?;
        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities.formats.iter().copied().find(|f| !f.is_srgb()).or(capabilities.formats.first().copied()).ok_or_else(|| js_error("the canvas has no surface format"))?;
        let alpha_mode = if capabilities.alpha_modes.contains(&wgpu::CompositeAlphaMode::PreMultiplied) { wgpu::CompositeAlphaMode::PreMultiplied } else { capabilities.alpha_modes[0] };
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
        Ok(Stage { engine: engine.engine.clone(), surface, config, device, queue, renderer, backend: format!("{:?} · {}", info.backend, info.name), atlas_of: None })
    }

    /// The graphics backend and adapter in use, for the status line and reports.
    pub fn backend(&self) -> String {
        self.backend.clone()
    }

    /// Sets the drawing buffer size in device pixels.
    pub fn resize(&mut self, width: u32, height: u32) {
        let (width, height) = (width.max(1), height.max(1));
        if (width, height) != (self.config.width, self.config.height) {
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(&self.device, &self.config);
        }
    }

    /// Draws `tick`. `zoom` is device pixels per scene unit and `pan` the device-pixel
    /// position of the scene's origin; with `camera`, the scene's own camera applies on
    /// top (playback), otherwise the view is the editor's neutral one. With `wait`, a
    /// frame no worker has prepared is prepared here; otherwise nothing is drawn and
    /// `false` is returned.
    #[allow(clippy::too_many_arguments)]
    pub fn render(&mut self, tick: u32, zoom: f64, pan_x: f64, pan_y: f64, camera: bool, clip_to_artboard: bool, wait: bool) -> Result<bool, JsError> {
        let Some(generation) = self.engine.generation() else { return Ok(false) };
        let frame = if wait { self.engine.frame_now(tick) } else { self.engine.frame(tick) };
        let Some(frame) = frame else { return Ok(false) };
        if !self.atlas_of.as_ref().is_some_and(|current| Arc::ptr_eq(current, &generation)) {
            self.renderer.prepare_sprites(&generation.assets);
            self.atlas_of = Some(generation.clone());
        }
        let images = self.engine.take_images(IMAGE_UPLOAD_BYTES);
        if !images.is_empty() {
            self.renderer.upload_images(&generation.assets, images.iter().map(|image| &**image));
        }
        let doc = &generation.prepared.doc;
        let m = styk_eval::view::camera_matrix(doc, if camera { frame.camera.as_ref() } else { None }, zoom, [pan_x, pan_y]);
        // The artboard is the scene rectangle under the workspace view alone: the camera moves the scene inside it.
        let (p0, p1) = ([pan_x, pan_y], [pan_x + doc.canvas[0] as f64 * zoom, pan_y + doc.canvas[1] as f64 * zoom]);
        let view = ViewParams {
            matrix: m,
            artboard: [p0[0].min(p1[0]), p0[1].min(p1[1]), p0[0].max(p1[0]), p0[1].max(p1[1])],
            clip_to_artboard,
            clear: [0.0; 4],
        };
        let target = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture) | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => texture,
            _ => {
                // The surface was lost or is being resized: configure it again and draw next frame.
                self.surface.configure(&self.device, &self.config);
                return Ok(false);
            }
        };
        self.renderer.set_list(&frame.list);
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("styk frame") });
        self.renderer.render(&mut encoder, &view, [self.config.width, self.config.height]);
        {
            let target_view = target.texture.create_view(&Default::default());
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("styk present"),
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
        Ok(true)
    }

    /// Primitive, isolation group and pass counts of the last frame, as JSON.
    pub fn stats(&self) -> String {
        let s = &self.renderer.stats;
        format!("{{\"prims\":{},\"groups\":{},\"passes\":{},\"downscaled\":{}}}", s.prims, s.groups, s.passes, s.downscaled)
    }
}
