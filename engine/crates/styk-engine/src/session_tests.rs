use super::*;
use crate::{Budgets, mirror::Change};
use styk_doc::{Value, json};

fn clock() -> f64 {
    0.0
}
fn parsed(text: &str) -> Value {
    json::parse(text.as_bytes()).unwrap()
}
fn fixture(path: &str) -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests").join(path);
    parsed(&std::fs::read_to_string(path).unwrap())
}
fn open(document: Value) -> (Engine, Handle, Stamp, Epochs) {
    let engine = Engine::new(clock, Budgets::for_device(None));
    engine.stage(0, Change::Reset).unwrap();
    engine
        .stage(
            0,
            Change::Properties(json::to_string(&document).into_bytes()),
        )
        .unwrap();
    engine.commit(0, 0).unwrap();
    let (_, tree, loader, prepared) = engine.mirrored_for_editing().unwrap();
    let session = Session::start_prepared(tree, "{}", loader, prepared).unwrap();
    let epochs = Epochs {
        shown: 51,
        view: 2,
        gesture: 0,
    };
    let (handle, stamp) = engine
        .attach_session_worker(session, HistoryLimits::default(), 0.0, epochs)
        .unwrap_or_else(|(error, _)| panic!("{error}"));
    (engine, handle, stamp, epochs)
}
fn empty() -> (Engine, Handle, Stamp, Epochs) {
    open(parsed(
        r#"{"version":5,"canvas_width":320,"canvas_height":200,"fps":20,"figure_types":[],"keyframes":[{"objects":[],"duration_raw":2}],"backgrounds":[],"_styk":{"future":{"raw":"keep"}}}"#,
    ))
}
fn native(dense: bool) -> (Engine, Handle, Stamp, Epochs) {
    let reference = fixture("edit/move-native.reference.json");
    let first = reference.get("steps").unwrap().item(0).unwrap();
    let mut document = parsed(first.get("document").unwrap().as_str().unwrap());
    if dense {
        let frames = document
            .make_obj()
            .unwrap()
            .get_mut("keyframes")
            .unwrap()
            .make_arr()
            .unwrap();
        frames[0]
            .make_obj()
            .unwrap()
            .get_mut("objects")
            .unwrap()
            .make_arr()
            .unwrap()[0]
            .make_obj()
            .unwrap()
            .set("futureDense", Value::nums(&vec![0.125; 30_000]));
    }
    open(document)
}
fn drain(engine: &Engine) {
    while engine.step() {}
}
// After terminal/failure disposal, only the actual receiver-ready raw head
// remains charged. Close/no-ready still require exactly zero, not a loose bound.
fn assert_current_raw_charge(engine: &Engine) {
    let core = engine.inner.session.lock().clone();
    let expected = core.as_ref().map_or(0, |core| {
        let worker = core.worker.lock().unwrap();
        worker.ready.as_ref().map_or(0, |ready| {
            styk_doc::retained_bytes_checked(&[&ready.record], |_| Ok::<_, ()>(())).unwrap() + 256
        })
    });
    assert_eq!(engine.inner.record_budget.retained_bytes(), expected);
}
fn owner_report(engine: &Engine) -> Value {
    let core = engine.inner.session.lock().clone().unwrap();
    let worker = core.worker.lock().unwrap();
    worker.session.as_ref().unwrap().report().clone()
}
// Native test reconstruction of the already-existing document-transport wire.
// App adoption will call its existing applyDocumentChunk owner and yield per ACK.
fn parent<'a>(value: &'a mut Value, path: &[Value]) -> &'a mut Value {
    if path.is_empty() {
        return value;
    }
    let child = if let Some(key) = path[0].as_str() {
        value.make_obj().unwrap().get_mut(key).unwrap()
    } else {
        &mut value.make_arr().unwrap()[path[0].as_f64().unwrap() as usize]
    };
    parent(child, &path[1..])
}
fn apply(mut value: Value, text: &str) -> Value {
    assert!(text.len() <= CHUNK_BYTES);
    let chunk = parsed(text);
    if let Some(header) = chunk.get("header") {
        return header.clone();
    }
    for op in chunk.get("operations").unwrap().elements().unwrap().iter() {
        let path: Vec<_> = op.get("path").unwrap().elements().unwrap().iter().collect();
        let node = parent(&mut value, &path[..path.len() - 1]);
        let key = path.last().unwrap();
        let mut next = if let Some(key) = key.as_str() {
            node.get(key).cloned()
        } else {
            node.item(key.as_f64().unwrap() as usize)
        };
        let next = if let Some(part) = op.get("extend") {
            let mut old = next.take().unwrap();
            old.make_arr()
                .unwrap()
                .extend(part.elements().unwrap().iter());
            old
        } else if let Some(part) = op.get("append") {
            Value::str(&format!(
                "{}{}",
                next.take().unwrap().text().unwrap(),
                part.text().unwrap()
            ))
        } else {
            op.get("value").unwrap().clone()
        };
        if let Some(key) = key.as_str() {
            node.make_obj().unwrap().set(key, next);
        } else {
            let array = node.make_arr().unwrap();
            let at = key.as_f64().unwrap() as usize;
            if at >= array.len() {
                array.resize(at + 1, Value::Null);
            }
            array[at] = next;
        }
    }
    value
}
fn staged(engine: &Engine, handle: &Handle, request: u64) -> (Value, usize) {
    let mut value = Value::object(styk_doc::Obj::new());
    let mut count = 0;
    for _ in 0..10_000 {
        drain(engine);
        let poll = handle.poll();
        assert_eq!(poll.request, request);
        if let Some(error) = poll.error {
            panic!("{error:?}");
        }
        if let Some(packet) = poll.packet {
            assert_eq!(packet.request, request);
            value = apply(value, &packet.json);
            count += 1;
            assert!(handle.ack_packet(request, packet.sequence).unwrap());
        } else if poll.phase == Phase::Ready {
            return (value, count);
        }
    }
    panic!("stream did not reach readiness")
}
fn commit(engine: &Engine, handle: &Handle, request: u64, epochs: Epochs) -> Success {
    assert!(handle.ready(request, epochs).unwrap());
    drain(engine);
    let poll = handle.poll();
    assert_eq!(poll.phase, Phase::Success);
    assert!(poll.error.is_none());
    poll.success.unwrap()
}

#[test]
fn quiet_owner_stays_unchanged_until_ready_and_retires_only_after_terminal_ack() {
    let (engine, handle, mut stamp, epochs) = empty();
    for (method, args, fps, can_undo, can_redo) in [
        ("setFPS", "[31]", 31.0, true, false),
        ("setFPS", "[27]", 27.0, true, false),
        ("undo", "[]", 31.0, true, true),
        ("redo", "[]", 27.0, true, false),
    ] {
        let before = owner_report(&engine);
        let generation = engine.generation().unwrap();
        let request = handle.submit(stamp, epochs, method, args, "null").unwrap();
        assert!(owner_report(&engine).same_node(&before));
        let (answer, _) = staged(&engine, &handle, request);
        assert_eq!(answer.get("status").and_then(Value::as_str), Some("ok"));
        assert_eq!(
            answer.path(&["state", "canUndo"]),
            Some(&Value::Bool(can_undo))
        );
        assert_eq!(
            answer.path(&["state", "canRedo"]),
            Some(&Value::Bool(can_redo))
        );
        assert!(owner_report(&engine).same_node(&before));
        assert!(engine.mirrored().unwrap().1.same_node(&before));
        assert!(Arc::ptr_eq(&generation, &engine.generation().unwrap()));
        assert!(engine.inner.record_budget.retained_bytes() > 0);
        let success = commit(&engine, &handle, request, epochs);
        stamp = success.stamp;
        assert_eq!(
            owner_report(&engine).get("fps").and_then(Value::as_f64),
            Some(fps)
        );
        assert!(owner_report(&engine).same_node(&engine.mirrored().unwrap().1));
        assert!(engine.inner.record_budget.retained_bytes() > 0);
        assert_eq!(
            handle.submit(stamp, epochs, "undo", "[]", "null"),
            Err(Error::Busy)
        );
        assert!(handle.terminal(request).unwrap());
        drain(&engine);
        assert_eq!(handle.poll().phase, Phase::Idle);
        assert_current_raw_charge(&engine);
    }
}

