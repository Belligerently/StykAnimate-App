//! The document as the interface last described it.
//!
//! While editing commands still run in JavaScript, the engine's document is a
//! mirror: the interface stages what changed, record by record, and commits. A
//! commit builds the next tree by copy-on-write, converts only the records that
//! changed into typed views, and publishes a new generation. Nothing is visible
//! to evaluation until the commit succeeds as a whole.

use std::sync::Arc;

use styk_doc::{Obj, Value};
use styk_eval::assets::Assets;
use styk_eval::eval::Prepared;
use styk_eval::load::{Converted, Loader};
use styk_formats::container::parse_lifted;

use crate::{Engine, Generation};

pub(crate) mod transfer;
mod root;
mod candidate;
pub use candidate::WorkerPublication;
pub use root::{EditPublicationAdmission, WorkerEditPublication};
pub use transfer::{Packet, PacketKind, RecordAck, RecordGuard, RecordLengths, TransferError, MAX_RECORD_PACKET_BYTES, MAX_RECORD_RETAINED_BYTES, MAX_RECORD_CHANGES};

/// Dense metadata transported independently from the document's small properties.
/// Codes are stable for browser and native hosts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetadataField {
    DraftBaseline,
    DraftAnimationBaseline,
    LayerSheet,
    Layers,
    Animation,
}

impl MetadataField {
    pub fn from_code(code: u32) -> Option<Self> {
        match code {
            0 => Some(Self::DraftBaseline),
            1 => Some(Self::DraftAnimationBaseline),
            2 => Some(Self::LayerSheet),
            3 => Some(Self::Layers),
            4 => Some(Self::Animation),
            _ => None,
        }
    }

    fn path(self) -> &'static [&'static str] {
        match self {
            Self::DraftBaseline => &["_styk", "simpleFrameDraft", "baseline"],
            Self::DraftAnimationBaseline => &["_styk", "simpleFrameDraft", "animationBaseline"],
            Self::LayerSheet => &["_styk", "layerSheet"],
            Self::Layers => &["_styk", "layers"],
            Self::Animation => &["_styk", "animation"],
        }
    }
}

/// The canonical location of one already-validated lifted value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordTarget {
    Table(Table, u32),
    Source(String),
    Metadata(MetadataField),
}

/// Source removal may need to restore the absence of the source table itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceDisposition {
    Preserve,
    AbsentIfEmpty,
}

/// The tables of a document that are synchronised record by record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Table {
    /// `figure_types`
    Figures,
    /// `keyframes`
    Keyframes,
    /// `backgrounds`
    Backgrounds,
}

impl Table {
    pub fn key(self) -> &'static str {
        match self {
            Table::Figures => "figure_types",
            Table::Keyframes => "keyframes",
            Table::Backgrounds => "backgrounds",
        }
    }

    pub fn from_code(code: u32) -> Option<Table> {
        match code {
            0 => Some(Table::Figures),
            1 => Some(Table::Keyframes),
            2 => Some(Table::Backgrounds),
            _ => None,
        }
    }
}

/// A record as it crosses the boundary: JSON with markers into two byte sections
/// (the layout `styk_formats::container::lift` writes), so number arrays and
/// images travel as bytes.
pub struct Record {
    pub skeleton: Vec<u8>,
    pub arrays: Vec<u8>,
    pub payloads: Vec<u8>,
}

/// One staged change.
pub enum Change {
    /// Forget the mirrored document: what follows describes another one.
    Reset,
    /// The top-level properties (everything but the tables and `_styk.frameSources`).
    Properties(Vec<u8>),
    /// The new order of a table: each entry is the index a record had before, or
    /// -1 for a record that a following [`Change::Set`] supplies.
    Layout(Table, Vec<i32>),
    Set(Table, u32, Record),
    /// `_styk.frameSources[id]`: a retained pose source, or its removal.
    Source(String, Option<Record>),
    /// The shared worker receiver parsed/materialized this value once. `None`
    /// removes a source/metadata field; table records must supply a value.
    Resolved(RecordTarget, Option<Value>),
    /// Applied after all source changes, so no nonempty source table is lost.
    Sources(SourceDisposition),
}

