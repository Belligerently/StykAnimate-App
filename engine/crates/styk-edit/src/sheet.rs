//! Writing poses into a layer sheet (src/editor-layer-sheet.mjs): `_commitSheetPose`,
//! the planner (`planner`, `write`, `capture`, `materialize`), `publish`, and undo and
//! redo of what it records (`applyLayerSheetCommand`, `_pruneFrameSources`).
//!
//! Ported for gestures on existing figures. Movieclip frame replacements, drawing
//! layers, new figures and reordering answer `Unported`.

use styk_doc::{Obj, Value};
use styk_eval::records::snapshot_pose_frame_of;

use crate::frame::{layer_of, objects_of};
use crate::history::{Command, CommandKind, Direction};
use crate::record::uid;
use crate::{EditError, Editor};

const MAX_SHEET_TICKS: f64 = 10_000_000.0;

fn fixed(id: &str) -> bool {
    id == "scene" || id == "camera"
}

fn num(value: &Value, key: &str) -> Option<f64> {
    value.get(key).and_then(Value::as_f64)
}

fn items(value: Option<&Value>) -> Vec<Value> {
    value.and_then(Value::elements).map(|e| e.iter().collect()).unwrap_or_default()
}

fn json(value: Option<&Value>) -> String {
    value.map(styk_doc::json::to_string).unwrap_or_default()
}

/// `continuations(frame)`.
fn continuations(frame: &Value) -> Vec<Value> {
    items(frame.path(&["_styk", "movieclipContinuations"]))
}

/// `frameOrder(frame)`: unique ids in drawing order.
fn frame_order(frame: &Value) -> Vec<f64> {
    let objects = objects_of(frame);
    match frame.get("object_draw_order").filter(|o| o.truthy()) {
        Some(order) => order.numbers().unwrap_or_default().into_iter().filter_map(|slot| objects.get(slot as usize).filter(|_| slot >= 0.0).and_then(|o| num(o, "unique_id"))).collect(),
        None => objects.iter().filter_map(|o| num(o, "unique_id")).collect(),
    }
}

/// `poseObjectSignature(object, frame)`: a pose without its slot and caches, its
/// join named by the figure it joins.
fn signature(object: &Value, frame_objects: &[Value]) -> String {
    let mut fields = object.as_obj().cloned().unwrap_or_default();
    fields.remove("runtime_segments");
    fields.remove("_movieclipSample");
    fields.remove("object_slot");
    if let Some(join) = object.get("join").filter(|j| j.truthy()) {
        let slot = num(join, "target_object_slot");
        let target = slot.filter(|s| *s >= 0.0).and_then(|s| frame_objects.get(s as usize)).and_then(|o| o.get("unique_id").cloned()).or_else(|| join.get("target_unique_id").cloned());
        let mut out = join.as_obj().cloned().unwrap_or_default();
        out.remove("target_object_slot");
        out.remove("target_unique_id");
        if let Some(target) = target {
            out.set("target_unique_id", target);
        }
        fields.set("join", Value::object(out));
    }
    styk_doc::json::to_string(&Value::object(fields))
}

/// `keyEasing(key)`: `{interpolation, bezier?}`, `hold` read as `constant`.
fn key_easing(easing: &Value) -> Vec<(&'static str, Value)> {
    let mode = match easing.get("interpolation").and_then(Value::as_str) {
        Some("hold") => "constant",
        Some(mode) => mode,
        None => "linear",
    };
    let mut fields = vec![("interpolation", Value::str(mode))];
    if mode == "bezier" {
        fields.push(("bezier", easing.get("bezier").cloned().unwrap_or_else(|| Value::array([0.42, 0.0, 0.58, 1.0].map(Value::Num).to_vec()))));
    }
    fields
}

/// `sheetKeyAt(track, tick)`: the index of the last key at or before `tick`.
fn key_at(keys: &[Value], tick: f64) -> Option<usize> {
    keys.partition_point(|k| num(k, "tick").unwrap_or(0.0) <= tick).checked_sub(1)
}

/// The planner of one command: the tracks it replaces, the pose sources it captures.
struct Plan {
    before: Value,
    /// Replaced tracks, in the order the command first touched them.
    tracks: Vec<(String, Obj)>,
    next_id: f64,
    duration: f64,
    sources: Vec<(String, Value)>,
    layers: Value,
    next_frame: u64,
}

