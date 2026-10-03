//! 3D bones as editing sees them (src/editor-spatial.mjs).
//!
//! Ported: a bone's pose read from the angles it is drawn with (`getSpatialPose`), turning
//! it (`setSpatialPose`, and `rotateJoint` on a bone), the travel of mechanical slides and
//! hinges (`getSpatialMotion`, `setSpatialMotion`), and the canvas drags: a bone turned
//! flat on screen or swung in depth (`poseSpatialJoint`, `_flatSpatialJoint`,
//! `_arcSpatialJoint`), twisted about its axis (`twistSpatialJoint`), a mechanism moved
//! along its track (`_moveMechanicalJoint`). Steering a planted 3D limb's knee, twisting
//! with 3D IK and pose choices are not ported.

use std::sync::Arc;

use styk_doc::{Obj, Value};
use styk_eval::doc::Figure;
use styk_eval::ik::spatial::{between_vectors, inverse_quaternion, quaternion_pose, spatial_world_quaternion, spatial_world_vector, Euler};
use styk_eval::rig::{quat_multiply, quat_rotate, spatial_point, spatial_quaternion, Motion, Spatial, SpatialFrame};
use styk_eval::spatial::Camera;
use styk_math::{atan2, cos, hypot, norm, sin};

use crate::record::uid;
use crate::{EditError, Editor};

/// The bone at a segment of a figure, as its rig relationship describes it.
pub(crate) struct Bone<'a> {
    pub parent: u32,
    pub turn: u32,
    pub pitch: u32,
    pub origin: [f64; 3],
    pub tip: [f64; 3],
    pub motion: Option<&'a Motion>,
}

pub(crate) fn bone(figure: &Figure, index: usize) -> Option<Bone<'_>> {
    match figure.segments.get(index)?.rig.as_ref()?.spatial.as_ref()? {
        Spatial::Bone { parent, turn, pitch, origin, tip, motion } => Some(Bone { parent: *parent, turn: *turn, pitch: *pitch, origin: *origin, tip: *tip, motion: motion.as_ref() }),
        Spatial::Skin { .. } => None,
    }
}

/// What the selected figure's 3D bones are read from: its record, its definition, the
/// frames geometry gave its bones, and the angles it is drawn with.
pub(crate) struct Context {
    pub slot: usize,
    pub object: Value,
    pub figure: Arc<Figure>,
    pub frames: Vec<Option<SpatialFrame>>,
    /// `geometry…solvedAngles ?? object.angles`.
    pub angles: Vec<f64>,
    /// `animationRotation(figure, object)`.
    pub figure_angle: f64,
    pub mirror: bool,
    pub root: [f64; 2],
    pub scale: f64,
    /// The pose's perspective (`projection(object, root)`).
    pub camera: Option<Camera>,
    /// `unprojectJoint(shape.start)` per segment: the start before perspective.
    pub starts: Vec<[f64; 2]>,
}

/// A 3D bone drag from its fixed start (`_spatialGesture(kind)`), or a mechanism's track.
#[derive(Clone, Debug)]
pub(crate) enum Gesture {
    Bone(BoneGesture),
    Mechanism(MechanismGesture),
}

#[derive(Clone, Debug)]
pub(crate) struct BoneGesture {
    origin: [f64; 3],
    tip: [f64; 3],
    length: f64,
    direction: [f64; 3],
    side: f64,
    world: [f64; 4],
    parent: [f64; 4],
    /// Set by the first sample: the twist's pointer, the flat drag's grab and the bone's
    /// length then, the arc's pointer.
    start: Option<[f64; 2]>,
    grab: Option<[f64; 2]>,
    native_length: Option<f64>,
    pointer: Option<[f64; 2]>,
}

#[derive(Clone, Debug)]
pub(crate) struct MechanismGesture {
    slide: bool,
    tip: [f64; 3],
    pivot: [f64; 3],
    axis: [f64; 3],
    value: f64,
    start: [f64; 3],
    grab: [f64; 2],
    figure_angle: f64,
    mirror: bool,
    scale: f64,
    root: [f64; 2],
}

impl MechanismGesture {
    /// `along(p)`: where the moving end is at travel `p`, in the scene.
    fn along(&self, p: f64) -> [f64; 3] {
        let d = p - self.value;
        let local = if self.slide {
            [self.tip[0] + self.axis[0] * d, self.tip[1] + self.axis[1] * d, self.tip[2] + self.axis[2] * d]
        } else {
            let turned = quat_rotate(axis_quaternion(self.axis, d), [self.tip[0] - self.pivot[0], self.tip[1] - self.pivot[1], self.tip[2] - self.pivot[2]]);
            [turned[0] + self.pivot[0], turned[1] + self.pivot[1], turned[2] + self.pivot[2]]
        };
        world_point(local, self.figure_angle, self.mirror, self.scale, self.root)
    }
}

