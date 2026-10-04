//! The engine as JavaScript sees it.

use std::sync::{Arc, Mutex};

use styk_engine::mirror::{Change, Committed, MetadataField, Packet, PacketKind, Record, RecordGuard, RecordLengths, RecordTarget, SourceDisposition, Table, MAX_RECORD_PACKET_BYTES};
use styk_engine::{Budgets, Engine, Opened};
use wasm_bindgen::prelude::*;

use crate::{clock, js_error};

/// The engine. Create one, give its [`EngineHandle::address`] to each worker.
#[wasm_bindgen]
pub struct EngineHandle {
    pub(crate) engine: Engine,
    /// The outcome of the last [`EngineHandle::open`], until JavaScript takes it.
    opened: Arc<Mutex<Option<Result<Opened, String>>>>,
    /// Outcomes of commits, oldest first, until JavaScript takes them.
    committed: Arc<Mutex<Vec<Result<Committed, (u64, String)>>>>,
}

fn table(code: u32) -> Result<Table, JsError> {
    Table::from_code(code).ok_or_else(|| js_error(format!("unknown document table {code}")))
}

pub(crate) fn record_counter(value: f64) -> Result<u64, &'static str> {
    if !value.is_finite() || value < 0.0 || value.fract() != 0.0 || value > 9_007_199_254_740_991.0 {
        return Err("the record transfer counter is not an exact nonnegative JavaScript integer");
    }
    Ok(value as u64)
}

fn record_guard(counters: &js_sys::Float64Array, length: u32) -> Result<RecordGuard, JsError> {
    if counters.length() != length { return Err(js_error("the record transfer guard has the wrong length")); }
    Ok(RecordGuard {
        generation: record_counter(counters.get_index(0)).map_err(js_error)?,
        lineage: record_counter(counters.get_index(1)).map_err(js_error)?,
        revision: record_counter(counters.get_index(2)).map_err(js_error)?,
        nonce: record_counter(counters.get_index(3)).map_err(js_error)?,
    })
}

fn opened_json(opened: &Opened) -> String {
    let unsupported: Vec<String> = opened.unsupported.iter().map(|feature| format!("{feature:?}")).collect();
    format!(
        "{{\"generation\":{},\"name\":{:?},\"version\":{},\"canvas\":[{},{}],\"fps\":{},\"ticks\":{},\"figures\":{},\"keyframes\":{},\"images\":{},\"openMs\":{:.3},\"unsupported\":[{}]}}",
        opened.generation,
        opened.name,
        opened.version,
        opened.canvas[0],
        opened.canvas[1],
        opened.fps,
        opened.ticks,
        opened.figures,
        opened.keyframes,
        opened.images,
        opened.open_ms,
        unsupported.join(",")
    )
}

#[wasm_bindgen]
impl EngineHandle {
    /// `device_memory_gib` is `navigator.deviceMemory` where the browser has it.
    #[wasm_bindgen(constructor)]
    pub fn new(device_memory_gib: Option<f64>) -> EngineHandle {
        EngineHandle { engine: Engine::with_notifier(clock, Budgets::for_device(device_memory_gib), Some(notify_progress)), opened: Default::default(), committed: Default::default() }
    }

    /// An address a worker passes to [`worker_run`]. Each call adds one owner.
    pub fn address(&self) -> usize {
        Box::into_raw(Box::new(self.engine.clone())) as usize
    }

    /// Consumes a worker ticket whose message failed to transfer.
    pub fn forget_address(&self, address: usize) {
        if address != 0 {
            // SAFETY: the caller owns the untransferred address produced above.
            drop(unsafe { Box::from_raw(address as *mut styk_engine::Engine) });
        }
    }

    /// Address of the 32-bit progress counter in the module's memory, for
    /// `Atomics.waitAsync`: it changes whenever background work finishes.
    pub fn progress_address(&self) -> usize {
        self.engine.progress().as_ptr() as usize
    }

    /// Opens a file in the background. Watch the progress counter, then call
    /// [`EngineHandle::take_opened`].
    pub fn open(&self, bytes: Vec<u8>, extension: String, name: String) {
        let slot = self.opened.clone();
        self.engine.spawn(move |engine| {
            let result = engine.open(&bytes, &extension, &name).map_err(|e| e.to_string());
            *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
        });
    }