impl Plan {
    /// `get(id)`: the command's own copy of a track.
    fn track(&mut self, id: &str) -> &mut Obj {
        if let Some(at) = self.tracks.iter().position(|(track, _)| track == id) {
            return &mut self.tracks[at].1;
        }
        let track = self.before.path(&["tracks", id]).and_then(Value::as_obj).cloned().unwrap_or_else(|| {
            let mut track = Obj::new();
            track.set("keys", Value::array(Vec::new()));
            track.set("end", Value::Num(0.0));
            track
        });
        self.tracks.push((id.to_string(), track));
        &mut self.tracks.last_mut().unwrap().1
    }

    fn keys(&mut self, id: &str) -> Vec<Value> {
        items(self.track(id).get("keys"))
    }

    fn set_keys(&mut self, id: &str, keys: Vec<Value>) {
        self.track(id).set("keys", Value::array(keys));
    }

    fn cell(&mut self) -> String {
        let id = format!("cell-{}", styk_doc::json::to_string(&Value::Num(self.next_id)));
        self.next_id += 1.0;
        id
    }

    /// `capture(frame, id)`: the layer's figures of `frame` (and what they are joined
    /// to) as a new pose source.
    fn capture(&mut self, frame: &Value, id: &str) -> Value {
        let layers = Some(&self.layers);
        let owned = |uid: f64| layer_of(layers, uid as u32) == id;
        let uids: Vec<f64> = if fixed(id) { Vec::new() } else { objects_of(frame).iter().filter_map(|o| num(o, "unique_id")).filter(|u| owned(*u)).collect() };
        let steps: Vec<Value> = if fixed(id) { Vec::new() } else { continuations(frame).into_iter().filter(|step| num(step, "uid").is_some_and(owned)).collect() };
        let frame_id = format!("frame-{}", self.next_frame);
        self.next_frame += 1;
        let mut source = snapshot_pose_frame_of(frame, Some(&uids)).as_obj().cloned().unwrap_or_default();
        let mut styk = source.get("_styk").and_then(Value::as_obj).cloned().unwrap_or_default();
        styk.set("frameId", Value::str(&frame_id));
        if steps.is_empty() {
            styk.remove("movieclipContinuations");
        } else {
            styk.set("movieclipContinuations", Value::array(steps));
        }
        source.set("_styk", Value::object(styk));
        self.sources.push((frame_id.clone(), Value::object(source)));
        let mut value = Obj::new();
        value.set("frameId", Value::str(&frame_id));
        Value::object(value)
    }

    /// `write(id, tick, frame, {easing})`: a key at `tick` holding the layer's pose in `frame`.
    fn write(&mut self, id: &str, tick: f64, frame: &Value, easing: &Value) -> Result<(), EditError> {
        if tick.fract() != 0.0 || !(0.0..MAX_SHEET_TICKS).contains(&tick) {
            return Err(EditError::Invalid("errors.editorLayerSheet.chooseAValidWholeFrame".into()));
        }
        let mut keys = self.keys(id);
        let end = num(&Value::object(self.track(id).clone()), "end").unwrap_or(0.0);
        let index = key_at(&keys, tick);
        let existing = index.filter(|&i| num(&keys[i], "tick") == Some(tick));
        let mut key = match existing {
            Some(i) => keys[i].as_obj().cloned().unwrap_or_default(),
            None => {
                let mut key = Obj::new();
                key.set("id", Value::str(&self.cell()));
                key.set("tick", Value::Num(tick));
                for (name, value) in key_easing(easing) {
                    key.set(name, value);
                }
                key
            }
        };
        // Past the layer's end: an empty cell closes the gap before the new key.
        if !fixed(id) && tick > end && !keys.is_empty() {
            let mut pad = Obj::new();
            pad.set("id", Value::str(&self.cell()));
            pad.set("tick", Value::Num(end));
            pad.set("interpolation", Value::str("constant"));
            let mut value = Obj::new();
            value.set("frameId", Value::Null);
            pad.set("value", Value::object(value));
            pad.set("empty", Value::Bool(true));
            keys.push(Value::object(pad));
        }
        let layers = Some(&self.layers);
        let owned = |uid: Option<f64>| uid.is_some_and(|u| layer_of(layers, u as u32) == id);
        if layers.and_then(|l| l.path(&["byId", id, "drawing", "version"])).and_then(Value::as_f64) == Some(1.0) {
            return Err(EditError::Unported("poses on a drawing layer".into()));
        }
        let empty = !fixed(id) && !objects_of(frame).iter().any(|o| owned(num(o, "unique_id"))) && !continuations(frame).iter().any(|step| owned(num(step, "uid")));
        let value = if empty {
            let mut value = Obj::new();
            value.set("frameId", Value::Null);
            Value::object(value)
        } else {
            self.capture(frame, id)
        };
        key.set("value", value);
        key.set("empty", Value::Bool(empty));
        key.remove("sourceTick");
        for (name, value) in key_easing(easing) {
            key.set(name, value);
        }
        key.remove("patchOnly");
        key.remove("movieclipPatches");
        match existing {
            Some(i) => keys[i] = Value::object(key),
            None if keys.is_empty() || keys.last().and_then(|last| num(last, "tick")).is_some_and(|last| tick > last) => keys.push(Value::object(key)),
            None => keys.insert(index.map_or(0, |i| i + 1), Value::object(key)),
        }
        self.set_keys(id, keys);
        let end = end.max(tick + 1.0);
        self.track(id).set("end", Value::Num(end));
        self.duration = self.duration.max(end);
        Ok(())
    }
}