/// Why a commit was refused. The mirror is unchanged.
#[derive(Debug, thiserror::Error)]
pub enum MirrorError {
    #[error("a staged record is not valid: {0}")]
    Record(String),
    #[error("a staged record is not valid: {0}")]
    Transaction(&'static str),
    #[error("a staged change names record {index} of `{table}`, which has {len}")]
    Index { table: &'static str, index: usize, len: usize },
    #[error("record {index} of `{table}` was announced by the layout but never supplied")]
    Missing { table: &'static str, index: usize },
    #[error("the document cannot be evaluated: {0}")]
    Document(String),
}

/// What a commit did.
#[derive(Clone, Copy, Debug)]
pub struct Committed {
    pub generation: u64,
    pub revision: u64,
    /// Records converted into typed views; everything else was reused.
    pub converted: Converted,
    pub ms: f64,
}

pub(crate) struct Mirror {
    tree: Value,
    /// The interface revision of the last commit.
    revision: u64,
    loader: Loader,
    lineage: u64,
    assets: Option<Arc<Assets>>,
    /// The typed views of `tree`, as published with its generation.
    prepared: Option<Arc<Prepared>>,
}

impl Default for Mirror {
    fn default() -> Self {
        Mirror { tree: Value::object(Obj::new()), revision: 0, loader: Loader::new(), lineage: 0, assets: None, prepared: None }
    }
}

fn record(record: &Record) -> Result<Value, MirrorError> {
    parse_lifted(&record.skeleton, &record.arrays, &record.payloads).map_err(|e| MirrorError::Record(e.to_string()))
}

fn table_items(tree: &Value, table: Table) -> Vec<Value> {
    tree.get(table.key()).and_then(Value::as_arr).map(<[Value]>::to_vec).unwrap_or_default()
}

impl Mirror {
    /// Applies `changes` to a copy of the tree. The copy shares every record the changes do not name.
    fn apply(&self, changes: std::collections::LinkedList<Change>) -> Result<(Value, bool), MirrorError> {
        let mut tree = self.tree.clone();
        let mut reset = false;
        // Placeholders left by a layout until their record arrives.
        let mut pending: Vec<(Table, usize)> = Vec::new();
        let mut sources_disposition = SourceDisposition::Preserve;
        for change in changes {
            match change {
                Change::Reset => {
                    tree = Value::object(Obj::new());
                    reset = true;
                    pending.clear();
                }
                Change::Properties(text) => {
                    let properties = styk_doc::json::parse(&text).map_err(|e| MirrorError::Record(e.to_string()))?;
                    let Some(incoming) = properties.as_obj() else { return Err(MirrorError::Record("the properties are not an object".into())) };
                    let sources = tree.path(&["_styk", "frameSources"]).cloned();
                    let mut next = Obj::new();
                    for (key, value) in incoming.iter() {
                        next.set(key, value.clone());
                    }
                    for table in [Table::Figures, Table::Keyframes, Table::Backgrounds] {
                        if let Some(items) = tree.get(table.key()) {
                            next.set(table.key(), items.clone());
                        }
                    }
                    if let Some(sources) = sources {
                        let mut styk = next.get("_styk").cloned().filter(Value::is_object).unwrap_or_else(|| Value::object(Obj::new()));
                        styk.make_obj().expect("an object").set("frameSources", sources);
                        next.set("_styk", styk);
                    }
                    tree = Value::object(next);
                }
                Change::Layout(table, layout) => {
                    let old = table_items(&tree, table);
                    let mut items = Vec::with_capacity(layout.len());
                    pending.retain(|(t, _)| *t != table);
                    for (at, &from) in layout.iter().enumerate() {
                        if from < 0 {
                            pending.push((table, at));
                            items.push(Value::Null);
                        } else {
                            let from = from as usize;
                            items.push(old.get(from).cloned().ok_or(MirrorError::Index { table: table.key(), index: from, len: old.len() })?);
                        }
                    }
                    tree.make_obj().expect("the document is an object").set(table.key(), Value::Arr(Arc::new(items)));
                }
                Change::Set(table, index, lifted) => {
                    let value = record(&lifted)?;
                    resolved(&mut tree, RecordTarget::Table(table, index), Some(value), &mut pending)?;
                }
                Change::Source(id, lifted) => {
                    let value = lifted.as_ref().map(record).transpose()?;
                    resolved(&mut tree, RecordTarget::Source(id), value, &mut pending)?;
                }
                Change::Resolved(target, value) => resolved(&mut tree, target, value, &mut pending)?,
                Change::Sources(disposition) => sources_disposition = disposition,
            }
        }
        if let Some(&(table, index)) = pending.first() {
            return Err(MirrorError::Missing { table: table.key(), index });
        }
        if sources_disposition == SourceDisposition::AbsentIfEmpty {
            if tree.path(&["_styk", "frameSources"]).and_then(Value::as_obj).is_some_and(|sources| !sources.is_empty()) {
                return Err(MirrorError::Record("a nonempty source table cannot be removed".into()));
            }
            remove_path(&mut tree, &["_styk", "frameSources"]);
        }
        Ok((tree, reset))
    }
}

fn remove_path(tree: &mut Value, path: &[&str]) {
    let Some((key, parents)) = path.split_last() else { return };
    let mut owner = tree;
    for parent in parents {
        let Some(next) = owner.make_obj().and_then(|object| object.get_mut(parent)) else { return };
        owner = next;
    }
    if let Some(object) = owner.make_obj() { object.remove(key); }
}

fn set_path(tree: &mut Value, path: &[&str], value: Value) {
    let (key, parents) = path.split_last().expect("a canonical field path");
    let mut owner = tree;
    for parent in parents {
        let object = owner.make_obj().expect("the canonical owner is an object");
        if !object.get(parent).is_some_and(Value::is_object) { object.set(parent, Value::object(Obj::new())); }
        owner = object.get_mut(parent).expect("set above");
    }
    owner.make_obj().expect("the canonical owner is an object").set(key, value);
}

fn resolved(tree: &mut Value, target: RecordTarget, value: Option<Value>, pending: &mut Vec<(Table, usize)>) -> Result<(), MirrorError> {
    match target {
        RecordTarget::Table(table, index) => {
            let value = value.ok_or_else(|| MirrorError::Record("a table record cannot be absent".into()))?;
            let index = index as usize;
            let root = tree.make_obj().expect("the document is an object");
            let len = root.get(table.key()).and_then(Value::array_len).unwrap_or(0);
            let items = root.get_mut(table.key()).and_then(Value::make_arr).filter(|items| index < items.len()).ok_or(MirrorError::Index { table: table.key(), index, len })?;
            items[index] = value;
            pending.retain(|entry| *entry != (table, index));
        }
        RecordTarget::Source(id) => {
            if !tree.path(&["_styk", "frameSources"]).is_some_and(Value::is_object) {
                set_path(tree, &["_styk", "frameSources"], Value::object(Obj::new()));
            }
            match value {
                Some(value) => set_path(tree, &["_styk", "frameSources", &id], value),
                None => remove_path(tree, &["_styk", "frameSources", &id]),
            }
        }
        RecordTarget::Metadata(field) => {
            match value {
                Some(value) => set_path(tree, field.path(), value),
                None => remove_path(tree, field.path()),
            }
        }
    }
    Ok(())
}

/// Only the small in-flight identity stays staged while a worker owns changes.
/// Every error path retires that marker on this worker; discard can invalidate it.
struct CommitMarker<'a> { engine: &'a Engine, batch: u32, cancelled: Arc<std::sync::atomic::AtomicBool> }
impl Drop for CommitMarker<'_> {
    fn drop(&mut self) {
        let retired = {
            let mut staged = self.engine.inner.staged.lock();
            if staged.get(&self.batch).is_some_and(|owner| Arc::ptr_eq(&owner.cancelled, &self.cancelled)) { staged.remove(&self.batch) } else { None }
        };
        drop(retired);
    }
}

impl Engine {
    /// Stages one change in `batch`. Cheap: nothing is parsed until [`Engine::commit`].
    /// Batches are independent: a long description of the frames can be staged over
    /// many turns of the interface while small commits of other batches go through.
    pub fn stage(&self, batch: u32, change: Change) -> Result<(), TransferError> {
        // Allocate the single node outside the shared lock, then splice it in.
        let mut node = std::collections::LinkedList::new();
        node.push_back(change);
        let mut fresh = Some(transfer::Batch::default());
        let admitted = (|| {
            let mut staged = self.inner.staged.lock();
            if self.inner.stopping.load(std::sync::atomic::Ordering::SeqCst) { return Err(TransferError::Cancelled) }
            if !staged.contains_key(&batch) {
                if staged.len() >= transfer::MAX_RECORD_TRANSFERS || staged.len() == staged.capacity() { return Err(TransferError::Capacity) }
                staged.insert(batch, fresh.take().expect("prebuilt batch"));
            }
            let owner = staged.get_mut(&batch).expect("existing or inserted batch");
            if owner.guard.is_none() { owner.changes.append(&mut node); return Ok(None) }
            Ok(Some(owner.cancelled.clone()))
        })();
        let cancelled = match admitted {
            Ok(None) => return Ok(()),
            Ok(Some(cancelled)) => cancelled,
            Err(error) => { self.spawn_retained(move |_| drop(node)); return Err(error) }
        };
        // A legacy caller cannot inject unaccounted records into a guarded owner.
        // Serialize cancellation against its atomic publication, then retire both
        // the transaction and the foreign payload on the existing worker lane.
        let retired = {
            let _state = self.inner.state.lock();
            let mut staged = self.inner.staged.lock();
            if staged.get(&batch).is_some_and(|owner| Arc::ptr_eq(&owner.cancelled, &cancelled)) {
                cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
                if let Some(transfer) = staged.get(&batch).and_then(|owner| owner.transfer.as_ref()) { transfer.cancel(); }
                staged.remove(&batch)
            } else { None }
        };
        self.spawn_retained(move |_| { drop(retired); drop(node); });
        Err(TransferError::Stale)
    }