    /// The description of the opened document as JSON, once; `undefined` while it loads.
    pub fn take_opened(&self) -> Result<Option<String>, JsError> {
        // Only tried: the interface thread must not wait for a worker storing its result.
        let Ok(mut slot) = self.opened.try_lock() else { return Ok(None) };
        match slot.take() {
            None => Ok(None),
            Some(Ok(opened)) => Ok(Some(opened_json(&opened))),
            Some(Err(message)) => Err(js_error(message)),
        }
    }

    /// Starts describing another document: the mirrored one is forgotten at the next commit.
    pub fn stage_reset(&self, batch: u32) -> Result<(), JsError> {
        self.engine.stage(batch, Change::Reset).map_err(js_error)
    }

    /// Releases an abandoned, uncommitted mirror batch and its staged bytes.
    pub fn discard_batch(&self, batch: u32) { self.engine.discard_batch(batch); }

    /// One coherent guard snapshot: generation,lineage,revision; all zeros
    /// before the first document. The caller adds a transaction nonce/ticket.
    pub fn record_identity(&self) -> Vec<f64> {
        self.engine.generation().map_or_else(|| vec![0.0; 3], |generation| vec![generation.id as f64, generation.lineage as f64, generation.revision as f64])
    }

    /// Undefined means busy. Bootstrap must preserve its current owner and
    /// retry explicitly; it cannot wait on worker cache/publication locks.
    pub fn try_record_identity(&self) -> Option<Vec<f64>> {
        self.engine.generation_identity_now().map(|identity| identity.map(|value| value as f64).to_vec())
    }

    /// Guard array: generation,lineage,revision,batch nonce,record ticket.
    /// Target 0=table (code 0..2), 1=source, 2=metadata (code 0..4).
    /// Array lengths and IDs are bounded before crossing owned memory.
    pub fn begin_record_transfer(&self, batch: u32, counters: js_sys::Float64Array, target: u32, code: u32, index: u32, id: js_sys::JsString) -> Result<(), JsError> {
        let guard = record_guard(&counters, 5)?;
        let ticket = record_counter(counters.get_index(4)).map_err(js_error)?;
        let target = match target {
            0 => RecordTarget::Table(table(code)?, index),
            1 => {
                if id.length() > 256 { return Err(js_error("the retained source identity is too long")); }
                RecordTarget::Source(id.as_string().ok_or_else(|| js_error("a retained source identity must be a string"))?)
            }
            2 => RecordTarget::Metadata(MetadataField::from_code(code).ok_or_else(|| js_error("unknown canonical metadata field"))?),
            _ => return Err(js_error("unknown record transfer target")),
        };
        self.engine.begin_record(batch, guard, ticket, target).map_err(js_error)
    }

    /// Packet kinds: 0=skeleton UTF-8 bytes, 1=numbers, 2=payloads. The binding
    /// checks its byte bound before to_vec; worker append is acknowledged once.
    pub fn record_transfer_packet(&self, batch: u32, ticket: f64, sequence: u32, kind: u32, offset: u32, bytes: js_sys::Uint8Array) -> Result<(), JsError> {
        if bytes.length() == 0 || bytes.length() as usize > MAX_RECORD_PACKET_BYTES { return Err(js_error("the record packet exceeds its transfer bound")); }
        let kind = match kind { 0 => PacketKind::Skeleton, 1 => PacketKind::Arrays, 2 => PacketKind::Payloads, _ => return Err(js_error("unknown record packet kind")) };
        self.engine.record_packet(batch, record_counter(ticket).map_err(js_error)?, sequence as u64, Packet { kind, offset: offset as usize, bytes: bytes.to_vec() }).map_err(js_error)
    }

    /// Length array: skeleton,numbers,payloads,materialization limit. No record
    /// is admitted until the worker returns its tagged terminal ACK.
    pub fn finish_record_transfer(&self, batch: u32, ticket: f64, sequence: u32, lengths: js_sys::Uint32Array) -> Result<(), JsError> {
        if lengths.length() != 4 { return Err(js_error("the record section length array is invalid")); }
        self.engine.finish_record(batch, record_counter(ticket).map_err(js_error)?, sequence as u64, RecordLengths {
            skeleton: lengths.get_index(0) as usize, arrays: lengths.get_index(1) as usize, payloads: lengths.get_index(2) as usize, materialization_limit: lengths.get_index(3) as usize,
        }).map_err(js_error)
    }

