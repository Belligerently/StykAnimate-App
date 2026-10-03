//! Selection and the playhead (`select`, `selectMany`, `seek`, `_selectionState`,
//! `_restoreSelection`), and the timeline's layer selection (`selectSheetLayer`,
//! `_followSheetSelection`, `keepSelectionOnSeek`, `setActiveLayer`).

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
    /// `editor._sheetSelection`.
    pub sheet: Option<SheetSelection>,
}

/// `editor._sheetSelection`: a layer chosen on the timeline, and whether the selection
/// follows its figures as the playhead moves.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SheetSelection {
    pub id: String,
    pub contents: bool,
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
    pub(crate) fn set_active_layer(&mut self, id: &str) {
        let layers = self.report.make_obj().and_then(|r| r.get_mut("_styk")).and_then(Value::make_obj).and_then(|s| s.get_mut("layers")).and_then(Value::make_obj);
        if let Some(layers) = layers {
            layers.set("activeId", Value::str(id));
        }
    }

    pub(crate) fn selection_state(&self) -> SelectionState {
        SelectionState { frame_index: self.frame_index, tick: self.tick, selection: self.selection.clone(), active_layer: self.active_layer(), sheet: self.sheet_selection.clone() }
    }

    /// `getLayer(id)` exists: the scene, the camera or a user layer.
    pub(crate) fn layer_exists(&self, id: &str) -> bool {
        ["scene", "camera"].contains(&id) || self.report.path(&["_styk", "layers", "order"]).and_then(Value::elements).is_some_and(|order| order.iter().any(|layer| layer.as_str() == Some(id)))
    }

    /// `setActiveLayer(id)`.
    pub(crate) fn set_active_layer_command(&mut self, id: &Value) -> Result<bool, EditError> {
        let id = id.as_str().filter(|id| self.layer_exists(id)).ok_or_else(|| EditError::Invalid("errors.editorLayers.chooseAnExistingLayer".into()))?.to_string();
        if self.report.path(&["_styk", "layers"]).is_none() {
            return Err(EditError::Unported("an active layer without layer state".into()));
        }
        if self.report.path(&["_styk", "layers", "activeId"]).and_then(Value::as_str) == Some(id.as_str()) {
            return Ok(false);
        }
        self.set_active_layer(&id);
        self.emit("selection", vec![("layerSelection", Value::Bool(true)), ("activeLayerId", Value::str(&id)), ("layerRevision", Value::Num(self.layer_revision as f64))]);
        Ok(true)
    }

    /// `selectSheetLayer(id, {contents})`: the layer becomes active and, with its contents,
    /// its figures on show are selected and stay selected as the playhead moves.
    pub(crate) fn select_sheet_layer(&mut self, id: &Value, options: &Value) -> Result<bool, EditError> {
        let Some(id) = id.as_str().filter(|id| self.layer_exists(id)).map(str::to_string) else { return Ok(false) };
        let contents = options.get("contents").is_none_or(Value::truthy);
        self.set_active_layer_command(&Value::str(&id))?;
        self.sheet_selection = Some(SheetSelection { id, contents });
        if contents {
            self.follow_sheet_selection()?;
        }
        self.emit("selection", vec![("sheetSelection", Value::Bool(true))]);
        Ok(true)
    }

    /// `keepSelectionOnSeek()`: the figure selection stays as it is across seeks.
    pub(crate) fn keep_selection_on_seek(&mut self) {
        if let Some(selection) = self.sheet_selection.as_mut().filter(|s| s.contents) {
            selection.contents = false;
        }
    }

    /// `_followSheetSelection()`: the figures of the chosen layer on show are the selection.
    pub(crate) fn follow_sheet_selection(&mut self) -> Result<(), EditError> {
        let Some(chosen) = self.sheet_selection.clone().filter(|s| s.contents) else { return Ok(()) };
        if !self.sheet() {
            return Ok(());
        }
        let objects: Vec<Value> = crate::frame::objects_of(&self.shown_frame()?).into_iter().filter(|o| uid(o).is_some_and(|u| self.layer_of(u) == chosen.id)).collect();
        let ids: Vec<u32> = objects.iter().filter_map(uid).collect();
        if ids.len() != self.selection.uids.len() || ids.iter().any(|u| !self.selection.uids.contains(u)) {
            self.selection.uids = ids.clone();
        }
        let unjoined = objects.iter().find(|o| !o.get("join").is_some_and(Value::truthy)).and_then(uid);
        self.selection.uid = unjoined.or_else(|| ids.first().copied());
        self.selection.joint = -1;
        Ok(())
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
        self.sheet_selection = state.sheet.clone();
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