    /// Detaches an uncommitted batch in bounded work. Its lifted bytes are dropped
    /// by the existing retained cleanup lane, including across publication/stop.
    /// Guarded preparation remains cancellable until atomic publication.
    pub fn discard_batch(&self, batch: u32) {
        let changes = {
            let _state = self.inner.state.lock();
            let changes = self.inner.staged.lock().remove(&batch);
            if let Some(changes) = &changes {
                changes.cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
                if let Some(transfer) = &changes.transfer { transfer.cancel(); }
            }
            changes
        };
        if let Some(changes) = changes {
            self.spawn_retained(move |_| drop(changes));
        }
    }

    /// The mirrored document and the interface revision of its last commit, for an
    /// edit session to start from. `None` while a commit is running (this never
    /// waits) or before any document. The tree is shared, not copied.
    pub fn mirrored(&self) -> Option<(u64, Value)> {
        let mirror = self.inner.mirror.try_lock().ok()?;
        mirror.tree.get("keyframes")?;
        Some((mirror.revision, mirror.tree.clone()))
    }

    /// [`Engine::mirrored`] with what an edit session needs to start without converting
    /// the document again: a copy of the mirror's loader (typed records shared) and the
    /// typed views published for that revision. `None` while committing, as there.
    pub fn mirrored_for_editing(&self) -> Option<(u64, Value, Loader, Arc<Prepared>)> {
        let mirror = self.inner.mirror.try_lock().ok()?;
        mirror.tree.get("keyframes")?;
        Some((mirror.revision, mirror.tree.clone(), mirror.loader.clone(), mirror.prepared.clone()?))
    }

