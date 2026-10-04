fn replay(names: &[&str]) {
    let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/edit");
    for name in names {
        let reference = styk_doc::json::parse(
            &std::fs::read(folder.join(format!("{name}.reference.json"))).unwrap(),
        )
        .unwrap();
        let outcome = styk_edit::replay::replay(&reference).unwrap();
        assert!(
            outcome.passed() && outcome.matched == outcome.steps && outcome.unported.is_none(),
            "{name}: {outcome:#?}"
        );
        println!(
            "{name}: {} / {} steps matched",
            outcome.matched, outcome.steps
        );
    }
}

#[test]
fn drawing_new_figures_on_native_frames_matches_the_editor() {
    replay(&[
        "draw-new-figure-native",
        "draw-new-figure-active-layer-native",
    ]);
}

#[test]
fn drawing_new_figures_on_the_sheet_matches_the_editor() {
    replay(&[
        "draw-new-figure-sheet",
        "draw-new-figure-locked-sheet",
        "draw-new-figure-active-layer-sheet",
    ]);
}

#[test]
fn identity_capacity_preserves_document_library_history_and_open_gestures() {
    replay(&[
        "draw-identity-capacity-native",
        "draw-identity-capacity-sheet",
    ]);
}

#[test]
fn drawing_keeps_the_open_pose_gesture_and_its_history_order() {
    replay(&[
        "draw-new-figure-live-native",
        "draw-new-figure-live-sheet",
        "draw-new-figure-live-commit-native",
        "draw-new-figure-live-commit-sheet",
        "draw-new-figure-live-unchanged-native",
        "draw-new-figure-live-unchanged-sheet",
    ]);
}

#[test]
fn native_channel_creation_remains_atomic_unported() {
    use styk_doc::Value;
    use styk_edit::{EditError, Editor, Options, Selection};
    let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/edit");
    let reference = styk_doc::json::parse(
        &std::fs::read(folder.join("draw-new-figure-native-channels.reference.json")).unwrap(),
    )
    .unwrap();
    let steps = reference.get("steps").unwrap();
    let first = steps.item(0).unwrap();
    let mut editor = Editor::new(
        styk_doc::json::parse(first.get("document").unwrap().as_str().unwrap().as_bytes()).unwrap(),
        Options {
            property_channels: true,
            layer_timeline: false,
            auto_key: false,
        },
    );
    editor.set_view(
        0.0,
        0,
        Selection {
            uid: None,
            uids: Vec::new(),
            joint: -1,
        },
    );
    let before = (
        editor.report_json(),
        editor.state(),
        editor.drag_value(),
        editor.draft_figures(),
    );
    let call: Vec<Value> = steps
        .item(2)
        .unwrap()
        .get("call")
        .unwrap()
        .elements()
        .unwrap()
        .iter()
        .collect();
    assert!(matches!(
        editor.call(call[0].as_str().unwrap(), &call[1..]),
        Err(EditError::Unported(_))
    ));
    assert_eq!(
        (
            editor.report_json(),
            editor.state(),
            editor.drag_value(),
            editor.draft_figures()
        ),
        before
    );
    assert!(editor.take_events().is_empty());
}
