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

fn session_from_recorded_state(reference: &Value, state: &Value) -> Session {
    let first = reference.get("steps").unwrap().item(0).unwrap();
    let mut state = state.as_obj().unwrap().clone();
    for (key, value) in reference.get("editor").unwrap().as_obj().unwrap().iter() { state.set(key, value.clone()); }
    Session::start(parse(first.get("document").unwrap().as_str().unwrap()), &json(&Value::object(state))).unwrap()
}

/// Consume only the documented stored replacements; frame metadata/order stay local.
fn apply_split_packet(mut report: Value, packet: &Value) -> Value {
    let index = packet.get("index").unwrap().as_f64().unwrap() as usize;
    report.make_obj().unwrap().get_mut("figure_types").unwrap().make_arr().unwrap()[index] = packet.get("figure").unwrap().clone();
    for change in packet.get("frames").unwrap().elements().unwrap().iter() {
        let place = match change.get("sourceId").and_then(Value::as_str) {
            Some(id) => { assert_eq!(change.get("index").and_then(Value::as_f64), Some(-1.0)); crate::topology::Place::Source(id.into()) }
            None => crate::topology::Place::Keyframe(change.get("index").unwrap().as_f64().unwrap() as usize),
        };
        let frame = crate::topology::frame_mut(&mut report, &place).unwrap();
        let objects = frame.get_mut("objects").unwrap().make_arr().unwrap();
        for replacement in change.get("objects").unwrap().elements().unwrap().iter() {
            let slot = objects.iter_mut().find(|object| crate::record::uid(object) == crate::record::uid(&replacement)).unwrap();
            *slot = replacement;
        }
    }
    for (key, path) in [("animation", &["_styk"][..]), ("sheetAnimation", &["_styk", "layerSheet"][..])] {
        if let Some(change) = packet.get(key) {
            let mut parent = &mut report;
            for key in path { parent = parent.make_obj().unwrap().get_mut(key).unwrap(); }
            let parent = parent.make_obj().unwrap();
            let field = if key == "animation" { "animation" } else { "legacyAnimation" };
            if change.get("exists") == Some(&Value::Bool(true)) { parent.set(field, change.get("value").unwrap().clone()); }
            else { parent.remove(field); }
        }
    }
    report
}

