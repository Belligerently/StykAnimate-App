//! Posing joints (`poseJoint`, `rotateJoint`, `bendJoint`, `_rotateJoint`,
//! `_poseSnapState`) and the rotation targets of a joint (`rotationEdits` in
//! src/pivot/structure.mjs). Posing the end of a limb with an IK controller goes to
//! [`crate::ik`]. Rig controls, pinned inverse kinematics, 3D bones and Stick Nodes
//! curves are not ported: those calls answer `Unported`.

use std::collections::{HashMap, HashSet};

use styk_doc::{Obj, Value};
use styk_math::{atan2, hypot, shortest, sin, cos};

use crate::record::{join_target, num, uid};
use crate::transform::{field, finite};
use crate::{EditError, Editor};

/// A segment as the geometry drew it (`geometry.objects.get(slot).segments[joint]`).
#[derive(Clone, Copy, Debug)]
struct Shape {
    start: [f64; 2],
    angle: f64,
    bend: f64,
}

/// `_poseSnapState`: the definition's length and bend, and whether the gesture
/// started away from them (then the pointer snaps back to them).
struct SnapState {
    length: f64,
    bend: f64,
    length_enabled: bool,
    bend_enabled: bool,
}

/// The selected figure's slot and record, its definition's segments and the selected joint.
struct Joint {
    slot: usize,
    object: Value,
    segments: Vec<Value>,
    joint: usize,
}

/// `poseJoint`'s and `bendJoint`'s options.
struct PoseOptions {
    stretch: bool,
    isolate: bool,
    snap: bool,
    snap_tolerance: f64,
}

impl PoseOptions {
    fn from_value(options: &Value) -> PoseOptions {
        let flag = |key| options.get(key).is_some_and(Value::truthy);
        PoseOptions {
            stretch: flag("stretch"),
            isolate: flag("isolate"),
            // Destructuring defaults apply to absent options only: `snap: null` is off.
            snap: options.get("snap").is_none_or(Value::truthy),
            snap_tolerance: options.get("snapTolerance").map_or(6.0, |v| v.as_f64().unwrap_or(f64::NAN)),
        }
    }
}

/// `isStatic(segment)` on a figure record.
fn is_static(segment: &Value) -> bool {
    if let Some(aux) = segment.get("aux_code").filter(|v| !v.is_null()) {
        return (aux.as_f64().unwrap_or(0.0) as i64) & 1 != 0;
    }
    if let Some(raw) = segment.get("static_raw").filter(|v| !v.is_null()) {
        return raw.truthy();
    }
    if let Some(flag) = segment.path(&["flag_bits", "is_static"]).filter(|v| !v.is_null()) {
        return flag.truthy();
    }
    if let Some(flag) = segment.get("is_static").filter(|v| !v.is_null()) {
        return flag.truthy();
    }
    (segment.get("flags").and_then(Value::as_f64).unwrap_or(0.0) as i64) & 256 != 0
}

/// `staticGroup` in `rotationEdits`: the whole auxiliary byte when there is one.
fn static_group(segment: &Value) -> bool {
    match segment.get("aux_code").filter(|v| !v.is_null()) {
        Some(aux) => aux.as_f64().unwrap_or(0.0) != 0.0,
        None => is_static(segment),
    }
}

fn segments_of(figure: &Value) -> Vec<Value> {
    figure.get("segments").and_then(Value::elements).map(|e| e.iter().collect()).unwrap_or_default()
}

fn id_of(segment: &Value, key: &str) -> f64 {
    segment.get(key).and_then(Value::as_f64).unwrap_or(f64::NAN)
}

