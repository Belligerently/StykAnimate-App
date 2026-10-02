//! The engine's WebAssembly boundary.
//!
//! Everything JavaScript can call is declared in this crate and nowhere else.
//! `src/engine/` is its only caller on the JavaScript side.

use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console, js_name = error)]
    fn console_error(message: &str);
}

/// Runs once per instance (main thread and every worker): engine panics reach the
/// console with their message and location before the module aborts.
#[wasm_bindgen(start)]
fn start() {
    std::panic::set_hook(Box::new(|info| console_error(&format!("StykAnimate engine panic: {info}"))));
}

/// Version of the command/query API. The JavaScript loader refuses a module
/// whose number differs from the one it was written for.
#[wasm_bindgen]
pub fn api_version() -> u32 {
    1
}

/// Opens a native file and reports what it holds, as JSON. A first end-to-end
/// path through formats, the document store and the typed views.
#[wasm_bindgen]
pub fn describe_file(bytes: &[u8], extension: &str, name: &str) -> Result<String, JsError> {
    let started = js_sys::Date::now();
    let document = styk_eval::open_bytes(bytes, extension, name).map_err(|e| JsError::new(&e.to_string()))?;
    let prepared = styk_eval::prepare(document);
    Ok(format!(
        "{{\"name\":{:?},\"version\":{},\"canvas\":[{},{}],\"fps\":{},\"ticks\":{},\"figures\":{},\"keyframes\":{},\"openMs\":{}}}",
        prepared.doc.name,
        prepared.doc.version,
        prepared.doc.canvas[0],
        prepared.doc.canvas[1],
        prepared.doc.fps,
        prepared.total_ticks,
        prepared.doc.figures.len(),
        prepared.doc.keyframes.len(),
        js_sys::Date::now() - started
    ))
}
