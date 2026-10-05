//! An edit session as the interface drives it: calls in, JSON out.
//!
//! The session starts from the document the engine already holds (the mirror) and the
//! editor state the document does not carry, runs the editor's calls by name, and
//! answers with everything the JavaScript editor would show for the same call: the
//! returned value, the playhead and selection, the change events, the gesture's draft,
//! and the parts of the document a command published. `src/engine/engine-edit-shadow.mjs`
//! compares that with what the JavaScript editor did (docs/engine/editing.md).

use std::collections::HashSet;
use std::sync::Arc;

use styk_doc::{Obj, Value};

use crate::{EditError, Editor, Options, Selection};

pub struct Session {
    editor: Editor,
}

mod worker;
pub use worker::{WorkerFrame, WorkerPrepared, WorkerSession};

/// Exact counters supplied by the existing history owner during worker handoff.
#[derive(Clone, Copy, Debug)]
pub struct RevisionCounters {
    pub revision: u64,
    pub serial: u64,
    pub saved: u64,
}

fn number(value: &Value, key: &str) -> Option<f64> {
    value.get(key).and_then(Value::as_f64)
}

fn flag(value: &Value, key: &str) -> bool {
    value.get(key).is_some_and(Value::truthy)
}

/// `sheetSelection` of a view: the layer chosen on the timeline and whether it follows seeks.
fn sheet_selection_of(view: &Value) -> Option<(&str, bool)> {
    let chosen = view.get("sheetSelection").filter(|s| s.is_object())?;
    Some((chosen.get("id").and_then(Value::as_str)?, chosen.get("contents").is_some_and(Value::truthy)))
}

fn selection_of(view: &Value) -> Selection {
    Selection {
        uid: number(view, "selectedUid").map(|v| v as u32),
        uids: view.get("selectedUids").and_then(Value::numbers).unwrap_or_default().into_iter().map(|v| v as u32).collect(),
        joint: number(view, "selectedJoint").unwrap_or(-1.0) as i64,
    }
}

