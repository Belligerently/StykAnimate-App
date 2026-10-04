//! Other views the stage's device draws: the camera monitor and frame thumbnails.
//!
//! Each is a canvas of its own (an `OffscreenCanvas` the interface keeps and copies onto
//! the element that shows it) with a renderer of its own, so its frame, layer and depth
//! targets keep their size, sampling the stage's sprite atlas: the stage's pictures are
//! never uploaded twice. Only WebGPU presents one device to several canvases; where the
//! stage runs on WebGL2 these views are refused and the interface draws them itself.
//!
//! * The camera monitor shows the frame the stage drew last, through the scene camera,
//!   without onion skins (an edit frame is encoded again with the camera's parallax).
//! * A frame thumbnail is a tick of a revision the interface names, framed as the
//!   filmstrip frames it ([`styk_engine::thumbnails::card`]). Its frame is prepared on the
//!   engine's workers ([`ExportFrames`]); the interface thread only draws it (D7), and only
//!   once every picture of the document is in the atlas.

use std::sync::Arc;

use styk_draw::DrawList;
use styk_engine::edit::EditOptions;
use styk_engine::export::{ExportFrames, AHEAD};
use styk_engine::thumbnails::{card, Framing, CARD, MAX_SIDE};
use styk_eval::doc::Camera;
use styk_render::{Renderer, ViewParams, wgpu};
use wasm_bindgen::prelude::*;

use crate::js_error;
use crate::stage::{Shown, Stage};

/// One canvas drawn on the stage's device by a renderer of its own.
struct Aux {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    /// The stage atlas (lineage, pages) the renderer samples.
    shared: Option<(u64, u32)>,
}

/// What a monitor list of the edit frame was built from.
#[derive(Clone, PartialEq)]
struct MonitorKey {
    generation: u64,
    edits: u64,
    effects: bool,
    hidden: Vec<u32>,
    translations: Vec<(u32, [f64; 2])>,
}

#[derive(Default)]
pub(crate) struct Views {
    monitor: Option<Aux>,
    /// The last edit frame's list through the camera, reused while nothing changed.
    monitor_list: Option<(MonitorKey, Arc<DrawList>, Option<Camera>)>,
    thumbnails: Option<Aux>,
    /// The revision thumbnails are drawn from, pinned while the interface asks for it.
    frames: Option<ExportFrames>,
}

/// What [`Stage::prepare_thumbnail`] and [`Stage::render_thumbnail`] answer.
const READY: u32 = 0;
const WAIT: u32 = 1;
const STALE: u32 = 2;

impl Aux {
    fn create(stage: &Stage, canvas: web_sys::OffscreenCanvas) -> Result<Aux, JsError> {
        let (width, height) = (canvas.width().max(1), canvas.height().max(1));
        let surface = stage.instance.create_surface(wgpu::SurfaceTarget::OffscreenCanvas(canvas)).map_err(js_error)?;
        let config = wgpu::SurfaceConfiguration { width, height, ..stage.config.clone() };
        surface.configure(&stage.device, &config);
        let renderer = Renderer::new(&stage.device, &stage.queue, config.format);
        Ok(Aux { surface, config, renderer, shared: None })
    }

    /// Draws `list` under `view` into the canvas, at `width × height`.
    fn draw(&mut self, stage: &Stage, list: &Arc<DrawList>, view: &ViewParams, width: u32, height: u32) -> bool {
        if self.shared != stage.atlas_of {
            self.renderer.share_sprites(&stage.renderer);
            self.shared = stage.atlas_of;
        }
        let (width, height) = (width.max(1), height.max(1));
        if (width, height) != (self.config.width, self.config.height) {
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(&stage.device, &self.config);
        }
        let target = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture) | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => texture,
            _ => {
                self.surface.configure(&stage.device, &self.config);
                return false;
            }
        };
        self.renderer.set_list(list);
        let mut encoder = stage.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("styk view") });
        self.renderer.render(&mut encoder, view, [width, height]);
        {
            let target_view = target.texture.create_view(&Default::default());
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("styk view present"),
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
        stage.queue.submit([encoder.finish()]);
        stage.queue.present(target);
        true
    }
}

