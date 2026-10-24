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
    epoch: u64,
    ready: HashMap<u32, Arc<FrameData>>,
    inflight: HashSet<u32>,
    /// Requested frames wait here while this renderer preloads its pictures.
    pending: Vec<u32>,
    images: Vec<Arc<DecodedImage>>,
    images_left: usize,
}

/// Starts the bounded set of requested frames once their resource rectangles
/// are authoritative. No thread waits for PNG decoding to finish.
fn prepare_pending(engine: &Engine, generation: &Arc<Generation>, state: &Arc<Shared<State>>, ended: &Arc<AtomicBool>) {
    let (ticks, epoch) = {
        let mut state = state.lock();
        if ended.load(Ordering::Relaxed) || state.images_left != 0 {
            return;
        }
        (std::mem::take(&mut state.pending), state.epoch)
    };
    for tick in ticks {
        let (generation, state, ended, clock) = (generation.clone(), state.clone(), ended.clone(), engine.inner.clock);
        engine.spawn(move |_| {
            if ended.load(Ordering::Relaxed) {
                return;
            }
            let data = produce(&generation, tick as f64, &[], true, clock);
            let mut state = state.lock();
            if ended.load(Ordering::Relaxed) || epoch != state.epoch {
                return;
            }
            state.inflight.remove(&tick);
            state.ready.insert(tick, Arc::new(data));
        });
    }
}

fn pictures_ready(engine: &Engine, generation: &Arc<Generation>, state: &Arc<Shared<State>>, ended: &Arc<AtomicBool>, epoch: u64, images: impl IntoIterator<Item = DecodedImage>) {
    {
        let mut state = state.lock();
        if ended.load(Ordering::Relaxed) || epoch != state.epoch {
            return;
        }
        state.images_left = state.images_left.saturating_sub(1);
        state.images.extend(images.into_iter().map(Arc::new));
    }
    prepare_pending(engine, generation, state, ended);
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

    /// The artwork of `tick` as the drawing workspace samples it for fills and the magic wand:
    /// the figures the document shows, on transparency (no background), without effects, the
    /// scene camera or parallax. Evaluated on the calling thread: a worker's.
    pub fn composite(&self, tick: u32) -> Arc<styk_eval::drawlist::DrawList> {
        let prepared = &self.generation.prepared;
        let frame = prepared.evaluate(tick as f64);
        let scene = styk_eval::geometry::build(&prepared.doc, &frame);
        let hidden = crate::frames::hidden_slots(&self.generation, &frame, &[]);
        let overrides = styk_eval::drawlist::Overrides { hidden: &hidden, background: Some(0), bypass_effects: true, ..Default::default() };
        Arc::new(styk_eval::drawlist::encode_with(&prepared.doc, &self.generation.assets, &scene, &overrides))
    }

    /// Decodes every image and drawing cel of the document on the engine's workers.
    pub fn decode_images(&self) {
        self.decode_images_since(None);
    }

    /// Decodes on the engine's workers the pictures `since` did not have: the images an
    /// earlier generation of the same lineage (whose decoded pictures the caller keeps, at
    /// the same atlas places, `Assets::extended`) did not hold, and the drawing cels it had
    /// not placed. Every picture when `since` is `None` or of another lineage.
    pub fn decode_images_since(&self, since: Option<&Generation>) {
        let generation = &self.generation;
        let count = generation.prepared.doc.images.len();
        let since = since.filter(|earlier| earlier.lineage == generation.lineage);
        let known = since.map_or(0, |earlier| earlier.prepared.doc.images.len().min(count));
        let cels: Vec<usize> = generation.assets.cels.keys().copied().filter(|cel| !since.is_some_and(|earlier| earlier.assets.cels.contains_key(cel))).collect();
        let epoch = {
            let mut state = self.state.lock();
            let remaining = count - known + cels.len();
            if remaining != 0 {
                state.epoch = state.epoch.wrapping_add(1);
                let mut redo: Vec<_> = state.ready.keys().chain(state.inflight.iter()).copied().collect();
                redo.sort_unstable();
                redo.dedup();
                state.inflight = redo.iter().copied().collect();
                state.pending = redo;
                state.ready.clear();
            }
            state.images_left = remaining;
            state.epoch
        };
        for index in known..count {
            let (generation, state, ended) = (generation.clone(), self.state.clone(), self.ended.clone());
            self.engine.spawn(move |engine| {
                if ended.load(Ordering::Relaxed) {
                    return;
                }
                let image = styk_eval::assets::decode_image(&generation.prepared.doc, &generation.assets, index);
                pictures_ready(engine, &generation, &state, &ended, epoch, image);
            });
        }
        for cel in cels {
            let (generation, state, ended) = (generation.clone(), self.state.clone(), self.ended.clone());
            self.engine.spawn(move |engine| {
                if ended.load(Ordering::Relaxed) {
                    return;
                }
                let tiles = match generation.assets.cels.get(&cel) {
                    Some((tiles, _)) => {
                        let before = tiles.artwork_bounds();
                        let decoded = styk_eval::assets::decode_cel(&generation.assets, tiles);
                        if tiles.artwork_bounds() != before {
                            engine.drawing_bounds_changed(&generation, tiles);
                        }
                        decoded
                    }
                    None => Vec::new(),
                };
                pictures_ready(engine, &generation, &state, &ended, epoch, tiles);
            });
        }
        prepare_pending(&self.engine, generation, &self.state, &self.ended);
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
                state.pending.push(tick);
            }
        }
        prepare_pending(&self.engine, &self.generation, &self.state, &self.ended);
    }

    /// The frame of `tick`: prepared ahead, or evaluated here when no worker has it;
    /// `None` while a worker is still preparing it (wait for the engine's progress counter).
    pub fn take(&self, tick: u32) -> Option<Arc<FrameData>> {
        let epoch = {
            let mut state = self.state.lock();
            if state.images_left != 0 {
                return None;
            }
            if let Some(frame) = state.ready.remove(&tick) {
                return Some(frame);
            }
            if state.inflight.contains(&tick) {
                return None;
            }
            state.epoch
        };
        if tick >= self.ticks() {
            return None;
        }
        let data = Arc::new(produce(&self.generation, tick as f64, &[], true, self.engine.inner.clock));
        let state = self.state.lock();
        (state.epoch == epoch && state.images_left == 0).then_some(data)
    }

    /// The frame of `tick` if a worker has prepared it, left in place; never evaluated here
    /// (the interface thread asks this for thumbnails, which it may not evaluate, D7).
    pub fn ready(&self, tick: u32) -> Option<Arc<FrameData>> {
        let state = self.state.lock();
        if state.images_left != 0 { None } else { state.ready.get(&tick).cloned() }
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
        state.pending.clear();
        state.inflight.clear();
    }
}
