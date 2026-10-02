//! Selection and the playhead (`select`, `selectMany`, `seek`, `_selectionState`,
//! `_restoreSelection`).

use styk_doc::Value;

use crate::record::{join_target, uid};
use crate::timeline::{rows, total_ticks};
use crate::{EditError, Editor};

/// The selected figures and joint.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Selection {
    pub uid: Option<u32>,
    /// Selected figures in selection order (a JavaScript `Set`).
    pub uids: Vec<u32>,
    pub joint: i64,
}

/// `_selectionState()`: what a history entry restores.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SelectionState {
    pub frame_index: usize,
    pub tick: f64,
    pub selection: Selection,
    /// `editor.activeLayerId`.
    pub active_layer: String,
}

impl Editor {
    /// `select(uid, joint, {toggle, add})`.
    pub(crate) fn select(&mut self, uid: Option<u32>, joint: i64, toggle: bool, add: bool) -> Result<Value, EditError> {
        let frame = self.selection_frame()?;
        let valid = uid.filter(|&selected| frame.iter().any(|o| crate::record::uid(o) == Some(selected)));
        // On a layer sheet a selected figure makes its layer the active one.
        if let (Some(selected), true) = (valid, self.sheet()) {
            let layer = self.layer_of(selected);
            self.set_active_layer(&layer);
        }
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
        Ok(self.object()?.map(|(_, o)| o).unwrap_or(Value::Null))
    }

    /// `selectMany(uids)`: returns the selected figures in slot order.
    pub(crate) fn select_many(&mut self, uids: &[u32]) -> Result<Value, EditError> {
        let frame = self.selection_frame()?;
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
        let next = tick.clamp(0.0, (total_ticks(&rows) - 1.0).max(0.0));
        let index = rows.partition_point(|r| r.start <= next).saturating_sub(1);
        self.tick = next;
        self.frame_index = index;
        self.retained = None;
        self.emit("seek", vec![]);
        // `seek` returns the evaluated frame, a view the facade reads from its own copy.
        Ok(Value::Null)
    }

    /// `editor.activeLayerId`.
    pub(crate) fn active_layer(&self) -> String {
        let layers = self.report.path(&["_styk", "layers"]);
        let named = |key: &str| layers.and_then(|l| l.get(key)).and_then(Value::as_str).map(str::to_string);
        named("activeId").or_else(|| named("defaultLayerId")).unwrap_or_else(|| "layer-1".into())
    }

    /// `report._styk.layers.activeId = id`.
    fn set_active_layer(&mut self, id: &str) {
        let layers = self.report.make_obj().and_then(|r| r.get_mut("_styk")).and_then(Value::make_obj).and_then(|s| s.get_mut("layers")).and_then(Value::make_obj);
        if let Some(layers) = layers {
            layers.set("activeId", Value::str(id));
        }
    }

    pub(crate) fn selection_state(&self) -> SelectionState {
        SelectionState { frame_index: self.frame_index, tick: self.tick, selection: self.selection.clone(), active_layer: self.active_layer() }
    }

    /// `_restoreSelection`.
    pub(crate) fn restore_selection(&mut self, state: &SelectionState) -> Result<(), EditError> {
        let frames = self.report.get("keyframes").and_then(Value::array_len).unwrap_or(1);
        self.frame_index = state.frame_index.min(frames.saturating_sub(1));
        let total = total_ticks(&rows(&self.report));
        self.tick = state.tick.clamp(0.0, (total - 1.0).max(0.0));
        let existing: Vec<u32> = self.selection_frame()?.iter().filter_map(uid).collect();
        let selected = state.selection.uid.filter(|u| existing.contains(u));
        let uids: Vec<u32> = state.selection.uids.iter().copied().filter(|u| existing.contains(u)).collect();
        let selected = if selected.is_some_and(|u| uids.contains(&u)) { selected } else { uids.last().copied() };
        self.selection = Selection { uid: selected, uids, joint: if selected.is_none() { -1 } else { state.selection.joint } };
        // The active layer comes back too, or the default one when it is gone.
        if let Some(layers) = self.report.path(&["_styk", "layers"]) {
            let known = ["scene", "camera"].contains(&state.active_layer.as_str())
                || layers.get("order").and_then(Value::elements).is_some_and(|order| order.iter().any(|id| id.as_str() == Some(state.active_layer.as_str())));
            let active = if known { state.active_layer.clone() } else { layers.get("defaultLayerId").and_then(Value::as_str).unwrap_or("").to_string() };
            self.set_active_layer(&active);
        }
        Ok(())
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