/// `spatialAxisQuaternion(axis, angle)`.
fn axis_quaternion(axis: [f64; 3], angle: f64) -> [f64; 4] {
    let h = sin(angle / 2.0) / norm(&axis);
    [axis[0] * h, axis[1] * h, axis[2] * h, cos(angle / 2.0)]
}

/// The `world(p)` of the gestures: a figure-space point in the scene, before perspective.
fn world_point(p: [f64; 3], figure_angle: f64, mirror: bool, scale: f64, root: [f64; 2]) -> [f64; 3] {
    let v = spatial_world_vector(p, figure_angle, mirror, false);
    let v = [v[0] * scale, v[1] * scale, v[2] * scale];
    [v[0] + root[0], v[1] + root[1], v[2]]
}

/// `Math.sign` for the values it sees here.
fn sign(v: f64) -> f64 {
    if v > 0.0 {
        1.0
    } else if v < 0.0 {
        -1.0
    } else {
        v
    }
}

/// The state of a mechanical joint (`getSpatialMotion`).
pub(crate) struct MotionState {
    pub hinge: bool,
    pub min: f64,
    pub max: f64,
    pub value: f64,
}

/// `Math.min(a, b)` / `Math.max(a, b)` for the values clamps see (NaN wins, as there).
fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() { f64::NAN } else { a.min(b) }
}
fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() { f64::NAN } else { a.max(b) }
}

fn number_at(value: &Value, key: &str, index: usize) -> f64 {
    value.get(key).and_then(|v| v.item(index)).and_then(|v| v.as_f64()).unwrap_or(f64::NAN)
}

impl Editor {
    /// The selected figure's 3D context, when it is posed here.
    pub(crate) fn spatial_context(&self) -> Result<Option<Context>, EditError> {
        let Some((slot, object)) = self.object()? else { return Ok(None) };
        let Some(id) = uid(&object) else { return Ok(None) };
        let (prepared, frame, scene) = self.evaluate_only(Some(&[slot]))?;
        let Some(at) = frame.objects.iter().position(|o| o.uid == id) else { return Ok(None) };
        let evaluated = &frame.objects[at];
        let Some(figure) = evaluated.figure_override.clone().or_else(|| prepared.doc.figure(evaluated.figure).cloned()) else { return Ok(None) };
        let geometry = scene.objects.iter().find(|g| g.uid == id && g.resolved);
        let frames = geometry.and_then(|g| g.spatial.as_ref()).map(|s| s.frames.clone()).unwrap_or_default();
        let root = geometry.map_or(evaluated.root, |g| g.root);
        // The start of each segment before perspective (its parent joint's raw point).
        let starts = (0..figure.segments.len())
            .map(|i| {
                let drawn = geometry.and_then(|g| g.starts.get(i).copied()).unwrap_or([f64::NAN; 2]);
                let parent = figure.plan.parent.get(i).copied().filter(|&p| p != u32::MAX);
                let raw = parent.and_then(|p| geometry.and_then(|g| g.spatial.as_ref()).and_then(|s| s.depths.get(p as usize)).and_then(|d| d.raw));
                raw.map_or(drawn, |r| [r[0], r[1]])
            })
            .collect();
        let camera = Camera::of(evaluated, root);
        let scale = evaluated.scale;
        let solved = crate::ik::solve_frame(&prepared, &frame, &scene).remove(&id).map(|s| s.solution.angles);
        let angles = solved.unwrap_or_else(|| (0..figure.segments.len().max(object.get("angles").and_then(Value::array_len).unwrap_or(0))).map(|i| number_at(&object, "angles", i)).collect());
        Ok(Some(Context {
            slot,
            figure_angle: styk_eval::ik::figure_angle(&figure, evaluated),
            mirror: object.get("image_text_flags").is_some_and(Value::truthy),
            object,
            figure,
            frames,
            angles,
            root,
            scale,
            camera,
            starts,
        }))
    }

