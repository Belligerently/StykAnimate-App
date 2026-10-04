//! Bounded lifted-record receiver in the existing staged mirror owner.
//!
//! The caller sends one packet, waits for its tagged ACK, then sends the next.
//! Workers alone append, parse and retire dense buffers. A completed record is
//! staged as the canonical Value, with no second parse at document commit.

use std::collections::LinkedList;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use styk_formats::container::{MAX_BYTES, MAX_SKELETON_BYTES, MAX_MATERIALIZED_BYTES, parse_lifted_owned_admitted_within};

use super::{Change, Record, RecordTarget};
use crate::{Engine, State};
use crate::sync::Shared;

pub const MAX_RECORD_PACKET_BYTES: usize = 64 * 1024;
pub(crate) const MAX_RECORD_TRANSFERS: usize = 8;
/// Shared retained transport budget, independent from single-record format admission.
pub const MAX_RECORD_RETAINED_BYTES: usize = MAX_MATERIALIZED_BYTES;
pub const MAX_RECORD_CHANGES: usize = 65_536;

#[derive(Default)]
pub(crate) struct Budget { bytes: AtomicUsize, records: AtomicUsize }
#[cfg(test)]
impl Budget { pub(crate) fn retained_bytes(&self) -> usize { self.bytes.load(Ordering::SeqCst) } }

fn reserve(counter: &AtomicUsize, amount: usize, limit: usize) -> Result<(), TransferError> {
    counter.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |old| old.checked_add(amount).filter(|new| *new <= limit))
        .map(|_| ()).map_err(|_| TransferError::Capacity)
}

