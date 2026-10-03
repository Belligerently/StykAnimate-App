//! The layer timeline's clipboard (src/editor-layer-sheet.mjs): `sheetCopyFrames` keeps
//! each layer's poses and easing over a range, `sheetPasteFrames` writes them at another
//! frame, over what is there or inserted, on the same layers or on others (where the
//! figures take new identities). The clipboard belongs to the editor that copied: a
//! session pastes only what it copied itself (the facade checks that the editor's
//! clipboard is that one). Drawing layers, movieclip parts and movieclips on the
//! presence clock answer `Unported` when pasted, as does a clipboard from another
//! document, which the engine never holds.

use styk_doc::{Obj, Value};

use crate::frame::objects_of;
use crate::record::uid;
use crate::sheet::{constant, continuations, fixed, items, key_at, key_easing, num, Write, MAX_SHEET_TICKS};
use crate::{EditError, Editor};

/// `editor._sheetClipboard`, as far as pasting it in the same document reads it.
#[derive(Clone, Debug)]
pub(crate) struct Clipboard {
    start: f64,
    duration: f64,
    rows: Vec<Row>,
    /// Why pasting it stays in JavaScript, if it does.
    unported: Option<&'static str>,
}

#[derive(Clone, Debug)]
struct Row {
    id: String,
    drawing: bool,
    keys: Vec<Entry>,
}

#[derive(Clone, Debug)]
struct Entry {
    offset: f64,
    frame: Value,
    easing: Value,
}

fn invalid(key: &str) -> EditError {
    EditError::Invalid(format!("errors.editorLayerSheet.{key}"))
}

/// The layer ids of a call (an array of text), refusing repeated ones with `repeated`.
fn distinct(ids: &Value, repeated: &str) -> Result<Vec<String>, EditError> {
    let list: Vec<String> = match ids.elements() {
        Some(ids) => ids.iter().map(|id| id.as_str().map(str::to_string).ok_or_else(|| EditError::Unported("a layer id that is not text".into()))).collect::<Result<_, _>>()?,
        None => return Err(EditError::Unported("layer ids that are not a list".into())),
    };
    if list.iter().collect::<std::collections::HashSet<_>>().len() != list.len() {
        return Err(invalid(repeated));
    }
    Ok(list)
}

/// `Array.prototype.slice(start, end)`.
fn js_slice<T: Clone>(list: &[T], start: i64, end: i64) -> Vec<T> {
    let len = list.len() as i64;
    let clamp = |at: i64| if at < 0 { (len + at).max(0) } else { at.min(len) };
    let (from, to) = (clamp(start), clamp(end));
    if from >= to { Vec::new() } else { list[from as usize..to as usize].to_vec() }
}

/// `remapIKFollowers(object, uidMap)`: followers in other figures follow their new
/// identities; those not pasted are dropped. Integer keys come out in ascending order,
/// as a JavaScript object orders them.
fn remap_followers(object: &mut Obj, map: &[(u32, u32)]) {
    let Some(controllers) = object.get("_stykIK").filter(|c| c.truthy()).and_then(Value::as_obj).cloned() else { return };
    let mut out = Obj::new();
    for (id, controller) in controllers.iter() {
        let mut next = controller.as_obj().cloned().unwrap_or_default();
        for field in ["joinedFollowers", "joinedAttachmentOffsets", "joinedSpatialFollowers"] {
            let Some(table) = controller.get(field).filter(|t| t.truthy()) else { continue };
            let mut entries: Vec<(u32, Value)> = table
                .as_obj()
                .map(|t| t.iter().filter_map(|(key, value)| key.parse::<u32>().ok().and_then(|old| map.iter().find(|(from, _)| *from == old)).map(|(_, to)| (*to, value.clone()))).collect())
                .unwrap_or_default();
            entries.sort_by_key(|(to, _)| *to);
            let mut remapped = Obj::new();
            for (to, value) in entries {
                remapped.set(&to.to_string(), value);
            }
            next.set(field, Value::object(remapped));
        }
        out.set(id, Value::object(next));
    }
    object.set("_stykIK", Value::object(out));
}

