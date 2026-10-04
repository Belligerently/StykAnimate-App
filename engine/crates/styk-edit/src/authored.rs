//! Gestures on a layer sheet (src/editor-authored-pose.mjs): a gesture edits a draft
//! of the frame on show, never the keys it was evaluated from, and commits by
//! capturing the draft into the tracks of the layers it changed ([`crate::sheet`]).

use std::collections::HashMap;

use styk_doc::Value;

use crate::frame::objects_of;
use crate::record::{join_target, same, uid};
use crate::select::resolve_join_selection;
use crate::{Authored, Drag, EditError, Editor};

impl Editor {
    /// `_beginAuthoredPoseDrag(label, scope)`: drafts the frame at the playhead.
    pub(crate) fn begin_authored_drag(&mut self, label: &str, translation: bool) -> Result<bool, EditError> {
        if self.tick.fract() != 0.0 {
            // `Math.round`.
            self.tick = (self.tick + 0.5).floor();
        }
        let source = self.shown_frame()?;
        let objects = objects_of(&source);
        if !objects.iter().any(|o| uid(o).is_some_and(|u| self.selection.uids.contains(&u))) {
            return Ok(false);
        }
        // Moving takes whole connected figures; posing takes the selected figures and
        // what is joined to them, and leaves what they are joined to in place.
        let connected = resolve_join_selection(&objects, &self.selection.uids, true)?;
        let slots = if translation { connected } else { resolve_join_selection(&objects, &self.selection.uids, false)? };
        let slots = Editor::ik_pose_slots(&objects, slots);
        self.authored = Some(Authored { frame: source.clone(), source, tick: self.tick, transaction: false });
        self.drag = Some(Drag {
            label: label.to_string(),
            translation,
            index: self.frame_index,
            before: slots.iter().map(|&slot| objects[slot].clone()).collect(),
            before_selection: self.selection_state(),
            changed: false,
            detached: HashMap::new(),
            rotation_targets: HashMap::new(),
            spatial_gestures: HashMap::new(),
            spatial_kinds: HashMap::new(),
            ik_captures: HashMap::new(),
            spatial_ik_captures: HashMap::new(),
            ik_resume: HashMap::new(),
            handle_edit: None,
            figure_handle: None,
            segment_value_options: None,
            camera: None,
            rig: None,
            authored: true,
        });
        Ok(true)
    }

    /// `commitAuthoredPoseDrag`: nothing changed, or the draft goes to the layer sheet.
    pub(crate) fn commit_authored_drag(&mut self) -> Result<bool, EditError> {
        let (Some(drag), Some(edit)) = (self.drag.as_ref(), self.authored.as_ref()) else { return Ok(false) };
        let mut ids: Vec<u32> = drag.before.iter().filter_map(uid).collect();
        if edit.transaction {
            ids.extend(objects_of(&edit.frame).iter().filter_map(uid).filter(|u| !ids.contains(u)).collect::<Vec<_>>());
        }
        let after: Vec<Value> = objects_of(&edit.frame).into_iter().filter(|o| uid(o).is_some_and(|u| ids.contains(&u))).collect();
        // A gesture compares the figures it took; a command's draft, the whole frame.
        let unchanged = if edit.transaction { same(std::slice::from_ref(&edit.source), std::slice::from_ref(&edit.frame)) } else { same(&drag.before, &after) };
        if unchanged {
            self.drag = None;
            self.authored = None;
            self.retained = None;
            return Ok(false);
        }
        let committed = self.finish_authored_drag(&ids, &after);
        if committed.is_err() {
            // The draft is released on failure too, or every seek would retry it.
            self.cancel_authored_drag()?;
        }
        committed
    }

    fn finish_authored_drag(&mut self, ids: &[u32], after: &[Value]) -> Result<bool, EditError> {
        // A pose stores where its joined figures sit as well as their joins.
        if after.iter().any(|o| join_target(o).is_some()) {
            let roots = self.roots_of(ids)?;
            for (slot, object) in self.frame_objects().iter().enumerate() {
                let Some(id) = uid(object).filter(|u| ids.contains(u)) else { continue };
                if join_target(object).is_none() {
                    continue;
                }
                if let (Some(root), Some(record)) = (roots.get(&id), self.frame_object_mut(slot)) {
                    record.set("root_x", Value::Num(root[0]));
                    record.set("root_y", Value::Num(root[1]));
                }
            }
        }
        self.commit_sheet_pose()
    }

    /// `transaction(label, mutate)` on a layer sheet (`_authoredTransaction`): `mutate`
    /// changes a draft of the whole frame on show, figures included, which commits
    /// through the sheet like a gesture.
    pub(crate) fn transaction(&mut self, label: &str, mutate: impl FnOnce(&mut Editor) -> Result<(), EditError>) -> Result<bool, EditError> {
        if self.drag.is_some() {
            self.commit_drag()?;
        }
        if !self.sheet() {
            return Err(EditError::Unported(format!("{label} on a native timeline")));
        }
        if self.tick.fract() != 0.0 {
            self.tick = (self.tick + 0.5).floor();
        }
        let source = self.shown_frame()?;
        self.authored = Some(Authored { frame: source.clone(), source: source.clone(), tick: self.tick, transaction: true });
        self.drag = Some(Drag {
            label: label.to_string(),
            translation: false,
            index: self.frame_index,
            before: objects_of(&source),
            before_selection: self.selection_state(),
            changed: false,
            detached: HashMap::new(),
            rotation_targets: HashMap::new(),
            spatial_gestures: HashMap::new(),
            spatial_kinds: HashMap::new(),
            ik_captures: HashMap::new(),
            spatial_ik_captures: HashMap::new(),
            ik_resume: HashMap::new(),
            handle_edit: None,
            figure_handle: None,
            segment_value_options: None,
            camera: None,
            rig: None,
            authored: true,
        });
        let result = mutate(self).and_then(|()| self.commit_authored_drag());
        if result.is_err() {
            self.cancel_authored_drag()?;
        }
        result
    }

    /// The draft's frame record, to change (`this.frame` inside a transaction).
    pub(crate) fn draft_mut(&mut self) -> Option<&mut styk_doc::Obj> {
        self.authored.as_mut()?.frame.make_obj()
    }

    /// `cancelAuthoredPoseDrag`: the draft is dropped; the document was never touched.
    pub(crate) fn cancel_authored_drag(&mut self) -> Result<bool, EditError> {
        if self.authored.is_none() {
            return Ok(false);
        }
        let drag = self.drag.take();
        self.authored = None;
        self.retained = None;
        let mut changed = Vec::new();
        if let Some(drag) = drag {
            self.restore_selection(&drag.before_selection)?;
            changed = drag.before.iter().filter_map(uid).map(|u| Value::Num(u as f64)).collect();
        }
        self.emit("edit", vec![("label", Value::str("Cancel pose")), ("animation", Value::Bool(true)), ("changedUids", Value::array(changed))]);
        Ok(true)
    }
}