    /// Applies everything staged in `batch` and publishes the result as the document
    /// of `revision` (the interface's own revision counter). All or nothing: on an
    /// error the previous document stays. Runs on the calling thread; hosts call it
    /// through [`Engine::spawn`] for anything large.
    pub fn commit(&self, revision: u64, batch: u32) -> Result<Committed, MirrorError> {
        let started = self.now();
        // Only one commit runs at a time; commits are background tasks, so waiting here is allowed.
        let mirror = self.inner.mirror.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        self.commit_locked(mirror, started, revision, batch, None)
    }

    /// A guarded transaction must still exist with this exact nonce. A discarded
    /// or replaced batch cannot produce a successful empty legacy commit.
    /// Runs on a worker, as [`Engine::commit`].
    pub fn commit_record(&self, revision: u64, batch: u32, guard: RecordGuard) -> Result<Committed, MirrorError> {
        let started = self.now();
        let mirror = self.inner.mirror.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        self.commit_locked(mirror, started, revision, batch, Some(guard))
    }

    /// Worker-only paired publication. Prepare/validate the editor outside the
    /// shared State lock. `exchange` must only swap prebuilt heads, allocating and
    /// dropping nothing; it runs once after the final cancellation/nonce check.
    /// Keep both returned retirements on the worker through terminal ACK.
    /// Refusal returns the uncalled callback for worker retention/disposal.
    pub fn commit_record_with<F, R>(&self, revision: u64, batch: u32, guard: RecordGuard, exchange: F) -> Result<(Committed, crate::PublicationRetirement, R), (MirrorError, F)>
    where F: FnOnce() -> R {
        let started = self.now();
        let mirror = self.inner.mirror.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut exchange = Some(exchange);
        self.commit_locked_with(mirror, started, revision, batch, Some(guard), None, &mut exchange).map_err(|error| (error, exchange.take().expect("refused before head exchange")))
    }