impl Editor {
    /// `assertLayers(editor, ids)`: layers that exist and are not locked.
    fn assert_layers(&self, ids: &[String]) -> Result<(), EditError> {
        let layers = self.report.path(&["_styk", "layers"]);
        if ids.is_empty() {
            return Err(invalid("selectALayer"));
        }
        for id in ids {
            let listed = fixed(id) || items(layers.and_then(|l| l.get("order"))).iter().any(|layer| layer.as_str() == Some(id.as_str()));
            if !listed {
                return Err(invalid("theSelectedLayerNoLongerExists"));
            }
            let state = layers.and_then(|l| if fixed(id) { l.path(&["fixed", id]) } else { l.path(&["byId", id]) });
            if state.and_then(|s| s.get("locked")).is_some_and(Value::truthy) {
                return Err(invalid("unlockBeforeEditingIt"));
            }
        }
        Ok(())
    }

    fn drawing_layer_id(&self, id: &str) -> bool {
        !fixed(id) && self.report.path(&["_styk", "layers", "byId", id, "drawing", "version"]).and_then(Value::as_f64) == Some(1.0)
    }

    /// `sheetCopyFrames(ids, start, end = start)`: each layer's poses at `start`, at its
    /// keys up to `end` and at its end inside the range, with their easing. Returns the
    /// clipboard, which the editor keeps to itself (not compared).
    pub(crate) fn sheet_copy_frames(&mut self, ids: &Value, start: f64, end: &Value) -> Result<Value, EditError> {
        let end = if end.is_null() { start } else { end.as_f64().unwrap_or(f64::NAN) };
        let ids = distinct(ids, "selectDistinctSourceLayers")?;
        self.assert_layers(&ids)?;
        let sheet = self.report.path(&["_styk", "layerSheet"]).cloned().ok_or_else(|| EditError::Unported("copying frames without a layer timeline".into()))?;
        let mut unported = None;
        let mut rows = Vec::with_capacity(ids.len());
        let mut types: Vec<f64> = Vec::new();
        for id in &ids {
            let track = sheet.path(&["tracks", id]).cloned();
            let keys = items(track.as_ref().and_then(|t| t.get("keys")));
            let mut ticks = vec![start];
            ticks.extend(keys.iter().filter_map(|k| num(k, "tick")).filter(|&t| t > start && t <= end));
            if !fixed(id) {
                let track_end = track.as_ref().and_then(|t| num(t, "end")).ok_or_else(|| EditError::Unported("a layer without a track".into()))?;
                if track_end > start && track_end <= end {
                    ticks.push(track_end);
                }
            }
            let mut entries = Vec::with_capacity(ticks.len());
            for tick in ticks {
                let frame = self.evaluated(tick, false)?;
                let (uids, steps): (Vec<u32>, Vec<Value>) = if fixed(id) {
                    (Vec::new(), Vec::new())
                } else {
                    let uids = objects_of(&frame).iter().filter_map(uid).filter(|u| self.layer_of(*u) == *id).collect();
                    let steps = continuations(&frame).into_iter().filter(|step| num(step, "uid").is_some_and(|u| self.layer_of(u as u32) == *id)).collect();
                    (uids, steps)
                };
                let owned = self.owned_frame(&frame, &uids, &steps)?;
                let objects = objects_of(&owned);
                types.extend(objects.iter().filter_map(|o| num(o, "figure_type_id")));
                if !continuations(&owned).is_empty() || objects.iter().any(|o| o.path(&["_stykMovieclip", "clock"]).and_then(Value::as_str) == Some("presence")) {
                    unported.get_or_insert("pasting movieclips on the presence clock");
                }
                if objects.iter().any(|o| o.get("_stykMovieclipPart").is_some_and(Value::truthy)) {
                    unported.get_or_insert("pasting movieclip parts");
                }
                let at = key_at(&keys, tick).map(|i| keys[i].clone()).unwrap_or_else(constant);
                let mut easing = Obj::new();
                for (name, value) in key_easing(&at) {
                    easing.set(name, value);
                }
                entries.push(Entry { offset: tick - start, frame: owned, easing: Value::object(easing) });
            }
            let drawing = self.drawing_layer_id(id);
            if drawing {
                unported.get_or_insert("pasting drawing keys");
            }
            rows.push(Row { id: id.clone(), drawing, keys: entries });
        }
        let figures = items(self.report.get("figure_types"));
        if figures.iter().any(|f| num(f, "figure_type_id").is_some_and(|t| types.contains(&t)) && f.path(&["_styk", "drawing"]).is_some_and(Value::truthy)) {
            unported.get_or_insert("pasting drawings");
        }
        self.sheet_clipboard = Some(Clipboard { start, duration: end - start + 1.0, rows, unported });
        Ok(Value::Null)
    }