/// The view of a framing through the scene camera, clipped to its artboard, on `clear`.
fn view_of(framing: Framing, clear: [f64; 4]) -> ViewParams {
    ViewParams {
        matrix: framing.matrix,
        artboard: framing.artboard,
        clip_to_artboard: true,
        clear,
        zoom: framing.zoom,
        pan: framing.pan,
        stretch: 1.0,
        camera_view: true,
        camera_limit: styk_render::CAMERA_PREVIEW_LIMIT,
    }
}

#[wasm_bindgen]
impl Stage {
    /// Gives the camera monitor a canvas (`canvas`, drawn by a renderer of its own that
    /// samples the stage's pictures). `false` where the device cannot present to a second
    /// canvas (WebGL2): the interface draws the monitor itself there.
    pub fn attach_monitor(&mut self, canvas: web_sys::OffscreenCanvas) -> Result<bool, JsError> {
        if !self.is_webgpu {
            return Ok(false);
        }
        self.views.monitor = Some(Aux::create(self, canvas)?);
        Ok(true)
    }

    /// Draws the frame the stage drew last into the camera monitor, through the scene camera:
    /// `width × height` device pixels, `zoom` device pixels per scene unit, `pan` where the
    /// scene's origin lands. An edit frame is seen with its parallax layers moved and without
    /// onion skins, as the editor's monitor shows it. `false` when there is nothing to draw
    /// (no monitor, no frame yet, a prepared frame the cache no longer holds).
    pub fn render_monitor(&mut self, width: u32, height: u32, zoom: f64, pan_x: f64, pan_y: f64) -> Result<bool, JsError> {
        let Some(generation) = self.engine.generation() else { return Ok(false) };
        if self.views.monitor.is_none() {
            return Ok(false);
        }
        let (list, camera) = match self.shown.clone() {
            Shown::Nothing => return Ok(false),
            // Never evaluated here: the stage drew this tick from the cache a moment ago.
            Shown::Prepared(tick) => match self.engine.frame(tick) {
                Some(frame) => (frame.list.clone(), frame.camera),
                None => return Ok(false),
            },
            Shown::Edit { hidden, translations } => {
                let key = MonitorKey { generation: generation.id, edits: self.edits, effects: self.effects, hidden, translations };
                match self.views.monitor_list.as_ref().filter(|(known, _, _)| *known == key) {
                    Some((_, list, camera)) => (list.clone(), *camera),
                    None => {
                        let options = EditOptions { hidden: &key.hidden, translations: &key.translations, background: None, onions: &[], bypass_effects: !self.effects, camera: true, clock: Some(crate::clock), anchor_budget_ms: 6.0 };
                        let Some(list) = self.edit.draw_list(&generation, &options).map_err(js_error)? else { return Ok(false) };
                        let camera = self.edit.camera();
                        // Generated effects still being refined are built again next time.
                        self.views.monitor_list = (!self.edit.refining()).then(|| (key, list.clone(), camera));
                        (list, camera)
                    }
                }
            }
        };
        let doc = &generation.prepared.doc;
        let pan = [pan_x, pan_y];
        let framing = Framing {
            matrix: styk_eval::view::camera_matrix(doc, camera.as_ref(), zoom, pan),
            artboard: [pan[0], pan[1], pan[0] + doc.canvas[0] as f64 * zoom, pan[1] + doc.canvas[1] as f64 * zoom],
            zoom,
            pan,
        };
        let mut monitor = self.views.monitor.take().expect("checked above");
        let drawn = monitor.draw(self, &list, &view_of(framing, [0.0; 4]), width, height);
        self.views.monitor = Some(monitor);
        Ok(drawn)
    }

