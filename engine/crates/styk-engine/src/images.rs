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

    /// A live drawing slot was written at the corner `(x, y)` of page `layer`.
    pub fn mark(&mut self, layer: u32, x: u32, y: u32) {
        self.places.insert(place(layer, x, y));
    }

    /// How many pictures the atlas holds.
    pub fn len(&self) -> usize {
        self.places.len()
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
    ready: std::collections::LinkedList<Arc<DecodedImage>>,
    epoch: u64,
    redecode_pending: bool,
    preparing: Option<DecodeScope>,
    remaining: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DecodeScope { lineage: u64, epoch: u64 }
impl DecodeScope {
    fn current(self, images: &Store) -> bool { (self.lineage, self.epoch) == (images.lineage, images.epoch) }
}

/// All exits (including worker unwind) release only this matching preparation.
/// A replaced lineage/epoch cannot clear a newer worker's selection barrier.
struct Preparation<'a> { engine: &'a Engine, scope: DecodeScope }
impl Drop for Preparation<'_> {
    fn drop(&mut self) {
        {
            let mut state = self.engine.inner.state.lock();
            if state.images.preparing == Some(self.scope) { state.images.preparing = None; state.images.redecode_pending = false; }
        }
        self.engine.inner.state.notify_all();
    }
}

impl Store {
    pub(crate) fn has_storage(&self) -> bool { !self.ready.is_empty() }

    pub(crate) fn for_publication(lineage: u64, count: usize) -> Self {
        Self { lineage, ready: Default::default(), remaining: count, epoch: 0, redecode_pending: false, preparing: None }
    }

    pub(crate) fn preparation_active(&self) -> bool { self.preparing.is_some() }
    pub(crate) fn cancel_preparation(&mut self) { self.preparing = None; }
    pub(crate) fn start_preparation(&mut self, scope: DecodeScope) {
        if scope.current(self) && self.redecode_pending { self.preparing = Some(scope); }
    }

    /// A later revision of the same document added `count` images.
    pub fn grow(&mut self, count: usize) {
        self.remaining += count;
    }
}

impl Engine {
    pub(crate) fn decode_image(&self, generation: &Arc<Generation>, index: usize) {
        let epoch = {
            let state = self.inner.state.lock();
            if state.images.lineage != generation.lineage { return }
            state.images.epoch
        };
        let mut ready = std::collections::LinkedList::new();
        if let Some(decoded) = styk_eval::assets::decode_image(&generation.prepared.doc, &generation.assets, index) { ready.push_back(Arc::new(decoded)); }
        let mut state = self.inner.state.lock();
        if state.images.lineage != generation.lineage || state.images.epoch != epoch { return }
        state.images.remaining = state.images.remaining.saturating_sub(1);
        state.images.ready.append(&mut ready);
    }

    /// Decodes drawing cel `cel` (every display level) for the atlas of `generation`.
    pub(crate) fn decode_cel(&self, generation: &Arc<Generation>, cel: usize) {
        let epoch = {
            let state = self.inner.state.lock();
            if state.images.lineage != generation.lineage { return }
            state.images.epoch
        };
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
        let mut ready: std::collections::LinkedList<_> = decoded.into_iter().map(Arc::new).collect();
        let mut state = self.inner.state.lock();
        if state.images.lineage != generation.lineage || state.images.epoch != epoch { return }
        state.images.remaining = state.images.remaining.saturating_sub(1);
        state.images.ready.append(&mut ready);
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
        let mut taken = std::collections::LinkedList::new();
        let mut bytes = 0;
        {
            let mut state = self.inner.state.lock();
            while let Some(image) = state.images.ready.back() {
                if !taken.is_empty() && image.rgba.len() > max_bytes.saturating_sub(bytes) { break }
                bytes += image.rgba.len();
                let tail = state.images.ready.len() - 1;
                taken.append(&mut state.images.ready.split_off(tail));
            }
        }
        // Node disposal and output allocation happen after the shared lock.
        taken.into_iter().collect()
    }

