//! The engine as JavaScript sees it.

use std::sync::{Arc, Mutex};

use styk_engine::{Budgets, Engine, Opened};
use wasm_bindgen::prelude::*;

use crate::{clock, js_error};

/// The engine. Create one, give its [`EngineHandle::address`] to each worker.
#[wasm_bindgen]
pub struct EngineHandle {
    pub(crate) engine: Engine,
    /// The outcome of the last [`EngineHandle::open`], until JavaScript takes it.
    opened: Arc<Mutex<Option<Result<Opened, String>>>>,
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
        EngineHandle { engine: Engine::new(clock, Budgets::for_device(device_memory_gib)), opened: Default::default() }
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

    /// Replaces the ticks wanted in the background, most urgent first.
    pub fn request(&self, playhead: u32, ticks: &[u32]) {
        self.engine.request(playhead, ticks);
    }

    pub fn frame_ready(&self, tick: u32) -> bool {
        self.engine.frame(tick).is_some()
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
