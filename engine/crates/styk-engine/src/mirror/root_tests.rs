use super::*;
use crate::{
    Budgets,
    mirror::{Change, MetadataField, RecordTarget, transfer::TransferError},
};
use styk_doc::{Obj, json};
use styk_edit::{HistoryLimits, session::Session};

fn clock() -> f64 {
    0.0
}
#[test]
fn session_union_reuses_one_charge_and_counts_actual_shared_packed_sections_once() {
    let (engine, mut session) = opened();
    let before = session.report().clone();
    let prepared = candidate(&mut session, "setFPS", "[31]");
    let packed = Value::nums(&[0.125; 4096]);
    let blob = Value::Blob(Arc::new(styk_doc::Blob { prefix: Arc::from("raw:"), bytes: vec![17; 8192].into() }));
    let current = Value::array(vec![packed.clone(), blob.clone(), before.clone()]);
    let response = Value::array(vec![packed, blob, prepared.result().clone()]);
    let union = styk_doc::retained_bytes_checked(&[&before, prepared.before_report(), prepared.report(),
        prepared.result(), prepared.state(), prepared.history(), prepared.events(),
        prepared.before_state(), prepared.before_history(), &response, &current], |_| Ok::<_, ()>(())).unwrap() + 256;
    let naive = [&response, &current].into_iter().map(|value|
        styk_doc::retained_bytes_checked(&[value], |_| Ok::<_, ()>(())).unwrap()).sum::<usize>();
    assert!(naive > union, "actual shared sections must distinguish union from a per-head sum");
    let mut continuation = None;let transaction = guard(&engine);
    let admission = engine.stage_publication_response_worker(7, transaction, &prepared, &response,
        None, false, true, None, Some(SessionRoot { current: Some(&current), charge: &mut continuation, wake: std::sync::Weak::new() }), || Ok(())).unwrap();
    assert_eq!(engine.inner.record_budget.retained_bytes(), union);
    let identity = Arc::as_ptr(&continuation.as_ref().unwrap().charge);
    let batch_budget = continuation.as_ref().unwrap().charge.budget();
    assert!(!ReadyCharge::rescope(&mut continuation, Some(&current)).unwrap());
    engine.discard_batch_worker(7);drop(admission);
    assert!(ReadyCharge::rescope(&mut continuation, Some(&current)).unwrap());
    let current_bytes = styk_doc::retained_bytes_checked(&[&current], |_| Ok::<_, ()>(())).unwrap() + 256;
    assert_eq!(engine.inner.record_budget.retained_bytes(), current_bytes);
    let admission = engine.stage_publication_response_worker(8, transaction, &prepared, &response,
        None, false, true, None, Some(SessionRoot { current: Some(&current), charge: &mut continuation, wake: std::sync::Weak::new() }), || Ok(())).unwrap();
    assert_eq!(Arc::as_ptr(&continuation.as_ref().unwrap().charge), identity, "same allocation, not a replacement ready charge");
    assert!(Arc::ptr_eq(&continuation.as_ref().unwrap().charge.budget(), &batch_budget));
    assert_eq!(engine.inner.record_budget.retained_bytes(), union, "reserve additional bytes only");
    engine.discard_batch_worker(8);drop(admission);drop(continuation);
    assert_eq!(engine.inner.record_budget.retained_bytes(), 0);
}

#[test]
fn same_charge_hard_uniqueness_barrier_includes_weak_aliases_and_preserves_one_gib_boundary() {
    use super::super::transfer::{MAX_RECORD_RETAINED_BYTES, TransferError};
    assert_eq!(MAX_RECORD_RETAINED_BYTES, 1024 * 1024 * 1024);
    let global = Arc::new(Budget::default());let batch = Arc::new(Budget::default());
    let mut owner = Some(ReadyCharge { charge: Arc::new(Charge::new(global.clone(), batch.clone()).unwrap()) });
    let charge = Arc::get_mut(&mut owner.as_mut().unwrap().charge).unwrap();
    charge.grow_to(MAX_RECORD_RETAINED_BYTES - 1).unwrap();
    charge.grow_to(MAX_RECORD_RETAINED_BYTES).unwrap();
    assert_eq!(charge.grow_to(MAX_RECORD_RETAINED_BYTES + 1), Err(TransferError::Capacity));
    assert_eq!((global.retained_bytes(), batch.retained_bytes()), (MAX_RECORD_RETAINED_BYTES, MAX_RECORD_RETAINED_BYTES));
    let weak = Arc::downgrade(&owner.as_ref().unwrap().charge);
    assert!(!owner.as_mut().unwrap().unique());
    assert!(!ReadyCharge::rescope(&mut owner, None).unwrap());
    assert_eq!(global.retained_bytes(), MAX_RECORD_RETAINED_BYTES);
    drop(weak);assert!(ReadyCharge::rescope(&mut owner, None).unwrap());
    assert!(owner.is_none());assert_eq!((global.retained_bytes(), batch.retained_bytes()), (0, 0));
}