/// The charge follows the payload, including detached/cancelled worker ownership.
pub(crate) struct Charge { global: Arc<Budget>, batch: Arc<Budget>, bytes: usize }
impl Charge {
    pub(super) fn new(global: Arc<Budget>, batch: Arc<Budget>) -> Result<Self, TransferError> {
        reserve(&global.records, 1, MAX_RECORD_CHANGES)?;
        if let Err(error) = reserve(&batch.records, 1, MAX_RECORD_CHANGES) { global.records.fetch_sub(1, Ordering::SeqCst); return Err(error) }
        Ok(Self { global, batch, bytes: 0 })
    }
    pub(super) fn grow(&mut self, bytes: usize) -> Result<(), TransferError> {
        reserve(&self.global.bytes, bytes, MAX_RECORD_RETAINED_BYTES)?;
        if let Err(error) = reserve(&self.batch.bytes, bytes, MAX_RECORD_RETAINED_BYTES) { self.global.bytes.fetch_sub(bytes, Ordering::SeqCst); return Err(error) }
        self.bytes += bytes;
        Ok(())
    }
    pub(super) fn budget(&self) -> Arc<Budget> { self.batch.clone() }
    pub(super) fn grow_to(&mut self, bytes: usize) -> Result<(), TransferError> {
        self.grow(bytes.saturating_sub(self.bytes))
    }
    // Only the owning worker, after every displaced raw head is physically
    // dropped, may shrink the SAME reservation. No replacement allowance.
    pub(super) fn shrink_to(&mut self, bytes: usize) {
        assert!(bytes <= self.bytes);
        let released = self.bytes - bytes;
        self.global.bytes.fetch_sub(released, Ordering::SeqCst);
        self.batch.bytes.fetch_sub(released, Ordering::SeqCst);
        self.bytes = bytes;
    }
}
impl Drop for Charge {
    fn drop(&mut self) {
        self.global.bytes.fetch_sub(self.bytes, Ordering::SeqCst);
        self.batch.bytes.fetch_sub(self.bytes, Ordering::SeqCst);
        self.global.records.fetch_sub(1, Ordering::SeqCst);
        self.batch.records.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordGuard {
    pub generation: u64,
    pub lineage: u64,
    pub revision: u64,
    /// The batch transaction identity, distinct from its record ticket.
    pub nonce: u64,
}

impl RecordGuard {
    pub(crate) fn current(self, state: &State) -> bool {
        state.current.as_ref().map_or(
            self.generation == 0 && self.lineage == 0 && self.revision == 0,
            |generation| (generation.id, generation.lineage, generation.revision) == (self.generation, self.lineage, self.revision),
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PacketKind { Skeleton, Arrays, Payloads }

pub struct Packet {
    pub kind: PacketKind,
    pub offset: usize,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordLengths {
    pub skeleton: usize,
    pub arrays: usize,
    pub payloads: usize,
    pub materialization_limit: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TransferError {
    #[error("the record transfer is not current")]
    Stale,
    #[error("the record receiver is full")]
    Capacity,
    #[error("the previous record packet has not been acknowledged")]
    Pending,
    #[error("the record transfer was cancelled")]
    Cancelled,
    #[error("the record packet exceeds the transfer bound")]
    PacketSize,
    #[error("the record packet sequence or offset is invalid")]
    Sequence,
    #[error("the record section lengths do not match")]
    Length,
    #[error("a lifted transaction cannot reuse legacy staged changes")]
    LegacyBatch,
    #[error("the lifted record is invalid: {0}")]
    Record(String),
}

/// A small terminal or packet response. It never contains a dense record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordAck {
    pub ticket: u64,
    pub sequence: u64,
    pub finished: bool,
    pub error: Option<TransferError>,
}

#[derive(Default)]
pub(crate) struct Batch {
    pub changes: LinkedList<Change>,
    pub guard: Option<RecordGuard>,
    pub transfer: Option<Arc<Transfer>>,
    pub cancelled: Arc<AtomicBool>,
    pub committing: bool,
    pub budget: Arc<Budget>,
    pub charges: LinkedList<Charge>,
    pub(super) root: Option<super::root::Root>,
}
impl Batch {
    pub(crate) fn with_budget(budget: Arc<Budget>) -> Self {
        Self { changes: LinkedList::new(), guard: None, transfer: None, cancelled: Arc::new(AtomicBool::new(false)), committing: false, budget, charges: LinkedList::new(), root: None }
    }
    pub(crate) fn retire_worker(self) {
        let wake = self.root.as_ref().and_then(|root| root.wake.clone());
        drop(self); // all raw heads/charge aliases FIRST, original Core wake SECOND
        if let Some(core) = wake.and_then(|core| core.upgrade()) { core.schedule(); }
    }
}

/// The existing staged owner, with a bounded index of guarded batches. Legacy
/// stages keep their existing API; retirement never enumerates their records.
pub(crate) struct Staged {
    batches: std::collections::HashMap<u32, Batch>,
    guarded: [Option<(u32, RecordGuard)>; MAX_RECORD_TRANSFERS],
}

impl Default for Staged {
    fn default() -> Self {
        Self { batches: std::collections::HashMap::with_capacity(MAX_RECORD_TRANSFERS), guarded: [None; MAX_RECORD_TRANSFERS] }
    }
}
impl std::ops::Deref for Staged {
    type Target = std::collections::HashMap<u32, Batch>;
    fn deref(&self) -> &Self::Target { &self.batches }
}
impl std::ops::DerefMut for Staged {
    fn deref_mut(&mut self) -> &mut Self::Target { &mut self.batches }
}
impl Staged {
    fn retire_worker(self) {
        // Actor roots are guarded; this existing index bounds wake extraction.
        let wakes: [_; MAX_RECORD_TRANSFERS] = std::array::from_fn(|index| {
            self.guarded[index].and_then(|(id, _)| self.get(&id))
                .and_then(|batch| batch.root.as_ref()).and_then(|root| root.wake.clone())
        });
        drop(self);
        for core in wakes.into_iter().flatten().filter_map(|core| core.upgrade()) {
            core.schedule();
        }
    }
    pub(crate) fn remove(&mut self, batch: &u32) -> Option<Batch> {
        for entry in &mut self.guarded { if entry.is_some_and(|(id, _)| id == *batch) { *entry = None; } }
        self.batches.remove(batch)
    }
    fn track(&mut self, batch: u32, guard: RecordGuard) -> Result<(), TransferError> {
        if let Some(old) = self.guarded.iter().flatten().find(|(id, _)| *id == batch) {
            return if old.1 == guard { Ok(()) } else { Err(TransferError::Stale) };
        }
        let slot = self.guarded.iter_mut().find(|slot| slot.is_none()).ok_or(TransferError::Capacity)?;
        *slot = Some((batch, guard));
        Ok(())
    }
}

struct Reservation(Arc<AtomicUsize>);
impl Drop for Reservation {
    fn drop(&mut self) { self.0.fetch_sub(1, Ordering::SeqCst); }
}

struct Buffer {
    record: Option<Record>,
    reservation: Option<Reservation>,
    sequence: u64,
    charge: Option<Charge>,
}

pub(crate) struct Transfer {
    guard: RecordGuard,
    ticket: u64,
    target: RecordTarget,
    cancelled: AtomicBool,
    pending: AtomicBool,
    completed: AtomicBool,
    ack: Shared<Option<RecordAck>>,
    /// No interface method locks or drops this worker-owned buffer.
    buffer: Mutex<Buffer>,
}

impl Transfer {
    pub(crate) fn cancel(&self) { self.cancelled.store(true, Ordering::SeqCst); }
    pub(crate) fn complete(&self) -> bool { self.completed.load(Ordering::SeqCst) }
    pub(crate) fn acknowledged(&self) -> bool { self.complete() && !self.pending.load(Ordering::SeqCst) }

    fn response(&self, sequence: u64, finished: bool, error: Option<TransferError>) {
        *self.ack.lock() = Some(RecordAck { ticket: self.ticket, sequence, finished, error });
    }

    fn retire(&self) {
        let mut buffer = self.buffer.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        buffer.record.take();
        buffer.reservation.take();
        buffer.charge.take();
    }

    fn live(&self, engine: &Engine) -> Result<(), TransferError> {
        if self.cancelled.load(Ordering::SeqCst) || engine.inner.stopping.load(Ordering::SeqCst) { return Err(TransferError::Cancelled) }
        if !engine.record_guard_current(self.guard) { return Err(TransferError::Stale) }
        Ok(())
    }

    fn packet(&self, engine: &Engine, sequence: u64, packet: Packet) {
        let result = (|| {
            self.live(engine)?;
            let mut buffer = self.buffer.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if sequence != buffer.sequence + 1 { return Err(TransferError::Sequence) }
            let Buffer { record, charge, .. } = &mut *buffer;
            let record = record.as_mut().ok_or(TransferError::Sequence)?;
            let (bytes, limit) = match packet.kind {
                PacketKind::Skeleton => (&mut record.skeleton, MAX_SKELETON_BYTES),
                PacketKind::Arrays => (&mut record.arrays, MAX_BYTES),
                PacketKind::Payloads => (&mut record.payloads, MAX_BYTES),
            };
            if packet.offset != bytes.len() { return Err(TransferError::Sequence) }
            if packet.bytes.len() > limit.saturating_sub(bytes.len()) { return Err(TransferError::Length) }
            charge.as_mut().expect("active record charge").grow(packet.bytes.len())?;
            bytes.extend_from_slice(&packet.bytes);
            buffer.sequence = sequence;
            drop(buffer);
            self.live(engine)
        })();
        if result.is_err() { self.cancel(); self.retire(); }
        self.response(sequence, false, result.err());
    }

    fn finish(self: &Arc<Self>, engine: &Engine, batch: u32, sequence: u64, lengths: Option<RecordLengths>) {
        let result = (|| {
            self.live(engine)?;
            let (lifted, mut charge) = {
                let mut buffer = self.buffer.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                if sequence != buffer.sequence + 1 { return Err(TransferError::Sequence) }
                let record = buffer.record.as_ref().ok_or(TransferError::Sequence)?;
                let expected = lengths.map_or((0, 0, 0), |lengths| (lengths.skeleton, lengths.arrays, lengths.payloads));
                if (record.skeleton.len(), record.arrays.len(), record.payloads.len()) != expected {
                    return Err(TransferError::Length);
                }
                buffer.sequence = sequence;
                (buffer.record.take().expect("checked above"), buffer.charge.take().expect("active record charge"))
            };
            // The existing parser alone owns marker admission and materialization.
            let value = if let Some(lengths) = lengths {
                let admitted = parse_lifted_owned_admitted_within(lifted.skeleton, lifted.arrays, lifted.payloads, lengths.materialization_limit).map_err(|error| {
                    let mut message = error.to_string();
                    let mut end = message.len().min(1024);
                    while !message.is_char_boundary(end) { end -= 1; }
                    message.truncate(end);
                    TransferError::Record(message)
                })?;
                charge.grow(admitted.retained_bytes.saturating_sub(charge.bytes))?;
                Some(admitted.document)
            } else {
                if matches!(self.target, RecordTarget::Table(_, _)) { return Err(TransferError::Record("a table record cannot be absent".into())); }
                None
            };
            self.live(engine)?;
            // Allocate before taking either UI-visible lock. The splice below
            // only moves pointers; dense validation and payload drops stay here.
            let mut node = LinkedList::new();
            node.push_back(Change::Resolved(self.target.clone(), value));
            let mut charge_node = LinkedList::new();
            charge_node.push_back(charge);
            let state = engine.inner.state.lock();
            if !self.guard.current(&state) || engine.inner.stopping.load(Ordering::SeqCst) || self.cancelled.load(Ordering::SeqCst) { return Err(TransferError::Stale) }
            let mut staged = engine.inner.staged.lock();
            let owner = staged.get_mut(&batch).ok_or(TransferError::Stale)?;
            if owner.guard != Some(self.guard) || !owner.transfer.as_ref().is_some_and(|current| Arc::ptr_eq(current, self)) { return Err(TransferError::Stale) }
            owner.changes.append(&mut node);
            owner.charges.append(&mut charge_node);
            self.completed.store(true, Ordering::SeqCst);
            Ok(())
        })();
        if result.is_err() { self.cancel(); }
        self.retire();
        self.response(sequence, true, result.err());
    }
}

impl Engine {
    pub(crate) fn record_guard_current(&self, guard: RecordGuard) -> bool { guard.current(&self.inner.state.lock()) }

    /// Begins one lifted record. No payload is enumerated, parsed or copied here.
    /// Finished transfers cannot be replaced until their terminal ACK was taken.
    pub fn begin_record(&self, batch: u32, guard: RecordGuard, ticket: u64, target: RecordTarget) -> Result<(), TransferError> {
        if matches!(&target, RecordTarget::Source(id) if id.len() > 1024) { return Err(TransferError::PacketSize) }
        if self.inner.stopping.load(Ordering::SeqCst) { return Err(TransferError::Cancelled) }
        self.inner.record_slots.fetch_update(Ordering::SeqCst, Ordering::SeqCst,
            |count| (count < MAX_RECORD_TRANSFERS).then_some(count + 1)).map_err(|_| TransferError::Capacity)?;
        let existing_budget = { let staged = self.inner.staged.lock(); staged.get(&batch).map(|owner| owner.budget.clone()) };
        let budget = existing_budget.unwrap_or_else(|| Arc::new(Budget::default()));
        let mut fresh = Some(Batch::with_budget(budget.clone()));
        let charge = match Charge::new(self.inner.record_budget.clone(), budget.clone()) {
            Ok(charge) => charge,
            Err(error) => { self.inner.record_slots.fetch_sub(1, Ordering::SeqCst); return Err(error) }
        };
        let transfer = Arc::new(Transfer {
            guard, ticket, target, cancelled: AtomicBool::new(false), pending: AtomicBool::new(false), completed: AtomicBool::new(false), ack: Shared::new(None),
            buffer: Mutex::new(Buffer { record: Some(Record { skeleton: Vec::new(), arrays: Vec::new(), payloads: Vec::new() }), reservation: Some(Reservation(self.inner.record_slots.clone())), sequence: 0, charge: Some(charge) }),
        });
        let state = self.inner.state.lock();
        if !guard.current(&state) || self.inner.stopping.load(Ordering::SeqCst) { return Err(TransferError::Stale) }
        let mut staged = self.inner.staged.lock();
        if !staged.contains_key(&batch) && (staged.len() >= MAX_RECORD_TRANSFERS || staged.len() == staged.capacity()) { return Err(TransferError::Capacity) }
        if let Some(owner) = staged.get(&batch) {
            if owner.guard.is_none() && !owner.changes.is_empty() { return Err(TransferError::LegacyBatch) }
            if owner.committing || owner.root.is_some() { return Err(TransferError::Pending) }
            if owner.guard.is_some_and(|old| old != guard) { return Err(TransferError::Stale) }
            if owner.transfer.as_ref().is_some_and(|old| !old.complete() || old.pending.load(Ordering::SeqCst)) { return Err(TransferError::Pending) }
        }
        staged.track(batch, guard)?;
        if !staged.contains_key(&batch) { staged.insert(batch, fresh.take().expect("prebuilt batch")); }
        let owner = staged.get_mut(&batch).expect("existing or inserted batch");
        if !Arc::ptr_eq(&owner.budget, &budget) { return Err(TransferError::Stale) }
        owner.guard = Some(guard);
        let retired = owner.transfer.replace(transfer);
        drop(staged);
        drop(state);
        drop(retired);
        Ok(())
    }

    fn transfer(&self, batch: u32, ticket: u64) -> Result<Arc<Transfer>, TransferError> {
        self.inner.staged.lock().get(&batch).and_then(|batch| batch.transfer.clone()).filter(|transfer| transfer.ticket == ticket).ok_or(TransferError::Stale)
    }

    /// Copies at most one bounded packet into existing retained worker work.
    pub fn record_packet(&self, batch: u32, ticket: u64, sequence: u64, packet: Packet) -> Result<(), TransferError> {
        if packet.bytes.is_empty() || packet.bytes.len() > MAX_RECORD_PACKET_BYTES { return Err(TransferError::PacketSize) }
        let transfer = self.transfer(batch, ticket)?;
        if transfer.complete() || transfer.cancelled.load(Ordering::SeqCst) { return Err(TransferError::Cancelled) }
        transfer.pending.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).map_err(|_| TransferError::Pending)?;
        self.spawn_retained(move |engine| transfer.packet(engine, sequence, packet));
        Ok(())
    }

    /// Assembles/parses on a worker, staging a resolved Value only after exact
    /// lengths, marker admission and generation/transaction identity all match.
    pub fn finish_record(&self, batch: u32, ticket: u64, sequence: u64, lengths: RecordLengths) -> Result<(), TransferError> {
        let transfer = self.transfer(batch, ticket)?;
        if transfer.complete() || transfer.cancelled.load(Ordering::SeqCst) { return Err(TransferError::Cancelled) }
        transfer.pending.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).map_err(|_| TransferError::Pending)?;
        self.spawn_retained(move |engine| transfer.finish(engine, batch, sequence, Some(lengths)));
        Ok(())
    }

    /// Explicit absence keeps `None` distinct from a transported JSON `null`.
    pub fn finish_absent_record(&self, batch: u32, ticket: u64, sequence: u64) -> Result<(), TransferError> {
        let transfer = self.transfer(batch, ticket)?;
        if transfer.complete() || transfer.cancelled.load(Ordering::SeqCst) { return Err(TransferError::Cancelled) }
        transfer.pending.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).map_err(|_| TransferError::Pending)?;
        self.spawn_retained(move |engine| transfer.finish(engine, batch, sequence, None));
        Ok(())
    }

    /// Worker-only control changes. Dense values must carry the existing reader's
    /// validated admission via `stage_guarded_admitted`, never a second traversal.
    pub fn stage_guarded(&self, batch: u32, guard: RecordGuard, change: Change) -> Result<(), TransferError> {
        let bytes = match &change {
            Change::Reset | Change::Sources(_) | Change::Resolved(_, None | Some(styk_doc::Value::Null)) => 64,
            Change::Layout(_, layout) => layout.len().checked_mul(4).ok_or(TransferError::Capacity)?,
            _ => return Err(TransferError::Record("dense canonical changes require validated admission".into())),
        };
        self.stage_guarded_admitted(batch, guard, change, bytes)
    }

    /// Worker-only: `retained_bytes` comes from the consuming reader or the owning
    /// producer's existing validated allocation accounting. The charge remains
    /// reserved until that worker-owned payload is disposed, even after cancel.
    pub fn stage_guarded_admitted(&self, batch: u32, guard: RecordGuard, change: Change, retained_bytes: usize) -> Result<(), TransferError> {
        let existing_budget = { let staged = self.inner.staged.lock(); staged.get(&batch).map(|owner| owner.budget.clone()) };
        let budget = existing_budget.unwrap_or_else(|| Arc::new(Budget::default()));
        let mut charge = Charge::new(self.inner.record_budget.clone(), budget.clone())?;
        charge.grow(retained_bytes)?;
        let mut charges = LinkedList::new();
        charges.push_back(charge);
        let mut node = LinkedList::new();
        node.push_back(change);
        self.stage_guarded_owner(batch, guard, budget, node, charges, None)
    }

    // The sole admission/publication owner for ordinary admitted records and
    // worker-prepared roots. All payload/node allocation precedes shared locks.
    pub(super) fn stage_guarded_owner(&self, batch: u32, guard: RecordGuard, budget: Arc<Budget>, node: LinkedList<Change>, charges: LinkedList<Charge>, root: Option<super::root::Root>) -> Result<(), TransferError> {
        self.stage_guarded_cancelled_owner(batch, guard, budget, node, charges, root, None)
    }

    // Actor-only cancellation alias; every producer still uses this SAME
    // admission/index/charge owner and final publication cancellation check.
    pub(super) fn stage_guarded_cancelled_owner(&self, batch: u32, guard: RecordGuard, budget: Arc<Budget>, mut node: LinkedList<Change>, mut charges: LinkedList<Charge>, root: Option<super::root::Root>, cancelled: Option<Arc<AtomicBool>>) -> Result<(), TransferError> {
        let mut fresh = Some(Batch::with_budget(budget.clone()));
        if let Some(cancelled) = cancelled.as_ref() { fresh.as_mut().expect("prebuilt batch").cancelled = cancelled.clone(); }
        let mut incoming_root = root;
        let state = self.inner.state.lock();
        if !guard.current(&state) || self.inner.stopping.load(Ordering::SeqCst) { return Err(TransferError::Stale) }
        let mut staged = self.inner.staged.lock();
        if !staged.contains_key(&batch) && (staged.len() >= MAX_RECORD_TRANSFERS || staged.len() == staged.capacity()) { return Err(TransferError::Capacity) }
        if let Some(owner) = staged.get(&batch) {
            if cancelled.as_ref().is_some_and(|cancelled| !Arc::ptr_eq(cancelled, &owner.cancelled)) { return Err(TransferError::Stale) }
            if owner.guard.is_none() && !owner.changes.is_empty() { return Err(TransferError::LegacyBatch) }
            if owner.committing || owner.root.is_some() { return Err(TransferError::Pending) }
            if incoming_root.is_some() && (!owner.changes.is_empty() || owner.transfer.is_some()) { return Err(TransferError::LegacyBatch) }
            if owner.guard.is_some_and(|old| old != guard) { return Err(TransferError::Stale) }
        }
        staged.track(batch, guard)?;
        if !staged.contains_key(&batch) { staged.insert(batch, fresh.take().expect("prebuilt batch")); }
        let owner = staged.get_mut(&batch).expect("existing or inserted batch");
        if !Arc::ptr_eq(&owner.budget, &budget) { return Err(TransferError::Stale) }
        owner.guard = Some(guard);
        owner.changes.append(&mut node);
        owner.charges.append(&mut charges);
        std::mem::swap(&mut owner.root, &mut incoming_root);
        Ok(())
    }

    pub fn record_transfer_current(&self, batch: u32, ticket: u64) -> bool {
        self.transfer(batch, ticket).is_ok_and(|transfer| transfer.live(self).is_ok())
    }

    /// Nonblocking, small tagged response. A result from another generation is
    /// returned as refused rather than certifying stale staged data.
    pub fn take_record_ack(&self, batch: u32, ticket: u64) -> Option<RecordAck> {
        let transfer = self.transfer(batch, ticket).ok()?;
        let mut ack = transfer.ack.lock().take()?;
        let stale = {
            let state = self.inner.state.lock();
            let stale = !transfer.guard.current(&state) || self.inner.stopping.load(Ordering::SeqCst) || transfer.cancelled.load(Ordering::SeqCst) && ack.error.is_none();
            transfer.pending.store(false, Ordering::SeqCst);
            stale
        };
        // A parse-error ACK owns an allocated String. Rewrite/drop that error
        // after State unlock, preserving the same guarded ACK linearization.
        if stale { ack.error = Some(TransferError::Stale); }
        Some(ack)
    }

    /// Publication detaches only its displaced generation's guarded batches,
    /// at most eight. A later accepted publication may already own the current
    /// head; this delayed cleanup must preserve its transfers and terminal ACKs.
    /// Records and partial buffers use the same retained worker disposal lane.
    pub(crate) fn retire_record_batches(&self, generation: u64) {
        let mut retired = Vec::with_capacity(MAX_RECORD_TRANSFERS);
        {
            let mut staged = self.inner.staged.lock();
            let entries = staged.guarded;
            for (batch, guard) in entries.into_iter().flatten() {
                if guard.generation == generation {
                    if let Some(batch) = staged.remove(&batch) {
                        batch.cancelled.store(true, Ordering::SeqCst);
                        if let Some(transfer) = &batch.transfer { transfer.cancel(); }
                        retired.push(batch);
                    }
                }
            }
        }
        if !retired.is_empty() { self.spawn_retained(move |_| {
            for batch in retired { batch.retire_worker(); }
        }); }
    }

    pub(crate) fn stop_record_batches(&self) {
        if self.inner.stopping.load(Ordering::SeqCst) { return }
        let replacement = Staged::default();
        // Build the empty disposal job before locking. Publish its pointer and
        // the stop flag together, so no worker can return before cleanup exists.
        let payload = Arc::new(Mutex::new(None::<Staged>));
        let worker_payload = payload.clone();
        let mut node: LinkedList<Box<dyn FnOnce(&Engine) + Send>> = LinkedList::new();
        node.push_back(Box::new(move |_| {
            let retired = worker_payload.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take();
            if let Some(retired) = retired { retired.retire_worker(); }
        }));
        {
            let mut state = self.inner.state.lock();
            if self.inner.stopping.load(Ordering::SeqCst) { return }
            let retired = std::mem::replace(&mut *self.inner.staged.lock(), replacement);
            *payload.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(retired);
            state.retained_tasks.append(&mut node);
            state.images.cancel_preparation();
            self.inner.stopping.store(true, Ordering::SeqCst);
        }
    }
}

#[cfg(test)]
#[path = "transfer_tests.rs"]
mod tests;
