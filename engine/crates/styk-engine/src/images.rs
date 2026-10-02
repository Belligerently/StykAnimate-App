//! Image pixels, decoded in the background and handed to the renderer in pieces.

use std::sync::Arc;

use crate::{Engine, Generation};

pub use styk_eval::assets::DecodedImage;

#[derive(Default)]
pub(crate) struct Store {
    generation: u64,
    /// Decoded and not yet taken by the renderer.
    ready: Vec<Arc<DecodedImage>>,
    remaining: usize,
}

impl Store {
    pub fn reset(&mut self, generation: u64, count: usize) {
        self.generation = generation;
        self.ready.clear();
        self.remaining = count;
    }
}

impl Engine {
    pub(crate) fn decode_image(&self, generation: &Arc<Generation>, index: usize) {
        let decoded = styk_eval::assets::decode_image(&generation.prepared.doc, &generation.assets, index);
        let mut state = self.inner.state.lock();
        if state.images.generation != generation.id {
            return;
        }
        state.images.remaining = state.images.remaining.saturating_sub(1);
        if let Some(image) = decoded {
            state.images.ready.push(Arc::new(image));
        }
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

    /// Images of the open document still being decoded or waiting for upload.
    pub fn images_pending(&self) -> usize {
        let state = self.inner.state.lock();
        state.images.remaining + state.images.ready.len()
    }
}
