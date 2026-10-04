//! Raw-record side of animation-origin gesture mutations (src/editor-origin-mutations.mjs).
//! Numeric tree/support arithmetic stays in styk_eval::origin; unknown record fields
//! survive synchronization and reflection, including ghost metadata.
use std::collections::HashMap;
use std::sync::Arc;
use styk_doc::{Obj, Value};
use crate::record::{num, uid};
use crate::{EditError, Editor};

pub(crate) fn recorded_call(method: &str) -> bool {
    matches!(method, "select" | "@select" | "selectMany" | "seek" | "@seek" | "beginDrag" | "beginTranslationDrag" |
        "commitDrag" | "cancelDrag" | "moveSelection" | "rotateSelection" | "scaleSelection" | "scaleSelected" |
        "mirrorSelection" | "mirrorFigure" | "rotateFigure" | "centerFigure" | "resetPose" | "setOrigin" | "mirrorSegment" | "mirrorSegmentBranch" | "splitSegment" | "splitSegmentBranch" | "splitBuilderSegments" | "unsplitSegment" |
        "deleteSingleSegment" | "deleteSegment" | "addSegment" | "addSegments" | "moveBuilderSegment" | "duplicateSegment" | "duplicateSegmentBranch" | "replaceFigureDefinition" | "undo" | "redo")
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
