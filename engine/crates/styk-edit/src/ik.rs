//! Inverse-kinematics controllers as editing sees them (src/ik.mjs, src/editor-ik.mjs).
//!
//! Ported: moving controller targets with their figure (`translateIKTargets`), dragging
//! the end of a limb (`poseIKTarget`: a pinned target moves, an unpinned limb is solved
//! and baked), and what gestures do around pinned limbs: the displayed solution baked
//! before a pose edit (`_objectEdit(…, {posed})`, `_bakeIKFollowers`), follower offsets
//! turned with their joint (`_rotateIKFollowers`), the bend remembered after each change
//! (`_rememberIKBends`) and a stretched end followed by its pin (`_followStretchedIK`).
//! Across joined figures the same runs on the assembly's temporary skeleton
//! (`styk_eval::ik::joined::Assembly`: `context()`, `_ikPoseSlots`, `poseGraph`).
//! 3D controllers answer `Unported` (the gate below).

use std::collections::{BTreeMap, HashMap, HashSet};

use styk_doc::{Obj, Value};
use styk_eval::eval::{Frame, Prepared};
use styk_eval::geometry::Scene;
use styk_eval::ik::joined::{Assembly, Followers};
use styk_eval::ik::{self, BendReference, Controller, Pose, Skeleton, Solution, Table, Target};
use styk_math::{atan2, cos, hypot, sin};

