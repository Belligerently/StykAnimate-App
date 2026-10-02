//! An edit converts only the records it replaced into typed views.

use std::sync::Arc;

use styk_doc::{Value, json};
use styk_eval::load::{Converted, Loader};

fn project() -> Value {
    let figure = |id: u32| format!(r#"{{"figure_type_id":{id},"name":"f{id}","segments":[{{"segment_id":1,"parent_id":0,"length":40,"angle_rad":0,"thickness":8,"primitive_code":0,"color_rgb":[0,0,0]}}]}}"#);
    let key = |x: f64| format!(r#"{{"duration_raw":4,"objects":[{{"unique_id":7,"figure_type_id":1,"root_x":{x},"root_y":10,"scale":1,"angles":[0.25]}},{{"unique_id":8,"figure_type_id":2,"root_x":5,"root_y":5,"scale":1,"angles":[0]}}]}}"#);
    let text = format!(r#"{{"version":5,"canvas_width":320,"canvas_height":200,"fps":20,"figure_types":[{},{}],"keyframes":[{},{},{}],"backgrounds":[]}}"#, figure(1), figure(2), key(1.0), key(2.0), key(3.0));
    json::parse(text.as_bytes()).unwrap()
}

/// `document.keyframes[frame].objects[object].root_x = x`, copying only that path.
fn move_object(document: &mut Value, frame: usize, object: usize, x: f64) {
    let frames = document.make_obj().unwrap().get_mut("keyframes").unwrap().make_arr().unwrap();
    let objects = frames[frame].make_obj().unwrap().get_mut("objects").unwrap().make_arr().unwrap();
    objects[object].make_obj().unwrap().set("root_x", Value::Num(x));
}

#[test]
fn a_pose_edit_converts_one_keyframe_and_no_figure() {
    let mut loader = Loader::new();
    let original = project();
    let first = loader.document(&original, "p").unwrap();
    assert_eq!(loader.converted, Converted { figures: 2, keyframes: 3 });

    let mut edited = original.clone();
    move_object(&mut edited, 1, 0, 99.0);
    let second = loader.document(&edited, "p").unwrap();
    assert_eq!(loader.converted, Converted { figures: 0, keyframes: 1 });
    assert_eq!(second.keyframes[1].objects[0].root[0], 99.0);
    // Untouched records are the very same views.
    assert!(Arc::ptr_eq(&first.keyframes[0], &second.keyframes[0]));
    assert!(Arc::ptr_eq(&first.keyframes[2], &second.keyframes[2]));
    assert!(Arc::ptr_eq(&first.figures[0], &second.figures[0]));
    assert!(!Arc::ptr_eq(&first.keyframes[1], &second.keyframes[1]));
    // The revision before the edit is intact.
    assert_eq!(first.keyframes[1].objects[0].root[0], 2.0);

    // Converting the same tree again converts nothing.
    loader.document(&edited, "p").unwrap();
    assert_eq!(loader.converted, Converted::default());
}

#[test]
fn views_of_dropped_revisions_are_released() {
    let mut loader = Loader::new();
    let original = project();
    loader.document(&original, "p").unwrap();
    let mut edited = original.clone();
    move_object(&mut edited, 0, 1, 42.0);
    loader.document(&edited, "p").unwrap();
    drop(original);
    loader.release_unused();
    // The replaced keyframe of the dropped revision is gone; the live revision converts nothing new.
    loader.document(&edited, "p").unwrap();
    assert_eq!(loader.converted, Converted::default());
    drop(edited);
    loader.release_unused();
    let fresh = project();
    loader.document(&fresh, "p").unwrap();
    assert_eq!(loader.converted, Converted { figures: 2, keyframes: 3 });
}
