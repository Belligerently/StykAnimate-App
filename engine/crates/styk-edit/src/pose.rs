//! Posing joints (`poseJoint`, `rotateJoint`, `bendJoint`, `_rotateJoint`,
//! `_poseSnapState`) and the rotation targets of a joint (`rotationEdits` in
//! src/pivot/structure.mjs). Posing the end of a limb with an IK controller goes to
//! [`crate::ik`]. Rig controls pose in the rig's space: connectors slide and bend along
//! their centerline (`slideConnector`, `setConnectorPosition`, `_bendConnector`), angle locks
//! stretch, and a joint in a rig space takes the pointer through `authoredRigTarget`. 3D
//! bones answer `Unported`.

use std::collections::{HashMap, HashSet};

use styk_doc::{Obj, Value};
use styk_math::{atan2, hypot, shortest, sin, cos};

use crate::record::{join_target, num, uid};
use crate::transform::{field, finite};
use crate::{EditError, Editor};

/// A segment as the geometry drew it (`geometry.objects.get(slot).segments[joint]`).
#[derive(Clone, Debug)]
struct Shape {
    start: [f64; 2],
    angle: f64,
    bend: f64,
    /// The drawn (scaled) length and end, and the centerline's points (a rigged joint).
    length: f64,
    end: [f64; 2],
    points: Vec<[f64; 2]>,
    /// `rigSpace`: the space a rigged joint's authored angle is in.
    space: Option<styk_eval::rig::Space>,
    connector: bool,
}

