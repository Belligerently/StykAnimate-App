//! Image pixels, decoded in the background and handed to the renderer in pieces.

use std::sync::Arc;

use crate::{Engine, Generation};

pub use styk_eval::assets::DecodedImage;

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
            Some((tiles, _)) => styk_eval::assets::decode_cel(&generation.assets, tiles),
            None => Vec::new(),
        };
        let mut state = self.inner.state.lock();
        if state.images.lineage != generation.lineage {
            return;
        }
        state.images.remaining = state.images.remaining.saturating_sub(1);
        state.images.ready.extend(decoded.into_iter().map(Arc::new));
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
}
