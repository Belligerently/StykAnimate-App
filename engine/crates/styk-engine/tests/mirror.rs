//! The mirrored document: staged changes, atomic commits, and edit frames.

use std::sync::OnceLock;
use std::time::Instant;

use styk_engine::edit::{EditOptions, EditScene};
use styk_engine::mirror::{Change, Record, Table};
use styk_engine::{Budgets, Engine};
use styk_eval::load::Converted;

fn clock() -> f64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}

fn plain(text: &str) -> Record {
    Record { skeleton: text.as_bytes().to_vec(), arrays: Vec::new(), payloads: Vec::new() }
}

const FIGURE: &str = r#"{"figure_type_id":1,"name":"walker","segments":[{"segment_id":1,"parent_id":0,"length":40,"angle_rad":0,"thickness":8,"primitive_code":0,"color_rgb":[0,0,0]}]}"#;

fn key(x: f64) -> String {
    format!(r#"{{"duration_raw":10,"objects":[{{"unique_id":7,"figure_type_id":1,"root_x":{x},"root_y":100,"scale":1,"angles":[0.5]}}]}}"#)
}

/// Stages a whole three-key document, as the interface does when a project opens.
fn stage_document(engine: &Engine) {
    engine.stage(0, Change::Reset);
    engine.stage(0, Change::Properties(br#"{"version":5,"canvas_width":320,"canvas_height":200,"fps":20,"_styk":{"name":"Walk"}}"#.to_vec()));
    engine.stage(0, Change::Layout(Table::Figures, vec![-1]));
    engine.stage(0, Change::Set(Table::Figures, 0, plain(FIGURE)));
    engine.stage(0, Change::Layout(Table::Backgrounds, vec![]));
    engine.stage(0, Change::Layout(Table::Keyframes, vec![-1, -1, -1]));
    for (index, x) in [10.0, 20.0, 30.0].into_iter().enumerate() {
        engine.stage(0, Change::Set(Table::Keyframes, index as u32, plain(&key(x))));
    }
}

#[test]
fn a_staged_document_becomes_the_open_one_on_commit() {
    let engine = Engine::new(clock, Budgets::for_device(None));
    stage_document(&engine);
    assert!(engine.generation().is_none(), "nothing is visible before the commit");
    let committed = engine.commit(41, 0).unwrap();
    assert_eq!(committed.converted, Converted { figures: 1, keyframes: 3 });
    let generation = engine.generation().unwrap();
    assert_eq!((generation.id, generation.revision), (committed.generation, 41));
    assert_eq!(generation.prepared.doc.name, "Walk");
    assert_eq!(generation.prepared.total_ticks, 21);
    assert_eq!(generation.prepared.doc.keyframes[2].objects[0].root[0], 30.0);
}

#[test]
fn an_edit_sends_and_converts_only_what_changed() {
    let engine = Engine::new(clock, Budgets::for_device(None));
    stage_document(&engine);
    let first = engine.commit(1, 0).unwrap();
    let before = engine.generation().unwrap();
    assert!(engine.frame_now(5).is_some());

    engine.stage(0, Change::Set(Table::Keyframes, 1, plain(&key(99.0))));
    let second = engine.commit(2, 0).unwrap();
    assert_eq!(second.converted, Converted { figures: 0, keyframes: 1 });
    let after = engine.generation().unwrap();
    assert_eq!(after.lineage, before.lineage, "the same project");
    assert!(second.generation > first.generation);
    assert_eq!(after.prepared.doc.keyframes[1].objects[0].root[0], 99.0);
    assert!(std::sync::Arc::ptr_eq(&before.prepared.doc.keyframes[0], &after.prepared.doc.keyframes[0]));
    assert!(engine.frame(5).is_none(), "frames of the previous revision are not shown");
    // The revision held by `before` still evaluates as it did.
    assert_eq!(before.prepared.doc.keyframes[1].objects[0].root[0], 20.0);
}

#[test]
fn frames_can_be_inserted_removed_and_reordered_without_resending_them() {
    let engine = Engine::new(clock, Budgets::for_device(None));
    stage_document(&engine);
    engine.commit(1, 0).unwrap();
    // New order: the old third key, a new key, the old first key. The old second key is gone.
    engine.stage(0, Change::Layout(Table::Keyframes, vec![2, -1, 0]));
    engine.stage(0, Change::Set(Table::Keyframes, 1, plain(&key(55.0))));
    let committed = engine.commit(2, 0).unwrap();
    assert_eq!(committed.converted, Converted { figures: 0, keyframes: 1 });
    let doc = &engine.generation().unwrap().prepared.doc;
    let roots: Vec<f64> = doc.keyframes.iter().map(|k| k.objects[0].root[0]).collect();
    assert_eq!(roots, [30.0, 55.0, 10.0]);
}

#[test]
fn a_refused_commit_leaves_the_document_as_it_was() {
    let engine = Engine::new(clock, Budgets::for_device(None));
    stage_document(&engine);
    engine.commit(1, 0).unwrap();
    let before = engine.generation().unwrap();
    // A record outside the table, a layout whose new record never arrives, and broken JSON.
    for changes in [
        vec![Change::Set(Table::Keyframes, 9, plain(&key(1.0)))],
        vec![Change::Layout(Table::Keyframes, vec![0, -1])],
        vec![Change::Set(Table::Keyframes, 0, plain("{broken"))],
        vec![Change::Set(Table::Keyframes, 0, plain(&key(5.0))), Change::Layout(Table::Figures, vec![7])],
    ] {
        for change in changes {
            engine.stage(0, change);
        }
        assert!(engine.commit(2, 0).is_err());
        let current = engine.generation().unwrap();
        assert_eq!(current.id, before.id);
        assert_eq!(current.prepared.doc.keyframes[0].objects[0].root[0], 10.0);
    }
    // The next good commit is not affected by the refused ones.
    engine.stage(0, Change::Set(Table::Keyframes, 0, plain(&key(11.0))));
    assert_eq!(engine.commit(3, 0).unwrap().converted, Converted { figures: 0, keyframes: 1 });
}

#[test]
fn batches_are_staged_and_committed_independently() {
    let engine = Engine::new(clock, Budgets::for_device(None));
    stage_document(&engine);
    engine.commit(1, 0).unwrap();
    // A frame batch is half staged when a small commit of another batch goes through.
    engine.stage(7, Change::Layout(Table::Keyframes, vec![0, 1, 2, -1]));
    engine.stage(0, Change::Properties(br#"{"version":5,"canvas_width":400,"canvas_height":200,"fps":20}"#.to_vec()));
    engine.commit(2, 0).unwrap();
    assert_eq!(engine.generation().unwrap().prepared.doc.canvas[0], 400);
    assert_eq!(engine.generation().unwrap().prepared.doc.keyframes.len(), 3);
    engine.stage(7, Change::Set(Table::Keyframes, 3, plain(&key(44.0))));
    engine.commit(3, 7).unwrap();
    let doc = engine.generation().unwrap().prepared.doc.clone();
    assert_eq!((doc.keyframes.len(), doc.canvas[0]), (4, 400));
}

#[test]
fn pose_sources_are_added_and_removed_by_identity() {
    let engine = Engine::new(clock, Budgets::for_device(None));
    stage_document(&engine);
    engine.stage(0, Change::Source("pose-a".into(), Some(plain(&key(70.0)))));
    engine.commit(1, 0).unwrap();
    let doc = engine.generation().unwrap().prepared.doc.clone();
    assert_eq!(doc.frame_sources["pose-a"].objects[0].root[0], 70.0);
    // New properties keep the sources that were not named.
    engine.stage(0, Change::Properties(br#"{"version":5,"canvas_width":640,"canvas_height":200,"fps":20,"_styk":{"name":"Walk 2"}}"#.to_vec()));
    engine.commit(2, 0).unwrap();
    let doc = engine.generation().unwrap().prepared.doc.clone();
    assert_eq!((doc.canvas[0], doc.name.as_str(), doc.frame_sources.len()), (640, "Walk 2", 1));
    engine.stage(0, Change::Source("pose-a".into(), None));
    engine.commit(3, 0).unwrap();
    assert!(engine.generation().unwrap().prepared.doc.frame_sources.is_empty());
}

#[test]
fn an_edit_frame_draws_the_pose_the_editor_shows() {
    let engine = Engine::new(clock, Budgets::for_device(None));
    stage_document(&engine);
    engine.commit(1, 0).unwrap();
    let generation = engine.generation().unwrap();
    let mut scene = EditScene::default();
    let pose = |x: f64| format!(r#"{{"objects":[{{"unique_id":7,"figure_type_id":1,"root_x":{x},"root_y":100,"scale":1,"angles":[0]}}]}}"#);
    scene.update(&generation, pose(50.0).as_bytes()).unwrap();
    let list = scene.draw_list(&generation, &EditOptions::default()).unwrap();
    // The background fill and one capsule from (50, 100) to (90, 100).
    let capsule = list.prims.last().unwrap();
    assert_eq!(capsule.a, [50.0, 100.0, 90.0, 100.0]);

    // A dragged figure moves by its translation; a hidden one is not drawn.
    let dragged = scene.draw_list(&generation, &EditOptions { translations: &[(0, [5.0, -3.0])], ..Default::default() }).unwrap();
    assert_eq!(dragged.prims.last().unwrap().a, [55.0, 97.0, 95.0, 97.0]);
    let hidden = scene.draw_list(&generation, &EditOptions { hidden: &[0], ..Default::default() }).unwrap();
    assert_eq!(hidden.prims.len(), list.prims.len() - 1);

    // `null` keeps the object of the previous frame.
    scene.update(&generation, br#"{"objects":[null]}"#).unwrap();
    assert_eq!(scene.draw_list(&generation, &EditOptions::default()).unwrap().prims.last().unwrap().a, [50.0, 100.0, 90.0, 100.0]);
    // A frame that keeps a slot the previous one did not have is refused.
    assert!(scene.update(&generation, br#"{"objects":[null,null]}"#).is_err());
}
