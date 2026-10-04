//! Compact responses reconstruct the complete session response and keep the oracle's
//! publication bytes. No ownership gate is changed by this protocol test.
use super::*;
use std::path::Path;

fn json(value: &Value) -> String { styk_doc::json::to_string(value) }
fn parse(text: &str) -> Value { styk_doc::json::parse(text.as_bytes()).unwrap() }

fn merge_figures(base: &Value, changed: &Value, order: Option<&Value>) -> Value {
    let mut objects: Vec<Value> = base.elements().unwrap().iter().collect();
    for figure in changed.elements().unwrap().iter() {
        let uid = crate::record::uid(&figure).unwrap();
        match objects.iter().position(|object| crate::record::uid(object) == Some(uid)) {
            Some(slot) => objects[slot] = figure,
            None => objects.push(figure),
        }
    }
    if let Some(order) = order {
        objects = order.elements().unwrap().iter().map(|uid| objects.iter().find(|object| object.get("unique_id") == Some(&uid)).unwrap().clone()).collect();
    }
    Value::array(objects)
}

fn clean_objects(frame: &Value) -> Value {
    let objects = frame.get("objects").unwrap().elements().unwrap().iter().map(|figure| {
        let mut figure = figure.as_obj().unwrap().clone();
        figure.remove("_movieclipSample");
        Value::object(figure)
    }).collect();
    Value::array(objects)
}

fn reconstruct_sheet(before: &Value, answer: &Value) -> Value {
    let previous = before.path(&["_styk", "layerSheet"]).unwrap().as_obj().unwrap();
    let mut sheet = previous.clone();
    for (key, value) in answer.get("sheet").unwrap().as_obj().unwrap().iter() {
        if &**key != "tracks" { sheet.set(key, value.clone()); }
    }
    for key in answer.get("sheetRemoved").and_then(Value::elements).into_iter().flat_map(|keys| keys.iter()) {
        sheet.remove(key.as_str().unwrap());
    }
    let old_tracks = previous.get("tracks").unwrap().as_obj().unwrap();
    let changed = answer.path(&["sheet", "tracks"]).unwrap().as_obj().unwrap();
    let order: Vec<Value> = answer.get("trackOrder").and_then(Value::elements).map(|order| order.iter().collect()).unwrap_or_else(|| old_tracks.keys().map(|key| Value::str(key)).collect());
    let mut tracks = Obj::new();
    for id in order {
        let id = id.as_str().unwrap();
        tracks.set(id, changed.get(id).or_else(|| old_tracks.get(id)).unwrap().clone());
    }
    sheet.set("tracks", Value::object(tracks));
    if let Some(order) = answer.get("sheetOrder") {
        let mut ordered = Obj::new();
        for key in order.elements().unwrap().iter() {
            let key = key.as_str().unwrap();
            ordered.set(key, sheet.get(key).unwrap().clone());
        }
        sheet = ordered;
    }
    Value::object(sheet)
}

