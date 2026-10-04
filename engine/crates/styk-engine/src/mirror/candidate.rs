//! Worker-scoped preparation in the sole Mirror/Loader/publication owner.
//! The existing mirror guard and in-flight batch marker stay here until consume
//! or worker disposal. This value must not span host ACKs or scheduler turns.
use super::*;
use std::sync::MutexGuard;

struct Stored {
    tree: Value,
    publication: crate::publication::Prepared,
    converted: Converted,
    previous_lineage: u64,
    previous_assets: Option<Arc<Assets>>,
}
enum Content {
    Stored(Stored),
    Working(Arc<Generation>),
}

/// Opaque preparation borrowed/consumed entirely in one worker scope. Its exact
/// Generation includes the compatible typed document/Assets, before encoding.
/// It deliberately owns the existing MutexGuard and is not a Send/ACK payload.
#[must_use = "consume or dispose this candidate on its owning worker"]
pub struct WorkerPublication<'a> {
    engine: &'a Engine,
    mirror: MutexGuard<'a, Mirror>,
    staged: transfer::Batch,
    marker: Option<CommitMarker<'a>>,
    content: Option<Content>,
    started: f64,
    revision: u64,
    committed: bool,
}
impl WorkerPublication<'_> {
    /// Borrow on the worker; no independently rebuilt Generation/Assets.
    pub fn generation(&self) -> &Generation {
        match self.content.as_ref().expect("unconsumed publication") {
            Content::Stored(stored) => stored.publication.generation(),
            Content::Working(generation) => generation,
        }
    }
    pub fn stores_generation(&self) -> bool { matches!(self.content, Some(Content::Stored(_))) }

    pub(super) fn commit<F, R>(mut self, exchange: &mut Option<F>)
        -> Result<(Committed, crate::PublicationRetirement, R), (MirrorError, Self)>
    where F: FnOnce() -> R {
        let result = match self.content.take().expect("consume once") {
            Content::Working(generation) => {
                let guard = self.staged.guard.expect("working publication is guarded");
                match self.engine.exchange_session_working_worker(guard, &self.staged.cancelled, exchange) {
                    Some(exchanged) => {
                        let mut retirement = crate::PublicationRetirement::empty();
                        retirement.record_charges.append(&mut self.staged.charges);
                        Ok((Committed { generation: generation.id, revision: guard.revision,
                            converted: Default::default(), ms: self.engine.now() - self.started }, retirement, exchanged))
                    }
                    None => {
                        self.content = Some(Content::Working(generation));
                        Err(MirrorError::Transaction("the working Session was superseded before publication"))
                    }
                }
            }
            Content::Stored(mut stored) => {
                let guard = self.staged.guard.map(|guard| (guard, self.staged.cancelled.clone()));
                let generation = stored.publication.generation().clone();
                match self.engine.publish_prepared_compatible_worker(stored.publication, guard.as_ref(), exchange) {
                    Ok((id, mut retirement, exchanged)) => {
                        // The publication's exact resources are now the sole
                        // State generation. Reuse them for the mirror head.
                        let had_canonical_head = self.mirror.prepared.is_some();
                        let previous_root = std::mem::replace(&mut self.mirror.tree, stored.tree);
                        if had_canonical_head { retirement.canonical_root = Some(previous_root); }
                        retirement.record_charges.append(&mut self.staged.charges);
                        self.mirror.revision = self.revision;
                        self.mirror.prepared = Some(generation.prepared.clone());
                        self.mirror.assets = Some(generation.assets.clone());
                        self.mirror.loader.release_unused();
                        Ok((Committed { generation: id, revision: self.revision, converted: stored.converted,
                            ms: generation.open_ms }, retirement, exchanged))
                    }
                    Err(publication) => {
                        stored.publication = publication;
                        self.content = Some(Content::Stored(stored));
                        Err(MirrorError::Transaction("the lifted transfer was superseded before publication"))
                    }
                }
            }
        };
        match result {
            Ok(result) => { self.committed = true; Ok(result) }
            Err(error) => Err((error, self)),
        }
    }

    /// Allocation/drop-free exchange under the existing final State guard. The
    /// caller retains all returned retirements on its worker through host ACK.
    /// Refusal returns this candidate and the uncalled callback for worker drop.
    pub fn commit_with_worker<F, R>(self, exchange: F)
        -> Result<(Committed, crate::PublicationRetirement, R), (MirrorError, Self, F)>
    where F: FnOnce() -> R {
        let mut exchange = Some(exchange);
        self.commit(&mut exchange).map_err(|(error, candidate)|
            (error, candidate, exchange.take().expect("refused before exchange")))
    }
}
impl Drop for WorkerPublication<'_> {
    fn drop(&mut self) {
        if !self.committed {
            if let Some(Content::Stored(stored)) = self.content.as_ref() {
                // Same failure disposition as the original guarded publisher:
                // no canonical head changes; sole Loader scratch is rebuilt.
                self.mirror.loader = Loader::new();
                self.mirror.lineage = stored.previous_lineage;
                self.mirror.assets = stored.previous_assets.clone();
            }
        }
        // The existing CommitMarker and remaining payloads retire on this
        // worker, after State has released, exactly as ordinary commit did.
        drop(self.marker.take());
    }
}

