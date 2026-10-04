//! Raw-record side of animation-origin gesture mutations (src/editor-origin-mutations.mjs).
//! Numeric tree/support arithmetic stays in styk_eval::origin; unknown record fields
//! survive synchronization and reflection, including ghost metadata.
use std::collections::HashMap;
use std::sync::Arc;
use styk_doc::{Obj, Value};
use crate::record::{num, uid};
use crate::{EditError, Editor};

pub(crate) fn recorded_call(method: &str) -> bool {
    posing_call(method) || property_call(method) || matches!(method, "select" | "@select" | "selectMany" | "seek" | "@seek" | "beginDrag" | "beginTranslationDrag" |
        "commitDrag" | "cancelDrag" | "moveSelection" | "rotateSelection" | "scaleSelection" | "scaleSelected" |
        "mirrorSelection" | "mirrorFigure" | "rotateFigure" | "centerFigure" | "resetPose" | "setOrigin" | "mirrorSegment" | "mirrorSegmentBranch" | "splitSegment" | "splitSegmentBranch" | "splitBuilderSegments" | "unsplitSegment" |
        "deleteSingleSegment" | "deleteSegment" | "addSegment" | "addSegments" | "moveBuilderSegment" | "duplicateSegment" | "duplicateSegmentBranch" | "replaceFigureDefinition" | "undo" | "redo")
}

pub(crate) fn property_call(method: &str) -> bool {
    matches!(method, "beginAnimationEdit" | "previewAnimationEdit" | "commitAnimationEdit" | "cancelAnimationEdit" | "addAnimationKey" | "addAnimationKeys")
}

pub(crate) fn recorded_channel_call(method: &str) -> bool {
    property_call(method) || matches!(method, "select" | "@select" | "selectMany" | "seek" | "@seek" | "beginDrag" | "beginTranslationDrag" | "commitDrag" | "cancelDrag" |
        "moveSelection" | "rotateSelection" | "scaleSelection" | "scaleSelected" | "mirrorSelection" | "mirrorFigure" | "rotateFigure" | "centerFigure" | "resetPose" |
        "poseJoint" | "rotateJoint" | "bendJoint" | "undo" | "redo")
}

pub(crate) fn posing_call(method: &str) -> bool {
    matches!(method, "poseJoint" | "rotateJoint" | "bendJoint" | "beginHandleEditDrag" | "editHandlePosition" | "commitHandleEditDrag" | "cancelHandleEditDrag" |
        "beginFigureHandleDrag" | "transformFigureHandle" | "commitFigureHandleDrag" | "cancelFigureHandleDrag")
}

fn same_pose(a: &Value, b: &Value) -> bool {
    if ["root_x", "root_y", "scale", "len_flag", "bend_flag", "angles", "lengths", "bends"].iter().any(|key| a.get(key) != b.get(key)) {
        return false;
    }
    let ghosts = |object: &Value| object.path(&["_stykAnimationOrigin", "ghosts"]).and_then(Value::as_obj).cloned().unwrap_or_default();
    let (a, b) = (ghosts(a), ghosts(b));
    a.len() == b.len() && a.iter().all(|(key, ghost)| b.get(key).is_some_and(|other|
        other.truthy() && ["angle_rad", "bend_rad", "length"].iter().all(|field| ghost.get(field) == other.get(field))))
}

fn support(figure: &Value) -> bool {
    figure.path(&["_styk", "animationOrigin", "support"]).is_some_and(Value::truthy)
}

pub(crate) fn rotate_support(figure: &Value, object: &mut Obj, delta: f64) {
    if delta == 0.0 || !support(figure) { return; }
    let Some(origin) = object.get("_stykAnimationOrigin").and_then(Value::as_obj) else { return };
    let Some(ghosts) = origin.get("ghosts").and_then(Value::as_obj) else { return };
    let mut out = ghosts.clone();
    for (key, pose) in ghosts.iter() {
        let mut pose = pose.as_obj().cloned().unwrap_or_default();
        pose.set("angle_rad", Value::Num(pose.get("angle_rad").and_then(Value::as_f64).unwrap_or(f64::NAN) + delta));
        out.set(key, Value::object(pose));
    }
    let mut origin = origin.clone(); origin.set("ghosts", Value::object(out));
    object.set("_stykAnimationOrigin", Value::object(origin));
}

