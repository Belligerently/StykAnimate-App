//! Every command recording in engine/tests/edit (engine/tools/edit-replay.mjs) replays
//! without a difference, and the scenarios of ported commands run to the end.

use std::path::Path;

const PORTED: &[&str] = &[
    "move-native", "move-joined", "drag-cancel", "selection-native", "rotate-native", "transform-joined", "pose-native", "pose-joints", "pose-joined", "pose-snap",
    // The app's configuration: gestures on a layer sheet.
    "move-sheet", "gestures-sheet", "autokey-sheet", "layers-sheet", "locked-sheet", "movieclip-sheet", "movieclip-loose-sheet",
    // Keys and frames of the layer timeline.
    "keys-sheet", "frames-sheet", "frames-movieclip-sheet", "frames-timed-clip-sheet",
    // Commands that draft the whole frame.
    "figures-sheet", "figures-layers-sheet",
    // Inverse-kinematics controllers: the document the app starts with, posing unpinned limbs,
    // gestures around pinned limbs (followers, stretch, chains of four bones).
    "startup-sheet", "ik-unpinned-sheet", "pinned-ik-sheet", "ik-followers-sheet", "ik-chains-sheet",
];

#[test]
fn recordings_replay() {
    let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/edit");
    let mut seen = Vec::new();
    for entry in std::fs::read_dir(&folder).unwrap() {
        let path = entry.unwrap().path();
        if !path.to_string_lossy().ends_with(".reference.json") {
            continue;
        }
        let reference = styk_doc::json::parse(&std::fs::read(&path).unwrap()).unwrap();
        let outcome = styk_edit::replay::replay(&reference).unwrap();
        assert!(outcome.passed(), "{}: {:#?}", outcome.scenario, outcome.mismatches);
        if PORTED.contains(&outcome.scenario.as_str()) {
            assert_eq!((outcome.matched, &outcome.unported), (outcome.steps, &None), "{}", outcome.scenario);
        }
        seen.push(outcome.scenario);
    }
    for name in PORTED {
        assert!(seen.iter().any(|s| s == name), "{name} has no recording");
    }
}