#[test]
fn actor_union_capacity_refusal_preserves_current_reservation_and_atomic_counter_rollback() {
    use super::super::transfer::MAX_RECORD_RETAINED_BYTES;
    for short in [false, true] {
        let (engine, mut session) = opened();let prepared = candidate(&mut session, "setFPS", "[31]");
        let current = Value::nums(&[0.5; 4096]);let response = prepared.result();
        let canonical = session.report();
        let full = styk_doc::retained_bytes_checked(&[canonical, prepared.before_report(), prepared.report(), prepared.result(),
            prepared.state(), prepared.history(), prepared.events(), prepared.before_state(), prepared.before_history(), response, &current], |_| Ok::<_, ()>(())).unwrap() + 256;
        let mut continuation = Some(ReadyCharge { charge: Arc::new(Charge::new(engine.inner.record_budget.clone(), Arc::new(Budget::default())).unwrap()) });
        ReadyCharge::rescope(&mut continuation, Some(&current)).unwrap();
        let previous = engine.inner.record_budget.retained_bytes();
        let mut other = Charge::new(engine.inner.record_budget.clone(), Arc::new(Budget::default())).unwrap();
        other.grow(MAX_RECORD_RETAINED_BYTES - full + usize::from(short)).unwrap();
        let result = engine.stage_publication_response_worker(7, guard(&engine), &prepared, response,
            None, false, true, None, Some(SessionRoot { current: Some(&current), charge: &mut continuation, wake: std::sync::Weak::new() }), || Ok(()));
        if short {
            assert!(result.err().unwrap().to_string().contains("full"));
            assert!(!engine.inner.staged.lock().contains_key(&7));
        } else {
            let admission = result.unwrap();assert_eq!(engine.inner.record_budget.retained_bytes(), MAX_RECORD_RETAINED_BYTES);
            engine.discard_batch_worker(7);drop(admission);
        }
        assert!(ReadyCharge::rescope(&mut continuation, Some(&current)).unwrap());
        assert_eq!(engine.inner.record_budget.retained_bytes(), MAX_RECORD_RETAINED_BYTES - full + usize::from(short) + previous);
        drop(other);assert_eq!(engine.inner.record_budget.retained_bytes(), previous);
        drop(continuation);assert_eq!(engine.inner.record_budget.retained_bytes(), 0);
    }
}
fn parse(text: &str) -> Value {
    json::parse(text.as_bytes()).unwrap()
}
fn opened() -> (Engine, Session) {
    opened_with(clock)
}
fn opened_with(clock: crate::Clock) -> (Engine, Session) {
    let engine = Engine::new(clock, Budgets::for_device(None));
    engine.stage(0, Change::Reset).unwrap();
    // Initial file bootstrap is deliberately outside the interactive path under
    // test. The canonical root's ordering and unknown fields come from its owner.
    engine.stage(0, Change::Properties(br#"{"unknownBefore":{"absent":null},"version":5,"canvas_width":320,"canvas_height":200,"fps":20,"_styk":{"future":{"keep":"exact"},"frameSources":{"retained":{"objects":[],"future":[1,2,3]}}},"figure_types":[],"keyframes":[{"duration_raw":2,"objects":[]}],"backgrounds":[],"unknownAfter":["preserved"]}"#.to_vec())).unwrap();
    engine.commit(0, 0).unwrap();
    let (_, report, loader, prepared) = engine.mirrored_for_editing().unwrap();
    let mut session = Session::start_prepared(report, "{}", loader, prepared).unwrap();
    session
        .prepare_worker_history(HistoryLimits::default(), 0.0, || Ok(()))
        .unwrap();
    (engine, session)
}
fn guard(engine: &Engine) -> RecordGuard {
    let generation = engine.generation().unwrap();
    RecordGuard {
        generation: generation.id,
        lineage: generation.lineage,
        revision: generation.revision,
        nonce: 83,
    }
}
fn candidate(session: &mut Session, method: &str, args: &str) -> PreparedPublication {
    session
        .prepare_worker_call_values(method, &parse(args), &Value::Null, || Ok(()))
        .unwrap()
}
fn finish(
    engine: &Engine,
    session: &mut Session,
    prepared: PreparedPublication,
    transaction: RecordGuard,
    admission: EditPublicationAdmission,
) -> (
    super::super::Committed,
    PublicationRetirement,
    RetiredPublication,
) {
    let guarded = session
        .guard_worker_publication_checked(prepared, || Ok(()))
        .unwrap_or_else(|(error, _)| panic!("{error:?}"));
    engine
        .commit_edit_publication_worker(7, transaction, guarded, admission)
        .unwrap_or_else(|(error, _, _)| panic!("{error:?}"))
}

#[test]
fn exact_editor_roots_preserve_unknown_key_order_absence_sources_and_loader_reuse() {
    let (engine, mut session) = opened();
    let before = session.report().clone();
    let prepared = candidate(&mut session, "setFPS", "[31]");
    let after = prepared.report().clone();
    let transaction = guard(&engine);
    let admission = engine
        .stage_edit_publication_worker(7, transaction, &prepared, || Ok(()))
        .unwrap();
    assert!(session.report().same_node(&before));
    assert!(engine.mirrored().unwrap().1.same_node(&before));
    let (committed, engine_retired, editor_retired) =
        finish(&engine, &mut session, prepared, transaction, admission);
    assert_eq!(committed.revision, 1);
    assert_eq!(committed.converted.figures, 0);
    assert_eq!(committed.converted.keyframes, 0);
    assert!(session.report().same_node(&after));
    assert!(engine.mirrored().unwrap().1.same_node(&after));
    assert!(editor_retired.previous_report().same_node(&before));
    assert!(
        engine_retired
            .canonical_root
            .as_ref()
            .unwrap()
            .same_node(&before)
    );
    assert_eq!(
        session
            .report()
            .as_obj()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        before.as_obj().unwrap().keys().collect::<Vec<_>>()
    );
    for key in [
        "unknownBefore",
        "unknownAfter",
        "figure_types",
        "keyframes",
        "backgrounds",
        "_styk",
    ] {
        assert!(
            session
                .report()
                .get(key)
                .unwrap()
                .same_node(before.get(key).unwrap())
        );
    }
    assert!(session.report().get("frame_delay_ms").is_none());
    drop((engine_retired, editor_retired));
    for (method, args) in [
        ("updateProject", "[{\"canvas_height\":480,\"fps\":12}]"),
        ("undo", "[]"),
        ("redo", "[]"),
    ] {
        let prepared = candidate(&mut session, method, args);
        let after = prepared.report().clone();
        let transaction = guard(&engine);
        let admission = engine
            .stage_edit_publication_worker(7, transaction, &prepared, || Ok(()))
            .unwrap();
        let (_, engine_retired, editor_retired) =
            finish(&engine, &mut session, prepared, transaction, admission);
        assert!(engine.mirrored().unwrap().1.same_node(&after));
        assert!(session.report().same_node(&after));
        drop((engine_retired, editor_retired));
    }
    assert_eq!(
        session
            .report()
            .get("canvas_height")
            .and_then(Value::as_f64),
        Some(480.0)
    );
    assert_eq!(
        session.report().get("fps").and_then(Value::as_f64),
        Some(12.0)
    );
}

#[test]
fn root_charge_and_old_raw_head_remain_reserved_through_worker_terminal_ack() {
    let (engine, mut session) = opened();
    let Value::Obj(before) = session.report() else {
        panic!()
    };
    let old_head = Arc::downgrade(before);
    let prepared = candidate(&mut session, "setFPS", "[31]");
    let transaction = guard(&engine);
    let admission = engine
        .stage_edit_publication_worker(7, transaction, &prepared, || Ok(()))
        .unwrap();
    let charged = engine.inner.record_budget.retained_bytes();
    assert!(charged > 1024);
    let (_, engine_retired, editor_retired) =
        finish(&engine, &mut session, prepared, transaction, admission);
    assert_eq!(
        engine.inner.record_budget.retained_bytes(),
        charged,
        "final publication must not release the admission charge before terminal ACK"
    );
    assert!(old_head.upgrade().is_some());
    engine.discard_batch(7); // postcommit cancellation cannot undo either owner
    while engine.step() {}
    assert_eq!(engine.inner.record_budget.retained_bytes(), charged);
    assert_eq!(
        session.report().get("fps").and_then(Value::as_f64),
        Some(31.0)
    );
    drop(session); // dispose the same active worker cache before isolating retirements
    drop(editor_retired);
    assert!(
        old_head.upgrade().is_some(),
        "Engine retirement retains the actual old raw root including unknown fields"
    );
    drop(engine_retired);
    assert_eq!(engine.inner.record_budget.retained_bytes(), 0);
    assert!(old_head.upgrade().is_none());
}

#[test]
fn typed_pairing_refuses_mixed_records_arbitrary_callbacks_and_different_prepared_roots() {
    let (engine, mut session) = opened();
    let before = session.report().clone();
    let prepared = candidate(&mut session, "setFPS", "[31]");
    let transaction = guard(&engine);
    let admission = engine
        .stage_edit_publication_worker(7, transaction, &prepared, || Ok(()))
        .unwrap();
    assert!(engine.commit_record(1, 7, transaction).is_err());
    assert!(engine.commit_record_with(1, 7, transaction, || ()).is_err());
    assert!(matches!(
        engine.stage_guarded(
            7,
            transaction,
            Change::Resolved(RecordTarget::Metadata(MetadataField::Animation), None)
        ),
        Err(TransferError::Pending)
    ));
    assert!(matches!(
        engine.begin_record(
            7,
            transaction,
            1,
            RecordTarget::Metadata(MetadataField::Animation)
        ),
        Err(TransferError::Pending)
    ));
    let other = candidate(&mut session, "setFPS", "[37]");
    let guarded = session
        .guard_worker_publication_checked(other, || Ok(()))
        .unwrap_or_else(|(error, _)| panic!("{error:?}"));
    let refused = engine
        .commit_edit_publication_worker(7, transaction, guarded, admission)
        .err()
        .expect("different actual root refused");
    drop(refused);
    assert!(session.report().same_node(&before));
    assert!(engine.mirrored().unwrap().1.same_node(&before));
    engine.discard_batch(7);
    while engine.step() {}
    let transaction = RecordGuard {
        nonce: transaction.nonce + 1,
        ..transaction
    };
    let admission = engine
        .stage_edit_publication_worker(7, transaction, &prepared, || Ok(()))
        .unwrap();
    let (_, engine_retired, editor_retired) =
        finish(&engine, &mut session, prepared, transaction, admission);
    drop((engine_retired, editor_retired));
    assert_eq!(
        session.report().get("fps").and_then(Value::as_f64),
        Some(31.0)
    );
}

#[test]
fn cancel_stop_nonce_and_lineage_refusals_preserve_both_heads_and_retire_on_worker() {
    for operation in ["cancel", "stop", "nonce", "lineage"] {
        let (engine, mut session) = opened();
        let before = session.report().clone();
        let prepared = candidate(&mut session, "setFPS", "[31]");
        let mut transaction = guard(&engine);
        let admission = engine
            .stage_edit_publication_worker(7, transaction, &prepared, || Ok(()))
            .unwrap();
        let charged = engine.inner.record_budget.retained_bytes();
        match operation {
            "cancel" => engine.discard_batch(7),
            "stop" => engine.stop(),
            "nonce" => transaction.nonce += 1,
            _ => {
                engine.stage(0, Change::Reset).unwrap();
                engine.stage(0, Change::Properties(br#"{"version":5,"canvas_width":1,"canvas_height":1,"fps":20,"keyframes":[],"figure_types":[],"backgrounds":[]}"#.to_vec())).unwrap();
                engine.commit(0, 0).unwrap();
            }
        }
        let guarded = session
            .guard_worker_publication_checked(prepared, || Ok(()))
            .unwrap_or_else(|(error, _)| panic!("{error:?}"));
        let refused = engine
            .commit_edit_publication_worker(7, transaction, guarded, admission)
            .err()
            .expect("refused");
        drop(refused);
        assert!(session.report().same_node(&before));
        assert_eq!(
            engine.inner.record_budget.retained_bytes(),
            charged,
            "detached root must remain charged until actual worker cleanup: {operation}"
        );
        engine.discard_batch(7);
        while engine.step() {}
        assert_eq!(engine.inner.record_budget.retained_bytes(), 0);
    }
}

#[test]
fn worker_checks_cancel_dense_single_object_and_stale_admission_without_mutation() {
    let (engine, _) = opened();
    let mut root = engine.inner.mirror.lock().unwrap();
    let mut unknown = Obj::new();
    for at in 0..6000 {
        unknown.set(&format!("field{at}"), Value::str("retained"));
    }
    root.tree
        .make_obj()
        .unwrap()
        .set("denseUnknown", Value::object(unknown));
    let (_, report, loader, prepared) = {
        drop(root);
        engine.mirrored_for_editing().unwrap()
    };
    let mut session = Session::start_prepared(report, "{}", loader, prepared).unwrap();
    session
        .prepare_worker_history(HistoryLimits::default(), 0.0, || Ok(()))
        .unwrap();
    let before = session.report().clone();
    let prepared = candidate(&mut session, "setFPS", "[31]");
    let mut checks = 0;
    let result = engine.stage_edit_publication_worker(7, guard(&engine), &prepared, || {
        checks += 1;
        if checks == 4 {
            Err(MirrorError::Transaction("cancel"))
        } else {
            Ok(())
        }
    });
    assert!(result.is_err());
    assert_eq!(checks, 4);
    assert!(session.report().same_node(&before));
    assert!(engine.mirrored().unwrap().1.same_node(&before));
    assert!(!engine.inner.staged.lock().contains_key(&7));
    assert_eq!(engine.inner.record_budget.retained_bytes(), 0);
    let mut stale = guard(&engine);
    stale.lineage += 1;
    assert!(
        engine
            .stage_edit_publication_worker(7, stale, &prepared, || Ok(()))
            .is_err()
    );
    assert_eq!(engine.inner.record_budget.retained_bytes(), 0);
}

#[test]
fn paired_roots_match_actual_editor_project_settings_replays_at_every_step() {
    for name in ["project-settings-sheet", "project-settings-format4-sheet"] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../tests/edit/{name}.reference.json"));
        let reference = json::parse(&std::fs::read(path).unwrap()).unwrap();
        let steps = reference.get("steps").unwrap();
        let first = steps.item(0).unwrap();
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
        engine.commit(0, 0).unwrap();
        let mut state = first.get("state").unwrap().as_obj().unwrap().clone();
        for (key, value) in reference.get("editor").unwrap().as_obj().unwrap().iter() {
            state.set(key, value.clone());
        }
        let (_, report, loader, prepared) = engine.mirrored_for_editing().unwrap();
        let mut session = Session::start_prepared(
            report,
            &json::to_string(&Value::object(state)),
            loader,
            prepared,
        )
        .unwrap();
        session
            .prepare_worker_history(HistoryLimits::default(), 0.0, || Ok(()))
            .unwrap();
        let mut expected_document = first.get("document").unwrap().as_str().unwrap().to_string();
        for at in 1..steps.array_len().unwrap() {
            let want = steps.item(at).unwrap();
            let call = want
                .get("call")
                .unwrap()
                .elements()
                .unwrap()
                .iter()
                .collect::<Vec<_>>();
            let method = call[0].as_str().unwrap();
            let before = session.report().clone();
            let prepared = session
                .prepare_worker_call_values(
                    method,
                    &Value::array(call[1..].to_vec()),
                    &Value::Null,
                    || Ok(()),
                )
                .unwrap();
            let answer = prepared.result();
            if let Some(document) = want.get("document").and_then(Value::as_str) {
                expected_document = document.to_string();
            }
            assert_eq!(answer.get("state"), want.get("state"), "{name}/{at} state");
            assert_eq!(
                answer.get("events"),
                want.get("events"),
                "{name}/{at} events"
            );
            if let Some(error) = want.get("error").filter(|error| !error.is_null()) {
                assert_eq!(answer.get("status").and_then(Value::as_str), Some("error"));
                assert_eq!(
                    answer.get("error"),
                    error.get("key"),
                    "{name}/{at} recorded error"
                );
            } else {
                assert_eq!(
                    answer.get("result"),
                    want.get("result"),
                    "{name}/{at} return"
                );
            }
            assert_eq!(
                json::to_string(prepared.report()),
                expected_document,
                "{name}/{at} prepared canonical root"
            );
            assert!(
                before.same_node(session.report()),
                "preparation preserves the live owner"
            );
            let transaction = guard(&engine);
            let admission = engine
                .stage_edit_publication_worker(7, transaction, &prepared, || Ok(()))
                .unwrap();
            let after = prepared.report().clone();
            let (_, engine_retired, editor_retired) =
                finish(&engine, &mut session, prepared, transaction, admission);
            assert!(session.report().same_node(&after));
            assert!(engine.mirrored().unwrap().1.same_node(&after));
            assert_eq!(
                json::to_string(session.report()),
                expected_document,
                "{name}/{at} committed document"
            );
            assert!(engine.inner.record_budget.retained_bytes() > 0);
            drop((engine_retired, editor_retired));
            assert_eq!(engine.inner.record_budget.retained_bytes(), 0);
            while engine.step() {}
        }
    }
}

#[test]
fn prepared_roots_cannot_bypass_shared_retained_budget_or_revision_pairing() {
    let (engine, mut session) = opened();
    let prepared = candidate(&mut session, "setFPS", "[31]");
    let transaction = guard(&engine);
    let mut wrong = transaction;
    wrong.revision += 1;
    assert!(
        engine
            .stage_edit_publication_worker(7, wrong, &prepared, || Ok(()))
            .is_err()
    );
    // Existing producer-admitted charge fills the SAME global receiver budget.
    engine
        .stage_guarded_admitted(
            9,
            transaction,
            Change::Reset,
            super::super::MAX_RECORD_RETAINED_BYTES,
        )
        .unwrap();
    assert!(
        engine
            .stage_edit_publication_worker(7, transaction, &prepared, || Ok(()))
            .is_err()
    );
    assert!(!engine.inner.staged.lock().contains_key(&7));
    engine.discard_batch(9);
    assert!(
        engine
            .stage_edit_publication_worker(7, transaction, &prepared, || Ok(()))
            .is_err(),
        "detached worker payload remains charged"
    );
    while engine.step() {}
    let admission = engine
        .stage_edit_publication_worker(7, transaction, &prepared, || Ok(()))
        .unwrap();
    let (_, engine_retired, editor_retired) =
        finish(&engine, &mut session, prepared, transaction, admission);
    drop((engine_retired, editor_retired));
}

#[test]
fn real_native_source_publication_history_and_dense_pose_replays_share_the_paired_owner() {
    use styk_edit::{Editor, Options, Selection};
    fn num(value: &Value, key: &str) -> f64 {
        value.get(key).unwrap().as_f64().unwrap()
    }
    let mut calls = 0;
    let mut removed = 0;
    for name in ["independent-count-one", "dense-single-figure"] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../tests/native-layer-publication/{name}.json"));
        let reference = json::parse(&std::fs::read(path).unwrap()).unwrap();
        let steps = reference.get("steps").unwrap();
        let first = steps.item(0).unwrap();
        let state = first.get("state").unwrap();
        let history = first.get("history").unwrap();
        let selection = first.get("selection").unwrap();
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
        engine.commit(num(history, "revision") as u64, 0).unwrap();
        let (_, report, loader, prepared) = engine.mirrored_for_editing().unwrap();
        let mut editor = Editor::new(report, Options::default());
        editor.adopt_prepared(loader, prepared);
        editor.set_view(
            num(selection, "tick"),
            num(selection, "frameIndex") as usize,
            Selection {
                uid: selection
                    .get("uid")
                    .and_then(Value::as_f64)
                    .map(|n| n as u32),
                uids: selection
                    .get("uids")
                    .unwrap()
                    .numbers()
                    .unwrap()
                    .iter()
                    .map(|n| *n as u32)
                    .collect(),
                joint: num(selection, "joint") as i64,
            },
        );
        editor.set_active_layer_id(selection.get("activeLayerId").unwrap().as_str().unwrap());
        editor.set_counters(
            num(state, "layerRevision") as u64,
            num(state, "nextFrameIdentity") as u64,
            num(state, "nextUid") as u64,
        );
        editor.set_revision_counters(
            num(history, "revision") as u64,
            num(history, "revisionSerial") as u64,
            num(history, "savedRevision") as u64,
        );
        let limits = reference.get("historyLimits").unwrap();
        editor
            .configure_worker_history(HistoryLimits {
                count: num(limits, "count") as usize,
                bytes: num(limits, "bytes"),
            })
            .unwrap();
        for at in 1..steps.array_len().unwrap() {
            let want = steps.item(at).unwrap();
            let call = want.get("call").unwrap();
            let method = call.item(0).unwrap();
            let method = method.as_str().unwrap();
            let transaction = guard(&engine);
            let before = editor.report().clone();
            let prepared = editor
                .prepare_worker_publication(
                    |editor, check| {
                        if method == "undo" || method == "redo" {
                            return editor
                                .apply_history_worker(method == "redo")
                                .map(Value::Bool);
                        }
                        let request = reference.get("request").unwrap();
                        let uids = request
                            .get("uids")
                            .unwrap()
                            .numbers()
                            .unwrap()
                            .iter()
                            .map(|uid| *uid as u32)
                            .collect::<Vec<_>>();
                        let plan = editor.prepare_native_layer_migration_worker(
                            &uids,
                            request.get("targetLayerId").unwrap().as_str().unwrap(),
                            None,
                            want.get("wire").unwrap(),
                            &mut *check,
                            |_| {},
                            clock,
                        )?;
                        editor
                            .publish_native_layer_migration_worker(plan, &mut *check)
                            .map(Value::Bool)
                    },
                    || Ok(()),
                )
                .unwrap();
            assert_eq!(
                json::to_string(prepared.report()),
                want.get("document").unwrap().as_str().unwrap(),
                "{name}/{at} prepared bytes"
            );
            assert_eq!(
                prepared.history(),
                want.get("history").unwrap(),
                "{name}/{at} exact history"
            );
            assert_eq!(
                prepared.state(),
                want.get("state").unwrap(),
                "{name}/{at} state"
            );
            assert_eq!(
                prepared.events(),
                want.get("events").unwrap(),
                "{name}/{at} events"
            );
            assert_eq!(
                prepared.result(),
                want.get("result").unwrap(),
                "{name}/{at} return"
            );
            let retiring_sources = before
                .path(&["_styk", "frameSources"])
                .and_then(Value::as_obj)
                .into_iter()
                .flat_map(|table| table.iter())
                .filter_map(|(id, source)| {
                    if prepared
                        .report()
                        .path(&["_styk", "frameSources", id])
                        .is_none()
                        && let Value::Obj(source) = source
                    {
                        return Some(Arc::downgrade(source));
                    }
                    None
                })
                .collect::<Vec<_>>();
            removed += retiring_sources.len();
            let admission = engine
                .stage_edit_publication_worker(7, transaction, &prepared, || Ok(()))
                .unwrap();
            let after = prepared.report().clone();
            let guarded = prepared
                .guard_worker(&mut editor)
                .unwrap_or_else(|(error, _)| panic!("{error:?}"));
            let (_, engine_retired, editor_retired) = engine
                .commit_edit_publication_worker(7, transaction, guarded, admission)
                .unwrap_or_else(|(error, _, _)| panic!("{error:?}"));
            assert!(editor.report().same_node(&after));
            assert!(engine.mirrored().unwrap().1.same_node(&after));
            drop(before);
            assert!(
                retiring_sources
                    .iter()
                    .all(|source| source.upgrade().is_some()),
                "captured/pruned source remains alive through terminal ACK"
            );
            assert!(engine.inner.record_budget.retained_bytes() > 0);
            drop((engine_retired, editor_retired));
            while engine.step() {}
            assert_eq!(engine.inner.record_budget.retained_bytes(), 0);
            calls += 1;
        }
    }
    assert_eq!(calls, 16);
    assert!(
        removed >= 2,
        "actual undo/branch traces must retire generated source payloads"
    );
}

#[test]
fn staged_root_and_actual_guarded_editor_exchange_allocate_and_drop_nothing_under_shared_locks() {
    for shared in 0..2 {
        let (engine, mut session) = opened();
        let prepared = candidate(&mut session, "setFPS", "[31]");
        let transaction = guard(&engine);
        let pointer = if shared == 0 {
            &engine.inner.state as *const _ as usize
        } else {
            &engine.inner.staged as *const _ as usize
        };
        let (admitted, counts) = crate::allocation::watch(pointer, || {
            engine.stage_edit_publication_worker(7, transaction, &prepared, || Ok(()))
        });
        let admission = admitted.unwrap();
        assert!(counts.locks >= 1);
        assert_eq!(
            (counts.allocations, counts.deallocations),
            (0, 0),
            "admission {shared}: {counts:?}"
        );
        let guarded = session
            .guard_worker_publication_checked(prepared, || Ok(()))
            .unwrap_or_else(|(error, _)| panic!("{error:?}"));
        let (result, counts) = crate::allocation::watch(pointer, || {
            engine.commit_edit_publication_worker(7, transaction, guarded, admission)
        });
        let (_, engine_retired, editor_retired) =
            result.unwrap_or_else(|(error, _, _)| panic!("{error:?}"));
        assert!(counts.locks >= 1);
        assert_eq!(
            (counts.allocations, counts.deallocations),
            (0, 0),
            "atomic commit {shared}: {counts:?}"
        );
        drop((engine_retired, editor_retired));
    }
}

#[test]
fn cancellation_after_typed_root_preparation_leaves_engine_and_moved_session_heads_unchanged() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Barrier, OnceLock};
    static ENABLED: AtomicBool = AtomicBool::new(false);
    static CLOCK_CALLS: AtomicUsize = AtomicUsize::new(0);
    static BARRIERS: OnceLock<[Barrier; 2]> = OnceLock::new();
    fn raced_clock() -> f64 {
        if ENABLED.load(Ordering::SeqCst) && CLOCK_CALLS.fetch_add(1, Ordering::SeqCst) == 1 {
            let barriers = BARRIERS.get().unwrap();
            barriers[0].wait();
            barriers[1].wait();
        }
        0.0
    }
    BARRIERS.set([Barrier::new(2), Barrier::new(2)]).unwrap();
    let (engine, mut session) = opened_with(raced_clock);
    let before = session.report().clone();
    let old = engine.generation().unwrap();
    let prepared = candidate(&mut session, "setFPS", "[31]");
    let transaction = guard(&engine);
    let admission = engine
        .stage_edit_publication_worker(7, transaction, &prepared, || Ok(()))
        .unwrap();
    ENABLED.store(true, Ordering::SeqCst);
    let worker = engine.clone();
    let (ready, ready_rx) = std::sync::mpsc::sync_channel(0);
    let (ack, ack_rx) = std::sync::mpsc::sync_channel(0);
    let thread = std::thread::spawn(move || {
        let guarded = session
            .guard_worker_publication_checked(prepared, || Ok(()))
            .unwrap_or_else(|(error, _)| panic!("{error:?}"));
        let refused = worker
            .commit_edit_publication_worker(7, transaction, guarded, admission)
            .err()
            .expect("cancel before final shared head exchange");
        ready.send(()).unwrap();
        ack_rx.recv().unwrap();
        drop(refused);
        session
    });
    BARRIERS.get().unwrap()[0].wait();
    engine.discard_batch(7);
    BARRIERS.get().unwrap()[1].wait();
    ready_rx.recv().unwrap();
    while engine.step() {} // batch retirement alone cannot release the caller lease
    assert!(engine.inner.record_budget.retained_bytes() > 0);
    ack.send(()).unwrap();
    let session = thread.join().unwrap();
    ENABLED.store(false, Ordering::SeqCst);
    assert!(session.report().same_node(&before));
    assert!(engine.mirrored().unwrap().1.same_node(&before));
    assert!(Arc::ptr_eq(&old, &engine.generation().unwrap()));
    while engine.step() {}
    assert_eq!(engine.inner.record_budget.retained_bytes(), 0);
}

