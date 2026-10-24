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

/// The numbers of a row of a group's `segs`.
fn row(shape: &Value) -> Vec<f64> {
    shape.elements().unwrap().iter().map(|value| value.as_f64().unwrap_or(f64::NAN)).collect()
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
            let segments = definition.get("segments").unwrap().elements().unwrap();
            let ids: Vec<f64> = segments.iter().map(|s| s.get("segment_id").and_then(Value::as_f64).unwrap()).collect();
            let ends: Vec<f64> = g.get("ends").unwrap().elements().unwrap().iter().map(|value| value.as_f64().unwrap_or(f64::NAN)).collect();
            assert_eq!(ends.len(), 2 * ids.len(), "{name} slot {slot}: a joint for every segment");
            let rows = g.get("segs").unwrap().elements().unwrap();
            assert_eq!(rows.len(), ids.len(), "{name} slot {slot}: a row for every segment, drawn or not");
            let joint = |index: usize| [ends[2 * index], ends[2 * index + 1]];
            for (i, shape) in rows.iter().enumerate() {
                let row = row(&shape);
                assert_eq!(row[0], i as f64, "the segment's place in the figure");
                let flags = row[11] as u32;
                // A segment is drawn to its joint and starts where its parent ends (or at the root), unless the row says otherwise.
                let mut at = 12;
                let parent = segments.get(i).unwrap().get("parent_id").and_then(Value::as_f64).unwrap();
                let origin = if parent == 0.0 { point(g.get("root").unwrap()) } else { joint(ids.iter().position(|id| *id == parent).unwrap()) };
                let start = if flags & 1 != 0 { at += 2; [row[at - 2], row[at - 1]] } else { origin };
                let end = if flags & 2 != 0 { at += 2; [row[at - 2], row[at - 1]] } else { joint(i) };
                if flags & 8 == 0 {
                    assert_eq!(flags & 2, 0, "{name} slot {slot} segment {i}: only a connector is drawn to somewhere but its joint");
                }
                assert!(start[0].is_finite() && end[0].is_finite() && row[3].is_finite(), "{name} slot {slot} segment {i}: finite");
                assert_eq!(flags & 4 != 0, shape.elements().unwrap().iter().any(|value| value.is_array()), "{name} slot {slot} segment {i}: a curve is its own array");
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

/// What the session answers to a read of the editor's own, as `{"v": value}` text (or `{}`).
fn read(session: &mut Session, name: &str, args: &str) -> Value {
    let view = view(session);
    parse(&session.query(name, args, &view).unwrap_or_else(|error| panic!("{name}: {error}")))
}

#[test]
fn the_values_the_interface_shows_are_read_from_the_frame_on_show() {
    let (mut session, _) = tape("channels-pose-sheet");
    call(&mut session, "seek", "[11]");
    let shown = session.editor.shown_frame().unwrap();
    let figure = |uid: u32| shown.get("objects").unwrap().elements().unwrap().iter().find(|o| o.get("unique_id").and_then(Value::as_f64) == Some(uid as f64)).unwrap();
    // A property at the playhead is the frame's own value, at another tick the document's there.
    let x = read(&mut session, "animationValue", r#"[{"kind":"figure","uid":4},"x"]"#);
    assert_eq!(x.get("v").and_then(Value::as_f64), figure(4).get("root_x").and_then(Value::as_f64));
    let camera = read(&mut session, "animationValue", r#"[{"kind":"camera"},"x"]"#);
    assert_eq!(camera.get("v").and_then(Value::as_f64), shown.get("camera").and_then(|c| c.get("x")).and_then(Value::as_f64));
    let later = read(&mut session, "animationValue", r#"[{"kind":"figure","uid":4},"x",13]"#);
    assert!(later.get("v").and_then(Value::as_f64).is_some(), "the document at another tick has its own value");
    // A figure that is not there has no value: the editor answers undefined.
    assert_eq!(json(&read(&mut session, "animationValue", r#"[{"kind":"figure","uid":999},"x"]"#)), "{}");
    // A tick the document does not have is refused with the editor's message.
    let at = view(&session);
    assert_eq!(session.query("animationValue", r#"[{"kind":"figure","uid":4},"x",-1]"#, &at).unwrap_err(), "errors.editorAnimation.keyframeTimeMustBeAnIntegerFrom0");
    // The target of a segment's property is its figure, handle and nothing more on a layer sheet.
    assert_eq!(json(&read(&mut session, "segmentAnimationTarget", r#"[4,2,"x"]"#)), r#"{"v":{"kind":"segment","uid":4,"handleId":2}}"#);
    // No effect yet: an empty stack; one added shows in its target's.
    call(&mut session, "select", "[4,-1]");
    assert_eq!(json(&read(&mut session, "vfxEffects", r#"[{"kind":"figure","uid":4}]"#)), r#"{"v":[]}"#);
    // The identity the effect takes is one the editor drew and hands over with the call.
    let mut drawn = parse(&view(&session));
    drawn.make_obj().unwrap().set("draws", parse(r#"[["uuid","8a3c2f10-5c3b-4d4e-9d0a-6b2f1e7c9a11"],["seed",7],["random",0.5]]"#));
    drawn.make_obj().unwrap().set("drawPool", Value::Bool(true));
    let answer = parse(&session.call("addVfx", r#"[[{"kind":"figure","uid":4}],"glow"]"#, &json(&drawn)));
    assert_eq!(answer.get("status").and_then(Value::as_str), Some("ok"), "{}", json(&answer));
    let stack = read(&mut session, "vfxEffects", r#"[{"kind":"figure","uid":4}]"#);
    assert_eq!(stack.get("v").unwrap().elements().unwrap().len(), 1);
    assert_eq!(stack.get("v").unwrap().item(0).unwrap().get("type").and_then(Value::as_str), Some("glow"));
    let at = view(&session);
    assert!(session.query("elsewhere", "[]", &at).unwrap_err().contains("answers no read"));
}

#[test]
fn the_settings_of_a_controller_are_the_defaults_over_what_the_node_has() {
    let (mut session, _) = tape("channels-pose-sheet");
    call(&mut session, "seek", "[11]");
    // Nothing selected below the figure: no node, no chain.
    call(&mut session, "select", "[4,-1]");
    let none = read(&mut session, "getIKSettings", "[]");
    assert_eq!(json(none.get("v").unwrap()), r#"{"enabled":false,"pinned":false,"allowStretch":false,"bendDirection":0,"chainLength":0,"maxLength":0}"#);
    // A node of an unjoined figure: a chain of two where there are two bones, and the longest it can have.
    call(&mut session, "select", "[4,0]");
    let node = read(&mut session, "getIKSettings", "[]");
    let node = node.get("v").unwrap();
    assert_eq!(node.get("enabled"), Some(&Value::Bool(false)));
    let longest = node.get("maxLength").and_then(Value::as_f64).unwrap();
    assert!(longest >= 1.0);
    assert_eq!(node.get("chainLength").and_then(Value::as_f64), Some(longest.min(2.0)));
    // A node of a figure with others joined to it: the controllers act across the assembly, which the editor answers.
    call(&mut session, "select", "[2,0]");
    let at = view(&session);
    assert!(session.query("getIKSettings", "[]", &at).unwrap_err().starts_with(crate::session::UNPORTED));
}