use crate::frame::objects_of;
use crate::record::{join_target, uid};
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
        if !GESTURES.contains(&method) {
            return Ok(None);
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
        self.object_edit("Move IK target", |editor| {
            let key = handle.to_string();
            let current = editor.frame_objects().get(slot).and_then(|o| o.path(&["_stykIK", &key]).cloned()).unwrap_or(Value::Null);
            if !truthy(&current, "pinned") {
                return if editor.in_assembly(slot)? { editor.pose_graph(slot, handle, x, y) } else { editor.bake_unpinned(slot, handle, x, y) };
            }
            // A pinned target persists where it is put; geometry solves the limb to it.
            let mut next = current.as_obj().cloned().unwrap_or_default();
            let mut target = current.get("target").and_then(Value::as_obj).cloned().unwrap_or_default();
            target.set("x", Value::Num(x));
            target.set("y", Value::Num(y));
            next.set("target", Value::object(target));
            let record = editor.frame_object_mut(slot).ok_or_else(|| EditError::Unported("posing an IK target outside the frame".into()))?;
            set_controller(record, &key, next);
            record.remove("runtime_segments");
            Ok(())
        })
    }

    /// `layerLocked(report, uid)` without focus locks, which the facade keeps in JavaScript.
    fn layer_locked(&self, uid: u32) -> bool {
        matches!(self.report.path(&["_styk", "layers", "byId", &self.layer_of(uid), "locked"]), Some(Value::Bool(true)))
    }

    /// The pinned limbs of the frame being edited as geometry solves them, by figure
    /// identity (`geometry.objects.get(slot).ik`, `solvedAngles`, `solvedLengths`).
    fn solutions(&self) -> Result<HashMap<u32, Solved>, EditError> {
        let (prepared, frame, scene) = self.evaluate()?;
        Ok(solve_frame(&prepared, &frame, &scene))
    }

    /// `context()` is a graph: the figure in `slot` belongs to joined figures that compile
    /// into one skeleton.
    fn in_assembly(&self, slot: usize) -> Result<bool, EditError> {
        let objects = self.selection_frame()?;
        if !objects.iter().any(|o| o.get("join").is_some_and(Value::truthy)) {
            return Ok(false);
        }
        let (prepared, frame, _) = self.evaluate()?;
        Ok(assembly_of(&prepared, &frame, slot).is_some())
    }

    /// `_ikPoseSlots(frame, slots)`: with IK on joined figures, a gesture takes in every
    /// figure of an assembly it touches that has an enabled controller.
    pub(crate) fn ik_pose_slots(objects: &[Value], slots: Vec<usize>) -> Vec<usize> {
        let enabled = |object: &Value| controllers(object).iter().any(|(_, c)| truthy(c, "enabled"));
        if !objects.iter().any(|o| o.get("join").is_some_and(Value::truthy)) || !objects.iter().any(enabled) {
            return slots;
        }
        let mut affected: std::collections::BTreeSet<usize> = slots.into_iter().collect();
        let joins: Vec<Option<usize>> = objects.iter().map(join_target).collect();
        for group in ik::joined::components_of(&joins) {
            if group.iter().any(|slot| affected.contains(slot)) && group.iter().any(|&slot| enabled(&objects[slot])) {
                affected.extend(group);
            }
        }
        affected.into_iter().collect()
    }

    /// `_expandIKDrag(slots)`: the gesture's snapshots take in these figures too.
    fn expand_drag(&mut self, slots: &[usize]) {
        let objects = self.frame_objects();
        let Some(drag) = self.drag.as_mut() else { return };
        let slot_of = |o: &Value| o.get("object_slot").and_then(Value::as_f64).unwrap_or(f64::NAN);
        for &slot in slots {
            if !drag.before.iter().any(|o| slot_of(o) == slot as f64)
                && let Some(object) = objects.get(slot)
            {
                drag.before.push(object.clone());
            }
        }
        drag.before.sort_by(|a, b| slot_of(a).total_cmp(&slot_of(b)));
    }

    /// The joined branch of `poseIKTarget` for an unpinned controller (`poseGraph`): the
    /// limb is captured across the assembly (`captureFor`), solved with the captured
    /// controller pinned at the pointer, and every figure of the assembly takes its part
    /// of the solution.
    fn pose_graph(&mut self, slot: usize, handle: u32, x: f64, y: f64) -> Result<(), EditError> {
        let unported = |what: &str| EditError::Unported(format!("posing a joined IK target {what}"));
        let (prepared, frame, scene) = self.evaluate()?;
        let mut assembly = assembly_of(&prepared, &frame, slot).ok_or_else(|| unported("outside an assembly"))?;
        self.expand_drag(&assembly.slots.clone());
        let solved = solve_frame(&prepared, &frame, &scene);
        let object = self.frame_objects().get(slot).cloned().ok_or_else(|| unported("outside the frame"))?;
        let key = handle.to_string();
        let current = object.path(&["_stykIK", &key]).cloned().unwrap_or(Value::Null);
        let controller = ik::parse::table(object.get("_stykIK")).and_then(|t| t.get(handle).cloned()).ok_or_else(|| unported("with a controller the engine cannot read"))?;
        let id = assembly.id(slot, handle).ok_or_else(|| unported("of a handle outside the assembly"))?;
        let geometry_of = |slot: usize| {
            let uid = frame.objects[slot].uid;
            scene.objects.iter().find(|g| g.uid == uid && g.resolved)
        };
        // captureFor(editor, geometry, handle, controller, graph)
        let chain = assembly.chain(slot, handle, controller.chain_length);
        if chain.is_empty() {
            return Err(unported("without a chain"));
        }
        let rotation = controller.target.rotation;
        let (mut followers, mut joined) = (BTreeMap::new(), BTreeMap::<u32, BTreeMap<u32, f64>>::new());
        for i in assembly.skeleton().follower_indices(id) {
            let entry = assembly.entry(i);
            let angle = displayed_angle(&prepared, &frame, &solved, entry.slot, entry.index) - rotation;
            if entry.slot == slot {
                followers.insert(entry.handle, angle);
            } else {
                joined.entry(entry.uid).or_default().insert(entry.handle, angle);
            }
        }
        let mut points = Vec::with_capacity(chain.len() + 1);
        let mut lengths = Vec::with_capacity(chain.len());
        for (k, &i) in chain.iter().enumerate() {
            let entry = assembly.entry(i);
            let geometry = geometry_of(entry.slot).ok_or_else(|| unported("of a figure the engine does not draw"))?;
            let (start, end) = (geometry.starts[entry.index], geometry.ends[entry.index]);
            if k == 0 {
                points.push(start);
            }
            points.push(end);
            lengths.push(hypot(end[0] - start[0], end[1] - start[1]));
        }
        let captured = ik::bend_reference(&points, &lengths, controller.bend_reference.as_ref());
        let temporary = Controller {
            pinned: true,
            target: Target { x, y, rotation },
            followers: followers.clone(),
            joined_followers: Some(joined.clone()),
            bend_reference: Some(captured.clone()),
            ..controller
        };
        // The lengths the gesture began with, scaled into the skeleton.
        let before = self.drag.as_ref().map(|d| d.before.clone()).unwrap_or_default();
        let baseline_of = |slot: usize| before.iter().find(|o| o.get("object_slot").and_then(Value::as_f64) == Some(slot as f64));
        for i in 0..assembly.lengths_mut().len() {
            let entry = assembly.entry(i);
            let baseline = baseline_of(entry.slot).ok_or_else(|| unported("outside the gesture"))?;
            let length = if baseline.get("len_flag").is_some_and(Value::truthy) {
                baseline.get("lengths").and_then(|l| l.item(entry.index)).and_then(|v| v.as_f64()).unwrap_or(f64::NAN)
            } else {
                frame.objects[entry.slot].figure_override.as_ref().or_else(|| prepared.doc.figure(frame.objects[entry.slot].figure)).map_or(f64::NAN, |f| f.segments[entry.index].length)
            };
            assembly.lengths_mut()[i] = length * entry.scale;
        }
        let mapped = assembly.map_controller(slot, id, &temporary);
        let table = Table { controllers: BTreeMap::from([(id, mapped)]) };
        let solution = assembly.solve(&table).ok_or_else(|| unported("the solver did not reach"))?;
        let bend = solution.results.iter().find(|(h, _)| *h == id).and_then(|(_, o)| o.bend_reference.clone()).unwrap_or(captured);
        for part in assembly.scatter(&solution, &table) {
            let baseline = baseline_of(part.slot).cloned().ok_or_else(|| unported("outside the gesture"))?;
            let record = self.frame_object_mut(part.slot).ok_or_else(|| unported("outside the frame"))?;
            record.set("angles", Value::nums(&part.solution.angles));
            match (&part.solution.lengths, baseline.get("len_flag")) {
                (Some(_), _) => record.set("len_flag", Value::Num(1.0)),
                (None, Some(flag)) => record.set("len_flag", flag.clone()),
                (None, None) => {
                    record.remove("len_flag");
                }
            }
            match (&part.solution.lengths, baseline.get("lengths")) {
                (Some(lengths), _) => record.set("lengths", Value::nums(lengths)),
                (None, Some(lengths)) => record.set("lengths", lengths.clone()),
                (None, None) => {
                    record.remove("lengths");
                }
            }
            record.remove("runtime_segments");
        }
        // `Object.assign(next, {...captured, target, bendReference})`
        let mut next = current.as_obj().cloned().unwrap_or_default();
        let mut target = current.get("target").and_then(Value::as_obj).cloned().unwrap_or_default();
        target.set("x", Value::Num(x));
        target.set("y", Value::Num(y));
        next.set("target", Value::object(target));
        next.set("followers", angle_table(&followers));
        next.set("joinedFollowers", joined_table(&joined));
        match current.get("bendDirection") {
            Some(direction) => next.set("bendDirection", direction.clone()),
            None => {
                next.remove("bendDirection");
            }
        }
        next.set("bendReference", reference_value(&bend));
        let record = self.frame_object_mut(slot).ok_or_else(|| unported("outside the frame"))?;
        set_controller(record, &key, next);
        record.remove("runtime_segments");
        Ok(())
    }

    /// The posed half of `_objectEdit`: every figure of the gesture with a pinned limb takes
    /// the angles it is displayed with, and its controllers the followers of that solution.
    pub(crate) fn bake_pinned(&mut self) -> Result<(), EditError> {
        let Some(drag) = self.drag.as_ref() else { return Ok(()) };
        let before = drag.before.clone();
        if !before.iter().any(has_pinned) {
            return Ok(());
        }
        let solved = self.solutions()?;
        for snapshot in &before {
            let Some(id) = uid(snapshot) else { continue };
            if self.layer_locked(id) {
                continue;
            }
            let objects = self.frame_objects();
            let Some(slot) = objects.iter().position(|o| uid(o) == Some(id)) else { continue };
            let Some(Solved { solution, followers, joined }) = solved.get(&id) else { continue };
            if objects[slot].get("angles").and_then(Value::array_len) != Some(solution.angles.len()) {
                return Err(EditError::Unported("baking IK into a pose without one angle per segment".into()));
            }
            let mut controllers = objects[slot].get("_stykIK").and_then(Value::as_obj).cloned().unwrap_or_default();
            let mut changed = false;
            for (handle, outcome) in &solution.results {
                let key = handle.to_string();
                let Some(c) = controllers.get(&key).filter(|c| truthy(c, "enabled") && truthy(c, "pinned")) else { continue };
                let (Some((own, others)), false) = (followers.get(handle), outcome.conflict) else { continue };
                let mut next = c.as_obj().cloned().unwrap_or_default();
                next.set("followers", angle_table(own));
                if *joined {
                    next.set("joinedFollowers", joined_table(others));
                }
                controllers.set(&key, Value::object(next));
                changed = true;
            }
            let record = self.frame_object_mut(slot).unwrap();
            if changed {
                record.set("_stykIK", Value::object(controllers));
            }
            record.set("angles", Value::nums(&solution.angles));
        }
        Ok(())
    }

    /// `_rememberIKBends`: after a change, each pinned automatic ("Both") controller of the
    /// gesture keeps the bend its limb is now solved with, so the next solve continues
    /// from it. A bend within 1e-7 of the one the gesture started with stays as it was.
    pub(crate) fn remember_bends(&mut self) -> Result<(), EditError> {
        let Some(drag) = self.drag.as_ref() else { return Ok(()) };
        let before = drag.before.clone();
        let objects = self.frame_objects();
        let mut solved: Option<HashMap<u32, Solved>> = None;
        for snapshot in &before {
            let Some(slot) = snapshot.get("object_slot").and_then(Value::as_f64).filter(|s| *s >= 0.0).map(|s| s as usize) else { continue };
            let Some(object) = objects.get(slot) else { continue };
            let Some(id) = uid(object).filter(|&u| Some(u) == uid(snapshot)) else { continue };
            if self.layer_locked(id) {
                continue;
            }
            for (key, c) in controllers(object) {
                if !(truthy(&c, "enabled") && truthy(&c, "pinned") && c.get("bendDirection").and_then(Value::as_f64) == Some(0.0)) {
                    continue;
                }
                if solved.is_none() {
                    solved = Some(self.solutions()?);
                }
                let Ok(handle) = key.parse::<u32>() else { continue };
                let outcome = solved.as_ref().unwrap().get(&id).and_then(|s| s.solution.results.iter().find(|(h, _)| *h == handle));
                let Some(reference) = outcome.and_then(|(_, o)| o.bend_reference.as_ref()) else { continue };
                let value = match snapshot.path(&["_stykIK", &key, "bendReference"]) {
                    Some(baseline) if same_bend(reference, baseline) => baseline.clone(),
                    _ => reference_value(reference),
                };
                let mut next = c.as_obj().cloned().unwrap_or_default();
                next.set("bendReference", value);
                set_controller(self.frame_object_mut(slot).unwrap(), &key, next);
            }
        }
        Ok(())
    }

    /// `_rotateIKFollowers(targets, delta, includeJoint)`: turning a joint of a pinned
    /// controller's outgoing group by hand changes the angle that group keeps relative
    /// to the target; turning an ancestor leaves the target's orientation planted.
    pub(crate) fn rotate_ik_followers(&mut self, slot: usize, joint: usize, targets: &[(usize, usize)], delta: f64, include_joint: bool) -> Result<(), EditError> {
        if delta == 0.0 || delta.is_nan() || !self.drag.as_ref().is_some_and(|d| d.before.iter().any(has_pinned)) {
            return Ok(());
        }
        let unported = |what: &str| EditError::Unported(format!("turning the followers of an IK target {what}"));
        let selected: HashSet<(usize, usize)> = targets.iter().copied().collect();
        if self.in_assembly(slot)? {
            return self.rotate_assembly_followers(slot, joint, &selected, delta, include_joint);
        }
        let source = self.frame_objects().get(slot).cloned().ok_or_else(|| unported("outside the frame"))?;
        let prepared = self.prepared()?;
        let figure = source.get("figure_type_id").and_then(Value::as_f64).and_then(|id| prepared.doc.figure(id as u32)).ok_or_else(|| unported("of a figure without a definition"))?;
        let skeleton = Skeleton { segments: &figure.segments, plan: &figure.plan, polygons: &figure.polyfills, sources: None };
        let angles = source.get("angles").and_then(Value::numbers).filter(|a| a.len() >= figure.segments.len()).ok_or_else(|| unported("of a pose without every angle"))?;
        let table = ik::parse::table(source.get("_stykIK"));
        for (key, c) in controllers(&source) {
            if !(truthy(&c, "enabled") && truthy(&c, "pinned")) {
                continue;
            }
            let Some(id) = key.parse::<u32>().ok() else { continue };
            let Some(endpoint) = skeleton.index(id) else { continue };
            if selected.contains(&(slot, endpoint)) {
                continue;
            }
            let controller = table.as_ref().and_then(|t| t.get(id)).ok_or_else(|| unported("with a controller the engine cannot read"))?;
            let followers = skeleton.followers(&angles, id, controller);
            let current = self.frame_objects()[slot].path(&["_stykIK", &key]).cloned().unwrap_or(Value::Null);
            let mut next: Option<Obj> = None;
            for i in skeleton.follower_indices(id) {
                if (!include_joint && i == joint) || !selected.contains(&(slot, i)) {
                    continue;
                }
                let handle = figure.segments[i].id;
                if current.path(&["spatialFollowers", &handle.to_string()]).is_some_and(Value::truthy) {
                    return Err(unported("with 3D followers"));
                }
                let next = next.get_or_insert_with(|| {
                    let mut next = current.as_obj().cloned().unwrap_or_default();
                    for key in ["followers", "joinedFollowers"] {
                        next.set(key, Value::object(current.get(key).and_then(Value::as_obj).cloned().unwrap_or_default()));
                    }
                    next
                });
                let mut table = next.get("followers").and_then(Value::as_obj).cloned().unwrap_or_default();
                assign(&mut table, &handle.to_string(), Value::Num(followers.get(&handle).copied().unwrap_or(f64::NAN) + delta));
                next.set("followers", Value::object(table));
            }
            if let Some(next) = next {
                set_controller(self.frame_object_mut(slot).unwrap(), &key, next);
            }
        }
        Ok(())
    }

    /// `_rotateIKFollowers` on an assembly (`context()` is a graph): every pinned
    /// controller of the joined figures, its followers in their own figures or in others.
    fn rotate_assembly_followers(&mut self, slot: usize, joint: usize, selected: &HashSet<(usize, usize)>, delta: f64, include_joint: bool) -> Result<(), EditError> {
        let unported = |what: &str| EditError::Unported(format!("turning the followers of a joined IK target {what}"));
        let (prepared, frame, _) = self.evaluate()?;
        let assembly = assembly_of(&prepared, &frame, slot).ok_or_else(|| unported("outside an assembly"))?;
        let skeleton = assembly.skeleton();
        for (&id, c) in &assembly.controllers.controllers {
            if !c.active() {
                continue;
            }
            let Some(endpoint) = skeleton.index(id) else { continue };
            let owner = assembly.entry(endpoint);
            if selected.contains(&(owner.slot, owner.index)) {
                continue;
            }
            let key = owner.handle.to_string();
            let current = self.frame_objects().get(owner.slot).and_then(|o| o.path(&["_stykIK", &key]).cloned()).unwrap_or(Value::Null);
            let followers = skeleton.followers(assembly.angles(), id, c);
            let mut next: Option<Obj> = None;
            for i in skeleton.follower_indices(id) {
                let child = assembly.entry(i);
                if (!include_joint && child.slot == slot && child.index == joint) || !selected.contains(&(child.slot, child.index)) {
                    continue;
                }
                let spatial = if child.slot == owner.slot { vec!["spatialFollowers".to_string(), child.handle.to_string()] } else { vec!["joinedSpatialFollowers".to_string(), child.uid.to_string(), child.handle.to_string()] };
                if current.path(&spatial.iter().map(String::as_str).collect::<Vec<_>>()).is_some_and(Value::truthy) {
                    return Err(unported("with 3D followers"));
                }
                let next = next.get_or_insert_with(|| {
                    let mut next = current.as_obj().cloned().unwrap_or_default();
                    for key in ["followers", "joinedFollowers"] {
                        next.set(key, Value::object(current.get(key).and_then(Value::as_obj).cloned().unwrap_or_default()));
                    }
                    next
                });
                let angle = Value::Num(followers.get(&skeleton.segments[i].id).copied().unwrap_or(f64::NAN) + delta);
                if child.slot == owner.slot {
                    let mut table = next.get("followers").and_then(Value::as_obj).cloned().unwrap_or_default();
                    assign(&mut table, &child.handle.to_string(), angle);
                    next.set("followers", Value::object(table));
                } else {
                    let mut table = next.get("joinedFollowers").and_then(Value::as_obj).cloned().unwrap_or_default();
                    let mut inner = table.get(&child.uid.to_string()).and_then(Value::as_obj).cloned().unwrap_or_default();
                    assign(&mut inner, &child.handle.to_string(), angle);
                    assign(&mut table, &child.uid.to_string(), Value::object(inner));
                    next.set("joinedFollowers", Value::object(table));
                }
            }
            if let Some(next) = next {
                set_controller(self.frame_object_mut(owner.slot).unwrap(), &key, next);
            }
        }
        Ok(())
    }

    /// `_ikLimbPoints(handle)` of a pinned controller as displayed (its base to the joint
    /// before the end), with its chain and the stretched lengths it is displayed with.
    pub(crate) fn pinned_limb(&self, object: &Value, handle: u32) -> Result<PinnedLimb, EditError> {
        let unported = |what: &str| EditError::Unported(format!("stretching a pinned IK limb {what}"));
        let (prepared, frame, scene) = self.evaluate()?;
        let id = uid(object).ok_or_else(|| unported("without an identity"))?;
        let slot = frame.objects.iter().position(|o| o.uid == id).ok_or_else(|| unported("the engine does not draw"))?;
        let evaluated = &frame.objects[slot];
        let figure = evaluated.figure_override.as_deref().or_else(|| prepared.doc.figure(evaluated.figure).map(|f| &**f)).ok_or_else(|| unported("without a definition"))?;
        let controller = ik::parse::table(object.get("_stykIK")).and_then(|t| t.get(handle).cloned()).ok_or_else(|| unported("with a controller the engine cannot read"))?;
        // The limb's bones: in the assembly's skeleton when the figure is joined to others.
        let joined = self.selection_frame()?.iter().any(|o| o.get("join").is_some_and(Value::truthy));
        let assembly = if joined { assembly_of(&prepared, &frame, slot) } else { None };
        let bones: Vec<(usize, usize)> = match &assembly {
            Some(assembly) => assembly.chain(slot, handle, controller.chain_length).into_iter().map(|i| (assembly.entry(i).slot, assembly.entry(i).index)).collect(),
            None => {
                let skeleton = Skeleton { segments: &figure.segments, plan: &figure.plan, polygons: &figure.polyfills, sources: None };
                skeleton.chain(handle, controller.chain_length).into_iter().map(|i| (slot, i)).collect()
            }
        };
        if bones.is_empty() {
            return Err(unported("without a chain"));
        }
        let geometry_of = |slot: usize| scene.objects.iter().find(|g| g.uid == frame.objects[slot].uid && g.resolved);
        let mut points = Vec::with_capacity(bones.len());
        for (k, &(s, i)) in bones.iter().enumerate() {
            let geometry = geometry_of(s).ok_or_else(|| unported("the engine does not draw"))?;
            if k == 0 {
                points.push(geometry.starts[i]);
            }
            if k + 1 < bones.len() {
                points.push(geometry.ends[i]);
            }
        }
        // `ikChain(this.figure, handle, pin.chainLength)`: the limb in the figure's own bones.
        let skeleton = Skeleton { segments: &figure.segments, plan: &figure.plan, polygons: &figure.polyfills, sources: None };
        let chain = skeleton.chain(handle, controller.chain_length);
        let solved_lengths = solve_frame(&prepared, &frame, &scene).remove(&id).and_then(|s| s.solution.lengths);
        Ok(PinnedLimb { points, chain, solved_lengths })
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
        let solved = solve_frame(&prepared, &frame, &scene);
        let at = frame.objects.iter().position(|o| o.uid == id).unwrap();
        let drawn = |i: usize| displayed_angle(&prepared, &frame, &solved, at, i);

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
            shape: evaluated.spatial.as_ref().and_then(|s| s.shape.as_ref()),
        };
        let table = Table { controllers: BTreeMap::from([(handle, temporary)]) };
        let solution = ik::solve_object(&skeleton, &pose, geometry.root, &table).ok_or_else(|| unported("the solver did not reach"))?;
        let solved = solution.results.iter().find(|(h, _)| *h == handle).and_then(|(_, outcome)| outcome.bend_reference.clone());

        let mut next = current.as_obj().cloned().unwrap_or_default();
        let mut target = current.get("target").and_then(Value::as_obj).cloned().unwrap_or_default();
        target.set("x", Value::Num(x));
        target.set("y", Value::Num(y));
        next.set("target", Value::object(target));
        next.set("followers", angle_table(&followers));
        next.set("bendReference", reference_value(solved.as_ref().unwrap_or(&captured)));
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
        set_controller(record, &key, next);
        record.remove("runtime_segments");
        Ok(())
    }
}