    /// `sheetPasteFrames(ids, tick, {insert})`: the clipboard's rows on `ids` (or, given one
    /// layer, on it and the layers below it), replacing the frames they cover or inserted
    /// before what follows. Figures pasted on a layer other than theirs take new identities.
    pub(crate) fn sheet_paste_frames(&mut self, ids: &Value, tick: &Value, options: &Value) -> Result<bool, EditError> {
        let Some(clipboard) = self.sheet_clipboard.clone() else { return Err(invalid("copyTimelineFramesFirst")) };
        let tick = tick.as_f64().filter(|t| t.fract() == 0.0 && (0.0..MAX_SHEET_TICKS).contains(t)).ok_or_else(|| invalid("chooseAValidWholeFrame"))?;
        let ids = distinct(ids, "selectDistinctDestinationLayers")?;
        if let Some(reason) = clipboard.unported {
            return Err(EditError::Unported(reason.into()));
        }
        let insert = options.get("insert").is_some_and(Value::truthy);
        let rows = &clipboard.rows;
        if ids.len() != 1 && ids.len() != rows.len() {
            return Err(invalid("selectOneDestinationLayerOrTheSameNumber"));
        }
        let destinations = if ids.len() == rows.len() {
            ids.clone()
        } else {
            let mut ordered: Vec<String> = items(self.report.path(&["_styk", "layers", "order"])).iter().filter_map(|id| id.as_str().map(str::to_string)).collect();
            ordered.reverse();
            let offset = ordered.iter().position(|id| *id == ids[0]).map_or(-1, |at| at as i64);
            js_slice(&ordered, offset, offset + rows.len() as i64)
        };
        if destinations.len() != rows.len() {
            return Err(invalid("createEnoughDestinationLayersForThisRange"));
        }
        if rows.iter().zip(&destinations).any(|(row, id)| row.drawing != self.drawing_layer_id(id)) {
            return Err(invalid("pasteDrawingKeysIntoDrawingLayersAndFigure"));
        }
        if rows.iter().zip(&destinations).any(|(row, id)| (fixed(&row.id) || fixed(id)) && row.id != *id) {
            return Err(invalid("pasteFramesIntoTheSameTrackKindFigures"));
        }
        let mut plan = self.planner(&destinations)?;
        let finish = tick + clipboard.duration;
        if finish > MAX_SHEET_TICKS {
            return Err(invalid("pastedFramesExceedTheTimelineLimit"));
        }
        // Every identity of the range is allocated first, so joins to a figure that
        // appears later in the range follow it.
        let mut next_uid = self.next_uid;
        let mut map: Vec<(u32, u32)> = Vec::new();
        for (row, id) in rows.iter().zip(&destinations) {
            for entry in &row.keys {
                for old in objects_of(&entry.frame).iter().filter_map(uid) {
                    if row.id == *id && self.layer_of(old) == *id {
                        continue;
                    }
                    if !map.iter().any(|(from, _)| *from == old) {
                        if next_uid > 0x7ffffffe {
                            return Err(invalid("objectIdentityCapacityReached"));
                        }
                        map.push((old, next_uid as u32));
                        next_uid += 1;
                    }
                }
            }
        }
        let renamed = |old: u32| map.iter().find(|(from, _)| *from == old).map(|(_, to)| *to);
        for (row, id) in rows.iter().zip(&destinations) {
            let restore = self.evaluated(finish, false)?;
            let old_end = plan.end(id);
            if insert {
                let shifted = plan
                    .keys(id)
                    .into_iter()
                    .map(|key| match num(&key, "tick") {
                        Some(at) if at >= tick => {
                            let mut key = key.as_obj().cloned().unwrap_or_default();
                            key.set("tick", Value::Num(at + clipboard.duration));
                            Value::object(key)
                        }
                        _ => key,
                    })
                    .collect();
                plan.set_keys(id, shifted);
            } else {
                let kept = plan.keys(id).into_iter().filter(|k| num(k, "tick").is_some_and(|at| at < tick || at >= finish)).collect();
                plan.set_keys(id, kept);
            }
            if !fixed(id) && tick > old_end && !plan.keys(id).is_empty() {
                plan.write(id, old_end, &Value::object(Obj::new()), Write { blank: true, ..Write::default() })?;
            }
            let end = finish.max(old_end + if insert { clipboard.duration } else { 0.0 });
            plan.set_end(id, end);
            for entry in &row.keys {
                let mut frame = crate::sheet_frames::shift_clip_time(&entry.frame, tick - clipboard.start);
                if !map.is_empty() {
                    let mut record = frame.as_obj().cloned().unwrap_or_default();
                    let mut objects = objects_of(&frame);
                    for object in objects.iter_mut() {
                        let Some(old) = uid(object) else { continue };
                        let Some(record) = object.make_obj() else { continue };
                        if let Some(new) = renamed(old) {
                            record.set("unique_id", Value::Num(new as f64));
                            plan.assign(new, id);
                        }
                    }
                    for object in objects.iter_mut() {
                        let Some(record) = object.make_obj() else { continue };
                        if let Some(join) = record.get("join").filter(|j| j.truthy()).and_then(Value::as_obj).cloned() {
                            let mut join = join;
                            if let Some(target) = join.get("target_unique_id").and_then(Value::as_f64).and_then(|t| renamed(t as u32)) {
                                join.set("target_unique_id", Value::Num(target as f64));
                            }
                            record.set("join", Value::object(join));
                        }
                        remap_followers(record, &map);
                    }
                    let order: Vec<Value> = items(record.get("object_draw_order"))
                        .iter()
                        .map(|slot| slot.as_f64().and_then(|s| objects.get(s as usize)).and_then(|o| o.get("unique_id").cloned()).unwrap_or(Value::Null))
                        .collect();
                    record.set("objects", Value::array(objects));
                    record.set("object_draw_order_unique_ids", Value::array(order));
                    frame = Value::object(record);
                }
                plan.write(id, tick + entry.offset, &frame, Write { easing: Some(&entry.easing), ..Write::default() })?;
            }
            let keys = plan.keys(id);
            let at_finish = keys.iter().find(|k| num(k, "tick") == Some(finish));
            if !insert && finish < old_end && at_finish.is_none_or(|k| k.get("patchOnly").is_some_and(Value::truthy)) {
                plan.write(id, finish, &restore, Write::default())?;
            }
            plan.set_end(id, end);
            plan.duration = plan.duration.max(end);
        }
        let selection = self.selection_state();
        let published = self.publish(plan, "Paste frames", selection, None)?;
        self.next_uid = next_uid;
        Ok(published)
    }
}
