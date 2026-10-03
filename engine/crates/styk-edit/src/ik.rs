//! Inverse-kinematics controllers as editing sees them (src/ik.mjs, src/editor-ik.mjs).
//!
//! Ported: moving controller targets with their figure (`translateIKTargets`), and the
//! gates below. A gesture on a frame that would need a controller's solution or its
//! remembered bend answers `Unported`, so the JavaScript editor runs it.

use styk_doc::{Obj, Value};

use crate::frame::objects_of;
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
    pub(crate) fn ik_gate(&self, method: &str, args: &[Value]) -> Result<Option<String>, EditError> {
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
        // Posing a controller's endpoint moves its target and solves the limb.
        if method == "poseJoint" && !args.get(2).is_some_and(|o| truthy(o, "stretch")) && self.selection.joint >= 0 {
            let object = self.object()?.map(|(_, o)| o);
            let figure = object.as_ref().and_then(|o| {
                let figures = self.report.get("figure_types").and_then(Value::elements)?;
                figures.iter().find(|f| f.get("figure_type_id") == o.get("figure_type_id"))
            });
            let handle = figure.and_then(|f| f.get("segments").and_then(|s| s.item(self.selection.joint as usize))).and_then(|s| s.get("segment_id").cloned());
            if let (Some(object), Some(handle)) = (object, handle) {
                let key = styk_doc::json::to_string(&handle);
                if object.path(&["_stykIK", &key]).is_some_and(|c| truthy(c, "enabled")) {
                    return Ok(Some("posing an IK target".into()));
                }
            }
        }
        Ok(None)
    }
}
