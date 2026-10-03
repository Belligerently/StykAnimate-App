//! What the scene camera does to drawn frames: prepared frames are always shown
//! through it, edit frames only in the camera preview (docs/engine/evaluation.md).

use std::sync::OnceLock;
use std::time::Instant;

use styk_engine::edit::{EditOptions, EditScene};
use styk_engine::{Budgets, Engine};
use styk_eval::drawlist::{encode_with, Overrides};

fn clock() -> f64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}

/// The editor-built parallax fixture: a back layer (response 0.25, reference at
/// (60, -30)) and a front layer (1.5 across, 0.6 down, reference at (-40, 20)).
fn parallax_engine() -> Engine {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../styk-eval/tests/fixtures/eval/parallax-layers.json");
    let engine = Engine::new(clock, Budgets::for_device(None));
    engine.open(&std::fs::read(path).expect("fixture"), "json", "parallax-layers").expect("opens");
    engine
}

#[test]
fn prepared_frames_move_parallax_layers() {
    let engine = parallax_engine();
    let generation = engine.generation().unwrap();
    let prepared = &generation.prepared;
    for tick in [3u32, 11, 20] {
        let frame = prepared.evaluate(tick as f64);
        let parallax = frame.parallax.clone().expect("the camera has travelled: layers move");
        let scene = styk_eval::geometry::build(&prepared.doc, &frame);
        let translations = parallax.translations(&[]);
        let moved = encode_with(&prepared.doc, &generation.assets, &scene, &Overrides { translations: &translations, ..Default::default() });
        let still = encode_with(&prepared.doc, &generation.assets, &scene, &Overrides::default());
        let drawn = engine.frame_now(tick).expect("prepared");
        assert_eq!(drawn.list.prims, moved.prims, "tick {tick}: the prepared frame is drawn with its parallax");
        assert_ne!(drawn.list.prims, still.prims, "tick {tick}: parallax moves something");
    }
}

#[test]
fn edit_frames_move_parallax_layers_only_through_the_camera() {
    let engine = parallax_engine();
    let generation = engine.generation().unwrap();
    let mut scene = EditScene::default();
    // The front figure (uid 2) alone, with the camera at (100, 0).
    let pose = r#"{"camera":{"is_default":false,"x":100,"y":0,"scale":1,"angle_rad":0},"objects":[{"unique_id":2,"figure_type_id":2,"root_x":300,"root_y":150,"scale":1,"angles":[0.8,2,0]}]}"#;
    scene.update(&generation, pose.as_bytes()).unwrap();
    let workspace = scene.draw_list(&generation, &EditOptions::default()).unwrap().unwrap();
    let preview = scene.draw_list(&generation, &EditOptions { camera: true, ..Default::default() }).unwrap().unwrap();
    let dragged = scene.draw_list(&generation, &EditOptions { camera: true, translations: &[(0, [5.0, 1.0])], ..Default::default() }).unwrap().unwrap();
    // (1 - 1.5) × (100 - -40) across and (1 - 0.6) × (0 - 20) down; a drag adds to it.
    let shift = |list: &styk_eval::drawlist::DrawList, d: [f32; 2]| list.prims.iter().zip(&workspace.prims).filter(|(_, w)| w.kind & 255 == styk_eval::drawlist::K_CAPSULE).all(|(p, w)| p.a == [w.a[0] + d[0], w.a[1] + d[1], w.a[2] + d[0], w.a[3] + d[1]]);
    assert!(workspace.prims.iter().filter(|p| p.kind & 255 == styk_eval::drawlist::K_CAPSULE).count() == 3, "three segments");
    assert!(shift(&preview, [-70.0, -8.0]), "the front layer moves against the camera in the preview");
    assert!(shift(&dragged, [-65.0, -7.0]), "a drag adds to the parallax");
}