pub(crate) fn mirror_support(figure: &Value, object: &mut Obj, horizontal: bool) {
    if !support(figure) { return; }
    let Some(origin) = object.get("_stykAnimationOrigin").and_then(Value::as_obj) else { return };
    let Some(ghosts) = origin.get("ghosts").and_then(Value::as_obj) else { return };
    let mut out = ghosts.clone();
    for (key, pose) in ghosts.iter() {
        let angle = num(pose, "angle_rad").unwrap_or(f64::NAN);
        let mut record = pose.as_obj().cloned().unwrap_or_default();
        record.set("angle_rad", Value::Num(if horizontal { std::f64::consts::PI - angle } else { -angle }));
        record.set("bend_rad", Value::Num(-num(pose, "bend_rad").unwrap_or(f64::NAN)));
        out.set(key, Value::object(record));
    }
    let mut origin = origin.clone(); origin.set("ghosts", Value::object(out));
    object.set("_stykAnimationOrigin", Value::object(origin));
}

pub(crate) fn reset_support(figure: &Value, object: &mut Obj) {
    let Some(bones) = figure.path(&["_styk", "animationOrigin", "support", "bones"]).and_then(Value::elements) else { return };
    let mut ghosts = Obj::new();
    for (index, bone) in bones.iter().enumerate().filter(|(_, bone)| bone.get("handleId").is_some_and(Value::is_null)) {
        let mut pose = Obj::new();
        for field in ["angle_rad", "bend_rad", "length"] { pose.set(field, bone.get(field).cloned().unwrap_or(Value::Null)); }
        ghosts.set(&index.to_string(), Value::object(pose));
    }
    if !ghosts.is_empty() {
        let mut origin = object.get("_stykAnimationOrigin").and_then(Value::as_obj).cloned().unwrap_or_default();
        origin.set("ghosts", Value::object(ghosts)); object.set("_stykAnimationOrigin", Value::object(origin));
    }
}

impl Editor {
    /// Only the ordinary 2D and retained-support definitions in the posing
    /// recordings are admitted. Reuse the gesture owner's affected slots.
    pub(crate) fn origin_pose_gate(&self) -> Result<(), EditError> {
        self.origin_plain_targets(&self.selection.uids)
    }

