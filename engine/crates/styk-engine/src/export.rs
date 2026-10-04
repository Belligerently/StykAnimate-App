//! Frames for an export (and for thumbnails, which pin a revision the same way).
//!
//! An export draws one revision of the document from its first frame to its last, while
//! the interface may go on drawing the stage. [`ExportFrames`] pins the generation the
//! export started from and evaluates its ticks as playback does, with effects and without
//! the figures hidden for the editing session (exports show what the document shows: its
//! hidden layers stay hidden), a few ticks ahead on the engine's workers. Its pictures are
//! decoded for the export's own renderer, which has its own GPU device.
//!
//! Memory is bounded: at most [`AHEAD`] frames wait to be taken, and decoded images wait
//! only until the renderer uploads them.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use styk_eval::assets::DecodedImage;

use crate::frames::{produce, FrameData};
use crate::sync::Shared;
use crate::{Engine, Generation};

/// Frames prepared ahead of the one the export draws, at most (the export worker draws as
/// many as six ahead while earlier ones are read back).
pub const AHEAD: usize = 8;

#[derive(Default)]
struct State {
    ready: HashMap<u32, Arc<FrameData>>,
    inflight: HashSet<u32>,
    images: Vec<Arc<DecodedImage>>,
    images_left: usize,
}

/// The frames and pictures of one export. Dropping it abandons work not yet done.
pub struct ExportFrames {
    engine: Engine,
    generation: Arc<Generation>,
    state: Arc<Shared<State>>,
    /// Set when the export ends: tasks still queued do nothing.
    ended: Arc<AtomicBool>,
}

impl Engine {
    /// Pins the open document for an export, when it is the interface's `revision`.
    pub fn export_frames(&self, revision: u64) -> Result<ExportFrames, String> {
        let generation = self.generation().ok_or("no document is open")?;
        if generation.revision != revision {
            return Err(format!("the engine holds revision {} of the document, the export is of revision {revision}", generation.revision));
        }
        if generation.playable_ticks() == 0 {
            return Err("the document has no frames yet".into());
        }
        Ok(ExportFrames { engine: self.clone(), generation, state: Arc::new(Shared::new(State::default())), ended: Arc::new(AtomicBool::new(false)) })
    }
}

impl ExportFrames {
    pub fn generation(&self) -> &Arc<Generation> {
        &self.generation
    }

    /// Ticks of the pinned document.
    pub fn ticks(&self) -> u32 {
        self.generation.playable_ticks()
    }

    /// Decodes every image and drawing cel of the document on the engine's workers.
    pub fn decode_images(&self) {
        let generation = &self.generation;
        let count = generation.prepared.doc.images.len();
        let cels: Vec<usize> = generation.assets.cels.keys().copied().collect();
        self.state.lock().images_left = count + cels.len();
        for index in 0..count {
            let (generation, state, ended) = (generation.clone(), self.state.clone(), self.ended.clone());
            self.engine.spawn(move |_| {
                if ended.load(Ordering::Relaxed) {
                    return;
                }
                let image = styk_eval::assets::decode_image(&generation.prepared.doc, &generation.assets, index);
                let mut state = state.lock();
                state.images_left = state.images_left.saturating_sub(1);
                state.images.extend(image.map(Arc::new));
            });
        }
        for cel in cels {
            let (generation, state, ended) = (generation.clone(), self.state.clone(), self.ended.clone());
            self.engine.spawn(move |_| {
                if ended.load(Ordering::Relaxed) {
                    return;
                }
                let tiles = match generation.assets.cels.get(&cel) {
                    Some((tiles, _)) => styk_eval::assets::decode_cel(&generation.assets, tiles),
                    None => Vec::new(),
                };
                let mut state = state.lock();
                state.images_left = state.images_left.saturating_sub(1);
                state.images.extend(tiles.into_iter().map(Arc::new));
            });
        }
    }

    /// Decoded pictures not yet taken, up to `max_bytes` of pixels (at least one).
    pub fn take_images(&self, max_bytes: usize) -> Vec<Arc<DecodedImage>> {
        let mut state = self.state.lock();
        let mut taken = Vec::new();
        let mut bytes = 0;
        while let Some(image) = state.images.last() {
            if !taken.is_empty() && bytes + image.rgba.len() > max_bytes {
                break;
            }
            bytes += image.rgba.len();
            taken.push(state.images.pop().expect("checked above"));
        }
        taken
    }

    /// Pictures still being decoded or waiting to be taken.
    pub fn images_pending(&self) -> usize {
        let state = self.state.lock();
        state.images_left + state.images.len()
    }

    /// Asks the engine's workers for `ticks` (the ones after the frame being drawn), at most
    /// [`AHEAD`] of them waiting or in progress at a time.
    pub fn prefetch(&self, ticks: impl IntoIterator<Item = u32>) {
        let total = self.ticks();
        let mut spawn = Vec::new();
        {
            let mut state = self.state.lock();
            for tick in ticks {
                if state.ready.len() + state.inflight.len() >= AHEAD {
                    break;
                }
                if tick >= total || state.ready.contains_key(&tick) || state.inflight.contains(&tick) {
                    continue;
                }
                state.inflight.insert(tick);
                spawn.push(tick);
            }
        }
        for tick in spawn {
            let (generation, state, ended, clock) = (self.generation.clone(), self.state.clone(), self.ended.clone(), self.engine.inner.clock);
            self.engine.spawn(move |_| {
                if ended.load(Ordering::Relaxed) {
                    state.lock().inflight.remove(&tick);
                    return;
                }
                let data = produce(&generation, tick, &[], true, clock);
                let mut state = state.lock();
                state.inflight.remove(&tick);
                state.ready.insert(tick, Arc::new(data));
            });
        }
    }

    /// The frame of `tick`: prepared ahead, or evaluated here when no worker has it;
    /// `None` while a worker is still preparing it (wait for the engine's progress counter).
    pub fn take(&self, tick: u32) -> Option<Arc<FrameData>> {
        {
            let mut state = self.state.lock();
            if let Some(frame) = state.ready.remove(&tick) {
                return Some(frame);
            }
            if state.inflight.contains(&tick) {
                return None;
            }
        }
        if tick >= self.ticks() {
            return None;
        }
        Some(Arc::new(produce(&self.generation, tick, &[], true, self.engine.inner.clock)))
    }

    /// The frame of `tick` if a worker has prepared it, left in place; never evaluated here
    /// (the interface thread asks this for thumbnails, which it may not evaluate, D7).
    pub fn ready(&self, tick: u32) -> Option<Arc<FrameData>> {
        self.state.lock().ready.get(&tick).cloned()
    }

    /// Frames prepared ahead and not yet taken, or being prepared (at most [`AHEAD`]).
    pub fn waiting(&self) -> usize {
        let state = self.state.lock();
        state.ready.len() + state.inflight.len()
    }

    /// Drops frames prepared for ticks the export will not draw (another range, a retry).
    pub fn forget(&self) {
        self.state.lock().ready.clear();
    }
}

impl Drop for ExportFrames {
    fn drop(&mut self) {
        self.ended.store(true, Ordering::Relaxed);
        let mut state = self.state.lock();
        state.ready.clear();
        state.images.clear();
    }
}