/// These response fields have unique, nonnumeric keys, appended in the same
/// order as the JSON owner emits them. Build their existing ordered shape once
/// rather than replacing the shape after each missing-key `Obj::set`.
fn response_value(fields: Vec<(&'static str, Value)>) -> Value {
    let (keys, values): (Vec<Arc<str>>, Vec<Value>) = fields.into_iter().map(|(key, value)| (Arc::from(key), value)).unzip();
    Value::object(Obj::from_shape(styk_doc::Shape::new(keys), values))
}

/// The COW figures the call replaced, without evaluation's runtime movieclip clock.
/// A draft keeps its slots; a retained frame can also add, remove or reorder figures.
fn changed_figures(frame: &Value, previous: Option<&Value>, taken: Option<&HashSet<u32>>) -> Vec<Value> {
    let objects = frame.get("objects").and_then(Value::elements);
    let before = previous.and_then(|frame| frame.get("objects")).and_then(Value::elements);
    objects.map(|objects| objects.iter().enumerate().filter_map(|(slot, figure)| {
        let uid = crate::record::uid(&figure)?;
        if taken.is_some_and(|taken| !taken.contains(&uid)) {
            return None;
        }
        if before.and_then(|objects| objects.get(slot)).is_some_and(|old| crate::record::uid(&old) == Some(uid) && old.same_node(&figure)) {
            return None;
        }
        // Ordinary pose drafts already are immutable COW records. Reuse their
        // values without copying every field just to remove an absent runtime
        // movieclip clock; the serialized figure remains identical.
        if figure.get("_movieclipSample").is_none() {
            Some(figure)
        } else {
            let mut figure = figure.as_obj().cloned().unwrap_or_default();
            figure.remove("_movieclipSample");
            Some(Value::object(figure))
        }
    }).collect()).unwrap_or_default()
}

fn figure_order(frame: &Value) -> Vec<Value> {
    frame.get("objects").and_then(Value::elements).map(|objects| objects.iter().filter_map(|figure| crate::record::uid(&figure)).map(|uid| Value::Num(uid as f64)).collect()).unwrap_or_default()
}

impl Session {
    /// `state` is JSON: the editor's options (`propertyChannels`, `layerTimeline`,
    /// `autoKey`), view (`tick`, `frameIndex`, `selectedUid`, `selectedUids`,
    /// `selectedJoint`, `activeLayerId`) and counters (`layerRevision`, `nextFrameIdentity`, `nextUid`).
    /// [`Session::start`] from typed views already made for `report` (the engine's mirror).
    pub fn start_prepared(report: Value, state: &str, loader: styk_eval::load::Loader, prepared: std::sync::Arc<styk_eval::eval::Prepared>) -> Result<Session, String> {
        Self::start_with(report, state, Some((loader, prepared)))
    }

    /// Worker-only initialization from an actually fresh, empty-history owner.
    /// The host must refuse foreign warm history, not rebuild it from the report.
    /// Existing Rust warm Sessions move as-is through Engine::attach_session_worker.
    /// Counters come from that original owner; never infer them from the mirror.
    pub fn start_worker_prepared(report: Value, state: &str, loader: styk_eval::load::Loader, prepared: std::sync::Arc<styk_eval::eval::Prepared>, counters: RevisionCounters) -> Result<Session, String> {
        if counters.revision > counters.serial || counters.saved > counters.serial {
            return Err("the original history revision counters are invalid".into());
        }
        let mut session = Self::start_prepared(report, state, loader, prepared)?;
        session.editor.set_revision_counters(counters.revision, counters.serial, counters.saved);
        Ok(session)
    }

    pub fn start(report: Value, state: &str) -> Result<Session, String> {
        Self::start_with(report, state, None)
    }

    fn start_with(report: Value, state: &str, prepared: Option<(styk_eval::load::Loader, std::sync::Arc<styk_eval::eval::Prepared>)>) -> Result<Session, String> {
        let state = styk_doc::json::parse(state.as_bytes()).map_err(|error| error.to_string())?;
        let options = Options { property_channels: flag(&state, "propertyChannels"), layer_timeline: flag(&state, "layerTimeline"), auto_key: flag(&state, "autoKey") };
        let mut editor = Editor::new(report, options);
        editor.set_view(number(&state, "tick").unwrap_or(0.0), number(&state, "frameIndex").unwrap_or(0.0) as usize, selection_of(&state));
        // A metadata restore may inspect stable handles of the figure on show:
        // the interface session must use the mirror's already prepared owner.
        if let Some((loader, prepared)) = prepared { editor.adopt_prepared(loader, prepared); editor.worker_prepared_only = true; }
        editor.set_selection_metadata(&state).map_err(|error| format!("{error:?}"))?;
        let counter = |key: &str| number(&state, key).unwrap_or(0.0) as u64;
        editor.set_counters(counter("layerRevision"), counter("nextFrameIdentity"), counter("nextUid"));
        if let Some(active) = state.get("activeLayerId").and_then(Value::as_str) {
            editor.set_active_layer_id(active);
        }
        editor.set_sheet_selection(sheet_selection_of(&state));
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
    /// calls the session did not see. With `changesOnly: true`, draft/retained figures
    /// and sheet fields carry only changed COW records; optional order/removal fields
    /// reconstruct their previous layout. Results the facade returns from its adopted
    /// document are `null`. Default answers remain complete for shadow comparison.
    /// Returns the answer as JSON (see the module).
    pub fn call(&mut self, method: &str, args: &str, view: &str) -> String {
        let parsed = (styk_doc::json::parse(args.as_bytes()), styk_doc::json::parse(view.as_bytes()));
        let (Ok(args), Ok(view)) = parsed else {
            let mut answer = Vec::with_capacity(8);
            answer.push(("status", Value::str("error")));
            answer.push(("error", Value::str("the call's arguments are not JSON")));
            return styk_doc::json::to_string(&response_value(answer));
        };
        styk_doc::json::to_string(&self.call_values(method, &args, &view))
    }

    /// The same command and response owner over already decoded COW values.
    /// This worker-only boundary avoids whole-answer JSON serialization: its
    /// returned value stays worker-owned until the common bounded packet ACK.
    /// Arguments and view must also be decoded/admitted on that worker; this
    /// method does not enable an owning editor on both UI and worker.
    pub fn call_values(&mut self, method: &str, args: &Value, view: &Value) -> Value {
        call_values(&mut self.editor, method, args, view, false, None).expect("legacy response has no host cancellation callback")
    }

    /// Borrow the canonical COW report on its owning worker. The host pairs this
    /// with the acknowledged Engine generation/lineage; no report is serialized.
    pub fn report(&self) -> &Value {
        self.editor.report()
    }

    /// Actual owning history revision, for worker handoff validation.
    pub fn revision(&self) -> u64 { self.editor.revision }

    /// Scalar checkpoint of the exact prepared owner, never a reconstructed sum.
    pub fn history_checkpoint_worker(&self) -> Option<f64> { self.editor.history_size() }

    /// Prepare the existing exact history owner before accepting worker calls.
    /// A warm session uses its original owner checkpoint, never a reconstructed sum.
    pub fn prepare_worker_history(
        &mut self,
        limits: crate::HistoryLimits,
        checkpoint: f64,
        check: impl FnMut() -> Result<(), EditError>,
    ) -> Result<(), EditError> {
        self.editor.prepare_worker_history(limits, checkpoint, check)
    }

    /// Immutable preparation using the SAME Session/editor/response owner.
    /// This retains the existing quiet-editor requirement; gesture/preflight
    /// requests require their separately recorded worker boundary.
    pub fn prepare_worker_call_values(
        &mut self,
        method: &str,
        args: &Value,
        view: &Value,
        check: impl FnMut() -> Result<(), EditError>,
    ) -> Result<crate::publication::PreparedPublication, EditError> {
        self.editor.prepare_worker_publication(
            |editor, check| {
                check()?;
                let answer = call_values(editor, method, args, view, true, Some(&mut *check))?;
                check()?;
                Ok(answer)
            },
            check,
        )
    }

    /// Worker-only final validation before the Engine's atomic publication hook.
    /// Refusal returns the immutable payload for durable worker disposal.
    pub fn guard_worker_publication_checked<'a>(
        &'a mut self,
        prepared: crate::publication::PreparedPublication,
        check: impl FnMut() -> Result<(), EditError>,
    ) -> Result<crate::publication::GuardedPublication<'a>, (EditError, crate::publication::PreparedPublication)> {
        prepared.guard_worker_checked(&mut self.editor, check)
    }
}