impl Editor {
    /// `insertionOwner(editor, uid, hints)` for a figure the document already has.
    fn owner(&self, uid: u32) -> String {
        self.new_layer_hints.get(&uid).cloned().unwrap_or_else(|| self.layer_of(uid))
    }

    /// `_commitSheetPose(edit, drag)`: keys the draft on every layer whose figures changed.
    pub(crate) fn commit_sheet_pose(&mut self) -> Result<bool, EditError> {
        let (Some(edit), Some(drag)) = (self.authored.as_ref(), self.drag.as_ref()) else { return Ok(false) };
        let (before, after, tick) = (edit.source.clone(), edit.frame.clone(), edit.tick);
        let (old, current) = (objects_of(&before), objects_of(&after));
        let find = |objects: &[Value], id: u32| objects.iter().find(|o| uid(o) == Some(id)).cloned();
        let changed: Vec<u32> = drag
            .before
            .iter()
            .filter_map(uid)
            .filter(|&id| match (find(&old, id), find(&current, id)) {
                (Some(a), Some(b)) => signature(&a, &old) != signature(&b, &current),
                (None, None) => false,
                _ => true,
            })
            .collect();
        let mut ids: Vec<String> = Vec::new();
        for &id in &changed {
            let owner = self.owner(id);
            if !ids.contains(&owner) {
                ids.push(owner);
            }
        }
        if frame_order(&before) != frame_order(&after) {
            return Err(EditError::Unported("reordering figures on a layer sheet".into()));
        }
        if json(before.get("camera")) != json(after.get("camera")) {
            ids.push("camera".into());
        }
        if ["background_id", "background_id_b", "background_mix", "background_color", "_stykBackgroundColor", "_stykVfx"].iter().any(|key| json(before.get(key)) != json(after.get(key))) {
            ids.push("scene".into());
        }
        if ids.is_empty() {
            self.drag = None;
            self.authored = None;
            self.retained = None;
            return Ok(false);
        }
        // The draft stays on show after the commit unless a clip's playback changed.
        let playback = |objects: &[Value], id: u32| json(find(objects, id).as_ref().and_then(|o| o.get("_stykMovieclip")));
        let retain = changed.iter().all(|&id| find(&old, id).is_some() && find(&current, id).is_some() && playback(&old, id) == playback(&current, id));
        let label = drag.label.clone();
        let selection = drag.before_selection.clone();
        let mut plan = self.planner(&ids)?;
        for id in &ids {
            let keys = plan.keys(id);
            let end = num(&Value::object(plan.track(id).clone()), "end").unwrap_or(0.0);
            let at_key = key_at(&keys, tick);
            let key = at_key.map(|i| keys[i].clone());
            let hold = key.as_ref().is_some_and(|k| tick < end && matches!(k.get("interpolation").and_then(Value::as_str), Some("constant" | "hold")));
            // Figures that a key's movieclip replacements placed are edited in the
            // replacement they came from: the key keeps its pose and its replacements.
            let patches = items(key.as_ref().and_then(|k| k.get("movieclipPatches")));
            let changed_here: Vec<u32> = changed.iter().copied().filter(|&u| self.owner(u) == *id).collect();
            if let (false, Some(index), false) = (self.auto_key, at_key, patches.is_empty()) {
                let members = |patch: &Value| -> Vec<u32> {
                    patch.get("frameId").and_then(Value::as_str).and_then(|frame| self.frame_by_id(frame)).map(|frame| objects_of(&frame).iter().filter_map(uid).collect()).unwrap_or_default()
                };
                let placed: Vec<u32> = patches.iter().flat_map(&members).collect();
                if !changed_here.is_empty() && changed_here.iter().all(|u| placed.contains(u)) {
                    let mut owned = keys[index].as_obj().cloned().unwrap_or_default();
                    owned.set("empty", Value::Bool(!current.iter().any(|o| uid(o).is_some_and(|u| self.owner(u) == *id))));
                    let mut replaced = Vec::with_capacity(patches.len());
                    for patch in &patches {
                        let uids = members(patch);
                        if !changed_here.iter().any(|u| uids.contains(u)) {
                            replaced.push(patch.clone());
                            continue;
                        }
                        let source = patch.get("frameId").and_then(Value::as_str).and_then(|frame| self.frame_by_id(frame)).unwrap_or(Value::Null);
                        let frame = self.owned_frame(&after, &uids, &continuations(&source))?;
                        let mut patch = patch.as_obj().cloned().unwrap_or_default();
                        patch.set("frameId", plan.capture(&frame, id).get("frameId").cloned().unwrap_or(Value::Null));
                        replaced.push(Value::object(patch));
                    }
                    owned.set("movieclipPatches", Value::array(replaced));
                    let mut keys = keys;
                    keys[index] = Value::object(owned);
                    plan.set_keys(id, keys);
                    continue;
                }
            }
            // A boundary of a broken-apart movieclip frame: the pose after it is fixed first.
            if key.as_ref().is_some_and(|k| k.get("patchOnly").is_some_and(Value::truthy))
                && let Some(next) = at_key.and_then(|i| keys.get(i + 1)).cloned()
            {
                let until = num(&next, "tick").unwrap_or(tick);
                let frame = self.evaluated(until, false)?;
                plan.write(id, until, &frame, &next)?;
            }
            // A held cell is edited at its key; auto key always keys the playhead.
            let at = match &key {
                Some(key) if hold && !self.auto_key => num(key, "tick").unwrap_or(tick),
                _ => tick,
            };
            let constant = {
                let mut easing = Obj::new();
                easing.set("interpolation", Value::str("constant"));
                Value::object(easing)
            };
            plan.write(id, at, &after, key.as_ref().unwrap_or(&constant))?;
        }
        self.drag = None;
        self.authored = None;
        self.publish(plan, &label, selection, retain.then_some(after))
    }

