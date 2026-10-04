//! The deepest movieclip graph the editor accepts must evaluate without truncation.
use std::path::Path;
use styk_eval::parity;

fn field<'a>(value: &'a mut styk_doc::Value, key: &str) -> &'a mut styk_doc::Value {
    let styk_doc::Value::Obj(fields) = value else { panic!("object") };
    std::sync::Arc::make_mut(fields).get_mut(key).unwrap()
}
fn first(value: &mut styk_doc::Value) -> &mut styk_doc::Value {
    let styk_doc::Value::Arr(items) = value else { panic!("array") };
    &mut std::sync::Arc::make_mut(items)[0]
}

#[test]
fn eight_nested_movieclips_match_the_editor_and_require_the_innermost_pose() {
    let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/document");
    let reference = styk_doc::json::parse(&std::fs::read(folder.join("deep-eight.native.reference.json")).unwrap()).unwrap();
    let document = styk_eval::open(&folder.join("deep-eight.json")).unwrap();
    let prepared = styk_eval::prepare(document);
    let report = parity::compare(&prepared, &reference);
    assert!(report.frames == 5 && report.primitives > 0 && report.joints > 0);
    assert!(report.matches(), "{:?}", report.bad_frames);
    let mut value = styk_doc::json::parse(&std::fs::read(folder.join("deep-eight.json")).unwrap()).unwrap();
    let mut leaf = &mut value;
    for _ in 0..8 {
        leaf = field(field(field(first(field(leaf, "figure_types")), "_styk"), "movieclip"), "document");
    }
    let pose = first(field(leaf, "keyframes"));
    let root = field(first(field(pose, "objects")), "root_x");
    *root = styk_doc::Value::Num(root.as_f64().unwrap() + 100.0);
    let mutated = styk_eval::prepare(styk_eval::from_value(&value, "mutated").unwrap());
    assert!(!parity::compare(&mutated, &reference).matches(), "the innermost pose did not affect the picture");
}