/// Whether a segment's rig relationship has `path` (`segment._stykRig?.…`).
fn rig_has(segment: &Value, path: &[&str]) -> bool {
    let mut keys = vec!["_stykRig"];
    keys.extend_from_slice(path);
    segment.path(&keys).is_some_and(Value::truthy)
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
    let ids: Vec<f64> = segments.iter().map(|s| id_of(s, "segment_id")).collect();
    let mut handles: Vec<f64> = vec![ids[joint]];
    // Membership by bits, as the handle lists compare them; a figure may have thousands of segments.
    let mut taken: HashSet<u64> = HashSet::from([ids[joint].to_bits()]);
    let mut children: HashMap<u64, Vec<f64>> = HashMap::new();
    let mut by_id: HashMap<u64, usize> = HashMap::new();
    for (i, s) in segments.iter().enumerate() {
        children.entry(id_of(s, "parent_id").to_bits()).or_default().push(ids[i]);
        by_id.entry(ids[i].to_bits()).or_insert(i);
    }
    let mut q = 0;
    while q < handles.len() {
        for &id in children.get(&handles[q].to_bits()).map(Vec::as_slice).unwrap_or(&[]) {
            if !taken.contains(&id.to_bits()) && (!isolate || by_id.get(&id.to_bits()).is_some_and(|&i| static_group(&segments[i]))) {
                taken.insert(id.to_bits());
                handles.push(id);
            }
        }
        q += 1;
    }
    let mut targets: Vec<(usize, usize)> = (0..segments.len()).filter(|&i| taken.contains(&ids[i].to_bits())).map(|i| (slot, i)).collect();
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
/// `object.angles[i] += delta` for each of `indices`, the angle array unpacked once.
fn add_angles(object: &mut Obj, indices: &[usize], delta: f64) {
    let Some(angles) = object.get_mut("angles") else { return };
    let Some(list) = angles.make_arr() else { return };
    for &index in indices {
        if let Some(angle) = list.get_mut(index) {
            *angle = Value::Num(angle.as_f64().unwrap_or(f64::NAN) + delta);
        }
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
        // The selected figure's geometry, and that of the figures it hangs from.
        let slot = self.frame_objects().iter().position(|o| uid(o) == uid(object));
        let (prepared, frame, scene) = self.evaluate_only(slot.as_ref().map(std::slice::from_ref))?;
        let Some(id) = uid(object) else { return Ok(None) };
        let Some(evaluated) = frame.objects.iter().find(|o| o.uid == id) else { return Ok(None) };
        let Some(geometry) = scene.objects.iter().find(|g| g.uid == id && g.resolved) else { return Ok(None) };
        let figure = evaluated.figure_override.as_ref().or_else(|| prepared.doc.figure(evaluated.figure));
        let Some(segment) = figure.and_then(|f| f.segments.get(joint)) else { return Ok(None) };
        let start = geometry.starts.get(joint).copied().unwrap_or([f64::NAN; 2]);
        // A rigged joint is drawn as its rig evaluates it.
        if let Some(rig) = &geometry.rig {
            let item = scene.items.iter().find(|item| item.slot == geometry.slot && item.index as usize == joint && !matches!(item.kind, styk_eval::geometry::ItemKind::Poly | styk_eval::geometry::ItemKind::Mesh));
            let Some(item) = item else { return Err(EditError::Unported("posing a rigged joint the engine does not draw".into())) };
            let points = if item.points.1 > item.points.0 { scene.points_of(item).to_vec() } else { vec![item.start, item.end] };
            let joint_rig = rig.get(joint).copied().unwrap_or_default();
            return Ok(Some(Shape { start, angle: item.angle, bend: item.bend, length: item.length, end: item.end, points, space: joint_rig.space, connector: joint_rig.connector }));
        }
        let (_, bend, _) = styk_eval::geometry::segment_pose(evaluated, segment, joint);
        // A straight Stick Nodes segment with a sagitta is drawn as a curve, which geometry
        // reports with a bend of 1e-6.
        let bend = if bend.abs() < 1e-7 && styk_eval::sticknodes::shape_for(segment, evaluated).is_some_and(|s| s.sagitta != 0.0) { 1e-6 } else { bend };
        // A pinned limb is drawn with its solved angles.
        let slot = frame.objects.iter().position(|o| o.uid == id).unwrap();
        let angle = crate::ik::displayed_angle(&prepared, &frame, &crate::ik::solve_frame(&prepared, &frame, &scene), slot, joint);
        let length = styk_eval::geometry::segment_pose(evaluated, segment, joint).2 * evaluated.scale;
        let end = geometry.ends.get(joint).copied().unwrap_or([f64::NAN; 2]);
        Ok(Some(Shape { start, angle, bend, length, end, points: vec![start, end], space: None, connector: false }))
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

    /// `_rotateJoint(delta, includeJoint, {isolate})` (`_applyJointRotation`) on the frame being edited.
    fn rotate_joint_by(&mut self, slot: usize, joint: usize, delta: f64, include_joint: bool, isolate: bool) -> Result<(), EditError> {
        let objects = self.frame_objects();
        let figures: Vec<Value> = self.report.get("figure_types").and_then(Value::elements).map(|e| e.iter().collect()).unwrap_or_default();
        // An angle-locked joint does not turn, nor does anything below it.
        let figure = objects.get(slot).and_then(|o| figures.iter().find(|f| num(f, "figure_type_id") == num(o, "figure_type_id")));
        if include_joint && figure.and_then(|f| f.get("segments").and_then(|s| s.item(joint))).is_some_and(|s| rig_has(&s, &["angleLock"])) {
            return Ok(());
        }
        let key = (slot, joint, isolate);
        let targets = match self.drag.as_ref().and_then(|drag| drag.rotation_targets.get(&key)) {
            Some(targets) => targets.clone(),
            None => {
                let targets = rotation_targets(&objects, &figures, slot, joint, isolate);
                if let Some(drag) = self.drag.as_mut() {
                    drag.rotation_targets.insert(key, targets.clone());
                }
                targets
            }
        };
        self.rotate_ik_followers(slot, joint, &targets, delta, include_joint)?;
        // Grouped by figure, in the order the figures first appear (each angle changes once).
        let mut by_object: Vec<(usize, Vec<usize>)> = Vec::new();
        for (object_slot, segment) in targets {
            if include_joint || object_slot != slot || segment != joint {
                match by_object.iter_mut().find(|(s, _)| *s == object_slot) {
                    Some((_, list)) => list.push(segment),
                    None => by_object.push((object_slot, vec![segment])),
                }
            }
        }
        for (object_slot, segments) in by_object {
            if let Some(record) = self.frame_object_mut(object_slot) {
                add_angles(record, &segments, delta);
            }
        }
        Ok(())
    }

    pub(crate) fn rotate_joint(&mut self, delta: f64, options: &Value) -> Result<bool, EditError> {
        finite(delta)?;
        let Some(Joint { slot, segments, joint, .. }) = self.joint_context()? else { return Ok(false) };
        if joint >= segments.len() {
            return Ok(false);
        }
        let isolate = options.get("isolate").is_some_and(Value::truthy);
        if !isolate && rig_has(&segments[joint], &["spatial", "origin"]) {
            return Err(EditError::Unported("rotating a 3D bone".into()));
        }
        self.object_edit_with("Rotate joint", true, |editor| editor.rotate_joint_by(slot, joint, delta, true, isolate))
    }

    pub(crate) fn pose_joint(&mut self, x: f64, y: f64, options: &Value) -> Result<bool, EditError> {
        // Rigged figures (refused by joint_context) are checked before the arguments, as there.
        let context = self.joint_context()?;
        // A skin point, an angle path or a weighted joint is not posed by hand; checked
        // before the arguments, as there.
        if let Some(Joint { segments, joint, .. }) = &context
            && segments.get(*joint).is_some_and(|s| rig_has(s, &["spatial", "bindings"]) || rig_has(s, &["anglePath"]) || rig_has(s, &["weighted"]))
        {
            return Ok(false);
        }
        let options = PoseOptions::from_value(options);
        finite(x)?;
        finite(y)?;
        finite(options.snap_tolerance)?;
        if options.snap_tolerance < 0.0 {
            return Err(EditError::Invalid("errors.editor.snapToleranceCannotBeNegative".into()));
        }
        let Some(Joint { slot, object, segments, joint }) = context else { return Ok(false) };
        if segments.get(joint).is_some_and(|s| rig_has(s, &["connector"])) {
            return self.slide_connector(slot, &object, joint, x, y);
        }
        // A stretch drag poses the node directly, like one without a controller.
        let handle = segments.get(joint).map(|s| id_of(s, "segment_id")).filter(|&h| h.fract() == 0.0 && (1.0..=u32::MAX as f64).contains(&h));
        if !options.stretch
            && let Some(handle) = handle
            && object.path(&["_stykIK", &handle.to_string()]).is_some_and(|c| c.get("enabled").is_some_and(Value::truthy))
        {
            if segments.iter().any(|s| s.get("_stykRig").is_some()) {
                return Err(EditError::Unported("posing an IK target on a rigged figure".into()));
            }
            return self.pose_ik_target(slot, handle as u32, x, y);
        }
        let Some(segment) = segments.get(joint) else { return Ok(false) };
        if is_static(segment) {
            return Ok(false);
        }
        if rig_has(segment, &["spatial", "origin"]) {
            return Err(EditError::Unported("posing a 3D bone".into()));
        }
        let Some(shape) = self.shape(&object, joint)? else { return Ok(false) };
        let scale = object.get("scale").and_then(Value::as_f64).unwrap_or(f64::NAN);
        let authored_length = if object.get("len_flag").is_some_and(Value::truthy) {
            object.get("lengths").and_then(|l| l.item(joint)).and_then(|v| v.as_f64()).unwrap_or(f64::NAN)
        } else {
            id_of(segment, "length")
        };
        // An angle-locked joint only stretches, along its drawn chord.
        if rig_has(segment, &["angleLock"]) {
            if !options.stretch || scale.abs() < 1e-8 {
                return Ok(false);
            }
            let axis = shape.angle + shape.bend / 2.0;
            let along = 0f64.max((x - shape.start[0]) * cos(axis) + (y - shape.start[1]) * sin(axis));
            let chord_length = hypot(shape.end[0] - shape.start[0], shape.end[1] - shape.start[1]);
            if authored_length < 1e-8 || chord_length < 1e-8 {
                return Err(EditError::Invalid("errors.editor.expandTheStretchDriverBeforePosingAFully".into()));
            }
            return self.object_edit_with("Stretch locked joint", true, |editor| {
                let record = editor.frame_object_mut(slot).unwrap();
                if !record.get("len_flag").is_some_and(Value::truthy) {
                    record.set("lengths", Value::array(segments.iter().map(|s| s.get("length").cloned().unwrap_or(Value::Null)).collect()));
                }
                record.set("len_flag", Value::Num(1.0));
                set_item(record, "lengths", joint, along * authored_length / chord_length);
                Ok(())
            });
        }
        // Stretching the end of a pinned limb: its pin follows the stretched end.
        let pin = handle.filter(|&h| {
            options.stretch && object.path(&["_stykIK", &h.to_string()]).is_some_and(|c| c.get("enabled").is_some_and(Value::truthy) && c.get("pinned").is_some_and(Value::truthy))
        });
        let pinned = match pin {
            Some(handle) => Some(self.pinned_limb(&object, handle as u32)?),
            None => None,
        };
        let pointer = [x, y];
        // The pointer in the joint's rig space (`authoredRigTarget`).
        let pose = (!options.stretch).then(|| (object.get("angles").and_then(|a| a.item(joint)).and_then(|v| v.as_f64()).unwrap_or(f64::NAN), authored_length * object.get("scale").and_then(Value::as_f64).unwrap_or(1.0)));
        let Some([x, y]) = styk_eval::ik::rig::authored_rig_target_with(shape.start, shape.space, shape.bend, pointer, pose) else {
            return Err(EditError::Invalid("errors.rigConstraints.expandTheStretchDriverBeforePosingAFully".into()));
        };
        let angle = atan2(y - shape.start[1], x - shape.start[0]) - shape.bend / 2.0;
        self.object_edit_with("Pose joint", true, |editor| {
            let defaults = editor.snap_state(&segments, joint);
            let current = if shape.space.is_some() { editor.frame_objects()[slot].get("angles").and_then(|a| a.item(joint)).and_then(|v| v.as_f64()).unwrap_or(f64::NAN) } else { shape.angle };
            editor.rotate_joint_by(slot, joint, shortest(angle - current), true, options.isolate)?;
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
                // The rest of a limb displayed beyond its reach keeps the length shown.
                if let Some(limb) = &pinned
                    && let Some(solved) = &limb.solved_lengths
                {
                    for &i in &limb.chain {
                        set_item(record, "lengths", i, solved[i]);
                    }
                }
                set_item(record, "lengths", joint, length);
                if let (Some(limb), Some(handle)) = (&pinned, pin) {
                    let ratio = if reached > 1e-8 { length / reached } else { 0.0 };
                    let tip = [shape.start[0] + (pointer[0] - shape.start[0]) * ratio, shape.start[1] + (pointer[1] - shape.start[1]) * ratio];
                    crate::ik::follow_stretched(record, handle as u32, &limb.points, tip);
                }
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
        if rig_has(segment, &["spatial", "origin"]) {
            return Err(EditError::Unported("twisting a 3D bone".into()));
        }
        let primitive = segment.get("primitive_code").and_then(Value::as_f64).unwrap_or_else(|| ((segment.get("flags").and_then(Value::as_f64).unwrap_or(0.0) as i64) & 255) as f64);
        if is_static(segment) || !(primitive == 0.0 || primitive == 6.0) {
            return Ok(false);
        }
        let Some(shape) = self.shape(&object, joint)? else { return Ok(false) };
        if shape.connector {
            return self.bend_connector(slot, &segments, joint, x, y, &shape);
        }
        let distance = hypot(x - shape.start[0], y - shape.start[1]);
        let mut bend = if distance < 1e-8 { shape.bend } else { 2.0 * shortest(atan2(y - shape.start[1], x - shape.start[0]) - shape.angle) };
        let segment_length = segment.get("length").and_then(Value::as_f64).unwrap_or(f64::NAN);
        self.object_edit_with("Bend joint", true, |editor| {
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
            editor.rotate_joint_by(slot, joint, bend - shape.bend, false, options.isolate)?;
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

impl Editor {
    /// `slideConnector(x, y)`: the connector's position from the pointer projected onto its
    /// drawn centerline (curves included); ties keep the first interval.
    fn slide_connector(&mut self, slot: usize, object: &Value, joint: usize, x: f64, y: f64) -> Result<bool, EditError> {
        rig_finite(x, -1e9, 1e9, "Joint X")?;
        rig_finite(y, -1e9, 1e9, "Joint Y")?;
        let Some(shape) = self.shape(object, joint)? else { return Ok(false) };
        if !shape.connector {
            return Ok(false);
        }
        let points = &shape.points;
        let distances: Vec<f64> = points.windows(2).map(|w| hypot(w[1][0] - w[0][0], w[1][1] - w[0][1])).collect();
        let total: f64 = distances.iter().fold(0.0, |a, b| a + b);
        if total < 1e-8 {
            return Ok(false);
        }
        let (mut best, mut position, mut before) = (f64::INFINITY, None, 0.0);
        for i in 1..points.len() {
            let (a, b, d) = (points[i - 1], points[i], distances[i - 1]);
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            if d < 1e-8 {
                continue;
            }
            let u = (((x - a[0]) * dx + (y - a[1]) * dy) / (d * d)).clamp(0.0, 1.0);
            let distance = (x - a[0] - dx * u).powi(2) + (y - a[1] - dy * u).powi(2);
            if distance < best {
                best = distance;
                position = Some((before + u * d) / total);
            }
            before += d;
        }
        match position {
            Some(t) => self.set_connector_position_at(slot, joint, t),
            None => Ok(false),
        }
    }

    /// `setConnectorPosition(position)` for the selected joint.
    pub(crate) fn set_connector_position(&mut self, position: f64) -> Result<bool, EditError> {
        let Some((slot, object)) = self.object()? else { return Ok(false) };
        let figure = self.report.get("figure_types").and_then(Value::elements).and_then(|e| e.iter().find(|f| num(f, "figure_type_id") == num(&object, "figure_type_id")));
        let joint = self.selection.joint;
        let segment = figure.and_then(|f| f.get("segments").and_then(|s| if joint >= 0 { s.item(joint as usize) } else { None }));
        if !segment.is_some_and(|s| rig_has(&s, &["connector"])) {
            return Ok(false);
        }
        self.set_connector_position_at(slot, joint as usize, position)
    }

    fn set_connector_position_at(&mut self, slot: usize, joint: usize, position: f64) -> Result<bool, EditError> {
        rig_finite(position, 0.0, 1.0, "Slide position")?;
        let figure_lengths = self.frame_objects().get(slot).and_then(|o| {
            let figures = self.report.get("figure_types").and_then(Value::elements)?;
            let figure = figures.iter().find(|f| num(f, "figure_type_id") == num(o, "figure_type_id"))?;
            Some(segments_of(&figure).iter().map(|s| s.get("length").cloned().unwrap_or(Value::Null)).collect::<Vec<_>>())
        });
        self.object_edit_with("Slide connector", true, |editor| {
            let record = editor.frame_object_mut(slot).unwrap();
            if !record.get("len_flag").is_some_and(Value::truthy) {
                record.set("lengths", Value::array(figure_lengths.unwrap_or_default()));
            }
            record.set("len_flag", Value::Num(1.0));
            set_item(record, "lengths", joint, position);
            record.remove("runtime_segments");
            Ok(())
        })
    }

    /// `_bendConnector(x, y, shape)`: a connector's chord is fixed by its anchors, so its arc
    /// is fitted through the pointer (the tangent-chord angle is the supplement of the
    /// inscribed angle).
    fn bend_connector(&mut self, slot: usize, segments: &[Value], joint: usize, x: f64, y: f64, shape: &Shape) -> Result<bool, EditError> {
        use std::f64::consts::PI;
        let factor = if shape.bend.abs() < 1e-7 { 1.0 } else { 2.0 * sin(shape.bend / 2.0) / shape.bend };
        let chord = shape.angle + shape.bend / 2.0;
        let span = shape.length * factor;
        let a = shape.start;
        let b = [a[0] + span * cos(chord), a[1] + span * sin(chord)];
        let (ux, uy, vx, vy) = (a[0] - x, a[1] - y, b[0] - x, b[1] - y);
        if hypot(ux, uy) < 1e-6 || hypot(vx, vy) < 1e-6 || span.abs() < 1e-6 {
            return Ok(false);
        }
        let inscribed = atan2((ux * vy - uy * vx).abs(), ux * vx + uy * vy);
        let side = (b[0] - a[0]) * (y - a[1]) - (b[1] - a[1]) * (x - a[0]);
        let bend = (-PI * 1.99).max((PI * 1.99).min(2.0 * (PI - inscribed) * if side < 0.0 { 1.0 } else { -1.0 }));
        self.object_edit_with("Bend joint", true, |editor| {
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

/// `finite(value, min, max, label)` of the rig commands.
fn rig_finite(value: f64, min: f64, max: f64, _label: &str) -> Result<f64, EditError> {
    if !value.is_finite() || value < min || value > max {
        return Err(EditError::Invalid("errors.cameraMotion.mustBeBetweenAnd".into()));
    }
    Ok(value)
}