impl Engine {
    /// Prepare a completed guarded record batch in the existing sole Mirror
    /// owner. Worker-scoped only: borrow generation/assets, then consume/drop.
    pub fn prepare_record_publication_worker(&self, revision: u64, batch: u32, guard: RecordGuard)
        -> Result<WorkerPublication<'_>, MirrorError> {
        let started = self.now();
        let mirror = self.inner.mirror.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        self.prepare_locked(mirror, started, revision, batch, Some(guard), None)
    }

    pub(super) fn prepare_locked<'a>(&'a self, mut mirror: MutexGuard<'a, Mirror>, started: f64,
        revision: u64, batch: u32, guard: Option<RecordGuard>,
        expected_root: Option<(&Value, &Value, &Arc<transfer::Charge>, bool)>)
        -> Result<WorkerPublication<'a>, MirrorError> {
        let mut empty = Some(transfer::Batch::default());
        let mut staged = {
            let mut staged = self.inner.staged.lock();
            if guard.is_none() && staged.get(&batch).is_some_and(|owner| owner.guard.is_some()) {
                return Err(MirrorError::Transaction("a lifted transaction requires its guarded commit"));
            }
            if guard.is_some_and(|guard| staged.get(&batch).and_then(|batch| batch.guard) != Some(guard)) {
                return Err(MirrorError::Transaction("the lifted transaction was discarded or replaced"));
            }
            match (staged.get(&batch).and_then(|batch| batch.root.as_ref()), expected_root) {
                (None, None) => {},
                (Some(root), Some((before, after, charge, stored))) if root.before.same_node(before) && root.after.same_node(after)
                    && Arc::ptr_eq(&root.charge, charge) && root.stored == stored && mirror.tree.same_node(before) => {},
                _ => return Err(MirrorError::Transaction("a prepared root requires its exact guarded Editor publication")),
            }
            if staged.get(&batch).and_then(|batch| batch.transfer.as_ref()).is_some_and(|transfer| !transfer.acknowledged()) {
                return Err(MirrorError::Transaction("the lifted transfer has not completed and been acknowledged"));
            }
            if guard.is_some() {
                let owner = staged.get_mut(&batch).expect("guard checked above");
                if owner.committing { return Err(MirrorError::Transaction("the lifted transaction is already preparing")); }
                owner.committing = true;
                transfer::Batch { changes: std::mem::take(&mut owner.changes), guard: owner.guard,
                    transfer: owner.transfer.clone(), cancelled: owner.cancelled.clone(), committing: true,
                    budget: owner.budget.clone(), charges: std::mem::take(&mut owner.charges), root: owner.root.take() }
            } else { staged.remove(&batch).unwrap_or_else(|| empty.take().expect("prebuilt empty batch")) }
        };
        let marker = staged.guard.map(|_| CommitMarker { engine: self, batch, cancelled: staged.cancelled.clone() });
        if staged.cancelled.load(std::sync::atomic::Ordering::SeqCst) || staged.guard.is_some_and(|guard| !self.record_guard_current(guard)) {
            return Err(MirrorError::Transaction("the lifted transfer belongs to another document generation"));
        }
        let content = if expected_root.is_some_and(|(_, _, _, stored)| !stored) {
            let generation = self.generation().ok_or(MirrorError::Transaction("the working Session has no stored generation"))?;
            let guard = staged.guard.expect("working publication is guarded");
            if generation.id != guard.generation || generation.lineage != guard.lineage || generation.revision != guard.revision {
                return Err(MirrorError::Transaction("the lifted transfer belongs to another document generation"));
            }
            Content::Working(generation)
        } else {
            let (tree, reset) = match staged.root.take() {
                Some(root) => (root.after, false), None => mirror.apply(std::mem::take(&mut staged.changes))?,
            };
            let previous_lineage = mirror.lineage;
            let previous_assets = mirror.assets.clone();
            if reset {
                mirror.loader = Loader::new();mirror.assets = None;
                mirror.lineage = self.inner.next_generation.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            let name = tree.path(&["_styk", "name"]).and_then(Value::as_str).unwrap_or("").to_string();
            let document = mirror.loader.document(&tree, &name).map_err(|e| MirrorError::Document(e.to_string()))?;
            let converted = mirror.loader.converted;
            let assets = Arc::new(match &mirror.assets {
                Some(previous) => previous.extended(&document), None => Assets::build(&document),
            });
            let prepared = Arc::new(Prepared::new(Arc::new(document)));
            let ms = self.now() - started;
            let guard = staged.guard.map(|guard| (guard, staged.cancelled.clone()));
            let generation = Generation { id: 0, lineage: mirror.lineage, revision, prepared, assets, open_ms: ms, anchors: None };
            let publication = match self.prepare_publication_worker(generation, guard.as_ref()) {
                Some(publication) => publication,
                None => {
                    mirror.loader = Loader::new();mirror.lineage = previous_lineage;mirror.assets = previous_assets;
                    return Err(MirrorError::Transaction("the lifted transfer was superseded before publication"));
                }
            };
            Content::Stored(Stored { tree, publication, converted, previous_lineage, previous_assets })
        };
        Ok(WorkerPublication { engine: self, mirror, staged, marker, content: Some(content),
            started, revision, committed: false })
    }
}

#[cfg(test)]
#[path = "candidate_tests.rs"]
mod tests;