    /// `getSpatialPose(index)`: the bone's bend, turn and pitch as the figure is drawn.
    pub(crate) fn spatial_pose(context: &Context, index: usize) -> Option<Euler> {
        let figure = &context.figure;
        let bone = bone(figure, index)?;
        let find = |id: u32| figure.segments.iter().position(|s| s.id == id);
        let delta = context.figure_angle;
        let sign = if context.mirror { -1.0 } else { 1.0 };
        let angle = |i: usize| context.angles.get(i).copied().unwrap_or(f64::NAN);
        let parent_angle = find(bone.parent).map_or(0.0, |p| (angle(p) - delta) * sign - figure.segments[p].angle);
        let bend = (angle(index) - delta) * sign - figure.segments[index].angle - parent_angle;
        if let Some(motion) = bone.motion {
            return Some(Euler { bend: if motion.hinge { atan2(sin(bend), cos(bend)) } else { 0.0 }, turn: 0.0, pitch: 0.0 });
        }
        let channel = |id: u32| find(id).map_or(f64::NAN, |i| (angle(i) - delta) * sign - figure.segments[i].angle);
        Some(Euler { bend, turn: channel(bone.turn), pitch: channel(bone.pitch) })
    }

    /// `getSpatialMotion(index)`.
    pub(crate) fn spatial_motion(context: &Context, index: usize) -> Option<MotionState> {
        let bone = bone(&context.figure, index)?;
        let motion = bone.motion?;
        let index_of = |id: u32| context.figure.plan.index_of.get(&id).map(|&i| i as usize);
        let driver = motion.driver.map_or(0.0, |(joint, multiplier)| {
            index_of(joint).and_then(|i| context.frames.get(i).copied().flatten()).map_or(0.0, |frame| frame.motion_value) * multiplier
        });
        let length = if context.object.get("len_flag").is_some_and(Value::truthy) { number_at(&context.object, "lengths", index) } else { context.figure.segments[index].length };
        let bend = if motion.hinge { Self::spatial_pose(context, index).map_or(f64::NAN, |p| p.bend) } else { 0.0 };
        // `spatialMotionValue(spatial, {length, bend, driver})`
        let raw = if motion.hinge { atan2(sin(bend), cos(bend)) } else { length - norm(&[bone.tip[0] - bone.origin[0], bone.tip[1] - bone.origin[1], bone.tip[2] - bone.origin[2]]) };
        let applied = js_min(motion.max, js_max(motion.min, raw + driver));
        Some(MotionState { hinge: motion.hinge, min: motion.min - driver, max: motion.max - driver, value: applied - driver })
    }

    /// `rotateJoint(delta)` on a 3D bone (without `isolate`): `setSpatialPose` with the bend turned.
    pub(crate) fn rotate_spatial_joint(&mut self, delta: f64) -> Result<Option<bool>, EditError> {
        let Some(context) = self.spatial_context()? else { return Ok(None) };
        let joint = self.selection.joint;
        if joint < 0 {
            return Ok(None);
        }
        let Some(pose) = Self::spatial_pose(&context, joint as usize) else { return Ok(None) };
        let sign = if context.mirror { -1.0 } else { 1.0 };
        let mut patch = Obj::new();
        patch.set("bend", Value::Num(pose.bend + delta * sign));
        self.set_spatial_pose(&Value::object(patch), &Value::Null).map(Some)
    }

