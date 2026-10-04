use std::path::{Path, PathBuf};
use std::sync::Arc;
use styk_doc::{Value, json};
use styk_edit::{EditError, HistoryLimits, session::{Session, RevisionCounters, WorkerSession}};

fn reference(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../tests/session-live/{name}.json"));
    json::parse(&std::fs::read(path).unwrap()).unwrap()
}
fn worker(reference: &Value) -> WorkerSession {
    let initial = reference.get("steps").unwrap().item(0).unwrap();
    let report = json::parse(initial.get("document").unwrap().as_str().unwrap().as_bytes()).unwrap();
    worker_from(reference, report)
}
fn worker_from(reference: &Value, report: Value) -> WorkerSession {
    let initial = reference.get("steps").unwrap().item(0).unwrap();
    let mut state = initial.get("state").unwrap().as_obj().unwrap().clone();
    for (key, value) in reference.get("options").unwrap().as_obj().unwrap().iter() { state.set(key, value.clone()); }
    for key in ["activeLayerId", "sheetSelection"] {
        if let Some(value) = initial.get("selection").unwrap().get(key) { state.set(key, value.clone()); }
    }
    let history = initial.get("history").unwrap();
    let counter = |key| history.get(key).unwrap().as_f64().unwrap() as u64;
    let mut loader = styk_eval::load::Loader::new();
    let document = loader.document(&report, "").unwrap();
    let prepared = Arc::new(styk_eval::eval::Prepared::new(Arc::new(document)));
    let mut session = Session::start_worker_prepared(report, &json::to_string(&Value::object(state)), loader, prepared,
        RevisionCounters { revision: counter("revision"), serial: counter("revisionSerial"), saved: counter("savedRevision") }).unwrap();
    let limits = reference.get("historyLimits").unwrap();
    session.prepare_worker_history(HistoryLimits { count: limits.get("count").unwrap().as_f64().unwrap() as usize,
        bytes: limits.get("bytes").unwrap().as_f64().unwrap() }, 0.0, || Ok(())).unwrap();
    session.into_worker()
}
fn history_entries(actual: &Value, expected: &Value, name: &str) {
    assert_eq!(actual.array_len(), expected.array_len(), "{name}");
    for (a, e) in actual.elements().unwrap().iter().zip(expected.elements().unwrap().iter()) {
        // The existing owning history view exports weights/labels, while the
        // recording also observes the private JS command kind. Do not fabricate
        // kinds in a second serializer; compare every field this owner exports.
        for (key, value) in a.as_obj().unwrap().iter() {
            if &**key == "children" { history_entries(value, e.get(key).unwrap(), name); }
            else { assert_eq!(Some(value), e.get(key), "{name}: history {key}"); }
        }
    }
}

#[test]
fn actual_editor_live_steps_keep_working_display_history_and_runtime_exact() {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/session-live");
    let mut paths: Vec<_> = std::fs::read_dir(directory).unwrap().map(|e|e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e|e=="json") && !p.file_name().unwrap().to_str().unwrap().contains("preflight")).collect();
    paths.sort(); assert_eq!(paths.len(), 18);
    let mut calls = 0;
    for path in paths {
        let reference = json::parse(&std::fs::read(&path).unwrap()).unwrap();
        let name = path.file_name().unwrap().to_str().unwrap();
        let returns_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../tests/session-live-return/{name}"));
        let returns = json::parse(&std::fs::read(returns_path).unwrap()).unwrap();
        let mut worker = worker(&reference);
        let steps = reference.get("steps").unwrap();
        if name.starts_with("native-channels") {
            // This actual option combination is explicitly outside the owning
            // native channel port. Retain its recorded evidence and atomic
            // refusal instead of changing options to make a replay pass.
            let before = worker.report().clone();
            let prepared = worker.prepare_live("select", &Value::array(vec![Value::Num(1.0),Value::Num(-1.0)]), &Value::Null, ||Ok(())).unwrap();
            assert_eq!(prepared.result().get("status").and_then(Value::as_str), Some("unported"));
            assert!(prepared.report().same_node(&before));
            assert!(worker.report().same_node(&before));
            assert_eq!(worker.operation_epoch(), 0);
            continue;
        }
        let first = steps.item(0).unwrap();
        let mut expected_document = first.get("document").unwrap().as_str().unwrap().to_owned();
        let mut expected_frame = first.get("frame").unwrap().as_str().unwrap().to_owned();
        for (index, step) in steps.elements().unwrap().iter().enumerate().skip(1) {
            calls += 1;
            let call = step.get("call").unwrap().elements().unwrap();
            let method = call.get(0).unwrap(); let method = method.as_str().unwrap();
            let args = Value::array(call.iter().skip(1).collect());
            let before = worker.report().clone();
            let epoch = worker.operation_epoch();
            let prepared = worker.prepare_live(method, &args, &Value::Null, || Ok(())).unwrap();
            assert!(worker.report().same_node(&before), "{name} step {index}: preparation published");
            assert_eq!(worker.operation_epoch(), epoch);
            if let Some(document) = step.get("document") { expected_document = document.as_str().unwrap().to_owned(); }
            if let Some(frame) = step.get("frame") { expected_frame = frame.as_str().unwrap().to_owned(); }
            let label = format!("{name} step {index} {method}");
            assert_eq!(json::to_string(prepared.report()), expected_document, "{label}: working document");
            let display = prepared.display().unwrap();
            assert_eq!(json::to_string(&display.record), expected_frame, "{label}: actual display");
            assert_eq!(display.frame.objects.len(), display.scene.scene().objects.len(), "{label}: typed scene");
            let answer = prepared.result();
            assert_eq!(answer.get("state"), step.get("state"), "{label}: state");
            if step.get("error").unwrap().is_null() {
                assert_eq!(answer.get("status").and_then(Value::as_str), Some("ok"), "{label}: {answer:?}");
                let expected_return = returns.get("steps").unwrap().item(index - 1).unwrap();
                assert_eq!(expected_return.get("call"), step.get("call"));
                assert_eq!(expected_return.get("resultDefined"), Some(&Value::Bool(true)), "{label}: recorded undefined result needs its explicit contract");
                assert_eq!(json::to_string(answer.get("result").unwrap()), expected_return.get("resultJSON").unwrap().as_str().unwrap(), "{label}: complete return view");
            } else {
                assert_eq!(answer.get("status").and_then(Value::as_str), Some("error"), "{label}");
                assert_eq!(answer.get("error"), step.get("error").unwrap().get("key"), "{label}: error key");
            }
            assert_eq!(answer.get("events"), step.get("events"), "{label}: events");
            let history = prepared.publication().history(); let expected = step.get("history").unwrap();
            for key in ["size", "revision", "revisionSerial", "savedRevision"] { assert_eq!(history.get(key), expected.get(key), "{label}: {key}"); }
            for key in ["undo", "redo"] { history_entries(history.get(key).unwrap(), expected.get(key).unwrap(), &label); }
            for (key, value) in prepared.runtime().as_obj().unwrap().iter() { assert_eq!(Some(value), step.get("live").unwrap().get(key), "{label}: live {key}"); }
            let (guard, display, runtime) = match worker.guard(prepared, || Ok(())) { Ok(value)=>value, Err((error,_))=>panic!("{label}: {error:?}") };
            let retired = guard.commit();
            assert_eq!(worker.operation_epoch(), epoch + 1);
            assert_eq!(json::to_string(worker.report()), expected_document);
            drop((retired, display, runtime)); // sole worker retirement owner
        }
    }
    assert_eq!(calls, 115);
}

