//! Worker-prepared publication and an allocation-free final head exchange.
//!
//! The editor callback only exchanges already-validated heads. Every displaced
//! engine/cache/image/task payload stays owned by the returned worker retirement.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::{Engine, Generation, State, TaskQueue, frames, images};
use crate::mirror::RecordGuard;

/// Opaque worker-owned payload. Keep it through publication ACK; the interface
/// must never own its last strong reference. Dropping it releases old resources.
pub struct PublicationRetirement {
    pub(crate) generation: Option<Arc<Generation>>,
    pub(crate) frames: frames::Cache,
    pub(crate) images: Option<images::Store>,
    pub(crate) tasks: TaskQueue,
    pub(crate) canonical_root: Option<styk_doc::Value>,
    pub(crate) record_charges: std::collections::LinkedList<crate::mirror::transfer::Charge>,
    pub(crate) root_admission: Option<crate::mirror::EditPublicationAdmission>,
}

impl PublicationRetirement {
    pub(crate) fn empty() -> Self {
        Self { generation: None, frames: frames::Cache::new(0), images: None, tasks: TaskQueue::new(), canonical_root: None, record_charges: std::collections::LinkedList::new(), root_admission: None }
    }
    pub(crate) fn needs_worker(&self) -> bool {
        self.generation.is_some() || self.frames.has_storage() || self.images.as_ref().is_some_and(images::Store::has_storage) || !self.tasks.is_empty() || self.canonical_root.is_some() || !self.record_charges.is_empty() || self.root_admission.is_some()
    }
}

pub(crate) struct Prepared {
    previous: Option<Arc<Generation>>,
    generation: Arc<Generation>,
    frames: Option<frames::Cache>,
    images: Option<images::Store>,
    tasks: TaskQueue,
    same_lineage: bool,
    image_delta: usize,
}

impl Prepared {
    pub(crate) fn generation(&self) -> &Arc<Generation> { &self.generation }

    fn new(generation: Arc<Generation>, current: Option<Arc<Generation>>) -> Self {
        let previous = current.as_ref().filter(|previous| previous.lineage == generation.lineage);
        let count = generation.prepared.doc.images.len();
        let known = previous.as_ref().map_or(0, |previous| previous.prepared.doc.images.len().min(count));
        let cels: Vec<usize> = generation.assets.cels.keys().copied().filter(|cel| !previous.as_ref().is_some_and(|old| old.assets.cels.contains_key(cel))).collect();
        let mut tasks = TaskQueue::new();
        for index in known..count {
            let generation = generation.clone();
            tasks.push_back(crate::OrdinaryTask::Run(Box::new(move |engine: &Engine| engine.decode_image(&generation, index))));
        }
        for cel in &cels {
            let generation = generation.clone();
            let cel = *cel;
            tasks.push_back(crate::OrdinaryTask::Run(Box::new(move |engine: &Engine| engine.decode_cel(&generation, cel))));
        }
        if generation.anchors.is_some() {
            for row in 0..styk_eval::vfx::generated::atlas::ROWS {
                if styk_eval::vfx::generated::atlas::ready(row).is_none() {
                    tasks.push_back(crate::OrdinaryTask::Run(Box::new(move |_: &Engine| { styk_eval::vfx::generated::atlas::row(row); })));
                }
            }
        }
        let same_lineage = previous.is_some();
        Self {
            previous: current,
            frames: Some(frames::Cache::new(0)),
            images: Some(images::Store::for_publication(generation.lineage, count + cels.len())),
            same_lineage, image_delta: count - known + cels.len(), generation, tasks,
        }
    }

    fn current(&self, state: &State) -> bool {
        match (&self.previous, &state.current) {
            (None, None) => true,
            (Some(before), Some(current)) => Arc::ptr_eq(before, current),
            _ => false,
        }
    }

    /// Only pointer/scalar operations. All fresh owners/nodes were built above.
    fn install(&mut self, state: &mut State) -> PublicationRetirement {
        let frames = state.frames.exchange_publication(self.frames.take().expect("prepared once"), self.generation.id);
        let images = if self.same_lineage {
            state.images.grow(self.image_delta);
            None
        } else { Some(std::mem::replace(&mut state.images, self.images.take().expect("prepared once"))) };
        let tasks = if self.same_lineage {
            state.tasks.append(&mut self.tasks);
            TaskQueue::new()
        } else { std::mem::replace(&mut state.tasks, std::mem::take(&mut self.tasks)) };
        let generation = state.current.replace(self.generation.clone());
        PublicationRetirement { generation, frames, images, tasks, canonical_root: None, record_charges: std::collections::LinkedList::new(), root_admission: None }
    }
}

