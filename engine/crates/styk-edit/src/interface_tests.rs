//! What the interface reads of the frame on show (`interface.rs`): the geometry of a recorded document
//! is complete and consistent, a gesture changes only the groups of the figures it moves, and the
//! frame and timeline are the ones the editor shows.
use std::path::Path;

use styk_doc::Value;

use crate::session::Session;

fn parse(text: &str) -> Value {
    styk_doc::json::parse(text.as_bytes()).unwrap()
}

fn json(value: &Value) -> String {
    styk_doc::json::to_string(value)
}

/// The session a recorded tape starts from, and the steps that follow its first.
fn tape(name: &str) -> (Session, Vec<Value>) {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../tests/edit/{name}.reference.json"));
    let reference = parse(&std::fs::read_to_string(file).unwrap());
    let steps: Vec<Value> = reference.get("steps").unwrap().elements().unwrap().iter().collect();
    let mut state = steps[0].get("state").unwrap().as_obj().unwrap().clone();
    for (key, value) in reference.get("editor").unwrap().as_obj().unwrap().iter() {
        state.set(key, value.clone());
    }
    let session = Session::start(parse(steps[0].get("document").unwrap().as_str().unwrap()), &json(&Value::object(state))).unwrap();
    (session, steps)
}

/// The view a call of `session` is made with.
fn view(session: &Session) -> String {
    let mut view = session.editor.state_value().as_obj().unwrap().clone();
    view.set("autoKey", Value::Bool(session.editor.auto_key));
    json(&Value::object(view))
}

fn run(session: &mut Session, step: &Value) {
    let call: Vec<Value> = step.get("call").unwrap().elements().unwrap().iter().collect();
    let method = call[0].as_str().unwrap();
    let args = json(&Value::array(call[1..].to_vec()));
    let answer = parse(&session.call(method.trim_start_matches('@'), &args, &view(session)));
    assert_ne!(answer.get("status").and_then(Value::as_str), Some("unported"), "{method}: {answer:?}");
}

fn geometry(session: &mut Session, since: Option<f64>) -> Value {
    let args = match since {
        Some(token) => format!("{{\"since\":{token}}}"),
        None => "{}".to_string(),
    };
    let view = view(session);
    parse(&session.query("geometry", &args, &view).unwrap())
}

fn slots_of(read: &Value) -> Vec<usize> {
    read.get("slots").unwrap().elements().unwrap().iter().map(|pair| pair.item(0).unwrap().as_f64().unwrap() as usize).collect()
}

fn group(read: &Value, slot: usize) -> Value {
    read.get("slots").unwrap().elements().unwrap().iter().find(|pair| pair.item(0).unwrap().as_f64().unwrap() as usize == slot).unwrap().item(1).unwrap()
}

fn point(value: &Value) -> [f64; 2] {
    [value.item(0).unwrap().as_f64().unwrap(), value.item(1).unwrap().as_f64().unwrap()]
}

#[test]
fn a_full_read_holds_every_segment_and_joint_of_each_figure_in_the_frame() {
    for name in ["gestures-sheet", "rig-sheet", "startup-sheet"] {
        let (mut session, _) = tape(name);
        let read = geometry(&mut session, None);
        assert_eq!(read.get("full"), Some(&Value::Bool(true)), "{name}");
        let objects = session.editor.shown_frame().unwrap().get("objects").unwrap().elements().unwrap().len();
        assert_eq!(slots_of(&read), (0..objects).collect::<Vec<_>>(), "{name}: every figure has a group");
        let record = session.editor.shown_frame().unwrap();
        for slot in 0..objects {
            let g = group(&read, slot);
            let figure_type = record.get("objects").unwrap().item(slot).unwrap().get("figure_type_id").and_then(Value::as_f64).unwrap();
            let definition = session.report().get("figure_types").unwrap().elements().unwrap().iter().find(|f| f.get("figure_type_id").and_then(Value::as_f64) == Some(figure_type)).unwrap();
            let ids: Vec<f64> = definition.get("segments").unwrap().elements().unwrap().iter().map(|s| s.get("segment_id").and_then(Value::as_f64).unwrap()).collect();
            let segments = g.get("segments").unwrap().elements().unwrap();
            assert_eq!(segments.len(), ids.len(), "{name} slot {slot}: a shape for every segment, drawn or not");
            let joints = g.get("joints").unwrap().elements().unwrap();
            assert_eq!(joints.iter().map(|j| j.item(0).unwrap().as_f64().unwrap()).collect::<Vec<_>>(), ids, "{name} slot {slot}: a joint for every segment");
            for (i, shape) in segments.iter().enumerate() {
                assert_eq!(shape.get("index").and_then(Value::as_f64), Some(i as f64));
                let (end, joint) = (point(shape.get("end").unwrap()), joints.get(i).unwrap());
                if shape.get("connector").is_none() {
                    assert_eq!([end[0], end[1]], [joint.item(1).unwrap().as_f64().unwrap(), joint.item(2).unwrap().as_f64().unwrap()], "{name} slot {slot} segment {i}: the endpoint is the joint");
                }
                // A segment starts at the end of the one it hangs from (or at the root).
                let parent = definition.get("segments").unwrap().item(i).unwrap().get("parent_id").and_then(Value::as_f64).unwrap();
                let origin = if parent == 0.0 { point(g.get("root").unwrap()) } else {
                    let at = ids.iter().position(|id| *id == parent).unwrap();
                    point(&Value::array(vec![joints.get(at).unwrap().item(1).unwrap(), joints.get(at).unwrap().item(2).unwrap()]))
                };
                assert_eq!(point(shape.get("start").unwrap()), origin, "{name} slot {slot} segment {i}: where it starts");
            }
        }
        assert_eq!(read.get("warnings").and_then(Value::array_len), Some(0), "{name}");
    }
}