    /// `getFrameById(report, id)`: a retained pose source, else the keyframe with that id.
    fn frame_by_id(&self, id: &str) -> Option<Value> {
        let retained = self.report.path(&["_styk", "frameSources", id]).filter(|frame| frame.truthy()).cloned();
        retained.or_else(|| items(self.report.get("keyframes")).into_iter().find(|frame| frame.path(&["_styk", "frameId"]).and_then(Value::as_str) == Some(id)))
    }

    /// `ownedFrame(report, frame, uids)` with `continuations` set: the frame reduced to
    /// the figures `uids` name, joins among them kept, slots by unique id.
    fn owned_frame(&self, frame: &Value, uids: &[u32], steps: &[Value]) -> Result<Value, EditError> {
        let all = objects_of(frame);
        let selected = |object: &Value| uid(object).is_some_and(|u| uids.contains(&u));
        let order: Vec<f64> = frame_order(frame).into_iter().filter(|u| uids.contains(&(*u as u32))).collect();
        let mut members: Vec<Obj> = Vec::new();
        for object in all.iter().filter(|o| selected(o)) {
            let mut record = object.as_obj().cloned().unwrap_or_default();
            if let Some(join) = object.get("join").filter(|j| j.truthy()) {
                let parent = num(join, "target_object_slot").filter(|s| *s >= 0.0).and_then(|s| all.get(s as usize));
                match parent.filter(|p| selected(p)).and_then(|p| p.get("unique_id")) {
                    Some(parent) => {
                        let mut join = join.as_obj().cloned().unwrap_or_default();
                        join.set("target_unique_id", parent.clone());
                        record.set("join", Value::object(join));
                    }
                    None => return Err(EditError::Unported("a replaced figure joined to a figure outside its movieclip frame".into())),
                }
            }
            members.push(record);
        }
        // `normalizeFrame`: slots follow unique ids, joins follow their figures.
        let id_of = |record: &Obj| record.get("unique_id").and_then(Value::as_f64).unwrap_or(f64::NAN);
        members.sort_by_key(|record| id_of(record) as i64 as i32);
        let slot_of = |target: f64| members.iter().position(|record| id_of(record) == target);
        let slots: Vec<Option<usize>> = members.iter().map(|record| record.get("join").filter(|j| j.truthy()).and_then(|j| num(j, "target_unique_id")).and_then(slot_of)).collect();
        let objects: Vec<Value> = members
            .iter()
            .enumerate()
            .map(|(slot, record)| {
                let mut record = record.clone();
                record.set("object_slot", Value::Num(slot as f64));
                if let (Some(target), Some(join)) = (slots[slot], record.get("join").and_then(Value::as_obj).cloned()) {
                    let mut join = join;
                    join.set("target_object_slot", Value::Num(target as f64));
                    record.set("join", Value::object(join));
                }
                Value::object(record)
            })
            .collect();
        let mut out = frame.as_obj().cloned().unwrap_or_default();
        out.set("objects", Value::array(objects));
        let mut styk = out.get("_styk").and_then(Value::as_obj).cloned().unwrap_or_default();
        if steps.is_empty() {
            styk.remove("movieclipContinuations");
        } else {
            styk.set("movieclipContinuations", Value::array(steps.to_vec()));
        }
        out.set("_styk", Value::object(styk));
        out.set("object_draw_order_unique_ids", Value::array(order.iter().map(|u| Value::Num(*u)).collect()));
        out.set("object_draw_order", Value::array(order.iter().map(|u| slot_of(*u).map_or(Value::Null, |slot| Value::Num(slot as f64))).collect()));
        Ok(Value::object(out))
    }