/// `rotationEdits(report, index, slot, joint, …, {isolate})`: the (slot, segment)
/// angles a rotation of `joint` turns: its own subtree, then every figure joined to a
/// handle of what turns, whole.
fn rotation_targets(objects: &[Value], figures: &[Value], slot: usize, joint: usize, isolate: bool) -> Vec<(usize, usize)> {
    let figure_of = |object: &Value| figures.iter().find(|f| num(f, "figure_type_id") == num(object, "figure_type_id")).cloned().unwrap_or(Value::Null);
    let segments = segments_of(&figure_of(&objects[slot]));
    let mut handles: Vec<f64> = vec![id_of(&segments[joint], "segment_id")];
    let mut children: HashMap<u64, Vec<f64>> = HashMap::new();
    for s in &segments {
        children.entry(id_of(s, "parent_id").to_bits()).or_default().push(id_of(s, "segment_id"));
    }
    let by_id = |id: f64| segments.iter().find(|s| id_of(s, "segment_id").to_bits() == id.to_bits());
    let mut q = 0;
    while q < handles.len() {
        for &id in children.get(&handles[q].to_bits()).map(Vec::as_slice).unwrap_or(&[]) {
            if !handles.iter().any(|h| h.to_bits() == id.to_bits()) && (!isolate || by_id(id).is_some_and(static_group)) {
                handles.push(id);
            }
        }
        q += 1;
    }
    let mut targets: Vec<(usize, usize)> = segments.iter().enumerate().filter(|(_, s)| handles.iter().any(|h| h.to_bits() == id_of(s, "segment_id").to_bits())).map(|(i, _)| (slot, i)).collect();
    let mut joined: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, child) in objects.iter().enumerate() {
        if let Some(target) = join_target(child) {
            joined.entry(target).or_default().push(i);
        }
    }
    let mut queue: Vec<(usize, Vec<f64>, bool)> = vec![(slot, handles, isolate)];
    let mut visited: HashSet<usize> = HashSet::from([slot]);
    let mut q = 0;
    while q < queue.len() {
        let (parent, parent_handles, boundary) = queue[q].clone();
        for &i in joined.get(&parent).map(Vec::as_slice).unwrap_or(&[]) {
            let child = &objects[i];
            let handle = child.path(&["join", "target_handle_id"]).and_then(Value::as_f64).unwrap_or(f64::NAN);
            if visited.contains(&i) || join_target(child) != Some(parent) || !parent_handles.contains(&handle) {
                continue;
            }
            let child_segments = segments_of(&figure_of(child));
            if boundary && !child_segments.iter().filter(|s| id_of(s, "parent_id") == 0.0).all(static_group) {
                continue;
            }
            visited.insert(i);
            let count = child.get("angles").and_then(Value::array_len).unwrap_or(0);
            targets.extend((0..count).map(|j| (i, j)));
            let mut next = vec![0.0];
            next.extend(child_segments.iter().map(|s| id_of(s, "segment_id")));
            queue.push((i, next, false));
        }
        q += 1;
    }
    targets
}

/// `angles[index] += delta` on an object record.
fn add_angle(object: &mut Obj, index: usize, delta: f64) {
    let Some(angles) = object.get_mut("angles") else { return };
    let Some(list) = angles.make_arr() else { return };
    if let Some(angle) = list.get_mut(index) {
        *angle = Value::Num(angle.as_f64().unwrap_or(f64::NAN) + delta);
    }
    angles.repack();
}

/// `object[key][index] = value` on an array of numbers.
fn set_item(object: &mut Obj, key: &str, index: usize, value: f64) {
    let Some(array) = object.get_mut(key) else { return };
    let Some(list) = array.make_arr() else { return };
    if index >= list.len() {
        // JavaScript fills the gap with holes, which JSON writes as null.
        list.resize(index + 1, Value::Null);
    }
    list[index] = Value::Num(value);
    array.repack();
}

impl Editor {
    /// The selected figure's record and definition, and the selected segment, when a
    /// joint gesture can run here; `Unported` for the parts it cannot run yet.
    fn joint_context(&self) -> Result<Option<Joint>, EditError> {
        let Some((slot, object)) = self.object()? else { return Ok(None) };
        let figures: Vec<Value> = self.report.get("figure_types").and_then(Value::elements).map(|e| e.iter().collect()).unwrap_or_default();
        let figure = figures.iter().find(|f| num(f, "figure_type_id") == num(&object, "figure_type_id")).cloned().unwrap_or(Value::Null);
        let segments = segments_of(&figure);
        if segments.iter().any(|s| s.get("_stykRig").is_some()) {
            return Err(EditError::Unported("posing a rigged figure".into()));
        }
        if object.get("_sticknodesShapes").is_some() || segments.iter().any(|s| s.get("_sticknodes").is_some()) {
            return Err(EditError::Unported("posing a Stick Nodes figure".into()));
        }
        // `selectedSlot` is the selected id's slot; without it there is no shape.
        let selected = self.selection.uid.filter(|&u| uid(&object) == Some(u));
        if selected.is_none() || self.selection.joint < 0 {
            return Ok(None);
        }
        let joint = self.selection.joint as usize;
        Ok(Some(Joint { slot, object, segments, joint }))
    }

    /// The selected segment as the geometry at the playhead draws it.
    fn shape(&self, object: &Value, joint: usize) -> Result<Option<Shape>, EditError> {
        let (prepared, frame, scene) = self.evaluate()?;
        let Some(id) = uid(object) else { return Ok(None) };
        let Some(evaluated) = frame.objects.iter().find(|o| o.uid == id) else { return Ok(None) };
        let Some(geometry) = scene.objects.iter().find(|g| g.uid == id && g.resolved) else { return Ok(None) };
        let figure = evaluated.figure_override.as_ref().or_else(|| prepared.doc.figure(evaluated.figure));
        let Some(segment) = figure.and_then(|f| f.segments.get(joint)) else { return Ok(None) };
        let start = geometry.starts.get(joint).copied().unwrap_or([f64::NAN; 2]);
        let (angle, bend, _) = styk_eval::geometry::segment_pose(evaluated, segment, joint);
        Ok(Some(Shape { start, angle, bend }))
    }

