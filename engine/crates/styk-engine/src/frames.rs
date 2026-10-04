//! Frame preparation and its cache.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use styk_eval::doc::Camera;
use styk_eval::drawlist::{DrawList, Overrides, encode_with};
use styk_eval::geometry::build;

use crate::{Clock, Engine, Generation};

/// A prepared frame: everything the renderer needs for one tick.
pub struct FrameData {
    pub generation: u64,
    pub tick: u32,
    pub list: Arc<DrawList>,
    /// The scene's camera at this tick.
    pub camera: Option<Camera>,
    /// The scene's backgrounds at this tick: both ids and the mix between them (0 to 255).
    pub background: (u32, u32, f64),
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

/// Figures the session hides: by identity, and by the layers the document hides.
pub(crate) fn hidden_slots(generation: &Generation, frame: &styk_eval::eval::Frame, hidden_uids: &[u32]) -> Vec<bool> {
    let layers = generation.prepared.doc.layers.as_ref().filter(|layers| !layers.hidden.is_empty());
    if hidden_uids.is_empty() && layers.is_none() {
        return Vec::new();
    }
    frame
        .objects
        .iter()
        .map(|object| hidden_uids.contains(&object.uid) || layers.is_some_and(|layers| layers.hidden.iter().any(|id| id == layers.layer_of(object.uid))))
        .collect()
}

pub(crate) fn produce(generation: &Generation, tick: u32, hidden_uids: &[u32], effects: bool, clock: Clock) -> FrameData {
    let started = clock();
    let prepared = &generation.prepared;
    let frame = prepared.evaluate(tick as f64);
    let scene = build(&prepared.doc, &frame);
    let hidden = hidden_slots(generation, &frame, hidden_uids);
    // Prepared frames are shown through the scene camera, which moves parallax layers,
    // and the generated effects beside a figure with it (`offsetGeneratorBatch`).
    let translations = frame.parallax.as_ref().map(|p| p.translations(&[])).unwrap_or_default();
    // Generated effects, with their targets' earlier positions from the generation's cache.
    let mut batch = generation.anchors.as_ref().filter(|_| effects).and_then(|anchors| {
        let sampler = anchors.sampler(clock, f64::INFINITY);
        styk_eval::vfx::generated::build(prepared, &frame, &scene, Some(tick as f64), Some(&sampler), 1.0)
    });
    if let (Some(batch), Some(parallax), Some(layers)) = (batch.as_mut(), frame.parallax.as_ref(), prepared.doc.layers.as_deref()) {
        batch.offset(|uid| Some(parallax.of_uid(layers, uid)));
    }
    let overrides = Overrides { hidden: &hidden, translations: &translations, bypass_effects: !effects, generators: batch.as_ref(), ..Default::default() };
    let list = Arc::new(encode_with(&prepared.doc, &generation.assets, &scene, &overrides));
    FrameData {
        generation: generation.id,
        tick,
        list,
        camera: scene.camera,
        background: scene.background,
        ms: (clock() - started) as f32,
    }
}

struct Entry {
    data: Arc<FrameData>,
    bytes: usize,
}

pub(crate) struct Cache {
    generation: u64,
    /// Resource and session changes also supersede work within one generation.
    pub(crate) epoch: u64,
    entries: HashMap<u32, Entry>,
    bytes: usize,
    budget: usize,
    /// Ticks to prepare, most urgent first.
    wanted: Vec<u32>,
    inflight: HashSet<u32>,
    /// The tick playback is at: the cache evicts what is farthest ahead of it.
    playhead: u32,
    /// Figures hidden for this session, by identity. Prepared frames leave them out.
    pub(crate) hidden: Arc<[u32]>,
    /// Effects are drawn (the editor's effects preview is on).
    pub(crate) effects: bool,
    total: u32,
    stats: FrameStats,
}

impl Cache {
    pub fn new(budget: usize) -> Cache {
        Cache { generation: 0, epoch: 0, entries: HashMap::new(), bytes: 0, budget, wanted: Vec::new(), inflight: HashSet::new(), playhead: 0, hidden: Arc::from(Vec::new()), effects: true, total: 1, stats: FrameStats::default() }
    }

    pub fn reset(&mut self, generation: u64) {
        self.generation = generation;
        self.epoch = self.epoch.wrapping_add(1);
        self.entries.clear();
        self.bytes = 0;
        self.wanted.clear();
        self.inflight.clear();
        self.stats = FrameStats::default();
    }

