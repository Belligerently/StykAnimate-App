use super::*;

// Behavioral worker-owner regressions, using the actual quiet/live protocol.
fn audit_ready_raw(core: &Arc<Core>) -> (usize, usize) {
    let worker = core.worker.lock().unwrap();
    let ready = worker.ready.as_ref().expect("actual durable-worker current ready");
    let bytes = styk_doc::retained_bytes_checked(&[&ready.record], |_| Ok::<_, ()>(())).unwrap();
    (bytes, Arc::strong_count(&ready.frame))
}
#[test]
fn audit_persistent_raw_charge_expectation_fails_before_any_accounting_fix() {
    let (engine,handle,stamp,epochs)=native(false);let core=handle.core.upgrade().unwrap();
    let request=handle.submit_live(stamp,epochs,"select","[1,-1]").unwrap();
    staged(&engine,&handle,request);commit(&engine,&handle,request,epochs);
    handle.terminal(request).unwrap();drain(&engine);
    let raw=audit_ready_raw(&core).0;let charge=engine.inner.record_budget.retained_bytes();
    assert!(raw>0);
    assert!(charge>=raw,"confirmed current ready raw storage remains live after ACK: raw={raw}, charged={charge}");
}

fn accepted(engine: &Engine, handle: &Handle, stamp: Stamp, epochs: Epochs,
    method: &str, args: &str, live: bool) -> Stamp {
    let request = if live { handle.submit_live(stamp, epochs, method, args) }
        else { handle.submit(stamp, epochs, method, args, "null") }.unwrap();
    staged(engine, handle, request);
    let next = commit(engine, handle, request, epochs).stamp;
    handle.terminal(request).unwrap();drain(engine);
    assert_current_raw_charge(engine);
    next
}
fn detach_cleanup(engine: &Engine) -> std::collections::LinkedList<Box<dyn FnOnce(&Engine) + Send>> {
    crate::detach_task(&mut engine.inner.state.lock().retained_tasks).expect("actual retained retirement node")
}
fn run_cleanup(engine: &Engine, mut cleanup: std::collections::LinkedList<Box<dyn FnOnce(&Engine) + Send>>) {
    let worker_engine = engine.clone();
    std::thread::spawn(move || cleanup.pop_front().unwrap()(&worker_engine)).join().unwrap();
}

#[test]
fn current_raw_charge_survives_quiet_live_history_branch_cancel_and_worker_detach() {
    let (engine, handle, mut stamp, epochs) = native(false);
    let core = handle.core.upgrade().unwrap();
    for (method, args, live) in [
        ("select", "[1,-1]", true), ("setFPS", "[27]", false),
        ("undo", "[]", false), ("redo", "[]", false),
        ("undo", "[]", false), ("setFPS", "[29]", false),
    ] {
        stamp = accepted(&engine, &handle, stamp, epochs, method, args, live);
        assert!(engine.inner.record_budget.retained_bytes() >= audit_ready_raw(&core).0);
        assert!(core.worker.lock().unwrap().ready_charge.is_some());
    }
    for after in [false, true] {
        let before = owner_report(&engine);
        let request = handle.submit(stamp, epochs, "setFPS", "[37]", "null").unwrap();
        staged(&engine, &handle, request);
        if after { stamp = commit(&engine, &handle, request, epochs).stamp; }
        handle.cancel(request);drain(&engine);
        if after {
            assert_eq!(handle.poll().phase, Phase::Success);
            assert!(core.worker.lock().unwrap().pending.as_ref().unwrap().ready_retired.is_some());
        } else {
            assert_eq!(handle.poll().phase, Phase::Failed);
            assert!(owner_report(&engine).same_node(&before));
            assert_current_raw_charge(&engine);
        }
        handle.terminal(request).unwrap();drain(&engine);assert_current_raw_charge(&engine);
    }
    let (_, moved, moved_epochs) = engine.detach_session_worker().unwrap();
    assert_eq!((moved, moved_epochs), (stamp, epochs));
    assert!(core.worker.lock().unwrap().ready.is_none());
    assert!(core.worker.lock().unwrap().ready_charge.is_none());
    assert_eq!(engine.inner.record_budget.retained_bytes(), 0);
}

