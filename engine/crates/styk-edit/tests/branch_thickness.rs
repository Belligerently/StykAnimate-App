#[test]
fn branch_thickness_recordings_match_every_step() {
    let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/edit");
    for name in ["branch-thickness-native", "branch-thickness-sheet", "branch-thickness-locked-sheet"] {
        let reference = styk_doc::json::parse(&std::fs::read(folder.join(format!("{name}.reference.json"))).unwrap()).unwrap();
        let outcome = styk_edit::replay::replay(&reference).unwrap();
        assert!(outcome.passed() && outcome.matched == outcome.steps && outcome.unported.is_none(), "{name}: {outcome:#?}");
        println!("{name}: {} / {} steps matched", outcome.matched, outcome.steps);
    }
}

#[test]
fn branch_thickness_on_an_origin_document_refuses_without_changes() {
    use styk_doc::Value;
    let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/edit");
    let reference = styk_doc::json::parse(&std::fs::read(folder.join("branch-thickness-origin-sheet.reference.json")).unwrap()).unwrap();
    let first = reference.get("steps").unwrap().item(0).unwrap();
    let report = styk_doc::json::parse(first.get("document").unwrap().as_str().unwrap().as_bytes()).unwrap();
    let mut editor = styk_edit::Editor::new(report, styk_edit::Options { property_channels: true, layer_timeline: true, auto_key: false });
    editor.set_view(0.0, 0, styk_edit::Selection { uid: Some(1), uids: vec![1], joint: 1 });
    let before = (editor.report_json(), editor.state(), editor.drag_value());
    assert!(matches!(editor.call("changeBranchThickness", &[Value::Num(0.15)]), Err(styk_edit::EditError::Unported(_))));
    assert_eq!((editor.report_json(), editor.state(), editor.drag_value()), before);
    assert!(editor.take_events().is_empty());
}