fn authorized(state: &State, guard: Option<(&RecordGuard, &AtomicBool)>, stopping: &AtomicBool) -> bool {
    guard.is_none_or(|(guard, cancelled)| guard.current(state)
        && !cancelled.load(Ordering::SeqCst) && !stopping.load(Ordering::SeqCst))
}

impl Engine {
    /// Working-only Session publication uses the same final current/cancel
    /// predicate without preparing or replacing the stored Engine generation.
    /// The supplied callback only swaps prebuilt editing/ready heads.
    pub(crate) fn exchange_session_working_worker<F, R>(
        &self, guard: RecordGuard, cancelled: &AtomicBool, exchange: &mut Option<F>,
    ) -> Option<R> where F: FnOnce() -> R {
        #[cfg(test)]
        crate::allocation::before_lock(cancelled as *const AtomicBool as usize);
        let state = self.inner.state.lock();
        if !authorized(&state, Some((&guard, cancelled)), &self.inner.stopping) { return None; }
        Some(exchange.take().expect("one guarded working head exchange")())
    }

    /// The SAME publication owner prepares its exact generation/resources
    /// before a worker encoder borrows them. Nothing is visible under State yet.
    pub(crate) fn prepare_publication_worker(
        &self, mut generation: Generation, guard: Option<&(RecordGuard, Arc<AtomicBool>)>,
    ) -> Option<Prepared> {
        let id = self.inner.next_generation.fetch_add(1, Ordering::Relaxed);
        generation.id = id;
        if styk_eval::vfx::generated::has_generators(&generation.prepared.doc) {
            generation.anchors = Some(Arc::new(crate::anchors::Anchors::new()));
        }
        self.prepare_generation_worker(Arc::new(generation), guard)
    }

    fn prepare_generation_worker(
        &self, generation: Arc<Generation>, guard: Option<&(RecordGuard, Arc<AtomicBool>)>,
    ) -> Option<Prepared> {
        let current = {
            let state = self.inner.state.lock();
            if !authorized(&state, guard.map(|(guard, cancelled)| (guard, cancelled.as_ref())), &self.inner.stopping) { return None; }
            state.current.clone()
        };
        Some(Prepared::new(generation, current))
    }

    /// Consume the exact prebuilt publication once. Failure returns all fresh
    /// payloads, and the uncalled callback remains with the owning worker.
    pub(crate) fn publish_prepared_worker<F, R>(
        &self, mut prepared: Prepared, guard: Option<&(RecordGuard, Arc<AtomicBool>)>, exchange: &mut Option<F>,
    ) -> Result<(u64, PublicationRetirement, R), Prepared>
    where F: FnOnce() -> R {
        let result = {
            let mut state = self.inner.state.lock();
            if !prepared.current(&state) || !authorized(&state, guard.map(|(guard, cancelled)| (guard, cancelled.as_ref())), &self.inner.stopping) {
                None
            } else {
                let exchanged = exchange.take().expect("the callback runs only once")();
                Some((prepared.install(&mut state), exchanged))
            }
        };
        if let Some((retirement, exchanged)) = result {
            if let Some(previous) = &retirement.generation {
                crate::planning::invalidate_generation(self, previous.id);
                self.retire_record_batches(previous.id);
            }
            self.inner.state.notify_all();
            Ok((prepared.generation.id, retirement, exchanged))
        } else { Err(prepared) }
    }

    /// Compatibility callers prepare and consume through that SAME owner.
    /// Only an unguarded open may retry against a newly published lineage.
    pub(crate) fn publish_with<F, R>(&self, generation: Generation, guard: Option<(RecordGuard, Arc<AtomicBool>)>, exchange: &mut Option<F>) -> Option<(u64, PublicationRetirement, R)>
    where F: FnOnce() -> R {
        let prepared = self.prepare_publication_worker(generation, guard.as_ref())?;
        self.publish_prepared_compatible_worker(prepared, guard.as_ref(), exchange).ok()
    }

    /// Existing unguarded-open retry behavior shares the same prebuilt owner;
    /// guarded candidates always return unchanged on final refusal.
    pub(crate) fn publish_prepared_compatible_worker<F, R>(
        &self, mut prepared: Prepared, guard: Option<&(RecordGuard, Arc<AtomicBool>)>, exchange: &mut Option<F>,
    ) -> Result<(u64, PublicationRetirement, R), Prepared>
    where F: FnOnce() -> R {
        loop {
            match self.publish_prepared_worker(prepared, guard, exchange) {
                Ok(result) => return Ok(result),
                Err(previous) => {
                    if guard.is_some() { return Err(previous); }
                    let generation = previous.generation.clone();
                    drop(previous);
                    prepared = self.prepare_generation_worker(generation, None).expect("unguarded preparation");
                }
            }
        }
    }

}

#[cfg(test)]
mod tests;