#[test]
fn sparse_project_response_refuses_absent_settings_and_other_history_without_mutation() {
    let mut absent = parsed(r#"{"version":5,"canvas_width":320,"canvas_height":200,"figure_types":[],"keyframes":[{"objects":[],"duration_raw":2}],"backgrounds":[],"future":null}"#);
    for non_fields in [false, true] {
        if non_fields { absent.make_obj().unwrap().set("fps", Value::Num(20.0)); }
        let (engine, handle, mut stamp, epochs) = open(absent.clone());
        if non_fields {
            let request = handle.submit(stamp, epochs, "addPaletteColor", "[\"#123456\"]", "null").unwrap();
            let (answer, _) = staged(&engine, &handle, request);
            assert_eq!(answer.get("status").and_then(Value::as_str), Some("ok"));
            stamp = commit(&engine, &handle, request, epochs).stamp;
            handle.terminal(request).unwrap(); drain(&engine);
        }
        let before = owner_report(&engine);
        let generation = engine.generation().unwrap();
        let core = handle.core.upgrade().unwrap();
        let checkpoint = core.worker.lock().unwrap().session.as_ref().unwrap().history_checkpoint_worker();
        let request = if non_fields { handle.submit_project_fields(stamp, epochs, "undo", "[]", "null") }
            else { handle.submit_project_fields(stamp, epochs, "setFPS", "[31]", "null") }.unwrap();
        drain(&engine);
        let poll = handle.poll(); assert_eq!(poll.phase, Phase::Failed);
        let reason = poll.error.unwrap().to_string();
        assert!(reason.contains(if non_fields { "history outside" } else { "absent-before" }), "{reason}");
        assert!(owner_report(&engine).same_node(&before));
        assert!(Arc::ptr_eq(&generation, &engine.generation().unwrap()));
        assert_eq!(core.worker.lock().unwrap().session.as_ref().unwrap().history_checkpoint_worker(), checkpoint);
        assert_current_raw_charge(&engine);
        handle.terminal(request).unwrap(); drain(&engine);
        let request = handle.submit_project_fields(stamp, epochs, "setFPS", "[31]", "{}").unwrap();
        drain(&engine);assert_eq!(handle.poll().error, Some(Error::InvalidInput));
        assert!(owner_report(&engine).same_node(&before));
        assert!(Arc::ptr_eq(&generation, &engine.generation().unwrap()));
        handle.terminal(request).unwrap(); drain(&engine);
    }
}

#[test]
fn one_packet_ack_dense_single_figure_and_bounded_queue_do_not_walk_on_submit() {
    let (engine, handle, stamp, epochs) = native(true);
    let before = owner_report(&engine);
    let request = handle
        .submit(stamp, epochs, "select", "[1,-1]", "null")
        .unwrap();
    assert!(owner_report(&engine).same_node(&before));
    assert_eq!(handle.poll().phase, Phase::Preparing);
    assert_eq!(
        handle.submit(stamp, epochs, "select", "[2,-1]", "null"),
        Err(Error::Busy)
    );
    assert_eq!(handle.ack_packet(request, 0), Err(Error::Ack));
    drain(&engine);
    let packet = handle.poll().packet.unwrap();
    assert_eq!(
        handle.ack_packet(request, packet.sequence + 1),
        Err(Error::Ack)
    );
    assert!(handle.ack_packet(request, packet.sequence).unwrap());
    assert_eq!(handle.ack_packet(request, packet.sequence), Err(Error::Ack));
    let (answer, count) = staged(&engine, &handle, request);
    assert!(
        count > 20,
        "dense single object crosses bounded packet ACKs: {count}"
    );
    assert_eq!(
        answer
            .path(&["result", "futureDense"])
            .and_then(Value::array_len),
        Some(30_000)
    );
    commit(&engine, &handle, request, epochs);
    assert!(handle.terminal(request).unwrap());
    drain(&engine);
    assert_current_raw_charge(&engine);
    let oversized = " ".repeat(MAX_SESSION_CONTROL_BYTES);
    assert_eq!(
        handle.submit(stamp, epochs, "select", &oversized, "null"),
        Err(Error::InputBudget)
    );
}

#[test]
fn cancellation_at_queued_streaming_and_ready_preserves_same_session_and_engine() {
    for phase in [Phase::Preparing, Phase::Streaming, Phase::Ready] {
        let (engine, handle, stamp, epochs) = empty();
        let before = owner_report(&engine);
        let generation = engine.generation().unwrap();
        let request = handle
            .submit(stamp, epochs, "setFPS", "[31]", "null")
            .unwrap();
        if phase == Phase::Streaming {
            drain(&engine);
            assert!(handle.poll().packet.is_some());
        }
        if phase == Phase::Ready {
            staged(&engine, &handle, request);
        }
        handle.cancel(request);
        drain(&engine);
        let poll = handle.poll();
        assert_eq!(poll.phase, Phase::Failed);
        assert_eq!(poll.error, Some(Error::Cancelled));
        assert!(owner_report(&engine).same_node(&before));
        assert!(engine.mirrored().unwrap().1.same_node(&before));
        assert!(Arc::ptr_eq(&generation, &engine.generation().unwrap()));
        assert_current_raw_charge(&engine);
        assert!(handle.terminal(request).unwrap());
        drain(&engine);
        let next = handle
            .submit(stamp, epochs, "setFPS", "[29]", "null")
            .unwrap();
        staged(&engine, &handle, next);
        commit(&engine, &handle, next, epochs);
        assert!(handle.terminal(next).unwrap());
        drain(&engine);
    }
}

#[test]
fn postcommit_cancel_retains_both_heads_until_ack_without_rollback_or_reexecution() {
    let (engine, handle, stamp, epochs) = empty();
    let request = handle
        .submit(stamp, epochs, "setFPS", "[31]", "null")
        .unwrap();
    staged(&engine, &handle, request);
    let success = commit(&engine, &handle, request, epochs);
    let after = owner_report(&engine);
    let generation = engine.generation().unwrap();
    handle.cancel(request);
    drain(&engine);
    let poll = handle.poll();
    assert_eq!(poll.phase, Phase::Success);
    assert_eq!(poll.success, Some(success));
    assert!(owner_report(&engine).same_node(&after));
    assert!(Arc::ptr_eq(&generation, &engine.generation().unwrap()));
    assert!(engine.inner.record_budget.retained_bytes() > 0);
    assert!(handle.terminal(request).unwrap());
    drain(&engine);
    assert_current_raw_charge(&engine);
}

#[test]
fn stale_readiness_epochs_and_invalid_input_refuse_atomically_then_terminal_resets() {
    let (engine, handle, stamp, epochs) = empty();
    let before = owner_report(&engine);
    let request = handle
        .submit(stamp, epochs, "setFPS", "[31]", "null")
        .unwrap();
    staged(&engine, &handle, request);
    assert!(
        handle
            .ready(
                request,
                Epochs {
                    view: epochs.view + 1,
                    ..epochs
                }
            )
            .unwrap()
    );
    drain(&engine);
    let poll = handle.poll();
    assert_eq!(poll.error, Some(Error::Stale));
    assert!(owner_report(&engine).same_node(&before));
    assert!(handle.terminal(request).unwrap());
    drain(&engine);
    let request = handle.submit(stamp, epochs, "setFPS", "[", "null").unwrap();
    drain(&engine);
    assert_eq!(handle.poll().error, Some(Error::InvalidInput));
    assert!(owner_report(&engine).same_node(&before));
    assert!(handle.terminal(request).unwrap());
    drain(&engine);
}

#[test]
fn quiet_actor_cannot_leave_a_live_gesture_or_duplicate_the_editor_owner() {
    let (engine, handle, stamp, epochs) = native(false);
    let before = owner_report(&engine);
    let request = handle
        .submit(
            stamp,
            epochs,
            "beginDrag",
            "[]",
            r#"{"selectedUid":1,"selectedUids":[1],"selectedJoint":-1}"#,
        )
        .unwrap();
    drain(&engine);
    let error = handle.poll().error.unwrap();
    assert!(matches!(error,Error::Owner(ref text) if text.contains("live editing gesture")));
    assert!(owner_report(&engine).same_node(&before));
    assert!(handle.terminal(request).unwrap());
    drain(&engine);
    let (_, tree, loader, prepared) = engine.mirrored_for_editing().unwrap();
    let other = Session::start_prepared(tree, "{}", loader, prepared).unwrap();
    assert!(matches!(
        engine.attach_session_worker(other, HistoryLimits::default(), 0.0, epochs),
        Err((Error::Busy, _))
    ));
}

#[test]
fn stop_and_new_lineage_dispose_unacked_cursor_and_session_on_durable_worker() {
    for stop in [false, true] {
        let (engine, handle, stamp, epochs) = native(true);
        let request = handle
            .submit(stamp, epochs, "select", "[1,-1]", "null")
            .unwrap();
        drain(&engine);
        assert!(handle.poll().packet.is_some());
        assert!(engine.inner.record_budget.retained_bytes() > 0);
        if stop {
            engine.stop();
        } else {
            engine.stage(0, Change::Reset).unwrap();
            engine.stage(0,Change::Properties(br#"{"version":5,"canvas_width":20,"canvas_height":20,"fps":20,"figure_types":[],"keyframes":[],"backgrounds":[]}"#.to_vec())).unwrap();
            engine.commit(0, 0).unwrap();
        }
        assert!(
            engine.inner.session.lock().is_some(),
            "UI stop/replacement cannot drop owning Session"
        );
        drain(&engine);
        assert!(engine.inner.session.lock().is_none());
        assert_eq!(handle.poll().phase, Phase::Closed);
        assert_current_raw_charge(&engine);
        assert_eq!(
            handle.submit(stamp, epochs, "setFPS", "[31]", "null"),
            Err(Error::Closed)
        );
        handle.cancel(request);
        drain(&engine);
    }
}

#[test]
fn stop_with_unacked_128_mib_figure_retires_cursor_and_charge_on_worker() {
    use crate::mirror::{RecordTarget, Table};
    let reference = fixture("edit/move-native.reference.json");
    let mut document = parsed(reference.get("steps").unwrap().item(0).unwrap().get("document").unwrap().as_str().unwrap());
    let mut frame = document.get("keyframes").unwrap().item(0).unwrap();
    let bytes = 128 * 1024 * 1024;
    let dense = Value::Nums(Arc::new(styk_doc::Nums::F64(vec![0.125 + f64::EPSILON; bytes / 8].into_boxed_slice())));
    frame.make_obj().unwrap().get_mut("objects").unwrap().make_arr().unwrap()[0]
        .make_obj().unwrap().set("futureDense", dense);
    // Native worker setup uses the existing resolved record owner, not a dense
    // full-object JSON string or a second document store on the interface.
    document.make_obj().unwrap().remove("keyframes");
    let engine = Engine::new(clock, Budgets::for_device(None));
    engine.stage(0, Change::Reset).unwrap();
    engine.stage(0, Change::Properties(json::to_string(&document).into_bytes())).unwrap();
    engine.stage(0, Change::Layout(Table::Keyframes, vec![-1])).unwrap();
    engine.stage(0, Change::Resolved(RecordTarget::Table(Table::Keyframes, 0), Some(frame))).unwrap();
    engine.commit(0, 0).unwrap();
    let (_, tree, loader, prepared) = engine.mirrored_for_editing().unwrap();
    let session = Session::start_prepared(tree, "{}", loader, prepared).unwrap();
    let epochs = Epochs::default();
    let (handle, stamp) = engine.attach_session_worker(session, HistoryLimits::default(), 0.0, epochs)
        .unwrap_or_else(|(error, _)| panic!("{error}"));
    handle.submit(stamp, epochs, "select", "[1,-1]", "null").unwrap();
    drain(&engine);
    let packet = handle.poll().packet.unwrap();assert!(packet.json.len() <= CHUNK_BYTES);
    assert!(engine.inner.record_budget.retained_bytes() >= bytes);
    engine.stop();
    assert!(engine.inner.session.lock().is_some(), "UI stop must not dispose the dense Session/cursor");
    drain(&engine);
    assert_eq!(handle.poll().phase, Phase::Closed);
    assert!(engine.inner.session.lock().is_none());
    assert_current_raw_charge(&engine);
}

#[test]
fn dropped_handle_is_weak_and_session_is_retired_by_worker_only() {
    let (engine, handle, _, _) = empty();
    let weak = handle.core.clone();
    drop(handle);
    assert!(engine.inner.session.lock().is_some());
    assert!(weak.upgrade().is_some());
    drain(&engine);
    assert!(engine.inner.session.lock().is_none());
    assert!(weak.upgrade().is_none());
}

#[test]
fn actual_nonzero_fresh_counters_and_warm_same_owner_move_preserve_undo_redo_and_saved_state() {
    for mode in ["native", "sheet"] {
        let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../tests/session-handoff/{mode}-fresh.json"));
        let reference = parsed(&std::fs::read_to_string(file).unwrap());
        let first = reference.get("steps").unwrap().item(0).unwrap();
        let original = first.get("history").unwrap();
        let counters = styk_edit::session::RevisionCounters {
            revision: original.get("revision").unwrap().as_f64().unwrap() as u64,
            serial: original.get("revisionSerial").unwrap().as_f64().unwrap() as u64,
            saved: original.get("savedRevision").unwrap().as_f64().unwrap() as u64,
        };
        assert!(counters.revision > 0);
        let engine = Engine::new(clock, Budgets::for_device(None));
        engine.stage(0, Change::Reset).unwrap();
        engine
            .stage(
                0,
                Change::Properties(
                    first
                        .get("document")
                        .unwrap()
                        .as_str()
                        .unwrap()
                        .as_bytes()
                        .to_vec(),
                ),
            )
            .unwrap();
        engine.commit(counters.revision, 0).unwrap();
        let (_, tree, loader, prepared) = engine.mirrored_for_editing().unwrap();
        let mut state = first.get("state").unwrap().clone();
        for (key, value) in reference.get("options").unwrap().as_obj().unwrap().iter() {
            state.make_obj().unwrap().set(key, value.clone());
        }
        let session = Session::start_worker_prepared(
            tree,
            &json::to_string(&state),
            loader,
            prepared,
            counters,
        )
        .unwrap();
        let epochs = Epochs::default();
        let (handle, mut stamp) = engine
            .attach_session_worker(
                session,
                HistoryLimits {
                    count: 2,
                    bytes: 48.0 * 1024.0 * 1024.0,
                },
                0.0,
                epochs,
            )
            .unwrap_or_else(|(error, _)| panic!("{error}"));
        for (method, args) in [("setFPS", "[31]"), ("setFPS", "[27]"), ("undo", "[]")] {
            let request = handle.submit(stamp, epochs, method, args, "null").unwrap();
            staged(&engine, &handle, request);
            stamp = commit(&engine, &handle, request, epochs).stamp;
            handle.terminal(request).unwrap();
            drain(&engine);
        }
        let before = owner_report(&engine);
        let (session, old_stamp, old_epochs) = engine.detach_session_worker().unwrap();
        assert!(session.report().same_node(&before));
        assert!(session.can_undo());
        assert!(session.can_redo());
        let checkpoint = session.history_checkpoint_worker().unwrap();
        assert!(checkpoint > 0.0);
        assert_eq!(old_stamp, stamp);
        assert_eq!(old_epochs, epochs);
        drop(handle);
        drain(&engine);
        let (handle, stamp) = engine
            .attach_session_worker(
                session,
                HistoryLimits {
                    count: 2,
                    bytes: 48.0 * 1024.0 * 1024.0,
                },
                checkpoint,
                epochs,
            )
            .unwrap_or_else(|(error, _)| panic!("{error}"));
        let request = handle.submit(stamp, epochs, "redo", "[]", "null").unwrap();
        let (answer, _) = staged(&engine, &handle, request);
        assert_eq!(answer.path(&["state", "canUndo"]), Some(&Value::Bool(true)));
        assert_eq!(
            answer.path(&["state", "canRedo"]),
            Some(&Value::Bool(false))
        );
        commit(&engine, &handle, request, epochs);
        handle.terminal(request).unwrap();
        drain(&engine);
        assert_eq!(
            owner_report(&engine).get("fps").and_then(Value::as_f64),
            Some(27.0)
        );
        let core = engine.inner.session.lock().clone().unwrap();
        let worker = core.worker.lock().unwrap();
        assert_eq!(
            worker.session.as_ref().unwrap().history_checkpoint_worker(),
            Some(checkpoint)
        );
    }
}

#[test]
fn recorded_quiet_editor_outcomes_history_and_exact_raw_document_match_every_ack_commit() {
    let mut calls = 0;
    for project_response in [false, true] {
    for mode in ["native", "sheet", "native-null", "sheet-null", "native-zero", "sheet-zero"] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../tests/session-quiet/{mode}.json"));
        let reference = parsed(&std::fs::read_to_string(path).unwrap());
        let steps: Vec<_> = reference
            .get("steps")
            .unwrap()
            .elements()
            .unwrap()
            .iter()
            .collect();
        let first = &steps[0];
        let history = first.get("history").unwrap();
        let revision = history.get("revision").unwrap().as_f64().unwrap() as u64;
        let engine = Engine::new(clock, Budgets::for_device(None));
        engine.stage(0, Change::Reset).unwrap();
        engine
            .stage(
                0,
                Change::Properties(
                    first
                        .get("document")
                        .unwrap()
                        .as_str()
                        .unwrap()
                        .as_bytes()
                        .to_vec(),
                ),
            )
            .unwrap();
        engine.commit(revision, 0).unwrap();
        let (_, tree, loader, prepared) = engine.mirrored_for_editing().unwrap();
        let mut state = first.get("state").unwrap().clone();
        for (key, value) in reference.get("options").unwrap().as_obj().unwrap().iter() {
            state.make_obj().unwrap().set(key, value.clone());
        }
        let session = Session::start_worker_prepared(
            tree,
            &json::to_string(&state),
            loader,
            prepared,
            styk_edit::session::RevisionCounters {
                revision,
                serial: history.get("revisionSerial").unwrap().as_f64().unwrap() as u64,
                saved: history.get("savedRevision").unwrap().as_f64().unwrap() as u64,
            },
        )
        .unwrap();
        let epochs = Epochs::default();
        let (handle, mut stamp) = engine
            .attach_session_worker(
                session,
                HistoryLimits {
                    count: reference.path(&["options", "historyLimit"]).unwrap().as_f64().unwrap() as usize,
                    bytes: 48.0 * 1024.0 * 1024.0,
                },
                0.0,
                epochs,
            )
            .unwrap_or_else(|(error, _)| panic!("{error}"));
        let mut expected_document = first.get("document").unwrap().as_str().unwrap().to_owned();
        for (index, step) in steps[1..].iter().enumerate() {
            let call: Vec<_> = step
                .get("call")
                .unwrap()
                .elements()
                .unwrap()
                .iter()
                .collect();
            let method = call[0].as_str().unwrap();
            let label = format!("{mode} step{} {method}", index + 1);
            let before = owner_report(&engine);
            let sparse = project_response && matches!(method, "setFPS" | "setCanvasSize" | "updateProject" | "undo" | "redo");
            let args = json::to_string(&Value::array(call[1..].to_vec()));
            let request = if sparse { handle.submit_project_fields(stamp, epochs, method, &args, "null") }
                else { handle.submit(stamp, epochs, method, &args, "null") }.unwrap();
            let (response, _) = staged(&engine, &handle, request);
            let answer = if sparse { response.get("answer").unwrap().clone() } else { response.clone() };
            let mut projected = before.clone();
            if sparse {
                assert_eq!(response.get("kind").and_then(Value::as_str), Some("project-fields"));
                let fields = response.get("fields").unwrap().elements().unwrap();
                assert_eq!(response.get("fields"), step.get("fields"), "{label}: actual JS Fields owner order");
                assert!(fields.len() <= 3);
                for field in fields.iter() {
                    let key = field.get("key").unwrap().as_str().unwrap();
                    if field.get("absent") == Some(&Value::Bool(true)) { projected.make_obj().unwrap().remove(key); }
                    else { projected.make_obj().unwrap().set(key, field.get("value").unwrap().clone()); }
                }
                for key in ["size", "revision", "revisionSerial", "savedRevision"] {
                    assert_eq!(response.path(&["history", key]), step.path(&["history", key]), "{label}: bounded history");
                }
                for key in ["undo", "redo"] {
                    assert_eq!(response.path(&["history", key]).and_then(Value::as_f64), Some(step.path(&["history", key]).unwrap().array_len().unwrap() as f64));
                }
                assert!(projected.get("keyframes").unwrap().same_node(before.get("keyframes").unwrap()));
                assert!(projected.get("__proto__").unwrap().same_node(before.get("__proto__").unwrap()));
            }
            let expected_error = step.get("error").unwrap();
            if expected_error.is_null() {
                assert_eq!(
                    answer.get("status").and_then(Value::as_str),
                    Some("ok"),
                    "{label}"
                );
                // Full evaluated views have the existing documented Session protocol;
                // command scalar results, state/events and raw publication stay exact.
                if !matches!(method, "select" | "selectMany" | "seek") {
                    assert_eq!(answer.get("result"), step.get("result"), "{label}: result");
                }
            } else {
                assert_eq!(
                    answer.get("status").and_then(Value::as_str),
                    Some("error"),
                    "{label}"
                );
                let error = expected_error
                    .get("key")
                    .filter(|value| !value.is_null())
                    .or_else(|| expected_error.get("message"));
                assert_eq!(answer.get("error"), error, "{label}: actual editor error");
            }
            assert_eq!(answer.get("state"), step.get("state"), "{label}: state");
            assert_eq!(answer.get("events"), step.get("events"), "{label}: events");
            let core = engine.inner.session.lock().clone().unwrap();
            {
                let worker = core.worker.lock().unwrap();
                let prepared = worker.pending.as_ref().unwrap().prepared.as_ref().unwrap();
                assert_eq!(
                    prepared.publication().history(),
                    step.get("history").unwrap(),
                    "{label}: exact history/checkpoint"
                );
            }
            assert!(
                owner_report(&engine).same_node(&before),
                "{label}: invisible staging mutated owner"
            );
            stamp = commit(&engine, &handle, request, epochs).stamp;
            if let Some(document) = step.get("document").and_then(Value::as_str) {
                expected_document = document.to_owned();
            }
            assert_eq!(
                json::to_string(&owner_report(&engine)),
                expected_document,
                "{label}: exact raw document"
            );
            if sparse { assert_eq!(json::to_string(&projected), expected_document, "{label}: sparse owning field order/absence"); }
            assert!(
                owner_report(&engine).same_node(&engine.mirrored().unwrap().1),
                "{label}: paired owner heads"
            );
            assert!(handle.terminal(request).unwrap());
            drain(&engine);
            assert_current_raw_charge(&engine);
            calls += 1;
        }
    }
    }
    assert_eq!(calls, 160);
}

#[test]
fn interface_controls_never_take_the_worker_session_lock_and_shared_slot_is_allocation_free() {
    let (engine, handle, stamp, epochs) = native(true);
    let core = handle.core.upgrade().unwrap();
    let guard = core.worker.lock().unwrap();
    std::thread::scope(|scope| {
        let (sent, received) = std::sync::mpsc::sync_channel(1);
        let controls = &handle;
        scope.spawn(move || {
            let submitted = controls.submit(stamp, epochs, "select", "[1,-1]", "null");
            let pending = controls.poll();
            sent.send((submitted, pending.phase)).unwrap();
        });
        // A worker lock can stay held throughout dense preparation. Controls
        // must complete before releasing it, without spinning on this owner.
        let outcome = received.recv_timeout(std::time::Duration::from_secs(1));
        drop(guard);
        let (request, phase) = outcome.expect("UI control blocked on the owning Session");
        assert_eq!(phase, Phase::Preparing);
        handle.cancel(request.unwrap());
    });
    let pointer = &engine.inner.session as *const _ as usize;
    let (_, counts) = crate::allocation::watch(pointer, || {
        drain(&engine);
        assert_eq!(handle.poll().phase, Phase::Failed);
        handle.close();
        drain(&engine);
    });
    assert!(counts.locks > 0);
    assert_eq!(
        (counts.allocations, counts.deallocations),
        (0, 0),
        "Session slot must only exchange prebuilt owner heads"
    );
    assert!(engine.inner.session.lock().is_none());
}

#[test]
fn actor_cancel_at_actual_final_state_lock_refuses_engine_and_session_exchange() {
    let (engine, handle, stamp, epochs) = empty();
    let before = owner_report(&engine);
    let generation = engine.generation().unwrap();
    let request = handle
        .submit(stamp, epochs, "setFPS", "[31]", "null")
        .unwrap();
    staged(&engine, &handle, request);
    let core = handle.core.upgrade().unwrap();
    let batch = 0x8000_0000 + request as u32;
    assert!(Arc::ptr_eq(&core.batch_cancelled, &engine.inner.staged.lock().get(&batch).unwrap().cancelled), "same final mirror cancellation owner");
    assert!(handle.ready(request, epochs).unwrap());
    let worker_engine = engine.clone();
    let pointer = &engine.inner.state as *const _ as usize;
    let barriers = Arc::new([std::sync::Barrier::new(2), std::sync::Barrier::new(2)]);
    let worker_barriers = barriers.clone();
    // Actual advance: cancellation check, commit check, two owner guard checks,
    // record-current check, publication preparation, then final publish State.
    let worker = std::thread::spawn(move || {
        crate::allocation::with_pause(pointer, 7, worker_barriers, || core.advance(&worker_engine))
    });
    barriers[0].wait();
    assert!(
        owner_report_try(&engine).is_none(),
        "worker owns Session while preparing final commit"
    );
    handle.cancel(request);
    assert!(Arc::ptr_eq(&generation, &engine.generation().unwrap()));
    barriers[1].wait();
    worker.join().unwrap();
    let refused = handle.poll();
    assert_eq!(refused.phase, Phase::Failed);
    assert!(refused.error.is_some());
    assert!(owner_report(&engine).same_node(&before));
    assert!(engine.mirrored().unwrap().1.same_node(&before));
    assert!(Arc::ptr_eq(&generation, &engine.generation().unwrap()));
    drain(&engine);
    assert_current_raw_charge(&engine);
    assert!(handle.terminal(request).unwrap());
    drain(&engine);
}

#[test]
fn actor_cancel_after_final_paired_exchange_before_header_keeps_exactly_one_commit() {
    let (engine, handle, stamp, epochs) = empty();
    let before = owner_report(&engine);
    let generation = engine.generation().unwrap();
    let request = handle.submit(stamp, epochs, "setFPS", "[31]", "null").unwrap();
    staged(&engine, &handle, request);
    assert!(handle.ready(request, epochs).unwrap());
    let core = handle.core.upgrade().unwrap();
    let pointer = &core.committing as *const AtomicBool as usize;
    let barriers = Arc::new([std::sync::Barrier::new(2), std::sync::Barrier::new(2)]);
    let worker_barriers = barriers.clone();
    let worker_engine = engine.clone();
    let worker = std::thread::spawn(move || crate::allocation::with_pause(pointer, 1, worker_barriers, || worker_engine.step()));
    barriers[0].wait();
    assert!(!Arc::ptr_eq(&generation, &engine.generation().unwrap()), "final Engine head already exchanged");
    assert!(owner_report_try(&engine).is_none());
    handle.cancel(request);
    barriers[1].wait();
    assert!(worker.join().unwrap());
    drain(&engine);
    let poll = handle.poll();
    assert_eq!(poll.phase, Phase::Success);
    assert!(poll.error.is_none());
    let after = owner_report(&engine);
    assert!(!after.same_node(&before));
    assert_eq!(after.get("fps"), Some(&Value::from(31.0)));
    assert!(engine.mirrored().unwrap().1.same_node(&after));
    let core = handle.core.upgrade().unwrap();
    let worker = core.worker.lock().unwrap();
    let pending = worker.pending.as_ref().unwrap();
    assert!(pending.engine_retired.is_some() && pending.editor_retired.is_some());
    assert!(engine.inner.record_budget.retained_bytes() > 0);
    assert_eq!(worker.stamp.sequence, request);
    drop(worker);
    assert!(handle.terminal(request).unwrap());
    drain(&engine);
    assert_current_raw_charge(&engine);
}

#[test]
fn expiry_from_another_worker_cannot_close_a_valid_paired_commit_before_stamp_update() {
    let (engine, handle, stamp, epochs) = empty();
    let request = handle.submit(stamp, epochs, "setFPS", "[31]", "null").unwrap();
    staged(&engine, &handle, request);
    assert!(handle.ready(request, epochs).unwrap());
    let core = handle.core.upgrade().unwrap();
    let state_pointer = &engine.inner.state as *const _ as usize;
    let commit_pointer = &core.committing as *const AtomicBool as usize;
    let expiry_barriers = Arc::new([std::sync::Barrier::new(2), std::sync::Barrier::new(2)]);
    let commit_barriers = Arc::new([std::sync::Barrier::new(2), std::sync::Barrier::new(2)]);
    let expiry_engine = engine.clone();
    let expiry_pause = expiry_barriers.clone();
    let expiry = std::thread::spawn(move || {
        crate::allocation::with_pause(state_pointer, 1, expiry_pause, || expire_current(&expiry_engine));
    });
    // The inherited predicate read committing=false before this actual State
    // lock. Publication can then change Engine heads before Core stamp update.
    expiry_barriers[0].wait();
    let commit_engine = engine.clone();
    let commit_core = core.clone();
    let commit_pause = commit_barriers.clone();
    let commit = std::thread::spawn(move || {
        crate::allocation::with_pause(commit_pointer, 1, commit_pause, || commit_core.advance(&commit_engine));
    });
    commit_barriers[0].wait();
    assert_ne!(engine.generation().unwrap().id, stamp.generation);
    assert_eq!(core.generation.load(Ordering::Acquire), stamp.generation);
    assert!(core.committing.load(Ordering::Acquire));
    expiry_barriers[1].wait();
    expiry.join().unwrap();
    let incorrectly_closed = core.closed.load(Ordering::Acquire);
    commit_barriers[1].wait();
    commit.join().unwrap();
    assert!(!incorrectly_closed, "incoherent expiry closed a valid canonical Session");
    let published = handle.poll();
    assert_eq!(published.phase, Phase::Success);
    assert_eq!(published.success.unwrap().stamp.sequence, request);
    assert_eq!(core.generation.load(Ordering::Acquire), engine.generation().unwrap().id);
    assert_eq!(owner_report(&engine).get("fps"), Some(&Value::Num(31.0)));
    assert!(handle.terminal(request).unwrap());
    drain(&engine);
    assert_eq!(handle.poll().phase, Phase::Idle);
    assert!(engine.inner.session.lock().is_some());
    handle.close();
    drain(&engine);
    assert_current_raw_charge(&engine);
}

#[test]
fn every_interface_actor_wake_completes_while_state_and_session_slot_are_held() {
    let (engine, handle, stamp, epochs) = empty();
    let pointer = &engine.inner.state as *const _ as usize;
    let state = engine.inner.state.lock();
    let slot = engine.inner.session.lock();
    let request = crate::allocation::with_failure(pointer, 1, || {
        assert!(engine.generation_identity_now().is_none());
        let request = handle.submit(stamp, epochs, "setFPS", "[31]", "null").unwrap();
        assert_eq!(handle.poll().phase, Phase::Preparing);
        handle.cancel(request);
        request
    });
    assert!(state.retained_tasks.is_empty(), "UI wake cannot append a closure");
    drop((slot, state));
    drain(&engine);
    assert_eq!(handle.poll().phase, Phase::Failed);
    let state = engine.inner.state.lock();
    let slot = engine.inner.session.lock();
    crate::allocation::with_failure(pointer, 1, || assert!(handle.terminal(request).unwrap()));
    drop((slot, state));
    drain(&engine);
    let request = handle.submit(stamp, epochs, "setFPS", "[32]", "null").unwrap();
    loop {
        drain(&engine);
        let poll = handle.poll();
        if let Some(packet) = poll.packet {
            let state = engine.inner.state.lock();
            let slot = engine.inner.session.lock();
            crate::allocation::with_failure(pointer, 1, || assert!(handle.ack_packet(request, packet.sequence).unwrap()));
            drop((slot, state));
        } else { assert_eq!(poll.phase, Phase::Ready); break; }
    }
    let state = engine.inner.state.lock();
    let slot = engine.inner.session.lock();
    crate::allocation::with_failure(pointer, 1, || assert!(handle.ready(request, epochs).unwrap()));
    drop((slot, state));
    drain(&engine);
    assert_eq!(handle.poll().phase, Phase::Success);
    let state = engine.inner.state.lock();
    let slot = engine.inner.session.lock();
    crate::allocation::with_failure(pointer, 1, || { handle.close(); cancel_current(&engine); });
    assert!(slot.as_ref().unwrap().worker.try_lock().unwrap().session.is_some(), "close only signals its worker");
    drop((slot, state));
    drain(&engine);
    assert!(engine.inner.session.lock().is_none());
    assert_current_raw_charge(&engine);
}

#[test]
fn fresh_and_warm_admission_busy_preserve_original_owner_before_any_exchange() {
    let (engine, stamp, epochs, state_text, history) = fresh_bootstrap("sheet");
    let before = engine.mirrored().unwrap().1;
    let pointer = &engine.inner.state as *const _ as usize;
    for state_busy in [false, true] {
        let state = state_busy.then(|| engine.inner.state.lock());
        let slot = (!state_busy).then(|| engine.inner.session.lock());
        crate::allocation::with_failure(pointer, 1, || {
            assert!(matches!(engine.request_fresh_session(stamp, epochs, &state_text, history), Err(Error::Busy)));
        });
        drop((state, slot));
        assert!(engine.inner.session.lock().is_none());
        assert!(engine.mirrored().unwrap().1.same_node(&before));
    }
    let fresh = engine.request_fresh_session(stamp, epochs, &state_text, history).unwrap();
    drain(&engine);
    assert_eq!(fresh.poll().phase, Phase::Attached);
    let state = engine.inner.state.lock();
    let slot = engine.inner.session.lock();
    crate::allocation::with_failure(pointer, 1, || assert!(fresh.activate(1, epochs).unwrap()));
    drop((slot, state));
    drain(&engine);
    assert_eq!(fresh.poll().phase, Phase::Idle);
    fresh.close();
    drain(&engine);
    let (_, tree, loader, prepared) = engine.mirrored_for_editing().unwrap();
    let session = Session::start_worker_prepared(tree, &state_text, loader, prepared, history.counters).unwrap();
    let original = session.report().clone();
    let checkpoint = session.history_checkpoint_worker();
    let state = engine.inner.state.lock();
    let (error, session) = engine.attach_session_worker(session, HistoryLimits { count: 1, bytes: 64.0 }, 999.0, epochs).err().unwrap();
    assert_eq!(error, Error::Busy);
    assert!(session.report().same_node(&original));
    assert_eq!(session.history_checkpoint_worker(), checkpoint, "Busy cannot alter original worker history settings/checkpoint");
    drop(state);
    let (handle, _) = engine.attach_session_worker(session, history.limits, 0.0, epochs).unwrap_or_else(|(error, _)| panic!("{error}"));
    handle.close();
    drain(&engine);
    assert!(engine.inner.session.lock().is_none());
}

#[test]
fn one_shot_cancel_after_final_stream_check_wakes_without_ack_or_later_control() {
    let (engine, handle, stamp, epochs) = empty();
    let before = owner_report(&engine);
    let generation = engine.generation().unwrap();
    let request = handle.submit(stamp, epochs, "setFPS", "[31]", "null").unwrap();
    let core = handle.core.upgrade().unwrap();
    let pointer = core.as_ref() as *const Core as usize;
    let barriers = Arc::new([std::sync::Barrier::new(2), std::sync::Barrier::new(2)]);
    let worker_engine = engine.clone();
    let worker_barriers = barriers.clone();
    // Run the actual retained task, not a direct advance leaving an older
    // callback queued. Pause after stream's final check, before packet publish.
    let worker = std::thread::spawn(move || {
        crate::allocation::with_pause(pointer, 1, worker_barriers, || worker_engine.step())
    });
    barriers[0].wait();
    assert!(core.queued.load(Ordering::Acquire));
    assert!(engine.inner.record_budget.retained_bytes() > 0);
    handle.cancel(request); // one producer wake; no ACK, terminal or later UI call
    barriers[1].wait();
    assert!(worker.join().unwrap());
    // Only queued worker continuations may finish cancellation. Cancellation
    // remains nonzero after Failed, which must not cause an endless requeue.
    for _ in 0..16 {
        if !engine.step() { break; }
    }
    assert!(!engine.step(), "failed cancellation spun the retained lane");
    let refused = handle.poll();
    assert_eq!(refused.phase, Phase::Failed);
    assert_eq!(refused.error, Some(Error::Cancelled));
    assert!(refused.packet.is_none());
    assert!(owner_report(&engine).same_node(&before));
    assert!(engine.mirrored().unwrap().1.same_node(&before));
    assert!(Arc::ptr_eq(&generation, &engine.generation().unwrap()));
    assert_current_raw_charge(&engine);
    assert!(core.worker.lock().unwrap().pending.is_none());
}

fn owner_report_try(engine: &Engine) -> Option<Value> {
    let core = engine.inner.session.lock().clone()?;
    let worker = core.worker.try_lock().ok()?;
    Some(worker.session.as_ref()?.report().clone())
}

#[test]
fn unacked_request_timeout_and_wrong_initial_stamp_preserve_exact_owner() {
    static NOW: AtomicU64 = AtomicU64::new(0);
    fn timeout_clock() -> f64 {
        f64::from_bits(NOW.load(Ordering::SeqCst))
    }
    NOW.store(0.0f64.to_bits(), Ordering::SeqCst);
    let (initial, _, _, _) = native(true);
    let report = owner_report(&initial);
    let engine = Engine::new(timeout_clock, Budgets::for_device(None));
    engine.stage(0, Change::Reset).unwrap();
    engine
        .stage(0, Change::Properties(json::to_string(&report).into_bytes()))
        .unwrap();
    engine.commit(0, 0).unwrap();
    let (_, tree, loader, prepared) = engine.mirrored_for_editing().unwrap();
    let session = Session::start_prepared(tree, "{}", loader, prepared).unwrap();
    let epochs = Epochs::default();
    let (handle, stamp) = engine
        .attach_session_worker(session, HistoryLimits::default(), 0.0, epochs)
        .unwrap_or_else(|(error, _)| panic!("{error}"));
    let before = owner_report(&engine);
    let request = handle
        .submit(
            Stamp {
                revision: stamp.revision + 1,
                ..stamp
            },
            epochs,
            "setFPS",
            "[31]",
            "null",
        )
        .unwrap();
    drain(&engine);
    assert_eq!(handle.poll().error, Some(Error::Stale));
    handle.terminal(request).unwrap();
    drain(&engine);
    let request = handle
        .submit(stamp, epochs, "select", "[1,-1]", "null")
        .unwrap();
    drain(&engine);
    assert!(handle.poll().packet.is_some());
    assert!(engine.inner.record_budget.retained_bytes() > 0);
    NOW.store((TIMEOUT_MS + 1.0).to_bits(), Ordering::SeqCst);
    drain(&engine);
    assert_eq!(handle.poll().error, Some(Error::TimedOut));
    assert!(owner_report(&engine).same_node(&before));
    assert!(engine.mirrored().unwrap().1.same_node(&before));
    assert_current_raw_charge(&engine);
    assert!(handle.terminal(request).unwrap());
    drain(&engine);
    assert_eq!(handle.poll().phase, Phase::Idle);
}

fn fresh_bootstrap(mode: &str) -> (Engine, Stamp, Epochs, String, FreshHistory) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../tests/session-handoff/{mode}-fresh.json"));
    let reference = parsed(&std::fs::read_to_string(path).unwrap());
    let first = reference.get("steps").unwrap().item(0).unwrap();
    let original = first.get("history").unwrap();
    let counters = RevisionCounters {
        revision: original.get("revision").unwrap().as_f64().unwrap() as u64,
        serial: original.get("revisionSerial").unwrap().as_f64().unwrap() as u64,
        saved: original.get("savedRevision").unwrap().as_f64().unwrap() as u64,
    };
    let engine = Engine::new(clock, Budgets::for_device(None));
    engine.stage(0, Change::Reset).unwrap();
    engine
        .stage(
            0,
            Change::Properties(
                first
                    .get("document")
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .as_bytes()
                    .to_vec(),
            ),
        )
        .unwrap();
    engine.commit(counters.revision, 0).unwrap();
    let generation = engine.generation().unwrap();
    let stamp = Stamp {
        generation: generation.id,
        lineage: generation.lineage,
        revision: generation.revision,
        sequence: 0,
    };
    let epochs = Epochs {
        shown: 71,
        view: 4,
        gesture: 0,
    };
    let mut state = first.get("state").unwrap().clone();
    for (key, value) in reference.get("options").unwrap().as_obj().unwrap().iter() {
        state.make_obj().unwrap().set(key, value.clone());
    }
    (
        engine,
        stamp,
        epochs,
        json::to_string(&state),
        FreshHistory {
            undo: 0,
            redo: 0,
            checkpoint: 0.0,
            counters,
            limits: HistoryLimits::default(),
        },
    )
}

#[test]
fn asynchronous_fresh_bootstrap_reserves_same_owner_then_requires_guarded_activation() {
    for mode in ["native", "sheet"] {
        let (engine, stamp, epochs, state, history) = fresh_bootstrap(mode);
        let before = engine.mirrored().unwrap().1;
        let handle = engine
            .request_fresh_session(stamp, epochs, &state, history)
            .unwrap();
        let core = handle.core.upgrade().unwrap();
        assert!(
            core.worker.lock().unwrap().session.is_none(),
            "no Editor constructed on request thread"
        );
        assert!(matches!(
            engine.request_fresh_session(stamp, epochs, &state, history),
            Err(Error::Busy)
        ));
        assert_eq!(
            handle.submit(stamp, epochs, "setFPS", "[31]", "null"),
            Err(Error::Busy)
        );
        assert_eq!(handle.activate(1, epochs), Err(Error::Ack));
        drain(&engine);
        let attached = handle.poll();
        assert_eq!(attached.phase, Phase::Attached);
        assert_eq!(
            attached.success,
            Some(Success {
                request: 1,
                stamp,
                epochs
            })
        );
        assert!(owner_report(&engine).same_node(&before));
        assert!(!core.worker.lock().unwrap().active);
        assert!(handle.activate(1, epochs).unwrap());
        drain(&engine);
        assert_eq!(handle.poll().phase, Phase::Idle);
        assert!(core.worker.lock().unwrap().active);
        let request = handle
            .submit(stamp, epochs, "setFPS", "[31]", "null")
            .unwrap();
        let (answer, _) = staged(&engine, &handle, request);
        assert_eq!(answer.path(&["state", "dirty"]), Some(&Value::Bool(true)));
        let worker = core.worker.lock().unwrap();
        let original = worker
            .pending
            .as_ref()
            .unwrap()
            .prepared
            .as_ref()
            .unwrap()
            .publication().before_history();
        assert_eq!(
            original.get("revision").unwrap().as_f64(),
            Some(history.counters.revision as f64)
        );
        assert_eq!(
            original.get("savedRevision").unwrap().as_f64(),
            Some(history.counters.saved as f64)
        );
        assert_eq!(original.get("size").unwrap().as_f64(), Some(0.0));
        drop(worker);
        let published = commit(&engine, &handle, request, epochs);
        assert_eq!(request, 2, "fresh bootstrap reserves request1");
        assert_eq!(published.stamp.sequence, request, "the committed stamp carries the actual request ID");
        handle.terminal(request).unwrap();
        drain(&engine);
    }
}

#[test]
fn fresh_bootstrap_cancel_stale_activation_and_foreign_warm_history_leave_original_head() {
    for operation in [
        "queued-cancel",
        "attached-cancel",
        "stale-selection",
        "new-lineage",
    ] {
        let (engine, stamp, epochs, state, history) = fresh_bootstrap("sheet");
        let before = engine.mirrored().unwrap().1;
        let handle = engine
            .request_fresh_session(stamp, epochs, &state, history)
            .unwrap();
        if operation != "queued-cancel" {
            drain(&engine);
            assert_eq!(handle.poll().phase, Phase::Attached);
        }
        match operation {
            "stale-selection" => {
                handle
                    .activate(
                        1,
                        Epochs {
                            view: epochs.view + 1,
                            ..epochs
                        },
                    )
                    .unwrap();
            }
            "new-lineage" => {
                engine.stage(0, Change::Reset).unwrap();
                engine
                    .stage(0, Change::Properties(json::to_string(&before).into_bytes()))
                    .unwrap();
                engine.commit(stamp.revision, 0).unwrap();
            }
            _ => handle.cancel(1),
        }
        drain(&engine);
        let poll = handle.poll();
        if operation == "new-lineage" {
            assert_eq!(poll.phase, Phase::Closed);
        } else {
            assert_eq!(poll.phase, Phase::Failed);
            assert!(poll.error.is_some());
            assert!(handle.terminal(1).unwrap());
            drain(&engine);
        }
        assert!(engine.inner.session.lock().is_none());
        assert_eq!(
            json::to_string(&engine.mirrored().unwrap().1),
            json::to_string(&before)
        );
        assert_current_raw_charge(&engine);
    }
    let (engine, stamp, epochs, state, fresh) = fresh_bootstrap("native");
    for history in [
        FreshHistory { undo: 1, ..fresh },
        FreshHistory { redo: 1, ..fresh },
        FreshHistory {
            checkpoint: 1.0,
            ..fresh
        },
    ] {
        assert!(matches!(
            engine.request_fresh_session(stamp, epochs, &state, history),
            Err(Error::WarmHistory)
        ));
        assert!(engine.inner.session.lock().is_none());
    }
    assert!(matches!(
        engine.request_fresh_session(
            stamp,
            epochs,
            &"x".repeat(MAX_SESSION_CONTROL_BYTES + 1),
            fresh
        ),
        Err(Error::InputBudget)
    ));
    assert!(engine.inner.session.lock().is_none());
}


fn opened_live_trace(reference: &Value) -> (Engine, Handle, Stamp, Epochs) {
    let first = reference.get("steps").unwrap().item(0).unwrap();
    let initial = first.get("history").unwrap();
    let counter = |key| initial.get(key).unwrap().as_f64().unwrap() as u64;
    let counters = RevisionCounters { revision: counter("revision"), serial: counter("revisionSerial"), saved: counter("savedRevision") };
    let engine = Engine::new(clock, Budgets::for_device(None));
    engine.stage(0, Change::Reset).unwrap();
    engine.stage(0, Change::Properties(first.get("document").unwrap().as_str().unwrap().as_bytes().to_vec())).unwrap();
    engine.commit(counters.revision, 0).unwrap();
    let (_, tree, loader, prepared) = engine.mirrored_for_editing().unwrap();
    let mut state = first.get("state").unwrap().as_obj().unwrap().clone();
    for (key, value) in reference.get("options").unwrap().as_obj().unwrap().iter() { state.set(key, value.clone()); }
    for key in ["activeLayerId", "sheetSelection"] {
        if let Some(value) = first.get("selection").unwrap().get(key) { state.set(key, value.clone()); }
    }
    let session = Session::start_worker_prepared(tree, &json::to_string(&Value::object(state)), loader, prepared, counters).unwrap();
    let limits = reference.get("historyLimits").unwrap();
    let limits = HistoryLimits { count: limits.get("count").unwrap().as_f64().unwrap() as usize, bytes: limits.get("bytes").unwrap().as_f64().unwrap() };
    let epochs = Epochs { shown: 71, view: 4, gesture: 0 };
    let (handle, stamp) = engine.attach_session_worker(session, limits, 0.0, epochs)
        .unwrap_or_else(|(error, _)| panic!("{error}"));
    (engine, handle, stamp, epochs)
}
fn live_history_entries(actual: &Value, expected: &Value, label: &str) {
    assert_eq!(actual.array_len(), expected.array_len(), "{label}");
    for (a, e) in actual.elements().unwrap().iter().zip(expected.elements().unwrap().iter()) {
        for (key, value) in a.as_obj().unwrap().iter() {
            if &**key == "children" { live_history_entries(value, e.get(key).unwrap(), label); }
            else { assert_eq!(Some(value), e.get(key), "{label}: history {key}"); }
        }
    }
}

#[test]
fn recorded_live_core_keeps_stored_working_ready_and_complete_returns_exact_through_ack() {
    let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/session-live");
    let mut paths: Vec<_> = std::fs::read_dir(folder).unwrap().map(|e| e.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "json")
            && !path.file_name().unwrap().to_str().unwrap().contains("preflight")).collect();
    paths.sort();assert_eq!(paths.len(), 18);
    let mut calls = 0;
    for path in paths {
        let reference = parsed(&std::fs::read_to_string(&path).unwrap());
        let name = path.file_name().unwrap().to_str().unwrap();
        let returns = fixture(&format!("session-live-return/{name}"));
        let (engine, handle, mut stamp, epochs) = opened_live_trace(&reference);
        let initial = reference.get("steps").unwrap().item(0).unwrap();
        let mut document = initial.get("document").unwrap().as_str().unwrap().to_owned();
        let mut display = initial.get("frame").unwrap().as_str().unwrap().to_owned();
        let core = handle.core.upgrade().unwrap();
        for (index, step) in reference.get("steps").unwrap().elements().unwrap().iter().enumerate().skip(1) {
            calls += 1;
            let call = step.get("call").unwrap().elements().unwrap();
            let method = call.get(0).unwrap();let method = method.as_str().unwrap();
            let args = Value::array(call.iter().skip(1).collect());
            let before = owner_report(&engine);
            let stored_before = engine.mirrored().unwrap().1;
            let generation = engine.generation().unwrap();
            let previous_ready = core.worker.lock().unwrap().ready.as_ref().map(|ready| Arc::downgrade(&ready.frame));
            let request = handle.submit_live(stamp, epochs, method, &json::to_string(&args)).unwrap();
            let (response, _) = staged(&engine, &handle, request);
            assert!(owner_report(&engine).same_node(&before), "{name} step{index} prepared published");
            assert!(engine.mirrored().unwrap().1.same_node(&stored_before));
            assert!(Arc::ptr_eq(&engine.generation().unwrap(), &generation));
            let answer = response.get("answer").unwrap();
            assert_eq!(answer.get("state"), step.get("state"), "{name} step{index}");
            assert_eq!(answer.get("events"), step.get("events"), "{name} step{index}");
            let expected_return = returns.get("steps").unwrap().item(index - 1).unwrap();
            if step.get("error").unwrap().is_null() {
                assert_eq!(answer.get("status").and_then(Value::as_str), Some("ok"));
                assert_eq!(expected_return.get("resultDefined"), Some(&Value::Bool(true)));
                assert_eq!(json::to_string(answer.get("result").unwrap()), expected_return.get("resultJSON").unwrap().as_str().unwrap(), "{name} step{index} complete return");
            } else {
                assert_eq!(answer.get("status").and_then(Value::as_str), Some("error"));
                assert_eq!(answer.get("error"), step.get("error").unwrap().get("key"));
            }
            let history = response.get("history").unwrap();let expected = step.get("history").unwrap();
            for key in ["size", "revision", "revisionSerial", "savedRevision"] { assert_eq!(history.get(key), expected.get(key)); }
            for key in ["undo", "redo"] { live_history_entries(history.get(key).unwrap(), expected.get(key).unwrap(), name); }
            for (key, value) in response.get("runtime").unwrap().as_obj().unwrap().iter() { assert_eq!(Some(value), step.get("live").unwrap().get(key)); }
            if let Some(next) = step.get("document") { document = next.as_str().unwrap().to_owned(); }
            if let Some(next) = step.get("frame") { display = next.as_str().unwrap().to_owned(); }
            let pointer = &engine.inner.state as *const _ as usize;
            let (success, allocations) = crate::allocation::watch(pointer, || commit(&engine, &handle, request, epochs));
            assert_eq!((allocations.allocations, allocations.deallocations), (0, 0), "{name} step{index}: {allocations:?}");
            stamp = success.stamp;
            assert_eq!(json::to_string(&owner_report(&engine)), document, "{name} step{index} working report");
            if response.get("stored").and_then(Value::as_bool) == Some(true) {
                assert_eq!(json::to_string(&engine.mirrored().unwrap().1), document);
                assert!(!Arc::ptr_eq(&generation, &engine.generation().unwrap()));
            } else {
                assert!(engine.mirrored().unwrap().1.same_node(&stored_before), "{name} step{index}: changed stored root without history");
                assert!(Arc::ptr_eq(&generation, &engine.generation().unwrap()));
            }
            {
                let worker = core.worker.lock().unwrap();
                assert_eq!(worker.session.as_ref().unwrap().operation_epoch(), index as u64);
                let ready = worker.ready.as_ref().unwrap();
                assert_eq!(json::to_string(&ready.record), display, "{name} step{index}: actual prepared display");
                assert_eq!(ready.frame.objects.len(), ready.scene.scene().objects.len());
                if let Some(old) = previous_ready.as_ref() {
                    let old = old.upgrade().expect("old ready retained through terminal ACK");
                    assert!(Arc::ptr_eq(&old, &worker.pending.as_ref().unwrap().ready_retired.as_ref().unwrap().frame));
                }
            }
            assert!(engine.inner.record_budget.retained_bytes() > 0);
            assert!(handle.terminal(request).unwrap());drain(&engine);
            assert_eq!(handle.poll().phase, Phase::Idle);
            assert_current_raw_charge(&engine);
            if let Some(old) = previous_ready { assert!(old.upgrade().is_none(), "prior ready disposed on worker after ACK"); }
        }
        handle.close();drain(&engine);assert!(engine.inner.session.lock().is_none());
    }
    assert_eq!(calls, 208);
}