#[test]
fn detached_batch_keeps_full_union_blocks_admission_and_automatically_rescopes_idle_core() {
    let (engine, handle, stamp, epochs) = native(true);
    let core = handle.core.upgrade().unwrap();
    let stamp = accepted(&engine, &handle, stamp, epochs, "select", "[1,-1]", true);
    let current = engine.inner.record_budget.retained_bytes();
    let before = owner_report(&engine);
    let request = handle.submit(stamp, epochs, "setFPS", "[37]", "null").unwrap();
    staged(&engine, &handle, request);
    let batch = core.worker.lock().unwrap().pending.as_ref().unwrap().batch;
    let full = engine.inner.record_budget.retained_bytes();assert!(full > current);
    engine.discard_batch(batch);
    let cleanup = detach_cleanup(&engine); // exact detached closure held behind a physical-disposal barrier
    handle.cancel(request);drain(&engine);handle.terminal(request).unwrap();drain(&engine);
    assert_eq!(handle.poll().phase, Phase::Idle);
    assert_eq!(core.request.load(Ordering::Acquire), 0);
    assert_eq!(engine.inner.record_budget.retained_bytes(), full, "no early shrink while detached raw aliases live");
    assert!(!service_current(&engine), "no poll/self-wake while uniqueness fails");
    let blocked = handle.submit(stamp, epochs, "setFPS", "[43]", "null").unwrap();
    drain(&engine);assert_eq!(handle.poll().error, Some(Error::Busy));
    assert!(owner_report(&engine).same_node(&before));
    assert_eq!(engine.inner.record_budget.retained_bytes(), full);
    handle.terminal(blocked).unwrap();drain(&engine);
    assert!(!service_current(&engine));
    run_cleanup(&engine, cleanup); // drop FIRST, original weak Core callback SECOND
    assert!(core.wake_pending.load(Ordering::Acquire));
    assert!(service_current(&engine), "automatic worker rescope with request == 0, without a UI control");
    assert_eq!(engine.inner.record_budget.retained_bytes(), current);
    assert_current_raw_charge(&engine);
    assert!(!service_current(&engine));
    accepted(&engine, &handle, stamp, epochs, "setFPS", "[43]", false);
}

#[test]
fn retirement_callback_before_pending_alias_disposal_is_not_a_lost_wake() {
    let (engine, handle, stamp, epochs) = empty();
    let stamp = accepted(&engine, &handle, stamp, epochs, "setFPS", "[29]", false);
    let core = handle.core.upgrade().unwrap();let current = engine.inner.record_budget.retained_bytes();
    let request = handle.submit(stamp, epochs, "setFPS", "[31]", "null").unwrap();
    staged(&engine, &handle, request);
    let batch = core.worker.lock().unwrap().pending.as_ref().unwrap().batch;
    engine.discard_batch(batch);let cleanup = detach_cleanup(&engine);
    run_cleanup(&engine, cleanup);
    assert!(service_current(&engine));
    assert!(engine.inner.record_budget.retained_bytes() > current, "Pending alias still owns candidate/response raw values");
    handle.cancel(request);drain(&engine);
    assert_current_raw_charge(&engine); // failure transition itself retries after actual final alias drop
    handle.terminal(request).unwrap();drain(&engine);assert_current_raw_charge(&engine);
}

