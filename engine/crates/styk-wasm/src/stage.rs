//! One canvas: the GPU surface and the renderer that draws prepared frames into it.

use std::sync::Arc;

use styk_engine::edit::{EditOptions, EditScene};
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
    /// The document lineage and page count the sprite atlas is allocated for.
    atlas_of: Option<(u64, u32)>,
    edit: EditScene,
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
        Ok(Stage { engine: engine.engine.clone(), surface, config, device, queue, renderer, backend: format!("{:?} · {}", info.backend, info.name), atlas_of: None, edit: EditScene::default() })
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
        self.draw(&generation, &frame.list, if camera { frame.camera } else { None }, zoom, [pan_x, pan_y], clip_to_artboard)
    }

    /// Replaces the frame the editor is showing: keyframe-shaped JSON whose `objects`
    /// are poses, or `null` for an object unchanged since the last call.
    pub fn edit_frame(&mut self, json: &str) -> Result<(), JsError> {
        let generation = self.engine.generation().ok_or_else(|| js_error("no document is open"))?;
        self.edit.update(&generation, json.as_bytes()).map_err(js_error)
    }

    /// Draws the edit frame. `hidden` lists object slots not drawn; `translations` holds
    /// `slot, dx, dy` triples of dragged objects; `background` is an ARGB canvas colour
    /// replacing the scene background, or a negative number for none.
    #[allow(clippy::too_many_arguments)]
    pub fn render_edit(&mut self, zoom: f64, pan_x: f64, pan_y: f64, camera: bool, clip_to_artboard: bool, hidden: &[u32], translations: &[f64], background: f64) -> Result<bool, JsError> {
        let Some(generation) = self.engine.generation() else { return Ok(false) };
        let translations: Vec<(u32, [f64; 2])> = translations.chunks_exact(3).map(|t| (t[0] as u32, [t[1], t[2]])).collect();
        let options = EditOptions { hidden, translations: &translations, background: (background >= 0.0).then_some(background as u32) };
        let Some(list) = self.edit.draw_list(&generation, &options) else { return Ok(false) };
        let scene_camera = if camera { self.edit.camera() } else { None };
        self.draw(&generation, &list, scene_camera, zoom, [pan_x, pan_y], clip_to_artboard)
    }

    fn draw(&mut self, generation: &Arc<Generation>, list: &Arc<styk_eval::drawlist::DrawList>, camera: Option<styk_eval::doc::Camera>, zoom: f64, pan: [f64; 2], clip_to_artboard: bool) -> Result<bool, JsError> {
        let layout = (generation.lineage, generation.assets.pages);
        if self.atlas_of != Some(layout) {
            // Another project, or more atlas pages than before: the texture is allocated
            // again and every image decoded so far is uploaded again by the engine.
            self.renderer.prepare_sprites(&generation.assets);
            if self.atlas_of.is_some_and(|(lineage, _)| lineage == layout.0) {
                self.engine.redecode_images();
            }
            self.atlas_of = Some(layout);
        }
        let images = self.engine.take_images(IMAGE_UPLOAD_BYTES);
        if !images.is_empty() {
            self.renderer.upload_images(&generation.assets, images.iter().map(|image| &**image));
        }
        let doc = &generation.prepared.doc;
        let m = styk_eval::view::camera_matrix(doc, camera.as_ref(), zoom, pan);
        // The artboard is the scene rectangle under the workspace view alone: the camera moves the scene inside it.
        let (p0, p1) = (pan, [pan[0] + doc.canvas[0] as f64 * zoom, pan[1] + doc.canvas[1] as f64 * zoom]);
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
        self.renderer.set_list(list);
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
