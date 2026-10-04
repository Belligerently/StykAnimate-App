//! One canonical Session on the existing durable worker lane.
//!
//! The interface owns a weak control handle. It never locks the Session or owns
//! its prepared heads/cursor/retirements. Quiet commands use invisible bounded
//! staging, readiness ACK, paired publication, then terminal success ACK.
//! Recorded live gestures use the same moved Session capability and final
//! current/cancel owner. Browser ready-ticket/shown admission remains separate.
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use styk_doc::json;
use styk_edit::publication::RetiredPublication;
use styk_edit::{
    EditError, HistoryLimits,
    session::{RevisionCounters, Session, WorkerSession, WorkerPrepared, WorkerFrame},
};

use crate::mirror::{EditPublicationAdmission, MirrorError, RecordGuard};
use crate::planning::{CHUNK_BYTES, Chunks};
use crate::{Engine, Inner, PublicationRetirement};

pub const MAX_SESSION_CONTROL_BYTES: usize = CHUNK_BYTES;
const TIMEOUT_MS: f64 = 30_000.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub generation: u64,
    pub lineage: u64,
    pub revision: u64,
    pub sequence: u64,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Epochs {
    pub shown: u64,
    pub view: u64,
    pub gesture: u64,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    Unavailable,
    #[default]
    Idle,
    Preparing,
    Attached,
    Streaming,
    Ready,
    Success,
    Failed,
    Closed,
}
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("The canonical editing session is busy.")]
    Busy,
    #[error("The canonical editing session is closed.")]
    Closed,
    #[error("The canonical editing session was cancelled.")]
    Cancelled,
    #[error("The canonical editing session belongs to a stale document or view.")]
    Stale,
    #[error("The canonical editing session timed out.")]
    TimedOut,
    #[error("The editing control input exceeds its byte budget.")]
    InputBudget,
    #[error("The editing control input is not valid JSON.")]
    InvalidInput,
    #[error("A warm JavaScript history must keep its existing owner.")]
    WarmHistory,
    #[error("The editing ACK arrived out of order.")]
    Ack,
    #[error("{0}")]
    Owner(String),
}
fn owner(error: impl std::fmt::Display) -> Error {
    Error::Owner(error.to_string())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Success {
    pub request: u64,
    pub stamp: Stamp,
    pub epochs: Epochs,
}
#[derive(Debug)]
pub struct Packet {
    pub request: u64,
    pub sequence: u32,
    pub json: String,
}
#[derive(Default, Debug)]
pub struct Poll {
    pub request: u64,
    pub phase: Phase,
    pub packet: Option<Packet>,
    pub success: Option<Success>,
    pub error: Option<Error>,
}
struct Input {
    request: u64,
    stamp: Stamp,
    epochs: Epochs,
    method: String,
    args: String,
    view: String,
    project_fields: bool,
    live: bool,
}
/// Only scalar evidence from the foreign owner's actual history. The UI never
/// exports, sums or hashes its entries to initialize a fresh Rust owner.
#[derive(Clone, Copy, Debug)]
pub struct FreshHistory {
    pub undo: usize,
    pub redo: usize,
    pub checkpoint: f64,
    pub counters: RevisionCounters,
    pub limits: HistoryLimits,
}
struct Initialization {
    state: String,
    history: FreshHistory,
}
#[derive(Clone, Copy)]
enum Control {
    Ack,
    Activate(Epochs),
    Ready(Epochs),
    Terminal,
}
#[derive(Default)]
struct Mailbox {
    request: u64,
    phase: Phase,
    input: Option<Input>,
    initialization: Option<Initialization>,
    control: Option<Control>,
    packet: Option<Packet>,
    awaiting: Option<u32>,
    delivered: bool,
    ready_delivered: bool,
    success: Option<Success>,
    error: Option<Error>,
}
struct Pending {
    request: u64,
    batch: u32,
    guard: RecordGuard,
    epochs: Epochs,
    phase: Phase,
    sequence: u32,
    prepared: Option<WorkerPrepared>,
    admission: Option<EditPublicationAdmission>,
    cursor: Option<Chunks>,
    engine_retired: Option<PublicationRetirement>,
    editor_retired: Option<RetiredPublication>,
    ready_retired: Option<WorkerFrame>,
}
struct Worker {
    session: Option<WorkerSession>,
    active: bool,
    stamp: Stamp,
    epochs: Epochs,
    pending: Option<Pending>,
    // Actual prepared editing display on the durable worker. It is not the
    // browser's Shown owner; no dense payload crosses a UI handle here.
    ready: Option<WorkerFrame>,
}
pub(crate) struct Core {
    engine: Weak<Inner>,
    worker: Mutex<Worker>,
    mailbox: Arc<Mutex<Mailbox>>,
    queued: AtomicBool,
    wake_pending: AtomicBool,
    closed: AtomicBool,
    cancellation: AtomicU8,
    // Alias of the existing mirror Batch.cancelled owner. The interface only
    // sets this atom; final paired publication reads it under State.
    batch_cancelled: Arc<AtomicBool>,
    request: AtomicU64,
    next_request: AtomicU64,
    generation: AtomicU64,
    committing: AtomicBool,
    deadline: AtomicU64,
}
/// Only weak ownership and one bounded control/packet mailbox cross the UI.
/// Dropping a handle schedules durable worker cleanup; it never drops Session.
pub struct Handle {
    core: Weak<Core>,
    mailbox: Arc<Mutex<Mailbox>>,
}

impl Core {
    fn new(engine: &Engine, worker: Worker, mail: Mailbox, request: u64) -> Arc<Self> {
        let generation = worker.stamp.generation;
        Arc::new(Self {
            engine: Arc::downgrade(&engine.inner),
            worker: Mutex::new(worker),
            mailbox: Arc::new(Mutex::new(mail)),
            queued: AtomicBool::new(false),
            wake_pending: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            cancellation: AtomicU8::new(0),
            batch_cancelled: Arc::new(AtomicBool::new(false)),
            request: AtomicU64::new(request),
            next_request: AtomicU64::new(request + 1),
            generation: AtomicU64::new(generation),
            committing: AtomicBool::new(false),
            deadline: AtomicU64::new((engine.now() + TIMEOUT_MS).to_bits()),
        })
    }
    fn initialize(
        &self,
        engine: &Engine,
        worker: &mut Worker,
        initialization: Initialization,
    ) -> Result<(), Error> {
        self.check(engine, worker.stamp)?;
        let (revision, report, loader, prepared) =
            engine.mirrored_for_editing().ok_or(Error::Stale)?;
        if revision != worker.stamp.revision {
            return Err(Error::Stale);
        }
        let mut session = Session::start_worker_prepared(
            report,
            &initialization.state,
            loader,
            prepared,
            initialization.history.counters,
        )
        .map_err(owner)?;
        session
            .prepare_worker_history(
                initialization.history.limits,
                initialization.history.checkpoint,
                || self.edit_check(engine, worker.stamp),
            )
            .map_err(owner)?;
        self.check(engine, worker.stamp)?;
        worker.session = Some(session.into_worker());
        let mut mail = self
            .mailbox
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        mail.phase = Phase::Attached;
        mail.success = Some(Success {
            request: mail.request,
            stamp: worker.stamp,
            epochs: worker.epochs,
        });
        drop(mail);
        self.touch(engine);
        engine.notify_progress();
        Ok(())
    }
    fn activate(&self, engine: &Engine, worker: &mut Worker, epochs: Epochs) -> Result<(), Error> {
        if worker.active || worker.session.is_none() || epochs != worker.epochs {
            return Err(Error::Stale);
        }
        self.check(engine, worker.stamp)?;
        worker.active = true;
        self.request.store(0, Ordering::Release);
        let mut mail = self
            .mailbox
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        mail.phase = Phase::Idle;
        mail.request = 0;
        mail.success = None;
        drop(mail);
        engine.notify_progress();
        Ok(())
    }
    fn touch(&self, engine: &Engine) {
        self.deadline
            .store((engine.now() + TIMEOUT_MS).to_bits(), Ordering::Release);
    }
    fn schedule(self: &Arc<Self>) {
        // Record every producer wake before coalescing. A cancellation can
        // arrive after the worker's last check while its task is still queued.
        self.wake_pending.store(true, Ordering::Release);
        if let Some(inner) = self.engine.upgrade() {
            // No State/session lock and no queued closure on the interface.
            // Engine::step claims this single existing Core on its worker.
            inner.state.notify_all();
        }
    }
    fn check(&self, engine: &Engine, stamp: Stamp) -> Result<(), Error> {
        if self.closed.load(Ordering::Acquire) || engine.inner.stopping.load(Ordering::Acquire) {
            return Err(Error::Closed);
        }
        match self.cancellation.load(Ordering::Acquire) {
            1 => return Err(Error::Cancelled),
            2 => return Err(Error::TimedOut),
            _ => {}
        }
        let current = engine.generation().ok_or(Error::Stale)?;
        if current.id != stamp.generation
            || current.lineage != stamp.lineage
            || current.revision != stamp.revision
        {
            return Err(Error::Stale);
        }
        Ok(())
    }
    fn edit_check(&self, engine: &Engine, stamp: Stamp) -> Result<(), EditError> {
        self.check(engine, stamp)
            .map_err(|error| EditError::Invalid(error.to_string()))
    }
    fn fail(&self, engine: &Engine, worker: &mut Worker, request: u64, error: Error) {
        if let Some(pending) = worker.pending.take() {
            engine.discard_batch(pending.batch);
            drop(pending); // all candidate/cursor/charged retirement disposal is worker-only
        }
        if !worker.active {
            drop(worker.session.take());
        }
        let (packet, input, initialization, previous) = {
            let mut mail = self.mailbox.lock().unwrap_or_else(|e| e.into_inner());
            mail.phase = Phase::Failed;
            mail.request = request;
            mail.awaiting = None;
            mail.delivered = false;
            mail.ready_delivered = false;
            mail.success = None;
            let old = mail.error.replace(error);
            (
                mail.packet.take(),
                mail.input.take(),
                mail.initialization.take(),
                old,
            )
        };
        drop((packet, input, initialization, previous));
        engine.notify_progress();
    }
    fn dispose(&self, engine: &Engine, worker: &mut Worker) {
        if let Some(pending) = worker.pending.take() {
            engine.discard_batch(pending.batch);
            drop(pending);
        }
        drop((worker.session.take(), worker.ready.take()));
        let retired = {
            let mut mail = self.mailbox.lock().unwrap_or_else(|e| e.into_inner());
            mail.phase = Phase::Closed;
            mail.awaiting = None;
            mail.delivered = false;
            (
                mail.packet.take(),
                mail.input.take(),
                mail.initialization.take(),
                mail.error.take(),
            )
        };
        drop(retired);
        let detached = {
            let mut active = engine.inner.session.lock();
            if active
                .as_ref()
                .is_some_and(|core| std::ptr::eq(core.as_ref(), self))
            {
                active.take()
            } else {
                None
            }
        };
        drop(detached); // Session is already absent; no UI last-strong payload
        engine.notify_progress();
    }
    fn prepare(&self, engine: &Engine, worker: &mut Worker, input: Input) -> Result<(), Error> {
        if input.stamp != worker.stamp || input.epochs.shown != worker.epochs.shown {
            return Err(Error::Stale);
        }
        self.check(engine, input.stamp)?;
        let args = json::parse(input.args.as_bytes()).map_err(|_| Error::InvalidInput)?;
        let view = json::parse(input.view.as_bytes()).map_err(|_| Error::InvalidInput)?;
        if input.project_fields && !view.is_null() {
            // The single owning Session already holds selection/playhead state.
            // A foreign view can mutate layer metadata without a revision, which
            // is outside this explicitly sparse root-field response surface.
            return Err(Error::InvalidInput);
        }
        let session = worker.session.as_mut().ok_or(Error::Closed)?;
        let prepared = if input.live {
            session.prepare_live(&input.method, &args, &view, || self.edit_check(engine, input.stamp))
        } else {
            session.prepare_quiet(&input.method, &args, &view, || self.edit_check(engine, input.stamp))
        }.map_err(owner)?;
        let response = if input.project_fields {
            prepared.publication().project_fields_response_worker(&input.method).map_err(owner)?
        } else if input.live {
            let mut response = styk_doc::Obj::new();
            response.set("kind", styk_doc::Value::str("session-live"));
            response.set("answer", prepared.result().clone());
            response.set("history", prepared.publication().history().clone());
            response.set("runtime", prepared.runtime().clone());
            response.set("stored", styk_doc::Value::Bool(prepared.revision() != prepared.before_revision()));
            styk_doc::Value::object(response)
        } else { prepared.result().clone() };
        self.check(engine, input.stamp)?;
        let guard = RecordGuard {
            generation: input.stamp.generation,
            lineage: input.stamp.lineage,
            revision: input.stamp.revision,
            nonce: input.request,
        };
        let batch = 0x8000_0000u32
            .checked_add(u32::try_from(input.request).map_err(|_| Error::Closed)?)
            .ok_or(Error::Closed)?;
        let check = || self.check(engine, input.stamp).map_err(|_|
            MirrorError::Transaction("the Session request was cancelled or superseded"));
        let admission = if input.live {
            engine.stage_live_session_publication_worker(batch, guard, &prepared, &response,
                self.batch_cancelled.clone(), check)
        } else {
            engine.stage_session_publication_response_worker(batch, guard, &prepared, &response,
                self.batch_cancelled.clone(), check)
        }.map_err(owner)?;
        let cursor = Chunks::new(response).map_err(owner)?;
        worker.pending = Some(Pending {
            request: input.request,
            batch,
            guard,
            epochs: input.epochs,
            phase: Phase::Streaming,
            sequence: 0,
            prepared: Some(prepared),
            admission: Some(admission),
            cursor: Some(cursor),
            engine_retired: None,
            editor_retired: None,
            ready_retired: None,
        });
        Ok(())
    }
    fn stream(&self, engine: &Engine, worker: &mut Worker) -> Result<(), Error> {
        let pending = worker.pending.as_mut().ok_or(Error::Ack)?;
        self.check(engine, worker.stamp)?;
        match pending
            .cursor
            .as_mut()
            .ok_or(Error::Ack)?
            .next_chunk()
            .map_err(owner)?
        {
            Some(json) => {
                self.check(engine, worker.stamp)?;
                #[cfg(test)]
                crate::allocation::before_lock(self as *const Self as usize);
                let packet = Packet {
                    request: pending.request,
                    sequence: pending.sequence,
                    json,
                };
                pending.sequence = pending.sequence.checked_add(1).ok_or(Error::Ack)?;
                self.touch(engine);
                let mut mail = self.mailbox.lock().unwrap_or_else(|e| e.into_inner());
                mail.phase = Phase::Streaming;
                mail.awaiting = Some(packet.sequence);
                mail.delivered = false;
                debug_assert!(mail.packet.is_none());
                mail.packet = Some(packet);
            }
            None => {
                drop(pending.cursor.take());
                pending.phase = Phase::Ready;
                let mut mail = self.mailbox.lock().unwrap_or_else(|e| e.into_inner());
                mail.phase = Phase::Ready;
                mail.ready_delivered = false;
            }
        }
        engine.notify_progress();
        Ok(())
    }
    fn commit(&self, engine: &Engine, worker: &mut Worker, epochs: Epochs) -> Result<(), Error> {
        let pending = worker.pending.as_mut().ok_or(Error::Ack)?;
        if pending.phase != Phase::Ready || pending.epochs != epochs {
            return Err(Error::Stale);
        }
        self.check(engine, worker.stamp)?;
        let session = worker.session.as_mut().ok_or(Error::Closed)?;
        let prepared = pending.prepared.take().ok_or(Error::Ack)?;
        let (publication, display, runtime) = match session
            .guard(prepared, || self.edit_check(engine, worker.stamp))
        {
            Ok(publication) => publication,
            Err((error, prepared)) => {
                pending.prepared = Some(prepared);
                return Err(owner(error));
            }
        };
        drop(runtime); // small observation; response already retains its exact view
        let admission = pending.admission.take().ok_or(Error::Ack)?;
        let ready = &mut worker.ready;
        let exchange = move |retired| {
            let previous = if let Some(display) = display {
                ready.replace(display)
            } else { None };
            (retired, previous)
        };
        self.committing.store(true, Ordering::Release);
        let result = engine.commit_edit_publication_with_worker(
            pending.batch,
            pending.guard,
            publication,
            admission,
            exchange,
        );
        match result {
            Ok((committed, engine_retired, (editor_retired, ready_retired))) => {
                #[cfg(test)]
                crate::allocation::before_lock(&self.committing as *const AtomicBool as usize);
                worker.stamp.generation = committed.generation;
                worker.stamp.revision = committed.revision;
                worker.stamp.sequence = pending.request;
                self.generation
                    .store(committed.generation, Ordering::Release);
                self.committing.store(false, Ordering::Release);
                worker.epochs = epochs;
                pending.phase = Phase::Success;
                pending.engine_retired = Some(engine_retired);
                pending.editor_retired = Some(editor_retired);
                pending.ready_retired = ready_retired;
                let success = Success {
                    request: pending.request,
                    stamp: worker.stamp,
                    epochs,
                };
                self.touch(engine);
                let mut mail = self.mailbox.lock().unwrap_or_else(|e| e.into_inner());
                mail.phase = Phase::Success;
                mail.success = Some(success);
            }
            Err((error, publication, retirement, exchange)) => {
                self.committing.store(false, Ordering::Release);
                drop((publication, retirement, exchange)); // caller is the durable owning worker
                return Err(owner(error));
            }
        }
        engine.notify_progress();
        Ok(())
    }
    fn advance(self: Arc<Self>, engine: &Engine) {
        // Consume this wake before observing controls/cancellation. Any later
        // producer marks another wake, even if enqueue coalesces with this task.
        self.wake_pending.swap(false, Ordering::AcqRel);
        let mut worker = self.worker.lock().unwrap_or_else(|e| e.into_inner());
        if self.closed.load(Ordering::Acquire) || engine.inner.stopping.load(Ordering::Acquire) {
            self.dispose(engine, &mut worker);
            self.queued.store(false, Ordering::Release);
            return;
        }
        let (input, initialization, control) = {
            let mut mail = self.mailbox.lock().unwrap_or_else(|e| e.into_inner());
            (
                mail.input.take(),
                mail.initialization.take(),
                mail.control.take(),
            )
        };
        let request = self.request.load(Ordering::Acquire);
        // Terminal ACK is valid after success or refusal, even when cancelled.
        if matches!(control, Some(Control::Terminal)) {
            drop(worker.pending.take());
            self.cancellation.store(0, Ordering::Release);
            self.request.store(0, Ordering::Release);
            let retired = {
                let mut mail = self.mailbox.lock().unwrap_or_else(|e| e.into_inner());
                mail.phase = Phase::Idle;
                mail.request = 0;
                mail.success = None;
                (mail.packet.take(), mail.error.take())
            };
            drop(retired);
            if !worker.active {
                self.dispose(engine, &mut worker);
            }
            engine.notify_progress();
        // Publication has already linearized: cancellation only retires resources.
        } else if worker
            .pending
            .as_ref()
            .is_some_and(|pending| pending.phase == Phase::Success)
            && self.cancellation.load(Ordering::Acquire) != 0
        {
            // Every committed operation now owns an actual ready display.
            // Keep both charged retirements and the prior ready head through
            // terminal acceptance/retirement ACK; cancellation cannot make a
            // consumer the last dense owner before that barrier.
        } else if let Err(error) = self.check(engine, worker.stamp) {
            if input.is_some()
                || initialization.is_some()
                || worker.pending.is_some()
                || !worker.active
            {
                self.fail(engine, &mut worker, request, error);
            }
        } else if let Some(initialization) = initialization {
            if let Err(error) = self.initialize(engine, &mut worker, initialization) {
                self.fail(engine, &mut worker, request, error);
            }
        } else if let Some(input) = input {
            match self
                .prepare(engine, &mut worker, input)
                .and_then(|()| self.stream(engine, &mut worker))
            {
                Ok(()) => {}
                Err(error) => self.fail(engine, &mut worker, request, error),
            }
        } else {
            let result = match control {
                Some(Control::Ack) => self.stream(engine, &mut worker),
                Some(Control::Activate(epochs)) => self.activate(engine, &mut worker, epochs),
                Some(Control::Ready(epochs)) => self.commit(engine, &mut worker, epochs),
                Some(Control::Terminal) => unreachable!("handled before cancellation"),
                None => Ok(()),
            };
            if let Err(error) = result {
                self.fail(engine, &mut worker, request, error);
            }
        }
        drop(worker);
        self.queued.store(false, Ordering::Release);
        let again = {
            let mail = self.mailbox.lock().unwrap_or_else(|e| e.into_inner());
            mail.input.is_some() || mail.initialization.is_some() || mail.control.is_some()
        };
        if again || self.wake_pending.load(Ordering::Acquire) || self.closed.load(Ordering::Acquire) {
            // Preserve the pending bit for the next task to consume. Merely
            // retaining a Failed/Success cancellation does not create a wake.
            self.schedule();
        }
    }
}

impl Handle {
    /// Encoded, already bounded control only. No JSON parsing/walk on the UI.
    /// Dense arguments require the common receiver; callers must validate their
    /// known small control shape BEFORE serialization. At most one active request.
    pub fn submit(
        &self,
        stamp: Stamp,
        epochs: Epochs,
        method: &str,
        args: &str,
        view: &str,
    ) -> Result<u64, Error> {
        self.submit_inner(stamp, epochs, method, args, view, false, false)
    }
    /// Narrow application response: <=3 ordered scalar root fields from the
    /// actual prepared history command, plus bounded scalar history/state.
    /// Host arguments still require known-shape validation BEFORE serialization.
    pub fn submit_project_fields(
        &self, stamp: Stamp, epochs: Epochs, method: &str, args: &str, view: &str,
    ) -> Result<u64, Error> {
        if !matches!(method, "setFPS" | "setCanvasSize" | "updateProject" | "undo" | "redo") {
            return Err(Error::InputBudget);
        }
        self.submit_inner(stamp, epochs, method, args, view, true, false)
    }
    /// Recorded exclusive live stream. Browser presentation/adoption remains
    /// unadmitted until its actual ready/Shown ACK uses this same owner.
    pub fn submit_live(
        &self, stamp: Stamp, epochs: Epochs, method: &str, args: &str,
    ) -> Result<u64, Error> {
        if !matches!(method, "select" | "beginDrag" | "moveSelection" | "commitDrag"
            | "cancelDrag" | "beginAnimationEdit" | "previewAnimationEdit"
            | "commitAnimationEdit" | "cancelAnimationEdit" | "seek" | "undo" | "redo") {
            return Err(Error::InputBudget);
        }
        self.submit_inner(stamp, epochs, method, args, "null", false, true)
    }
    fn submit_inner(
        &self, stamp: Stamp, epochs: Epochs, method: &str, args: &str, view: &str, project_fields: bool, live: bool,
    ) -> Result<u64, Error> {
        if method.len() > 128
            || method
                .len()
                .saturating_add(args.len())
                .saturating_add(view.len())
                > MAX_SESSION_CONTROL_BYTES
        {
            return Err(Error::InputBudget);
        }
        let core = self.core.upgrade().ok_or(Error::Closed)?;
        if core.closed.load(Ordering::Acquire) {
            return Err(Error::Closed);
        }
        let method = method.to_owned();
        let args = args.to_owned();
        let view = view.to_owned();
        let Ok(mut mail) = self.mailbox.try_lock() else {
            return Err(Error::Busy);
        };
        if mail.phase != Phase::Idle {
            return Err(Error::Busy);
        }
        let request = core.next_request.fetch_add(1, Ordering::Relaxed);
        if request > 0x7fff_ffff {
            return Err(Error::Closed);
        }
        mail.request = request;
        mail.phase = Phase::Preparing;
        mail.input = Some(Input {
            request,
            stamp,
            epochs,
            method,
            args,
            view,
            project_fields,
            live,
        });
        core.request.store(request, Ordering::Release);
        core.cancellation.store(0, Ordering::Release);
        core.batch_cancelled.store(false, Ordering::SeqCst);
        drop(mail);
        if let Some(inner) = core.engine.upgrade() {
            core.touch(&Engine { inner });
        }
        core.schedule();
        Ok(request)
    }
    pub fn poll(&self) -> Poll {
        let Ok(mut mail) = self.mailbox.try_lock() else {
            return Poll {
                phase: Phase::Unavailable,
                ..Poll::default()
            };
        };
        let packet = mail.packet.take();
        if packet.is_some() {
            mail.delivered = true;
        }
        if mail.phase == Phase::Ready {
            mail.ready_delivered = true;
        }
        Poll {
            request: mail.request,
            phase: mail.phase,
            packet,
            success: mail.success,
            error: mail.error.take(),
        }
    }
    fn control(&self, request: u64, control: Control) -> Result<bool, Error> {
        let core = self.core.upgrade().ok_or(Error::Closed)?;
        let Ok(mut mail) = self.mailbox.try_lock() else {
            return Ok(false);
        };
        if mail.request != request || mail.control.is_some() {
            return Err(Error::Ack);
        }
        match control {
            Control::Ack => unreachable!(),
            Control::Activate(_) if mail.phase != Phase::Attached => return Err(Error::Ack),
            Control::Ready(_) if mail.phase != Phase::Ready || !mail.ready_delivered => {
                return Err(Error::Ack);
            }
            Control::Terminal if !matches!(mail.phase, Phase::Success | Phase::Failed) => {
                return Err(Error::Ack);
            }
            _ => {}
        }
        mail.control = Some(control);
        drop(mail);
        if let Some(inner) = core.engine.upgrade() {
            core.touch(&Engine { inner });
        }
        core.schedule();
        Ok(true)
    }
    pub fn ack_packet(&self, request: u64, sequence: u32) -> Result<bool, Error> {
        let core = self.core.upgrade().ok_or(Error::Closed)?;
        let Ok(mut mail) = self.mailbox.try_lock() else {
            return Ok(false);
        };
        if mail.request != request
            || mail.phase != Phase::Streaming
            || mail.awaiting != Some(sequence)
            || !mail.delivered
            || mail.control.is_some()
        {
            return Err(Error::Ack);
        }
        mail.awaiting = None;
        mail.delivered = false;
        mail.control = Some(Control::Ack);
        drop(mail);
        if let Some(inner) = core.engine.upgrade() {
            core.touch(&Engine { inner });
        }
        core.schedule();
        Ok(true)
    }
    /// ACK only after invisible response staging and actual view/shown validation.
    pub fn ready(&self, request: u64, epochs: Epochs) -> Result<bool, Error> {
        self.control(request, Control::Ready(epochs))
    }
    /// Fresh handoff only: the host has frozen the original JS writer and
    /// revalidated document/counter/selection epochs after the Attached header.
    pub fn activate(&self, request: u64, epochs: Epochs) -> Result<bool, Error> {
        self.control(request, Control::Activate(epochs))
    }
    pub fn terminal(&self, request: u64) -> Result<bool, Error> {
        self.control(request, Control::Terminal)
    }
    pub fn cancel(&self, request: u64) {
        if let Some(core) = self.core.upgrade() {
            if core.request.load(Ordering::Acquire) != request {
                return;
            }
            core.cancellation.store(1, Ordering::Release);
            core.batch_cancelled.store(true, Ordering::SeqCst);
            core.schedule();
        }
    }
    pub fn close(&self) {
        if let Some(core) = self.core.upgrade() {
            core.closed.store(true, Ordering::Release);
            self.cancel(core.request.load(Ordering::Acquire));
            core.schedule();
        }
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        self.close();
    }
}

impl Engine {
    fn install_session_core(&self, core: &Arc<Core>, stamp: Stamp) -> Result<(), Error> {
        // Final reservation validates the same acknowledged generation. Only
        // prebuilt pointers are exchanged under the two constant-size locks.
        let state = self.inner.state.try_lock().ok_or(Error::Busy)?;
        if self.inner.stopping.load(Ordering::Acquire)
            || !state.current.as_ref().is_some_and(|generation| {
                generation.id == stamp.generation
                    && generation.lineage == stamp.lineage
                    && generation.revision == stamp.revision
            })
        {
            return Err(Error::Stale);
        }
        let mut active = self.inner.session.try_lock().ok_or(Error::Busy)?;
        if active.is_some() {
            return Err(Error::Busy);
        }
        *active = Some(core.clone());
        Ok(())
    }
    /// Bounded interface request. The SAME canonical Session slot is reserved
    /// before worker initialization; no document/history enters this mailbox.
    /// The host checks these actual foreign scalar lengths/checkpoint BEFORE
    /// serializing its known bounded state. Warm histories refuse unchanged.
    pub fn request_fresh_session(
        &self,
        stamp: Stamp,
        epochs: Epochs,
        state: &str,
        history: FreshHistory,
    ) -> Result<Handle, Error> {
        if history.undo != 0 || history.redo != 0 || history.checkpoint != 0.0 {
            return Err(Error::WarmHistory);
        }
        if stamp.sequence != 0
            || history.counters.revision != stamp.revision
            || history.counters.revision > history.counters.serial
            || history.counters.saved > history.counters.serial
        {
            return Err(Error::Stale);
        }
        if state.len() > MAX_SESSION_CONTROL_BYTES {
            return Err(Error::InputBudget);
        }
        if !history.limits.bytes.is_finite() || history.limits.bytes < 0.0 {
            return Err(Error::InvalidInput);
        }
        let core = Core::new(
            self,
            Worker {
                session: None,
                active: false,
                stamp,
                epochs,
                pending: None,
                ready: None,
            },
            Mailbox {
                request: 1,
                phase: Phase::Preparing,
                initialization: Some(Initialization {
                    state: state.to_owned(),
                    history,
                }),
                ..Mailbox::default()
            },
            1,
        );
        self.install_session_core(&core, stamp)?;
        let handle = Handle {
            core: Arc::downgrade(&core),
            mailbox: core.mailbox.clone(),
        };
        core.schedule();
        Ok(handle)
    }
    /// Worker-only move of the SAME idle Session, preserving its complete history,
    /// counters, settings, caches and source references. Never serialize/rebase it.
    /// No corresponding interface binding may return this strong owner.
    pub fn detach_session_worker(&self) -> Result<(Session, Stamp, Epochs), Error> {
        let core = self.inner.session.lock().clone().ok_or(Error::Closed)?;
        let mut worker = core.worker.lock().unwrap_or_else(|e| e.into_inner());
        {
            let mail = core.mailbox.lock().unwrap_or_else(|e| e.into_inner());
            if mail.phase != Phase::Idle
                || worker.pending.is_some()
                || core.queued.load(Ordering::Acquire)
            {
                return Err(Error::Busy);
            }
        }
        core.check(self, worker.stamp)?;
        let session = worker.session.take().ok_or(Error::Closed)?;
        let stamp = worker.stamp;
        let epochs = worker.epochs;
        drop(worker.ready.take());
        core.closed.store(true, Ordering::Release);
        {
            let mut mail = core.mailbox.lock().unwrap_or_else(|e| e.into_inner());
            mail.phase = Phase::Closed;
        }
        let detached = {
            let mut active = self.inner.session.lock();
            if active
                .as_ref()
                .is_some_and(|owner| Arc::ptr_eq(owner, &core))
            {
                active.take()
            } else {
                None
            }
        };
        drop(detached);
        drop(worker);
        Ok((session.into_session(), stamp, epochs))
    }
    /// Worker-only handoff of the SAME existing Session. No second editor is built.
    /// Warm history uses its original exact checkpoint/settings. Failure returns
    /// the owner to this worker; never let an interface handle own that return.
    pub fn attach_session_worker(
        &self,
        session: Session,
        limits: HistoryLimits,
        checkpoint: f64,
        epochs: Epochs,
    ) -> Result<(Handle, Stamp), (Error, Session)> {
        if self.inner.stopping.load(Ordering::Acquire) {
            return Err((Error::Closed, session));
        }
        let Some(state) = self.inner.state.try_lock() else {
            return Err((Error::Busy, session));
        };
        let stamp = match state.current.as_ref() {
            Some(generation) => Stamp { generation: generation.id, lineage: generation.lineage, revision: generation.revision, sequence: 0 },
            None => return Err((Error::Stale, session)),
        };
        drop(state);
        let Some(active) = self.inner.session.try_lock() else {
            return Err((Error::Busy, session));
        };
        if active.is_some() { return Err((Error::Busy, session)); }
        drop(active);
        if !self.mirrored().is_some_and(|(revision, report)| {
            revision == stamp.revision && report.same_node(session.report())
        }) || session.revision() != stamp.revision
        {
            return Err((Error::Stale, session));
        }
        let core = Core::new(
            self,
            Worker {
                session: Some(session.into_worker()),
                active: false,
                stamp,
                epochs,
                pending: None,
                ready: None,
            },
            Mailbox::default(),
            0,
        );
        // Hold the private owner across reservation/preparation so a stopping
        // worker cannot dispose it before a failed attach returns that owner.
        let mut worker = core.worker.lock().unwrap_or_else(|e| e.into_inner());
        if let Err(error) = self.install_session_core(&core, stamp) {
            let session = worker.session.take().expect("unattached owner");
            drop(worker);
            return Err((error, session.into_session()));
        }
        // Reserve the canonical slot BEFORE any warm-history preparation. A
        // racing admission Busy must return exactly the original Session.
        let prepared = worker.session.as_mut().expect("reserved owner")
                .prepare_worker_history(limits, checkpoint, || core.edit_check(self, stamp));
        if prepared.is_ok() { worker.active = true; }
        if let Err(error) = prepared {
            core.closed.store(true, Ordering::Release);
            let session = worker.session.take().expect("reserved owner");
            drop(worker);
            let detached = {
                let mut active = self.inner.session.lock();
                if active.as_ref().is_some_and(|owner| Arc::ptr_eq(owner, &core)) { active.take() } else { None }
            };
            drop(detached);
            return Err((owner(error), session.into_session()));
        }
        drop(worker);
        Ok((
            Handle {
                core: Arc::downgrade(&core),
                mailbox: core.mailbox.clone(),
            },
            stamp,
        ))
    }
}

pub(crate) fn cancel_current(engine: &Engine) {
    // stop_record_batches publishes the existing global stopping atom. The
    // worker services its current Session before draining stop retirements.
    // Do not lock or acquire the strong owner on the interface shutdown path.
    engine.inner.state.notify_all();
}
/// Claim only the single canonical slot. Workers may lock; producers never do.
pub(crate) fn service_current(engine: &Engine) -> bool {
    let core = engine.inner.session.lock().clone();
    let Some(core) = core else { return false };
    if !(core.wake_pending.load(Ordering::Acquire)
        || engine.inner.stopping.load(Ordering::Acquire))
        || core.queued.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err()
    {
        return false;
    }
    core.advance(engine);
    true
}
pub(crate) fn expire_current(engine: &Engine) {
    let core = engine.inner.session.lock().clone();
    if let Some(core) = core {
        // Compare the paired heads under the SAME State read. A valid commit
        // publishes Engine first, then updates Core generation before clearing
        // committing; an earlier standalone flag/snapshot can cross that gap.
        let superseded = {
            let state = engine.inner.state.lock();
            !core.committing.load(Ordering::Acquire)
                && state.current.as_ref().is_none_or(|generation| {
                    generation.id != core.generation.load(Ordering::Acquire)
                })
        };
        if superseded {
            core.closed.store(true, Ordering::Release);
            core.schedule();
            return;
        }
        if core.request.load(Ordering::Acquire) != 0
            && engine.now() >= f64::from_bits(core.deadline.load(Ordering::Acquire))
            && core
                .cancellation
                .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            core.batch_cancelled.store(true, Ordering::SeqCst);
            core.schedule();
        }
    }
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