    /// `_poseSnapState` for the selected joint, from the gesture's snapshot.
    fn snap_state(&self, segments: &[Value], joint: usize) -> SnapState {
        let segment = &segments[joint];
        let (seg_length, seg_bend) = (id_of(segment, "length"), segment.get("bend_rad").and_then(Value::as_f64).unwrap_or(0.0));
        let before = self.drag.as_ref().and_then(|d| d.before.iter().find(|o| uid(o) == self.selection.uid).cloned()).unwrap_or(Value::Null);
        let item = |key: &str, i: usize| before.get(key).and_then(|v| v.item(i)).and_then(|v| v.as_f64()).unwrap_or(f64::NAN);
        let length = if before.get("len_flag").is_some_and(Value::truthy) { item("lengths", joint) } else { seg_length };
        let bend = if before.get("bend_flag").is_some_and(Value::truthy) { item("bends", joint) } else { seg_bend };
        SnapState {
            length: seg_length,
            bend: seg_bend,
            length_enabled: (length - seg_length).abs() > 1f64.max(seg_length.abs()) * 1e-7,
            bend_enabled: (bend - seg_bend).abs() > 1e-7,
        }
    }

    /// `_rotateJoint(delta, includeJoint, {isolate})` on the native frame at the playhead.
    fn rotate_joint_by(&mut self, slot: usize, joint: usize, delta: f64, include_joint: bool, isolate: bool) {
        let objects = self.frame_objects();
        let figures: Vec<Value> = self.report.get("figure_types").and_then(Value::elements).map(|e| e.iter().collect()).unwrap_or_default();
        for (object_slot, segment) in rotation_targets(&objects, &figures, slot, joint, isolate) {
            if (include_joint || object_slot != slot || segment != joint)
                && let Some(record) = self.frame_object_mut(object_slot)
            {
                add_angle(record, segment, delta);
            }
        }
    }

    pub(crate) fn rotate_joint(&mut self, delta: f64, options: &Value) -> Result<bool, EditError> {
        finite(delta)?;
        let Some(Joint { slot, segments, joint, .. }) = self.joint_context()? else { return Ok(false) };
        if joint >= segments.len() {
            return Ok(false);
        }
        let isolate = options.get("isolate").is_some_and(Value::truthy);
        self.object_edit("Rotate joint", |editor| {
            editor.rotate_joint_by(slot, joint, delta, true, isolate);
            Ok(())
        })
    }

    pub(crate) fn pose_joint(&mut self, x: f64, y: f64, options: &Value) -> Result<bool, EditError> {
        // Rigged figures (refused by joint_context) are checked before the arguments, as there.
        let context = self.joint_context()?;
        let options = PoseOptions::from_value(options);
        finite(x)?;
        finite(y)?;
        finite(options.snap_tolerance)?;
        if options.snap_tolerance < 0.0 {
            return Err(EditError::Invalid("errors.editor.snapToleranceCannotBeNegative".into()));
        }
        let Some(Joint { slot, object, segments, joint }) = context else { return Ok(false) };
        // A stretch drag poses the node directly, like one without a controller.
        let handle = segments.get(joint).map(|s| id_of(s, "segment_id")).filter(|&h| h.fract() == 0.0 && (1.0..=u32::MAX as f64).contains(&h));
        if !options.stretch
            && let Some(handle) = handle
            && object.path(&["_stykIK", &handle.to_string()]).is_some_and(|c| c.get("enabled").is_some_and(Value::truthy))
        {
            return self.pose_ik_target(slot, handle as u32, x, y);
        }
        let Some(segment) = segments.get(joint) else { return Ok(false) };
        if is_static(segment) {
            return Ok(false);
        }
        let Some(shape) = self.shape(&object, joint)? else { return Ok(false) };
        let angle = atan2(y - shape.start[1], x - shape.start[0]) - shape.bend / 2.0;
        self.object_edit("Pose joint", |editor| {
            let defaults = editor.snap_state(&segments, joint);
            editor.rotate_joint_by(slot, joint, shortest(angle - shape.angle), true, options.isolate);
            let record = editor.frame_object_mut(slot).unwrap();
            let scale = field(record, "scale");
            if options.stretch && scale.abs() > 1e-8 {
                if !record.get("len_flag").is_some_and(Value::truthy) {
                    record.set("lengths", Value::array(segments.iter().map(|s| s.get("length").cloned().unwrap_or(Value::Null)).collect()));
                }
                record.set("len_flag", Value::Num(1.0));
                let bend = shape.bend;
                let chord = if bend.abs() < 1e-7 { 1.0 } else { (2.0 * sin(bend / 2.0) / bend).abs() };
                if chord < 1e-8 {
                    return Err(EditError::Invalid("errors.editorMotionPath.reduceTheFullCircleBendBeforeStretchingThis".into()));
                }
                let reached = hypot(x - shape.start[0], y - shape.start[1]) / (scale.abs() * chord);
                let mut length = reached;
                if options.snap && defaults.length_enabled && (length - defaults.length).abs() * scale.abs() * chord <= options.snap_tolerance {
                    length = defaults.length;
                }
                set_item(record, "lengths", joint, length);
            }
            Ok(())
        })
    }

