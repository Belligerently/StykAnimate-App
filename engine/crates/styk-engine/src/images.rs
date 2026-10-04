//! Image pixels, decoded in the background and handed to the renderer in pieces.

use std::collections::HashSet;
use std::sync::Arc;

use styk_draw::{DrawList, K_BG_IMAGE, K_IMAGE};
use styk_eval::assets::{AtlasEntry, Assets, PAGE};

use crate::{Engine, Generation};

pub use styk_eval::assets::DecodedImage;

/// Which places of a sprite atlas hold their pixels: what a renderer that takes decoded
/// images a slice at a time ([`Engine::take_images`]) has uploaded, so it can tell
/// whether a frame it drew lacks some of its pictures and should be drawn again once
/// they arrive (the stage, `styk-wasm`). A few bytes per picture of the document.
#[derive(Default)]
pub struct Uploaded {
    places: HashSet<u64>,
}

/// An atlas place by its page and corner (the gutter excluded), as records name it.
fn place(layer: u32, x: u32, y: u32) -> u64 {
    (layer as u64) << 32 | (y as u64) << 16 | x as u64
}

/// The place a record's atlas rectangle starts at: its page and `uv` corner.
fn record_place(layer: f32, u0: f32, v0: f32) -> u64 {
    let page = PAGE as f32;
    place(layer as u32, (u0 * page).round() as u32, (v0 * page).round() as u32)
}

impl Uploaded {
    /// The atlas was allocated again: it holds nothing.
    pub fn clear(&mut self) {
        self.places.clear();
    }

    /// `image` was written into the atlas `assets` lays out.
    pub fn record(&mut self, assets: &Assets, image: &DecodedImage) {
        let entry: Option<&AtlasEntry> = if image.drawing { assets.drawing.get(image.index as usize) } else { assets.atlas.get(image.index as usize) };
        if let Some(e) = entry.filter(|e| e.valid) {
            self.places.insert(place(e.layer, e.x, e.y));
        }
    }

    /// Whether `list` samples an atlas place that holds no pixels yet: a picture or a
    /// drawing cel tile (every display level of a cel is in a list) not uploaded.
    pub fn lacks(&self, list: &DrawList) -> bool {
        list.prims.iter().any(|p| match p.kind & 255 {
            K_IMAGE => !self.places.contains(&record_place(p.b[2], p.c[0], p.c[1])),
            K_BG_IMAGE => !self.places.contains(&record_place(p.a[2], p.c[0], p.c[1])),
            _ => false,
        })
    }
}

#[derive(Default)]
pub(crate) struct Store {
    /// The document lineage these images belong to.
    lineage: u64,
    /// Decoded and not yet taken by the renderer.
    ready: Vec<Arc<DecodedImage>>,
    remaining: usize,
}

impl Store {
    pub fn reset(&mut self, lineage: u64, count: usize) {
        self.lineage = lineage;
        self.ready.clear();
        self.remaining = count;
    }

    /// A later revision of the same document added `count` images.
    pub fn grow(&mut self, count: usize) {
        self.remaining += count;
    }
}

impl Engine {
    pub(crate) fn decode_image(&self, generation: &Arc<Generation>, index: usize) {
        let decoded = styk_eval::assets::decode_image(&generation.prepared.doc, &generation.assets, index);
        let mut state = self.inner.state.lock();
        if state.images.lineage != generation.lineage {
            return;
        }
        state.images.remaining = state.images.remaining.saturating_sub(1);
        if let Some(image) = decoded {
            state.images.ready.push(Arc::new(image));
        }
    }

    /// Decodes drawing cel `cel` (every display level) for the atlas of `generation`.
    pub(crate) fn decode_cel(&self, generation: &Arc<Generation>, cel: usize) {
        let decoded = match generation.assets.cels.get(&cel) {
            Some((tiles, _)) => {
                let before = tiles.artwork_bounds();
                let decoded = styk_eval::assets::decode_cel(&generation.assets, tiles);
                if tiles.artwork_bounds() != before {
                    self.drawing_bounds_changed(generation, tiles);
                }
                decoded
            }
            None => Vec::new(),
        };
        let mut state = self.inner.state.lock();
        if state.images.lineage != generation.lineage {
            return;
        }
        state.images.remaining = state.images.remaining.saturating_sub(1);
        state.images.ready.extend(decoded.into_iter().map(Arc::new));
    }

    /// Resource workers can finish an earlier revision's decode after a commit.
    /// Its correction also applies to a current revision that shares that cel.
    pub(crate) fn drawing_bounds_changed(&self, generation: &Generation, cel: &Arc<styk_eval::doc::DrawingTiles>) {
        {
            let mut state = self.inner.state.lock();
            let retained = state.current.as_ref().is_some_and(|current| {
                current.lineage == generation.lineage && current.assets.cels.contains_key(&(Arc::as_ptr(cel) as usize))
            });
            if !retained {
                return;
            }
            state.frames.invalidate_resources();
        }
        self.inner.state.notify_all();
    }

    /// Decoded images the renderer has not uploaded yet, up to `max_bytes` of pixels
    /// (at least one image), so one frame never uploads more than it can afford.
    pub fn take_images(&self, max_bytes: usize) -> Vec<Arc<DecodedImage>> {
        let mut state = self.inner.state.lock();
        let mut taken = Vec::new();
        let mut bytes = 0;
        while let Some(image) = state.images.ready.last() {
            if !taken.is_empty() && bytes + image.rgba.len() > max_bytes {
                break;
            }
            bytes += image.rgba.len();
            taken.push(state.images.ready.pop().expect("checked above"));
        }
        taken
    }

    /// Queues every image of the open document for decoding again. The renderer asks
    /// for this when it had to allocate a larger atlas, which starts empty.
    pub fn redecode_images(&self) {
        {
            let mut state = self.inner.state.lock();
            let Some(generation) = state.current.clone() else { return };
            let count = generation.prepared.doc.images.len();
            state.images.reset(generation.lineage, count + generation.assets.cels.len());
            for index in 0..count {
                let generation = generation.clone();
                state.tasks.push_back(Box::new(move |engine: &Engine| engine.decode_image(&generation, index)));
            }
            for &cel in generation.assets.cels.keys() {
                let generation = generation.clone();
                state.tasks.push_back(Box::new(move |engine: &Engine| engine.decode_cel(&generation, cel)));
            }
        }
        self.inner.state.notify_all();
    }

    /// Images of the open document still being decoded or waiting for upload.
    pub fn images_pending(&self) -> usize {
        let state = self.inner.state.lock();
        state.images.remaining + state.images.ready.len()
    }

    /// Images of the open document still being decoded, and decoded ones waiting for
    /// [`Engine::take_images`].
    pub fn images_backlog(&self) -> (usize, usize) {
        let state = self.inner.state.lock();
        (state.images.remaining, state.images.ready.len())
    }
}