#[test]
fn split_topology_packets_reconstruct_recorded_storage_without_changing_default_answers() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/edit/builder-split-sheet.reference.json");
    let reference = parse(&std::fs::read_to_string(path).unwrap());
    let steps: Vec<Value> = reference.get("steps").unwrap().elements().unwrap().iter().collect();
    for compact in [false, true] {
        let mut ordinary = session_from_recorded_state(&reference, steps[0].get("state").unwrap());
        let mut requested = session_from_recorded_state(&reference, steps[0].get("state").unwrap());
        let mut expected = steps[0].get("document").unwrap().as_str().unwrap().to_string();
        let mut packets = Vec::new();
        for step in &steps[1..] {
            let call: Vec<Value> = step.get("call").unwrap().elements().unwrap().iter().collect();
            let method = call[0].as_str().unwrap();
            let label = format!("builder split step{} compact={compact}", step.get("step").unwrap());
            if method == "@frame" {
                for session in [&ordinary, &requested] {
                    let frame = session.editor.shown_frame().unwrap();
                    assert_eq!(Some(&Value::str(&json(&styk_eval::records::snapshot_pose_frame(&frame)))), step.get("result"), "{label}: visible frame");
                }
            } else {
                let before = requested.report().clone();
                let mut view = ordinary.editor.state_value().as_obj().unwrap().clone();
                view.set("autoKey", Value::Bool(ordinary.editor.auto_key));
                view.set("changesOnly", Value::Bool(compact));
                let args = Value::array(call[1..].to_vec());
                let ordinary_answer = ordinary.call_values(method.trim_start_matches('@'), &args, &Value::object(view.clone()));
                view.set("topologyPublication", Value::Bool(true));
                let mut answer = requested.call_values(method.trim_start_matches('@'), &args, &Value::object(view));
                if let Some(packet) = answer.get("topology").cloned() {
                    assert!(matches!(method, "splitBuilderSegments" | "splitSegment" | "splitSegmentBranch"));
                    assert_eq!(answer.get("result"), Some(&Value::Bool(true)));
                    assert_eq!(json(&apply_split_packet(before.clone(), &packet)), json(requested.report()), "{label}: stored definition and poses");
                    if packets.is_empty() {
                        let mut unsupported = requested.report().clone();
                        crate::topology::frame_mut(&mut unsupported, &crate::topology::Place::Keyframe(0)).unwrap().set("unknownMetadata", Value::Bool(true));
                        assert!(split_topology_publication(&before, &unsupported).is_none(), "a pose packet cannot elide frame metadata changes");
                        let mut unsupported = requested.report().clone();
                        crate::topology::frame_mut(&mut unsupported, &crate::topology::Place::Keyframe(0)).unwrap().set("objects", Value::array(Vec::new()));
                        assert!(split_topology_publication(&before, &unsupported).is_none(), "a pose packet cannot remove membership");
                        for (key, path, field) in [("animation", &["_styk"][..], "animation"), ("sheetAnimation", &["_styk", "layerSheet"][..], "legacyAnimation")] {
                            let mut explicit_null = requested.report().clone();
                            let mut parent = &mut explicit_null;
                            for key in path { parent = parent.make_obj().unwrap().get_mut(key).unwrap(); }
                            parent.make_obj().unwrap().set(field, Value::Null);
                            let presence = split_topology_publication(&before, &explicit_null).unwrap();
                            assert_eq!(presence.get(key).unwrap().get("exists"), Some(&Value::Bool(true)));
                            assert_eq!(presence.get(key).unwrap().get("value"), Some(&Value::Null));
                            assert_eq!(json(&apply_split_packet(before.clone(), &presence)), json(&explicit_null));
                            let mut before_null = before.clone();
                            let mut parent = &mut before_null;
                            for key in path { parent = parent.make_obj().unwrap().get_mut(key).unwrap(); }
                            parent.make_obj().unwrap().set(field, Value::Null);
                            let absence = split_topology_publication(&before_null, requested.report()).unwrap();
                            assert_eq!(absence.get(key).unwrap().get("exists"), Some(&Value::Bool(false)));
                            assert!(absence.get(key).unwrap().get("value").is_none());
                            assert_eq!(json(&apply_split_packet(before_null, &absence)), json(requested.report()));
                        }
                    }
                    let index = packet.get("index").unwrap().as_f64().unwrap() as usize;
                    assert!(packet.get("figure").unwrap().same_node(&requested.report().get("figure_types").unwrap().item(index).unwrap()), "{label}: COW definition");
                    packets.push((packet.clone(), json(&packet)));
                    answer.make_obj().unwrap().remove("topology");
                } else if matches!(method, "splitBuilderSegments" | "splitSegment" | "splitSegmentBranch") && step.get("result") == Some(&Value::Bool(true)) {
                    panic!("{label}: successful split did not publish its stored changes");
                }
                assert_eq!(json(&answer), json(&ordinary_answer), "{label}: existing answer bytes");
                assert!(ordinary_answer.get("topology").is_none());
                assert_eq!(json(answer.get("state").unwrap()), json(step.get("state").unwrap()), "{label}: recorded state");
                assert_eq!(json(answer.get("events").unwrap()), json(step.get("events").unwrap()), "{label}: recorded events");
                if let Some(error) = step.get("error").filter(|error| !error.is_null()) {
                    assert_eq!(answer.get("status").and_then(Value::as_str), Some("error"));
                    assert_eq!(answer.get("error"), error.get("key"));
                } else {
                    assert_eq!(answer.get("status").and_then(Value::as_str), Some("ok"));
                    assert_eq!(crate::replay::plain(answer.get("result").unwrap(), 4).unwrap_or(Value::Null), step.get("result").cloned().unwrap());
                }
            }
            if let Some(document) = step.get("document").and_then(Value::as_str) { expected = document.into(); }
            assert_eq!(requested.editor.report_json(), expected, "{label}: recorded document");
            assert_eq!(ordinary.editor.report_json(), expected, "{label}: ordinary document");
        }
        assert_eq!(packets.len(), 4, "both direct modes and the two aliases publish");
        for (packet, bytes) in packets { assert_eq!(json(&packet), bytes, "later history leaves a returned packet immutable"); }
    }
}

