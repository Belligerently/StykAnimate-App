//! CPU rasterizer for StykAnimate draw lists: the GPU renderer's pipeline (styk-render)
//! carried out on the CPU, so a frame drawn here is the frame the stage, thumbnails and
//! exports draw on a GPU. For devices with neither WebGPU nor WebGL2 (thumbnails, library
//! previews and exports in the app) and for tools (`styk raster`, `styk export --cpu`).
//!
//! It follows styk-render rule for rule:
//!
//! * **Layers**: every isolation group is drawn where styk-render's plan puts it for the
//!   view (`styk_render::effects::plan` and `record`, built without the `gpu` feature):
//!   translucent figures on their parent's grid, raster entries (text, 3D artwork) and
//!   filtered artwork on the editor's raster grid at its density, the camera image through
//!   the scene camera. Each into a target of its own, composited at its place in painter
//!   order, sampled bilinearly (to the nearest texel with crisp edges).
//! * **Samples** (`target`, `coverage`): four a pixel at the standard positions of 4×
//!   multisampling (one at the centre for crisp edges), each blended on its own in RGBA8 and
//!   averaged when resolved; triangles and quads covered by the GPU's rasterization rules
//!   (sub-pixel snapping, the top-left rule), so meshes, glyph fans and adjacent polygons
//!   show no seams.
//! * **Shapes** (`shade`, `frame`): strokes, arcs, polylines, discs and rings covered by
//!   distance at each pixel's centre as `prim.wgsl` covers them; custom paints shaded by
//!   [`styk_draw::sample_paint`]; sprites, drawing cels and backgrounds sampled as the GPU
//!   samples them.
//! * **3D faces**: depth runs depth tested per sample at 24 bits, a face drawn over what
//!   is there when it is not behind it (`less-equal`), as the GPU's depth test does.
//! * **Lone glyphs** (`glyphs`): from masks filled at four samples a texel for the view's
//!   raster density, kept across frames within a budget; their triangles where the view
//!   allows no mask.
//! * **Generated effects** (`generators`): the GPU's quads, shaded by its fragment arithmetic.
//! * **Filter effects** (`effects`): each layer's effect recipe
//!   (`styk_render::effects::recipe`) run pass by pass with the arithmetic of `passes.wgsl`.
//!
//! A [`Raster`] keeps what outlives a frame (glyph masks); [`render`] draws one frame alone.
//! Nothing here blocks on a lock or a thread: callers run it where it may take its time
//! (a worker, a tool).

mod coverage;
mod effects;
mod frame;
mod generators;
mod glyphs;
mod shade;
mod target;

use std::collections::HashMap;
use std::sync::Arc;

use styk_draw::DrawList;
use styk_eval::assets::{Assets, DecodedImage, PAGE};

pub use frame::Stats;
pub use styk_render::{Edges, ViewParams};

use shade::Texture;

/// Decoded pixels of a document's images, keyed by their atlas place (the draw list
/// addresses images by atlas rectangle, as the GPU samples them). The decoded images are
/// shared, not copied.
#[derive(Default)]
pub struct Images {
    /// Premultiplied sprites with their one-texel gutter repeating their edges.
    sprites: HashMap<(u32, u32, u32), Arc<DecodedImage>>,
    /// Drawing cel tiles: straight alpha, with a two-texel border of neighbouring texels.
    tiles: HashMap<(u32, u32, u32), Arc<DecodedImage>>,
}

impl Images {
    /// Adds a decoded image (`styk_eval::assets::decode_image` / `decode_cel`), copied.
    pub fn add(&mut self, assets: &Assets, image: &DecodedImage) {
        self.add_shared(assets, Arc::new(DecodedImage { index: image.index, drawing: image.drawing, width: image.width, height: image.height, rgba: image.rgba.clone() }));
    }

    /// Adds a decoded image the engine already holds.
    pub fn add_shared(&mut self, assets: &Assets, image: Arc<DecodedImage>) {
        let entry = if image.drawing { assets.drawing.get(image.index as usize) } else { assets.atlas.get(image.index as usize) };
        let Some(e) = entry.filter(|e| e.valid) else { return };
        if image.rgba.len() < image.width as usize * image.height as usize * 4 || image.width == 0 || image.height == 0 {
            return;
        }
        let key = (e.layer, e.x, e.y);
        if image.drawing {
            self.tiles.insert(key, image);
        } else {
            self.sprites.insert(key, image);
        }
    }