    /// `setSpatialPose(patch, {wholeFigure})`.
    pub(crate) fn set_spatial_pose(&mut self, patch: &Value, options: &Value) -> Result<bool, EditError> {
        let Some(context) = self.spatial_context()? else { return Ok(false) };
        if self.selection.joint < 0 {
            return Ok(false);
        }
        let index = self.selection.joint as usize;
        let Some(before) = Self::spatial_pose(&context, index) else { return Ok(false) };
        let entries: Vec<(String, f64)> = patch.as_obj().map(|o| o.iter().map(|(k, v)| (k.to_string(), v.as_f64().unwrap_or(f64::NAN))).collect()).unwrap_or_default();
        if entries.iter().any(|(k, v)| !["bend", "turn", "pitch"].contains(&k.as_str()) || !v.is_finite() || v.abs() > 1e6) {
            return Err(EditError::Invalid("errors.editorSpatial.chooseAFiniteJointRotation".into()));
        }
        let get = |key: &str| entries.iter().find(|(k, _)| k == key).map(|(_, v)| *v);
        // A hinge takes only its own rotation; a slide does not rotate.
        if let Some(motion) = Self::spatial_motion(&context, index) {
            return match (motion.hinge, get("bend")) {
                (true, Some(bend)) => self.set_spatial_motion(bend, false),
                _ => Ok(false),
            };
        }
        let previous = |key: &str| match key {
            "bend" => before.bend,
            "turn" => before.turn,
            _ => before.pitch,
        };
        if entries.iter().all(|(k, v)| (v - previous(k)).abs() < 1e-12) {
            return Ok(false);
        }
        let bone = bone(&context.figure, index).unwrap();
        let Some(frame) = context.frames.get(index).copied().flatten() else {
            return Err(EditError::Unported("turning a 3D bone geometry did not frame".into()));
        };
        let parent = quat_multiply(frame.q, inverse_quaternion(spatial_quaternion(before.bend, before.turn, before.pitch)));
        let next = Euler { bend: get("bend").unwrap_or(before.bend), turn: get("turn").unwrap_or(before.turn), pitch: get("pitch").unwrap_or(before.pitch) };
        let world = spatial_world_quaternion(quat_multiply(parent, spatial_quaternion(next.bend, next.turn, next.pitch)), context.figure_angle, context.mirror, false);
        let whole = options.get("wholeFigure").is_some_and(Value::truthy);
        if whole && bone.parent != 0 {
            return Err(EditError::Invalid("errors.editorSpatial.selectTheRootArticulationToRotateTheWhole".into()));
        }
        if whole && context.object.get("_stykIK").and_then(Value::as_obj).is_some_and(|t| t.values().iter().any(|c| c.get("spatialTarget").is_some_and(Value::truthy))) {
            return Err(EditError::Unported("turning a whole figure with 3D IK".into()));
        }
        let handle = context.figure.segments[index].id;
        let index_of = |id: u32| context.figure.plan.index_of.get(&id).map(|&i| i as usize);
        let channels = [("turn", index_of(bone.turn)), ("pitch", index_of(bone.pitch))];
        let slot = context.slot;
        // `wholeFigure` rewrites the controllers from the table the figure had before the
        // edit (those without a 3D target unchanged).
        let controllers = if whole { context.object.get("_stykIK").filter(|t| t.truthy()).cloned() } else { None };
        let figure = context.figure.clone();
        self.object_edit_with("Rotate articulated joint", true, |editor| {
            crate::ik::release_spatial_chain(editor.frame_object_mut(slot).unwrap(), &figure, index);
            let sign = if editor.frame_objects()[slot].get("image_text_flags").is_some_and(Value::truthy) { -1.0 } else { 1.0 };
            if let Some(bend) = get("bend") {
                editor.rotate_joint_by(slot, index, (bend - before.bend) * sign, true, false)?;
            }
            let record = editor.frame_object_mut(slot).unwrap();
            for (key, at) in channels {
                if let (Some(value), Some(at)) = (get(key), at) {
                    crate::pose::add_angle_at(record, at, (value - previous(key)) * sign);
                }
            }
            plant_spatial_follower(record, handle, world);
            if let Some(table) = &controllers {
                record.set("_stykIK", table.clone());
            }
            Ok(())
        })
    }

    /// `setSpatialMotion(value, {fraction})`: a mechanical joint's travel, clamped to its range.
    pub(crate) fn set_spatial_motion(&mut self, value: f64, fraction: bool) -> Result<bool, EditError> {
        let Some(context) = self.spatial_context()? else { return Ok(false) };
        if self.selection.joint < 0 {
            return Ok(false);
        }
        let index = self.selection.joint as usize;
        let Some(motion) = Self::spatial_motion(&context, index) else { return Ok(false) };
        if !value.is_finite() {
            return Err(EditError::Invalid("errors.editorSpatial.chooseAFinitePosition".into()));
        }
        let target = js_min(motion.max, js_max(motion.min, if fraction { motion.min + value * (motion.max - motion.min) } else { value }));
        let bone = bone(&context.figure, index).unwrap();
        let rest = norm(&[bone.tip[0] - bone.origin[0], bone.tip[1] - bone.origin[1], bone.tip[2] - bone.origin[2]]);
        let length_flag = context.object.get("len_flag").is_some_and(Value::truthy);
        let current = if motion.hinge {
            Self::spatial_pose(&context, index).map_or(f64::NAN, |p| p.bend)
        } else if length_flag {
            number_at(&context.object, "lengths", index)
        } else {
            context.figure.segments[index].length
        };
        let next = if motion.hinge { target } else { rest + target };
        if (next - current).abs() < 1e-12 {
            return Ok(false);
        }
        let slot = context.slot;
        let lengths: Vec<Value> = context.figure.segments.iter().map(|s| Value::Num(s.length)).collect();
        self.object_edit_with(if motion.hinge { "Turn hinge" } else { "Slide mechanical joint" }, true, |editor| {
            if motion.hinge {
                let sign = if editor.frame_objects()[slot].get("image_text_flags").is_some_and(Value::truthy) { -1.0 } else { 1.0 };
                editor.rotate_joint_by(slot, index, (next - current) * sign, true, false)?;
            }
            let record = editor.frame_object_mut(slot).unwrap();
            if !motion.hinge {
                if !record.get("len_flag").is_some_and(Value::truthy) {
                    record.set("lengths", Value::array(lengths.clone()));
                }
                record.set("len_flag", Value::Num(1.0));
                crate::pose::set_item_at(record, "lengths", index, next);
            }
            record.remove("runtime_segments");
            Ok(())
        })
    }
}