    /// `planner(editor, ids)`: `ids` must be layers that exist and are not locked.
    fn planner(&self, ids: &[String]) -> Result<Plan, EditError> {
        let styk = self.report.get("_styk");
        let layers = styk.and_then(|s| s.get("layers")).cloned().unwrap_or(Value::Null);
        if ids.is_empty() {
            return Err(EditError::Invalid("errors.editorLayerSheet.selectALayer".into()));
        }
        for id in ids {
            let state = if fixed(id) { layers.path(&["fixed", id]) } else { layers.path(&["byId", id]) };
            let listed = fixed(id) || items(layers.get("order")).iter().any(|layer| layer.as_str() == Some(id.as_str()));
            if !listed {
                return Err(EditError::Invalid("errors.editorLayerSheet.theSelectedLayerNoLongerExists".into()));
            }
            if state.and_then(|s| s.get("locked")).is_some_and(Value::truthy) {
                return Err(EditError::Invalid("errors.editorLayerSheet.unlockBeforeEditingIt".into()));
            }
        }
        let Some(before) = styk.and_then(|s| s.get("layerSheet")).cloned() else { return Err(EditError::Invalid("errors.editorLayerSheet.openTheLayerTimelineFirst".into())) };
        Ok(Plan {
            next_id: num(&before, "nextId").unwrap_or(1.0),
            duration: num(&before, "duration").unwrap_or(1.0),
            before,
            tracks: Vec::new(),
            sources: Vec::new(),
            layers,
            next_frame: self.next_frame_identity,
        })
    }

