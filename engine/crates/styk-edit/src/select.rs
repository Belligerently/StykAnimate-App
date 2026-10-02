//! Selection and the playhead (`select`, `seek`, `_restoreSelection`).

use styk_doc::Value;

use crate::record::{frame_objects, join_target, uid};
use crate::timeline::{rows, total_ticks};
use crate::{EditError, Editor};

/// `_selectionState()`: what a history entry restores.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Selection {
    pub uid: Option<u32>,
    /// Selected figures in selection order (a JavaScript `Set`).
    pub uids: Vec<u32>,
    pub joint: i64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SelectionState {
    pub frame_index: usize,
    pub tick: f64,
    pub selection: Selection,
}

impl Editor {
    /// The frame selection and editing read: on a native timeline, the keyframe at the playhead.
    pub(crate) fn selection_frame(&self) -> Vec<Value> {
        frame_objects(&self.report, self.frame_index)
    }

    /// `editor.object`: the selected figure's record in the selection frame, else the
    /// last selected figure in slot order.
    pub(crate) fn object(&self) -> Option<(usize, Value)> {
        let frame = self.selection_frame();
        let found = self.selection.uid.and_then(|selected| frame.iter().position(|o| uid(o) == Some(selected)));
        let slot = found.or_else(|| frame.iter().rposition(|o| uid(o).is_some_and(|u| self.selection.uids.contains(&u))))?;
        Some((slot, frame[slot].clone()))
    }

    /// `select(uid, joint, {toggle, add})`.
    pub(crate) fn select(&mut self, uid: Option<u32>, joint: i64, toggle: bool, add: bool) -> Result<Value, EditError> {
        let valid = uid.filter(|&selected| self.selection_frame().iter().any(|o| crate::record::uid(o) == Some(selected)));
        match valid {
            None => self.selection = Selection { uid: None, uids: Vec::new(), joint: -1 },
            Some(selected) if toggle || add => {
                if let Some(at) = self.selection.uids.iter().position(|&u| u == selected).filter(|_| toggle) {
                    self.selection.uids.remove(at);
                } else if !self.selection.uids.contains(&selected) {
                    self.selection.uids.push(selected);
                }
                self.selection.uid = if self.selection.uids.contains(&selected) { Some(selected) } else { self.selection.uids.last().copied() };
            }
            Some(selected) => self.selection = Selection { uid: Some(selected), uids: vec![selected], joint },
        }
        self.selection.joint = if self.selection.uids.len() == 1 { joint } else { -1 };
        self.emit("selection", vec![]);
        Ok(self.object().map(|(_, o)| o).unwrap_or(Value::Null))
    }

    /// `selectMany(uids)`: returns the selected figures in slot order.
    pub(crate) fn select_many(&mut self, uids: &[u32]) -> Result<Value, EditError> {
        let frame = self.selection_frame();
        let mut selected = Vec::new();
        for &u in uids {
            if frame.iter().any(|o| uid(o) == Some(u)) && !selected.contains(&u) {
                selected.push(u);
            }
        }
        self.selection = Selection { uid: selected.last().copied(), uids: selected, joint: -1 };
        self.emit("selection", vec![]);
        Ok(Value::array(frame.into_iter().filter(|o| uid(o).is_some_and(|u| self.selection.uids.contains(&u))).collect()))
    }

    pub(crate) fn seek(&mut self, tick: f64) -> Result<Value, EditError> {
        if !tick.is_finite() {
            return Err(EditError::Invalid("errors.editorCamera.mustBeFinite".into()));
        }
        if self.drag.is_some() {
            self.commit_drag()?;
        }
        let rows = rows(&self.report);
        let next = tick.clamp(0.0, (total_ticks(&rows) as f64 - 1.0).max(0.0));
        let index = rows.partition_point(|r| r.start as f64 <= next).saturating_sub(1);
        self.tick = next;
        self.frame_index = index;
        self.emit("seek", vec![]);
        // `seek` returns the evaluated frame, a view the facade reads from its own copy.
        Ok(Value::Null)
    }

    pub(crate) fn selection_state(&self) -> SelectionState {
        SelectionState { frame_index: self.frame_index, tick: self.tick, selection: self.selection.clone() }
    }

    /// `_restoreSelection`.
    pub(crate) fn restore_selection(&mut self, state: &SelectionState) {
        let frames = self.report.get("keyframes").and_then(Value::array_len).unwrap_or(1);
        self.frame_index = state.frame_index.min(frames.saturating_sub(1));
        let total = total_ticks(&rows(&self.report)) as f64;
        self.tick = state.tick.clamp(0.0, (total - 1.0).max(0.0));
        let existing: Vec<u32> = self.selection_frame().iter().filter_map(uid).collect();
        let selected = state.selection.uid.filter(|u| existing.contains(u));
        let uids: Vec<u32> = state.selection.uids.iter().copied().filter(|u| existing.contains(u)).collect();
        let selected = if selected.is_some_and(|u| uids.contains(&u)) { selected } else { uids.last().copied() };
        self.selection = Selection { uid: selected, uids, joint: if selected.is_none() { -1 } else { state.selection.joint } };
    }
}

/// `resolveJoinSelection` (src/pivot/structure.mjs): the slots a gesture on
/// `uids` affects. With `ancestors`, a selected joined figure stands for the
/// root of its assembly. Returns the affected slots in slot order.
pub(crate) fn resolve_join_selection(objects: &[Value], uids: &[u32], ancestors: bool) -> Result<Vec<usize>, EditError> {
    let mut selected = Vec::new();
    let mut children: std::collections::HashMap<usize, Vec<usize>> = std::collections::HashMap::new();
    for (slot, object) in objects.iter().enumerate() {
        if uid(object).is_some_and(|u| uids.contains(&u)) {
            selected.push(slot);
        }
        if let Some(parent) = join_target(object) {
            children.entry(parent).or_default().push(slot);
        }
    }
    let mut roots = Vec::new();
    let mut resolved: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    for &start in &selected {
        let mut slot = start;
        if ancestors {
            let (mut path, mut visiting) = (Vec::new(), std::collections::HashSet::new());
            while !resolved.contains_key(&slot) && objects.get(slot).and_then(join_target).is_some() {
                if !visiting.insert(slot) {
                    return Err(EditError::Invalid("errors.editorSelection.figuresContainACyclicJoin".into()));
                }
                path.push(slot);
                slot = join_target(&objects[slot]).unwrap();
                if objects.get(slot).is_none() {
                    return Err(EditError::Invalid("errors.editorSelection.joinTargetIsMissing".into()));
                }
            }
            slot = resolved.get(&slot).copied().unwrap_or(slot);
            for visited in path {
                resolved.insert(visited, slot);
            }
        }
        if !roots.contains(&slot) {
            roots.push(slot);
        }
    }
    let mut affected: Vec<usize> = roots.clone();
    let mut queue = roots;
    let mut q = 0;
    while q < queue.len() {
        for &child in children.get(&queue[q]).map(Vec::as_slice).unwrap_or(&[]) {
            if !affected.contains(&child) {
                affected.push(child);
                queue.push(child);
            }
        }
        q += 1;
    }
    affected.sort_unstable();
    Ok(affected)
}