    /// The recorded sheet property subset, validated before a property begin
    /// could commit a live gesture. Other targets/properties stay with the editor.
    pub(crate) fn origin_property_gate(&self, method: &str, args: &[Value]) -> Result<(), EditError> {
        let take = |target: Option<&Value>, property: Option<&Value>, tick: Option<&Value>| -> Result<(), EditError> {
            if target.and_then(|target| target.get("kind")).and_then(Value::as_str) != Some("figure") ||
                !property.and_then(Value::as_str).is_some_and(|property| ["x", "y", "scale", "rotation"].contains(&property)) {
                return Err(EditError::Unported("an unrecorded origin property target or property".into()));
            }
            let target = crate::property_edit::normalize_target(target.unwrap()).map_err(|_| EditError::Unported("an unrecorded origin property target".into()))?;
            let uid = num(&target, "uid").unwrap() as u32;
            let fallback = Value::Num(self.tick);
            let at = tick.filter(|tick| !tick.is_null()).unwrap_or(&fallback);
            // Keys use the sheet owner's tick check; a begin reads the current property view.
            let at = if method == "beginAnimationEdit" { at.clone() } else { Value::Num(crate::property_edit::checked_tick(at)?) };
            let frame = self.animation_frame_at(&at)?;
            let mut objects = crate::frame::objects_of(&frame);
            let has_target = |objects: &[Value]| objects.iter().any(|object| crate::record::uid(object) == Some(uid));
            if !has_target(&objects) {
                // Recorded explicit keys past an exposure's end write an empty cell.
                // Admit that no-target path only for a proven ordinary target on show.
                objects = if method == "beginAnimationEdit" { Vec::new() } else { self.selection_frame()? };
                if !has_target(&objects) {
                    return Err(EditError::Unported("an absent origin property target at the edited tick".into()));
                }
            }
            self.origin_plain_targets_in(&objects, &[uid], matches!(property.and_then(Value::as_str), Some("x" | "y")))
        };
        match method {
            "beginAnimationEdit" => take(args.first(), args.get(1), None),
            "addAnimationKey" => take(args.first(), args.get(1), args.get(2)),
            "addAnimationKeys" => {
                if let Some(entries) = args.first().and_then(Value::elements) {
                    for entry in entries.iter() { take(entry.get("target"), entry.get("property"), entry.get("tick"))?; }
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn origin_plain_targets(&self, uids: &[u32]) -> Result<(), EditError> {
        self.origin_plain_targets_in(&self.selection_frame()?, uids, false)
    }

    fn origin_plain_targets_in(&self, objects: &[Value], uids: &[u32], ancestors: bool) -> Result<(), EditError> {
        let slots = Self::ik_pose_slots(objects, crate::select::resolve_join_selection(objects, uids, ancestors)?);
        for slot in slots {
            let object = &objects[slot];
            let figure = self.report.get("figure_types").and_then(Value::elements).and_then(|figures| figures.iter().find(|figure| num(figure, "figure_type_id") == num(object, "figure_type_id")));
            let complex = figure.is_some_and(|figure|
                ["poseChoices", "spatialMesh", "movieclip", "drawing"].iter().any(|key| figure.path(&["_styk", key]).is_some_and(Value::truthy)) ||
                figure.get("segments").and_then(Value::elements).is_some_and(|segments| segments.iter().any(|segment|
                    ["_stykRig", "_stykPoseChoice"].iter().any(|key| segment.get(key).is_some_and(Value::truthy))))) ||
                object.get("_stykSpatial").is_some_and(Value::truthy) ||
                object.get("_stykIK").and_then(Value::as_obj).is_some_and(|controllers| controllers.iter().any(|(_, controller)| controller.get("spatialTarget").is_some_and(Value::truthy)));
            if complex { return Err(EditError::Unported("origin posing with an unrecorded rig, pose choice, 3D, movieclip or drawing definition".into())); }
        }
        Ok(())
    }

    /// Decode the existing typed origin descriptor before a command can commit or
    /// index its raw support. The loader's figure cache remains the single owner.
    pub(crate) fn origin_figure(&self, figure: &Value) -> Result<Arc<styk_eval::doc::Figure>, EditError> {
        let mut definition = Obj::new();
        definition.set("version", self.report.get("version").cloned().unwrap_or(Value::Num(5.0)));
        definition.set("figure_types", Value::array(vec![figure.clone()]));
        let doc = self.eval.borrow_mut().loader.document(&Value::object(definition), "").map_err(|e| EditError::Invalid(e.to_string()))?;
        doc.figures.first().filter(|f| f.origin.is_some()).cloned()
            .ok_or_else(|| EditError::Unported("a malformed animation origin".into()))
    }

    /// Decode only an origin-bearing pose, reusing the typed loader's figure cache.
    /// A tiny definition view avoids converting the whole document on a translation.
    pub(crate) fn origin_view(&self, figure: &Value, object: &Value) -> Result<(Arc<styk_eval::doc::Figure>, Arc<styk_eval::doc::Object>), EditError> {
        let figure = self.origin_figure(figure)?;
        let mut frame = Obj::new(); frame.set("objects", Value::array(vec![object.clone()]));
        let pose = self.eval.borrow_mut().loader.keyframe(&Value::object(frame)).map_err(|e| EditError::Invalid(e.to_string()))?.objects.into_iter().next()
            .filter(|pose| pose.figure == figure.id)
            .ok_or_else(|| EditError::Unported("an animation origin pose without an identity or figure".into()))?;
        Ok((figure, pose))
    }

    pub(crate) fn origin_rotation(&self, figure: &Value, object: &Value) -> Result<f64, EditError> {
        if !figure.path(&["_styk", "animationOrigin"]).is_some_and(Value::truthy) || !object.get("_stykAnimationOrigin").is_some_and(Value::truthy) { return Ok(0.0); }
        let (figure, pose) = self.origin_view(figure, object)?;
        Ok(styk_eval::origin::rotation(&figure, &pose))
    }

    pub(crate) fn reflect_origin_offset(&self, figure: &Value, object: &mut Obj, horizontal: bool, previous: f64) -> Result<(), EditError> {
        let Some(origin) = object.get("_stykAnimationOrigin").and_then(Value::as_obj) else { return Ok(()) };
        if !figure.path(&["_styk", "animationOrigin"]).is_some_and(Value::truthy) { return Ok(()); }
        let (x, y) = (origin.get("offsetX").and_then(Value::as_f64).unwrap_or(0.0), origin.get("offsetY").and_then(Value::as_f64).unwrap_or(0.0));
        if x == 0.0 && y == 0.0 { return Ok(()); }
        let (c, s) = (styk_math::cos(previous), styk_math::sin(previous));
        let world_x = (if horizontal { -1.0 } else { 1.0 }) * (x * c - y * s);
        let world_y = (if horizontal { 1.0 } else { -1.0 }) * (x * s + y * c);
        let turn = self.origin_rotation(figure, &Value::object(object.clone()))?;
        let (c, s) = (styk_math::cos(turn), styk_math::sin(turn));
        let mut origin = origin.clone();
        origin.set("offsetX", Value::Num(world_x * c + world_y * s));
        origin.set("offsetY", Value::Num(-world_x * s + world_y * c));
        object.set("_stykAnimationOrigin", Value::object(origin)); Ok(())
    }

    /// `touch` and both commit paths synchronize only the gesture's captured poses.
    /// Pointer return preserves the exact original pivot metadata instead of rounding it again.
    pub(crate) fn synchronize_origins(&mut self, before: &[Value]) -> Result<(), EditError> {
        let figures: HashMap<u32, Value> = self.report.get("figure_types").and_then(Value::elements).map(|items| items.iter()
            .filter(|figure| figure.path(&["_styk", "animationOrigin"]).is_some_and(Value::truthy))
            .filter_map(|figure| Some((num(&figure, "figure_type_id")? as u32, figure))).collect()).unwrap_or_default();
        if figures.is_empty() { return Ok(()); }
        let previous: HashMap<u32, &Value> = before.iter().filter_map(|object| Some((uid(object)?, object))).collect();
        for (slot, object) in self.frame_objects().iter().enumerate() {
            let Some(figure) = num(object, "figure_type_id").and_then(|id| figures.get(&(id as u32))) else { continue };
            let Some(origin) = object.get("_stykAnimationOrigin").and_then(Value::as_obj) else { continue };
            let old = uid(object).and_then(|id| previous.get(&id)).copied();
            if !before.is_empty() && old.is_none() { continue; }
            let updated = if let Some(old) = old.filter(|old| same_pose(old, object)) {
                if ["x", "y", "offsetX", "offsetY"].iter().all(|field| old.path(&["_stykAnimationOrigin", field]) == origin.get(field)) { continue; }
                let Some(origin) = old.get("_stykAnimationOrigin").cloned() else { continue }; origin
            } else {
                let (typed, pose) = self.origin_view(figure, object)?;
                let offset = styk_eval::origin::offset(&typed, &pose);
                let mut origin = origin.clone();
                origin.set("x", Value::Num(num(object, "root_x").unwrap_or(f64::NAN) + offset[0]));
                origin.set("y", Value::Num(num(object, "root_y").unwrap_or(f64::NAN) + offset[1]));
                Value::object(origin)
            };
            self.frame_object_mut(slot).unwrap().set("_stykAnimationOrigin", updated);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn future_or_absent_property_refusal_preserves_the_shown_frame() {
        let plain = include_bytes!("../../../tests/edit/origin-future-rig-sheet.document.json").as_slice();
        let joined = include_bytes!("../../../tests/edit/origin-future-rig-joined-sheet.document.json").as_slice();
        let changing = include_bytes!("../../../tests/edit/origin-changing-rig-sheet.document.json").as_slice();
        for (document, id, property, begin) in [(plain, 3.0, "rotation", true), (plain, 77.0, "rotation", true), (joined, 2.0, "x", false), (changing, 3.0, "rotation", false)] {
            let report = styk_doc::json::parse(document).unwrap();
            let mut editor = Editor::new(report, crate::Options { property_channels: true, layer_timeline: true, auto_key: false });
            editor.call("select", &[Value::Num(1.0), Value::Num(-1.0)]).unwrap();
            editor.call("beginTranslationDrag", &[]).unwrap();
            editor.call("moveSelection", &[Value::Num(7.25), Value::Num(-3.5)]).unwrap();
            editor.take_events();
            let snapshot = |editor: &Editor| (editor.report_json(), editor.state(), editor.drag_value(), styk_doc::json::to_string(&styk_eval::records::snapshot_pose_frame(&editor.shown_frame().unwrap())));
            let before = snapshot(&editor);
            let mut target = Obj::new();target.set("kind", Value::str("figure"));target.set("uid", Value::Num(id));let target = Value::object(target);
            let mut entry = Obj::new();entry.set("target", target.clone());entry.set("property", Value::str(property));entry.set("tick", Value::Num(8.0));entry.set("value", Value::Num(0.47));
            let mut calls = vec![("addAnimationKey", vec![target.clone(), Value::str(property), Value::Num(8.0), Value::Num(0.47)]), ("addAnimationKeys", vec![Value::array(vec![Value::object(entry)])])];
            if begin { calls.push(("beginAnimationEdit", vec![target, Value::str(property)])); }
            for (method, args) in calls {
                assert!(matches!(editor.call(method, &args), Err(EditError::Unported(_))), "{id} {property} {method}");
                assert_eq!(snapshot(&editor), before, "{id} {property} {method} changed the shown frame or editor on refusal");
                assert!(editor.take_events().is_empty(), "{id} {property} {method} emitted on refusal");
            }
        }
    }

    #[test]
    fn invalid_future_join_guard_is_atomic_and_never_panics() {
        use std::panic::{catch_unwind, AssertUnwindSafe};
        for cyclic in [true, false] {
            let mut report = styk_doc::json::parse(include_bytes!("../../../tests/edit/origin-future-rig-joined-sheet.document.json")).unwrap();
            let invalidate = |frame: &mut Value| {
                let objects = frame.make_obj().unwrap().get_mut("objects").unwrap().make_arr().unwrap();
                let Some(slot) = objects.iter().position(|object| uid(object) == Some(3)) else { return };
                let parent = objects.iter().position(|object| uid(object) == Some(2)).unwrap();
                let mut join = Obj::new();join.set("target_object_slot", Value::Num(if cyclic { parent as f64 } else { 999.0 }));join.set("target_unique_id", Value::Num(2.0));join.set("target_handle_id", Value::Num(22.0));
                objects[slot].make_obj().unwrap().set("join", Value::object(join));
            };
            for frame in report.make_obj().unwrap().get_mut("keyframes").unwrap().make_arr().unwrap() { invalidate(frame); }
            let sources = report.make_obj().unwrap().get_mut("_styk").unwrap().make_obj().unwrap().get_mut("frameSources").unwrap().make_obj().unwrap();
            let names: Vec<_> = sources.keys().map(|key| key.to_string()).collect();for name in names { invalidate(sources.get_mut(&name).unwrap()); }
            let mut editor = Editor::new(report, crate::Options { property_channels: true, layer_timeline: true, auto_key: false });
            editor.call("select", &[Value::Num(1.0), Value::Num(-1.0)]).unwrap();editor.call("beginTranslationDrag", &[]).unwrap();editor.call("moveSelection", &[Value::Num(7.25), Value::Num(-3.5)]).unwrap();editor.take_events();
            let snapshot = |editor: &Editor| (editor.report_json(), editor.state(), editor.drag_value(), styk_doc::json::to_string(&styk_eval::records::snapshot_pose_frame(&editor.shown_frame().unwrap())));
            let before = snapshot(&editor);
            let mut target = Obj::new();target.set("kind", Value::str("figure"));target.set("uid", Value::Num(2.0));let target = Value::object(target);
            let mut entry = Obj::new();entry.set("target", target.clone());entry.set("property", Value::str("x"));entry.set("tick", Value::Num(8.0));entry.set("value", Value::Num(410.75));
            for (method, args) in [("addAnimationKey", vec![target, Value::str("x"), Value::Num(8.0), Value::Num(410.75)]), ("addAnimationKeys", vec![Value::array(vec![Value::object(entry)])])] {
                let result = catch_unwind(AssertUnwindSafe(|| editor.call(method, &args))).expect("invalid future join guard panicked");
                assert!(matches!(result, Err(EditError::Unported(_) | EditError::Invalid(_))), "{cyclic} {method} admitted an invalid future join");
                assert_eq!(snapshot(&editor), before, "{cyclic} {method} changed the shown frame or editor on refusal");assert!(editor.take_events().is_empty());
            }
        }
    }

}
