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
    pub tick: f64,
    pub list: Arc<DrawList>,
    /// The scene's camera at this tick.
    pub camera: Option<Camera>,
    /// The scene's backgrounds at this tick: both ids and the mix between them (0 to 255).
    pub background: (u32, u32, f64),
    /// Time spent preparing it.
    pub ms: f32,
}

/// One coherent read of the existing prepared-frame owner. Its epoch also
/// identifies the hidden/effect settings and decoded resource bounds.
pub struct FrameSnapshot {
    pub generation: Arc<Generation>,
    pub frame: Arc<FrameData>,
    pub epoch: u64,
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

pub(crate) fn produce(generation: &Generation, tick: f64, hidden_uids: &[u32], effects: bool, clock: Clock) -> FrameData {
    let started = clock();
    let prepared = &generation.prepared;
    let frame = prepared.evaluate(tick);
    let scene = build(&prepared.doc, &frame);
    let hidden = hidden_slots(generation, &frame, hidden_uids);
    // Prepared frames are shown through the scene camera, which moves parallax layers,
    // and the generated effects beside a figure with it (`offsetGeneratorBatch`).
    let translations = frame.parallax.as_ref().map(|p| p.translations(&[])).unwrap_or_default();
    // Generated effects, with their targets' earlier positions from the generation's cache.
    let mut batch = generation.anchors.as_ref().filter(|_| effects).and_then(|anchors| {
        let sampler = anchors.sampler(clock, f64::INFINITY);
        styk_eval::vfx::generated::build(prepared, &frame, &scene, Some(tick), Some(&sampler), 1.0)
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

/// A finite, nonnegative sample in the existing frame cache. Integer and fractional
/// callers share these identities; positive and negative zero have one identity.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub(crate) struct Sample(u64);

impl Sample {
    fn new(time: f64) -> Option<Sample> {
        (time.is_finite() && time >= 0.0).then(|| Sample(if time == 0.0 { 0 } else { time.to_bits() }))
    }
    pub(crate) fn time(self) -> f64 { f64::from_bits(self.0) }
    fn ahead(self, playhead: Sample, total: u32) -> f64 {
        (self.time() - playhead.time().rem_euclid(total.max(1) as f64)).rem_euclid(total.max(1) as f64)
    }
}
impl From<u32> for Sample {
    fn from(tick: u32) -> Self { Sample((tick as f64).to_bits()) }
}

struct Entry {
    data: Arc<FrameData>,
    bytes: usize,
}

pub(crate) struct Cache {
    generation: u64,
    /// Resource and session changes also supersede work within one generation.
    pub(crate) epoch: u64,
    entries: HashMap<Sample, Entry>,
    bytes: usize,
    budget: usize,
    /// Ticks to prepare, most urgent first.
    wanted: Vec<Sample>,
    inflight: HashSet<Sample>,
    /// The tick playback is at: the cache evicts what is farthest ahead of it.
    playhead: Sample,
    /// Figures hidden for this session, by identity. Prepared frames leave them out.
    pub(crate) hidden: Arc<[u32]>,
    /// Effects are drawn (the editor's effects preview is on).
    pub(crate) effects: bool,
    total: u32,
    stats: FrameStats,
}

impl Cache {
    pub fn new(budget: usize) -> Cache {
        Cache { generation: 0, epoch: 0, entries: HashMap::new(), bytes: 0, budget, wanted: Vec::new(), inflight: HashSet::new(), playhead: Sample::default(), hidden: Arc::from(Vec::new()), effects: true, total: 1, stats: FrameStats::default() }
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

    pub(crate) fn has_storage(&self) -> bool { self.entries.capacity() != 0 || self.wanted.capacity() != 0 || self.inflight.capacity() != 0 || !self.hidden.is_empty() }

    /// Install a worker-prebuilt empty owner without dropping the old cache.
    /// Live session settings may change during preparation; preserve them here.
    pub(crate) fn exchange_publication(&mut self, mut fresh: Cache, generation: u64) -> Cache {
        fresh.generation = generation;
        fresh.epoch = self.epoch.wrapping_add(1);
        fresh.budget = self.budget;
        fresh.playhead = self.playhead;
        fresh.total = self.total;
        fresh.effects = self.effects;
        std::mem::swap(&mut fresh.hidden, &mut self.hidden);
        std::mem::replace(self, fresh)
    }

    /// A decoded cel changed the rectangles effects read. Rebuild previously
    /// requested work, and reject any worker still producing the earlier grid.
    pub(crate) fn invalidate_resources(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        let mut redo: Vec<Sample> = self.entries.keys().chain(self.inflight.iter()).copied().collect();
        let total = self.total.max(1);
        redo.sort_unstable_by(|a, b| a.ahead(self.playhead, total).total_cmp(&b.ahead(self.playhead, total)));
        let mut seen: HashSet<Sample> = self.wanted.iter().copied().collect();
        self.wanted.extend(redo.into_iter().filter(|tick| seen.insert(*tick)));
        self.entries.clear();
        self.bytes = 0;
        self.inflight.clear();
    }

    /// The next tick a worker should prepare, marked as in progress.
    pub fn take_wanted(&mut self) -> Option<Sample> {
        let at = self.wanted.iter().position(|tick| !self.entries.contains_key(tick) && !self.inflight.contains(tick))?;
        let tick = self.wanted[at];
        self.wanted.drain(..=at);
        self.inflight.insert(tick);
        Some(tick)
    }

    /// Stores a finished frame, unless the document changed while it was prepared.
    pub fn finish(&mut self, generation: u64, epoch: u64, tick: Sample, data: Option<FrameData>) -> Option<Arc<FrameData>> {
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
        let mut ticks: Vec<Sample> = self.entries.keys().copied().collect();
        ticks.sort_by(|a, b| b.ahead(playhead, total).total_cmp(&a.ahead(playhead, total)));
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
    /// Replaces the integer ticks wanted in the background, most urgent first.
    /// Integer and fractional requests use the same cache and worker queue.
    pub fn request(&self, playhead: u32, ticks: &[u32]) {
        self.request_keys(Sample::from(playhead), ticks.iter().copied().map(Sample::from).collect());
    }

    /// Replaces the finite, nonnegative samples wanted in the background. Invalid
    /// samples are omitted; an invalid playhead leaves the current request intact.
    pub fn request_samples(&self, playhead: f64, ticks: &[f64]) {
        let Some(playhead) = Sample::new(playhead) else { return };
        self.request_keys(playhead, ticks.iter().copied().filter_map(Sample::new).collect());
    }

    fn request_keys(&self, playhead: Sample, mut wanted: Vec<Sample>) {
        let old = {
            let mut state = self.inner.state.lock();
            let total = state.current.as_ref().map_or(1, |g| g.playable_ticks());
            wanted.retain(|tick| tick.time() < total as f64);
            let frames = &mut state.frames;
            frames.playhead = playhead;
            frames.total = total;
            std::mem::replace(&mut frames.wanted, wanted)
        };
        drop(old);
        self.inner.state.notify_all();
    }

    /// The prepared integer frame, if ready, from the shared sample cache.
    pub fn frame(&self, tick: u32) -> Option<Arc<FrameData>> {
        self.frame_at(tick as f64)
    }

    /// The prepared fractional frame, if ready. Never evaluates or prepares work.
    pub fn frame_at(&self, tick: f64) -> Option<Arc<FrameData>> {
        let tick = Sample::new(tick)?;
        self.inner.state.lock().frames.entries.get(&tick).map(|entry| entry.data.clone())
    }

    /// Reads a ready frame and its generation/cache epoch together. An expected
    /// identity refuses superseded tickets before cloning either shared pointer;
    /// this never evaluates, prepares or repairs a cold sample.
    pub fn prepared_frame(&self, time: f64, expected: Option<(u64, u64)>) -> Option<FrameSnapshot> {
        self.read_prepared(time, expected, |generation, frame, epoch| FrameSnapshot {
            generation: generation.clone(), frame: frame.clone(), epoch,
        })
    }

    /// Readiness of one exact ticket, without taking ownership of a draw list.
    pub fn frame_matches(&self, time: f64, generation: u64, epoch: u64) -> bool {
        self.read_prepared(time, Some((generation, epoch)), |_, _, _| ()).is_some()
    }

    fn read_prepared<R>(&self, time: f64, expected: Option<(u64, u64)>, read: impl FnOnce(&Arc<Generation>, &Arc<FrameData>, u64) -> R) -> Option<R> {
        let tick = Sample::new(time)?;
        let state = self.inner.state.lock();
        let generation = state.current.as_ref()?;
        let frames = &state.frames;
        if frames.generation != generation.id || expected.is_some_and(|identity| identity != (generation.id, frames.epoch)) { return None; }
        let frame = &frames.entries.get(&tick)?.data;
        if frame.generation != generation.id { return None; }
        Some(read(generation, frame, frames.epoch))
    }

    /// A captured prepared monitor may repair an evicted sample on its worker,
    /// but must never substitute newer hidden/effect/resource settings.
    pub fn frame_epoch_current(&self, generation: u64, epoch: u64) -> bool {
        let state = self.inner.state.lock();
        state.current.as_ref().is_some_and(|current| current.id == generation)
            && state.frames.generation == generation && state.frames.epoch == epoch
    }

    /// Prepares an integer frame on the calling thread when not already ready.
    /// Worker/CLI use; browser presentation should read readiness instead.
    pub fn frame_now(&self, tick: u32) -> Option<Arc<FrameData>> {
        self.frame_now_at(tick as f64)
    }

    /// Prepares a finite fractional frame on the calling worker or CLI thread.
    pub fn frame_now_at(&self, time: f64) -> Option<Arc<FrameData>> {
        self.prepared_frame_now_at(time).map(|snapshot| snapshot.frame)
    }

    /// Worker/CLI preparation returns the actual produced frame even if its
    /// cache budget immediately evicts it. Nonblocking presentation uses
    /// `prepared_frame` instead and never enters this preparation path.
    pub fn prepared_frame_now_at(&self, time: f64) -> Option<FrameSnapshot> {
        let tick = Sample::new(time)?;
        let time = tick.time();
        if let Some(ready) = self.prepared_frame(time, None) {
            return Some(ready);
        }
        let (generation, hidden, effects, epoch) = {
            let state = self.inner.state.lock();
            (state.current.clone()?, state.frames.hidden.clone(), state.frames.effects, state.frames.epoch)
        };
        if time >= generation.playable_ticks() as f64 {
            return None;
        }
        let data = produce(&generation, time, &hidden, effects, self.inner.clock);
        let mut state = self.inner.state.lock();
        if !Arc::ptr_eq(&state.frames.hidden, &hidden) || state.frames.effects != effects {
            return None;
        }
        let ready = state.frames.finish(generation.id, epoch, tick, Some(data));
        if ready.is_some() {
            state.frames.stats.inline += 1;
        }
        ready.map(|frame| FrameSnapshot { generation, frame, epoch })
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
        FrameData { generation, tick: tick as f64, list: Arc::new(DrawList::default()), camera: None, background: (0, 0, 0.0), ms: 1.0 }
    }

    #[test]
    fn resource_changes_requeue_cached_inflight_and_requested_frames() {
        let mut cache = Cache::new(1024 * 1024);
        cache.reset(7);
        cache.total = 10;
        cache.playhead = 2.into();
        let epoch = cache.epoch;
        cache.finish(7, epoch, 2.into(), Some(frame(7, 2))).unwrap();
        cache.wanted = vec![3.into(), 4.into()];
        assert_eq!(cache.take_wanted(), Some(3.into()));
        cache.invalidate_resources();
        assert!(cache.entries.is_empty());
        assert_eq!(cache.bytes, 0);
        let mut rebuilt = Vec::new();
        while let Some(tick) = cache.take_wanted() { rebuilt.push(tick.time()); }
        rebuilt.sort_unstable_by(f64::total_cmp);
        assert_eq!(rebuilt, [2.0, 3.0, 4.0]);
    }

    #[test]
    fn an_old_worker_cannot_finish_or_remove_the_newer_worker_for_the_same_tick() {
        let mut cache = Cache::new(1024 * 1024);
        cache.reset(7);
        cache.wanted = vec![0.into()];
        let old = cache.epoch;
        assert_eq!(cache.take_wanted(), Some(0.into()));
        cache.invalidate_resources();
        let current = cache.epoch;
        assert_eq!(cache.take_wanted(), Some(0.into()));
        assert!(cache.finish(7, old, 0.into(), Some(frame(7, 0))).is_none());
        assert!(cache.inflight.contains(&0.into()), "the new job still owns its tick");
        assert!(cache.finish(7, current, 0.into(), Some(frame(7, 0))).is_some());
        cache.reset(8);
        assert!(cache.finish(7, current, 0.into(), Some(frame(7, 0))).is_none(), "replacement drops old work");
        assert!(cache.entries.is_empty());
    }

    fn sample_frame(generation: u64, tick: Sample) -> FrameData {
        FrameData { generation, tick: tick.time(), list: Arc::new(DrawList::default()), camera: None, background: (0, 0, 0.0), ms: 1.0 }
    }

    #[test]
    fn fractional_resource_invalidation_requeues_exact_samples_and_rejects_old_workers() {
        let mut cache = Cache::new(1024 * 1024);
        cache.reset(7);
        cache.total = 10;
        let half = Sample::new(3.5).unwrap();
        let other = Sample::new(4.25).unwrap();
        cache.playhead = half;
        let old_epoch = cache.epoch;
        cache.finish(7, old_epoch, half, Some(sample_frame(7, half))).unwrap();
        cache.wanted = vec![other];
        assert_eq!(cache.take_wanted(), Some(other));
        cache.invalidate_resources();
        assert_eq!(cache.take_wanted(), Some(half));
        assert_eq!(cache.take_wanted(), Some(other));
        assert!(cache.finish(7, old_epoch, other, Some(sample_frame(7, other))).is_none());
        assert!(cache.inflight.contains(&other), "old worker cannot release newer sample owner");
        assert!(cache.finish(7, cache.epoch, other, Some(sample_frame(7, other))).is_some());
        cache.reset(8);
        assert!(cache.finish(7, old_epoch, half, Some(sample_frame(7, half))).is_none());
    }

    #[test]
    fn fractional_eviction_preserves_exact_playhead_and_existing_byte_budget() {
        let mut cache = Cache::new(700);
        cache.reset(7);
        cache.total = 10;
        let playhead = Sample::new(3.5).unwrap();
        cache.playhead = playhead;
        let epoch = cache.epoch;
        for time in [3.5, 3.0, 4.25, 8.75, 0.5] {
            let sample = Sample::new(time).unwrap();
            cache.finish(7, epoch, sample, Some(sample_frame(7, sample))).unwrap();
        }
        assert!(cache.bytes <= cache.budget);
        assert!(cache.entries.len() < 5);
        assert!(cache.entries.contains_key(&playhead));
        assert_eq!(cache.entries[&playhead].data.tick, 3.5);
    }

}
