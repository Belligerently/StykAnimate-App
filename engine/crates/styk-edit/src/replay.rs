//! The command-replay harness, engine side (docs/engine/editing.md): runs a reference
//! recorded by `engine/tools/edit-replay.mjs` in an [`Editor`] and compares, after
//! every step, the document's bytes, the playhead, the selection, history and the
//! change events with what the JavaScript editor did.

use styk_doc::{Obj, Value};

use crate::{EditError, Editor, Options, Selection};

/// What a replay found.
#[derive(Debug, Default)]
pub struct Outcome {
    pub scenario: String,
    /// Steps in the reference (not counting the loaded document).
    pub steps: usize,
    /// Steps that ran in the engine and matched.
    pub matched: usize,
    /// The first step the engine does not run yet, and why. Later steps are not compared.
    pub unported: Option<(usize, String)>,
    /// Differences, one line each. A replay stops at the first step with one.
    pub mismatches: Vec<String>,
}

impl Outcome {
    pub fn passed(&self) -> bool {
        self.mismatches.is_empty()
    }
}

/// Replays `reference` (the JSON the JavaScript side wrote).
pub fn replay(reference: &Value) -> Result<Outcome, String> {
    let steps: Vec<Value> = reference.get("steps").and_then(Value::elements).ok_or("the reference has no steps")?.iter().collect();
    let first = steps.first().ok_or("the reference has no steps")?;
    let mut expected = first.get("document").and_then(Value::as_str).ok_or("step 0 has no document")?.to_string();
    let report = styk_doc::json::parse(expected.as_bytes()).map_err(|error| error.to_string())?;
    let flag = |key: &str| reference.path(&["editor", key]).is_some_and(Value::truthy);
    let mut editor = Editor::new(report, Options { property_channels: flag("propertyChannels"), layer_timeline: flag("layerTimeline") });
    let state = first.get("state").ok_or("step 0 has no state")?;
    let selected = state.get("selectedUid").and_then(Value::as_f64).map(|v| v as u32);
    let uids = state.get("selectedUids").and_then(Value::numbers).unwrap_or_default().into_iter().map(|v| v as u32).collect();
    editor.set_view(
        state.get("tick").and_then(Value::as_f64).unwrap_or(0.0),
        state.get("frameIndex").and_then(Value::as_f64).unwrap_or(0.0) as usize,
        Selection { uid: selected, uids, joint: state.get("selectedJoint").and_then(Value::as_f64).unwrap_or(-1.0) as i64 },
    );
    let mut outcome = Outcome { scenario: reference.get("scenario").and_then(Value::as_str).unwrap_or("").to_string(), steps: steps.len() - 1, ..Outcome::default() };
    if editor.report_json() != expected {
        outcome.mismatches.push("step 0: the loaded document does not survive the engine's store byte for byte".into());
        return Ok(outcome);
    }
    for step in &steps[1..] {
        let number = step.get("step").and_then(Value::as_f64).unwrap_or(0.0) as usize;
        let call: Vec<Value> = step.get("call").and_then(Value::elements).map(|e| e.iter().collect()).unwrap_or_default();
        let method = call.first().and_then(Value::as_str).unwrap_or("").to_string();
        let result = editor.call(&method, &call[1..]);
        let mut differences = Vec::new();
        let recorded_error = step.get("error").filter(|e| !e.is_null());
        match (&result, recorded_error) {
            (Err(EditError::Unported(what)), _) => {
                outcome.unported = Some((number, what.clone()));
                return Ok(outcome);
            }
            (Err(EditError::Invalid(key)), Some(error)) => {
                let recorded = error.get("key").and_then(Value::as_str).unwrap_or("");
                if recorded != key {
                    differences.push(format!("error {key}, the editor's was {recorded}"));
                }
            }
            (Err(EditError::Invalid(key)), None) => differences.push(format!("error {key}, the editor succeeded")),
            (Ok(_), Some(error)) => differences.push(format!("succeeded, the editor failed with {}", error.get("key").and_then(Value::as_str).unwrap_or("an error"))),
            (Ok(value), None) => {
                // `seek` returns the evaluated frame: a view the facade reads from its own copy.
                if method != "seek" && method != "@seek" {
                    let got = json(&plain(value, 4).unwrap_or(Value::Null));
                    let want = json(step.get("result").unwrap_or(&Value::Null));
                    if got != want {
                        differences.push(format!("returned {}, the editor {}", short(&got), short(&want)));
                    }
                }
            }
        }
        if let Some(document) = step.get("document").and_then(Value::as_str) {
            expected = document.to_string();
        }
        let actual = editor.report_json();
        if actual != expected {
            differences.push(format!("document differs: {}", first_difference(&actual, &expected)));
        }
        let got = json(&state_value(&editor));
        let want = json(step.get("state").unwrap_or(&Value::Null));
        if got != want {
            differences.push(format!("state {got}, the editor's {want}"));
        }
        let got = json(&Value::array(editor.take_events()));
        let want = json(step.get("events").unwrap_or(&Value::Null));
        if got != want {
            differences.push(format!("events {}, the editor's {}", short(&got), short(&want)));
        }
        if !differences.is_empty() {
            outcome.mismatches.extend(differences.into_iter().map(|d| format!("step {number} ({method}): {d}")));
            return Ok(outcome);
        }
        outcome.matched += 1;
    }
    Ok(outcome)
}