#[test]
fn capability_rejects_other_owner_old_epoch_and_cancel_without_mutating_live_baseline() {
    let trace = reference("native-drag-commit");
    let mut worker = worker(&trace);
    let mut other = worker_from(&trace, worker.report().clone());
    let foreign = other.prepare_live("moveSelection", &Value::array(vec![Value::Num(4.0), Value::Num(2.0)]), &Value::Null, || Ok(())).unwrap();
    assert!(matches!(worker.guard(foreign, || Ok(())), Err((EditError::Invalid(_), _))));
    // A real repeated selection can leave every report/counter/canonical view
    // equal. Its actual operation epoch must still reject an older candidate.
    let selection = Value::array(vec![Value::Num(1.0), Value::Num(-1.0)]);
    let stale_same_head = worker.prepare_live("select", &selection, &Value::Null, || Ok(())).unwrap();
    let repeated = worker.prepare_live("select", &selection, &Value::Null, || Ok(())).unwrap();
    let (guard, display, runtime) = worker.guard(repeated, || Ok(())).unwrap_or_else(|(error,_)|panic!("{error:?}"));
    drop((guard.commit(), display, runtime));
    assert!(matches!(worker.guard(stale_same_head, || Ok(())), Err((EditError::Invalid(_), _))));
    let begin = worker.prepare_live("beginDrag", &Value::array(vec![Value::str("Worker gesture")]), &Value::Null, || Ok(())).unwrap();
    let (guard, display, runtime) = worker.guard(begin, || Ok(())).unwrap_or_else(|(error,_)|panic!("{error:?}"));
    drop((guard.commit(), display, runtime));
    let stale = worker.prepare_live("moveSelection", &Value::array(vec![Value::Num(4.0), Value::Num(2.0)]), &Value::Null, || Ok(())).unwrap();
    let before = worker.report().clone(); let epoch = worker.operation_epoch();
    let mut checks = 0;
    let cancelled = worker.prepare_live("moveSelection", &Value::array(vec![Value::Num(9.0), Value::Num(5.0)]), &Value::Null, || {
        checks += 1; if checks == 4 { Err(EditError::Invalid("cancelled".into())) } else { Ok(()) }
    });
    assert!(matches!(cancelled, Err(EditError::Invalid(ref reason)) if reason=="cancelled"));
    assert!(worker.report().same_node(&before)); assert_eq!(worker.operation_epoch(), epoch);
    let moved = worker.prepare_live("moveSelection", &Value::array(vec![Value::Num(2.0), Value::Num(1.0)]), &Value::Null, || Ok(())).unwrap();
    let (guard, display, runtime) = worker.guard(moved, || Ok(())).unwrap_or_else(|(error,_)|panic!("{error:?}"));
    drop((guard.commit(), display, runtime));
    assert!(matches!(worker.guard(stale, || Ok(())), Err((EditError::Invalid(_), _))));
    assert!(worker.prepare_quiet("setFPS", &Value::array(vec![Value::Num(30.0)]), &Value::Null, || Ok(())).is_err(), "quiet Guard was widened");
    let cancelled = worker.prepare_live("cancelDrag", &Value::array(vec![]), &Value::Null, || Ok(())).unwrap();
    assert_eq!(json::to_string(cancelled.report()), json::to_string(&before), "cancel must use the original live baseline; COW roots may differ after restoring its poses");
}
