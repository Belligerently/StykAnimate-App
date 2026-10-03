//! Commands on whole figures that draft the frame (`transaction` in src/editor.mjs):
//! `joinTo`, `unjoinFigure`, `joinFigure`, `deleteSelection`, `duplicateFigure`.
//! On a layer sheet they commit through the planner like a gesture, and may add or
//! remove figures.

use styk_doc::{Obj, Value};

use crate::frame::objects_of;
use crate::record::{join_target, num, uid};
use crate::{EditError, Editor};

/// `setOrder(frame, ids)`: the drawing order as unique ids and as slots.
fn set_order(frame: &mut Obj, ids: &[f64]) {
    let objects: Vec<Value> = frame.get("objects").and_then(Value::elements).map(|e| e.iter().collect()).unwrap_or_default();
    let slot = |id: f64| objects.iter().position(|o| num(o, "unique_id") == Some(id));
    frame.set("object_draw_order_unique_ids", Value::array(ids.iter().map(|id| Value::Num(*id)).collect()));
    frame.set("object_draw_order", Value::array(ids.iter().map(|id| slot(*id).map_or(Value::Null, |slot| Value::Num(slot as f64))).collect()));
}

/// `frameOrder(frame)`.
fn frame_order(frame: &Obj) -> Vec<f64> {
    let objects: Vec<Value> = frame.get("objects").and_then(Value::elements).map(|e| e.iter().collect()).unwrap_or_default();
    match frame.get("object_draw_order").filter(|o| o.truthy()) {
        Some(order) => order.numbers().unwrap_or_default().into_iter().filter_map(|slot| objects.get(slot as usize).filter(|_| slot >= 0.0).and_then(|o| num(o, "unique_id"))).collect(),
        None => objects.iter().filter_map(|o| num(o, "unique_id")).collect(),
    }
}

/// `{...join, [key]: value}` on a figure's join, in place.
fn set_join_field(object: &mut Obj, key: &str, value: Option<Value>) {
    let Some(mut join) = object.get("join").filter(|j| j.truthy()).and_then(Value::as_obj).cloned() else { return };
    match value {
        Some(value) => join.set(key, value),
        None => {
            join.remove(key);
        }
    }
    object.set("join", Value::object(join));
}

/// `normalizeFrame(frame)`: slots follow unique ids, joins follow their figures, the
/// drawing order keeps naming the same figures.
fn normalize_frame(frame: &mut Obj) {
    let ids = frame_order(frame);
    let objects: Vec<Value> = frame.get("objects").and_then(Value::elements).map(|e| e.iter().collect()).unwrap_or_default();
    let mut records: Vec<Obj> = objects
        .iter()
        .map(|object| {
            let mut record = object.as_obj().cloned().unwrap_or_default();
            let target = join_target(object).and_then(|slot| objects.get(slot)).and_then(|o| o.get("unique_id").cloned());
            set_join_field(&mut record, "target_unique_id", target);
            record
        })
        .collect();
    let id_of = |record: &Obj| record.get("unique_id").and_then(Value::as_f64).unwrap_or(f64::NAN);
    records.sort_by_key(|record| id_of(record) as i64 as i32);
    let slots: Vec<f64> = records.iter().map(id_of).collect();
    let normalized = records.into_iter().enumerate().map(|(slot, mut record)| {
        record.set("object_slot", Value::Num(slot as f64));
        let target = record.get("join").and_then(|j| num(j, "target_unique_id")).and_then(|u| slots.iter().position(|id| *id == u));
        set_join_field(&mut record, "target_object_slot", target.map(|slot| Value::Num(slot as f64)));
        Value::object(record)
    });
    frame.set("objects", Value::array(normalized.collect()));
    set_order(frame, &ids);
}

impl Editor {
    /// `editor.selectedSlot`: the slot of the selected figure in the selection frame.
    fn selected_slot(&self) -> Result<Option<usize>, EditError> {
        let frame = self.selection_frame()?;
        Ok(self.selection.uid.and_then(|selected| frame.iter().position(|o| uid(o) == Some(selected))))
    }