/// `_plantSpatialFollower(object, handle, world)`: a pinned hand or foot keeps the world
/// orientation it is given.
fn plant_spatial_follower(record: &mut Obj, handle: u32, world: [f64; 4]) {
    let Some(table) = record.get("_stykIK").and_then(Value::as_obj).cloned() else { return };
    let key = handle.to_string();
    let mut changed = table.clone();
    let mut any = false;
    for (id, c) in table.iter() {
        let truthy = |k: &str| c.get(k).is_some_and(Value::truthy);
        if !(truthy("enabled") && truthy("pinned")) || !c.path(&["spatialFollowers", &key]).is_some_and(Value::truthy) {
            continue;
        }
        let rotation = c.path(&["target", "rotation"]).and_then(Value::as_f64).unwrap_or(f64::NAN);
        let q = quat_multiply(spatial_quaternion(-rotation, 0.0, 0.0), world);
        let mut next = c.as_obj().cloned().unwrap_or_default();
        let mut followers = c.get("spatialFollowers").and_then(Value::as_obj).cloned().unwrap_or_default();
        followers.set(&key, Value::nums(&q));
        next.set("spatialFollowers", Value::object(followers));
        changed.set(id, Value::object(next));
        any = true;
    }
    if any {
        record.set("_stykIK", Value::object(changed));
    }
}

impl Context {
    /// `unprojectSpatial(p, depth, object, root)`.
    fn unproject(&self, p: [f64; 2], depth: f64) -> [f64; 2] {
        match &self.camera {
            Some(camera) => camera.unproject(p, depth),
            None => p,
        }
    }

    fn world(&self, p: [f64; 3]) -> [f64; 3] {
        world_point(p, self.figure_angle, self.mirror, self.scale, self.root)
    }
}

impl Editor {
    /// `poseSpatialJoint(x, y, {stretch, depth, flip, pixel, anchor})`.
    pub(crate) fn pose_spatial_joint(&mut self, x: f64, y: f64, options: &Value) -> Result<bool, EditError> {
        let Some(context) = self.spatial_context()? else { return Ok(false) };
        let index = self.selection.joint as usize;
        let Some(bone) = bone(&context.figure, index) else { return Ok(false) };
        if !x.is_finite() || !y.is_finite() {
            return Ok(false);
        }
        let flag = |key: &str| options.get(key).is_some_and(Value::truthy);
        let mut anchor = options.get("anchor").filter(|a| a.truthy()).map(|a| [a.get("x").and_then(Value::as_f64).unwrap_or(f64::NAN), a.get("y").and_then(Value::as_f64).unwrap_or(f64::NAN)]);
        if bone.motion.is_some() {
            return self.move_mechanical_joint(&context, index, x, y, anchor);
        }
        let stretch = flag("stretch");
        // A planted knee or elbow steers its 3D bend plane (`setSpatialIKPole`).
        if !stretch && planted_limb_base(&context, index) {
            return Err(EditError::Unported("steering a planted 3D limb".into()));
        }
        let kind = if flag("depth") && !stretch { "arc" } else { "flat" };
        let uid = uid(&context.object).unwrap_or(0);
        if let Some(drag) = self.drag.as_mut() {
            // Pressing or releasing the depth key mid-drag starts the other gesture from the
            // current pose and pointer, so the bone never jumps.
            let key = format!("{uid}:{index}");
            if let Some(last) = drag.spatial_kinds.get(&key)
                && *last != kind
            {
                for k in ["arc", "flat"] {
                    drag.spatial_gestures.remove(&format!("{k}:{key}"));
                }
                anchor = Some([x, y]);
            }
            drag.spatial_kinds.insert(key, kind);
        }
        if kind == "arc" {
            let pixel = options.get("pixel").and_then(Value::as_f64).unwrap_or(0.0);
            self.arc_spatial_joint(&context, index, x, y, flag("alternate"), pixel, anchor)
        } else {
            self.flat_spatial_joint(&context, index, x, y, stretch, anchor)
        }
    }