    /// A decoded cel changed the rectangles effects read. Rebuild previously
    /// requested work, and reject any worker still producing the earlier grid.
    pub(crate) fn invalidate_resources(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        let mut redo: Vec<u32> = self.entries.keys().chain(self.inflight.iter()).copied().collect();
        let total = self.total.max(1);
        redo.sort_unstable_by_key(|tick| (*tick + total - self.playhead % total) % total);
        let mut seen: HashSet<u32> = self.wanted.iter().copied().collect();
        self.wanted.extend(redo.into_iter().filter(|tick| seen.insert(*tick)));
        self.entries.clear();
        self.bytes = 0;
        self.inflight.clear();
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
    pub fn finish(&mut self, generation: u64, epoch: u64, tick: u32, data: Option<FrameData>) -> Option<Arc<FrameData>> {
        if generation != self.generation || epoch != self.epoch {
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
        let (generation, hidden, effects, epoch) = {
            let state = self.inner.state.lock();
            (state.current.clone()?, state.frames.hidden.clone(), state.frames.effects, state.frames.epoch)
        };
        if tick >= generation.playable_ticks() {
            return None;
        }
        let data = produce(&generation, tick, &hidden, effects, self.inner.clock);
        let mut state = self.inner.state.lock();
        if !Arc::ptr_eq(&state.frames.hidden, &hidden) || state.frames.effects != effects {
            return None;
        }
        let ready = state.frames.finish(generation.id, epoch, tick, Some(data));
        if ready.is_some() {
            state.frames.stats.inline += 1;
        }
        ready
    }

    /// Sets the figures hidden for this session (by identity). Prepared frames that
    /// showed them are dropped; hidden layers come from the document itself.
    pub fn set_hidden(&self, uids: &[u32]) {
        let mut state = self.inner.state.lock();
        if &*state.frames.hidden == uids {
            return;
        }
        state.frames.hidden = Arc::from(uids);
        let generation = state.frames.generation;
        state.frames.reset(generation);
    }

    /// Prepares frames with or without effects. Prepared frames of the other kind are dropped.
    pub fn set_effects(&self, on: bool) {
        let mut state = self.inner.state.lock();
        if state.frames.effects == on {
            return;
        }
        state.frames.effects = on;
        let generation = state.frames.generation;
        state.frames.reset(generation);
    }

    pub fn frame_stats(&self) -> FrameStats {
        let state = self.inner.state.lock();
        FrameStats { cached: state.frames.entries.len(), cached_bytes: state.frames.bytes, ..state.frames.stats }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(generation: u64, tick: u32) -> FrameData {
        FrameData { generation, tick, list: Arc::new(DrawList::default()), camera: None, background: (0, 0, 0.0), ms: 1.0 }
    }

    #[test]
    fn resource_changes_requeue_cached_inflight_and_requested_frames() {
        let mut cache = Cache::new(1024 * 1024);
        cache.reset(7);
        cache.total = 10;
        cache.playhead = 2;
        let epoch = cache.epoch;
        cache.finish(7, epoch, 2, Some(frame(7, 2))).unwrap();
        cache.wanted = vec![3, 4];
        assert_eq!(cache.take_wanted(), Some(3));
        cache.invalidate_resources();
        assert!(cache.entries.is_empty());
        assert_eq!(cache.bytes, 0);
        let mut rebuilt = Vec::new();
        while let Some(tick) = cache.take_wanted() { rebuilt.push(tick); }
        rebuilt.sort_unstable();
        assert_eq!(rebuilt, [2, 3, 4]);
    }

    #[test]
    fn an_old_worker_cannot_finish_or_remove_the_newer_worker_for_the_same_tick() {
        let mut cache = Cache::new(1024 * 1024);
        cache.reset(7);
        cache.wanted = vec![0];
        let old = cache.epoch;
        assert_eq!(cache.take_wanted(), Some(0));
        cache.invalidate_resources();
        let current = cache.epoch;
        assert_eq!(cache.take_wanted(), Some(0));
        assert!(cache.finish(7, old, 0, Some(frame(7, 0))).is_none());
        assert!(cache.inflight.contains(&0), "the new job still owns its tick");
        assert!(cache.finish(7, current, 0, Some(frame(7, 0))).is_some());
        cache.reset(8);
        assert!(cache.finish(7, current, 0, Some(frame(7, 0))).is_none(), "replacement drops old work");
        assert!(cache.entries.is_empty());
    }
}