#[test]
fn no_ready_first_failure_releases_its_allowance_only_after_detached_batch_disposal() {
    let (engine, handle, stamp, epochs) = empty();let core = handle.core.upgrade().unwrap();
    let request = handle.submit(stamp, epochs, "setFPS", "[29]", "null").unwrap();
    staged(&engine, &handle, request);
    let batch = core.worker.lock().unwrap().pending.as_ref().unwrap().batch;
    engine.discard_batch(batch);let cleanup = detach_cleanup(&engine);
    handle.cancel(request);drain(&engine);handle.terminal(request).unwrap();drain(&engine);
    assert!(core.worker.lock().unwrap().ready.is_none());
    assert!(engine.inner.record_budget.retained_bytes() > 256);
    assert!(!service_current(&engine));
    run_cleanup(&engine, cleanup);assert!(service_current(&engine));
    assert_eq!(engine.inner.record_budget.retained_bytes(), 0);
    assert!(core.worker.lock().unwrap().ready_charge.is_none());
}

#[test]
fn legacy_collision_generation_retirement_and_stop_use_the_same_drop_then_wake_owner() {
    for cause in ["legacy", "generation", "stop"] {
        let (engine, handle, stamp, epochs) = empty();let core = handle.core.upgrade().unwrap();
        let stamp = accepted(&engine, &handle, stamp, epochs, "setFPS", "[29]", false);
        let request = handle.submit(stamp, epochs, "setFPS", "[31]", "null").unwrap();
        staged(&engine, &handle, request);
        let batch = core.worker.lock().unwrap().pending.as_ref().unwrap().batch;
        match cause {
            "legacy" => { assert!(engine.stage(batch, Change::Reset).is_err()); }
            "generation" => engine.retire_record_batches(stamp.generation),
            "stop" => engine.stop_record_batches(),
            _ => unreachable!(),
        }
        let cleanup = detach_cleanup(&engine);
        if cause == "stop" { drain(&engine);assert!(engine.inner.session.lock().is_none()); }
        else { handle.cancel(request);drain(&engine);handle.terminal(request).unwrap();drain(&engine); }
        let full = engine.inner.record_budget.retained_bytes();assert!(full > 0);
        run_cleanup(&engine, cleanup);
        if cause == "stop" { assert_eq!(engine.inner.record_budget.retained_bytes(), 0); }
        else { assert!(service_current(&engine), "{cause}: original Core automatic wake");assert_current_raw_charge(&engine); }
    }
}

#[test]
fn late_original_retirement_never_wakes_replacement_core_or_reuses_its_allowance() {
    let (engine, old, stamp, epochs) = empty();let original = old.core.upgrade().unwrap();
    let request = old.submit(stamp, epochs, "setFPS", "[29]", "null").unwrap();staged(&engine, &old, request);
    let batch = original.worker.lock().unwrap().pending.as_ref().unwrap().batch;
    engine.discard_batch(batch);let cleanup = detach_cleanup(&engine);
    old.close();drain(&engine);assert!(engine.inner.session.lock().is_none());
    assert!(engine.inner.record_budget.retained_bytes() > 0);
    let (_, tree, loader, prepared) = engine.mirrored_for_editing().unwrap();
    let session = Session::start_prepared(tree, "{}", loader, prepared).unwrap();
    let (new, _) = engine.attach_session_worker(session, HistoryLimits::default(), 0.0, epochs).unwrap_or_else(|(error, _)| panic!("{error}"));
    let replacement = new.core.upgrade().unwrap();assert!(!replacement.wake_pending.load(Ordering::Acquire));
    run_cleanup(&engine, cleanup);
    assert!(original.wake_pending.load(Ordering::Acquire));
    assert!(!replacement.wake_pending.load(Ordering::Acquire));
    assert_eq!(engine.inner.record_budget.retained_bytes(), 0);
    assert!(!service_current(&engine));
}