/// A call of the session by hand.
fn call(session: &mut Session, method: &str, args: &str) {
    let answer = parse(&session.call(method, args, &view(session)));
    assert_eq!(answer.get("status").and_then(Value::as_str), Some("ok"), "{method}: {}", json(&answer));
}

/// The slots a gesture that moves figure `uid` changes in a read that follows a full one, in the tape of
/// figures 1 and 2 with 3 hanging from 2, and 4 alone (the second read, and the full one after it).
fn moved_by(uid: u32) -> (Vec<u32>, Vec<usize>, Value, Value, Value) {
    let (mut session, _) = tape("channels-pose-sheet");
    call(&mut session, "seek", "[11]");
    call(&mut session, "select", &format!("[{uid},-1]"));
    let first = geometry(&mut session, None);
    let token = first.get("token").and_then(Value::as_f64).unwrap();
    // Nothing changed: nothing to send.
    let same = geometry(&mut session, Some(token));
    assert!(slots_of(&same).is_empty(), "an unchanged frame sends no group");
    assert_eq!(same.get("full"), Some(&Value::Bool(false)));
    let token = same.get("token").and_then(Value::as_f64).unwrap();
    call(&mut session, "beginTranslationDrag", "[]");
    call(&mut session, "moveSelection", "[12,0,{\"fromStart\":true}]");
    assert!(session.editor.authored.is_some(), "the gesture is open");
    let moved = geometry(&mut session, Some(token));
    assert_eq!(moved.get("full"), Some(&Value::Bool(false)));
    let full = geometry(&mut session, None);
    // A read that does not hold the previous one is whole.
    let stale = geometry(&mut session, Some(12345.0));
    assert_eq!(stale.get("full"), Some(&Value::Bool(true)));
    let mut slots = slots_of(&moved);
    slots.sort();
    let shown = session.editor.shown_frame().unwrap();
    let mut uids: Vec<u32> = slots.iter().map(|&slot| shown.get("objects").unwrap().item(slot).unwrap().get("unique_id").and_then(Value::as_f64).unwrap() as u32).collect();
    uids.sort();
    (uids, slots, first, moved, full)
}

#[test]
fn a_later_read_names_only_the_figures_a_gesture_moved() {
    let (uids, slots, first, moved, full) = moved_by(4);
    assert_eq!(uids, vec![4], "only the figure the gesture moves");
    // Its group is the one a full read now gives, and has moved.
    assert_eq!(json(&group(&moved, slots[0])), json(&group(&full, slots[0])));
    assert_ne!(json(&group(&first, slots[0])), json(&group(&full, slots[0])));
}

#[test]
fn a_figure_hanging_from_another_moves_with_it_in_the_next_read() {
    // Moving 3 moves what it hangs from and everything joined to that.
    let (uids, slots, _, moved, full) = moved_by(3);
    assert_eq!(uids, vec![2, 3], "the joined figures, and no others");
    for slot in slots {
        assert_eq!(json(&group(&moved, slot)), json(&group(&full, slot)));
    }
}

#[test]
fn the_frame_on_show_is_the_document_at_the_playhead_until_a_gesture_drafts_it() {
    let (mut session, steps) = tape("gestures-sheet");
    let begin = steps.iter().position(|step| step.get("call").and_then(|c| c.item(0)).and_then(|m| m.as_str().map(str::to_string)).as_deref() == Some("beginDrag")).unwrap();
    for step in &steps[1..begin] {
        run(&mut session, step);
    }
    let query = |session: &mut Session, kind: &str| {
        let view = view(session);
        session.query(kind, "{}", &view).unwrap()
    };
    let before = query(&mut session, "frame");
    assert_eq!(before, query(&mut session, "document"), "with no gesture the frame is the document's");
    assert_eq!(before, json(&session.editor.shown_frame().unwrap()));
    for step in &steps[begin..begin + 2] {
        run(&mut session, step);
    }
    let drafted = query(&mut session, "frame");
    assert_ne!(drafted, before, "the draft shows the pose being made");
    assert_eq!(query(&mut session, "document"), before, "the document's frame stays what the draft is composed on");
    assert_eq!(query(&mut session, "motion"), "null", "no motion-path edit is open");
}

#[test]
fn the_timeline_is_one_start_and_count_per_row() {
    let (mut session, _) = tape("gestures-sheet");
    let view = view(&session);
    let rows = parse(&session.query("timeline", "{}", &view).unwrap());
    let rows = rows.elements().unwrap();
    assert!(rows.len() > 1);
    let mut start = 0.0;
    for row in rows.iter() {
        assert_eq!(row.item(0).and_then(|v| v.as_f64()), Some(start));
        start += row.item(1).and_then(|v| v.as_f64()).unwrap();
    }
    assert!(session.query("elsewhere", "{}", &view).is_err());
}
