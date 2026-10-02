//! The engine as JavaScript sees it.

use std::sync::{Arc, Mutex};

use styk_engine::mirror::{Change, Committed, Record, Table};
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
    pub fn stage_reset(&self, batch: u32) {
        self.engine.stage(batch, Change::Reset);
    }

    /// Stages the document's top-level properties (JSON, without the tables and pose sources).
    pub fn stage_properties(&self, batch: u32, json: &str) {
        self.engine.stage(batch, Change::Properties(json.as_bytes().to_vec()));
    }

    /// Stages the order of a table (0 figures, 1 keyframes, 2 backgrounds): for each
    /// position the index its record had before, or -1 for a record staged next.
    pub fn stage_layout(&self, batch: u32, table_code: u32, layout: &[i32]) -> Result<(), JsError> {
        self.engine.stage(batch, Change::Layout(table(table_code)?, layout.to_vec()));
        Ok(())
    }

    /// Stages one record of a table as lifted JSON and its two byte sections.
    pub fn stage_record(&self, batch: u32, table_code: u32, index: u32, skeleton: &str, arrays: &[u8], payloads: &[u8]) -> Result<(), JsError> {
        let record = Record { skeleton: skeleton.as_bytes().to_vec(), arrays: arrays.to_vec(), payloads: payloads.to_vec() };
        self.engine.stage(batch, Change::Set(table(table_code)?, index, record));
        Ok(())
    }

    /// Stages a retained pose source, or its removal when `skeleton` is empty.
    pub fn stage_source(&self, batch: u32, id: &str, skeleton: &str, arrays: &[u8], payloads: &[u8]) {
        let record = (!skeleton.is_empty()).then(|| Record { skeleton: skeleton.as_bytes().to_vec(), arrays: arrays.to_vec(), payloads: payloads.to_vec() });
        self.engine.stage(batch, Change::Source(id.to_string(), record));
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
    /// (figure definitions while editing); documents commit in the background.
    pub fn commit_now(&self, revision: f64, batch: u32) -> String {
        match self.engine.commit(revision as u64, batch) {
            Ok(done) => format!(
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
        self.engine.generation().map_or(-1.0, |generation| generation.revision as f64)
    }

    /// Length of the open document in ticks (0 when there is none).
    pub fn total_ticks(&self) -> u32 {
        self.engine.generation().map_or(0, |generation| generation.playable_ticks())
    }

    /// Replaces the ticks wanted in the background, most urgent first.
    pub fn request(&self, playhead: u32, ticks: &[u32]) {
        self.engine.request(playhead, ticks);
    }

    pub fn frame_ready(&self, tick: u32) -> bool {
        self.engine.frame(tick).is_some()
    }

    /// What the interface draws around a prepared frame (the camera outline, the
    /// background being shown), as the JSON of a keyframe without objects;
    /// `undefined` while the frame is not ready.
    pub fn frame_meta(&self, tick: u32) -> Option<String> {
        let frame = self.engine.frame(tick)?;
        let camera = match frame.camera {
            Some(c) => format!("{{\"x\":{},\"y\":{},\"scale\":{},\"angle_rad\":{},\"is_default\":{}}}", c.x, c.y, c.scale, c.angle, c.is_default),
            None => "null".to_string(),
        };
        let (a, b, mix) = frame.background;
        Some(format!("{{\"tick\":{tick},\"camera\":{camera},\"background_id\":{a},\"background_id_b\":{b},\"background_mix\":{mix},\"objects\":[]}}"))
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

    /// The id of the open document generation (0 when there is none). Prepared frames
    /// belong to one generation.
    pub fn generation(&self) -> f64 {
        self.engine.generation().map_or(0.0, |generation| generation.id as f64)
    }

    /// Runs one unit of background work here. For hosts without worker threads.
    pub fn step(&self) -> bool {
        self.engine.step()
    }

    /// Preparation statistics as JSON.
    pub fn stats(&self) -> String {
        let s = self.engine.frame_stats();
        format!(
            "{{\"prepared\":{},\"totalMs\":{:.3},\"worstMs\":{:.3},\"inline\":{},\"cached\":{},\"cachedBytes\":{},\"imagesPending\":{}}}",
            s.prepared,
            s.total_ms,
            s.worst_ms,
            s.inline,
            s.cached,
            s.cached_bytes,
            self.engine.images_pending()
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