/// The state as `edit-replay.mjs` records it, in its key order.
fn state_value(editor: &Editor) -> Value {
    let state = editor.state();
    let mut object = Obj::new();
    object.set("tick", Value::Num(state.tick));
    object.set("frameIndex", Value::Num(state.frame_index as f64));
    object.set("selectedUid", state.selected_uid.map_or(Value::Null, |u| Value::Num(u as f64)));
    object.set("selectedUids", Value::array(state.selected_uids.iter().map(|&u| Value::Num(u as f64)).collect()));
    object.set("selectedJoint", Value::Num(state.selected_joint as f64));
    object.set("canUndo", Value::Bool(state.can_undo));
    object.set("canRedo", Value::Bool(state.can_redo));
    object.set("undoLabel", Value::str(&state.undo_label));
    object.set("redoLabel", Value::str(&state.redo_label));
    object.set("dirty", Value::Bool(state.dirty));
    Value::object(object)
}

/// `plain()` in edit-replay.mjs: containers deeper than `depth` are dropped.
fn plain(value: &Value, depth: u32) -> Option<Value> {
    match value {
        Value::Null | Value::Bool(_) | Value::Num(_) | Value::Str(_) | Value::Blob(_) => Some(value.clone()),
        _ if depth == 0 => None,
        Value::Arr(_) | Value::Nums(_) => Some(Value::array(value.elements().unwrap().iter().filter_map(|item| plain(&item, depth - 1)).collect())),
        Value::Obj(object) => Some(Value::object(object.iter().filter_map(|(key, item)| Some((key.clone(), plain(item, depth - 1)?))).collect())),
    }
}

fn json(value: &Value) -> String {
    styk_doc::json::to_string(value)
}

fn short(text: &str) -> String {
    if text.len() <= 400 { text.to_string() } else { format!("{}…", &text[..text.floor_char_boundary(400)]) }
}

fn first_difference(actual: &str, expected: &str) -> String {
    let at = actual.bytes().zip(expected.bytes()).position(|(a, b)| a != b).unwrap_or(actual.len().min(expected.len()));
    let from = actual.floor_char_boundary(at.saturating_sub(80));
    format!(
        "at byte {at} of {} (editor {}): engine …{}… editor …{}…",
        actual.len(),
        expected.len(),
        &actual[from..actual.floor_char_boundary((at + 80).min(actual.len()))],
        &expected[expected.floor_char_boundary(from.min(expected.len()))..expected.floor_char_boundary((at + 80).min(expected.len()))]
    )
}