    pub fn len(&self) -> usize {
        self.sprites.len() + self.tiles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Bytes of the pixels held.
    pub fn bytes(&self) -> usize {
        self.sprites.values().chain(self.tiles.values()).map(|i| i.rgba.len()).sum()
    }

    fn texture(image: &DecodedImage, straight: bool) -> Texture<'_> {
        Texture { width: image.width as usize, height: image.height as usize, data: image.rgba.as_chunks::<4>().0, straight }
    }

    pub(crate) fn sprite(&self, key: (u32, u32, u32)) -> Option<Texture<'_>> {
        self.sprites.get(&key).map(|image| Images::texture(image, false))
    }

    pub(crate) fn tile(&self, key: (u32, u32, u32)) -> Option<Texture<'_>> {
        self.tiles.get(&key).map(|image| Images::texture(image, true))
    }
}

/// The atlas place a primitive's uv rectangle starts at.
pub(crate) fn atlas_key(layer: f32, uv: [f32; 4]) -> (u32, u32, u32) {
    let p = PAGE as f32;
    (layer.round() as u32, (uv[0] * p).round() as u32, (uv[1] * p).round() as u32)
}

/// A rasterized frame: premultiplied RGBA8 rows.
#[derive(Clone, Debug)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Frame {
    /// Straight RGBA8 (encoders take straight alpha), rounded as exports round
    /// (`styk_export::pixels`: to nearest, halves up).
    pub fn straight(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.pixels.len());
        for p in self.pixels.as_chunks::<4>().0 {
            let a = p[3] as u32;
            if a == 0 {
                out.extend_from_slice(&[0; 4]);
                continue;
            }
            let c = |v: u8| ((2 * 255 * v as u32 + a) / (2 * a)).min(255) as u8;
            out.extend_from_slice(&[c(p[0]), c(p[1]), c(p[2]), p[3]]);
        }
        out
    }
}

/// A CPU renderer: draws frames as styk-render's `Renderer` does, keeping lone glyphs'
/// masks across frames.
pub struct Raster {
    edges: Edges,
    glyphs: glyphs::Masks,
    pub stats: Stats,
}

impl Raster {
    pub fn new(edges: Edges) -> Raster {
        Raster { edges, glyphs: glyphs::Masks::default(), stats: Stats::default() }
    }

    /// How this renderer covers edges.
    pub fn edges(&self) -> Edges {
        self.edges
    }

    /// Draws `list` seen through `view` at `size`, with `images`.
    pub fn render(&mut self, list: &DrawList, images: &Images, view: &ViewParams, size: [u32; 2]) -> Frame {
        let size = [size[0].max(1), size[1].max(1)];
        let crisp = self.edges == Edges::Crisp;
        // Lone glyphs: the masks this view calls for (filled now when no frame has yet).
        let resolution = styk_render::effects::plan::raster_resolution(view.matrix[0].hypot(view.matrix[1]));
        let glyphs = if list.glyphs.is_empty() { Vec::new() } else { self.glyphs.prepare(list, resolution, !crisp) };
        let mut frame = frame::Frame::new(list, images, view, size, crisp, glyphs);
        let pixels = frame.draw();
        self.stats = frame::Stats { glyph_masks: self.glyphs.count() as u32, glyph_masks_filled: self.glyphs.filled, glyph_bytes: self.glyphs.bytes(), ..frame.stats };
        let mut bytes = Vec::with_capacity(pixels.len() * 4);
        for p in pixels {
            bytes.extend_from_slice(&p);
        }
        Frame { width: size[0], height: size[1], pixels: bytes }
    }

    /// Lets go of the glyph masks kept between frames.
    pub fn release(&mut self) {
        self.glyphs = glyphs::Masks::default();
    }
}

/// Draws one frame of `list` seen through `view` at `size`, covering edges as `edges` says.
pub fn render(list: &DrawList, images: &Images, view: &ViewParams, size: [u32; 2], edges: Edges) -> Frame {
    Raster::new(edges).render(list, images, view, size)
}

/// Every image of `assets` decoded for the CPU (tools and exports).
pub fn decode_all(doc: &styk_eval::doc::Document, assets: &Assets) -> Images {
    let mut images = Images::default();
    for index in 0..doc.images.len() {
        if let Some(image) = styk_eval::assets::decode_image(doc, assets, index) {
            images.add_shared(assets, Arc::new(image));
        }
    }
    for (cel, _) in assets.cels.values() {
        for image in styk_eval::assets::decode_cel(assets, cel) {
            images.add_shared(assets, Arc::new(image));
        }
    }
    images
}
