use super::*;
use crate::{Budgets, mirror::{Change, MetadataField, RecordTarget, Record, Table}};
use std::sync::atomic::AtomicUsize;

fn clock() -> f64 { 0.0 }
fn opened() -> Engine { opened_with(clock) }
fn opened_with(clock: crate::Clock) -> Engine {
    let engine = Engine::new(clock, Budgets::for_device(None));
    engine.stage(0, Change::Reset).unwrap();
    engine.stage(0, Change::Properties(br#"{"version":5,"canvas_width":320,"canvas_height":200,"fps":20}"#.to_vec())).unwrap();
    engine.stage(0, Change::Layout(Table::Figures, vec![])).unwrap();
    engine.stage(0, Change::Layout(Table::Backgrounds, vec![])).unwrap();
    engine.stage(0, Change::Layout(Table::Keyframes, vec![-1, -1])).unwrap();
    for (index, skeleton) in [br#"{"duration_raw":2,"objects":[]}"#.as_slice(), br#"{"duration_raw":1,"objects":[]}"#.as_slice()].into_iter().enumerate() {
        engine.stage(0, Change::Set(Table::Keyframes, index as u32, Record { skeleton: skeleton.to_vec(), arrays: vec![], payloads: vec![] })).unwrap();
    }
    engine.commit(0, 0).unwrap();
    engine
}
fn guard(engine: &Engine) -> RecordGuard {
    let generation = engine.generation().unwrap();
    RecordGuard { generation: generation.id, lineage: generation.lineage, revision: generation.revision, nonce: 73 }
}
fn staged(engine: &Engine) -> RecordGuard {
    let guard = guard(engine);
    engine.stage_guarded(7, guard, Change::Resolved(RecordTarget::Metadata(MetadataField::Animation), Some(styk_doc::Value::Null))).unwrap();
    guard
}

#[test]
fn allocation_instrumentation_detects_actual_allocations_and_disposals_under_state() {
    let engine = opened();
    let (_, counts) = crate::allocation::watch(&engine.inner.state as *const _ as usize, || {
        let _state = engine.inner.state.lock();
        drop(std::hint::black_box(vec![23u8; 1024]));
    });
    assert_eq!(counts.locks, 1);
    assert!(counts.allocations > 0 && counts.deallocations > 0, "{counts:?}");
}

#[test]
fn paired_publication_runs_once_without_state_allocation_or_disposal_and_retains_old_heads() {
    let engine = opened();
    let old_head = engine.generation().unwrap();
    let old_generation = Arc::downgrade(&old_head);
    drop(old_head);
    let frame = engine.frame_now(0).unwrap();
    let old_frame = Arc::downgrade(&frame);
    drop(frame);
    let transaction = staged(&engine);
    let calls = AtomicUsize::new(0);
    let mut editor_head = vec![11u8; 1024 * 1024];
    let candidate = vec![29u8; 1024 * 1024];
    let (result, counts) = crate::allocation::watch(&engine.inner.state as *const _ as usize, || engine.commit_record_with(1, 7, transaction, || {
        assert!(crate::allocation::active(), "editor head exchange shares the final guarded State lock");
        calls.fetch_add(1, Ordering::SeqCst);
        std::mem::replace(&mut editor_head, candidate)
    }));
    let (committed, retired_engine, retired_editor) = result.ok().expect("paired commit");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(editor_head[0], 29);
    assert_eq!(retired_editor[0], 11);
    assert_eq!(engine.generation().unwrap().id, committed.generation);
    assert!(engine.mirrored().unwrap().1.path(&["_styk", "animation"]).is_some());
    assert!(old_frame.upgrade().is_some(), "the old dense frame cache remains retired, not disposed in State");
    assert!(old_generation.upgrade().is_some());
    assert!(counts.locks >= 2);
    assert_eq!((counts.allocations, counts.deallocations), (0, 0), "{counts:?}");
    engine.discard_batch(7);
    assert_eq!(calls.load(Ordering::SeqCst), 1, "cancellation after linearization cannot rerun or undo the exchanged heads");
    assert_eq!(engine.generation().unwrap().id, committed.generation);
    drop(retired_engine);
    drop(retired_editor);
    assert!(old_frame.upgrade().is_none());
    assert!(old_generation.upgrade().is_none());
}

#[test]
fn nonce_cancel_and_stop_refusals_return_the_uncalled_worker_callback() {
    for shared in 0..2 {
    for refusal in 0..3 {
        let engine = opened();
        let mut transaction = staged(&engine);
        let before = engine.generation().unwrap();
        match refusal { 0 => transaction.nonce += 1, 1 => engine.discard_batch(7), _ => engine.stop() }
        let calls = AtomicUsize::new(0);
        let candidate = Arc::new(vec![37u8; 1024 * 1024]);
        let weak = Arc::downgrade(&candidate);
        let counter = &calls;
        let pointer = if shared == 0 { &engine.inner.state as *const _ as usize } else { &engine.inner.staged as *const _ as usize };
        let (result, counts) = crate::allocation::watch(pointer, || engine.commit_record_with(1, 7, transaction, move || { counter.fetch_add(1, Ordering::SeqCst); candidate }));
        assert_eq!((counts.allocations, counts.deallocations), (0, 0), "shared={shared} refusal={refusal}: {counts:?}");
        let (_error, callback) = result.err().expect("refused callback retained");
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert!(Arc::ptr_eq(&before, &engine.generation().unwrap()));
        assert!(weak.upgrade().is_some(), "the refused callback retains its dense candidate for worker disposal");
        drop(callback);
        assert!(weak.upgrade().is_none());
        while engine.step() {}
    }
    }
}

#[test]
fn cancellation_after_preparation_never_invokes_the_paired_head_exchange() {
    use std::sync::{Barrier, OnceLock};
    static ENABLED: AtomicBool = AtomicBool::new(false);
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    static BARRIERS: OnceLock<[Barrier; 2]> = OnceLock::new();
    fn race_clock() -> f64 {
        if ENABLED.load(Ordering::SeqCst) && CALLS.fetch_add(1, Ordering::SeqCst) == 1 {
            let barriers = BARRIERS.get().unwrap(); barriers[0].wait(); barriers[1].wait();
        }
        0.0
    }
    BARRIERS.set([Barrier::new(2), Barrier::new(2)]).unwrap();
    let engine = opened_with(race_clock);
    let transaction = staged(&engine);
    let before = engine.generation().unwrap();
    let exchanged = Arc::new(AtomicUsize::new(0));
    ENABLED.store(true, Ordering::SeqCst);
    let worker = engine.clone();
    let counter = exchanged.clone();
    let thread = std::thread::spawn(move || worker.commit_record_with(1, 7, transaction, move || { counter.fetch_add(1, Ordering::SeqCst); }));
    BARRIERS.get().unwrap()[0].wait();
    engine.discard_batch(7);
    BARRIERS.get().unwrap()[1].wait();
    let refused = thread.join().unwrap();
    assert!(refused.is_err());
    assert_eq!(exchanged.load(Ordering::SeqCst), 0);
    assert!(Arc::ptr_eq(&before, &engine.generation().unwrap()));
    drop(refused);
    ENABLED.store(false, Ordering::SeqCst);
}

#[test]
fn publication_preserves_live_projection_settings_changed_during_worker_preparation() {
    use std::sync::{Barrier, OnceLock};
    static ENABLED: AtomicBool = AtomicBool::new(false);
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    static BARRIERS: OnceLock<[Barrier; 2]> = OnceLock::new();
    fn race_clock() -> f64 {
        if ENABLED.load(Ordering::SeqCst) && CALLS.fetch_add(1, Ordering::SeqCst) == 1 {
            let barriers = BARRIERS.get().unwrap(); barriers[0].wait(); barriers[1].wait();
        }
        0.0
    }
    BARRIERS.set([Barrier::new(2), Barrier::new(2)]).unwrap();
    let engine = opened_with(race_clock);
    let transaction = staged(&engine);
    ENABLED.store(true, Ordering::SeqCst);
    let worker = engine.clone();
    let thread = std::thread::spawn(move || worker.commit_record_with(1, 7, transaction, || ()));
    BARRIERS.get().unwrap()[0].wait();
    engine.set_hidden(&[71, 73]); engine.set_effects(false); engine.request(1, &[1]);
    BARRIERS.get().unwrap()[1].wait();
    assert!(thread.join().unwrap().is_ok());
    ENABLED.store(false, Ordering::SeqCst);
    let state = engine.inner.state.lock();
    assert_eq!(&*state.frames.hidden, &[71, 73]);
    assert!(!state.frames.effects);
    drop(state);
    // The next generation retains playhead/control settings while its old work
    // is retired; a fresh request prepares that actual latest view.
    engine.request(1, &[1]);
    while engine.step() {}
    assert!(engine.frame(1).is_some());
}


#[test]
fn cancellation_at_the_actual_final_state_lock_refuses_both_heads() {
    use std::sync::Barrier;
    let engine = opened();
    let transaction = staged(&engine);
    let before = engine.generation().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let barriers = Arc::new([Barrier::new(2), Barrier::new(2)]);
    let worker = engine.clone();
    let wait = barriers.clone();
    let pointer = &engine.inner.state as *const _ as usize;
    let thread = std::thread::spawn(move || crate::allocation::with_pause(pointer, 3, wait, || {
        let result = worker.commit_record_with(1, 7, transaction, move || { counter.fetch_add(1, Ordering::SeqCst); });
        assert!(result.is_err(), "the final cancel guard refuses before Engine/Editor head exchange");
    }));
    barriers[0].wait();
    engine.discard_batch(7);
    barriers[1].wait();
    thread.join().unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(Arc::ptr_eq(&before, &engine.generation().unwrap()));
}
