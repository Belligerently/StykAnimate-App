//! Frame preparation and its cache.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use styk_eval::doc::Camera;
use styk_eval::drawlist::{DrawList, encode};
use styk_eval::geometry::build;

use crate::{Clock, Engine, Generation};

/// A prepared frame: everything the renderer needs for one tick.
pub struct FrameData {
    pub generation: u64,
    pub tick: u32,
    pub list: Arc<DrawList>,
    /// The scene's camera at this tick.
    pub camera: Option<Camera>,
    /// Time spent preparing it.
    pub ms: f32,
}

/// Preparation totals since the document opened.
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameStats {
    pub prepared: u64,
    pub total_ms: f64,
    pub worst_ms: f32,
    /// Frames prepared on the calling thread because no worker had them ready.
    pub inline: u64,
    pub cached: usize,
    pub cached_bytes: usize,
}

pub(crate) fn produce(generation: &Generation, tick: u32, clock: Clock) -> FrameData {
    let started = clock();
    let prepared = &generation.prepared;
    let frame = prepared.evaluate(tick as f64);
    let scene = build(&prepared.doc, &frame);
    let list = Arc::new(encode(&prepared.doc, &generation.assets, &scene));
    FrameData { generation: generation.id, tick, list, camera: scene.camera, ms: (clock() - started) as f32 }
}

struct Entry {
    data: Arc<FrameData>,
    bytes: usize,
}

pub(crate) struct Cache {
    generation: u64,
    entries: HashMap<u32, Entry>,
    bytes: usize,
    budget: usize,
    /// Ticks to prepare, most urgent first.
    wanted: Vec<u32>,
    inflight: HashSet<u32>,
    /// The tick playback is at: the cache evicts what is farthest ahead of it.
    playhead: u32,
    total: u32,
    stats: FrameStats,
}

impl Cache {
    pub fn new(budget: usize) -> Cache {
        Cache { generation: 0, entries: HashMap::new(), bytes: 0, budget, wanted: Vec::new(), inflight: HashSet::new(), playhead: 0, total: 1, stats: FrameStats::default() }
    }

    pub fn reset(&mut self, generation: u64) {
        self.generation = generation;
        self.entries.clear();
        self.bytes = 0;
        self.wanted.clear();
        self.inflight.clear();
        self.stats = FrameStats::default();
    }

    /// The next tick a worker should prepare, marked as in progress.
    pub fn take_wanted(&mut self) -> Option<u32> {
        let at = self.wanted.iter().position(|tick| !self.entries.contains_key(tick) && !self.inflight.contains(tick))?;
        let tick = self.wanted[at];
        self.wanted.drain(..=at);
        self.inflight.insert(tick);
        Some(tick)
    }

    /// Stores a finished frame, unless the document changed while it was prepared.
    pub fn finish(&mut self, generation: u64, tick: u32, data: Option<FrameData>) -> Option<Arc<FrameData>> {
        if generation != self.generation {
            return None;
        }
        self.inflight.remove(&tick);
        let data = Arc::new(data?);
        self.stats.prepared += 1;
        self.stats.total_ms += data.ms as f64;
        self.stats.worst_ms = self.stats.worst_ms.max(data.ms);
        let bytes = data.list.bytes() + 256;
        if let Some(old) = self.entries.insert(tick, Entry { data: data.clone(), bytes }) {
            self.bytes -= old.bytes;
        }
        self.bytes += bytes;
        self.trim();
        Some(data)
    }

    /// Keeps the cache within its budget by dropping the frames playback will reach last.
    fn trim(&mut self) {
        if self.bytes <= self.budget {
            return;
        }
        let (total, playhead) = (self.total.max(1), self.playhead);
        let mut ticks: Vec<u32> = self.entries.keys().copied().collect();
        ticks.sort_by_key(|&tick| std::cmp::Reverse((tick + total - playhead % total) % total));
        for tick in ticks {
            if self.bytes <= self.budget / 10 * 9 {
                break;
            }
            if tick != playhead
                && let Some(entry) = self.entries.remove(&tick)
            {
                self.bytes -= entry.bytes;
            }
        }
    }
}

impl Engine {
    /// Replaces the ticks wanted in the background, most urgent first. `playhead` is
    /// the tick on screen. Ticks already prepared or in progress are skipped.
    pub fn request(&self, playhead: u32, ticks: &[u32]) {
        {
            let mut state = self.inner.state.lock();
            let total = state.current.as_ref().map_or(1, |g| g.playable_ticks());
            let frames = &mut state.frames;
            frames.playhead = playhead;
            frames.total = total;
            frames.wanted.clear();
            frames.wanted.extend(ticks.iter().copied().filter(|&tick| tick < total));
        }
        self.inner.state.notify_all();
    }

    /// The prepared frame of `tick`, if it is ready.
    pub fn frame(&self, tick: u32) -> Option<Arc<FrameData>> {
        self.inner.state.lock().frames.entries.get(&tick).map(|entry| entry.data.clone())
    }

    /// The frame of `tick`, prepared on the calling thread when no worker has it.
    /// For a still frame (a seek while paused), where waiting a frame would show.
    pub fn frame_now(&self, tick: u32) -> Option<Arc<FrameData>> {
        if let Some(ready) = self.frame(tick) {
            return Some(ready);
        }
        let generation = self.generation()?;
        if tick >= generation.playable_ticks() {
            return None;
        }
        let data = produce(&generation, tick, self.inner.clock);
        let mut state = self.inner.state.lock();
        state.frames.stats.inline += 1;
        state.frames.finish(generation.id, tick, Some(data))
    }

    pub fn frame_stats(&self) -> FrameStats {
        let state = self.inner.state.lock();
        FrameStats { cached: state.frames.entries.len(), cached_bytes: state.frames.bytes, ..state.frames.stats }
    }
}