    pub(crate) fn bend_joint(&mut self, x: f64, y: f64, options: &Value) -> Result<bool, EditError> {
        let options = PoseOptions::from_value(options);
        finite(x)?;
        finite(y)?;
        finite(options.snap_tolerance)?;
        if options.snap_tolerance < 0.0 {
            return Err(EditError::Invalid("errors.editor.snapToleranceCannotBeNegative".into()));
        }
        let Some(Joint { slot, object, segments, joint }) = self.joint_context()? else { return Ok(false) };
        let Some(segment) = segments.get(joint) else { return Ok(false) };
        let primitive = segment.get("primitive_code").and_then(Value::as_f64).unwrap_or_else(|| ((segment.get("flags").and_then(Value::as_f64).unwrap_or(0.0) as i64) & 255) as f64);
        if is_static(segment) || !(primitive == 0.0 || primitive == 6.0) {
            return Ok(false);
        }
        let Some(shape) = self.shape(&object, joint)? else { return Ok(false) };
        let distance = hypot(x - shape.start[0], y - shape.start[1]);
        let mut bend = if distance < 1e-8 { shape.bend } else { 2.0 * shortest(atan2(y - shape.start[1], x - shape.start[0]) - shape.angle) };
        let segment_length = segment.get("length").and_then(Value::as_f64).unwrap_or(f64::NAN);
        self.object_edit("Bend joint", |editor| {
            let defaults = editor.snap_state(&segments, joint);
            let chord = |value: f64| if value.abs() < 1e-7 { 1.0 } else { (2.0 * sin(value / 2.0) / value).abs() };
            {
                let record = editor.frame_object_mut(slot).unwrap();
                let scale = field(record, "scale").abs();
                let mut length = if record.get("len_flag").is_some_and(Value::truthy) {
                    record.get("lengths").and_then(|l| l.item(joint)).and_then(|v| v.as_f64()).unwrap_or(f64::NAN)
                } else {
                    segment_length
                };
                if options.snap && defaults.bend_enabled {
                    let radius = if options.stretch { distance } else { length * scale * chord(defaults.bend) };
                    let direction = shape.angle + defaults.bend / 2.0;
                    if hypot(shape.start[0] + radius * cos(direction) - x, shape.start[1] + radius * sin(direction) - y) <= options.snap_tolerance {
                        bend = defaults.bend;
                    }
                }
                if options.stretch && scale > 1e-8 {
                    let factor = chord(bend);
                    if factor < 1e-8 {
                        return Err(EditError::Invalid("errors.editorMotionPath.reduceTheFullCircleBendBeforeStretchingThis".into()));
                    }
                    length = distance / (scale * factor);
                    if options.snap && defaults.length_enabled && (length - defaults.length).abs() * scale * factor <= options.snap_tolerance {
                        length = defaults.length;
                    }
                    if !record.get("len_flag").is_some_and(Value::truthy) {
                        record.set("lengths", Value::array(segments.iter().map(|s| s.get("length").cloned().unwrap_or(Value::Null)).collect()));
                    }
                    record.set("len_flag", Value::Num(1.0));
                    set_item(record, "lengths", joint, length);
                }
            }
            editor.rotate_joint_by(slot, joint, bend - shape.bend, false, options.isolate);
            let record = editor.frame_object_mut(slot).unwrap();
            if !record.get("bend_flag").is_some_and(Value::truthy) {
                record.set("bends", Value::array(segments.iter().map(|s| Value::Num(s.get("bend_rad").and_then(Value::as_f64).unwrap_or(0.0))).collect()));
            }
            record.set("bend_flag", Value::Num(1.0));
            set_item(record, "bends", joint, bend);
            Ok(())
        })
    }
}