    pub fn finish_absent_record_transfer(&self, batch: u32, ticket: f64, sequence: u32) -> Result<(), JsError> {
        self.engine.finish_absent_record(batch, record_counter(ticket).map_err(js_error)?, sequence as u64).map_err(js_error)
    }

    /// Small JSON response or undefined while pending. A retired ticket is an
    /// error, allowing a waiter to abandon its cursor rather than wait forever.
    pub fn take_record_transfer_ack(&self, batch: u32, ticket: f64) -> Result<Option<String>, JsError> {
        let ticket = record_counter(ticket).map_err(js_error)?;
        let ack = self.engine.take_record_ack(batch, ticket);
        if ack.is_none() && !self.engine.record_transfer_current(batch, ticket) { return Err(js_error("the record transfer was superseded or cancelled")); }
        Ok(ack.map(|ack| {
            let error = ack.error.map_or_else(|| "null".into(), |error| styk_doc::json::to_string(&styk_doc::Value::str(&error.to_string())));
            format!("{{\"ticket\":{},\"sequence\":{},\"finished\":{},\"error\":{error}}}", ack.ticket, ack.sequence, ack.finished)
        }))
    }

    /// Explicit final source-table absence, without replacing its contents.
    pub fn record_sources_absent_if_empty(&self, batch: u32, counters: js_sys::Float64Array) -> Result<(), JsError> {
        self.engine.stage_guarded(batch, record_guard(&counters, 4)?, Change::Sources(SourceDisposition::AbsentIfEmpty)).map_err(js_error)
    }

    /// Background commit guarded through final publication. A missing/replaced
    /// nonce is refused rather than acknowledging an empty legacy batch.
    pub fn commit_record_transfer(&self, revision: f64, batch: u32, counters: js_sys::Float64Array) -> Result<(), JsError> {
        let revision = record_counter(revision).map_err(js_error)?;
        let guard = record_guard(&counters, 4)?;
        let slot = self.committed.clone();
        self.engine.spawn(move |engine| {
            let result = engine.commit_record(revision, batch, guard).map_err(|error| (revision, error.to_string()));
            slot.lock().unwrap_or_else(|error| error.into_inner()).push(result);
        });
        Ok(())
    }

    /// Stages the document's top-level properties (JSON, without the tables and pose sources).
    pub fn stage_properties(&self, batch: u32, json: &str) -> Result<(), JsError> {
        self.engine.stage(batch, Change::Properties(json.as_bytes().to_vec())).map_err(js_error)
    }

    /// Stages the order of a table (0 figures, 1 keyframes, 2 backgrounds): for each
    /// position the index its record had before, or -1 for a record staged next.
    pub fn stage_layout(&self, batch: u32, table_code: u32, layout: &[i32]) -> Result<(), JsError> {
        self.engine.stage(batch, Change::Layout(table(table_code)?, layout.to_vec())).map_err(js_error)
    }

    /// Stages one record of a table as lifted JSON and its two byte sections.
    pub fn stage_record(&self, batch: u32, table_code: u32, index: u32, skeleton: &str, arrays: &[u8], payloads: &[u8]) -> Result<(), JsError> {
        let record = Record { skeleton: skeleton.as_bytes().to_vec(), arrays: arrays.to_vec(), payloads: payloads.to_vec() };
        self.engine.stage(batch, Change::Set(table(table_code)?, index, record)).map_err(js_error)
    }

    /// Stages a retained pose source, or its removal when `skeleton` is empty.
    pub fn stage_source(&self, batch: u32, id: &str, skeleton: &str, arrays: &[u8], payloads: &[u8]) -> Result<(), JsError> {
        let record = (!skeleton.is_empty()).then(|| Record { skeleton: skeleton.as_bytes().to_vec(), arrays: arrays.to_vec(), payloads: payloads.to_vec() });
        self.engine.stage(batch, Change::Source(id.to_string(), record)).map_err(js_error)
    }

    /// Commits everything staged in `batch` as the document of `revision`, in the
    /// background. Watch the progress counter, then call [`EngineHandle::take_committed`].
    pub fn commit(&self, revision: f64, batch: u32) {
        let revision = revision as u64;
        let slot = self.committed.clone();
        self.engine.spawn(move |engine| {
            let result = engine.commit(revision, batch).map_err(|e| (revision, e.to_string()));
            slot.lock().unwrap_or_else(|e| e.into_inner()).push(result);
        });
    }