#[test]
fn actual_candidate_and_commit_marker_refusal_rescope_only_after_worker_raw_disposal() {
    let (engine, handle, stamp, epochs) = empty();
    let stamp = accepted(&engine, &handle, stamp, epochs, "setFPS", "[29]", false);
    let before = owner_report(&engine);let generation = engine.generation().unwrap();
    let core = handle.core.upgrade().unwrap();let current = engine.inner.record_budget.retained_bytes();
    let request = handle.submit(stamp, epochs, "setFPS", "[31]", "null").unwrap();staged(&engine, &handle, request);
    handle.ready(request, epochs).unwrap();
    let pointer = &engine.inner.state as *const _ as usize;
    let barriers = Arc::new([std::sync::Barrier::new(2), std::sync::Barrier::new(2)]);
    let pause = barriers.clone();let worker_core = core.clone();let worker_engine = engine.clone();
    let worker = std::thread::spawn(move || crate::allocation::with_pause(pointer, 7, pause,
        || worker_core.advance(&worker_engine)));
    barriers[0].wait();
    assert!(engine.inner.record_budget.retained_bytes() > current, "actual prebuilt candidate/old ready still retained");
    handle.cancel(request);barriers[1].wait();worker.join().unwrap();drain(&engine);
    assert_eq!(handle.poll().phase, Phase::Failed);
    assert!(owner_report(&engine).same_node(&before));assert!(Arc::ptr_eq(&generation, &engine.generation().unwrap()));
    assert!(engine.inner.staged.lock().is_empty(), "same CommitMarker disposed exact placeholder");
    assert_current_raw_charge(&engine);assert_eq!(engine.inner.record_budget.retained_bytes(), current);
    handle.terminal(request).unwrap();drain(&engine);assert_current_raw_charge(&engine);
}

#[test]
fn actual_charge_publication_retirement_and_rescope_allocate_or_drop_nothing_under_state() {
    for state in [true, false] {
        let (engine, handle, stamp, epochs) = native(true);let core = handle.core.upgrade().unwrap();
        let pointer = if state { &engine.inner.state as *const _ as usize } else { &engine.inner.staged as *const _ as usize };
        let (stamp, counts) = crate::allocation::watch(pointer, || {
            let stamp = accepted(&engine, &handle, stamp, epochs, "select", "[1,-1]", true);
            let stamp = accepted(&engine, &handle, stamp, epochs, "setFPS", "[29]", false);
            let request = handle.submit(stamp, epochs, "setFPS", "[31]", "null").unwrap();staged(&engine, &handle, request);
            let batch = core.worker.lock().unwrap().pending.as_ref().unwrap().batch;
            engine.discard_batch(batch);handle.cancel(request);drain(&engine);
            handle.terminal(request).unwrap();drain(&engine);assert_current_raw_charge(&engine);
            stamp
        });
        assert!(counts.locks > 0);assert_eq!((counts.allocations, counts.deallocations), (0, 0), "{counts:?}");
        let (_, counts) = crate::allocation::watch(pointer, || {
            let (_, returned, _) = engine.detach_session_worker().unwrap();assert_eq!(returned, stamp);
        });
        assert_eq!((counts.allocations, counts.deallocations), (0, 0), "detach: {counts:?}");
        assert_eq!(engine.inner.record_budget.retained_bytes(), 0);
    }
}