    /// `_spatialGesture(kind)`: the drag's fixed start, kept for the rest of the gesture.
    fn bone_gesture(&mut self, context: &Context, index: usize, kind: &str) -> Result<BoneGesture, EditError> {
        let key = format!("{kind}:{}:{index}", uid(&context.object).unwrap_or(0));
        if let Some(Gesture::Bone(gesture)) = self.drag.as_ref().and_then(|d| d.spatial_gestures.get(&key)) {
            return Ok(gesture.clone());
        }
        let unported = || EditError::Unported("posing a 3D bone geometry did not frame".into());
        let bone = bone(&context.figure, index).ok_or_else(unported)?;
        let frame = context.frames.get(index).copied().flatten().ok_or_else(unported)?;
        let start = context.starts[index];
        let origin = [start[0], start[1], context.world(spatial_point(&frame, bone.origin))[2]];
        let tip = context.world(spatial_point(&frame, bone.tip));
        let vector = [tip[0] - origin[0], tip[1] - origin[1], tip[2] - origin[2]];
        let length = norm(&vector);
        let before = Self::spatial_pose(context, index).ok_or_else(unported)?;
        let local = spatial_quaternion(before.bend, before.turn, before.pitch);
        // Limbs lying in the view plane leave toward the character's front.
        let mut root_frame = frame;
        let mut at = index;
        while let Some(Spatial::Bone { parent, .. }) = context.figure.segments[at].rig.as_ref().and_then(|r| r.spatial.as_ref()) {
            if *parent == 0 {
                break;
            }
            let Some(p) = context.figure.plan.index_of.get(parent).map(|&p| p as usize) else { break };
            let Some(parent_frame) = context.frames.get(p).copied().flatten() else { break };
            root_frame = parent_frame;
            at = p;
        }
        let front = quat_rotate(spatial_world_quaternion(root_frame.q, context.figure_angle, context.mirror, false), [0.0, 0.0, 1.0])[2];
        let depth = if length > 1e-9 { vector[2] / length } else { 0.0 };
        let gesture = BoneGesture {
            origin,
            tip,
            length,
            direction: if length > 1e-9 { [vector[0] / length, vector[1] / length, vector[2] / length] } else { [1.0, 0.0, 0.0] },
            side: if depth.abs() > 0.04 { sign(depth) } else if front < -0.2 { -1.0 } else { 1.0 },
            world: spatial_world_quaternion(frame.q, context.figure_angle, context.mirror, false),
            parent: quat_multiply(frame.q, inverse_quaternion(local)),
            start: None,
            grab: None,
            native_length: None,
            pointer: None,
        };
        self.keep_gesture(key, Gesture::Bone(gesture.clone()));
        Ok(gesture)
    }

    fn keep_gesture(&mut self, key: String, gesture: Gesture) {
        if let Some(drag) = self.drag.as_mut() {
            drag.spatial_gestures.insert(key, gesture);
        }
    }

    /// `_setSpatialWorldOrientation(world, gesture, label, edit)`: the bone's channels for a
    /// world orientation.
    fn set_spatial_world_orientation(&mut self, context: &Context, index: usize, world: [f64; 4], gesture: &BoneGesture, label: &str, length: Option<f64>) -> Result<bool, EditError> {
        let before = Self::spatial_pose(context, index).ok_or_else(|| EditError::Unported("posing a 3D bone without a pose".into()))?;
        let bone = bone(&context.figure, index).unwrap();
        let pose = quaternion_pose(quat_multiply(inverse_quaternion(gesture.parent), spatial_world_quaternion(world, context.figure_angle, context.mirror, true)), before);
        let index_of = |id: u32| context.figure.plan.index_of.get(&id).map(|&i| i as usize);
        let channels = [(index_of(bone.turn), pose.turn - before.turn), (index_of(bone.pitch), pose.pitch - before.pitch)];
        let handle = context.figure.segments[index].id;
        let slot = context.slot;
        let lengths: Vec<Value> = context.figure.segments.iter().map(|s| Value::Num(s.length)).collect();
        let figure = context.figure.clone();
        self.object_edit_with(label, true, |editor| {
            crate::ik::release_spatial_chain(editor.frame_object_mut(slot).unwrap(), &figure, index);
            let sign = if editor.frame_objects()[slot].get("image_text_flags").is_some_and(Value::truthy) { -1.0 } else { 1.0 };
            editor.rotate_joint_by(slot, index, (pose.bend - before.bend) * sign, true, false)?;
            let record = editor.frame_object_mut(slot).unwrap();
            for (at, change) in channels {
                if let Some(at) = at {
                    crate::pose::add_angle_at(record, at, change * sign);
                }
            }
            plant_spatial_follower(record, handle, world);
            if let Some(length) = length {
                if !record.get("len_flag").is_some_and(Value::truthy) {
                    record.set("lengths", Value::array(lengths.clone()));
                }
                crate::pose::set_item_at(record, "lengths", index, length);
                record.set("len_flag", Value::Num(1.0));
            }
            Ok(())
        })
    }

