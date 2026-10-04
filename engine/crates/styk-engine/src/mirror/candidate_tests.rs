use super::*;
use crate::{Budgets, mirror::{MetadataField, RecordTarget}};
use styk_doc::json;
use styk_edit::{HistoryLimits, session::{RevisionCounters, Session}};
use std::sync::atomic::{AtomicUsize, Ordering};

fn clock() -> f64 { 0.0 }
fn parsed(text: &str) -> Value { json::parse(text.as_bytes()).unwrap() }
fn fixture(path: &str) -> Value {
    parsed(&std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path)).unwrap())
}
fn opened() -> Engine {
    let engine = Engine::new(clock, Budgets::for_device(None));
    engine.stage(0, Change::Reset).unwrap();
    engine.stage(0, Change::Properties(br#"{"unknownBefore":null,"version":5,"canvas_width":320,"canvas_height":200,"fps":20,"figure_types":[],"keyframes":[{"duration_raw":1,"objects":[]}],"backgrounds":[],"unknownAfter":{"keep":true}}"#.to_vec())).unwrap();
    engine.commit(0, 0).unwrap();engine
}
fn guard(engine: &Engine) -> RecordGuard {
    let generation = engine.generation().unwrap();
    RecordGuard { generation: generation.id, lineage: generation.lineage, revision: generation.revision, nonce: 911 }
}
fn stage(engine: &Engine) -> RecordGuard {
    let guard = guard(engine);
    engine.stage_guarded(7, guard, Change::Resolved(RecordTarget::Metadata(MetadataField::Animation), Some(Value::Null))).unwrap();guard
}
fn record(engine: &Engine, guard: RecordGuard, ticket: u64, target: RecordTarget, value: &Value) {
    let text = json::to_string(value);
    engine.begin_record(7, guard, ticket, target).unwrap();
    let mut sequence = 1;
    for (index, bytes) in text.as_bytes().chunks(MAX_RECORD_PACKET_BYTES).enumerate() {
        engine.record_packet(7, ticket, sequence, Packet { kind: PacketKind::Skeleton,
            offset: index * MAX_RECORD_PACKET_BYTES, bytes: bytes.to_vec() }).unwrap();
        assert!(engine.step());
        assert!(engine.take_record_ack(7, ticket).unwrap().error.is_none());
        sequence += 1;
    }
    engine.finish_record(7, ticket, sequence, RecordLengths { skeleton: text.len(), arrays: 0,
        payloads: 0, materialization_limit: styk_formats::container::MAX_MATERIALIZED_BYTES }).unwrap();
    assert!(engine.step());
    let ack = engine.take_record_ack(7, ticket).unwrap();
    assert!(ack.finished);assert!(ack.error.is_none());
}

#[test]
fn exact_prebuilt_assets_encode_new_glyphs_before_the_candidate_is_visible() {
    let engine = opened();let before = engine.generation().unwrap();
    let root = engine.mirrored().unwrap().1;
    let text = fixture("../styk-eval/tests/fixtures/drawlist/outlined-text.json");
    let guard = guard(&engine);
    let mut ticket = 1;
    for table in [Table::Figures, Table::Keyframes] {
        let values = text.get(table.key()).unwrap().elements().unwrap();
        engine.stage_guarded(7, guard, Change::Layout(table, vec![-1; values.len()])).unwrap();
        for (at, value) in values.iter().enumerate() {
            record(&engine, guard, ticket, RecordTarget::Table(table, at as u32), &value);
            ticket += 1;
        }
    }
    let candidate = engine.prepare_record_publication_worker(1, 7, guard).unwrap();
    assert!(candidate.stores_generation());
    let prepared = candidate.generation();
    let id = prepared.id;let document = prepared.prepared.clone();let assets = prepared.assets.clone();
    assert_ne!(id, before.id);assert!(Arc::ptr_eq(&engine.generation().unwrap(), &before));
    assert!(candidate.mirror.tree.same_node(&root));
    assert!(engine.inner.mirror.try_lock().is_err(), "the SAME worker-scoped Mirror guard stays owned");
    let glyph = document.doc.figures[0].texts[0].glyphs[0].clone();
    assert!(before.assets.glyph(&glyph).is_none(), "old assets cannot encode this recorded text definition before publication");
    assert!(assets.glyph(&glyph).is_some(), "candidate assets contain its exact typed glyph identity");
    let frame = document.evaluate(0.0);let scene = styk_eval::geometry::build(&document.doc, &frame);
    let list = styk_eval::drawlist::encode(&document.doc, &assets, &scene);
    assert!(!list.prims.is_empty(), "recorded outlined glyphs encode into raster-group triangles");
    let wrong_assets = styk_eval::drawlist::encode(&document.doc, &before.assets, &scene);
    assert!(wrong_assets.prims.len() < list.prims.len(), "encoding with the previous assets loses the new glyph triangles");
    let calls = AtomicUsize::new(0);
    let (result, counts) = crate::allocation::watch(&engine.inner.state as *const _ as usize, ||
        candidate.commit_with_worker(|| { calls.fetch_add(1, Ordering::SeqCst); 73 }));
    let (committed, retired, output) = result.unwrap_or_else(|(error, _, _)| panic!("{error}"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);assert_eq!(output, 73);
    assert_eq!((counts.allocations, counts.deallocations), (0, 0), "{counts:?}");
    let published = engine.generation().unwrap();assert_eq!(published.id, id);assert_eq!(committed.generation, id);
    assert!(Arc::ptr_eq(&published.prepared, &document));assert!(Arc::ptr_eq(&published.assets, &assets));
    {
        let mirror = engine.inner.mirror.lock().unwrap();
        assert!(Arc::ptr_eq(mirror.prepared.as_ref().unwrap(), &document));
        assert!(Arc::ptr_eq(mirror.assets.as_ref().unwrap(), &assets));
    }
    assert!(retired.canonical_root.as_ref().unwrap().same_node(&root));
    assert_eq!(engine.mirrored().unwrap().1.get("unknownAfter"), root.get("unknownAfter"));
    drop(retired);
    assert!(engine.prepare_record_publication_worker(1, 7, guard).is_err(), "consumed nonce cannot publish an empty second transaction");
}

#[test]
fn cancellation_after_encode_is_linearized_at_the_original_final_state_exchange() {
    use std::sync::Barrier;
    for before_exchange in [true, false] {
        let engine = opened();let before = engine.generation().unwrap();let transaction = stage(&engine);
        let barriers = Arc::new([Barrier::new(2), Barrier::new(2)]);
        let calls = Arc::new(AtomicUsize::new(0));let invoked = calls.clone();
        let worker = engine.clone();let wait = barriers.clone();
        let pointer = &engine.inner.state as *const _ as usize;
        let thread = std::thread::spawn(move || {
            let candidate = worker.prepare_record_publication_worker(1, 7, transaction).unwrap();
            let document = &candidate.generation().prepared;
            let frame = document.evaluate(0.0);let scene = styk_eval::geometry::build(&document.doc, &frame);
            let encoded = styk_eval::drawlist::encode(&document.doc, &candidate.generation().assets, &scene);
            let id = candidate.generation().id;
            let commit = || crate::allocation::watch(pointer, || candidate.commit_with_worker(|| invoked.fetch_add(1, Ordering::SeqCst)));
            let (result, counts) = if before_exchange {
                crate::allocation::with_pause(pointer, 1, wait, commit)
            } else {
                let result = commit();wait[0].wait();wait[1].wait();result
            };
            assert_eq!((counts.allocations, counts.deallocations), (0, 0));
            assert_eq!(result.is_ok(), !before_exchange);
            drop((result, encoded));id
        });
        barriers[0].wait();
        engine.discard_batch(7);
        barriers[1].wait();
        let id = thread.join().unwrap();
        let current = engine.generation().unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), usize::from(!before_exchange));
        assert_eq!(current.id, if before_exchange { before.id } else { id });
        assert_eq!(engine.mirrored().unwrap().1.path(&["_styk", "animation"]).is_some(), !before_exchange);
        while engine.step() {}
    }
}

#[test]
fn prepared_cancel_stop_stale_and_drop_keep_heads_and_callback_unconsumed() {
    for action in ["cancel", "stop", "stale", "lineage", "drop"] {
        let engine = opened();let before = engine.generation().unwrap();let root = engine.mirrored().unwrap().1;
        let transaction = stage(&engine);
        let candidate = engine.prepare_record_publication_worker(1, 7, transaction).unwrap();
        let new_id = candidate.generation().id;
        match action {
            "cancel" => engine.discard_batch(7),
            "stop" => engine.stop(),
            "stale" | "lineage" => {
                let next = Generation { id: 0, lineage: if action == "lineage" { before.lineage + 1 } else { before.lineage },
                    revision: before.revision + 1, prepared: before.prepared.clone(), assets: before.assets.clone(), open_ms: 0.0, anchors: None };
                engine.publish(next);
            }
            _ => {},
        }
        let active = engine.generation().unwrap();
        if action == "drop" { drop(candidate); }
        else {
            let calls = AtomicUsize::new(0);let payload = Arc::new(vec![17u8; 1024 * 1024]);let weak = Arc::downgrade(&payload);
            let callback = || { calls.fetch_add(1, Ordering::SeqCst); payload };
            let (result, counts) = crate::allocation::watch(&engine.inner.state as *const _ as usize, || candidate.commit_with_worker(callback));
            let (_, candidate, callback) = result.err().expect("final refusal");
            assert_eq!(calls.load(Ordering::SeqCst), 0);assert!(weak.upgrade().is_some());
            assert_eq!((counts.allocations, counts.deallocations), (0, 0), "{action}: {counts:?}");
            assert_eq!(candidate.generation().id, new_id);drop((candidate, callback));assert!(weak.upgrade().is_none());
        }
        assert!(Arc::ptr_eq(&active, &engine.generation().unwrap()));
        assert!(engine.mirrored().unwrap().1.same_node(&root));
        assert!(!engine.inner.staged.lock().contains_key(&7), "one original CommitMarker retired: {action}");
        while engine.step() {}
    }
}

#[test]
fn recorded_working_and_identical_byte_history_commit_borrow_the_exact_generation() {
    let reference = fixture("../../tests/session-live/native-drag-commit.json");
    let first = reference.get("steps").unwrap().item(0).unwrap();let history = first.get("history").unwrap();
    let n = |key| history.get(key).unwrap().as_f64().unwrap() as u64;
    let counters = RevisionCounters { revision: n("revision"), serial: n("revisionSerial"), saved: n("savedRevision") };
    let engine = Engine::new(clock, Budgets::for_device(None));
    engine.stage(0, Change::Reset).unwrap();engine.stage(0, Change::Properties(first.get("document").unwrap().as_str().unwrap().as_bytes().to_vec())).unwrap();
    engine.commit(counters.revision, 0).unwrap();
    let (_, tree, loader, prepared) = engine.mirrored_for_editing().unwrap();
    let mut session = Session::start_worker_prepared(tree, "{}", loader, prepared, counters).unwrap();
    session.prepare_worker_history(HistoryLimits::default(), 0.0, || Ok(())).unwrap();let mut session = session.into_worker();
    let mut document = first.get("document").unwrap().as_str().unwrap().to_owned();
    for step in reference.get("steps").unwrap().elements().unwrap().iter().skip(1).take(7) {
        let call = step.get("call").unwrap().elements().unwrap();let method = call.get(0).unwrap();let method = method.as_str().unwrap();
        let args = Value::array(call.iter().skip(1).collect());let before_document = json::to_string(session.report());
        let before = engine.generation().unwrap();let prepared = session.prepare_live(method, &args, &Value::Null, || Ok(())).unwrap();
        let stored = prepared.revision() != prepared.before_revision();let transaction = guard(&engine);
        let admission = engine.stage_live_session_publication_worker(7, transaction, &prepared, prepared.result(), Arc::new(std::sync::atomic::AtomicBool::new(false)), || Ok(())).unwrap();
        let (publication, display, _) = session.guard(prepared, || Ok(())).unwrap_or_else(|(error, _)| panic!("{error}"));
        let candidate = engine.prepare_edit_publication_worker(7, transaction, publication, admission)
            .unwrap_or_else(|(error, _, _)| panic!("{error}"));
        let id = candidate.generation().id;let actual_assets = candidate.generation().assets.clone();
        assert_eq!(candidate.stores_generation(), stored);
        assert_eq!(id == before.id, !stored);assert!(Arc::ptr_eq(&engine.generation().unwrap(), &before));
        assert!(Arc::ptr_eq(&actual_assets, &before.assets) == !stored);
        let display = display.unwrap();
        let list = styk_eval::drawlist::encode(&candidate.generation().prepared.doc, &actual_assets, display.scene.scene());
        assert!(!list.prims.is_empty());
        let (result, counts) = crate::allocation::watch(&engine.inner.state as *const _ as usize, || candidate.commit_worker());
        let (_, engine_retired, editor_retired) = result.unwrap_or_else(|(error, _, _)| panic!("{error}"));
        assert_eq!((counts.allocations, counts.deallocations), (0, 0));
        assert_eq!(engine.generation().unwrap().id, id);
        assert!(Arc::ptr_eq(&engine.generation().unwrap().assets, &actual_assets));
        if let Some(next) = step.get("document") { document = next.as_str().unwrap().to_owned(); }
        assert_eq!(json::to_string(session.report()), document);
        if method == "commitDrag" { assert_eq!(before_document, document);assert!(stored, "native identical-byte commit still publishes history revision"); }
        drop((engine_retired, editor_retired, display));
    }
}
