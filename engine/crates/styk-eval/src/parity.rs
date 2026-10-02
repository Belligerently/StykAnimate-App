//! Parity with the JavaScript editor: compares what this engine evaluates with
//! a reference dump written by `engine/tools/reference.mjs` (every drawable and
//! every joint of the sampled ticks).
//!
//! `styk compare` and the fixture tests share this one comparison, so a
//! tolerance cannot drift between them.

use styk_doc::Value;

use crate::eval::Prepared;
use crate::geometry::{build, ItemKind, Scene};

/// Largest accepted difference of a coordinate or a thickness, in scene units.
pub const TOLERANCE: f64 = 2e-3;

/// The differences found at one tick, as readable lines.
#[derive(Clone, Debug)]
pub struct FrameReport {
    pub tick: f64,
    pub problems: Vec<String>,
}

/// The outcome of comparing a document with a reference dump.
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// Sampled ticks in the reference.
    pub frames: usize,
    /// Drawables in the reference.
    pub primitives: usize,
    pub mismatched_primitives: usize,
    /// Joints in the reference (a root and one per segment of each object).
    pub joints: usize,
    pub mismatched_joints: usize,
    /// `(reference, engine)` when the timeline lengths differ.
    pub total_ticks: Option<(u32, u32)>,
    /// One entry per tick with differences, in tick order.
    pub bad_frames: Vec<FrameReport>,
}

impl Report {
    pub fn matches(&self) -> bool {
        self.bad_frames.is_empty()
    }
}

fn point(v: Option<&Value>) -> Option<[f64; 2]> {
    let a = v?.numbers()?;
    Some([*a.first()?, *a.get(1)?])
}

fn close(a: [f64; 2], b: [f64; 2]) -> bool {
    (a[0] - b[0]).abs() <= TOLERANCE && (a[1] - b[1]).abs() <= TOLERANCE
}

fn items(v: Option<&Value>) -> &[Value] {
    v.and_then(Value::as_arr).unwrap_or(&[])
}

/// Lines kept per tick: enough to see what differs without flooding a report.
const LINES: usize = 6;

/// Compares every tick of `reference` (`frames[].drawables` and
/// `frames[].objects[].joints`) with the engine's scene at that tick.
pub fn compare(prepared: &Prepared, reference: &Value) -> Report {
    let mut report = Report::default();
    let reference_total = reference.get("totalTicks").and_then(Value::as_f64).unwrap_or(0.0) as u32;
    if reference_total != prepared.total_ticks {
        report.total_ticks = Some((reference_total, prepared.total_ticks));
    }
    for frame in items(reference.get("frames")) {
        let tick = frame.get("tick").and_then(Value::as_f64).unwrap_or(0.0);
        let scene: Scene = build(&prepared.doc, &prepared.evaluate(tick));
        let mut problems: Vec<String> = Vec::new();
        report.frames += 1;
        drawables(&scene, items(frame.get("drawables")), &mut report, &mut problems);
        joints(prepared, &scene, items(frame.get("objects")), &mut report, &mut problems);
        if !problems.is_empty() {
            report.bad_frames.push(FrameReport { tick, problems });
        }
    }
    report
}

fn drawables(scene: &Scene, expected: &[Value], report: &mut Report, problems: &mut Vec<String>) {
    report.primitives += expected.len();
    if expected.len() != scene.items.len() {
        problems.push(format!("drawable count: reference {}, engine {}", expected.len(), scene.items.len()));
    }
    for (i, (e, it)) in expected.iter().zip(scene.items.iter()).enumerate() {
        let uid = scene.objects[it.slot as usize].uid;
        let mut why = Vec::new();
        if e.get("uid").and_then(Value::as_f64) != Some(uid as f64) {
            why.push(format!("uid {} vs {}", e.get("uid").unwrap_or(&Value::Null), uid));
        }
        if e.get("type").and_then(Value::as_str) != Some(it.kind.name()) {
            why.push(format!("type {} vs {}", e.get("type").unwrap_or(&Value::Null), it.kind.name()));
        }
        if let Some(z) = e.get("z").and_then(Value::as_f64)
            && (z - it.z).abs() > 1e-6
        {
            why.push(format!("z {z} vs {}", it.z));
        }
        let rgb: Vec<u8> = e.get("rgb").and_then(Value::numbers).map(|a| a.iter().map(|&x| x as u8).collect()).unwrap_or_default();
        if rgb.len() == 3 && rgb != it.rgb {
            why.push(format!("rgb {rgb:?} vs {:?}", it.rgb));
        }
        for (key, value) in [("alpha", it.alpha), ("opacity", it.opacity)] {
            if let Some(v) = e.get(key).and_then(Value::as_f64)
                && (v - value).abs() > 1e-4
            {
                why.push(format!("{key} {v} vs {value}"));
            }
        }
        if it.kind == ItemKind::Poly {
            let points: Vec<[f64; 2]> = e.get("points").and_then(Value::elements).map(|a| a.iter().filter_map(|p| point(Some(&p))).collect()).unwrap_or_default();
            let ours = scene.points_of(it);
            if points.len() != ours.len() {
                why.push(format!("poly points {} vs {}", points.len(), ours.len()));
            } else if let Some(k) = (0..points.len()).find(|&k| !close(points[k], ours[k])) {
                why.push(format!("poly point {k} {:?} vs {:?}", points[k], ours[k]));
            }
        } else {
            if let (Some(start), Some(end)) = (point(e.get("start")), point(e.get("end"))) {
                if !close(start, it.start) {
                    why.push(format!("start {start:?} vs {:?}", it.start));
                }
                if !close(end, it.end) {
                    why.push(format!("end {end:?} vs {:?}", it.end));
                }
            }
            if let Some(thickness) = e.get("thickness").and_then(Value::as_f64)
                && (thickness - it.thickness).abs() > TOLERANCE
            {
                why.push(format!("thickness {thickness} vs {}", it.thickness));
            }
        }
        if !why.is_empty() {
            report.mismatched_primitives += 1;
            if problems.len() < LINES {
                problems.push(format!("#{i} {} uid {} seg/poly {}: {}", it.kind.name(), uid, it.index, why.join("; ")));
            }
        }
    }
}

/// Joints are the positions children and joined figures attach to: each
/// object's root (handle 0) and the handle of each of its segments.
fn joints(prepared: &Prepared, scene: &Scene, expected: &[Value], report: &mut Report, problems: &mut Vec<String>) {
    let mut lines = 0;
    for object in expected {
        let uid = object.get("uid").and_then(Value::as_f64).unwrap_or(-1.0);
        let slot = object.get("slot").and_then(Value::as_f64).unwrap_or(-1.0);
        let ours = scene.objects.get(slot as usize).filter(|g| g.resolved && g.uid as f64 == uid);
        let figure = ours.and_then(|g| prepared.doc.figure(g.figure));
        for joint in object.get("joints").and_then(Value::elements).into_iter().flatten() {
            let Some(values) = joint.numbers().filter(|v| v.len() == 3) else { continue };
            let (handle, position) = (values[0] as u32, [values[1], values[2]]);
            report.joints += 1;
            let found = match (ours, figure) {
                (Some(geometry), Some(figure)) => geometry.joint(figure, handle),
                _ => None,
            };
            if found.is_some_and(|p| close(position, p)) {
                continue;
            }
            report.mismatched_joints += 1;
            if lines < LINES {
                lines += 1;
                problems.push(match found {
                    Some(p) => format!("joint {handle} of uid {uid}: {position:?} vs {p:?}"),
                    None => format!("joint {handle} of uid {uid} (slot {slot}): missing in the engine"),
                });
            }
        }
    }
}