    /// Commits everything staged in `batch` on this thread and returns the outcome as JSON, as
    /// [`EngineHandle::take_committed`] does. For small commits the next frame needs
    /// (figure definitions while editing); documents commit in the background. Never waits
    /// (D7): while a background commit runs the answer is `{revision, busy: true}` and the
    /// batch stays staged for the next call.
    pub fn commit_now(&self, revision: f64, batch: u32) -> String {
        match self.engine.try_commit(revision as u64, batch) {
            Ok(None) => format!("{{\"revision\":{},\"busy\":true}}", revision as u64),
            Ok(Some(done)) => format!(
                "{{\"revision\":{},\"generation\":{},\"figures\":{},\"keyframes\":{},\"ms\":{:.3}}}",
                done.revision, done.generation, done.converted.figures, done.converted.keyframes, done.ms
            ),
            Err(error) => format!("{{\"revision\":{},\"error\":{:?}}}", revision as u64, error.to_string()),
        }
    }

    /// Features of the open document the engine does not draw yet, as a JSON array.
    pub fn unsupported(&self) -> String {
        let features = self.engine.generation().map(|generation| generation.prepared.doc.unsupported.clone()).unwrap_or_default();
        format!("[{}]", features.iter().map(|feature| format!("{feature:?}")).collect::<Vec<_>>().join(","))
    }

    /// The outcome of the oldest finished commit as JSON (`{revision, generation, …}`
    /// or `{revision, error}`), once; `undefined` when none is waiting.
    pub fn take_committed(&self) -> Option<String> {
        let mut slot = self.committed.try_lock().ok()?;
        if slot.is_empty() {
            return None;
        }
        Some(match slot.remove(0) {
            Ok(done) => format!(
                "{{\"revision\":{},\"generation\":{},\"figures\":{},\"keyframes\":{},\"ms\":{:.3}}}",
                done.revision, done.generation, done.converted.figures, done.converted.keyframes, done.ms
            ),
            Err((revision, message)) => format!("{{\"revision\":{revision},\"error\":{message:?}}}"),
        })
    }

    /// The interface revision of the open document, or -1 when there is none.
    pub fn revision(&self) -> f64 {
        self.engine.try_generation_header().ok().flatten().map_or(-1.0, |header| header.revision as f64)
    }

    /// Length of the open document in ticks (0 when there is none).
    pub fn total_ticks(&self) -> u32 {
        self.engine.try_generation_header().ok().flatten().map_or(0, |header| header.total_ticks)
    }

    /// Replaces the ticks wanted in the background, most urgent first.
    pub fn request(&self, playhead: u32, ticks: &[u32]) {
        self.engine.request(playhead, ticks);
    }

    /// Replaces fractional sample requests; the interface transfers Float64Array.
    pub fn request_samples(&self, playhead: f64, ticks: &[f64]) {
        self.engine.request_samples(playhead, ticks);
    }

    pub fn frame_ready(&self, tick: f64) -> bool {
        self.engine.frame_ready_at(tick)
    }

    /// A ready frame belongs to this exact generation and settings/resource epoch.
    pub fn frame_ticket_ready(&self, generation: f64, epoch: f64, tick: f64) -> bool {
        let (Ok(generation), Ok(epoch)) = (record_counter(generation), record_counter(epoch)) else { return false; };
        self.engine.frame_matches(tick, generation, epoch)
    }

    /// What the interface draws around a prepared frame (the camera outline, the
    /// background being shown), as the JSON of a keyframe without objects;
    /// `undefined` while the frame is not ready.
    pub fn frame_meta(&self, tick: f64) -> Option<String> {
        let frame = self.engine.frame_metadata(tick)?;
        let generation = frame.generation;
        let epoch = frame.epoch;
        let camera = match frame.camera {
            Some(c) => format!("{{\"x\":{},\"y\":{},\"scale\":{},\"angle_rad\":{},\"is_default\":{}}}", c.x, c.y, c.scale, c.angle, c.is_default),
            None => "null".to_string(),
        };
        let (a, b, mix) = frame.background;
        Some(format!("{{\"tick\":{tick},\"generation\":{generation},\"epoch\":{epoch},\"camera\":{camera},\"background_id\":{a},\"background_id_b\":{b},\"background_mix\":{mix},\"objects\":[]}}"))
    }