#[test]
fn working_only_final_cancel_before_or_after_state_exchange_preserves_exact_linearization() {
    for after in [false, true] {
        let reference = fixture("session-live/native-drag-commit.json");
        let (engine, handle, stamp, epochs) = opened_live_trace(&reference);
        let before = owner_report(&engine);let stored_before = engine.mirrored().unwrap().1;
        let generation = engine.generation().unwrap();
        let request = handle.submit_live(stamp, epochs, "select", "[1,-1]").unwrap();
        staged(&engine, &handle, request);assert!(handle.ready(request, epochs).unwrap());
        let core = handle.core.upgrade().unwrap();
        let pointer = if after { &core.committing as *const AtomicBool as usize } else { Arc::as_ptr(&core.batch_cancelled) as usize };
        let barriers = Arc::new([std::sync::Barrier::new(2), std::sync::Barrier::new(2)]);
        let pause = barriers.clone();let worker_engine = engine.clone();let worker_core = core.clone();
        let worker = std::thread::spawn(move || crate::allocation::with_pause(pointer, 1, pause, || worker_core.advance(&worker_engine)));
        barriers[0].wait();handle.cancel(request);barriers[1].wait();worker.join().unwrap();drain(&engine);
        let result = handle.poll();
        assert!(engine.mirrored().unwrap().1.same_node(&stored_before));
        assert!(Arc::ptr_eq(&generation, &engine.generation().unwrap()));
        if after {
            assert_eq!(result.phase, Phase::Success);
            assert_eq!(core.worker.lock().unwrap().session.as_ref().unwrap().operation_epoch(), 1);
            assert!(core.worker.lock().unwrap().ready.is_some());
            assert!(engine.inner.record_budget.retained_bytes() > 0, "postcommit cancel retains live retirement through terminal ACK");
        } else {
            assert_eq!(result.phase, Phase::Failed);
            assert_eq!(result.error, Some(Error::Owner("a staged record is not valid: the working Session was superseded before publication".into())));
            assert!(owner_report(&engine).same_node(&before));
            assert_eq!(core.worker.lock().unwrap().session.as_ref().unwrap().operation_epoch(), 0);
            assert!(core.worker.lock().unwrap().ready.is_none());
            assert_current_raw_charge(&engine);
        }
        assert!(handle.terminal(request).unwrap());drain(&engine);
        assert_eq!(handle.poll().phase, Phase::Idle);assert_current_raw_charge(&engine);
    }
}