    /// `_flatSpatialJoint(x, y, stretch, anchor)`: the bone turns about the view axis
    /// through its base, keeping its lean and twist.
    fn flat_spatial_joint(&mut self, context: &Context, index: usize, x: f64, y: f64, stretch: bool, anchor: Option<[f64; 2]>) -> Result<bool, EditError> {
        let mut gesture = self.bone_gesture(context, index, "flat")?;
        let depth = gesture.tip[2];
        let p = context.unproject([x, y], depth);
        let grab = *gesture.grab.get_or_insert_with(|| anchor.map_or([gesture.tip[0], gesture.tip[1]], |a| context.unproject(a, depth)));
        let length_flag = context.object.get("len_flag").is_some_and(Value::truthy);
        let native = *gesture.native_length.get_or_insert_with(|| if length_flag { number_at(&context.object, "lengths", index) } else { context.figure.segments[index].length });
        self.keep_gesture(format!("flat:{}:{index}", uid(&context.object).unwrap_or(0)), Gesture::Bone(gesture.clone()));
        let c = gesture.origin;
        let (gx, gy, px, py) = (grab[0] - c[0], grab[1] - c[1], p[0] - c[0], p[1] - c[1]);
        let (from, to) = (hypot(gx, gy), hypot(px, py));
        if from < 1e-6 || to < 1e-6 {
            return Ok(false);
        }
        let world = quat_multiply(spatial_quaternion(atan2(py, px) - atan2(gy, gx), 0.0, 0.0), gesture.world);
        self.set_spatial_world_orientation(context, index, world, &gesture, "Pose articulated joint", stretch.then(|| native * to / from))
    }

    /// `_arcSpatialJoint(x, y, flip, pixel, anchor)`: the bone keeps its length and swings in
    /// 3D so its end follows the pointer.
    #[allow(clippy::too_many_arguments)]
    fn arc_spatial_joint(&mut self, context: &Context, index: usize, x: f64, y: f64, flip: bool, pixel: f64, anchor: Option<[f64; 2]>) -> Result<bool, EditError> {
        let mut gesture = self.bone_gesture(context, index, "arc")?;
        if gesture.length < 1e-6 {
            return Ok(false);
        }
        let depth = gesture.origin[2] + gesture.direction[2] * gesture.length;
        let p = context.unproject([x, y], depth);
        let pointer = *gesture.pointer.get_or_insert_with(|| anchor.map_or([gesture.tip[0], gesture.tip[1]], |a| context.unproject(a, depth)));
        self.keep_gesture(format!("arc:{}:{index}", uid(&context.object).unwrap_or(0)), Gesture::Bone(gesture.clone()));
        let l = gesture.length.max(if pixel > 0.0 { 70.0 * pixel } else { 0.0 });
        let d = gesture.direction;
        let (dx, dy) = (d[0] * l + p[0] - pointer[0], d[1] * l + p[1] - pointer[1]);
        let reach = hypot(dx, dy);
        let direction = if reach >= l {
            [dx / reach, dy / reach, 0.0]
        } else {
            let side = gesture.side * if flip { -1.0 } else { 1.0 };
            [dx / l, dy / l, side * (l * l - reach * reach).sqrt() / l]
        };
        if reach < 1e-9 && direction[2].abs() < 1e-9 {
            return Ok(false);
        }
        self.set_spatial_world_orientation(context, index, quat_multiply(between_vectors(d, direction), gesture.world), &gesture, "Pose articulated joint", None)
    }

    /// `twistSpatialJoint(x, y, {pixel})`: the bone twists about its own axis; 120 screen
    /// pixels turn a quarter.
    pub(crate) fn twist_spatial_joint(&mut self, x: f64, y: f64, pixel: f64) -> Result<bool, EditError> {
        let Some(context) = self.spatial_context()? else { return Ok(false) };
        let index = self.selection.joint as usize;
        let Some(bone) = bone(&context.figure, index) else { return Ok(false) };
        if self.drag.is_none() || bone.motion.is_some() {
            return Ok(false);
        }
        if !x.is_finite() || !y.is_finite() || !pixel.is_finite() || pixel <= 0.0 {
            return Ok(false);
        }
        if crate::ik::spatial_ik_at(&context.object, &context.figure, index) {
            return Err(EditError::Unported("twisting with 3D IK".into()));
        }
        let mut gesture = self.bone_gesture(&context, index, "twist")?;
        let start = *gesture.start.get_or_insert([x, y]);
        self.keep_gesture(format!("twist:{}:{index}", uid(&context.object).unwrap_or(0)), Gesture::Bone(gesture.clone()));
        if gesture.length < 1e-6 {
            return Ok(false);
        }
        let angle = (x - start[0]) / pixel / 120.0 * std::f64::consts::PI / 2.0 * if context.mirror { -1.0 } else { 1.0 };
        let (d, h) = (gesture.direction, sin(angle / 2.0));
        let about = [d[0] * h, d[1] * h, d[2] * h, cos(angle / 2.0)];
        self.set_spatial_world_orientation(&context, index, quat_multiply(about, gesture.world), &gesture, "Twist articulated joint", None)
    }