/// Shared by legacy JSON calls and canonical worker transactions.
fn call_values(editor: &mut Editor, method: &str, args: &Value, view: &Value, worker_preparation: bool, check: Option<&mut dyn FnMut() -> Result<(), EditError>>) -> Result<Value, EditError> {
    let before = editor.report().clone();
    let retained = editor.retained_frame();
    let draft = editor.authored.as_ref().map(|edit| edit.frame.clone());
    let mut answer = Vec::with_capacity(8);
    // Owned calls request only changed COW records. The default remains complete for
    // shadow comparison and existing native session consumers.
    let changes_only = flag(view, "changesOnly");
    if changes_only {
        answer.push(("changesOnly", Value::Bool(true)));
    }
    if view.is_object() {
        editor.set_view(number(view, "tick").unwrap_or(0.0), number(view, "frameIndex").unwrap_or(0.0) as usize, selection_of(view));
        if let Err(error) = editor.sync_selection_metadata(view) {
            answer.push(("status", Value::str("unported")));
            answer.push(("reason", Value::str(&format!("{error:?}"))));
            return Ok(response_value(answer));
        }
        editor.set_auto_key(flag(view, "autoKey"));
        // Selecting changes the active layer without a new revision of the document.
        if let Some(active) = view.get("activeLayerId").and_then(Value::as_str) {
            editor.set_active_layer_id(active);
        }
        editor.set_sheet_selection(sheet_selection_of(view));
    }
    // What the editor drew at random during its own run of the call (D18).
    editor.set_draws(view.get("draws").unwrap_or(&Value::Null));
    let args: Vec<Value> = args.elements().map(|e| e.iter().collect()).unwrap_or_default();
    // Native movieclip insertion is admitted only by immutable worker preparation.
    // Legacy/live calls retain the same response owner and explicit refusal.
    let result = if !worker_preparation && !editor.sheet() && (method == "addFigure" || crate::movieclip_native::native_movieclip_method(method)) {
        Err(EditError::Unported(if method == "addFigure" { "native movieclip insertion requires worker preparation" } else { "native movieclip editing requires worker preparation" }.into()))
    } else {
        let result = if let Some(check) = check {
            // A recorded Invalid belongs in the canonical JSON response. Host
            // cancellation must instead escape the same preparation operation,
            // including a one-shot failure that later checks would not repeat.
            // This stack-local error retains no callback or authoritative state.
            let mut interrupted = None;
            let mut checked = || match check() {
                Ok(()) => Ok(()),
                Err(error) => {
                    let forwarded = match &error {
                        EditError::Invalid(message) => EditError::Invalid(message.clone()),
                        EditError::Unported(message) => EditError::Unported(message.clone()),
                    };
                    interrupted = Some(error);
                    Err(forwarded)
                }
            };
            let result = editor.call_worker_checked(method, &Value::array(args), &mut checked);
            if let Some(error) = interrupted { return Err(error); }
            result
        } else {
            editor.call(method, &args)
        };
        result.and_then(|result| {
            // Legacy facades read these views locally. The exclusive worker has
            // the sole owner, so it returns the recorded view from that owner.
            if worker_preparation && matches!(method, "seek" | "@seek") { editor.shown_frame() }
            else if worker_preparation && method == "beginAnimationEdit" { editor.animation_edit_value_worker() }
            else { Ok(result) }
        })
    };
    match result {
        Ok(result) => {
            answer.push(("status", Value::str("ok")));
            // The facade returns these views from its adopted document. Repeating
            // their complete figure/frame data in the answer serves no purpose.
            let adopted_view = changes_only && matches!(method, "duplicateFigure" | "addFigure" | "importFigure" | "pasteSelection" | "duplicateSelection" | "sheetAppendFrame");
            answer.push(("result", if adopted_view { Value::Null } else { result }));
        }
        Err(EditError::Unported(what)) => {
            answer.push(("status", Value::str("unported")));
            answer.push(("reason", Value::str(&what)));
        }
        Err(EditError::Invalid(key)) => {
            answer.push(("status", Value::str("error")));
            answer.push(("error", Value::str(&key)));
        }
    }
    answer.push(("state", editor.state_value()));
    let events = editor.take_events();
    let report = editor.report();
    // A command published the sheet when one of its events says so, as the facade reads
    // it: layer edits also replace the sheet (its tracks follow the layers) without it.
    let published = events.iter().any(|event| event.get("layerSheet").is_some_and(Value::truthy));
    if published {
        // What a command publishes: the sheet (with the tracks it replaced; `trackOrder`
        // names them all), the layers and the sources its events name.
        let styk = report.get("_styk");
        let tracks = |document: &Value| document.path(&["_styk", "layerSheet", "tracks"]).and_then(Value::as_obj).cloned().unwrap_or_default();
        let (old, new) = (tracks(&before), tracks(report));
        let changed: Vec<&std::sync::Arc<str>> = new.iter().filter(|(id, track)| !old.get(id).is_some_and(|previous| previous.same_node(track))).map(|(id, _)| id).collect();
        let current_sheet = styk.and_then(|s| s.get("layerSheet")).and_then(Value::as_obj).cloned().unwrap_or_default();
        let old_sheet = before.path(&["_styk", "layerSheet"]).and_then(Value::as_obj);
        if changes_only && !old_sheet.into_iter().flat_map(|sheet| sheet.keys()).eq(current_sheet.keys()) {
            answer.push(("sheetOrder", Value::array(current_sheet.keys().map(|key| Value::str(key)).collect())));
        }
        let mut sheet = if changes_only {
            let mut delta = Obj::new();
            for (key, value) in current_sheet.iter().filter(|(key, _)| &***key != "tracks") {
                if !old_sheet.and_then(|sheet| sheet.get(key)).is_some_and(|old| old.same_node(value)) {
                    delta.set(key, value.clone());
                }
            }
            let removed: Vec<Value> = old_sheet.into_iter().flat_map(|sheet| sheet.keys()).filter(|key| &***key != "tracks" && !current_sheet.contains(key)).map(|key| Value::str(key)).collect();
            if !removed.is_empty() {
                answer.push(("sheetRemoved", Value::array(removed)));
            }
            delta
        } else { current_sheet };
        let mut replaced = Obj::new();
        for id in &changed {
            replaced.set(id, new.get(id).cloned().unwrap_or(Value::Null));
        }
        sheet.set("tracks", Value::object(replaced));
        answer.push(("sheet", Value::object(sheet)));
        if !changes_only || !old.keys().eq(new.keys()) {
            answer.push(("trackOrder", Value::array(new.keys().map(|id| Value::str(id)).collect())));
        }
        if !changes_only {
            answer.push(("changedTracks", Value::array(changed.iter().map(|id| Value::str(id)).collect())));
        }
        let layers = styk.and_then(|s| s.get("layers")).cloned().unwrap_or(Value::Null);
        if !changes_only || !before.path(&["_styk", "layers"]).is_some_and(|old| old.same_node(&layers)) {
            answer.push(("layers", layers));
        }
        let mut sources = Obj::new();
        for id in events.iter().flat_map(|event| event.get("sourceIds").and_then(Value::elements).map(|e| e.iter().collect::<Vec<_>>()).unwrap_or_default()) {
            if let Some(id) = id.as_str() {
                sources.set(id, styk.and_then(|s| s.path(&["frameSources", id])).cloned().unwrap_or(Value::Null));
            }
        }
        answer.push(("sources", Value::object(sources)));
        // Figure definitions the publication added (a paste's copies), after the others.
        let count = |document: &Value| document.get("figure_types").and_then(Value::array_len).unwrap_or(0);
        if count(report) > count(&before) {
            let added: Vec<Value> = report.get("figure_types").and_then(Value::elements).map(|e| e.iter().skip(count(&before)).collect()).unwrap_or_default();
            answer.push(("definitions", Value::array(added)));
        }
        // A committed gesture's draft stays on show (`_authoredCommitPreview`), with what
        // the commit itself changed in it (joined figures' roots from geometry): its figures.
        if let Some(frame) = editor.retained_frame().filter(|frame| !retained.as_ref().is_some_and(|old| old.same_node(frame))) {
            if changes_only {
                let previous = draft.as_ref().or(retained.as_ref());
                answer.push(("retained", Value::array(changed_figures(&frame, previous, None))));
                if let Some(previous) = previous {
                    let order = figure_order(&frame);
                    if order != figure_order(previous) {
                        answer.push(("retainedOrder", Value::array(order)));
                    }
                    answer.push(("retainedChanges", Value::Bool(true)));
                }
            } else {
                answer.push(("retained", frame.get("objects").cloned().unwrap_or(Value::Null)));
            }
            // A transaction can change frame-level order, counts or other fields as
            // well as figures. Keeping only its objects would leave the displayed
            // frame's metadata at its pre-command value.
            let previous = draft.as_ref().or(retained.as_ref()).and_then(Value::as_obj);
            let current = frame.as_obj().cloned().unwrap_or_default();
            let mut fields = Obj::new();
            for (key, value) in current.iter().filter(|(key, _)| &***key != "objects") {
                if !previous.and_then(|frame| frame.get(key)).is_some_and(|old| old.same_node(value)) {
                    fields.set(key, value.clone());
                }
            }
            if !fields.is_empty() { answer.push(("retainedFields", Value::object(fields))); }
            let removed: Vec<Value> = previous.into_iter().flat_map(|frame| frame.keys()).filter(|key| &***key != "objects" && !current.contains(key)).map(|key| Value::str(key)).collect();
            if !removed.is_empty() { answer.push(("retainedRemoved", Value::array(removed))); }
            if !previous.into_iter().flat_map(|frame| frame.keys()).eq(current.keys()) {
                answer.push(("retainedFieldOrder", Value::array(current.keys().map(|key| Value::str(key)).collect())));
            }
        }
    }
    // What a property edit shows while the inspector scrubs, or a camera drag while it moves
    // (`_sheetPropertyPreview`), for the app to compare: the frame, without the clip sample
    // times evaluation adds.
    if (method == "previewAnimationEdit" || method == "updateCameraDrag")
        && let Some(preview) = editor.property_preview()
    {
        let mut frame = preview.as_obj().cloned().unwrap_or_default();
        let objects: Vec<Value> = frame.get("objects").and_then(Value::elements).map(|e| e.iter().collect()).unwrap_or_default();
        let objects = objects.into_iter().map(|object| {
            let mut object = object.as_obj().cloned().unwrap_or_default();
            object.remove("_movieclipSample");
            Value::object(object)
        });
        frame.set("objects", Value::array(objects.collect()));
        answer.push(("preview", Value::object(frame)));
    }
    answer.push(("events", Value::array(events)));
    let figures = if changes_only {
        match (&editor.authored, &editor.drag) {
            (Some(edit), Some(drag)) => {
                let taken = drag.before.iter().filter_map(crate::record::uid).collect();
                Some(changed_figures(&edit.frame, draft.as_ref(), Some(&taken)))
            }
            _ => None,
        }
    } else { editor.draft_figures() };
    answer.push(("draft", figures.map_or(Value::Null, Value::array)));
    answer.push(("drag", editor.drag_value()));
    Ok(response_value(answer))
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