/// A frame's pinned solution of one figure and each controller's followers (by handle),
/// solved on its own or as part of an assembly (`joined`).
pub(crate) struct Solved {
    pub solution: Solution,
    followers: BTreeMap<u32, Followers>,
    joined: bool,
}

/// The figures' typed views by slot, as geometry resolves them.
fn figure_of<'a>(prepared: &'a Prepared, frame: &'a Frame, slot: usize) -> Option<&'a std::sync::Arc<styk_eval::doc::Figure>> {
    let object = frame.objects.get(slot)?;
    object.figure_override.as_ref().or_else(|| prepared.doc.figure(object.figure))
}

/// `context()`: the assembly of joined figures that `slot` belongs to, when there is one
/// of two figures or more and it compiles.
fn assembly_of<'a>(prepared: &'a Prepared, frame: &'a Frame, slot: usize) -> Option<Assembly<'a>> {
    if !frame.objects.iter().any(|o| o.join.is_some()) {
        return None;
    }
    let slots = ik::joined::components(frame).into_iter().find(|slots| slots.contains(&slot)).filter(|slots| slots.len() > 1)?;
    Assembly::new(frame, &slots, &|slot| figure_of(prepared, frame, slot))
}

/// The pinned limbs of a frame solved as geometry solves them (`solveJoinedIK`, then
/// `evaluateIKPose` for the other figures with a pinned controller), by figure identity.
pub(crate) fn solve_frame(prepared: &Prepared, frame: &Frame, scene: &Scene) -> HashMap<u32, Solved> {
    let mut solved = HashMap::new();
    if !frame.objects.iter().any(|o| ik::has_pinned(o)) {
        return solved;
    }
    if frame.objects.iter().any(|o| o.join.is_some()) {
        for slots in ik::joined::components(frame) {
            if slots.len() < 2 || !slots.iter().any(|&slot| ik::has_pinned(&frame.objects[slot])) {
                continue;
            }
            let Some(assembly) = Assembly::new(frame, &slots, &|slot| figure_of(prepared, frame, slot)) else { continue };
            let Some(solution) = assembly.solve(&assembly.controllers) else { continue };
            for part in assembly.scatter(&solution, &assembly.controllers) {
                solved.insert(frame.objects[part.slot].uid, Solved { solution: part.solution, followers: part.followers, joined: true });
            }
        }
    }
    for (slot, object) in frame.objects.iter().enumerate() {
        if !ik::has_pinned(object) || solved.contains_key(&object.uid) {
            continue;
        }
        let Some(figure) = figure_of(prepared, frame, slot) else { continue };
        // Where geometry draws the figure from: a joined figure hangs from its parent's joint.
        let root = scene.objects.iter().find(|g| g.uid == object.uid).map_or(object.root, |g| g.root);
        let Some(solution) = ik::solve_local(figure, object, root) else { continue };
        let table = object.ik.as_ref().unwrap();
        let skeleton = Skeleton { segments: &figure.segments, plan: &figure.plan, polygons: &figure.polyfills, sources: None };
        let angles: Vec<f64> = figure.segments.iter().enumerate().map(|(i, s)| object.angles.get(i).copied().unwrap_or(s.angle)).collect();
        // `results.get(id).followers`: `ikFollowers` of each controller that solved.
        let followers = solution
            .results
            .iter()
            .filter(|(_, outcome)| !outcome.conflict)
            .filter_map(|(id, _)| Some((*id, (skeleton.followers(&angles, *id, table.get(*id)?).into_iter().collect(), BTreeMap::new()))))
            .collect();
        solved.insert(object.uid, Solved { solution, followers, joined: false });
    }
    solved
}