    /// `_moveMechanicalJoint(x, y, anchor)`: the slide or hinge moves to the point of its track
    /// nearest the pointer, relative to where its handle was grabbed.
    fn move_mechanical_joint(&mut self, context: &Context, index: usize, x: f64, y: f64, anchor: Option<[f64; 2]>) -> Result<bool, EditError> {
        let Some(motion) = Self::spatial_motion(context, index) else { return Ok(false) };
        if motion.max <= motion.min {
            return Ok(false);
        }
        let key = format!("mechanism:{}:{index}", uid(&context.object).unwrap_or(0));
        let gesture = match self.drag.as_ref().and_then(|d| d.spatial_gestures.get(&key)) {
            Some(Gesture::Mechanism(gesture)) => gesture.clone(),
            _ => {
                let unported = || EditError::Unported("moving a mechanism geometry did not frame".into());
                let bone = bone(&context.figure, index).ok_or_else(unported)?;
                let frame = context.frames.get(index).copied().flatten().ok_or_else(unported)?;
                let (tip, pivot) = (spatial_point(&frame, bone.tip), spatial_point(&frame, bone.origin));
                let local_axis = if motion.hinge { bone.motion.and_then(|m| m.axis).unwrap_or([0.0, 0.0, 1.0]) } else { frame.axis };
                let mut gesture = MechanismGesture {
                    slide: !motion.hinge,
                    tip,
                    pivot,
                    axis: quat_rotate(frame.q, local_axis),
                    value: motion.value,
                    start: [0.0; 3],
                    grab: [0.0; 2],
                    figure_angle: context.figure_angle,
                    mirror: context.mirror,
                    scale: context.scale,
                    root: context.root,
                };
                gesture.start = gesture.along(motion.value);
                gesture.grab = anchor.map_or([gesture.start[0], gesture.start[1]], |a| context.unproject(a, gesture.start[2]));
                self.keep_gesture(key, Gesture::Mechanism(gesture.clone()));
                gesture
            }
        };
        let pointer = context.unproject([x, y], gesture.start[2]);
        let target = [gesture.start[0] + pointer[0] - gesture.grab[0], gesture.start[1] + pointer[1] - gesture.grab[1]];
        let distance = |p: f64| {
            let q = gesture.along(p);
            (q[0] - target[0]).powi(2) + (q[1] - target[1]).powi(2)
        };
        // Sample the track, then refine between the neighbours of the nearest sample.
        let steps = 96;
        let at = |k: usize| motion.min + (motion.max - motion.min) * k as f64 / steps as f64;
        let mut best = 0;
        for k in 1..=steps {
            if distance(at(k)) < distance(at(best)) {
                best = k;
            }
        }
        let (mut lo, mut hi) = (at(best.saturating_sub(1)), at((best + 1).min(steps)));
        for _ in 0..40 {
            let (a, b) = (lo + (hi - lo) / 3.0, hi - (hi - lo) / 3.0);
            if distance(a) <= distance(b) {
                hi = b;
            } else {
                lo = a;
            }
        }
        // Past either end the pointer lands exactly on that stop.
        let value = [motion.min, motion.max].into_iter().fold((lo + hi) / 2.0, |v, end| if distance(end) <= distance(v) { end } else { v });
        Ok(self.set_spatial_motion(value, false)? || true)
    }
}

/// Whether a pinned automatic 3D controller drives a two-bone limb that starts at `index`.
fn planted_limb_base(context: &Context, index: usize) -> bool {
    let figure = &context.figure;
    let skeleton = styk_eval::ik::Skeleton { segments: &figure.segments, plan: &figure.plan, polygons: &figure.polyfills, sources: None };
    context.object.get("_stykIK").and_then(Value::as_obj).is_some_and(|table| {
        table.iter().any(|(id, c)| {
            let truthy = |k: &str| c.get(k).is_some_and(Value::truthy);
            let Ok(handle) = id.parse::<u32>() else { return false };
            if !(truthy("enabled") && truthy("pinned") && truthy("spatialTarget")) || c.get("bendDirection").and_then(Value::as_f64) != Some(0.0) {
                return false;
            }
            let length = c.get("chainLength").and_then(Value::as_f64).unwrap_or(2.0).max(0.0).min(u32::MAX as f64) as u32;
            let chain = skeleton.chain(handle, length);
            chain.len() == 2 && chain[0] == index
        })
    })
}
