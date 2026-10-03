//! An edit session as the interface drives it: calls in, JSON out.
//!
//! The session starts from the document the engine already holds (the mirror) and the
//! editor state the document does not carry, runs the editor's calls by name, and
//! answers with everything the JavaScript editor would show for the same call: the
//! returned value, the playhead and selection, the change events, the gesture's draft,
//! and the parts of the document a command published. `src/engine/engine-edit-shadow.mjs`
//! compares that with what the JavaScript editor did (docs/engine/editing.md).

use styk_doc::{Obj, Value};

use crate::{EditError, Editor, Options, Selection};

pub struct Session {
    editor: Editor,
}

fn number(value: &Value, key: &str) -> Option<f64> {
    value.get(key).and_then(Value::as_f64)
}

fn flag(value: &Value, key: &str) -> bool {
    value.get(key).is_some_and(Value::truthy)
}

fn selection_of(view: &Value) -> Selection {
    Selection {
        uid: number(view, "selectedUid").map(|v| v as u32),
        uids: view.get("selectedUids").and_then(Value::numbers).unwrap_or_default().into_iter().map(|v| v as u32).collect(),
        joint: number(view, "selectedJoint").unwrap_or(-1.0) as i64,
    }
}

impl Session {
    /// `state` is JSON: the editor's options (`propertyChannels`, `layerTimeline`,
    /// `autoKey`), view (`tick`, `frameIndex`, `selectedUid`, `selectedUids`,
    /// `selectedJoint`, `activeLayerId`) and counters (`layerRevision`, `nextFrameIdentity`, `nextUid`).
    pub fn start(report: Value, state: &str) -> Result<Session, String> {
        let state = styk_doc::json::parse(state.as_bytes()).map_err(|error| error.to_string())?;
        let options = Options { property_channels: flag(&state, "propertyChannels"), layer_timeline: flag(&state, "layerTimeline"), auto_key: flag(&state, "autoKey") };
        let mut editor = Editor::new(report, options);
        editor.set_view(number(&state, "tick").unwrap_or(0.0), number(&state, "frameIndex").unwrap_or(0.0) as usize, selection_of(&state));
        let counter = |key: &str| number(&state, key).unwrap_or(0.0) as u64;
        editor.set_counters(counter("layerRevision"), counter("nextFrameIdentity"), counter("nextUid"));
        if let Some(active) = state.get("activeLayerId").and_then(Value::as_str) {
            editor.set_active_layer_id(active);
        }
        Ok(Session { editor })
    }

    /// Whether the session's own history can undo (it only knows the commands it ran).
    pub fn can_undo(&self) -> bool {
        self.editor.state().can_undo
    }

    pub fn can_redo(&self) -> bool {
        self.editor.state().can_redo
    }

    /// Runs `method` with the JSON array `args`. `view` (JSON, or `null`) is the
    /// editor's view just before the call: selection and seeking may have gone through
    /// calls the session did not see. Returns the answer as JSON (see the module).
    pub fn call(&mut self, method: &str, args: &str, view: &str) -> String {
        let before = self.editor.report().clone();
        let retained = self.editor.retained_frame();
        let mut answer = Obj::new();
        let parsed = (styk_doc::json::parse(args.as_bytes()), styk_doc::json::parse(view.as_bytes()));
        let (Ok(args), Ok(view)) = parsed else {
            answer.set("status", Value::str("error"));
            answer.set("error", Value::str("the call's arguments are not JSON"));
            return styk_doc::json::to_string(&Value::object(answer));
        };
        if view.is_object() {
            self.editor.set_view(number(&view, "tick").unwrap_or(0.0), number(&view, "frameIndex").unwrap_or(0.0) as usize, selection_of(&view));
            self.editor.set_auto_key(flag(&view, "autoKey"));
            // Selecting changes the active layer without a new revision of the document.
            if let Some(active) = view.get("activeLayerId").and_then(Value::as_str) {
                self.editor.set_active_layer_id(active);
            }
        }
        let args: Vec<Value> = args.elements().map(|e| e.iter().collect()).unwrap_or_default();
        match self.editor.call(method, &args) {
            Ok(result) => {
                answer.set("status", Value::str("ok"));
                answer.set("result", result);
            }
            Err(EditError::Unported(what)) => {
                answer.set("status", Value::str("unported"));
                answer.set("reason", Value::str(&what));
            }
            Err(EditError::Invalid(key)) => {
                answer.set("status", Value::str("error"));
                answer.set("error", Value::str(&key));
            }
        }
        answer.set("state", self.editor.state_value());
        let events = self.editor.take_events();
        let report = self.editor.report();
        let sheet = |document: &Value| document.path(&["_styk", "layerSheet"]).cloned();
        let published = match (sheet(&before), sheet(report)) {
            (Some(old), Some(new)) => !old.same_node(&new),
            (old, new) => old.is_some() != new.is_some(),
        };
        if published {
            // What a command publishes: the sheet (with the tracks it replaced; `trackOrder`
            // names them all), the layers and the sources its events name.
            let styk = report.get("_styk");
            let tracks = |document: &Value| document.path(&["_styk", "layerSheet", "tracks"]).and_then(Value::as_obj).cloned().unwrap_or_default();
            let (old, new) = (tracks(&before), tracks(report));
            let changed: Vec<&std::sync::Arc<str>> = new.iter().filter(|(id, track)| !old.get(id).is_some_and(|previous| previous.same_node(track))).map(|(id, _)| id).collect();
            let mut sheet = styk.and_then(|s| s.get("layerSheet")).and_then(Value::as_obj).cloned().unwrap_or_default();
            let mut replaced = Obj::new();
            for id in &changed {
                replaced.set(id, new.get(id).cloned().unwrap_or(Value::Null));
            }
            sheet.set("tracks", Value::object(replaced));
            answer.set("sheet", Value::object(sheet));
            answer.set("trackOrder", Value::array(new.keys().map(|id| Value::str(id)).collect()));
            answer.set("changedTracks", Value::array(changed.iter().map(|id| Value::str(id)).collect()));
            answer.set("layers", styk.and_then(|s| s.get("layers")).cloned().unwrap_or(Value::Null));
            let mut sources = Obj::new();
            for id in events.iter().flat_map(|event| event.get("sourceIds").and_then(Value::elements).map(|e| e.iter().collect::<Vec<_>>()).unwrap_or_default()) {
                if let Some(id) = id.as_str() {
                    sources.set(id, styk.and_then(|s| s.path(&["frameSources", id])).cloned().unwrap_or(Value::Null));
                }
            }
            answer.set("sources", Value::object(sources));
            // A committed gesture's draft stays on show (`_authoredCommitPreview`), with what
            // the commit itself changed in it (joined figures' roots from geometry): its figures.
            if let Some(frame) = self.editor.retained_frame().filter(|frame| !retained.as_ref().is_some_and(|old| old.same_node(frame))) {
                answer.set("retained", frame.get("objects").cloned().unwrap_or(Value::Null));
            }
        }
        answer.set("events", Value::array(events));
        answer.set("draft", self.editor.draft_figures().map_or(Value::Null, Value::array));
        answer.set("drag", self.editor.drag_value());
        styk_doc::json::to_string(&Value::object(answer))
    }
}
