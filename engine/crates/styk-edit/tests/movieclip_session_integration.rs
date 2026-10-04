//! Combined canonical-publication admission: legacy UI calls never reach dense
//! movie command preflight; native tools and worker preparation keep one owner.
use std::path::Path;
use styk_doc::{Value, json};
use styk_edit::{HistoryLimits, session::Session};
fn fixture(folder: &str, name: &str) -> Value {
    json::parse(&std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests").join(folder).join(format!("{name}.reference.json"))).unwrap()).unwrap()
}
fn start(reference: &Value) -> Session {
    let first = reference.get("steps").unwrap().item(0).unwrap();
    Session::start(json::parse(first.get("document").unwrap().as_str().unwrap().as_bytes()).unwrap(), &json::to_string(first.get("state").unwrap())).unwrap()
}
fn call(session: &mut Session, method: &str, args: &str) -> Value {
    session.call_values(method, &json::parse(args.as_bytes()).unwrap(), &Value::Null)
}
#[test]
fn legacy_native_movie_calls_refuse_before_dense_ik_scan_and_preserve_live_gestures() {
    let methods = ["createEmptyMovieclip", "createMovieclip", "makeMovieclipUnique", "placeMovieclip", "splitMovieclip", "breakApartMovieclip", "renameMovieclip", "restartMovieclip", "removeMovieclipRestart", "setMovieclipPlayback", "setMovieclipDrawing", "addMovieclipToNextKeyframes", "bakeMovieclip"];
    for name in ["native-ik-shown", "native-ik-future", "native-ik-source", "native-ik-asset"] {
        let reference = fixture("movieclip-admission", name);
        for method in methods {
            let mut session = start(&reference);
            let before = json::to_string(session.report());
            let refused = call(&mut session, method, "[]");
            assert_eq!(refused.get("status").and_then(Value::as_str), Some("unported"), "{name} {method}");
            assert_eq!(refused.get("reason").and_then(Value::as_str), Some("native movieclip editing requires worker preparation"), "{name} {method}: must refuse before IK dispatch");
            assert_eq!(json::to_string(session.report()), before, "{name} {method}");
            assert_eq!(refused.get("events").unwrap().array_len(), Some(0));
        }
    }
    let reference = fixture("movieclip-insertion", "insert-native");
    for method in methods.into_iter().chain(["addFigure"]) {
        let mut guarded = start(&reference);
        let mut control = start(&reference);
        for (method, args) in [("select", "[1,-1]"), ("beginTranslationDrag", "[\"Guarded movie\"]"), ("moveSelection", "[4.5,-2.25]")] {
            assert_eq!(call(&mut guarded, method, args), call(&mut control, method, args));
        }
        let before = json::to_string(guarded.report());
        let state = call(&mut control, "select", "[1,-1]").get("state").unwrap().clone();
        let refused = call(&mut guarded, method, "[4]");
        assert_eq!(refused.get("status").and_then(Value::as_str), Some("unported"), "{method}");
        assert_eq!(refused.get("state"), Some(&state), "{method} state");
        assert_eq!(refused.get("events").unwrap().array_len(), Some(0), "{method} events");
        assert_eq!(json::to_string(guarded.report()), before, "{method} report");
        for (method, args) in [("moveSelection", "[2,1]"), ("cancelDrag", "[]")] {
            assert_eq!(call(&mut guarded, method, args), call(&mut control, method, args));
        }
    }
}
#[test]
fn ordinary_native_add_keeps_explicit_cold_refusal_without_definition_classification() {
    let mut session = start(&fixture("movieclip-insertion", "insert-native"));
    for args in ["[1]", "[4]", "[999]", "[]"] {
        let before = json::to_string(session.report());
        let result = call(&mut session, "addFigure", args);
        assert_eq!(result.get("status").and_then(Value::as_str), Some("unported"));
        assert_eq!(result.get("reason").and_then(Value::as_str), Some("native movieclip insertion requires worker preparation"));
        assert_eq!(json::to_string(session.report()), before);
        assert_eq!(result.get("events").unwrap().array_len(), Some(0));
    }
}
#[test]
fn worker_movie_insertion_matches_recorded_editor_and_stays_immutable_until_guarded_commit() {
    let reference = fixture("movieclip-insertion", "insert-native");
    let mut session = start(&reference);
    call(&mut session, "select", "[1,-1]");
    session.prepare_worker_history(HistoryLimits::default(), 0.0, || Ok(())).unwrap();
    let before = json::to_string(session.report());
    let prepared = session.prepare_worker_call_values("addFigure", &json::parse(b"[4]").unwrap(), &Value::Null, || Ok(())).unwrap();
    assert_eq!(prepared.result().get("status").and_then(Value::as_str), Some("ok"));
    let expected = reference.get("steps").unwrap().item(2).unwrap();
    assert_eq!(json::to_string(prepared.report()), expected.get("document").unwrap().as_str().unwrap());
    assert_eq!(json::to_string(session.report()), before);
    drop(prepared); // cancelled worker preparation never publishes its sources.
    assert_eq!(json::to_string(session.report()), before);
    let prepared = session.prepare_worker_call_values("addFigure", &json::parse(b"[4]").unwrap(), &Value::Null, || Ok(())).unwrap();
    let after = json::to_string(prepared.report());
    let guarded = match session.guard_worker_publication_checked(prepared, || Ok(())) { Ok(guarded) => guarded, Err((error, _)) => panic!("{error:?}") };
    let retired = guarded.commit();
    assert_eq!(json::to_string(session.report()), after);
    drop(retired); // displaced heads are retired on this worker, after publication.
}
