//! Inverse-kinematics controllers as editing sees them (src/ik.mjs, src/editor-ik.mjs).
//!
//! Ported: moving controller targets with their figure (`translateIKTargets`), posing
//! the end of a limb whose controller is not pinned (`poseIKTarget`), and the gates
//! below. A gesture on a frame that would need a pinned controller's solution or its
//! remembered bend answers `Unported`, so the JavaScript editor runs it.

use std::collections::BTreeMap;

use styk_doc::{Obj, Value};
use styk_eval::ik::{self, BendReference, Controller, Pose, Skeleton, Table, Target};
use styk_math::hypot;

use crate::frame::objects_of;
use crate::record::uid;
use crate::{EditError, Editor};

fn controllers(object: &Value) -> Vec<(std::sync::Arc<str>, Value)> {
    object.get("_stykIK").and_then(Value::as_obj).map(|table| table.iter().map(|(id, c)| (id.clone(), c.clone())).collect()).unwrap_or_default()
}

fn truthy(value: &Value, key: &str) -> bool {
    value.get(key).is_some_and(Value::truthy)
}

/// `translateIKTargets(object, dx, dy, {source, pinnedOnly})`: the targets of `object`'s
/// controllers (only the enabled, pinned ones with `pinned_only`) moved to `source`'s
/// target plus (dx, dy). Returns whether a target changed.
pub(crate) fn translate_targets(object: &mut Obj, dx: f64, dy: f64, source: &Value, pinned_only: bool) -> bool {
    let Some(table) = object.get("_stykIK").filter(|t| t.truthy()).and_then(Value::as_obj).cloned() else { return false };
    let mut next: Option<Obj> = None;
    for (id, c) in table.iter() {
        if pinned_only && !(truthy(c, "enabled") && truthy(c, "pinned")) {
            continue;
        }
        let Some(target) = source.path(&["_stykIK", id, "target"]).filter(|t| t.truthy()) else { continue };
        let number = |value: &Value, key: &str| value.get(key).and_then(Value::as_f64).unwrap_or(f64::NAN);
        let (x, y) = (number(target, "x") + dx, number(target, "y") + dy);
        let current = c.get("target").cloned().unwrap_or(Value::Null);
        if x == number(&current, "x") && y == number(&current, "y") {
            continue;
        }
        let mut moved = current.as_obj().cloned().unwrap_or_default();
        moved.set("x", Value::Num(x));
        moved.set("y", Value::Num(y));
        let mut controller = c.as_obj().cloned().unwrap_or_default();
        controller.set("target", Value::object(moved));
        next.get_or_insert_with(|| table.clone()).set(id, Value::object(controller));
    }
    match next {
        Some(next) => {
            object.set("_stykIK", Value::object(next));
            true
        }
        None => false,
    }
}

/// Calls that change poses on the frame on show (`_objectEdit` and the gestures).
const GESTURES: &[&str] = &[
    "beginDrag", "beginTranslationDrag", "moveSelection", "rotateSelection", "scaleSelection", "scaleSelected", "mirrorSelection", "rotateFigure", "centerFigure", "resetPose",
    "poseJoint", "rotateJoint", "bendJoint",
];
/// Calls that only read or restore: selection, the playhead, history.
const VIEWS: &[&str] = &["select", "@select", "selectMany", "seek", "@seek", "undo", "redo", "cancelDrag"];

impl Editor {
    /// Why `method` cannot run here because of IK controllers on the frame on show.
    pub(crate) fn ik_gate(&self, method: &str) -> Result<Option<String>, EditError> {
        if VIEWS.contains(&method) {
            return Ok(None);
        }
        let objects = objects_of(&self.shown_frame()?);
        let all: Vec<(std::sync::Arc<str>, Value)> = objects.iter().flat_map(controllers).collect();
        if all.is_empty() {
            return Ok(None);
        }
        if all.iter().any(|(_, c)| truthy(c, "spatialTarget")) {
            return Ok(Some("with 3D IK".into()));
        }
        let enabled = all.iter().any(|(_, c)| truthy(c, "enabled"));
        if enabled && objects.iter().any(|o| o.get("join").is_some_and(Value::truthy)) {
            return Ok(Some("with IK on joined figures".into()));
        }
        if !GESTURES.contains(&method) {
            return Ok(None);
        }
        if all.iter().any(|(_, c)| truthy(c, "enabled") && truthy(c, "pinned")) {
            return Ok(Some("with pinned IK".into()));
        }
        // A target is placed in the scene before the figure's perspective projects it.
        if method == "poseJoint"
            && let Some((_, object)) = self.object()?
            && object.get("_stykIK").is_some()
            && object.path(&["_stykSpatial", "camera", "strength"]).is_some_and(Value::truthy)
        {
            return Ok(Some("posing an IK target in perspective".into()));
        }
        Ok(None)
    }

