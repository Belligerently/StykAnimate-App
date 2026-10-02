//! Gestures on figures (`beginDrag`, `_objectEdit`, `touch`, `commitDrag`,
//! `cancelDrag`) and moving them (`moveSelection`).

use std::collections::HashMap;
use std::sync::Arc;

use styk_doc::Value;

use crate::history::{Command, CommandKind};
use crate::record::{frame_objects, join_target, num, object_mut, same, uid};
use crate::select::resolve_join_selection;
use crate::timeline::rows;
use crate::{Drag, EditError, Editor};

const NOT_FINITE: &str = "errors.editorCamera.mustBeFinite";

/// `moveSelection`'s options.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct MoveOptions {
    pub from_start: bool,
    pub detach_external: bool,
}

impl MoveOptions {
    pub(crate) fn from_value(options: &Value) -> MoveOptions {
        let flag = |key| options.get(key).is_some_and(Value::truthy);
        MoveOptions { from_start: flag("fromStart"), detach_external: flag("detachExternal") }
    }
}

impl Editor {
    /// `beginDrag(label, {scope})`: snapshots the figures the gesture may change.
    pub(crate) fn begin_drag(&mut self, label: &str, translation: bool) -> Result<bool, EditError> {
        if self.drag.is_some() {
            self.commit_drag()?;
        }
        let Some((_, object)) = self.object() else { return Ok(false) };
        self.selection.uid = uid(&object);
        if let Some(row) = rows(&self.report).get(self.frame_index) {
            self.tick = row.start as f64;
        } else {
            self.tick = 0.0;
        }
        let objects = frame_objects(&self.report, self.frame_index);
        let slots = resolve_join_selection(&objects, &self.selection.uids, translation)?;
        self.drag = Some(Drag {
            label: label.to_string(),
            translation,
            index: self.frame_index,
            before: slots.iter().map(|&slot| objects[slot].clone()).collect(),
            before_selection: self.selection_state(),
            changed: false,
            detached: HashMap::new(),
        });
        Ok(true)
    }

    /// `_expandTranslationDrag`: a pose gesture that starts moving takes in whole connected figures.
    fn expand_translation_drag(&mut self) -> Result<(), EditError> {
        let Some(drag) = self.drag.as_ref().filter(|d| !d.translation) else { return Ok(()) };
        let objects = frame_objects(&self.report, self.frame_index);
        let slots = resolve_join_selection(&objects, &self.selection.uids, true)?;
        let snapshots: HashMap<u32, Value> = drag.before.iter().filter_map(|o| Some((uid(o)?, o.clone()))).collect();
        let before = slots.iter().map(|&slot| uid(&objects[slot]).and_then(|u| snapshots.get(&u).cloned()).unwrap_or_else(|| objects[slot].clone())).collect();
        let drag = self.drag.as_mut().unwrap();
        drag.before = before;
        drag.translation = true;
        Ok(())
    }

    /// `_objectEdit`: runs `mutate` inside the current gesture, or as a gesture of its own.
    fn object_edit(&mut self, label: &str, mutate: impl FnOnce(&mut Editor) -> Result<(), EditError>) -> Result<bool, EditError> {
        if self.object().is_none() {
            return Ok(false);
        }
        let active = self.drag.is_some();
        if !active {
            self.begin_drag(label, false)?;
        }
        let result = mutate(self).and_then(|()| {
            self.touch();
            if !active { self.commit_drag().map(|_| ()) } else { Ok(()) }
        });
        if let Err(error) = result {
            self.cancel_drag()?;
            return Err(error);
        }
        Ok(true)
    }

    /// `touch`: marks the gesture changed and announces the live edit.
    fn touch(&mut self) {
        let changed = match self.drag.as_mut() {
            Some(drag) => {
                drag.changed = true;
                drag.before.iter().filter_map(uid).collect::<Vec<_>>()
            }
            None => {
                self.revision_serial += 1;
                self.revision = self.revision_serial;
                self.selection.uid.into_iter().collect()
            }
        };
        let live = self.drag.is_some();
        self.emit("edit", vec![("live", Value::Bool(live)), ("changedUids", uid_array(&changed))]);
    }