    /// Queues one coalesced preparation in the same ordinary worker queue.
    /// Pending counters change immediately, but dense task preparation/disposal
    /// never happens on the caller (a Stage may call this on the interface).
    pub fn redecode_images(&self) {
        let mut job = crate::TaskQueue::new();
        job.push_back(crate::OrdinaryTask::Redecode(DecodeScope { lineage: 0, epoch: 0 }));
        let retired = {
            let mut state = self.inner.state.lock();
            if self.inner.stopping.load(std::sync::atomic::Ordering::SeqCst) || state.images.redecode_pending { return }
            let Some(generation) = state.current.as_ref() else { return };
            let mut fresh = Store::for_publication(generation.lineage, generation.prepared.doc.images.len() + generation.assets.cels.len());
            fresh.epoch = state.images.epoch.wrapping_add(1);
            fresh.redecode_pending = true;
            *job.front_mut().expect("prebuilt placeholder") = crate::OrdinaryTask::Redecode(DecodeScope { lineage: fresh.lineage, epoch: fresh.epoch });
            let retired = std::mem::replace(&mut state.images, fresh);
            state.tasks.append(&mut job);
            retired
        };
        if retired.has_storage() { self.spawn_retained(move |_| drop(retired)); }
        self.inner.state.notify_all();
    }

    pub(crate) fn prepare_redecode(&self, scope: DecodeScope) {
        let _preparation = Preparation { engine: self, scope };
        loop {
            let (generation, epoch) = {
                let state = self.inner.state.lock();
                if self.inner.stopping.load(std::sync::atomic::Ordering::SeqCst) || !scope.current(&state.images) || !state.images.redecode_pending || state.images.preparing != Some(scope) { return }
                let Some(generation) = state.current.clone() else { return };
                (generation, state.images.epoch)
            };
            let mut tasks = crate::TaskQueue::new();
            for index in 0..generation.prepared.doc.images.len() {
                let generation = generation.clone();
                tasks.push_back(crate::OrdinaryTask::Run(Box::new(move |engine: &Engine| engine.decode_image(&generation, index))));
            }
            for &cel in generation.assets.cels.keys() {
                let generation = generation.clone();
                tasks.push_back(crate::OrdinaryTask::Run(Box::new(move |engine: &Engine| engine.decode_cel(&generation, cel))));
            }
            let mut fresh = Store::for_publication(generation.lineage, generation.prepared.doc.images.len() + generation.assets.cels.len());
            fresh.epoch = epoch.wrapping_add(1);
            let retired = {
                let mut state = self.inner.state.lock();
                if self.inner.stopping.load(std::sync::atomic::Ordering::SeqCst) || !scope.current(&state.images) || !state.images.redecode_pending || state.images.preparing != Some(scope) { return }
                if state.images.epoch != epoch || !state.current.as_ref().is_some_and(|current| Arc::ptr_eq(current, &generation)) { None }
                else {
                    // Decode work precedes commands queued after its preparation
                    // placeholder, exactly as the original immediate enqueue.
                    tasks.append(&mut state.tasks);
                    std::mem::swap(&mut state.tasks, &mut tasks);
                    Some(std::mem::replace(&mut state.images, fresh))
                }
            };
            if let Some(retired) = retired { drop(retired); self.inner.state.notify_all(); return }
        }
    }

    /// Images of the open document still being decoded or waiting for upload.
    pub fn images_pending(&self) -> usize {
        let state = self.inner.state.lock();
        state.images.remaining + state.images.ready.len()
    }

    /// Try-only scalar diagnostic; Busy is distinct from complete resources.
    pub fn try_images_pending(&self) -> Option<usize> {
        let state = self.inner.state.try_lock()?;
        Some(state.images.remaining + state.images.ready.len())
    }

    /// Images of the open document still being decoded, and decoded ones waiting for
    /// [`Engine::take_images`].
    pub fn images_backlog(&self) -> (usize, usize) {
        let state = self.inner.state.lock();
        (state.images.remaining, state.images.ready.len())
    }
}

#[cfg(test)]
mod tests;