/// The angle geometry draws segment `index` of the figure in `slot` with: solved for a
/// pinned limb (`ik.angles[i] ?? obj.angles[i]`).
pub(crate) fn displayed_angle(prepared: &Prepared, frame: &Frame, solved: &HashMap<u32, Solved>, slot: usize, index: usize) -> f64 {
    let object = &frame.objects[slot];
    if let Some(angle) = solved.get(&object.uid).and_then(|s| s.solution.angles.get(index)) {
        return *angle;
    }
    match figure_of(prepared, frame, slot).and_then(|f| f.segments.get(index)) {
        Some(segment) => styk_eval::geometry::segment_pose(object, segment, index).0,
        None => object.angles.get(index).copied().unwrap_or(f64::NAN),
    }
}

/// Followers of figures joined to a controller's owner, as the editor stores them
/// (`joinedFollowers`: by identity, then by handle).
fn joined_table(joined: &BTreeMap<u32, BTreeMap<u32, f64>>) -> Value {
    let mut table = Obj::new();
    for (uid, angles) in joined {
        table.set(&uid.to_string(), angle_table(angles));
    }
    Value::object(table)
}

/// A pinned limb as displayed, for a stretch drag on its end.
pub(crate) struct PinnedLimb {
    pub points: Vec<[f64; 2]>,
    pub chain: Vec<usize>,
    pub solved_lengths: Option<Vec<f64>>,
}

