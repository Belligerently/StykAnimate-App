use std::sync::Arc;
use styk_doc::Value;
fn json(text: &str) -> Value { styk_doc::json::parse(text.as_bytes()).unwrap() }
fn session() -> (styk_edit::session::Session, Arc<styk_eval::eval::Prepared>) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/edit/camera-follow-handles-sheet.reference.json");
    let reference = styk_doc::json::parse(&std::fs::read(path).unwrap()).unwrap();
    let first = reference.get("steps").unwrap().item(0).unwrap();
    let report = json(first.get("document").unwrap().as_str().unwrap());
    let mut loader = styk_eval::load::Loader::new();
    let doc = loader.document(&report, "readiness").unwrap();
    let prepared = Arc::new(styk_eval::eval::Prepared::new(Arc::new(doc)));
    let state = "{\"tick\":6,\"frameIndex\":1,\"selectedUid\":1,\"selectedUids\":[1],\"selectedJoint\":-1,\"layerTimeline\":true,\"propertyChannels\":true}";
    (styk_edit::session::Session::start_prepared(report, state, loader, prepared.clone()).unwrap(), prepared)
}
#[test]
fn mirror_session_refuses_cold_follow_and_reads_worker_warmed_path() {
    let (mut session, prepared) = session();
    assert!(prepared.travel_ready(None, 6.0).is_none());
    let answer = json(&session.call("nudgeHandle", "[1,2]", "null"));
    assert_eq!(answer.get("status").and_then(Value::as_str), Some("unported"));
    assert!(prepared.travel_ready(None, 6.0).is_none(), "the refused UI call warmed a path");
    assert!(!session.can_undo());
    let worker = prepared.clone();
    std::thread::spawn(move || worker.travel(None, 6.0)).join().unwrap();
    let answer = json(&session.call("nudgeHandle", "[1,2]", "null"));
    assert_eq!(answer.get("status").and_then(Value::as_str), Some("ok"), "{answer:?}");
    assert!(session.can_undo());
    let answer = json(&session.call("nudgeHandle", "[1,2]", "null"));
    assert_eq!(answer.get("status").and_then(Value::as_str), Some("unported"), "a local publication must use worker views for its new revision");
}
#[test]
fn committing_a_gesture_before_cold_follow_refusal_restores_the_gesture() {
    let (mut session, prepared) = session();
    let worker = prepared.clone();
    std::thread::spawn(move || worker.travel(None, 6.0)).join().unwrap();
    assert_eq!(json(&session.call("beginDrag", "[]", "null")).get("status").and_then(Value::as_str), Some("ok"));
    let before = json(&session.call("moveSelection", "[5,3]", "null"));
    assert_eq!(before.get("status").and_then(Value::as_str), Some("ok"));
    let refused = json(&session.call("applyCameraToFigures", "[]", "null"));
    assert_eq!(refused.get("status").and_then(Value::as_str), Some("unported"), "{refused:?}");
    assert_eq!(refused.get("draft"), before.get("draft"));
    assert!(!session.can_undo(), "the refused call committed the gesture");
    let cancelled = json(&session.call("cancelDrag", "[]", "null"));
    assert_eq!(cancelled.get("result"), Some(&Value::Bool(true)), "the original gesture was lost");
}
