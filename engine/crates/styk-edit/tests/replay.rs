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
    // IK across joined figures: pinned and free, a follower joined late, the start document joined.
    "ik-joined-sheet", "ik-joined-unpinned-sheet", "ik-joined-late-sheet", "startup-join-sheet",
    // Stick Nodes curves; rig controls, alone and with pinned IK.
    "sticknodes-sheet", "rig-sheet", "ik-rig-follow-sheet", "ik-rig-stretch-sheet",
    // Documents with 3D bones, meshes, surfaces, perspective and part proportions.
    "spatial-mesh-perspective-sheet", "spatial-mesh-sheet-sheet", "spatial-surfaces-skin-sheet", "spatial-surfaces-perspective-sheet",
    "spatial-mesh-mechanism-sheet", "spatial-clip-mesh-sheet",
    // 3D bones: turned, dragged, twisted; mechanisms' hinges and slides.
    "spatial-turn-sheet", "spatial-mechanism-sheet", "spatial-drag-sheet", "spatial-mechanism-drag-sheet",
    // A planted 3D leg (spatial IK target): Both, a side, mirrored.
    "ik-spatial-sheet", "ik-spatial-side-sheet", "ik-spatial-mirrored-sheet",
    // Posing 3D IK targets: planted (flat pole, depth push, swivel, pole and depth), bent to a side, free.
    "ik-spatial-target-sheet", "ik-spatial-side-target-sheet", "ik-spatial-free-sheet",
    "ik-spatial-steer-sheet", "ik-spatial-side-steer-sheet", "ik-spatial-mirrored-steer-sheet", "spatial-reset-sheet",
    // Pose choices: 3D bones shown one at a time, presets over channels.
    "pose-choices-sheet", "pose-presets-sheet", "pose-choice-mechanism-sheet",
    // IK controllers' settings: switched, pinned, stretched, chain and bend, targets turned, 3D feet.
    "ik-settings-sheet", "ik-settings-pinned-sheet", "ik-settings-spatial-sheet", "ik-settings-rig-sheet", "ik-settings-followers-sheet",
    // The layer list: names, colours, visibility, locks, new layers, order.
    "layer-list-sheet", "layers-native", "layers-folders-sheet",
    // Figures added from the library.
    "add-figure-sheet", "add-figure-layers-sheet",
    // Keys moved and copied along the timeline.
    "move-frames-sheet", "move-frames-imported-sheet",
    // The inspector's figure fields.
    "update-object-sheet", "update-object-native", "update-segment-sheet", "update-segment-rig-sheet",
    // Camera motion clips.
    "camera-motion-sheet", "palette-sheet", "audio-sheet", "audio-none-sheet", "sheet-clipboard-sheet", "sheet-clipboard-empty-sheet", "sheet-clipboard-ik-sheet",
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