/// `hasPinnedIK(object)`.
fn has_pinned(object: &Value) -> bool {
    controllers(object).iter().any(|(_, c)| truthy(c, "enabled") && truthy(c, "pinned"))
}

/// `_followStretchedIK(object, handle, limb, tip)`: the pin of a stretched end follows it
/// and keeps the bend now shown.
pub(crate) fn follow_stretched(record: &mut Obj, handle: u32, limb: &[[f64; 2]], tip: [f64; 2]) {
    let key = handle.to_string();
    let current = record.get("_stykIK").and_then(|t| t.get(&key)).cloned().unwrap_or(Value::Null);
    let previous = ik::parse::table(record.get("_stykIK")).and_then(|t| t.get(handle).and_then(|c| c.bend_reference.clone()));
    let points: Vec<[f64; 2]> = limb.iter().copied().chain(std::iter::once(tip)).collect();
    let lengths: Vec<f64> = points.windows(2).map(|w| hypot(w[1][0] - w[0][0], w[1][1] - w[0][1])).collect();
    let reference = ik::bend_reference(&points, &lengths, previous.as_ref());
    let mut next = current.as_obj().cloned().unwrap_or_default();
    let mut target = current.get("target").and_then(Value::as_obj).cloned().unwrap_or_default();
    target.set("x", Value::Num(tip[0]));
    target.set("y", Value::Num(tip[1]));
    next.set("target", Value::object(target));
    next.set("bendReference", reference_value(&reference));
    // `bendDirection: c.bendDirection && bendReference.side`
    match current.get("bendDirection") {
        Some(direction) if direction.truthy() => next.set("bendDirection", Value::Num(reference.side)),
        Some(direction) => next.set("bendDirection", direction.clone()),
        None => {
            next.remove("bendDirection");
        }
    }
    set_controller(record, &key, next);
}