    /// `publish(editor, plan, label, {selection})`: the plan becomes the document's
    /// sheet, with one history entry.
    fn publish(&mut self, mut plan: Plan, label: &str, selection: crate::select::SelectionState, retained: Option<Value>) -> Result<bool, EditError> {
        // Imported spans whose ends changed are frozen as poses: an unchanged
        // imported span keeps evaluating from the import.
        for (id, _) in plan.tracks.clone() {
            let previous: Vec<Value> = items(plan.before.path(&["tracks", &id, "keys"])).into_iter().filter(|k| !k.get("patchOnly").is_some_and(Value::truthy)).collect();
            let current: Vec<Value> = plan.keys(&id).into_iter().filter(|k| !k.get("patchOnly").is_some_and(Value::truthy)).collect();
            let name = |key: Option<&Value>| key.and_then(|k| k.get("id")).cloned();
            let tick = |key: Option<&Value>| key.and_then(|k| num(k, "tick"));
            for (i, key) in current.iter().enumerate() {
                let Some(at) = previous.iter().position(|old| name(Some(old)) == name(Some(key))) else { continue };
                let (old, old_next, next) = (&previous[at], previous.get(at + 1), current.get(i + 1));
                let moved = tick(Some(key)) != tick(Some(old))
                    || key.get("interpolation") != old.get("interpolation")
                    || json(key.get("bezier")) != json(old.get("bezier"))
                    || name(next) != name(old_next)
                    || tick(next) != tick(old_next);
                if moved {
                    for target in [Some(key), next].into_iter().flatten() {
                        self.materialize(&mut plan, &id, target)?;
                    }
                }
            }
        }
        // The new sheet: the old one with the replaced tracks, counters and length.
        let before = plan.before.clone();
        let before_layers = plan.layers.clone();
        let mut tracks = before.get("tracks").and_then(Value::as_obj).cloned().unwrap_or_default();
        for (id, track) in &plan.tracks {
            tracks.set(id, Value::object(track.clone()));
        }
        let mut state = before.as_obj().cloned().unwrap_or_default();
        state.set("tracks", Value::object(tracks));
        state.set("nextId", Value::Num(plan.next_id));
        state.set("duration", Value::Num(plan.duration));
        let after = Value::object(state);
        let after_layers = plan.layers.clone();
        self.set_sheet(&after, &after_layers);
        if !plan.sources.is_empty() {
            let sources = self.sources_mut();
            for (id, source) in &plan.sources {
                sources.set(id, source.clone());
            }
        }
        self.next_frame_identity = plan.next_frame;
        self.retained = retained;
        self.new_layer_hints = plan.sources.iter().flat_map(|(_, source)| objects_of(source)).filter_map(|o| uid(&o)).map(|u| (u, self.layer_of(u))).collect();
        let after_selection = self.selection_state();
        let source_ids: Vec<Value> = plan.sources.iter().map(|(id, _)| Value::str(id)).collect();
        self.record(Command {
            kind: CommandKind::LayerSheet { before, after, before_layers, after_layers, sources: plan.sources },
            label: label.to_string(),
            before_selection: selection,
            after_selection,
            before_revision: 0,
            after_revision: 0,
        });
        self.layer_revision += 1;
        self.emit_sheet("edit", label, source_ids);
        Ok(true)
    }

    /// `materialize(id, key)`: an imported key becomes the pose it showed.
    fn materialize(&self, plan: &mut Plan, id: &str, key: &Value) -> Result<(), EditError> {
        let mut keys = plan.keys(id);
        let Some(at) = keys.iter().position(|k| k.get("id") == key.get("id")) else { return Ok(()) };
        if keys[at].get("sourceTick").is_none() {
            return Ok(());
        }
        let old = items(plan.before.path(&["tracks", id, "keys"])).into_iter().find(|k| k.get("id") == key.get("id"));
        let tick = old.as_ref().and_then(|k| num(k, "tick")).or_else(|| num(&keys[at], "tick")).unwrap_or(0.0);
        let frame = self.evaluated(tick, false)?;
        let mut owned = keys[at].as_obj().cloned().unwrap_or_default();
        owned.set("value", plan.capture(&frame, id));
        owned.remove("sourceTick");
        keys[at] = Value::object(owned);
        plan.set_keys(id, keys);
        Ok(())
    }

    fn styk_mut(&mut self) -> Option<&mut Obj> {
        self.report.make_obj()?.get_mut("_styk")?.make_obj()
    }

    fn set_sheet(&mut self, sheet: &Value, layers: &Value) {
        if let Some(styk) = self.styk_mut() {
            styk.set("layerSheet", sheet.clone());
            styk.set("layers", layers.clone());
        }
    }

    /// `report._styk.frameSources ??= {}`.
    fn sources_mut(&mut self) -> &mut Obj {
        let styk = self.styk_mut().expect("a layer-sheet document has _styk");
        if !styk.get("frameSources").is_some_and(Value::is_object) {
            styk.set("frameSources", Value::object(Obj::new()));
        }
        styk.get_mut("frameSources").and_then(Value::make_obj).expect("just set")
    }