#[test]
fn split_topology_packet_survives_zero_history_and_keeps_source_cleanup_local() {
    let reference = parse(&std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/edit/builder-split-sheet.reference.json")).unwrap());
    let first = reference.get("steps").unwrap().item(0).unwrap();
    for limit in [0, 80] {
        let mut session = session_from_recorded_state(&reference, first.get("state").unwrap());
        // Existing authored moves create two real captured source owners; the first
        // is retained only by history after the second replaces the current key.
        for args in [parse("[3,0]"), parse("[1,0]")] {
            let answer = session.call_values("moveSelection", &args, &session.editor.state_value());
            assert_eq!(answer.get("status").and_then(Value::as_str), Some("ok"));
        }
        let captured = session.editor.generated_sources.clone();
        assert_eq!(captured.len(), 2);
        let selected = session.call_values("select", &parse("[1,1]"), &session.editor.state_value());
        assert_eq!(selected.get("status").and_then(Value::as_str), Some("ok"));
        session.editor.history_limits.count = limit;
        let before = session.report().clone();
        let before_counts = session.editor.uid_counts.clone();
        let source_table_was_present = session.editor.source_table_was_present;
        let old_source = before.path(&["_styk", "frameSources", &captured[0]]).unwrap().clone();
        let old_bytes = json(&old_source);
        let mut view = session.editor.state_value().as_obj().unwrap().clone();
        view.set("topologyPublication", Value::Bool(true));
        let answer = session.call_values("splitBuilderSegments", &parse(r#"[{"branch":false,"fraction":0.5}]"#), &Value::object(view));
        assert_eq!(answer.get("status").and_then(Value::as_str), Some("ok"));
        let packet = answer.get("topology").unwrap();
        let changes: Vec<Value> = packet.get("frames").unwrap().elements().unwrap().iter().collect();
        assert!(changes.iter().any(|change| change.get("sourceId").and_then(Value::as_str) == Some(captured[1].as_str())), "current captured poses are carried");
        for change in &changes {
            if let Some(id) = change.get("sourceId").and_then(Value::as_str) {
                assert!(before.path(&["_styk", "frameSources", id]).is_some(), "packet introduces no source");
                let current = session.report().path(&["_styk", "frameSources", id]).unwrap();
                for pose in change.get("objects").unwrap().elements().unwrap().iter() {
                    assert!(current.get("objects").unwrap().elements().unwrap().iter().any(|object| object.same_node(&pose)), "packet borrows the surviving stored pose");
                }
            }
        }
        if limit == 0 {
            assert!(session.editor.undo.is_empty());
            assert!(session.report().path(&["_styk", "frameSources", &captured[0]]).is_none(), "the existing zero-history cleanup drops the orphan");
            assert!(!changes.iter().any(|change| change.get("sourceId").and_then(Value::as_str) == Some(captured[0].as_str())), "cleanup is not a source deletion packet");
            // The adopter publishes the stored replacements and uses its existing
            // record/prune owner. Reconstruct that same empty-history cleanup.
            let mut reconstructed = Editor::new(apply_split_packet(before.clone(), packet), session.editor.options);
            reconstructed.generated_sources = captured.clone();
            reconstructed.uid_counts = before_counts;
            reconstructed.source_table_was_present = source_table_was_present;
            reconstructed.prune_frame_sources();
            assert_eq!(json(reconstructed.report()), json(session.report()), "existing cleanup reconstructs final source table and order");
            assert_eq!(reconstructed.generated_sources, session.editor.generated_sources);
            assert_eq!(reconstructed.uid_counts, session.editor.uid_counts);
        } else {
            assert!(changes.iter().any(|change| change.get("sourceId").and_then(Value::as_str) == Some(captured[0].as_str())), "the kept topology snapshot retains the history-only source");
            assert_eq!(json(&apply_split_packet(before.clone(), packet)), json(session.report()));
        }
        assert_eq!(json(&old_source), old_bytes, "pre-call source aliases are immutable");
    }
}

#[test]
fn unchanged_session_view_keeps_selection_storage_and_continues_options() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/edit");
    for name in ["move-native", "selection-metadata-plain-sheet"] {
        let reference = parse(&std::fs::read_to_string(root.join(format!("{name}.reference.json"))).unwrap());
        let first = reference.get("steps").unwrap().item(0).unwrap();
        let mut session = session_from_recorded_state(&reference, first.get("state").unwrap());
        let pointer = session.editor.selection.uids.as_ptr();
        assert!(!session.editor.selection.uids.is_empty());
        let report = session.report().clone();
        let state = session.editor.state_value();
        let answer = session.call_values("updateProject", &parse("[{}]"), &state);
        assert_eq!(answer.get("status").and_then(Value::as_str), Some("ok"));
        assert_eq!(answer.get("result"), Some(&Value::Bool(false)));
        assert_eq!(pointer, session.editor.selection.uids.as_ptr(), "{name}: unchanged view retains its existing selection allocation");
        assert_eq!(answer.get("state"), Some(&state));
        assert!(session.report().same_node(&report));
        assert_eq!(answer.get("events").and_then(Value::array_len), Some(0));
        assert_eq!(answer.get("drag"), Some(&Value::Null));
        assert_eq!(answer.get("draft"), Some(&Value::Null));

        let mut view = state.as_obj().unwrap().clone();
        view.set("autoKey", Value::Bool(true));
        view.set("sheetSelection", parse(r#"{"id":"camera","contents":true}"#));
        if report.path(&["_styk", "layers"]).is_some() { view.set("activeLayerId", Value::str("camera")); }
        let answer = session.call_values("updateProject", &parse("[{}]"), &Value::object(view));
        assert_eq!(answer.get("status").and_then(Value::as_str), Some("ok"));
        assert_eq!(pointer, session.editor.selection.uids.as_ptr());
        assert!(session.editor.auto_key);
        let sheet = session.editor.sheet_selection.as_ref().unwrap();
        assert_eq!(sheet.id, "camera"); assert!(sheet.contents);
        if report.path(&["_styk", "layers"]).is_some() {
            assert_eq!(session.report().path(&["_styk", "layers", "activeId"]).and_then(Value::as_str), Some("camera"));
        }
    }
}

#[test]
fn session_view_sync_keeps_existing_changed_cold_and_invalid_projection() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/edit");
    let reference = parse(&std::fs::read_to_string(root.join("move-native.reference.json")).unwrap());
    let first = reference.get("steps").unwrap().item(0).unwrap();
    for (view, tick, frame, uid, uids, joint) in [
        (r#"{"tick":1.25,"frameIndex":1,"selectedUid":2,"selectedUids":[2,1],"selectedJoint":0}"#, 1.25f64, 1usize, Some(2u32), vec![2, 1], 0i64),
        (r#"{"tick":-0,"frameIndex":0,"selectedUid":1,"selectedUids":[1],"selectedJoint":-1}"#, -0.0, 0, Some(1), vec![1], -1),
        (r#"{}"#, 0.0, 0, None, vec![], -1),
        (r#"{"tick":0,"frameIndex":0,"selectedUid":1,"selectedUids":[1,false],"selectedJoint":-1}"#, 0.0, 0, Some(1), vec![], -1),
        (r#"{"tick":null,"frameIndex":0.75,"selectedUid":"1","selectedUids":null,"selectedJoint":1.5}"#, 0.0, 0, None, vec![], 1),
        (r#"{"tick":0,"frameIndex":-1,"selectedUid":-1,"selectedUids":[-1,2.75],"selectedJoint":-1}"#, 0.0, 0, Some(0), vec![0, 2], -1),
        (r#"{"tick":1e400,"frameIndex":0,"selectedUid":1,"selectedUids":[1],"selectedJoint":-1}"#, f64::INFINITY, 0, Some(1), vec![1], -1),
    ] {
        let mut session = session_from_recorded_state(&reference, first.get("state").unwrap());
        let report = session.report().clone();
        let answer = session.call_values("updateProject", &parse("[{}]"), &parse(view));
        assert_eq!(answer.get("status").and_then(Value::as_str), Some("ok"), "{view}: {answer:?}");
        assert_eq!(answer.get("result"), Some(&Value::Bool(false)));
        assert_eq!(session.editor.tick.to_bits(), tick.to_bits(), "{view}: tick bits");
        assert_eq!(session.editor.frame_index, frame); assert_eq!(session.editor.selection.uid, uid);
        assert_eq!(session.editor.selection.uids, uids); assert_eq!(session.editor.selection.joint, joint);
        assert!(session.report().same_node(&report));
        assert_eq!(answer.get("events").and_then(Value::array_len), Some(0));
    }
}

#[test]
fn optional_view_sync_preserves_present_stale_and_explicit_null_metadata() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/edit");
    for mode in ["native", "sheet"] {
        let reference = parse(&std::fs::read_to_string(root.join(format!("selection-metadata-plain-{mode}.reference.json"))).unwrap());
        let steps = reference.get("steps").unwrap().elements().unwrap();
        for key in ["polygonSelection", "segmentIds", "branch"] {
            let state = steps.iter().find_map(|step| step.get("state").filter(|state| state.get(key).is_some()).cloned()).unwrap();
            let mut session = session_from_recorded_state(&reference, &state);
            let raw = session.editor.selection_metadata.clone();
            let shown = session.editor.state_value();
            let answer = session.call_values("updateProject", &parse("[{}]"), &shown);
            assert_eq!(answer.get("status").and_then(Value::as_str), Some("ok"));
            assert_eq!(raw, session.editor.selection_metadata, "{mode}/{key}: canonical repeated metadata");
            assert_eq!(answer.get("state"), Some(&shown));
            let mut view = shown.as_obj().unwrap().clone(); view.set(key, Value::Null);
            let answer = session.call_values("updateProject", &parse("[{}]"), &Value::object(view));
            assert_eq!(answer.get("status").and_then(Value::as_str), Some("ok"));
            assert!(session.editor.selection_metadata.polygon.is_none());
            assert!(session.editor.selection_metadata.segments.is_none());
            assert!(session.editor.selection_metadata.branch.is_none());
            assert!(answer.get("state").unwrap().get(key).is_none());
        }
        let state = steps.iter().find_map(|step| step.get("state").filter(|state| state.get("branch").is_some()).cloned()).unwrap();
        let mut session = session_from_recorded_state(&reference, &state);
        let mut branch = session.editor.selection_metadata.branch.as_ref().unwrap().as_obj().unwrap().clone();
        branch.set("unknown", parse(r#"{"retain":true}"#)); session.editor.selection_metadata.branch = Some(Value::object(branch));
        let original = session.editor.selection_metadata.clone();
        session.editor.selection.joint = -1;
        let hidden = session.editor.state_value(); assert!(hidden.get("branch").is_none());
        let answer = session.call_values("updateProject", &parse("[{}]"), &hidden);
        assert_eq!(answer.get("status").and_then(Value::as_str), Some("ok"));
        assert_eq!(original, session.editor.selection_metadata, "{mode}: absent canonical metadata retains hidden stale fields");
        let mut view = hidden.as_obj().unwrap().clone(); view.set("branch", Value::Null);
        let answer = session.call_values("updateProject", &parse("[{}]"), &Value::object(view));
        assert_eq!(answer.get("status").and_then(Value::as_str), Some("ok"));
        assert!(session.editor.selection_metadata.branch.is_none());
    }
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
        "channels-figure-sheet", "channels-movieclip-sheet", "channels-style-sheet", "property-clip-patches-sheet", "topology-sheet", "topology-native", "ik-movieclip-parent-sheet"] {
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
fn unpinned_pose_under_movieclip_uses_the_same_selected_and_ancestor_geometry() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/edit/ik-movieclip-parent-sheet.reference.json");
    let reference = parse(&std::fs::read_to_string(path).unwrap());
    let steps: Vec<Value> = reference.get("steps").unwrap().elements().unwrap().iter().collect();
    let mut checked_poses = 0;
    for compact in [false, true] {
        let mut session = session_from_recorded_state(&reference, steps[0].get("state").unwrap());
        let mut expected = steps[0].get("document").unwrap().as_str().unwrap().to_string();
        for step in &steps[1..] {
            let call: Vec<Value> = step.get("call").unwrap().elements().unwrap().iter().collect();
            let method = call[0].as_str().unwrap().trim_start_matches('@');
            if method == "poseJoint" {
                let (prepared, frame, full) = session.editor.evaluate().unwrap();
                let slot = frame.objects.iter().position(|object| object.uid == 2).unwrap();
                let parent = frame.objects[slot].join.unwrap().slot as usize;
                let figure = prepared.doc.figure(frame.objects[parent].figure).unwrap();
                assert!(figure.movieclip.is_some(), "the recorded ancestor is a movieclip");
                assert!(crate::ik::assembly_of(&prepared, &frame, slot).is_none(), "this pose takes the non-assembled bake_unpinned path");
                let controller = frame.objects[slot].ik.as_ref().unwrap().get(22).unwrap();
                assert!(controller.enabled && !controller.pinned);
                let small = styk_eval::geometry::build_only(&prepared.doc, &frame, &[slot]);
                for included in [parent, slot] {
                    let before = &full.objects[included];
                    let after = &small.objects[included];
                    assert!(before.resolved && after.resolved);
                    // Point-buffer offsets of unrelated objects are deliberately absent
                    // from this comparison; bake_unpinned consumes the selected root.
                    assert_eq!(before.root.map(f64::to_bits), after.root.map(f64::to_bits));
                    assert_eq!(format!("{:?}", (&before.starts, &before.ends, &before.lengths, &before.spatial, &before.rig)),
                        format!("{:?}", (&after.starts, &after.ends, &after.lengths, &after.spatial, &after.rig)));
                }
                checked_poses += 1;
            }
            let mut view = session.editor.state_value().as_obj().unwrap().clone();
            view.set("autoKey", Value::Bool(session.editor.options.auto_key));
            view.set("changesOnly", Value::Bool(compact));
            let answer = parse(&session.call(method, &json(&Value::array(call[1..].to_vec())), &json(&Value::object(view))));
            let label = format!("movieclip ancestor step {}, compact={compact}", step.get("step").unwrap());
            assert_eq!(answer.get("status").and_then(Value::as_str), Some("ok"), "{label}");
            assert_eq!(answer.get("state"), step.get("state"), "{label}: state");
            assert_eq!(answer.get("events"), step.get("events"), "{label}: events");
            if let Some(document) = step.get("document").and_then(Value::as_str) { expected = document.to_string(); }
            assert_eq!(session.editor.report_json(), expected, "{label}: document");
        }
    }
    assert_eq!(checked_poses, 6);
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

fn ik_capture_bits(capture: &crate::spatial_ik::Capture) -> String {
    let angles = |table: &std::collections::BTreeMap<u32, f64>| table.iter().map(|(&id, &angle)| (id, angle.to_bits())).collect::<Vec<_>>();
    let quaternions = |table: &std::collections::BTreeMap<u32, [f64; 4]>| table.iter().map(|(&id, q)| (id, q.map(f64::to_bits))).collect::<Vec<_>>();
    let reference = &capture.bend_reference;
    format!("{:?}", (
        capture.target.map(f64::to_bits), angles(&capture.followers),
        capture.joined_followers.as_ref().map(|parts| parts.iter().map(|(&id, followers)| (id, angles(followers))).collect::<Vec<_>>()),
        capture.bend_direction.to_bits(),
        ([reference.side, reference.angle, reference.axis, reference.ratio].map(f64::to_bits), reference.shape.as_ref().map(|shape| shape.iter().map(|n| n.to_bits()).collect::<Vec<_>>())),
        capture.spatial_followers.as_ref().map(quaternions),
        capture.joined_spatial_followers.as_ref().map(|parts| parts.iter().map(|(&id, followers)| (id, quaternions(followers))).collect::<Vec<_>>()),
        capture.spatial_target.as_ref().map(|target| (target.depth.to_bits(), target.pole.map(f64::to_bits), target.swivel.to_bits(), target.normals.map(|normals| normals.map(|n| n.map(f64::to_bits))))),
    ))
}

#[test]
fn same_call_ik_capture_matches_both_recorded_passes_and_refusal_order() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/edit");
    let mut checked = 0;
    for name in ["ik-capture-double-2d-sheet", "ik-capture-double-spatial-sheet"] {
        let reference = parse(&std::fs::read_to_string(root.join(format!("{name}.reference.json"))).unwrap());
        let ordinary = crate::replay::replay(&reference).unwrap();
        assert!(ordinary.mismatches.is_empty() && ordinary.unported.is_none(), "{name}: {ordinary:?}");
        assert_eq!(ordinary.matched, 29);
        let steps: Vec<Value> = reference.get("steps").unwrap().elements().unwrap().iter().collect();
        for compact in [false, true] {
            let mut session = session_from_recorded_state(&reference, steps[0].get("state").unwrap());
            let mut expected = steps[0].get("document").unwrap().as_str().unwrap().to_string();
            for step in &steps[1..] {
                let call: Vec<Value> = step.get("call").unwrap().elements().unwrap().iter().collect();
                let method = call[0].as_str().unwrap();
                let label = format!("{name}/{} compact={compact}", step.get("step").unwrap());
                if method == "@frame" {
                    let frame = session.editor.shown_frame().unwrap();
                    let snapshot = Value::str(&json(&styk_eval::records::snapshot_pose_frame(&frame)));
                    assert_eq!(Some(&snapshot), step.get("result"), "{label}: visible pose");
                } else {
                    let before = session.editor.report().clone(); let drag = session.editor.drag_value();
                    let mut view = session.editor.state_value().as_obj().unwrap().clone();
                    view.set("autoKey", Value::Bool(session.editor.options.auto_key));
                    view.set("changesOnly", Value::Bool(compact));
                    view.set("draws", step.get("draws").cloned().unwrap_or(Value::Null));
                    let answer = session.call_values(method.trim_start_matches('@'), &Value::array(call[1..].to_vec()), &Value::object(view));
                    if let Some(error) = step.get("error").filter(|e| !e.is_null()) {
                        assert_eq!(answer.get("status").and_then(Value::as_str), Some("error"), "{label}");
                        assert_eq!(answer.get("error"), error.get("key"), "{label}: validation priority");
                        assert!(session.editor.report().same_node(&before), "{label}: refused report identity");
                        assert_eq!(session.editor.drag_value(), drag, "{label}: refused gesture");
                    } else {
                        assert_eq!(answer.get("status").and_then(Value::as_str), Some("ok"), "{label}: {answer:?}");
                        // The recorder and ordinary replay compare the same depth-4 view.
                        // Full document/state/events and visible frames remain exact below.
                        let returned = crate::replay::plain(answer.get("result").unwrap_or(&Value::Null), 4).unwrap_or(Value::Null);
                        assert_eq!(Some(&returned), step.get("result"), "{label}: recorded result");
                        if method == "setNodeIK" && answer.get("result") == Some(&Value::Bool(false)) {
                            assert!(session.editor.report().same_node(&before), "{label}: false result report identity");
                            assert_eq!(session.editor.drag_value(), drag, "{label}: false result gesture");
                        }
                    }
                    assert_eq!(answer.get("events"), step.get("events"), "{label}: events");
                }
                if let Some(document) = step.get("document").and_then(Value::as_str) { expected = document.to_string(); }
                assert_eq!(session.editor.report_json(), expected, "{label}: report/history data");
                assert_eq!(Some(&session.editor.state_value()), step.get("state"), "{label}: selection/clock/history");
                checked += 1;
            }
        }
    }
    assert_eq!(checked, 116);
}

#[test]
fn same_call_ik_capture_borrows_full_solutions_for_rig_spatial_and_joined_ancestors() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/edit");
    let (mut compared, mut spatial, mut pinned, mut ancestor, mut retained_2d, mut retained_3d) = (0, 0, 0, 0, 0, 0);
    for name in ["ik-settings-sheet", "ik-settings-pinned-sheet", "ik-settings-spatial-sheet", "ik-settings-followers-sheet", "ik-settings-rig-sheet", "ik-unpinned-sheet", "ik-spatial-free-sheet", "ik-movieclip-parent-sheet"] {
        let reference = parse(&std::fs::read_to_string(root.join(format!("{name}.reference.json"))).unwrap());
        let steps: Vec<Value> = reference.get("steps").unwrap().elements().unwrap().iter().collect();
        let mut session = session_from_recorded_state(&reference, steps[0].get("state").unwrap());
        let mut expected = steps[0].get("document").unwrap().as_str().unwrap().to_string();
        for step in &steps[1..] {
            let call: Vec<Value> = step.get("call").unwrap().elements().unwrap().iter().collect();
            let method = call[0].as_str().unwrap().trim_start_matches('@');
            let label = format!("{name}/{} {method}", step.get("step").unwrap());
            let prior = session.editor.drag.as_ref().map(|drag| drag.ik_captures.iter().map(|(key, capture)| (key.clone(), ik_capture_bits(capture))).collect::<std::collections::BTreeMap<_, _>>()).unwrap_or_default();
            if matches!(method, "poseJoint" | "setNodeIK") && let Some((slot, object)) = session.editor.object().unwrap() {
                let (prepared, frame, scene) = session.editor.evaluate_only(Some(&[slot])).unwrap();
                let id = crate::record::uid(&object).unwrap();
                let at = frame.objects.iter().position(|o| o.uid == id).unwrap();
                let evaluated = &frame.objects[at];
                let figure = evaluated.figure_override.as_deref().or_else(|| prepared.doc.figure(evaluated.figure).map(|figure| &**figure)).unwrap();
                if let Some(segment) = figure.segments.get(session.editor.selection.joint as usize)
                    && let Some(controller) = styk_eval::ik::parse::table(object.get("_stykIK")).and_then(|table| table.get(segment.id).cloned())
                {
                    let solved = crate::ik::solve_frame(&prepared, &frame, &scene);
                    let mut keys = solved.keys().copied().collect::<Vec<_>>();keys.sort_unstable();
                    let ordinary = session.editor.capture_limb(slot, segment.id).unwrap();
                    let borrowed = session.editor.capture_limb_from(slot, segment.id, &prepared, &frame, &scene, &solved).unwrap();
                    assert_eq!(ik_capture_bits(&borrowed), ik_capture_bits(&ordinary), "{label}: complete capture numeric bits");
                    let mut after = solved.keys().copied().collect::<Vec<_>>();after.sort_unstable();assert_eq!(keys, after);
                    compared += 1;spatial += usize::from(borrowed.spatial_target.is_some());pinned += usize::from(controller.pinned && !solved.is_empty());
                    ancestor += usize::from(evaluated.join.is_some());
                }
            }
            let mut view = session.editor.state_value().as_obj().unwrap().clone();view.set("autoKey", Value::Bool(session.editor.options.auto_key));
            view.set("draws", step.get("draws").cloned().unwrap_or(Value::Null));
            let args = step.get("boundsArgs").cloned().unwrap_or_else(|| Value::array(call[1..].to_vec()));
            let answer = session.call_values(method, &args, &Value::object(view));
            assert_ne!(answer.get("status").and_then(Value::as_str), Some("unported"), "{label}: {answer:?}");
            if let Some(error) = step.get("error").filter(|e| !e.is_null()) {
                assert_eq!(answer.get("error"), error.get("key"), "{label}: error");
            } else { assert_eq!(answer.get("status").and_then(Value::as_str), Some("ok"), "{label}: {answer:?}"); }
            assert_eq!(answer.get("state"), step.get("state"), "{label}: state");assert_eq!(answer.get("events"), step.get("events"), "{label}: events");
            if let Some(document) = step.get("document").and_then(Value::as_str) { expected = document.to_string(); }
            assert_eq!(session.editor.report_json(), expected, "{label}: report");
            if method == "poseJoint" && let Some(drag) = &session.editor.drag {
                for (key, capture) in &drag.ik_captures {
                    if let Some(before) = prior.get(key) {
                        assert_eq!(&ik_capture_bits(capture), before, "{label}: stored gesture baseline is unchanged");
                        if capture.spatial_target.is_some() { retained_3d += 1; } else { retained_2d += 1; }
                    }
                }
            }
        }
    }
    assert!(compared >= 8 && spatial > 0 && pinned > 0 && ancestor > 0, "capture coverage: {compared}/{spatial}/{pinned}/{ancestor}");
    assert!(retained_2d > 0 && retained_3d > 0, "repeated capture lifecycle: 2D={retained_2d}, 3D={retained_3d}");
}

fn check_recorded_shape_angle(editor: &Editor, label: &str, counts: &mut [usize; 4]) {
    let Some(selected) = editor.selection.uid else { return };
    if editor.selection.joint < 0 { return; }
    let slot = editor.selection_frame().unwrap().iter().position(|object| crate::record::uid(object) == Some(selected));
    let (prepared, frame, scene) = editor.evaluate_only(slot.as_ref().map(std::slice::from_ref)).unwrap();
    let Some(slot) = frame.objects.iter().position(|object| object.uid == selected) else { return };
    let Some(geometry) = scene.objects.iter().find(|geometry| geometry.uid == selected && geometry.resolved) else { return };
    let object = &frame.objects[slot];
    let Some(figure) = object.figure_override.as_ref().or_else(|| prepared.doc.figure(object.figure)) else { return };
    let joint = editor.selection.joint as usize;
    if figure.segments.get(joint).is_none() { return; }
    let solved = crate::ik::solve_frame(&prepared, &frame, &scene);
    let old = crate::ik::displayed_angle(&prepared, &frame, &scene, &solved, slot, joint);
    let next = crate::ik::shape_angle(&prepared, &frame, &scene, slot, joint);
    assert_eq!(old.to_bits(), next.to_bits(), "{label}: selected shape angle bits");
    counts[0] += 1;
    counts[1] += usize::from(frame.objects.iter().any(|object| styk_eval::ik::has_pinned(object)));
    counts[2] += usize::from(styk_eval::ik::joined::components(&frame).iter().any(|slots| slots.len() > 1 && slots.contains(&slot)));
    counts[3] += usize::from(geometry.rig.is_some());
}

#[test]
fn selected_shape_angle_matches_recorded_pinned_joined_and_ordinary_states() {
    let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/edit");
    let mut counts = [0usize; 4];
    let mut checked = 0usize;
    for name in ["startup-sheet", "rig-sheet", "gestures-sheet", "pinned-ik-sheet", "ik-followers-sheet", "ik-joined-sheet", "ik-joined-late-sheet", "ik-settings-rig-sheet", "ik-spatial-joined-free-sheet", "ik-movieclip-parent-sheet", "origin-pose-support-free-native", "origin-pose-support-free-sheet", "origin-pose-support-pinned-native", "origin-pose-support-pinned-sheet"] {
        let reference = parse(&std::fs::read_to_string(folder.join(format!("{name}.reference.json"))).unwrap());
        let steps: Vec<Value> = reference.get("steps").unwrap().elements().unwrap().iter().collect();
        let mut session = session_from_recorded_state(&reference, steps[0].get("state").unwrap());
        let mut expected = steps[0].get("document").unwrap().as_str().unwrap().to_string();
        check_recorded_shape_angle(&session.editor, &format!("{name}/0"), &mut counts);
        for step in &steps[1..] {
            let call: Vec<Value> = step.get("call").unwrap().elements().unwrap().iter().collect();
            let method = call[0].as_str().unwrap().trim_start_matches('@');
            let label = format!("{name}/{} {method}", step.get("step").unwrap());
            let args = step.get("boundsArgs").cloned().unwrap_or_else(|| Value::array(call[1..].to_vec()));
            let mut view = session.editor.state_value().as_obj().unwrap().clone();
            view.set("autoKey", Value::Bool(session.editor.options.auto_key));
            view.set("draws", step.get("draws").cloned().unwrap_or(Value::Null));
            let answer = session.call_values(method, &args, &Value::object(view));
            if let Some(error) = step.get("error").filter(|error| !error.is_null()) {
                assert_eq!(answer.get("error"), error.get("key"), "{label}: error priority");
            } else {
                assert_eq!(answer.get("status").and_then(Value::as_str), Some("ok"), "{label}: {answer:?}");
            }
            assert_eq!(answer.get("events"), step.get("events"), "{label}: events");
            if let Some(document) = step.get("document").and_then(Value::as_str) { expected = document.to_string(); }
            assert_eq!(session.editor.report_json(), expected, "{label}: complete document/history");
            assert_eq!(Some(&session.editor.state_value()), step.get("state"), "{label}: clock/selection/history");
            check_recorded_shape_angle(&session.editor, &label, &mut counts);
            checked += 1;
        }
    }
    assert_eq!(checked, 559);
    assert!(counts[0] > 20 && counts[1] > 0 && counts[2] > 0 && counts[3] > 0, "real state coverage: {counts:?}");
    eprintln!("SELECTED_ANGLE_RECORDED steps={checked} angles={} pinned={} joined={} rig={}", counts[0], counts[1], counts[2], counts[3]);
}

#[test]
fn selected_shape_angle_preserves_duplicate_uid_and_conflict_fallbacks() {
    // Existing Editor-built evaluation fixture: genuine competing pinned chains.
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../styk-eval/tests/fixtures/ik/conflict.json");
    let editor = Editor::new(parse(&std::fs::read_to_string(path).unwrap()), crate::Options::default());
    let (prepared, frame, _) = editor.evaluate().unwrap();
    let mut conflicts = 0usize;
    let mut compared = 0usize;
    for (slot, object) in frame.objects.iter().enumerate() {
        let Some(figure) = object.figure_override.as_ref().or_else(|| prepared.doc.figure(object.figure)) else { continue };
        let scene = styk_eval::geometry::build_only(&prepared.doc, &frame, &[slot]);
        let solved = crate::ik::solve_frame(&prepared, &frame, &scene);
        conflicts += solved.values().flat_map(|solved| &solved.solution.results).filter(|(_, outcome)| outcome.conflict).count();
        for index in 0..figure.segments.len() {
            assert_eq!(crate::ik::displayed_angle(&prepared, &frame, &scene, &solved, slot, index).to_bits(), crate::ik::shape_angle(&prepared, &frame, &scene, slot, index).to_bits(), "actual conflict slot{slot}/joint{index}");
            compared += 1;
        }
    }
    assert!(conflicts > 0 && compared > 0, "actual conflict coverage {conflicts}/{compared}");
    // Derived identity controls ONLY: preserve every source/angle/controller of a
    // real joined recording; alias one typed UID to expose global map semantics.
    // This is not represented as a new valid Editor document or new D15 tape.
    let reference = parse(&std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/edit/ik-joined-sheet.reference.json")).unwrap());
    let steps: Vec<Value> = reference.get("steps").unwrap().elements().unwrap().iter().collect();
    let session = session_from_recorded_state(&reference, steps[0].get("state").unwrap());
    let (prepared, frame, scene) = session.editor.evaluate().unwrap();
    assert!(frame.objects.len() >= 2);
    let mut duplicate = 0usize;
    for aliased_slot in [0usize, frame.objects.len()-1] {
        let other = if aliased_slot == 0 { frame.objects.len()-1 } else { 0 };
        let mut aliased = frame.clone();
        let duplicate_uid = aliased.objects[aliased_slot].uid;
        std::sync::Arc::make_mut(&mut aliased.objects[other]).uid = duplicate_uid;
        let solved = crate::ik::solve_frame(&prepared, &aliased, &scene);
        let object = &aliased.objects[aliased_slot];
        let figure = object.figure_override.as_ref().or_else(|| prepared.doc.figure(object.figure)).unwrap();
        for index in 0..figure.segments.len() {
            assert_eq!(crate::ik::displayed_angle(&prepared, &aliased, &scene, &solved, aliased_slot, index).to_bits(), crate::ik::shape_angle(&prepared, &aliased, &scene, aliased_slot, index).to_bits(), "derived duplicate UID slot{aliased_slot}/joint{index}");
            duplicate += 1;
        }
    }
    assert!(duplicate > 0);
    eprintln!("SELECTED_ANGLE_EDGE conflicts={conflicts} compared={compared} duplicate={duplicate}");
}