    /// Prepares `tick` on the calling thread if needed and describes its draw list as
    /// JSON (`{prims, groups, passes, ms}`); `undefined` when the tick does not exist.
    /// For tests and tools: the interface never needs a frame's contents.
    pub fn frame_info(&self, tick: u32) -> Option<String> {
        let frame = self.engine.frame_now(tick)?;
        Some(format!("{{\"prims\":{},\"groups\":{},\"passes\":{},\"ms\":{:.3}}}", frame.list.prims.len(), frame.list.groups.len() - 1, frame.list.passes.len(), frame.ms))
    }

    /// A per-object description of an evaluated tick, for comparing engine builds.
    pub fn describe_tick(&self, tick: u32) -> Option<String> {
        let generation = self.engine.generation()?;
        Some(styk_eval::describe_tick(&generation.prepared, tick))
    }

    /// Sets the figures hidden for this session, by identity. Hidden layers come from the document.
    pub fn set_hidden(&self, uids: &[u32]) {
        self.engine.set_hidden(uids);
    }

    /// 0=Busy (unchanged), 1=Unchanged, 2=Applied. UI never spins on State.
    pub fn try_set_hidden(&self, uids: &[u32]) -> u32 { self.engine.try_set_hidden(uids) as u32 }

    pub fn try_request_samples(&self, playhead: f64, ticks: &[f64]) -> u32 {
        self.engine.try_request_samples(playhead, ticks) as u32
    }

    /// Prepares frames with or without effects (the editor's effects preview).
    pub fn set_effects(&self, on: bool) {
        self.engine.set_effects(on);
    }

    pub fn try_set_effects(&self, on: bool) -> u32 { self.engine.try_set_effects(on) as u32 }

    /// The id of the open document generation (0 when there is none). Prepared frames
    /// belong to one generation.
    pub fn generation(&self) -> f64 {
        self.engine.try_generation_header().ok().flatten().map_or(0.0, |header| header.generation as f64)
    }

    /// Runs one unit of background work here. For hosts without worker threads.
    pub fn step(&self) -> bool {
        self.engine.step()
    }

    /// Preparation statistics as JSON.
    pub fn stats(&self) -> String {
        let frames = self.engine.try_frame_stats();
        let images = self.engine.try_images_pending();
        let busy = frames.is_none() || images.is_none();
        let s = frames.unwrap_or_default();
        format!(
            "{{\"prepared\":{},\"totalMs\":{:.3},\"worstMs\":{:.3},\"inline\":{},\"cached\":{},\"cachedBytes\":{},\"imagesPending\":{},\"busy\":{busy}}}",
            s.prepared,
            s.total_ms,
            s.worst_ms,
            s.inline,
            s.cached,
            s.cached_bytes,
            images.unwrap_or_default()
        )
    }

    /// Asks every worker to leave [`worker_run`].
    pub fn stop(&self) {
        self.engine.stop();
    }
}

/// A worker thread's whole life: prepares frames until the engine stops.
/// `address` comes from [`EngineHandle::address`] and is consumed.
#[wasm_bindgen]
pub fn worker_run(address: usize) {
    // SAFETY: `address` was produced by `EngineHandle::address` in this memory and is used once.
    let engine = unsafe { Box::from_raw(address as *mut Engine) };
    engine.work();
}

/// Wakes `Atomics.waitAsync` waiters on the progress counter. Runs on whichever
/// thread finished the work.
fn notify_progress(counter: &std::sync::atomic::AtomicU32) {
    thread_local! {
        /// This thread's view of the shared memory (a view cannot cross threads).
        static MEMORY: std::cell::RefCell<Option<js_sys::Int32Array>> = const { std::cell::RefCell::new(None) };
    }
    let index = counter.as_ptr() as u32 / 4;
    MEMORY.with(|cell| {
        let mut view = cell.borrow_mut();
        // A grown memory has a new buffer; the old view no longer covers it.
        if view.as_ref().is_none_or(|view| view.length() <= index) {
            let memory: js_sys::WebAssembly::Memory = wasm_bindgen::memory().unchecked_into();
            *view = Some(js_sys::Int32Array::new(&memory.buffer()));
        }
        if let Some(view) = view.as_ref() {
            let _ = js_sys::Atomics::notify(view, index);
        }
    });
}