    /// The event of a published or replayed sheet command: every layer view refreshes.
    fn emit_sheet(&mut self, kind: &str, label: &str, source_ids: Vec<Value>) {
        // `{layers: true, layerRevision, ...detail}` over the base event, whose
        // `structural` and `timing` the detail sets.
        let mut event = Obj::new();
        event.set("kind", Value::str(kind));
        event.set("frameIndex", Value::Num(self.frame_index as f64));
        event.set("tick", Value::Num(self.tick));
        event.set("structural", Value::Bool(true));
        event.set("timing", Value::Bool(true));
        event.set("layers", Value::Bool(true));
        event.set("layerRevision", Value::Num(self.layer_revision as f64));
        event.set("label", Value::str(label));
        event.set("layerSheet", Value::Bool(true));
        event.set("definition", Value::Bool(false));
        event.set("frameIndices", Value::array(Vec::new()));
        event.set("sourceIds", Value::array(source_ids));
        self.events.push(Value::object(event));
    }

    /// `applyLayerSheetCommand(editor, command, direction)`.
    pub(crate) fn apply_sheet_command(&mut self, command: &Command, direction: Direction) -> Result<(), EditError> {
        let CommandKind::LayerSheet { before, after, before_layers, after_layers, sources } = &command.kind else { return Ok(()) };
        let (sheet, layers, selection, revision) = match direction {
            Direction::Before => (before, before_layers, &command.before_selection, command.before_revision),
            Direction::After => (after, after_layers, &command.after_selection, command.after_revision),
        };
        self.set_sheet(sheet, layers);
        // `_applyFrameSourceChanges`: the command's captures exist after it, not before.
        if !sources.is_empty() {
            let table = self.sources_mut();
            for (id, source) in sources {
                match direction {
                    Direction::After => table.set(id, source.clone()),
                    Direction::Before => {
                        table.remove(id);
                    }
                }
            }
            if direction == Direction::Before && table.is_empty() {
                self.styk_mut().and_then(|styk| styk.remove("frameSources"));
            }
        }
        self.layer_revision += 1;
        self.retained = None;
        self.restore_selection(selection)?;
        self.revision = revision;
        let source_ids = sources.iter().map(|(id, _)| Value::str(id)).collect();
        self.emit_sheet("history", &command.label, source_ids);
        Ok(())
    }

    /// `_pruneFrameSources`: this session's pose captures that neither the sheet nor
    /// any history entry refers to any more are dropped.
    pub(crate) fn prune_frame_sources(&mut self) {
        if self.generated_sources.is_empty() {
            return;
        }
        let mut retained: Vec<String> = Vec::new();
        let keep_sheet = |sheet: Option<&Value>, retained: &mut Vec<String>| {
            for (_, track) in sheet.and_then(|s| s.get("tracks")).and_then(Value::as_obj).map(|t| t.iter().collect::<Vec<_>>()).unwrap_or_default() {
                for key in items(track.get("keys")) {
                    let patches = items(key.get("movieclipPatches"));
                    for id in key.path(&["value", "frameId"]).into_iter().chain(patches.iter().filter_map(|patch| patch.get("frameId"))).filter_map(Value::as_str) {
                        if !retained.iter().any(|kept| kept == id) {
                            retained.push(id.to_string());
                        }
                    }
                }
            }
        };
        keep_sheet(self.report.path(&["_styk", "layerSheet"]), &mut retained);
        for command in self.undo.iter().chain(&self.redo) {
            if let CommandKind::LayerSheet { before, after, sources, .. } = &command.kind {
                keep_sheet(Some(before), &mut retained);
                keep_sheet(Some(after), &mut retained);
                retained.extend(sources.iter().map(|(id, _)| id.clone()));
            }
        }
        let dropped: Vec<String> = self.generated_sources.iter().filter(|id| !retained.contains(id)).cloned().collect();
        if dropped.is_empty() {
            return;
        }
        self.generated_sources.retain(|id| !dropped.contains(id));
        let keep_table = self.source_table_was_present;
        if let Some(styk) = self.styk_mut() {
            let empty = match styk.get_mut("frameSources").and_then(Value::make_obj) {
                Some(table) => {
                    for id in &dropped {
                        table.remove(id);
                    }
                    table.is_empty()
                }
                None => false,
            };
            if empty && !keep_table {
                styk.remove("frameSources");
            }
        }
    }
}