#[test]
fn dense_live_stream_stale_ready_and_close_retire_candidates_without_losing_live_baseline() {
    for close_unacked in [false, true] {
        let (engine, handle, mut stamp, epochs) = native(true);
        let stored = engine.mirrored().unwrap().1;let generation = engine.generation().unwrap();
        let core = handle.core.upgrade().unwrap();
        for (method, args) in [("select", "[1,-1]"), ("beginDrag", r#"["Dense live"]"#)] {
            let request = handle.submit_live(stamp, epochs, method, args).unwrap();
            staged(&engine, &handle, request);stamp = commit(&engine, &handle, request, epochs).stamp;
            handle.terminal(request).unwrap();drain(&engine);
        }
        let before = owner_report(&engine);
        let (ready, budget) = {
            let worker = core.worker.lock().unwrap();let frame = worker.ready.as_ref().unwrap();
            (Arc::downgrade(&frame.frame), frame.prepared.doc.scene_budget.clone())
        };
        assert_eq!(budget.retained_entries(), 1);
        let request = handle.submit_live(stamp, epochs, "moveSelection", "[8,5]").unwrap();
        if close_unacked {
            drain(&engine);let first = handle.poll();
            assert_eq!(first.phase, Phase::Streaming);assert!(first.packet.is_some());
            assert!(engine.inner.record_budget.retained_bytes() > 0);
            handle.close();drain(&engine);
            assert_eq!(handle.poll().phase, Phase::Closed);
            assert!(core.worker.lock().unwrap().session.is_none());
            assert!(core.worker.lock().unwrap().ready.is_none());
            assert!(ready.upgrade().is_none());assert_eq!(budget.retained_entries(), 0);
        } else {
            let (_, packets) = staged(&engine, &handle, request);
            assert!(packets > 1, "one dense changed figure is sliced by the existing Chunks owner");
            assert!(owner_report(&engine).same_node(&before));
            let wrong = Epochs { gesture: epochs.gesture + 1, ..epochs };
            handle.ready(request, wrong).unwrap();drain(&engine);
            assert_eq!(handle.poll().error, Some(Error::Stale));
            assert!(owner_report(&engine).same_node(&before));
            assert!(ready.upgrade().is_some());assert_eq!(budget.retained_entries(), 1);
            assert_eq!(core.worker.lock().unwrap().session.as_ref().unwrap().operation_epoch(), 2);
            handle.terminal(request).unwrap();drain(&engine);
            // Failed transport did not cancel or rebuild the actual Drag owner.
            // Execute its real cancellation through the same ordered stream.
            let cancel = handle.submit_live(stamp, epochs, "cancelDrag", "[]").unwrap();
            let (response, _) = staged(&engine, &handle, cancel);
            assert_eq!(response.get("runtime").unwrap().get("drag"), Some(&Value::Bool(false)));
            commit(&engine, &handle, cancel, epochs);handle.terminal(cancel).unwrap();drain(&engine);
            handle.close();drain(&engine);assert!(core.worker.lock().unwrap().ready.is_none());
            assert_eq!(budget.retained_entries(), 0);
        }
        assert!(engine.mirrored().unwrap().1.same_node(&stored));
        assert!(Arc::ptr_eq(&generation, &engine.generation().unwrap()));
        assert_current_raw_charge(&engine);
        assert!(engine.inner.session.lock().is_none());
    }
}


#[test]
fn recorded_quiet_history_after_live_refreshes_actual_ready_and_retains_previous_until_ack() {
    let reference = fixture("session-live/native-drag-commit.json");
    let (engine, handle, mut stamp, epochs) = opened_live_trace(&reference);
    let core = handle.core.upgrade().unwrap();
    let mut document = reference.get("steps").unwrap().item(0).unwrap().get("document").unwrap().as_str().unwrap().to_owned();
    let mut display = reference.get("steps").unwrap().item(0).unwrap().get("frame").unwrap().as_str().unwrap().to_owned();
    for (index, step) in reference.get("steps").unwrap().elements().unwrap().iter().enumerate().skip(1).take(7) {
        let call = step.get("call").unwrap().elements().unwrap();
        let method = call.get(0).unwrap();let method = method.as_str().unwrap();
        let args = json::to_string(&Value::array(call.iter().skip(1).collect()));
        let quiet = matches!(method, "undo" | "redo");
        let previous = core.worker.lock().unwrap().ready.as_ref().map(|ready| Arc::downgrade(&ready.frame));
        let request = if quiet { handle.submit(stamp, epochs, method, &args, "null") }
            else { handle.submit_live(stamp, epochs, method, &args) }.unwrap();
        let (response, _) = staged(&engine, &handle, request);
        let answer = if quiet { &response } else { response.get("answer").unwrap() };
        assert_eq!(answer.get("state"), step.get("state"), "step{index}");
        assert_eq!(answer.get("events"), step.get("events"), "step{index}");
        if let Some(next) = step.get("document") { document = next.as_str().unwrap().to_owned(); }
        if let Some(next) = step.get("frame") { display = next.as_str().unwrap().to_owned(); }
        let pointer = &engine.inner.state as *const _ as usize;
        let (done, allocations) = crate::allocation::watch(pointer, || commit(&engine, &handle, request, epochs));
        assert_eq!((allocations.allocations, allocations.deallocations), (0, 0));stamp = done.stamp;
        assert_eq!(json::to_string(&owner_report(&engine)), document, "step{index}");
        let worker = core.worker.lock().unwrap();
        assert_eq!(json::to_string(&worker.ready.as_ref().unwrap().record), display, "step{index} actual quiet/live ready");
        if let Some(previous) = &previous { assert!(previous.upgrade().is_some(), "prior ready retained through ACK"); }
        drop(worker);handle.terminal(request).unwrap();drain(&engine);
        if let Some(previous) = previous { assert!(previous.upgrade().is_none(), "prior ready retired after ACK"); }
    }
    handle.close();drain(&engine);
}

#[path = "session_raw_tests.rs"]
mod raw;
