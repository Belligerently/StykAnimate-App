//! A mirrored document may have its definitions before its frames.

use std::sync::OnceLock;
use std::time::Instant;

use styk_engine::mirror::{Change, Record, Table};
use styk_engine::{Budgets, Engine};

fn clock() -> f64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}

#[test]
fn definitions_commit_before_any_frame_is_known() {
    let engine = Engine::new(clock, Budgets::for_device(None));
    // A layer sheet that names poses the engine has not received yet.
    let properties = br#"{"version":5,"canvas_width":320,"canvas_height":200,"fps":20,"_styk":{"name":"Walk","layers":{"order":["layer-1"],"defaultLayerId":"layer-1","assignments":{},"byId":{"layer-1":{"visible":true}}},"layerSheet":{"duration":12,"tracks":{"layer-1":{"end":12,"keys":[{"tick":0,"value":{"frameId":"pose-a"}},{"tick":6,"value":{"frameId":"pose-b"}}]},"scene":{"end":12,"keys":[{"tick":0,"value":{"frameId":"pose-a"}}]},"camera":{"end":12,"keys":[{"tick":0,"value":{"frameId":"pose-a"}}]}}}}}"#;
    engine.stage(0, Change::Reset).unwrap();
    engine.stage(0, Change::Properties(properties.to_vec())).unwrap();
    engine.stage(0, Change::Layout(Table::Figures, vec![-1])).unwrap();
    let figure = br#"{"figure_type_id":1,"name":"walker","segments":[{"segment_id":1,"parent_id":0,"length":40,"angle_rad":0,"thickness":8,"primitive_code":0,"color_rgb":[0,0,0]}]}"#;
    engine.stage(0, Change::Set(Table::Figures, 0, Record { skeleton: figure.to_vec(), arrays: Vec::new(), payloads: Vec::new() })).unwrap();
    let committed = engine.commit(1, 0).unwrap();
    assert_eq!(committed.converted.figures, 1);
    let generation = engine.generation().unwrap();
    assert_eq!(generation.playable_ticks(), 0, "nothing can be evaluated yet");
    assert!(engine.frame_now(0).is_none());
    engine.request(0, &[0, 1]);
    assert!(!engine.step(), "no frame work is queued for a document without frames");
}