    /// `joinTo(targetUid, handleId)`.
    pub(crate) fn join_to(&mut self, target: Option<f64>, handle: &Value) -> Result<bool, EditError> {
        let Some((_, object)) = self.object()? else { return Ok(false) };
        let handle = if handle.is_null() { Some(0.0) } else { handle.as_f64() };
        let frame = self.selection_frame()?;
        let slot = self.selected_slot()?;
        let target_slot = target.and_then(|target| frame.iter().position(|o| num(o, "unique_id") == Some(target)));
        let figures: Vec<Value> = self.report.get("figure_types").and_then(Value::elements).map(|e| e.iter().collect()).unwrap_or_default();
        let handles = |figure: &Value| -> Vec<f64> {
            let segments = figure.get("segments").and_then(Value::elements).map(|e| e.iter().collect::<Vec<_>>()).unwrap_or_default();
            std::iter::once(0.0).chain(segments.iter().filter_map(|s| num(s, "segment_id"))).collect()
        };
        let valid = target_slot.filter(|&t| Some(t) != slot).and_then(|t| figures.iter().find(|f| num(f, "figure_type_id") == num(&frame[t], "figure_type_id"))).is_some_and(|figure| handle.is_some_and(|h| handles(figure).contains(&h)));
        let (Some(target_slot), true) = (target_slot, valid) else { return Err(EditError::Invalid("errors.editor.chooseAnotherFigureAndAValidJoint".into())) };
        let mut current = target_slot;
        let mut steps = 0;
        while let Some(parent) = frame.get(current).and_then(join_target) {
            current = parent;
            steps += 1;
            if Some(current) == slot || steps > frame.len() {
                return Err(EditError::Invalid("errors.editor.joiningTheseFiguresWouldCreateACycle".into()));
            }
        }
        let selected = uid(&object);
        let mut join = Obj::new();
        join.set("target_object_slot", Value::Num(target_slot as f64));
        join.set("target_unique_id", Value::Num(target.unwrap()));
        join.set("target_handle_id", Value::Num(handle.unwrap()));
        self.transaction("Join figures", |editor| {
            let at = editor.frame_objects().iter().position(|o| uid(o) == selected);
            if let Some(record) = at.and_then(|at| editor.frame_object_mut(at)) {
                record.set("join", Value::object(join));
            }
            Ok(())
        })
    }

    /// `unjoinFigure()`: the figure stays where its join held it.
    pub(crate) fn unjoin_figure(&mut self) -> Result<bool, EditError> {
        let Some((_, object)) = self.object()?.filter(|(_, o)| join_target(o).is_some()) else { return Ok(false) };
        let selected = uid(&object);
        let root = match self.selected_slot()? {
            Some(_) => selected.and_then(|u| self.roots().ok()?.get(&u).copied()),
            None => None,
        };
        self.transaction("Unjoin figure", |editor| {
            let at = editor.frame_objects().iter().position(|o| uid(o) == selected);
            if let Some(record) = at.and_then(|at| editor.frame_object_mut(at)) {
                if let Some(root) = root {
                    record.set("root_x", Value::Num(root[0]));
                    record.set("root_y", Value::Num(root[1]));
                }
                record.set("join", Value::Null);
            }
            Ok(())
        })
    }