/// `object._stykIK = {...object._stykIK, [key]: next}`.
fn set_controller(record: &mut Obj, key: &str, next: Obj) {
    let mut table = record.get("_stykIK").and_then(Value::as_obj).cloned().unwrap_or_default();
    assign(&mut table, key, Value::object(next));
    record.set("_stykIK", Value::object(table));
}

/// An array index as JavaScript objects enumerate them: first, in ascending order.
fn index_key(key: &str) -> Option<u32> {
    if key.is_empty() || (key.len() > 1 && key.starts_with('0')) || !key.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    key.parse::<u32>().ok().filter(|&n| n < u32::MAX)
}

/// `object[key] = value` in JavaScript's key order: an existing key keeps its place, a new
/// index key goes among the index keys in ascending order, any other key last.
fn assign(object: &mut Obj, key: &str, value: Value) {
    let Some(n) = index_key(key).filter(|_| !object.contains(key)) else {
        object.set(key, value);
        return;
    };
    let entries: Vec<(std::sync::Arc<str>, Value)> = object.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    let at = entries.iter().position(|(k, _)| index_key(k).is_none_or(|m| m > n)).unwrap_or(entries.len());
    let mut next = Obj::new();
    for (k, v) in &entries[..at] {
        next.set(k, v.clone());
    }
    next.set(key, value);
    for (k, v) in &entries[at..] {
        next.set(k, v.clone());
    }
    *object = next;
}