    /// `poseIKTarget(x, y)` for the enabled, unpinned controller ending at `handle` of the
    /// figure in `slot`: the pointer becomes its target and the limb is solved to it.
    pub(crate) fn pose_ik_target(&mut self, slot: usize, handle: u32, x: f64, y: f64) -> Result<bool, EditError> {
        self.object_edit("Move IK target", |editor| editor.bake_unpinned(slot, handle, x, y))
    }

    /// The unpinned branch of `poseIKTarget`: `capture` the limb as drawn, solve it with
    /// the captured controller pinned at the pointer (`solveObjectIK`), keep the solved
    /// angles (and stretched lengths) in the pose, and remember the followers and the
    /// bend in the controller. Unpinned IK is a posing tool: the target does not persist.
    fn bake_unpinned(&mut self, slot: usize, handle: u32, x: f64, y: f64) -> Result<(), EditError> {
        let unported = |what: &str| EditError::Unported(format!("posing an IK target {what}"));
        let (prepared, frame, scene) = self.evaluate()?;
        let object = self.frame_objects().get(slot).cloned().ok_or_else(|| unported("outside the frame"))?;
        let id = uid(&object).ok_or_else(|| unported("of a figure without an identity"))?;
        let evaluated = frame.objects.iter().find(|o| o.uid == id).ok_or_else(|| unported("of a figure the engine does not draw"))?;
        let geometry = scene.objects.iter().find(|g| g.uid == id && g.resolved).ok_or_else(|| unported("of a figure the engine does not draw"))?;
        let figure = evaluated.figure_override.as_deref().or_else(|| prepared.doc.figure(evaluated.figure).map(|f| &**f)).ok_or_else(|| unported("of a figure without a definition"))?;
        let key = handle.to_string();
        let current = object.path(&["_stykIK", &key]).cloned().unwrap_or(Value::Null);
        let controller = ik::parse::table(object.get("_stykIK")).and_then(|table| table.get(handle).cloned()).ok_or_else(|| unported("with a controller the engine cannot read"))?;
        let skeleton = Skeleton { segments: &figure.segments, plan: &figure.plan, polygons: &figure.polyfills, sources: None };
        let angles = object.get("angles").and_then(Value::numbers).filter(|a| a.len() >= figure.segments.len()).ok_or_else(|| unported("of a pose without every angle"))?;
        let drawn = |i: usize| styk_eval::geometry::segment_pose(evaluated, &figure.segments[i], i).0;

        // capture(figure, object, geometry, handle, controller)
        let rotation = controller.target.rotation;
        let followers: BTreeMap<u32, f64> = skeleton.follower_indices(handle).into_iter().map(|i| (figure.segments[i].id, drawn(i) - rotation)).collect();
        let chain = skeleton.chain(handle, controller.chain_length);
        let mut bend_direction = 1.0;
        if chain.len() >= 2 {
            let (a, c) = (geometry.starts[chain[0]], geometry.ends[chain[chain.len() - 1]]);
            let mut cross = 0.0f64;
            for &i in &chain[..chain.len() - 1] {
                let b = geometry.ends[i];
                let value = (c[0] - a[0]) * (b[1] - a[1]) - (c[1] - a[1]) * (b[0] - a[0]);
                if value.abs() > cross.abs() {
                    cross = value;
                }
            }
            if cross.abs() > 1e-8 {
                bend_direction = if cross < 0.0 { -1.0 } else { 1.0 };
            }
        }
        if skeleton.index(handle).is_none() || geometry.joint(figure, handle).is_none() {
            return Err(EditError::Invalid("errors.editorIk.selectAValidIkNode".into()));
        }
        if chain.is_empty() {
            return Err(unported("without a chain"));
        }
        let points: Vec<[f64; 2]> = std::iter::once(geometry.starts[chain[0]]).chain(chain.iter().map(|&i| geometry.ends[i])).collect();
        let lengths: Vec<f64> = chain.iter().map(|&i| hypot(geometry.ends[i][0] - geometry.starts[i][0], geometry.ends[i][1] - geometry.starts[i][1])).collect();
        let captured = ik::bend_reference(&points, &lengths, controller.bend_reference.as_ref());
        // `bendDirection: controller.bendDirection ?? bendDirection`
        let bend_direction = if current.get("bendDirection").is_some_and(|v| !v.is_null()) { controller.bend_direction } else { bend_direction };

        let temporary = Controller {
            pinned: true,
            target: Target { x, y, rotation },
            followers: followers.clone(),
            bend_direction,
            bend_reference: Some(captured.clone()),
            ..controller
        };
        let baseline = self.drag.as_ref().and_then(|drag| drag.before.iter().find(|o| uid(o) == Some(id)).cloned()).ok_or_else(|| unported("outside a gesture"))?;
        let length_flag = baseline.get("len_flag").cloned();
        let rest = if length_flag.as_ref().is_some_and(Value::truthy) {
            Some(baseline.get("lengths").and_then(Value::numbers).ok_or_else(|| unported("of a pose with unreadable lengths"))?)
        } else {
            None
        };
        let bends = if object.get("bend_flag").is_some_and(Value::truthy) { object.get("bends").and_then(Value::numbers) } else { None };
        let pose = Pose {
            scale: object.get("scale").and_then(Value::as_f64).unwrap_or(f64::NAN),
            angles: &angles,
            lengths: rest.as_deref(),
            bends: bends.as_deref(),
            mirror: object.get("image_text_flags").is_some_and(Value::truthy),
            figure_angle: ik::figure_angle(figure, evaluated),
        };
        let table = Table { controllers: BTreeMap::from([(handle, temporary)]) };
        let solution = ik::solve_object(&skeleton, &pose, geometry.root, &table).ok_or_else(|| unported("the solver did not reach"))?;
        let solved = solution.results.iter().find(|(h, _)| *h == handle).and_then(|(_, outcome)| outcome.bend_reference.clone());

        let mut next = current.as_obj().cloned().unwrap_or_default();
        let mut target = current.get("target").and_then(Value::as_obj).cloned().unwrap_or_default();
        target.set("x", Value::Num(x));
        target.set("y", Value::Num(y));
        next.set("target", Value::object(target));
        let mut table = Obj::new();
        for (segment, angle) in &followers {
            table.set(&segment.to_string(), Value::Num(*angle));
        }
        next.set("followers", Value::object(table));
        next.set("bendReference", reference_value(solved.as_ref().unwrap_or(&captured)));
        let mut controllers = object.get("_stykIK").and_then(Value::as_obj).cloned().unwrap_or_default();
        controllers.set(&key, Value::object(next));

        let record = self.frame_object_mut(slot).ok_or_else(|| unported("outside the frame"))?;
        record.set("angles", Value::nums(&solution.angles));
        match (&solution.lengths, length_flag) {
            (Some(_), _) => record.set("len_flag", Value::Num(1.0)),
            (None, Some(flag)) => record.set("len_flag", flag),
            (None, None) => {
                record.remove("len_flag");
            }
        }
        match (&solution.lengths, baseline.get("lengths")) {
            (Some(lengths), _) => record.set("lengths", Value::nums(lengths)),
            (None, Some(lengths)) => record.set("lengths", lengths.clone()),
            (None, None) => {
                record.remove("lengths");
            }
        }
        record.set("_stykIK", Value::object(controllers));
        record.remove("runtime_segments");
        Ok(())
    }
}

/// `ikBendReference`'s result as the editor stores it in a controller.
fn reference_value(reference: &BendReference) -> Value {
    let mut value = Obj::new();
    value.set("side", Value::Num(reference.side));
    value.set("angle", Value::Num(reference.angle));
    value.set("axis", Value::Num(reference.axis));
    value.set("ratio", Value::Num(reference.ratio));
    if let Some(shape) = &reference.shape {
        value.set("shape", Value::nums(shape));
    }
    Value::object(value)
}
