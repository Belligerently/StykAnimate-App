use super::*;
use crate::{Budgets, mirror::{MetadataField, SourceDisposition, Table}};
use styk_doc::Value;
use styk_formats::container::MAX_MATERIALIZED_BYTES;

fn clock() -> f64 { 0.0 }
fn opened() -> Engine {
    let engine = Engine::new(clock, Budgets::for_device(None));
    engine.stage(0, Change::Reset).unwrap();
    engine.stage(0, Change::Properties(br#"{"version":5,"canvas_width":320,"canvas_height":200,"fps":20,"_styk":{"future":"preserved"}}"#.to_vec())).unwrap();
    for table in [Table::Figures, Table::Keyframes, Table::Backgrounds] { engine.stage(0, Change::Layout(table, vec![])).unwrap(); }
    engine.commit(1, 0).unwrap();
    engine
}
fn guard(engine: &Engine) -> RecordGuard {
    let generation = engine.generation().unwrap();
    RecordGuard { generation: generation.id, lineage: generation.lineage, revision: generation.revision, nonce: 17 }
}
fn lengths(skeleton: usize, arrays: usize, payloads: usize) -> RecordLengths {
    RecordLengths { skeleton, arrays, payloads, materialization_limit: MAX_MATERIALIZED_BYTES }
}
fn send(engine: &Engine, batch: u32, ticket: u64, sequence: u64, kind: PacketKind, offset: usize, bytes: &[u8]) -> RecordAck {
    engine.record_packet(batch, ticket, sequence, Packet { kind, offset, bytes: bytes.to_vec() }).unwrap();
    assert!(engine.step());
    engine.take_record_ack(batch, ticket).unwrap()
}
fn finish(engine: &Engine, batch: u32, ticket: u64, sequence: u64, lengths: RecordLengths) -> RecordAck {
    engine.finish_record(batch, ticket, sequence, lengths).unwrap();
    assert!(engine.step());
    engine.take_record_ack(batch, ticket).unwrap()
}
fn animation(engine: &Engine, batch: u32, ticket: u64) {
    let skeleton = br#"{"version":1,"channels":[],"future":{"unknown":"exact"}}"#;
    engine.begin_record(batch, guard(engine), ticket, RecordTarget::Metadata(MetadataField::Animation)).unwrap();
    assert!(send(engine, batch, ticket, 1, PacketKind::Skeleton, 0, skeleton).error.is_none());
    assert!(finish(engine, batch, ticket, 2, lengths(skeleton.len(), 0, 0)).error.is_none());
}

#[test]
fn one_tagged_ack_is_required_even_after_worker_processing() {
    let engine = opened();
    engine.begin_record(7, guard(&engine), 123, RecordTarget::Metadata(MetadataField::Animation)).unwrap();
    engine.record_packet(7, 123, 1, Packet { kind: PacketKind::Skeleton, offset: 0, bytes: b"null".to_vec() }).unwrap();
    assert!(matches!(engine.finish_record(7, 123, 2, lengths(4, 0, 0)), Err(TransferError::Pending)));
    assert!(engine.take_record_ack(7, 123).is_none(), "no response before worker execution");
    assert!(engine.step());
    assert!(engine.take_record_ack(7, 124).is_none(), "another ticket cannot consume the response");
    assert!(matches!(engine.finish_record(7, 123, 2, lengths(4, 0, 0)), Err(TransferError::Pending)));
    assert_eq!(engine.take_record_ack(7, 123).unwrap(), RecordAck { ticket: 123, sequence: 1, finished: false, error: None });
    engine.finish_record(7, 123, 2, lengths(4, 0, 0)).unwrap();
    engine.step();
    assert!(engine.commit_record(2, 7, guard(&engine)).is_err(), "the terminal ACK must be consumed before publication");
    assert!(engine.inner.staged.lock().contains_key(&7));
    assert!(matches!(engine.begin_record(7, guard(&engine), 125, RecordTarget::Metadata(MetadataField::Layers)), Err(TransferError::Pending)));
    assert!(engine.take_record_ack(7, 123).unwrap().finished);
    engine.begin_record(7, guard(&engine), 125, RecordTarget::Metadata(MetadataField::Layers)).unwrap();
}

#[test]
fn oversized_packets_do_not_allocate_receiver_work_or_consume_the_cursor() {
    let engine = opened();
    engine.begin_record(7, guard(&engine), 1, RecordTarget::Metadata(MetadataField::Animation)).unwrap();
    assert!(matches!(engine.record_packet(7, 1, 1, Packet { kind: PacketKind::Skeleton, offset: 0, bytes: vec![0; MAX_RECORD_PACKET_BYTES + 1] }), Err(TransferError::PacketSize)));
    assert_eq!(engine.inner.state.lock().retained_tasks.len(), 0);
    assert!(send(&engine, 7, 1, 1, PacketKind::Skeleton, 0, b"null").error.is_none());
    assert!(finish(&engine, 7, 1, 2, lengths(4, 0, 0)).error.is_none());
}

#[test]
fn invalid_offsets_sequences_lengths_and_markers_never_stage_a_value() {
    for case in 0..4 {
        let engine = opened();
        engine.begin_record(7, guard(&engine), 1, RecordTarget::Metadata(MetadataField::Animation)).unwrap();
        let ack = match case {
            0 => send(&engine, 7, 1, 1, PacketKind::Skeleton, 1, b"null"),
            1 => send(&engine, 7, 1, 2, PacketKind::Skeleton, 0, b"null"),
            2 => { send(&engine, 7, 1, 1, PacketKind::Skeleton, 0, b"null"); finish(&engine, 7, 1, 2, lengths(3, 0, 0)) },
            _ => { let text = br#""S2B:u8:0:0:1""#; send(&engine, 7, 1, 1, PacketKind::Skeleton, 0, text); finish(&engine, 7, 1, 2, lengths(text.len(), 0, 0)) },
        };
        assert!(ack.error.is_some());
        assert_eq!(engine.inner.staged.lock().get(&7).unwrap().changes.len(), 0);
        assert_eq!(engine.inner.record_slots.load(Ordering::SeqCst), 0, "refused payloads retire on workers");
        assert_eq!(engine.generation().unwrap().revision, 1);
    }
}

#[test]
fn near_limit_expansion_is_admitted_exactly_before_terminal_ack() {
    for (references, should_admit) in [(5, true), (100, false)] {
        let engine = opened();
        engine.begin_record(7, guard(&engine), 1, RecordTarget::Metadata(MetadataField::Layers)).unwrap();
        let text = format!("[{}]", vec!["\"S2B:f64:0:0:33\""; references].join(","));
        let arrays: Vec<u8> = (0..33).flat_map(|index| (index as f64 * 0.125).to_le_bytes()).collect();
        send(&engine, 7, 1, 1, PacketKind::Skeleton, 0, text.as_bytes());
        send(&engine, 7, 1, 2, PacketKind::Arrays, 0, &arrays);
        let ack = finish(&engine, 7, 1, 3, RecordLengths { materialization_limit: 4096, ..lengths(text.len(), arrays.len(), 0) });
        assert_eq!(ack.error.is_none(), should_admit);
        assert_eq!(engine.inner.staged.lock().get(&7).unwrap().changes.len(), usize::from(should_admit));
        assert_eq!(engine.generation().unwrap().revision, 1, "the terminal ACK does not publish a document");
    }
}

#[test]
fn a_dense_record_only_appends_or_parses_when_workers_run() {
    let engine = opened();
    engine.begin_record(7, guard(&engine), 1, RecordTarget::Metadata(MetadataField::Animation)).unwrap();
    let text = format!("{{\"version\":1,\"channels\":[],\"future\":\"{}\"}}", "x".repeat(1024 * 1024));
    let mut sequence = 1;
    for (index, packet) in text.as_bytes().chunks(MAX_RECORD_PACKET_BYTES).enumerate() {
        engine.record_packet(7, 1, sequence, Packet { kind: PacketKind::Skeleton, offset: index * MAX_RECORD_PACKET_BYTES, bytes: packet.to_vec() }).unwrap();
        let transfer = engine.transfer(7, 1).unwrap();
        assert_eq!(transfer.buffer.lock().unwrap().record.as_ref().unwrap().skeleton.len(), index * MAX_RECORD_PACKET_BYTES);
        assert_eq!(engine.inner.staged.lock().get(&7).unwrap().changes.len(), 0);
        engine.step();
        assert!(engine.take_record_ack(7, 1).unwrap().error.is_none());
        sequence += 1;
    }
    engine.finish_record(7, 1, sequence, lengths(text.len(), 0, 0)).unwrap();
    assert_eq!(engine.inner.staged.lock().get(&7).unwrap().changes.len(), 0);
    engine.step();
    assert!(engine.take_record_ack(7, 1).unwrap().error.is_none());
    assert!(engine.try_commit(2, 7).unwrap().is_none(), "a chunked batch cannot prepare on the interface");
    engine.commit_record(2, 7, guard(&engine)).unwrap();
    assert_eq!(engine.inner.mirror.lock().unwrap().tree.path(&["_styk", "animation", "future"]).and_then(Value::as_str).unwrap().len(), 1024 * 1024);
}

#[test]
fn cancellation_holds_reservations_until_workers_retire_payloads() {
    let engine = opened();
    for index in 0..MAX_RECORD_TRANSFERS {
        let batch = index as u32 + 7;
        engine.begin_record(batch, guard(&engine), 1, RecordTarget::Metadata(MetadataField::Animation)).unwrap();
        engine.record_packet(batch, 1, 1, Packet { kind: PacketKind::Skeleton, offset: 0, bytes: vec![b' '; MAX_RECORD_PACKET_BYTES] }).unwrap();
        engine.discard_batch(batch);
    }
    assert_eq!(engine.inner.record_slots.load(Ordering::SeqCst), MAX_RECORD_TRANSFERS);
    assert!(matches!(engine.begin_record(99, guard(&engine), 1, RecordTarget::Metadata(MetadataField::Animation)), Err(TransferError::Capacity)));
    assert_eq!(engine.inner.state.lock().retained_tasks.len(), MAX_RECORD_TRANSFERS * 2);
    for _ in 0..100000 { engine.discard_batch(7); }
    assert_eq!(engine.inner.state.lock().retained_tasks.len(), MAX_RECORD_TRANSFERS * 2);
    while engine.step() {}
    assert_eq!(engine.inner.record_slots.load(Ordering::SeqCst), 0);
    engine.begin_record(99, guard(&engine), 1, RecordTarget::Metadata(MetadataField::Animation)).unwrap();
}

#[test]
fn replacement_refuses_pending_and_ready_payloads_and_retains_cleanup() {
    for ready in [false, true] {
        let engine = opened();
        if ready { animation(&engine, 7, 1); }
        else {
            engine.begin_record(7, guard(&engine), 1, RecordTarget::Metadata(MetadataField::Animation)).unwrap();
            engine.record_packet(7, 1, 1, Packet { kind: PacketKind::Skeleton, offset: 0, bytes: b"null".to_vec() }).unwrap();
        }
        engine.open(br#"{"version":5,"canvas_width":320,"canvas_height":200,"fps":20,"figure_types":[],"keyframes":[{"duration_raw":1,"objects":[]}]}"#, "styk", "replacement").unwrap();
        assert!(!engine.inner.staged.lock().contains_key(&7));
        assert!(engine.take_record_ack(7, 1).is_none());
        assert!(matches!(engine.finish_record(7, 1, 2, lengths(4, 0, 0)), Err(TransferError::Stale)));
        while engine.step() {}
        assert_eq!(engine.inner.record_slots.load(Ordering::SeqCst), 0);
        assert!(engine.inner.mirror.lock().unwrap().tree.path(&["_styk", "animation"]).is_none());
    }
}

const EMPTY_OPEN: &[u8] = br#"{"version":5,"canvas_width":320,"canvas_height":200,"fps":20,"figure_types":[],"keyframes":[{"duration_raw":1,"objects":[]}]}"#;

#[test]
fn delayed_publication_cleanup_preserves_newer_packet_and_terminal_ack() {
    use std::sync::Barrier;
    for terminal in [false, true] {
        let engine = opened();
        animation(&engine, 7, 1);
        let barriers = Arc::new([Barrier::new(2), Barrier::new(2)]);
        // The accepted open has exchanged State but has not begun its planner
        // or record cleanup. A later actual open can publish in this interval.
        let pointer = &engine.inner.planning as *const _ as usize;
        let state_pointer = &engine.inner.state as *const _ as usize;
        let worker = engine.clone();
        let wait = barriers.clone();
        let thread = std::thread::spawn(move || crate::allocation::watch(state_pointer, || {
            crate::allocation::with_pause(pointer, 1, wait, || worker.open(EMPTY_OPEN, "styk", "earlier"))
        }));
        barriers[0].wait();
        let newer = engine.open(EMPTY_OPEN, "styk", "newer").unwrap();
        while engine.step() {}
        let transaction = guard(&engine);
        assert_eq!(transaction.generation, newer.generation);
        engine.begin_record(9, transaction, 91, RecordTarget::Metadata(MetadataField::Animation)).unwrap();
        engine.record_packet(9, 91, 1, Packet { kind: PacketKind::Skeleton, offset: 0, bytes: b"null".to_vec() }).unwrap();
        assert!(engine.step());
        if terminal {
            assert!(engine.take_record_ack(9, 91).unwrap().error.is_none());
            engine.finish_record(9, 91, 2, lengths(4, 0, 0)).unwrap();
            assert!(engine.step());
        }
        barriers[1].wait();
        let (earlier, counts) = thread.join().unwrap();
        earlier.unwrap();
        assert_eq!((counts.allocations, counts.deallocations), (0, 0), "{counts:?}");
        assert_eq!(engine.generation().unwrap().id, newer.generation);
        assert!(engine.record_transfer_current(9, 91), "earlier cleanup cannot cancel the current generation's transfer");
        while engine.step() {}
        assert!(!engine.inner.staged.lock().contains_key(&7), "the displaced scope is retired");
        assert_eq!(engine.inner.record_budget.records.load(Ordering::SeqCst), 1);
        assert!(engine.inner.record_budget.bytes.load(Ordering::SeqCst) > 0);
        assert_eq!(engine.inner.record_slots.load(Ordering::SeqCst), usize::from(!terminal));
        let ack = engine.take_record_ack(9, 91).expect("the exact current ACK remains available");
        assert_eq!(ack, RecordAck { ticket: 91, sequence: if terminal { 2 } else { 1 }, finished: terminal, error: None });
        if !terminal { assert!(finish(&engine, 9, 91, 2, lengths(4, 0, 0)).error.is_none()); }
        assert_eq!(engine.inner.staged.lock().get(&9).unwrap().changes.len(), 1);
        engine.discard_batch(9);
        while engine.step() {}
        assert_eq!(engine.inner.record_slots.load(Ordering::SeqCst), 0);
        assert_eq!(engine.inner.record_budget.records.load(Ordering::SeqCst), 0);
        assert_eq!(engine.inner.record_budget.bytes.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn publication_retires_its_displaced_scope_even_when_ids_publish_out_of_order() {
    use std::sync::Barrier;
    let engine = opened();
    let barriers = Arc::new([Barrier::new(2), Barrier::new(2)]);
    // An unguarded open reserves its ID before preparation. Another open can
    // win first; the earlier caller then retries against that actual head.
    let pointer = &engine.inner.state as *const _ as usize;
    let worker = engine.clone();
    let wait = barriers.clone();
    let thread = std::thread::spawn(move || crate::allocation::with_pause(pointer, 2, wait, || {
        worker.open(EMPTY_OPEN, "styk", "reserved earlier")
    }));
    barriers[0].wait();
    let later_id = engine.open(EMPTY_OPEN, "styk", "published first").unwrap();
    while engine.step() {}
    engine.begin_record(7, guard(&engine), 71, RecordTarget::Metadata(MetadataField::Animation)).unwrap();
    assert!(send(&engine, 7, 71, 1, PacketKind::Skeleton, 0, b"null").error.is_none());
    barriers[1].wait();
    let earlier_id = thread.join().unwrap().unwrap();
    assert!(earlier_id.generation < later_id.generation, "publication order is not generation-ID order");
    assert_eq!(engine.generation().unwrap().id, earlier_id.generation);
    assert!(!engine.inner.staged.lock().contains_key(&7), "the exact displaced generation must retire even if its ID is larger");
    assert_eq!(engine.inner.record_slots.load(Ordering::SeqCst), 1, "buffer ownership is retained until a worker disposes it");
    while engine.step() {}
    assert_eq!(engine.inner.record_slots.load(Ordering::SeqCst), 0);
    assert_eq!(engine.inner.record_budget.records.load(Ordering::SeqCst), 0);
    assert_eq!(engine.inner.record_budget.bytes.load(Ordering::SeqCst), 0);
    assert!(engine.take_record_ack(7, 71).is_none());
}

#[test]
fn stop_drains_partial_and_completed_staged_payloads_on_workers() {
    let engine = opened();
    animation(&engine, 7, 1);
    engine.begin_record(8, guard(&engine), 2, RecordTarget::Metadata(MetadataField::Layers)).unwrap();
    send(&engine, 8, 2, 1, PacketKind::Skeleton, 0, &vec![b' '; MAX_RECORD_PACKET_BYTES]);
    assert_eq!(engine.inner.record_slots.load(Ordering::SeqCst), 1);
    engine.stop();
    assert!(engine.inner.staged.lock().is_empty());
    assert_eq!(engine.inner.record_slots.load(Ordering::SeqCst), 1, "the caller only detached the dense buffer");
    assert_eq!(engine.inner.state.lock().retained_tasks.len(), 1);
    engine.work();
    assert_eq!(engine.inner.record_slots.load(Ordering::SeqCst), 0);
    assert_eq!(engine.inner.state.lock().retained_tasks.len(), 0);
}

#[test]
fn targeted_metadata_preserves_unknown_fields_and_exact_absence() {
    let mut mirror = super::super::Mirror::default();
    mirror.tree = styk_doc::json::parse(br#"{"_styk":{"future":{"unknown":true},"simpleFrameDraft":{"version":1,"frameId":"k0","future":"draft"}}}"#).unwrap();
    let baseline = styk_doc::json::parse(br#"{"objects":[],"_styk":{"frameId":"k0"},"future":{"dense":[1,2,3]}}"#).unwrap();
    let animation = styk_doc::json::parse(br#"{"version":1,"channels":[],"future":"animation"}"#).unwrap();
    let (next, _) = mirror.apply(LinkedList::from([
        Change::Resolved(RecordTarget::Metadata(MetadataField::DraftBaseline), Some(baseline.clone())),
        Change::Resolved(RecordTarget::Metadata(MetadataField::DraftAnimationBaseline), Some(animation.clone())),
    ])).unwrap();
    assert_eq!(styk_doc::json::to_string(next.path(&["_styk", "simpleFrameDraft", "baseline"]).unwrap()), styk_doc::json::to_string(&baseline));
    assert_eq!(styk_doc::json::to_string(next.path(&["_styk", "simpleFrameDraft", "animationBaseline"]).unwrap()), styk_doc::json::to_string(&animation));
    assert_eq!(next.path(&["_styk", "simpleFrameDraft", "future"]).and_then(Value::as_str), Some("draft"));
    assert_eq!(next.path(&["_styk", "future", "unknown"]).and_then(Value::as_bool), Some(true));
    mirror.tree = next;
    let (next, _) = mirror.apply(LinkedList::from([Change::Resolved(RecordTarget::Metadata(MetadataField::DraftAnimationBaseline), None)])).unwrap();
    assert!(next.path(&["_styk", "simpleFrameDraft", "animationBaseline"]).is_none());
    assert!(next.path(&["_styk", "simpleFrameDraft", "baseline"]).is_some());
}

#[test]
fn source_absence_is_an_atomic_final_disposition_and_never_removes_nonempty_sources() {
    let mut mirror = super::super::Mirror::default();
    mirror.tree = styk_doc::json::parse(br#"{"_styk":{"future":true,"frameSources":{"a":{"objects":[]}}}}"#).unwrap();
    assert!(mirror.apply(LinkedList::from([Change::Sources(SourceDisposition::AbsentIfEmpty)])).is_err());
    let (next, _) = mirror.apply(LinkedList::from([
        Change::Sources(SourceDisposition::AbsentIfEmpty),
        Change::Resolved(RecordTarget::Source("a".into()), None),
    ])).unwrap();
    assert!(next.path(&["_styk", "frameSources"]).is_none());
    assert_eq!(next.path(&["_styk", "future"]).and_then(Value::as_bool), Some(true));
    assert!(mirror.tree.path(&["_styk", "frameSources", "a"]).is_some());
}

#[test]
fn replacement_after_preparation_cannot_publish_or_change_the_canonical_mirror() {
    use std::sync::{Barrier, OnceLock};
    static ENABLED: AtomicBool = AtomicBool::new(false);
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    static BARRIERS: OnceLock<[Barrier; 2]> = OnceLock::new();
    fn race_clock() -> f64 {
        if ENABLED.load(Ordering::SeqCst) && CALLS.fetch_add(1, Ordering::SeqCst) == 1 {
            let barriers = BARRIERS.get().unwrap();
            barriers[0].wait();
            barriers[1].wait();
        }
        0.0
    }
    BARRIERS.set([Barrier::new(2), Barrier::new(2)]).unwrap();
    let engine = Engine::new(race_clock, Budgets::for_device(None));
    engine.stage(0, Change::Reset).unwrap();
    engine.stage(0, Change::Properties(br#"{"version":5,"canvas_width":320,"canvas_height":200,"fps":20}"#.to_vec())).unwrap();
    for table in [Table::Figures, Table::Keyframes, Table::Backgrounds] { engine.stage(0, Change::Layout(table, vec![])).unwrap(); }
    engine.commit(1, 0).unwrap();
    animation(&engine, 7, 1);
    let prepared = engine.mirrored_for_editing().unwrap().3;
    let previous_assets = engine.inner.mirror.lock().unwrap().assets.clone().unwrap();
    ENABLED.store(true, Ordering::SeqCst);
    let committing = engine.clone();
    let transaction = guard(&engine);
    let thread = std::thread::spawn(move || committing.commit_record(2, 7, transaction));
    BARRIERS.get().unwrap()[0].wait();
    engine.open(br#"{"version":5,"canvas_width":320,"canvas_height":200,"fps":20,"figure_types":[],"keyframes":[{"duration_raw":1,"objects":[]}]}"#, "styk", "replacement").unwrap();
    let replacement = engine.generation().unwrap();
    BARRIERS.get().unwrap()[1].wait();
    assert!(thread.join().unwrap().is_err());
    ENABLED.store(false, Ordering::SeqCst);
    assert!(Arc::ptr_eq(&replacement, &engine.generation().unwrap()), "the completed stale preparation cannot replace the new document");
    let (revision, tree, _, after) = engine.mirrored_for_editing().unwrap();
    assert_eq!(revision, 1);
    assert!(tree.path(&["_styk", "animation"]).is_none());
    assert!(Arc::ptr_eq(&prepared, &after), "actual typed view survives the refused publication");
    assert!(Arc::ptr_eq(&previous_assets, engine.inner.mirror.lock().unwrap().assets.as_ref().unwrap()));
}

#[test]
fn discarded_or_wrong_nonce_transactions_cannot_ack_an_empty_commit() {
    let engine = opened();
    let transaction = guard(&engine);
    animation(&engine, 7, 1);
    let wrong = RecordGuard { nonce: transaction.nonce + 1, ..transaction };
    assert!(engine.commit_record(2, 7, wrong).is_err());
    assert!(engine.inner.staged.lock().contains_key(&7), "another nonce cannot consume the current transaction");
    let generation = engine.generation().unwrap();
    engine.discard_batch(7);
    assert!(engine.commit_record(2, 7, transaction).is_err());
    assert!(Arc::ptr_eq(&generation, &engine.generation().unwrap()));
    assert_eq!(engine.mirrored().unwrap().0, 1);
    while engine.step() {}
}

#[test]
fn absent_records_preserve_absence_distinct_from_null_and_refuse_absent_table_slots() {
    for target in [RecordTarget::Metadata(MetadataField::Animation), RecordTarget::Source("retained".into())] {
        let engine = opened();
        engine.begin_record(7, guard(&engine), 1, target).unwrap();
        engine.finish_absent_record(7, 1, 1).unwrap();
        engine.step();
        assert!(engine.take_record_ack(7, 1).unwrap().error.is_none());
        engine.stage_guarded(7, guard(&engine), Change::Sources(SourceDisposition::AbsentIfEmpty)).unwrap();
        engine.commit_record(2, 7, guard(&engine)).unwrap();
        let (_, tree) = engine.mirrored().unwrap();
        assert!(tree.path(&["_styk", "animation"]).is_none());
        assert!(tree.path(&["_styk", "frameSources"]).is_none());
        assert_eq!(tree.path(&["_styk", "future"]).and_then(Value::as_str), Some("preserved"));
    }
    let engine = opened();
    engine.stage_guarded(7, guard(&engine), Change::Resolved(RecordTarget::Metadata(MetadataField::Animation), Some(Value::Null))).unwrap();
    engine.commit_record(2, 7, guard(&engine)).unwrap();
    assert!(matches!(engine.mirrored().unwrap().1.path(&["_styk", "animation"]), Some(Value::Null)));
    let engine = opened();
    engine.begin_record(7, guard(&engine), 1, RecordTarget::Table(Table::Keyframes, 0)).unwrap();
    engine.finish_absent_record(7, 1, 1).unwrap();
    engine.step();
    assert!(engine.take_record_ack(7, 1).unwrap().error.is_some());
    assert_eq!(engine.inner.staged.lock().get(&7).unwrap().changes.len(), 0);
}

#[test]
fn concurrent_workers_cannot_return_between_the_stop_flag_and_staged_retirement() {
    for _ in 0..20 {
        let engine = opened();
        engine.begin_record(7, guard(&engine), 1, RecordTarget::Metadata(MetadataField::Animation)).unwrap();
        send(&engine, 7, 1, 1, PacketKind::Skeleton, 0, &vec![b' '; MAX_RECORD_PACKET_BYTES]);
        let mut workers = vec![];
        for _ in 0..2 { let worker = engine.clone(); workers.push(std::thread::spawn(move || worker.work())); }
        engine.stop();
        engine.stop();
        for worker in workers { worker.join().unwrap(); }
        assert_eq!(engine.inner.record_slots.load(Ordering::SeqCst), 0);
        assert!(engine.inner.staged.lock().is_empty());
        assert!(engine.inner.state.lock().retained_tasks.is_empty());
    }
}

#[test]
fn discard_after_preparation_cannot_publish_or_change_the_canonical_mirror() {
    use std::sync::{Barrier, OnceLock};
    static ENABLED: AtomicBool = AtomicBool::new(false);
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    static BARRIERS: OnceLock<[Barrier; 2]> = OnceLock::new();
    fn race_clock() -> f64 {
        if ENABLED.load(Ordering::SeqCst) && CALLS.fetch_add(1, Ordering::SeqCst) == 1 {
            let barriers = BARRIERS.get().unwrap();
            barriers[0].wait();
            barriers[1].wait();
        }
        0.0
    }
    BARRIERS.set([Barrier::new(2), Barrier::new(2)]).unwrap();
    let engine = Engine::new(race_clock, Budgets::for_device(None));
    engine.stage(0, Change::Reset).unwrap();
    engine.stage(0, Change::Properties(br#"{"version":5,"canvas_width":320,"canvas_height":200,"fps":20}"#.to_vec())).unwrap();
    for table in [Table::Figures, Table::Keyframes, Table::Backgrounds] { engine.stage(0, Change::Layout(table, vec![])).unwrap(); }
    engine.commit(1, 0).unwrap();
    animation(&engine, 7, 1);
    let prepared = engine.mirrored_for_editing().unwrap().3;
    let previous_assets = engine.inner.mirror.lock().unwrap().assets.clone().unwrap();
    ENABLED.store(true, Ordering::SeqCst);
    let committing = engine.clone();
    let transaction = guard(&engine);
    let thread = std::thread::spawn(move || committing.commit_record(2, 7, transaction));
    BARRIERS.get().unwrap()[0].wait();
    let current = engine.generation().unwrap();
    engine.discard_batch(7);
    BARRIERS.get().unwrap()[1].wait();
    assert!(thread.join().unwrap().is_err());
    ENABLED.store(false, Ordering::SeqCst);
    assert!(Arc::ptr_eq(&current, &engine.generation().unwrap()), "the cancelled preparation cannot publish");
    let (revision, tree, _, after) = engine.mirrored_for_editing().unwrap();
    assert_eq!(revision, 1);
    assert!(tree.path(&["_styk", "animation"]).is_none());
    assert!(Arc::ptr_eq(&prepared, &after), "actual typed view survives the refused publication");
    assert!(Arc::ptr_eq(&previous_assets, engine.inner.mirror.lock().unwrap().assets.as_ref().unwrap()));
}


#[test]
fn completed_record_charges_bound_repeated_tickets_until_worker_retirement() {
    let engine = opened();
    let transaction = guard(&engine);
    for index in 0..100_000 {
        let result = engine.stage_guarded(7, transaction, Change::Sources(SourceDisposition::Preserve));
        if index < MAX_RECORD_CHANGES { assert!(result.is_ok()); }
        else { assert!(matches!(result, Err(TransferError::Capacity))); }
    }
    assert_eq!(engine.inner.staged.lock().get(&7).unwrap().changes.len(), MAX_RECORD_CHANGES);
    assert_eq!(engine.inner.record_budget.records.load(Ordering::SeqCst), MAX_RECORD_CHANGES);
    engine.discard_batch(7);
    assert_eq!(engine.inner.record_budget.records.load(Ordering::SeqCst), MAX_RECORD_CHANGES, "detachment cannot release a charge while its payload is retained");
    assert!(matches!(engine.begin_record(8, transaction, 1, RecordTarget::Metadata(MetadataField::Animation)), Err(TransferError::Capacity)));
    while engine.step() {}
    assert_eq!(engine.inner.record_budget.records.load(Ordering::SeqCst), 0);
    assert_eq!(engine.inner.record_budget.bytes.load(Ordering::SeqCst), 0);
    engine.begin_record(8, transaction, 1, RecordTarget::Metadata(MetadataField::Animation)).unwrap();
}

#[test]
fn retained_byte_budget_is_shared_across_batches_and_cancellation_cannot_reuse_it_early() {
    let engine = opened();
    let transaction = guard(&engine);
    // A conservative admitted charge exercises the limit without allocating 1 GiB.
    engine.stage_guarded_admitted(7, transaction, Change::Resolved(RecordTarget::Metadata(MetadataField::Animation), Some(Value::Null)), MAX_RECORD_RETAINED_BYTES - 4).unwrap();
    engine.begin_record(8, transaction, 1, RecordTarget::Metadata(MetadataField::Layers)).unwrap();
    assert!(send(&engine, 8, 1, 1, PacketKind::Skeleton, 0, b"null").error.is_none());
    assert_eq!(engine.inner.record_budget.bytes.load(Ordering::SeqCst), MAX_RECORD_RETAINED_BYTES);
    assert!(matches!(engine.stage_guarded(9, transaction, Change::Sources(SourceDisposition::Preserve)), Err(TransferError::Capacity)));
    engine.discard_batch(7);
    assert_eq!(engine.inner.record_budget.bytes.load(Ordering::SeqCst), MAX_RECORD_RETAINED_BYTES);
    while engine.step() {}
    assert_eq!(engine.inner.record_budget.bytes.load(Ordering::SeqCst), 4);
    assert!(finish(&engine, 8, 1, 2, lengths(4, 0, 0)).error.is_none());
    engine.discard_batch(8);
    while engine.step() {}
    assert_eq!(engine.inner.record_budget.bytes.load(Ordering::SeqCst), 0);
    assert_eq!(engine.inner.record_budget.records.load(Ordering::SeqCst), 0);
}

#[test]
fn parser_expansion_charge_is_reserved_before_a_completed_record_accumulates() {
    let engine = opened();
    let transaction = guard(&engine);
    let skeleton = br#"{"items":"S2B:f64:0:0:33","alias":"S2B:f64:0:0:33"}"#;
    let arrays: Vec<u8> = (0..33).flat_map(|index| (index as f64 * 0.125).to_le_bytes()).collect();
    let admitted = styk_formats::container::parse_lifted_owned_admitted_within(skeleton.to_vec(), arrays.clone(), vec![], MAX_MATERIALIZED_BYTES).unwrap();
    let initial = skeleton.len() + arrays.len();
    assert!(admitted.retained_bytes > initial);
    engine.stage_guarded_admitted(7, transaction, Change::Sources(SourceDisposition::Preserve), MAX_RECORD_RETAINED_BYTES - initial).unwrap();
    engine.begin_record(8, transaction, 1, RecordTarget::Metadata(MetadataField::Layers)).unwrap();
    assert!(send(&engine, 8, 1, 1, PacketKind::Skeleton, 0, skeleton).error.is_none());
    assert!(send(&engine, 8, 1, 2, PacketKind::Arrays, 0, &arrays).error.is_none());
    let response = finish(&engine, 8, 1, 3, lengths(skeleton.len(), arrays.len(), 0));
    assert!(response.finished);
    assert!(matches!(response.error, Some(TransferError::Capacity)));
    assert!(engine.inner.staged.lock().get(&8).unwrap().changes.is_empty());
    assert_eq!(engine.inner.record_budget.bytes.load(Ordering::SeqCst), MAX_RECORD_RETAINED_BYTES - initial);
    engine.discard_batch(7);
    engine.discard_batch(8);
    while engine.step() {}
    assert_eq!(engine.inner.record_budget.bytes.load(Ordering::SeqCst), 0);
}

#[test]
fn guarded_transactions_refuse_legacy_commit_and_preexisting_raw_changes() {
    let engine = opened();
    animation(&engine, 7, 1);
    assert!(engine.commit(2, 7).is_err());
    assert_eq!(engine.inner.staged.lock().get(&7).unwrap().changes.len(), 1);
    engine.commit_record(2, 7, guard(&engine)).unwrap();
    engine.stage(8, Change::Sources(SourceDisposition::Preserve)).unwrap();
    assert!(matches!(engine.begin_record(8, guard(&engine), 1, RecordTarget::Metadata(MetadataField::Layers)), Err(TransferError::LegacyBatch)));
    assert!(matches!(engine.stage_guarded(8, guard(&engine), Change::Sources(SourceDisposition::Preserve)), Err(TransferError::LegacyBatch)));
    assert_eq!(engine.inner.staged.lock().get(&8).unwrap().changes.len(), 1);
    assert!(engine.inner.staged.lock().get(&8).unwrap().guard.is_none());
    engine.commit(3, 8).unwrap();
}

#[test]
fn a_foreign_legacy_stage_cancels_guarded_ownership_and_retires_dense_bytes_on_workers() {
    let engine = opened();
    animation(&engine, 7, 1);
    let transaction = guard(&engine);
    let previous = engine.generation().unwrap();
    let charged = engine.inner.record_budget.bytes.load(Ordering::SeqCst);
    assert!(matches!(engine.stage(7, Change::Properties(vec![b' '; 1024 * 1024])), Err(TransferError::Stale)));
    assert!(!engine.inner.staged.lock().contains_key(&7));
    assert_eq!(engine.inner.record_budget.bytes.load(Ordering::SeqCst), charged);
    assert!(engine.commit_record(2, 7, transaction).is_err());
    assert!(Arc::ptr_eq(&previous, &engine.generation().unwrap()));
    assert_eq!(engine.inner.state.lock().retained_tasks.len(), 1);
    while engine.step() {}
    assert_eq!(engine.inner.record_budget.bytes.load(Ordering::SeqCst), 0);
}


#[test]
fn receiver_admission_allocates_and_disposes_nothing_under_shared_locks() {
    for shared in 0..2 {
        for admission in 0..2 {
            let engine = opened();
            let transaction = guard(&engine);
            let pointer = if shared == 0 { &engine.inner.state as *const _ as usize } else { &engine.inner.staged as *const _ as usize };
            let (result, counts) = crate::allocation::watch(pointer, || {
                if admission == 0 { engine.begin_record(7, transaction, 1, RecordTarget::Metadata(MetadataField::Animation)) }
                else { engine.stage_guarded(7, transaction, Change::Sources(SourceDisposition::Preserve)) }
            });
            assert!(result.is_ok());
            assert_eq!((counts.allocations, counts.deallocations), (0, 0), "shared={shared} admission={admission}: {counts:?}");
        }
    }
}


#[test]
fn receiver_refusal_and_completed_transfer_replacement_do_not_allocate_under_shared_locks() {
    for shared in 0..2 {
        for refusal in 0..4 {
            let engine = opened();
            let mut transaction = guard(&engine);
            match refusal {
                0 => transaction.generation += 1,
                1 => engine.stage(7, Change::Sources(SourceDisposition::Preserve)).unwrap(),
                2 => for batch in 20..28 { engine.stage(batch, Change::Sources(SourceDisposition::Preserve)).unwrap(); },
                _ => engine.stop(),
            }
            let pointer = if shared == 0 { &engine.inner.state as *const _ as usize } else { &engine.inner.staged as *const _ as usize };
            let (result, counts) = crate::allocation::watch(pointer, || engine.begin_record(7, transaction, 1, RecordTarget::Metadata(MetadataField::Animation)));
            assert!(result.is_err());
            assert_eq!((counts.allocations, counts.deallocations), (0, 0), "begin shared={shared} refusal={refusal}: {counts:?}");
            let (result, counts) = crate::allocation::watch(pointer, || engine.stage_guarded(7, transaction, Change::Sources(SourceDisposition::Preserve)));
            assert!(result.is_err());
            assert_eq!((counts.allocations, counts.deallocations), (0, 0), "stage shared={shared} refusal={refusal}: {counts:?}");
        }
        let engine = opened();
        let transaction = guard(&engine);
        let target = RecordTarget::Metadata(MetadataField::Animation);
        engine.begin_record(7, transaction, 1, target.clone()).unwrap();
        engine.finish_absent_record(7, 1, 1).unwrap();
        engine.step(); engine.take_record_ack(7, 1).unwrap();
        let previous = engine.inner.staged.lock().get(&7).unwrap().transfer.as_ref().map(Arc::downgrade).unwrap();
        let pointer = if shared == 0 { &engine.inner.state as *const _ as usize } else { &engine.inner.staged as *const _ as usize };
        let (result, counts) = crate::allocation::watch(pointer, || engine.begin_record(7, transaction, 2, target));
        result.unwrap();
        assert!(previous.upgrade().is_none());
        assert_eq!((counts.allocations, counts.deallocations), (0, 0), "replace shared={shared}: {counts:?}");
        let (result, counts) = crate::allocation::watch(pointer, || engine.stage_guarded(7, transaction, Change::Sources(SourceDisposition::Preserve)));
        result.unwrap();
        assert_eq!((counts.allocations, counts.deallocations), (0, 0), "existing stage shared={shared}: {counts:?}");
    }
}

#[test]
fn legacy_admission_is_bounded_refuses_observably_and_retires_refused_dense_values() {
    for shared in 0..2 {
        let engine = opened();
        let pointer = if shared == 0 { &engine.inner.state as *const _ as usize } else { &engine.inner.staged as *const _ as usize };
        for batch in 0..8 {
            for _ in 0..2 {
                let (result, counts) = crate::allocation::watch(pointer, || engine.stage(batch, Change::Sources(SourceDisposition::Preserve)));
                result.unwrap();
                assert_eq!((counts.allocations, counts.deallocations), (0, 0), "legacy admission shared={shared}: {counts:?}");
            }
        }
        let capacity = engine.inner.staged.lock().capacity();
        let bytes: Arc<str> = Arc::from("q".repeat(1024 * 1024));
        let weak = Arc::downgrade(&bytes);
        let change = Change::Resolved(RecordTarget::Metadata(MetadataField::Animation), Some(Value::Str(bytes)));
        let (result, counts) = crate::allocation::watch(pointer, || engine.stage(8, change));
        assert!(matches!(result, Err(TransferError::Capacity)));
        assert_eq!((counts.allocations, counts.deallocations), (0, 0), "legacy refusal shared={shared}: {counts:?}");
        assert!(weak.upgrade().is_some(), "the refused dense payload is retained for worker disposal");
        assert_eq!(engine.inner.staged.lock().len(), 8);
        assert_eq!(engine.inner.staged.lock().capacity(), capacity);
        for batch in 0..8 { assert_eq!(engine.inner.staged.lock().get(&batch).unwrap().changes.len(), 2); }
        engine.discard_batch(0);
        while engine.step() {}
        assert!(weak.upgrade().is_none());
        engine.stage(8, Change::Sources(SourceDisposition::Preserve)).unwrap();
        assert_eq!(engine.inner.staged.lock().len(), 8);
        assert_eq!(engine.inner.staged.lock().capacity(), capacity);
        engine.stop();
        let (result, counts) = crate::allocation::watch(pointer, || engine.stage(9, Change::Sources(SourceDisposition::Preserve)));
        assert!(matches!(result, Err(TransferError::Cancelled)));
        assert_eq!((counts.allocations, counts.deallocations), (0, 0), "stopped legacy shared={shared}: {counts:?}");
        while engine.step() {}
    }
}


#[test]
fn stale_and_stopped_parse_error_ack_disposes_its_string_after_state_unlock() {
    use std::sync::Barrier;
    for stop in [false, true] {
        let engine = opened();
        let transaction = guard(&engine);
        engine.begin_record(7, transaction, 1, RecordTarget::Metadata(MetadataField::Animation)).unwrap();
        assert!(send(&engine, 7, 1, 1, PacketKind::Skeleton, 0, b"{").error.is_none());
        engine.finish_record(7, 1, 2, lengths(1, 0, 0)).unwrap();
        engine.step();
        assert!(matches!(engine.transfer(7, 1).unwrap().ack.lock().as_ref().unwrap().error, Some(TransferError::Record(_))), "an actual worker parse failure supplies the allocated error string");
        let barriers = Arc::new([Barrier::new(2), Barrier::new(2)]);
        let wait = barriers.clone();
        let worker = engine.clone();
        let pointer = &engine.inner.state as *const _ as usize;
        let thread = std::thread::spawn(move || crate::allocation::watch(pointer, || crate::allocation::with_pause(pointer, 1, wait, || worker.take_record_ack(7, 1))));
        // Reader has detached the allocated parse-error ACK, but not read State.
        barriers[0].wait();
        if stop { engine.stop(); }
        else { engine.open(br#"{"version":5,"canvas_width":20,"canvas_height":20,"fps":12,"figure_types":[],"keyframes":[{"duration_raw":1,"objects":[]}],"backgrounds":[]}"#, "json", "replacement").unwrap(); }
        barriers[1].wait();
        let (ack, counts) = thread.join().unwrap();
        assert!(matches!(ack.unwrap().error, Some(TransferError::Stale)));
        assert_eq!((counts.allocations, counts.deallocations), (0, 0), "stop={stop}: {counts:?}");
        while engine.step() {}
    }
}
