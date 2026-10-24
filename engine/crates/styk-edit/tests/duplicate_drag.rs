//! Alt-drag duplication in the session (src/editor-duplicate-drag.mjs): the copies and their move are
//! one history entry when the gesture commits, and no entry at all when it is cancelled.

use styk_doc::Value;
use styk_edit::session::Session;

fn json(text: &str) -> Value { styk_doc::json::parse(text.as_bytes()).unwrap() }

fn session(name: &str) -> Session {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../tests/edit/{name}.reference.json"));
    let reference = styk_doc::json::parse(&std::fs::read(path).unwrap()).unwrap();
    let first = reference.get("steps").unwrap().item(0).unwrap();
    let mut state = first.get("state").unwrap().as_obj().unwrap().clone();
    for (key, value) in reference.get("editor").unwrap().as_obj().unwrap().iter() { state.set(key, value.clone()); }
    Session::start(json(first.get("document").unwrap().as_str().unwrap()), &styk_doc::json::to_string(&Value::object(state))).unwrap()
}

fn call(session: &mut Session, method: &str, args: &str) -> Value {
    let answer = json(&session.call(method, args, "null"));
    assert_eq!(answer.get("status").and_then(Value::as_str), Some("ok"), "{method} {args}: {answer:?}");
    answer
}

/// How many figures the first key of the first figure layer shows: its pose source, else the stored keyframe.
fn figures(session: &Session) -> usize {
    let report = session.report();
    let key = report.path(&["_styk", "layerSheet", "tracks", "layer-1", "keys"]).and_then(Value::elements).and_then(|keys| keys.get(0)).expect("a key");
    let frame = match key.path(&["value", "frameId"]).and_then(Value::as_str) {
        Some(id) => report.path(&["_styk", "frameSources", id]).cloned().expect("its pose source"),
        None => report.get("keyframes").and_then(Value::elements).and_then(|frames| frames.get(0)).expect("a keyframe"),
    };
    frame.get("objects").and_then(Value::array_len).unwrap()
}

fn state(answer: &Value, key: &str) -> Value { answer.get("state").unwrap().get(key).unwrap().clone() }

#[test]
fn the_copies_and_their_move_are_one_entry_that_undo_takes_away() {
    let mut session = session("rig-drag-sheet");
    let before = figures(&session);
    assert_eq!(call(&mut session, "beginDuplicateDrag", "[]").get("result"), Some(&Value::Bool(true)));
    assert_eq!(figures(&session), before + 1, "the copy is in the document while the gesture lasts");
    assert!(!session.can_undo(), "nothing is in history yet");
    // The copy is what moves.
    let drag = call(&mut session, "moveSelection", "[40,20]");
    assert_eq!(drag.get("drag").and_then(|d| d.get("before")).and_then(Value::array_len), Some(1));
    assert_eq!(call(&mut session, "commitDrag", "[]").get("result"), Some(&Value::Bool(true)));
    assert_eq!(figures(&session), before + 1);
    let committed = call(&mut session, "seek", "[0]");
    assert_eq!(state(&committed, "undoLabel").as_str(), Some("Duplicate figures"));
    assert!(session.can_undo());
    // One undo takes the copy away together with its move.
    let undone = call(&mut session, "undo", "[]");
    assert_eq!(figures(&session), before);
    assert!(!session.can_undo(), "the copy and the move were one entry");
    assert_eq!(state(&undone, "canRedo").as_bool(), Some(true));
    call(&mut session, "redo", "[]");
    assert_eq!(figures(&session), before + 1);
    assert!(session.can_undo());
}

#[test]
fn cancelling_takes_the_copies_away_and_records_nothing() {
    let mut session = session("rig-drag-sheet");
    let before = figures(&session);
    let report = styk_doc::json::to_string(session.report());
    assert_eq!(call(&mut session, "beginDuplicateDrag", "[]").get("result"), Some(&Value::Bool(true)));
    call(&mut session, "moveSelection", "[40,20]");
    assert_eq!(call(&mut session, "cancelDrag", "[]").get("result"), Some(&Value::Bool(true)));
    assert_eq!(figures(&session), before);
    assert!(!session.can_undo() && !session.can_redo(), "a cancelled duplicate leaves history alone");
    assert_eq!(call(&mut session, "seek", "[0]").get("drag"), Some(&Value::Null), "the gesture ended");
    // The figures are as they were (the sheet may count the pose sources it dropped in its own bookkeeping).
    let after = styk_doc::json::to_string(session.report());
    assert_eq!(report.matches("\"unique_id\"").count(), after.matches("\"unique_id\"").count());
}

#[test]
fn nothing_selected_duplicates_nothing() {
    let mut session = session("rig-drag-sheet");
    call(&mut session, "select", "[null]");
    assert_eq!(call(&mut session, "beginDuplicateDrag", "[]").get("result"), Some(&Value::Bool(false)));
    assert!(!session.can_undo());
}
