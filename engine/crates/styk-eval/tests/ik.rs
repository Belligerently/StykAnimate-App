//! Inverse kinematics against the JavaScript editor.
//!
//! Every fixture in `tests/fixtures/ik/` is a small project and the editor's
//! own evaluation of it (`node engine/tools/ik-fixtures.mjs` regenerates
//! them). The engine must draw the same joints and primitives, within the
//! tolerance `styk compare` uses, and the solver functions must return what
//! the JavaScript returns for the recorded inputs.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use styk_doc::Value;
use styk_eval::doc::{Document, Keyframe};
use styk_eval::ik::{self, parse, BendReference};
use styk_eval::parity::{self, Report};

fn folder() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ik")
}

fn json(path: &Path) -> Value {
    styk_doc::json::parse(&std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Fixture names, from the reference dumps present in the folder.
fn fixtures() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(folder())
        .expect("fixture folder")
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.strip_suffix(".reference.json").map(str::to_string))
        .collect();
    names.sort();
    names
}

fn open(name: &str) -> Document {
    let path = folder().join(format!("{name}.json"));
    styk_eval::open(&path).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn compare(name: &str, document: Document) -> Report {
    parity::compare(&styk_eval::prepare(document), &json(&folder().join(format!("{name}.reference.json"))))
}

fn describe(report: &Report) -> String {
    report.bad_frames.iter().map(|frame| format!("tick {}:\n  {}", frame.tick, frame.problems.join("\n  "))).collect::<Vec<_>>().join("\n")
}

/// The same document with every controller removed: what the engine drew
/// before it evaluated IK.
fn without_ik(mut document: Document) -> Document {
    let strip = |frame: &mut Arc<Keyframe>| {
        for object in Arc::make_mut(frame).objects.iter_mut() {
            Arc::make_mut(object).ik = None;
        }
    };
    document.keyframes.iter_mut().for_each(strip);
    document.frame_sources.values_mut().for_each(strip);
    document
}

/// Fixtures whose poses are drawn as authored: no pinned controller, so
/// removing the tables changes nothing.
const AUTHORED: [&str; 2] = ["startup", "unpinned"];
/// The pinned limb is inside the clip's own document, which `without_ik` does not reach.
const NESTED: [&str; 1] = ["movieclip"];

#[test]
fn every_fixture_evaluates_like_the_editor() {
    let names = fixtures();
    assert!(names.len() >= 20, "fixtures are missing: run node engine/tools/ik-fixtures.mjs");
    for name in &names {
        let document = open(name);
        assert!(document.unsupported.is_empty(), "{name}: not evaluated: {:?}", document.unsupported);
        let report = compare(name, document);
        assert!(report.frames > 0 && report.joints > 0, "{name}: an empty reference");
        assert!(report.total_ticks.is_none(), "{name}: timeline length {:?}", report.total_ticks);
        assert!(
            report.matches(),
            "{name}: {} of {} primitives and {} of {} joints differ\n{}",
            report.mismatched_primitives,
            report.primitives,
            report.mismatched_joints,
            report.joints,
            describe(&report)
        );
    }
}

/// A fixture that passes with its controllers removed would prove nothing.
#[test]
fn fixtures_with_pins_depend_on_the_solver() {
    for name in fixtures() {
        if NESTED.contains(&name.as_str()) {
            continue;
        }
        let document = open(&name);
        let pinned = document.keyframes.iter().chain(document.frame_sources.values()).any(|frame| frame.objects.iter().any(|o| ik::has_pinned(o)));
        assert_eq!(pinned, !AUTHORED.contains(&name.as_str()), "{name}: pinned controllers");
        let report = compare(&name, without_ik(document));
        assert_eq!(report.matches(), !pinned, "{name}: without IK, {} joints differ", report.mismatched_joints);
    }
}

#[test]
fn the_startup_document_has_four_two_bone_controllers() {
    let document = open("startup");
    let blue = &document.keyframes[0].objects[0];
    let table = blue.ik.as_ref().expect("Blue's controllers");
    assert_eq!(table.controllers.len(), 4);
    assert!(table.controllers.values().all(|c| c.enabled && !c.pinned && c.chain_length == 2));
    assert!(!ik::has_pinned(blue));
}

fn number(v: &Value, key: &str) -> f64 {
    v.get(key).and_then(Value::as_f64).unwrap_or_else(|| panic!("{key} in {v}"))
}

fn points(v: &Value) -> Vec<[f64; 2]> {
    v.elements().expect("points").iter().map(|p| { let p = p.numbers().expect("a point"); [p[0], p[1]] }).collect()
}

fn reference(v: Option<&Value>) -> Option<BendReference> {
    let v = v?;
    Some(BendReference { side: number(v, "side"), angle: number(v, "angle"), axis: number(v, "axis"), ratio: number(v, "ratio"), shape: v.get("shape").and_then(Value::numbers) })
}

fn near(a: f64, b: f64, what: &str) {
    assert!((a - b).abs() <= 1e-9 * (1.0 + b.abs()), "{what}: {a} != {b}");
}

fn same_reference(ours: Option<&BendReference>, theirs: Option<&BendReference>, what: &str) {
    assert_eq!(ours.is_some(), theirs.is_some(), "{what}: bend reference");
    let (Some(a), Some(b)) = (ours, theirs) else { return };
    near(a.side, b.side, what);
    near(a.angle, b.angle, what);
    near(a.axis, b.axis, what);
    near(a.ratio, b.ratio, what);
    assert_eq!(a.shape.as_ref().map(Vec::len), b.shape.as_ref().map(Vec::len), "{what}: shape");
    for (x, y) in a.shape.iter().flatten().zip(b.shape.iter().flatten()) {
        near(*x, *y, what);
    }
}

/// `solveIKPoints`, case by case: the same points, iteration count, error and
/// continuity reference as the JavaScript.
#[test]
fn point_solver_matches_the_recorded_javascript_results() {
    let vectors = json(&folder().join("vectors.json"));
    let cases = vectors.get("solve").and_then(Value::as_arr).expect("solve cases");
    assert!(cases.len() >= 20);
    for (index, case) in cases.iter().enumerate() {
        let what = format!("solve case {index}");
        let mut chain = points(case.get("points").unwrap());
        let lengths = case.get("lengths").and_then(Value::numbers).unwrap_or_default();
        let target = case.get("target").and_then(Value::numbers).unwrap();
        let solved = ik::solve_points(&mut chain, &lengths, [target[0], target[1]], number(case, "bendDirection"), reference(case.get("bendReference")).as_ref());
        let expected = case.get("result").unwrap();
        assert_eq!(solved.iterations as f64, number(expected, "iterations"), "{what}: iterations");
        near(solved.error, number(expected, "error"), &what);
        for (ours, theirs) in chain.iter().zip(points(expected.get("points").unwrap())) {
            near(ours[0], theirs[0], &what);
            near(ours[1], theirs[1], &what);
        }
        same_reference(solved.bend_reference.as_ref(), reference(expected.get("bendReference")).as_ref(), &what);
    }
}

/// `interpolateIK` without a channel sampler (linear, angles on the short arc).
#[test]
fn interpolation_matches_the_recorded_javascript_results() {
    let vectors = json(&folder().join("vectors.json"));
    let cases = vectors.get("interpolate").and_then(Value::as_arr).expect("interpolate cases");
    assert!(cases.len() >= 8);
    for (index, case) in cases.iter().enumerate() {
        let what = format!("interpolate case {index}");
        let (a, b) = (parse::table(case.get("a")).unwrap(), parse::table(case.get("b")).unwrap());
        let ours = ik::interpolate(&a, Some(&b), number(case, "t"), None);
        let theirs = parse::table(case.get("result")).unwrap();
        assert_eq!(ours.controllers.keys().collect::<Vec<_>>(), theirs.controllers.keys().collect::<Vec<_>>(), "{what}: handles");
        for (id, c) in &ours.controllers {
            let e = &theirs.controllers[id];
            let what = format!("{what}, handle {id}");
            assert_eq!((c.enabled, c.pinned, c.allow_stretch, c.chain_length), (e.enabled, e.pinned, e.allow_stretch, e.chain_length), "{what}");
            near(c.target.x, e.target.x, &what);
            near(c.target.y, e.target.y, &what);
            near(c.target.rotation, e.target.rotation, &what);
            assert_eq!(c.followers.keys().collect::<Vec<_>>(), e.followers.keys().collect::<Vec<_>>(), "{what}: followers");
            for (child, angle) in &c.followers {
                near(*angle, e.followers[child], &what);
            }
            assert_eq!(c.joined_followers.is_some(), e.joined_followers.is_some(), "{what}: joined followers");
            for (uid, children) in c.joined_followers.iter().flatten() {
                for (child, angle) in children {
                    near(*angle, e.joined_followers.as_ref().unwrap()[uid][child], &what);
                }
            }
            same_reference(c.bend_reference.as_ref(), e.bend_reference.as_ref(), &what);
            assert_eq!(c.spatial_followers.is_some(), e.spatial_followers.is_some(), "{what}: spatial followers");
            for (child, q) in c.spatial_followers.iter().flatten() {
                for (x, y) in q.iter().zip(e.spatial_followers.as_ref().unwrap()[child]) {
                    near(*x, y, &what);
                }
            }
            for (uid, children) in c.joined_spatial_followers.iter().flatten() {
                for (child, q) in children {
                    for (x, y) in q.iter().zip(e.joined_spatial_followers.as_ref().unwrap()[uid][child]) {
                        near(*x, y, &what);
                    }
                }
            }
            assert_eq!(c.spatial_target.is_some(), e.spatial_target.is_some(), "{what}: spatial target");
            if let (Some(s), Some(t)) = (&c.spatial_target, &e.spatial_target) {
                near(s.depth, t.depth, &what);
                near(s.swivel, t.swivel, &what);
                for (x, y) in s.pole.iter().zip(t.pole).chain(s.normals.iter().flatten().flatten().zip(t.normals.into_iter().flatten().flatten())) {
                    near(*x, y, &what);
                }
            }
        }
    }
}
