//! Compact responses reconstruct the complete session response and keep the oracle's
//! publication bytes. No ownership gate is changed by this protocol test.
use super::*;
use std::path::Path;

fn json(value: &Value) -> String { styk_doc::json::to_string(value) }
fn parse(text: &str) -> Value { styk_doc::json::parse(text.as_bytes()).unwrap() }

#[test]
fn compact_pose_records_reuse_cow_values_and_strip_only_runtime_movieclip_clock() {
    let frame = parse(r#"{"objects":[{"unique_id":1,"angles":[-0,0.25],"unknown":{"_movieclipSample":{"keep":true}}},{"unique_id":2,"angles":[0.5],"_movieclipSample":{"tick":3},"unknown":"retained"},{"unique_id":3,"angles":[1]}]}"#);
    let before = json(&frame);
    let taken = HashSet::from([1, 2]);
    let records = changed_figures(&frame, None, Some(&taken));
    assert_eq!(records.len(), 2);
    let first = frame.get("objects").unwrap().item(0).unwrap();
    assert!(records[0].same_node(&first), "an ordinary live pose response borrows its existing COW record");
    assert_eq!(json(&Value::array(records.clone())), r#"[{"unique_id":1,"angles":[0,0.25],"unknown":{"_movieclipSample":{"keep":true}}},{"unique_id":2,"angles":[0.5],"unknown":"retained"}]"#);
    assert_eq!(json(&frame), before, "clock cleanup never edits the authored frame");
    let mut later = records[0].clone();
    later.make_obj().unwrap().set("angles", parse("[0.75]"));
    assert_eq!(json(&records[0]), json(&first), "later edits retain the already returned pose");
    assert!(changed_figures(&frame, Some(&frame), Some(&taken)).is_empty(), "unchanged records are still omitted");
}

#[test]
fn ordered_state_response_matches_all_five_recorded_figure_tapes() {
    let mut calls = 0;
    for name in ["startup-sheet", "rig-sheet", "gestures-sheet", "figures-layers-sheet", "drag-cancel"] {
        let file = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../tests/edit/{name}.reference.json"));
        let reference = styk_doc::json::parse(&std::fs::read(file).unwrap()).unwrap();
        let steps = reference.get("steps").unwrap().elements().unwrap();
        let first = steps.get(0).unwrap();
        let mut state = first.get("state").unwrap().as_obj().unwrap().clone();
        if let Some(options) = reference.get("editor").and_then(Value::as_obj) {
            for (key, value) in options.iter() { state.set(key, value.clone()); }
        }
        let mut session = Session::start(parse(first.get("document").unwrap().as_str().unwrap()), &json(&Value::object(state))).unwrap();
        assert_eq!(json(&session.editor.state_value()), json(first.get("state").unwrap()), "{name}: initial state");
        for (index, step) in steps.iter().enumerate().skip(1) {
            let call = step.get("call").unwrap().elements().unwrap();
            let method = call.get(0).unwrap();
            let args = Value::array(call.iter().skip(1).collect());
            let mut view = session.editor.state_value().as_obj().unwrap().clone();
            view.set("autoKey", Value::Bool(session.editor.auto_key));
            view.set("changesOnly", Value::Bool(true));
            let answer = session.call_values(method.as_str().unwrap(), &args, &Value::object(view));
            assert_ne!(answer.get("status").and_then(Value::as_str), Some("unported"), "{name} step {index}: {answer:?}");
            assert_eq!(json(answer.get("state").unwrap()), json(step.get("state").unwrap()), "{name} step {index}: ordered state");
            calls += 1;
        }
    }
    assert_eq!(calls, 130);
}

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
    for name in ["move-native", "drag-cancel", "range-native", "startup-sheet", "rig-sheet", "gestures-sheet", "layers-sheet", "figures-layers-sheet", "locked-sheet",
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

#[test]
fn worker_values_and_legacy_json_share_recorded_editor_outcomes() {
    let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/edit");
    let mut calls = 0;
    for name in ["move-native", "drag-cancel", "range-native", "gestures-sheet",
        "layers-sheet", "figures-layers-sheet", "locked-sheet",
        "figure-clipboard-sheet", "figure-clipboard-clip-sheet",
        "property-clip-patches-sheet", "topology-sheet", "topology-native",
        "vfx-clip-paints-native", "vfx-clip-paints-sheet"] {
        let reference = parse(&std::fs::read_to_string(folder.join(format!("{name}.reference.json"))).unwrap());
        let steps: Vec<Value> = reference.get("steps").unwrap().elements().unwrap().iter().collect();
        for compact in [false, true] {
            let mut state = steps[0].get("state").unwrap().as_obj().unwrap().clone();
            for (key, value) in reference.get("editor").unwrap().as_obj().unwrap().iter() { state.set(key, value.clone()); }
            let initial = parse(steps[0].get("document").unwrap().as_str().unwrap());
            let mut legacy = Session::start(initial.clone(), &json(&Value::object(state.clone()))).unwrap();
            let mut worker = Session::start(initial, &json(&Value::object(state))).unwrap();
            let mut expected = steps[0].get("document").unwrap().as_str().unwrap().to_string();
            for step in &steps[1..] {
                let call: Vec<Value> = step.get("call").unwrap().elements().unwrap().iter().collect();
                let method = call[0].as_str().unwrap().trim_start_matches('@');
                let mut view = legacy.editor.state_value().as_obj().unwrap().clone();
                view.set("autoKey", Value::Bool(legacy.editor.options.auto_key));
                view.set("draws", step.get("draws").cloned().unwrap_or(Value::Null));
                if compact { view.set("changesOnly", Value::Bool(true)); }
                let view = Value::object(view);let args = Value::array(call[1..].to_vec());
                let old = legacy.call(method, &json(&args), &json(&view));
                let answer = worker.call_values(method, &args, &view);
                let label = format!("{name}/{} {method}, compact={compact}", step.get("step").unwrap());
                assert_eq!(json(&answer), old, "{label}: wire bytes");
                assert_ne!(answer.get("status").and_then(Value::as_str), Some("unported"), "{label}");
                assert_eq!(answer.get("state"), step.get("state"), "{label}: editor state");
                assert_eq!(answer.get("events"), step.get("events"), "{label}: editor events");
                if let Some(document) = step.get("document").and_then(Value::as_str) { expected = document.to_string(); }
                assert_eq!(worker.editor.report_json(), expected, "{label}: editor document");
                assert_eq!(legacy.editor.report_json(), worker.editor.report_json(), "{label}: owner");
                calls += 1;
            }
        }
    }
    assert!(calls > 200);
}

#[test]
fn malformed_json_wrapper_does_not_mutate_the_canonical_owner() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/edit/move-sheet.reference.json");
    let reference = parse(&std::fs::read_to_string(path).unwrap());
    let first = reference.get("steps").unwrap().item(0).unwrap();
    let mut session = Session::start(parse(first.get("document").unwrap().as_str().unwrap()), &json(first.get("state").unwrap())).unwrap();
    let report = session.editor.report().clone();let state = session.editor.state_value();
    for (args, view) in [("[", "null"), ("[]", "{"), ("[", "{")] {
        let answer = parse(&session.call("noSuchEditingCommand", args, view));
        assert_eq!(json(&answer), "{\"status\":\"error\",\"error\":\"the call's arguments are not JSON\"}");
        assert!(report.same_node(session.editor.report()));assert_eq!(session.editor.state_value(), state);
        assert!(session.editor.take_events().is_empty());
    }
}

#[test]
fn quiet_worker_session_prepares_the_same_owner_and_keeps_retired_heads() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/edit/move-native.reference.json");
    let reference = parse(&std::fs::read_to_string(path).unwrap());
    let first = reference.get("steps").unwrap().item(0).unwrap();
    let mut session = Session::start(parse(first.get("document").unwrap().as_str().unwrap()), &json(first.get("state").unwrap())).unwrap();
    let before = session.report().clone();
    assert!(matches!(session.prepare_worker_call_values("setFPS", &parse("[31]"), &Value::Null, || Ok(())), Err(EditError::Unported(_))));
    assert!(session.report().same_node(&before));
    session.prepare_worker_history(crate::HistoryLimits::default(), 0.0, || Ok(())).unwrap();
    let state = session.editor.state_value();
    let ready = session.prepare_worker_call_values("setFPS", &parse("[31]"), &Value::Null, || Ok(())).unwrap();
    assert_eq!(ready.result().get("status").and_then(Value::as_str), Some("ok"));
    assert_eq!(ready.result().path(&["state", "undoLabel"]).and_then(Value::as_str), Some("Change frame rate"));
    assert_eq!(ready.result().get("events").and_then(Value::array_len), Some(1));
    assert!(session.report().same_node(&before));assert_eq!(session.editor.state_value(), state);
    let after = ready.report().clone();
    let guard = session.guard_worker_publication_checked(ready, || Ok(())).unwrap_or_else(|(error, _)| panic!("{error:?}"));
    let retired = guard.commit();
    assert!(retired.previous_report().same_node(&before));assert!(session.report().same_node(&after));
    drop(retired);
    let undo = session.prepare_worker_call_values("undo", &parse("[]"), &Value::Null, || Ok(())).unwrap();
    assert_eq!(undo.result().get("status").and_then(Value::as_str), Some("ok"));
    let guard = session.guard_worker_publication_checked(undo, || Ok(())).unwrap_or_else(|(error, _)| panic!("{error:?}"));
    drop(guard.commit());assert_eq!(json(session.report()), json(&before));
}

#[test]
fn worker_session_preserves_cancellation_stale_views_and_quiet_refusals() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/edit/move-native.reference.json");
    let reference = parse(&std::fs::read_to_string(path).unwrap());
    let first = reference.get("steps").unwrap().item(0).unwrap();
    let mut session = Session::start(parse(first.get("document").unwrap().as_str().unwrap()), &json(first.get("state").unwrap())).unwrap();
    session.prepare_worker_history(crate::HistoryLimits::default(), 0.0, || Ok(())).unwrap();
    let before = session.report().clone();let state = session.editor.state_value();
    let mut checks = 0;
    let error = session.prepare_worker_call_values("setFPS", &parse("[31]"), &Value::Null, || {
        checks += 1;if checks == 3 { Err(EditError::Invalid("cancel after owner call".into())) } else { Ok(()) }
    });
    assert!(matches!(error, Err(EditError::Invalid(ref reason)) if reason == "cancel after owner call"));
    assert!(session.report().same_node(&before));assert_eq!(session.editor.state_value(), state);
    let ready = session.prepare_worker_call_values("setFPS", &parse("[31]"), &Value::Null, || Ok(())).unwrap();
    session.call_values("select", &parse("[2,-1]"), &Value::Null);
    let newer = session.editor.state_value();
    let rejected = match session.guard_worker_publication_checked(ready, || Ok(())) {
        Err((EditError::Invalid(_), ready)) => ready,
        Err((error, _)) => panic!("unexpected {error:?}"),
        Ok(_) => panic!("stale session view accepted"),
    };
    assert_eq!(session.editor.state_value(), newer);assert!(session.report().same_node(&before));drop(rejected);
    session.call_values("select", &parse("[1,-1]"), &Value::Null);
    session.call_values("beginDrag", &parse("[\"Live gesture\"]"), &Value::Null);
    let drag = session.editor.drag_value();
    assert!(matches!(session.prepare_worker_call_values("setFPS", &parse("[31]"), &Value::Null, || Ok(())), Err(EditError::Unported(_))));
    assert_eq!(session.editor.drag_value(), drag);
    session.call_values("cancelDrag", &parse("[]"), &Value::Null);
    assert!(session.editor.drag.is_none());
}