    /// Releases the camera monitor's canvas and renderer.
    pub fn detach_monitor(&mut self) {
        self.views.monitor = None;
        self.views.monitor_list = None;
    }

    /// Gives frame thumbnails a canvas, as [`Stage::attach_monitor`] does the monitor.
    pub fn attach_thumbnails(&mut self, canvas: web_sys::OffscreenCanvas) -> Result<bool, JsError> {
        if !self.is_webgpu {
            return Ok(false);
        }
        self.views.thumbnails = Some(Aux::create(self, canvas)?);
        Ok(true)
    }

    /// Asks the engine's workers for `tick` of the document at `revision`, for a thumbnail.
    /// 0: ready to draw; 1: being prepared (wait for the engine's progress, then ask again);
    /// 2: the engine holds another revision (or none). Fails for a tick past the end.
    pub fn prepare_thumbnail(&mut self, revision: f64, tick: u32) -> Result<u32, JsError> {
        let Some(frames) = self.thumbnail_frames(revision as u64) else { return Ok(STALE) };
        if tick >= frames.ticks() {
            return Err(js_error(format!("The document has no tick {tick} ({} ticks).", frames.ticks())));
        }
        if frames.ready(tick).is_some() {
            return Ok(READY);
        }
        // Frames asked for and never drawn (a cancelled thumbnail) give their places back.
        if frames.waiting() >= AHEAD {
            frames.forget();
        }
        frames.prefetch([tick]);
        Ok(WAIT)
    }

    /// Draws the thumbnail of `tick` of `revision` on a `width × height` card into the
    /// thumbnails' canvas. 0: drawn; 1: its frame or some of the document's pictures are not
    /// ready yet (they are uploaded as they come: wait for the engine's progress, then ask
    /// again); 2: the engine holds another revision.
    pub fn render_thumbnail(&mut self, revision: f64, tick: u32, width: u32, height: u32) -> Result<u32, JsError> {
        if !(1..=MAX_SIDE).contains(&width) || !(1..=MAX_SIDE).contains(&height) {
            return Err(js_error(format!("Thumbnails are 1 to {MAX_SIDE} pixels a side ({width}×{height} asked).")));
        }
        if self.views.thumbnails.is_none() {
            return Err(js_error("attach the thumbnails' canvas first"));
        }
        let Some(frames) = self.thumbnail_frames(revision as u64) else { return Ok(STALE) };
        let Some(frame) = frames.ready(tick) else {
            frames.prefetch([tick]);
            return Ok(WAIT);
        };
        let generation = frames.generation().clone();
        // A thumbnail never shows a frame without its pictures: they reach the atlas first.
        self.sync_images(&generation);
        if self.engine.images_pending() > 0 {
            return Ok(WAIT);
        }
        let framing = card(&generation.prepared.doc, frame.camera.as_ref(), width, height);
        let mut thumbnails = self.views.thumbnails.take().expect("checked above");
        let drawn = thumbnails.draw(self, &frame.list, &view_of(framing, CARD), width, height);
        self.views.thumbnails = Some(thumbnails);
        if drawn {
            // Drawn: its place among the frames prepared ahead is free again.
            let _ = self.views.frames.as_ref().and_then(|frames| frames.take(tick));
        }
        Ok(if drawn { READY } else { WAIT })
    }

    /// Releases the thumbnails' canvas, renderer and pinned frames.
    pub fn detach_thumbnails(&mut self) {
        self.views.thumbnails = None;
        self.views.frames = None;
    }
}

impl Stage {
    /// The frames of `revision`, pinned for thumbnails; `None` when the engine holds another.
    fn thumbnail_frames(&mut self, revision: u64) -> Option<&ExportFrames> {
        let generation = self.engine.generation().filter(|g| g.revision == revision)?;
        if !self.views.frames.as_ref().is_some_and(|frames| frames.generation().id == generation.id) {
            // A new revision: frames prepared for the old one are abandoned.
            self.views.frames = self.engine.export_frames(revision).ok();
        }
        self.views.frames.as_ref()
    }
}