#[test]
fn exact_detached_raw_root_is_physically_dropped_before_the_original_core_wake() {
    let (engine, handle, stamp, epochs) = empty();let core = handle.core.upgrade().unwrap();
    let stamp = accepted(&engine, &handle, stamp, epochs, "setFPS", "[29]", false);
    let request = handle.submit(stamp, epochs, "setFPS", "[31]", "null").unwrap();staged(&engine, &handle, request);
    let (batch, after) = {
        let worker = core.worker.lock().unwrap();let pending = worker.pending.as_ref().unwrap();
        let Value::Obj(after) = pending.prepared.as_ref().unwrap().publication().report() else { panic!("actual prepared object root") };
        (pending.batch, Arc::downgrade(after))
    };
    engine.discard_batch(batch);let mut cleanup = detach_cleanup(&engine);
    handle.cancel(request);drain(&engine);handle.terminal(request).unwrap();drain(&engine);
    assert!(after.upgrade().is_some());
    assert!(!core.worker.lock().unwrap().ready_charge.as_mut().unwrap().unique());
    let pointer = Arc::as_ptr(&core) as usize;
    let barriers = Arc::new([std::sync::Barrier::new(2), std::sync::Barrier::new(2)]);
    let pause = barriers.clone();let worker_engine = engine.clone();let (sent, received) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        sent.send(std::thread::current().id()).unwrap();
        crate::allocation::with_pause(pointer, 1, pause, || cleanup.pop_front().unwrap()(&worker_engine));
    });
    assert_ne!(received.recv_timeout(std::time::Duration::from_secs(1)).unwrap(), std::thread::current().id());
    barriers[0].wait();
    let physically_dropped = after.upgrade().is_none();
    let unique = core.worker.lock().unwrap().ready_charge.as_mut().unwrap().unique();
    let not_yet_woken = !core.wake_pending.load(Ordering::Acquire);
    barriers[1].wait();worker.join().unwrap();
    assert!(physically_dropped && unique && not_yet_woken,
        "worker root drop must precede scalar wake: disposed={physically_dropped}, unique={unique}, not_woken={not_yet_woken}");
    assert!(service_current(&engine));assert_current_raw_charge(&engine);
}

#[test]
fn same_eleven_call_lifecycle_and_dense_worker_cost_diagnostic() {
    for dense in [false, true] {
        let (engine, handle, mut stamp, epochs) = native(dense);let core = handle.core.upgrade().unwrap();
        for (method, args, live) in [
            ("select", "[1,-1]", true), ("beginDrag", r#"["Charge audit"]"#, true),
            ("moveSelection", "[2,1]", true), ("commitDrag", "[]", true),
            ("undo", "[]", false), ("redo", "[]", false), ("undo", "[]", false),
            ("setFPS", "[31]", false), ("beginDrag", r#"["Cancelled drag audit"]"#, true),
            ("moveSelection", "[3,2]", true), ("cancelDrag", "[]", true),
        ] {
            let previous = core.worker.lock().unwrap().ready.as_ref().map(|ready| Arc::downgrade(&ready.frame));
            let request = if live { handle.submit_live(stamp, epochs, method, args) }
                else { handle.submit(stamp, epochs, method, args, "null") }.unwrap();
            let started = std::time::Instant::now();
            let (answer, _) = staged(&engine, &handle, request);
            let prepare_us = started.elapsed().as_micros();
            let actual = if live { answer.get("answer").unwrap() } else { &answer };
            assert_eq!(actual.get("status").and_then(Value::as_str), Some("ok"), "{method}");
            let full = engine.inner.record_budget.retained_bytes();
            stamp = commit(&engine, &handle, request, epochs).stamp;
            if let Some(previous) = previous.as_ref() { assert!(previous.upgrade().is_some()); }
            let started = std::time::Instant::now();handle.terminal(request).unwrap();drain(&engine);
            let terminal_us = started.elapsed().as_micros();
            assert_current_raw_charge(&engine);
            let (raw, strong) = audit_ready_raw(&core);
            let current = engine.inner.record_budget.retained_bytes();
            assert_eq!(current, raw + 256);assert!(current < full);
            if let Some(previous) = previous { assert!(previous.upgrade().is_none()); }
            eprintln!(r#"RAW_DIAGNOSTIC {{"dense":{dense},"method":"{method}","preparedCharge":{full},"currentRawBytes":{raw},"postAckCharge":{current},"frameStrong":{strong},"prepareUs":{prepare_us},"terminalUs":{terminal_us}}}"#);
        }
        handle.close();drain(&engine);assert_current_raw_charge(&engine);
        assert!(core.worker.lock().unwrap().ready.is_none());assert!(core.worker.lock().unwrap().ready_charge.is_none());
    }
}