    /// `deleteSelection()`: the selected figures leave the frame; figures joined to
    /// them stay where the join held them.
    pub(crate) fn delete_selection(&mut self) -> Result<bool, EditError> {
        let selected: Vec<u32> = self.selected_objects()?.iter().filter_map(uid).collect();
        if selected.is_empty() {
            return Ok(false);
        }
        let figures: Vec<Value> = self.report.get("figure_types").and_then(Value::elements).map(|e| e.iter().collect()).unwrap_or_default();
        let clip = |object: &Value| figures.iter().find(|f| num(f, "figure_type_id") == num(object, "figure_type_id")).is_some_and(|f| f.path(&["_styk", "movieclip"]).is_some_and(Value::truthy));
        if self.sheet() && self.selected_objects()?.iter().any(clip) {
            return Err(EditError::Unported("deleting a movieclip instance".into()));
        }
        let roots = self.roots()?;
        let gone = |object: &Value| uid(object).is_some_and(|u| selected.contains(&u));
        self.transaction("Delete figures", |editor| {
            let Some(frame) = editor.draft_mut() else { return Ok(()) };
            let objects = objects_of(&Value::object(frame.clone()));
            let order: Vec<f64> = frame_order(frame).into_iter().filter(|u| !selected.contains(&(*u as u32))).collect();
            let mut kept = Vec::new();
            for object in &objects {
                let mut record = object.as_obj().cloned().unwrap_or_default();
                if let Some(parent) = join_target(object).and_then(|slot| objects.get(slot)) {
                    set_join_field(&mut record, "target_unique_id", parent.get("unique_id").cloned());
                    if !gone(object) && gone(parent) {
                        if let Some(root) = uid(object).and_then(|u| roots.get(&u)) {
                            record.set("root_x", Value::Num(root[0]));
                            record.set("root_y", Value::Num(root[1]));
                        }
                        record.set("join", Value::Null);
                    }
                }
                if !gone(object) {
                    kept.push(record);
                }
            }
            let ids: Vec<f64> = kept.iter().map(|record| record.get("unique_id").and_then(Value::as_f64).unwrap_or(f64::NAN)).collect();
            let renumbered = kept.into_iter().enumerate().map(|(slot, mut record)| {
                record.set("object_slot", Value::Num(slot as f64));
                let target = record.get("join").and_then(|j| num(j, "target_unique_id")).and_then(|u| ids.iter().position(|id| *id == u));
                set_join_field(&mut record, "target_object_slot", target.map(|slot| Value::Num(slot as f64)));
                Value::object(record)
            });
            frame.set("objects", Value::array(renumbered.collect()));
            set_order(frame, &order);
            editor.selection = crate::Selection { uid: None, uids: Vec::new(), joint: -1 };
            Ok(())
        })
    }

    /// `duplicateFigure()`: a copy of the selected figure beside it, selected.
    pub(crate) fn duplicate_figure(&mut self) -> Result<Value, EditError> {
        if self.selected_objects()?.len() > 1 {
            return Err(EditError::Unported("duplicating several figures".into()));
        }
        let Some((_, object)) = self.object()? else { return Ok(Value::Null) };
        let figures: Vec<Value> = self.report.get("figure_types").and_then(Value::elements).map(|e| e.iter().collect()).unwrap_or_default();
        if figures.iter().find(|f| num(f, "figure_type_id") == num(&object, "figure_type_id")).is_some_and(|f| f.path(&["_styk", "movieclip"]).is_some_and(Value::truthy)) {
            return Err(EditError::Unported("duplicating a movieclip instance".into()));
        }
        if self.next_uid > 0x7fff_fffe {
            return Err(EditError::Invalid("errors.editorLayerSheet.objectIdentityCapacityReached".into()));
        }
        let identity = self.next_uid;
        self.next_uid += 1;
        let original = uid(&object);
        let mut copy = object.as_obj().cloned().unwrap_or_default();
        copy.set("unique_id", Value::Num(identity as f64));
        copy.remove("_stykMovieclipPart");
        if let Some(original) = original {
            let layer = self.layer_of(original);
            self.new_layer_hints.insert(identity as u32, layer);
        }
        let root = match self.selected_slot()? {
            Some(_) => original.and_then(|u| self.roots().ok()?.get(&u).copied()),
            None => None,
        };
        let at = |key: &str, world: Option<f64>| world.or_else(|| copy.get(key).and_then(Value::as_f64)).unwrap_or(f64::NAN) + 24.0;
        let (x, y) = (at("root_x", root.map(|r| r[0])), at("root_y", root.map(|r| r[1])));
        copy.set("root_x", Value::Num(x));
        copy.set("root_y", Value::Num(y));
        // Every controller target of the copy moves with it.
        let source = Value::object(copy.clone());
        crate::ik::translate_targets(&mut copy, 24.0, 24.0, &source, false);
        copy.set("join", Value::Null);
        let copy = Value::object(copy);
        self.transaction("Duplicate figure", |editor| {
            let Some(frame) = editor.draft_mut() else { return Ok(()) };
            let mut ids = frame_order(frame);
            let mut objects = objects_of(&Value::object(frame.clone()));
            objects.push(copy);
            frame.set("objects", Value::array(objects));
            ids.push(identity as f64);
            set_order(frame, &ids);
            normalize_frame(frame);
            editor.selection = crate::Selection { uid: Some(identity as u32), uids: vec![identity as u32], joint: -1 };
            Ok(())
        })?;
        Ok(self.object()?.map(|(_, o)| o).unwrap_or(Value::Null))
    }
}