#[test]
fn ordinary_guarded_receiver_retains_removed_unknown_raw_nodes_through_terminal_ack() {
    let (engine, session) = opened();
    let before = engine.mirrored().unwrap().1;
    let Value::Obj(root) = &before else { panic!() };
    let weak_root = Arc::downgrade(root);
    let Value::Obj(unknown) = before.get("unknownBefore").unwrap() else {
        panic!()
    };
    let weak_unknown = Arc::downgrade(unknown);
    drop(before);
    drop(session);
    let transaction = guard(&engine);
    let text = br#"{"version":5,"canvas_width":320,"canvas_height":200,"fps":31}"#.to_vec();
    let bytes = text.len() + 512;
    engine
        .stage_guarded_admitted(7, transaction, Change::Properties(text), bytes)
        .unwrap();
    let (_, retired, ()) = engine
        .commit_record_with(1, 7, transaction, || ())
        .ok()
        .expect("existing receiver commit");
    assert!(engine.mirrored().unwrap().1.get("unknownBefore").is_none());
    assert!(weak_root.upgrade().is_some());
    assert!(
        weak_unknown.upgrade().is_some(),
        "removed raw unknown fields have a worker owner until terminal ACK"
    );
    assert!(engine.inner.record_budget.retained_bytes() > 0);
    drop(retired);
    assert!(weak_root.upgrade().is_none());
    assert!(weak_unknown.upgrade().is_none());
    assert_eq!(engine.inner.record_budget.retained_bytes(), 0);
}