/// Angles by handle, as the editor stores followers (index keys, ascending).
fn angle_table(angles: &BTreeMap<u32, f64>) -> Value {
    let mut table = Obj::new();
    for (handle, angle) in angles {
        table.set(&handle.to_string(), Value::Num(*angle));
    }
    Value::object(table)
}

/// `_rememberIKBends`' test that a solved bend is the one the gesture began with.
fn same_bend(reference: &BendReference, baseline: &Value) -> bool {
    let number = |key: &str| baseline.get(key).and_then(Value::as_f64);
    let close = |a: f64, b: Option<f64>| {
        let d = a - b.unwrap_or(f64::NAN);
        atan2(sin(d), cos(d)).abs() < 1e-7
    };
    let shape = baseline.get("shape");
    Some(reference.side) == number("side")
        && close(reference.angle, number("angle"))
        && close(reference.axis, number("axis"))
        && (reference.ratio - number("ratio").unwrap_or(f64::NAN)).abs() < 1e-7
        && reference.shape.as_ref().map(Vec::len) == shape.and_then(Value::array_len)
        && reference.shape.as_ref().is_none_or(|s| s.iter().enumerate().all(|(i, &a)| close(a, shape.and_then(|v| v.item(i)).and_then(|v| v.as_f64()))))
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