    pub(crate) fn commit_drag(&mut self) -> Result<bool, EditError> {
        let Some(drag) = self.drag.as_ref() else { return Ok(false) };
        let ids: Vec<u32> = drag.before.iter().filter_map(uid).collect();
        let index = drag.index;
        let collect = |editor: &Editor| frame_objects(&editor.report, index).into_iter().filter(|o| uid(o).is_some_and(|u| ids.contains(&u))).collect::<Vec<_>>();
        let mut after = collect(self);
        if same(&drag.before, &after) {
            self.drag = None;
            return Ok(false);
        }
        // Native Pivot stores final attachment coordinates as well as join links.
        if after.iter().any(|o| join_target(o).is_some()) {
            let roots = self.roots()?;
            let objects = frame_objects(&self.report, index);
            for (slot, object) in objects.iter().enumerate() {
                let Some(id) = uid(object).filter(|u| ids.contains(u)) else { continue };
                if join_target(object).is_none() {
                    continue;
                }
                if let (Some(root), Some(record)) = (roots.get(&id), object_mut(&mut self.report, index, slot)) {
                    record.set("root_x", Value::Num(root[0]));
                    record.set("root_y", Value::Num(root[1]));
                }
            }
            after = collect(self);
        }
        let drag = self.drag.take().unwrap();
        let after_selection = self.selection_state();
        self.record(Command {
            kind: CommandKind::Objects,
            label: drag.label.clone(),
            index,
            before: drag.before,
            after,
            before_selection: drag.before_selection,
            after_selection,
            before_revision: 0,
            after_revision: 0,
        });
        self.emit("edit", vec![("label", Value::str(&drag.label)), ("changedUids", uid_array(&ids))]);
        Ok(true)
    }

    pub(crate) fn cancel_drag(&mut self) -> Result<bool, EditError> {
        let Some(drag) = self.drag.take() else { return Ok(false) };
        self.replace_objects(drag.index, &drag.before);
        let ids: Vec<u32> = drag.before.iter().filter_map(uid).collect();
        self.emit("edit", vec![("label", Value::str("Cancel pose")), ("changedUids", uid_array(&ids))]);
        Ok(true)
    }

    /// `moveSelection`: translates connected roots once, including when a selected figure is a joined child.
    pub(crate) fn move_selection(&mut self, dx: f64, dy: f64, options: &Value) -> Result<bool, EditError> {
        if !dx.is_finite() || !dy.is_finite() {
            return Err(EditError::Invalid(NOT_FINITE.into()));
        }
        let options = MoveOptions::from_value(options);
        self.object_edit("Move figures", |editor| {
            if !options.detach_external {
                editor.expand_translation_drag()?;
            }
            let index = editor.frame_index;
            let snapshots: HashMap<u32, Value> = editor.drag.as_ref().unwrap().before.iter().filter_map(|o| Some((uid(o)?, o.clone()))).collect();
            let objects = frame_objects(&editor.report, index);
            let affected: Vec<usize> = (0..objects.len()).filter(|&slot| uid(&objects[slot]).is_some_and(|u| snapshots.contains_key(&u))).collect();
            let external: Vec<usize> = affected
                .iter()
                .copied()
                .filter(|&slot| join_target(&objects[slot]).is_some_and(|target| !objects.get(target).and_then(uid).is_some_and(|u| snapshots.contains_key(&u))))
                .collect();
            let mut detached: HashMap<u32, [f64; 2]> = HashMap::new();
            if !external.is_empty() {
                let roots = editor.roots()?;
                for &slot in &external {
                    let id = uid(&objects[slot]).unwrap();
                    let record = object_mut(&mut editor.report, index, slot).unwrap();
                    if let Some(root) = roots.get(&id) {
                        detached.insert(id, *root);
                        record.set("root_x", Value::Num(root[0]));
                        record.set("root_y", Value::Num(root[1]));
                    }
                    record.set("join", Value::Null);
                }
                editor.drag.as_mut().unwrap().detached.extend(detached.iter().map(|(k, v)| (*k, *v)));
            }
            for &slot in &affected {
                let object = frame_objects(&editor.report, index).swap_remove(slot);
                if join_target(&object).is_some() {
                    continue;
                }
                let id = uid(&object).unwrap();
                let (x, y) = if options.from_start {
                    match editor.drag.as_ref().unwrap().detached.get(&id) {
                        Some(root) => (root[0], root[1]),
                        None => {
                            let baseline = &snapshots[&id];
                            (num(baseline, "root_x").unwrap_or(f64::NAN), num(baseline, "root_y").unwrap_or(f64::NAN))
                        }
                    }
                } else {
                    (num(&object, "root_x").unwrap_or(f64::NAN), num(&object, "root_y").unwrap_or(f64::NAN))
                };
                let record = object_mut(&mut editor.report, index, slot).unwrap();
                record.set("root_x", Value::Num(x + dx));
                record.set("root_y", Value::Num(y + dy));
                record.remove("runtime_segments");
            }
            Ok(())
        })
    }

    /// World positions of every figure's root at the playhead, by unique id (`editor.geometry`).
    fn roots(&mut self) -> Result<HashMap<u32, [f64; 2]>, EditError> {
        let document = self.loader.document(&self.report, "").map_err(|error| EditError::Invalid(error.to_string()))?;
        let prepared = styk_eval::eval::Prepared::new(Arc::new(document));
        let frame = prepared.evaluate(self.tick);
        let scene = styk_eval::geometry::build(&prepared.doc, &frame);
        Ok(scene.objects.iter().map(|object| (object.uid, object.root)).collect())
    }
}

fn uid_array(uids: &[u32]) -> Value {
    Value::array(uids.iter().map(|&u| Value::Num(u as f64)).collect())
}