#[test]
fn minimal_root_without_keyframes_retains_removed_unknown_nodes_until_terminal_ack() {
    let engine = Engine::new(clock, Budgets::for_device(None));
    engine.stage(0, Change::Reset).unwrap();
    engine.stage(0, Change::Properties(br#"{"version":5,"canvas_width":320,"canvas_height":200,"fps":20,"unknownBefore":{"future":["raw-only"]}}"#.to_vec())).unwrap();
    engine.commit(0, 0).unwrap();
    while engine.step() {}
    let before = engine.inner.mirror.lock().unwrap().tree.clone();
    assert!(
        before.get("keyframes").is_none(),
        "this valid minimal root has no native table"
    );
    let Value::Obj(root) = &before else { panic!() };
    let weak_root = Arc::downgrade(root);
    let Value::Obj(unknown) = before.get("unknownBefore").unwrap() else {
        panic!()
    };
    let weak_unknown = Arc::downgrade(unknown);
    drop(before);
    let transaction = guard(&engine);
    let text = br#"{"version":5,"canvas_width":320,"canvas_height":200,"fps":31}"#.to_vec();
    let bytes = text.len() + 512;
    engine
        .stage_guarded_admitted(7, transaction, Change::Properties(text), bytes)
        .unwrap();
    let (_, retired, ()) = engine
        .commit_record_with(1, 7, transaction, || ())
        .ok()
        .expect("valid minimal receiver commit");
    assert!(
        engine
            .inner
            .mirror
            .lock()
            .unwrap()
            .tree
            .get("unknownBefore")
            .is_none()
    );
    assert!(
        weak_root.upgrade().is_some(),
        "every displaced canonical raw head belongs to retirement"
    );
    assert!(
        weak_unknown.upgrade().is_some(),
        "absence of keyframes cannot release unknown raw values before ACK"
    );
    assert!(engine.inner.record_budget.retained_bytes() > 0);
    drop(retired);
    assert!(weak_root.upgrade().is_none());
    assert!(weak_unknown.upgrade().is_none());
    assert_eq!(engine.inner.record_budget.retained_bytes(), 0);
}
