//! Editing calls as JavaScript sees them (docs/engine/editing.md).

use wasm_bindgen::prelude::*;

use crate::engine::EngineHandle;
use crate::js_error;

/// The editor's calls run on the document the engine mirrors. Lives on the interface
/// thread; a call is plain computation on the session's own copy-on-write document
/// and takes no lock.
#[wasm_bindgen]
pub struct EditSession {
    session: styk_edit::session::Session,
    revision: f64,
}

#[wasm_bindgen]
impl EngineHandle {
    /// Starts an edit session on the mirrored document with the editor's `state`
    /// (JSON: options, view and counters). `undefined` while the mirror is committing
    /// or empty: this never waits.
    pub fn edit_session(&self, state: &str) -> Result<Option<EditSession>, JsError> {
        // The mirror's typed views of this revision: the session converts nothing to start.
        let Some((revision, tree, loader, prepared)) = self.engine.mirrored_for_editing() else { return Ok(None) };
        let session = styk_edit::session::Session::start_prepared(tree, state, loader, prepared).map_err(js_error)?;
        Ok(Some(EditSession { session, revision: revision as f64 }))
    }
}

#[wasm_bindgen]
impl EditSession {
    /// Starts a session on `document` (its JSON, exactly as the editor holds it) instead
    /// of the mirrored one: the session converts what it evaluates itself (D19).
    pub fn from_document(document: &str, state: &str, revision: f64) -> Result<EditSession, JsError> {
        let tree = styk_doc::json::parse(document.as_bytes()).map_err(js_error)?;
        let session = styk_edit::session::Session::start(tree, state).map_err(js_error)?;
        Ok(EditSession { session, revision })
    }

    /// The session's document as JSON (tests and recovery; never on an interactive path).
    pub fn report_json(&self) -> String {
        styk_doc::json::to_string(self.session.report())
    }

    /// The interface revision of the document the session started from.
    pub fn revision(&self) -> f64 {
        self.revision
    }

    pub fn can_undo(&self) -> bool {
        self.session.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.session.can_redo()
    }

    /// Runs one editor call: its JavaScript name, its arguments as a JSON array and
    /// the editor's view before it (JSON). Answers with JSON: status, result, state,
    /// events, the gesture's draft and what a command published. With `changesOnly: true`
    /// in the view, unchanged COW figures and sheet fields are omitted (the app's owned
    /// adoption path); default responses stay complete for shadow comparison.
    pub fn call(&mut self, method: &str, args: &str, view: &str) -> String {
        self.session.call(method, args, view)
    }
}