    /// [`Engine::commit`] for the interface thread, which may not wait (D7): `Ok(None)` while
    /// another commit runs, the batch staged as it was, to be committed by a later call.
    pub fn try_commit(&self, revision: u64, batch: u32) -> Result<Option<Committed>, MirrorError> {
        // A chunked/validated batch is always prepared and published by workers.
        if self.inner.staged.lock().get(&batch).is_some_and(|batch| batch.guard.is_some()) { return Ok(None) }
        let started = self.now();
        let mirror = match self.inner.mirror.try_lock() {
            Ok(mirror) => mirror,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return Ok(None),
        };
        self.commit_locked(mirror, started, revision, batch, None).map(Some)
    }

    fn commit_locked(&self, mirror: std::sync::MutexGuard<'_, Mirror>, started: f64, revision: u64, batch: u32, guard: Option<RecordGuard>) -> Result<Committed, MirrorError> {
        let (committed, retirement, ()) = self.commit_locked_with(mirror, started, revision, batch, guard, None, &mut Some(|| ()))?;
        if retirement.needs_worker() { self.spawn_retained(move |_| drop(retirement)); }
        Ok(committed)
    }

    fn commit_locked_with<F, R>(&self, mirror: std::sync::MutexGuard<'_, Mirror>, started: f64, revision: u64, batch: u32, guard: Option<RecordGuard>, expected_root: Option<(&Value, &Value, &Arc<transfer::Charge>, bool)>, exchange: &mut Option<F>) -> Result<(Committed, crate::PublicationRetirement, R), MirrorError>
    where F: FnOnce() -> R {
        self.prepare_locked(mirror, started, revision, batch, guard, expected_root, None)?
            .commit(exchange).map_err(|(error, candidate)| { drop(candidate); error })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Budgets;

    fn clock() -> f64 {
        0.0
    }

    #[test]
    fn the_interface_thread_never_waits_for_a_commit() {
        let engine = Engine::new(clock, Budgets::for_device(None));
        engine.stage(0, Change::Reset).unwrap();
        engine.stage(0, Change::Properties(br#"{"version":5,"canvas_width":320,"canvas_height":200,"fps":20}"#.to_vec())).unwrap();
        engine.stage(0, Change::Layout(Table::Figures, vec![])).unwrap();
        engine.stage(0, Change::Layout(Table::Keyframes, vec![])).unwrap();
        // Another commit holds the mirror (a background commit of the document's frames).
        let busy = engine.inner.mirror.lock().unwrap();
        assert!(matches!(engine.try_commit(1, 0), Ok(None)), "the interface thread is told to come back, not made to wait");
        assert!(engine.inner.staged.lock().contains_key(&0), "the batch stays staged");
        drop(busy);
        let done = engine.try_commit(1, 0).unwrap().expect("the mirror is free");
        assert_eq!(done.revision, 1);
        assert!(!engine.inner.staged.lock().contains_key(&0));
    }

    #[test]
    fn discarded_dense_batches_are_detached_once_and_disposed_by_retained_workers() {
        let engine = Engine::new(clock, Budgets::for_device(None));
        engine.stage(7, Change::Properties(vec![b' '; 64 * 1024 * 1024])).unwrap();
        engine.discard_batch(7);
        assert!(!engine.inner.staged.lock().contains_key(&7));
        assert_eq!(engine.inner.state.lock().retained_tasks.len(), 1, "the dense bytes have not been dropped on the caller");
        for _ in 0..100 { engine.discard_batch(7); }
        assert_eq!(engine.inner.state.lock().retained_tasks.len(), 1, "empty/repeated discards enqueue no work");
        engine.stage(0, Change::Reset).unwrap();
        engine.stage(0, Change::Properties(br#"{"version":5,"canvas_width":320,"canvas_height":200,"fps":20}"#.to_vec())).unwrap();
        engine.stage(0, Change::Layout(Table::Keyframes, vec![])).unwrap();
        engine.commit(2, 0).unwrap();
        assert_eq!(engine.inner.state.lock().retained_tasks.len(), 1, "publication retains worker disposal");
        engine.stop();
        engine.work();
        assert_eq!(engine.inner.state.lock().retained_tasks.len(), 0, "stop drains disposal before workers return");
    }
}