#[test]
fn compact_answers_preserve_recorded_publications_drafts_and_history() {
    let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/edit");
    let mut savings = 0;
    for name in ["move-native", "drag-cancel", "range-native", "gestures-sheet", "layers-sheet", "figures-layers-sheet", "locked-sheet",
        "handle-edit-chain-sheet", "figure-clipboard-sheet", "figure-clipboard-clip-sheet", "movieclip-sheet", "movieclip-loose-sheet",
        "channels-figure-sheet", "channels-movieclip-sheet", "channels-style-sheet", "property-clip-patches-sheet", "topology-sheet", "topology-native"] {
        let reference = styk_doc::json::parse(&std::fs::read(folder.join(format!("{name}.reference.json"))).unwrap()).unwrap();
        let steps: Vec<Value> = reference.get("steps").unwrap().elements().unwrap().iter().collect();
        let mut state = steps[0].get("state").unwrap().as_obj().unwrap().clone();
        for (key, value) in reference.get("editor").unwrap().as_obj().unwrap().iter() { state.set(key, value.clone()); }
        let report = parse(steps[0].get("document").unwrap().as_str().unwrap());
        let mut full = Session::start(report.clone(), &json(&Value::object(state.clone()))).unwrap();
        let mut compact = Session::start(report, &json(&Value::object(state))).unwrap();
        for step in &steps[1..] {
            let args: Vec<Value> = step.get("call").unwrap().elements().unwrap().iter().collect();
            let method = args[0].as_str().unwrap().trim_start_matches('@');
            let mut view = full.editor.state_value().as_obj().unwrap().clone();
            view.set("autoKey", Value::Bool(full.editor.options.auto_key));
            view.set("draws", step.get("draws").cloned().unwrap_or(Value::Null));
            let mut small_view = view.clone(); small_view.set("changesOnly", Value::Bool(true));
            let before = compact.editor.report().clone();
            let draft = compact.editor.authored.as_ref().map(|edit| clean_objects(&edit.frame));
            let retained = compact.editor.retained_frame().map(|frame| clean_objects(&frame));
            let payload = json(&Value::array(args[1..].to_vec()));
            let complete_text = full.call(method, &payload, &json(&Value::object(view)));
            let small_text = compact.call(method, &payload, &json(&Value::object(small_view)));
            let complete = parse(&complete_text); let small = parse(&small_text);
            let label = format!("{name} step {} ({method})", step.get("step").unwrap());
            for key in ["status", "result", "reason", "error", "state", "events", "drag", "preview", "sources", "definitions", "retainedFields", "retainedRemoved", "retainedFieldOrder"] {
                if key == "result" && complete.get("status").and_then(Value::as_str) == Some("ok") && matches!(method, "duplicateFigure" | "addFigure" | "importFigure" | "pasteSelection" | "duplicateSelection" | "sheetAppendFrame") {
                    assert_eq!(small.get("result"), Some(&Value::Null));
                    continue;
                }
                assert_eq!(complete.get(key).map(json), small.get(key).map(json), "{label}: {key}");
            }
            assert_eq!(json(full.editor.report()), json(compact.editor.report()), "{label}: document");
            if let Some(changed) = small.get("draft").filter(|value| value.is_array()) {
                let reconstructed = match &draft { Some(draft) => merge_figures(draft, changed, None), None => changed.clone() };
                // Taking another figure mid-gesture can add an unchanged figure to the draft.
                let taken: Vec<u32> = full.editor.drag.as_ref().unwrap().before.iter().filter_map(crate::record::uid).collect();
                let raw = full.editor.authored.as_ref().unwrap();
                let mut adopted = reconstructed.elements().unwrap().iter().filter(|figure| crate::record::uid(figure).is_some_and(|uid| taken.contains(&uid))).collect::<Vec<_>>();
                for figure in clean_objects(&raw.frame).elements().unwrap().iter().filter(|figure| crate::record::uid(figure).is_some_and(|uid| taken.contains(&uid))) {
                    if !adopted.iter().any(|known| crate::record::uid(known) == crate::record::uid(&figure)) { adopted.push(figure); }
                }
                adopted.sort_by_key(|object| object.get("object_slot").and_then(Value::as_f64).unwrap() as usize);
                assert_eq!(json(&Value::array(adopted)), json(complete.get("draft").unwrap()), "{label}: draft");
            } else { assert_eq!(complete.get("draft"), small.get("draft"), "{label}: draft ended"); }
            if small.get("sheet").is_some() {
                assert_eq!(json(&reconstruct_sheet(&before, &small)), json(compact.editor.report().path(&["_styk", "layerSheet"]).unwrap()), "{label}: sheet");
                let layers = small.get("layers").or_else(|| before.path(&["_styk", "layers"]));
                assert_eq!(layers.map(json), complete.get("layers").map(json), "{label}: layers");
            }
            if let Some(changed) = small.get("retained") {
                let adopted = if small.get("retainedChanges").is_some_and(Value::truthy) {
                    merge_figures(draft.as_ref().or(retained.as_ref()).unwrap(), changed, small.get("retainedOrder"))
                } else { changed.clone() };
                let mut expected_frame = Obj::new();
                expected_frame.set("objects", complete.get("retained").unwrap().clone());
                assert_eq!(json(&adopted), json(&clean_objects(&Value::object(expected_frame))), "{label}: retained without runtime samples");
            }
            if small_text.len() < complete_text.len() { savings += 1; }
        }
    }
    assert!(savings > 100, "{savings} calls had a smaller answer");
}

#[test]
fn refused_and_invalid_calls_send_no_replacements_and_preserve_the_live_gesture() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/edit/move-sheet.reference.json");
    let reference = styk_doc::json::parse(&std::fs::read(path).unwrap()).unwrap();
    let first = reference.get("steps").unwrap().item(0).unwrap();
    let mut state = first.get("state").unwrap().as_obj().unwrap().clone();
    for (key, value) in reference.get("editor").unwrap().as_obj().unwrap().iter() { state.set(key, value.clone()); }
    let mut session = Session::start(parse(first.get("document").unwrap().as_str().unwrap()), &json(&Value::object(state))).unwrap();
    assert_eq!(parse(&session.call("select", "[1,-1]", "null")).get("status").and_then(Value::as_str), Some("ok"));
    assert_eq!(parse(&session.call("beginDrag", "[\"Pose figure\"]", "null")).get("status").and_then(Value::as_str), Some("ok"));
    let before = json(session.editor.report());
    let before_state = session.editor.state_value();
    let before_drag = session.editor.drag_value();
    let mut view = before_state.as_obj().unwrap().clone(); view.set("changesOnly", Value::Bool(true));
    for (method, args, status) in [("moveSelection", "[null,1]", "error"), ("noSuchEditingCommand", "[]", "unported")] {
        let answer = parse(&session.call(method, args, &json(&Value::object(view.clone()))));
        assert_eq!(answer.get("status").and_then(Value::as_str), Some(status));
        assert_eq!(answer.get("draft").and_then(Value::array_len), Some(0), "{method}: no replaced figures");
        assert_eq!(answer.get("state"), Some(&before_state), "{method}: state");
        assert_eq!(answer.get("drag"), Some(&before_drag), "{method}: drag");
        assert_eq!(answer.get("events").and_then(Value::array_len), Some(0), "{method}: no events");
        assert_eq!(json(session.editor.report()), before, "{method}: document");
    }
    let answer = parse(&session.call("commitDrag", "[]", &json(&Value::object(view))));
    assert_eq!(answer.get("status").and_then(Value::as_str), Some("ok"));
    assert_eq!(answer.get("result"), Some(&Value::Bool(false)));
    assert_eq!(answer.get("draft"), Some(&Value::Null));
    assert_eq!(answer.get("drag"), Some(&Value::Null));
    assert_eq!(json(session.editor.report()), before);
}
