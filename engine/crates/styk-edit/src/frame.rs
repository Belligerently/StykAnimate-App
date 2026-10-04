//! The frames a command reads and writes: the keyframe or the draft being edited
//! (`editor.frame`), the frame selection reads (`editor.selectionFrame`), the frame on
//! show and its geometry (`editor.evaluatedFrame`, `editor.geometry`).
//!
//! The evaluated frame comes from the engine's record evaluation
//! (`styk_eval::records`), byte for byte what the editor evaluates, and its geometry
//! from the typed view of that same record.

use std::sync::Arc;

use styk_doc::{Obj, Value};
use styk_eval::eval::Prepared;
use styk_eval::records::Records;

use crate::record::{frame_mut, uid};
use crate::{EditError, Editor};

/// What evaluation keeps between calls, for as long as the document does not change.
#[derive(Clone, Default)]
pub(crate) struct Cache {
    pub loader: styk_eval::load::Loader,
    /// The document `prepared` and `frames` belong to (a shared node: any change to
    /// the editor's report makes it a different one).
    report: Option<Value>,
    prepared: Option<Arc<Prepared>>,
    /// Evaluated frames by tick and by whether movieclips were sampled; a few recent ones.
    frames: Vec<(u64, bool, bool, Value)>,
}

impl Cache {
    pub(crate) fn typed_worker(&self, report: &Value) -> Option<(&styk_eval::load::Loader, &Arc<Prepared>)> {
        if !self.report.as_ref().is_some_and(|cached| cached.same_node(report)) { return None; }
        Some((&self.loader, self.prepared.as_ref()?))
    }
}

impl From<styk_eval::records::Error> for EditError {
    fn from(error: styk_eval::records::Error) -> EditError {
        match error {
            styk_eval::records::Error::Unported(what) => EditError::Unported(what),
            styk_eval::records::Error::Invalid(what) => EditError::Invalid(what),
        }
    }
}

pub(crate) fn objects_of(frame: &Value) -> Vec<Value> {
    frame.get("objects").and_then(Value::elements).map(|e| e.iter().collect()).unwrap_or_default()
}

impl Editor {
    /// Whether the document is edited through its layer sheet (always, in the app).
    pub(crate) fn sheet(&self) -> bool {
        self.report.path(&["_styk", "layerSheet"]).is_some()
    }

    /// Starts from typed views already made for the current document (the engine's
    /// mirror), and the loader that made them, so the first call converts nothing.
    pub fn adopt_prepared(&mut self, loader: styk_eval::load::Loader, prepared: Arc<Prepared>) {
        let mut cache = self.eval.borrow_mut();
        cache.loader = loader;
        cache.prepared = Some(prepared);
        cache.report = Some(self.report.clone());
        cache.frames.clear();
    }

    /// The typed views of the current document.
    pub(crate) fn prepared(&self) -> Result<Arc<Prepared>, EditError> {
        let mut cache = self.eval.borrow_mut();
        if cache.prepared.is_none() || !cache.report.as_ref().is_some_and(|report| report.same_node(&self.report)) {
            if self.worker_prepared_only && self.report.path(&["_styk", "cameraMotion", "clips"]).and_then(Value::elements)
                .is_some_and(|clips| clips.iter().any(|clip| clip.get("type").and_then(Value::as_str) == Some("follow")
                    && clip.get("targets").and_then(Value::array_len).is_some_and(|count| count > 0)))
            {
                return Err(EditError::Unported("follow projections require this revision's worker-prepared document".into()));
            }
            let document = cache.loader.document(&self.report, "").map_err(|error| EditError::Invalid(error.to_string()))?;
            cache.prepared = Some(Arc::new(Prepared::new(Arc::new(document))));
            cache.report = Some(self.report.clone());
            cache.frames.clear();
        }
        Ok(cache.prepared.clone().unwrap())
    }

    /// `evaluatedFrame(report, …, tick)` as records; without `clips`, the frame a
    /// key's pose is captured from (`at(report, tick)`: movieclips not sampled).
    pub(crate) fn evaluated(&self, tick: f64, clips: bool) -> Result<Value, EditError> {
        self.evaluated_mode(tick, clips, false)
    }

    fn evaluated_mode(&self, tick: f64, clips: bool, editor_native: bool) -> Result<Value, EditError> {
        let editor_native = editor_native && !self.sheet();
        let prepared = self.prepared()?;
        if let Some((_, _, _, frame)) = self.eval.borrow().frames.iter().find(|(at, with, editor, _)| *at == tick.to_bits() && *with == clips && *editor == editor_native) {
            return Ok(frame.clone());
        }
        let records = Records::new(&self.report, &prepared)?;
        let frame = if editor_native { records.evaluated_editor_with(tick, clips)? } else { records.evaluated_with(tick, clips)? };
        let mut cache = self.eval.borrow_mut();
        if cache.frames.len() >= 4 {
            cache.frames.remove(0);
        }
        cache.frames.push((tick.to_bits(), clips, editor_native, frame.clone()));
        Ok(frame)
    }

    /// `at(report, tick, {skipMovieclipPatches: true})`: movieclips not sampled and the keys'
    /// movieclip frame replacements left out.
    pub(crate) fn evaluated_without_patches(&self, tick: f64) -> Result<Value, EditError> {
        let prepared = self.prepared()?;
        Ok(Records::new(&self.report, &prepared)?.evaluated_options(tick, false, true)?)
    }

    /// `editor.evaluatedFrame`: the draft of the gesture in progress, else the
    /// document at the playhead.
    pub(crate) fn shown_frame(&self) -> Result<Value, EditError> {
        // A property edit shows its preview over everything else (`_sheetPropertyPreview`).
        if let Some(preview) = self.property_preview() {
            return Ok(preview);
        }
        match (&self.authored, &self.retained) {
            (Some(edit), _) => Ok(edit.frame.clone()),
            (None, Some(frame)) => Ok(frame.clone()),
            (None, None) => self.evaluated_mode(self.tick, true, true),
        }
    }

    /// `editor.selectionFrame.objects`: what selection reads. On a layer sheet the
    /// frame on show; on a native timeline with no saved channels the keyframe
    /// at the playhead.
    pub(crate) fn selection_frame(&self) -> Result<Vec<Value>, EditError> {
        if self.authored.is_some() || self.sheet() || self.report.path(&["_styk", "animation", "channels"]).and_then(Value::array_len).unwrap_or(0) > 0 {
            return Ok(objects_of(&self.shown_frame()?));
        }
        Ok(crate::record::frame_objects(&self.report, self.frame_index))
    }

    /// `editor.frame.objects`: what a command changes. The gesture's draft, else the
    /// native keyframe at the playhead.
    pub(crate) fn frame_objects(&self) -> Vec<Value> {
        match &self.authored {
            Some(edit) => objects_of(&edit.frame),
            None => crate::record::frame_objects(&self.report, self.frame_index),
        }
    }

    /// A figure of `editor.frame`, to change.
    pub(crate) fn frame_object_mut(&mut self, slot: usize) -> Option<&mut Obj> {
        let frame = match self.authored.as_mut() {
            Some(edit) => edit.frame.make_obj()?,
            None => frame_mut(&mut self.report, self.frame_index)?,
        };
        frame.get_mut("objects")?.make_arr()?.get_mut(slot)?.make_obj()
    }

    /// Replace one pose already copied for a transform, without first copying its old
    /// object again through `frame_object_mut`. Frame/array ownership stays here.
    pub(crate) fn replace_frame_object(&mut self, slot: usize, object: Obj) -> Option<()> {
        let frame = match self.authored.as_mut() {
            Some(edit) => edit.frame.make_obj()?,
            None => frame_mut(&mut self.report, self.frame_index)?,
        };
        *frame.get_mut("objects")?.make_arr()?.get_mut(slot)? = Value::object(object);
        Some(())
    }

    /// `editor.object`: the selected figure's record in the selection frame, else the
    /// last selected figure in slot order.
    pub(crate) fn object(&self) -> Result<Option<(usize, Value)>, EditError> {
        let frame = self.selection_frame()?;
        let found = self.selection.uid.and_then(|selected| frame.iter().position(|o| uid(o) == Some(selected)));
        let slot = found.or_else(|| frame.iter().rposition(|o| uid(o).is_some_and(|u| self.selection.uids.contains(&u))));
        Ok(slot.map(|slot| (slot, frame[slot].clone())))
    }

    /// The figures selected in the selection frame, in slot order (`selectedObjects`).
    pub(crate) fn selected_objects(&self) -> Result<Vec<Value>, EditError> {
        Ok(self.selection_frame()?.into_iter().filter(|o| uid(o).is_some_and(|u| self.selection.uids.contains(&u))).collect())
    }

    /// The frame on show as the typed evaluator reads it, and its geometry
    /// (`editor.geometry`). A document with features the engine cannot evaluate yet
    /// is not edited here.
    pub(crate) fn evaluate(&self) -> Result<(Arc<Prepared>, styk_eval::eval::Frame, styk_eval::geometry::Scene), EditError> {
        self.evaluate_only(None)
    }

    /// [`Editor::evaluate`] with the geometry of the figures in `slots` (and those they are
    /// joined to) only, when given.
    pub(crate) fn evaluate_only(&self, slots: Option<&[usize]>) -> Result<(Arc<Prepared>, styk_eval::eval::Frame, styk_eval::geometry::Scene), EditError> {
        let prepared = self.prepared()?;
        if let Some(feature) = prepared.doc.unsupported.first() {
            return Err(EditError::Unported(format!("geometry with {feature}")));
        }
        let record = self.shown_frame()?;
        let frame = self.eval.borrow_mut().loader.edit_frame(&record, None, &prepared.doc).map_err(|error| EditError::Invalid(error.to_string()))?;
        let scene = match slots {
            Some(slots) => styk_eval::geometry::build_only(&prepared.doc, &frame, slots),
            None => styk_eval::geometry::build(&prepared.doc, &frame),
        };
        Ok((prepared, frame, scene))
    }

    /// `layerIdForUid(report, uid)`.
    pub(crate) fn layer_of(&self, uid: u32) -> String {
        layer_of(self.report.path(&["_styk", "layers"]), uid)
    }
}

/// `layerIdForUid` on a layer state: the figure's assigned layer, else the default one.
pub(crate) fn layer_of(layers: Option<&Value>, uid: u32) -> String {
    let Some(state) = layers else { return "layer-1".into() };
    let assigned = state.get("assignments").and_then(|a| a.get(&uid.to_string()));
    assigned.or_else(|| state.get("defaultLayerId")).and_then(Value::as_str).unwrap_or("").to_string()
}

#[cfg(test)]
mod tests {
    #[test]
    fn camera_without_motion_does_not_prepare_typed_document() {
        let report = styk_doc::json::parse(br#"{"_styk":{"cameraMotion":{"clips":[]}}}"#).unwrap();
        let mut editor = crate::Editor::new(report, crate::Options::default());
        editor.worker_prepared_only = true;
        let frame = styk_doc::json::parse(br#"{"camera":{"is_default":false,"x":4}}"#).unwrap();
        assert_eq!(editor.camera_at_time(&frame, 90_000.0).unwrap(), frame.get("camera").cloned());
        assert!(editor.eval.borrow().prepared.is_none(), "empty camera clips prepared the document");
    }
}

/// A contiguous native row splice (`kind: 'ranges'`), shared by native frame commands.
/// Values remain immutable COW records; applying history never mutates its snapshots.
#[derive(Clone, Debug)]
pub struct FrameSlice {
    pub index: usize,
    pub before: Vec<Value>,
    pub after: Vec<Value>,
}

impl FrameSlice {
    pub(crate) fn history_value(&self) -> Value {
        let mut value = Obj::new();
        value.set("index", Value::Num(self.index as f64));
        value.set("before", Value::array(self.before.clone()));
        value.set("after", Value::array(self.after.clone()));
        Value::object(value)
    }
}

/// The existing native keyframe array owns row splices. The direction determines
/// ordering when more than one disjoint original range is replaced.
pub(crate) fn splice_ranges(report: &mut Value, changes: &[FrameSlice], after: bool) -> Result<(), EditError> {
    let frames = report.make_obj().and_then(|report| report.get_mut("keyframes")).and_then(Value::make_arr)
        .ok_or_else(|| EditError::Unported("native ranges without a keyframe array".into()))?;
    let mut ordered = changes.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|change| change.index);
    if after { ordered.reverse(); }
    for change in ordered {
        let (removed, inserted) = if after { (&change.before, &change.after) } else { (&change.after, &change.before) };
        if change.index > frames.len() || removed.len() > frames.len() - change.index {
            return Err(EditError::Unported("native ranges outside the keyframe array".into()));
        }
        frames.splice(change.index..change.index + removed.len(), inserted.iter().cloned());
    }
    Ok(())
}

impl Editor {
    /// `_reconcileFrameIds(previous)`: live unchanged nodes reserve their identity;
    /// retained sources and every earlier row reserve theirs in traversal order.
    fn reconcile_native_frame_ids(&self, report: &mut Value) -> Result<u64, EditError> {
        let previous = native_frames_of(&self.report);
        let mut frames = native_frames_of(report);
        let live = frames.iter().filter_map(|frame| match frame { Value::Obj(node) => Some(Arc::as_ptr(node) as usize), _ => None }).collect::<std::collections::HashSet<_>>();
        let mut reserved = std::collections::HashMap::new();
        for old in &previous {
            if matches!(old, Value::Obj(node) if live.contains(&(Arc::as_ptr(node) as usize))) {
                if let Some(id) = old.path(&["_styk", "frameId"]).and_then(Value::as_str) { reserved.insert(id.to_string(), old.clone()); }
            }
        }
        let mut retained = report.path(&["_styk", "frameSources"]).and_then(Value::as_obj)
            .map(|sources| sources.keys().map(|id| id.to_string()).collect::<std::collections::HashSet<_>>()).unwrap_or_default();
        let mut next = self.next_frame_identity;
        for frame in &mut frames {
            let id = frame.path(&["_styk", "frameId"]).and_then(Value::as_str).map(str::to_string);
            if id.as_ref().is_none_or(|id| retained.contains(id) || reserved.get(id).is_some_and(|old| !old.same_node(frame))) {
                if next > 9_007_199_254_740_991 { return Err(EditError::Unported("native row identity capacity before its recording".into())); }
                let mut fields = frame.as_obj().cloned().ok_or_else(|| EditError::Unported("a native row that is not an object".into()))?;
                let mut styk = frame.get("_styk").and_then(Value::as_obj).cloned().unwrap_or_default();
                styk.set("frameId", Value::str(&format!("frame-{next}"))); next += 1;
                fields.set("_styk", Value::object(styk)); *frame = Value::object(fields);
            }
            retained.insert(frame.path(&["_styk", "frameId"]).and_then(Value::as_str).unwrap().to_string());
        }
        report.make_obj().unwrap().set("keyframes", Value::array(frames));
        Ok(next)
    }

    /// A native row transaction shared by split, falloff and future recorded frame
    /// commands. Authored reference capture remains unavailable until recorded.
    pub(crate) fn commit_native_ranges(&mut self, mut changes: Vec<FrameSlice>, label: &str, before_selection: crate::select::SelectionState, structural: bool, timing: bool) -> Result<bool, EditError> {
        if changes.len() != 1 { return Err(EditError::Unported("multiple native row splices before their recordings".into())); }
        if self.report.path(&["_styk", "animation", "channels"]).and_then(Value::array_len).unwrap_or(0) != 0 || self.report.path(&["_styk", "simpleFrameDraft"]).is_some() {
            return Err(EditError::Unported("native ranges with authored frame references".into()));
        }
        let mut report = self.report.clone();
        splice_ranges(&mut report, &changes, true)?;
        let next = self.reconcile_native_frame_ids(&mut report)?;
        // Reconciliation may assign new identities; history captures exactly the
        // published rows, rather than the pre-reconciliation pieces.
        for change in &mut changes {
            change.after = native_frames_of(&report).into_iter().skip(change.index).take(change.after.len()).collect();
        }
        self.ensure_counts();
        self.report = report; self.next_frame_identity = next; self.retained = None;
        let previous = changes.iter().flat_map(|change| change.before.iter().cloned()).collect::<Vec<_>>();
        let current = changes.iter().flat_map(|change| change.after.iter().cloned()).collect::<Vec<_>>();
        let layers = self.track_frames(&previous, &current, true)?;
        self.frame_index = crate::timeline::rows(&self.report).iter().rposition(|row| row.start <= self.tick).unwrap_or(0);
        let indices = changes.iter().flat_map(|change| change.index..change.index + change.after.len()).map(|index| Value::Num(index as f64)).collect();
        self.record(crate::history::Command { kind: crate::history::CommandKind::Ranges { changes, structural, timing }, label: label.into(), before_selection, after_selection: self.selection_state(), before_revision: 0, after_revision: 0, layer_changes: Vec::new(), captured_source_bytes: 0, layers })?;
        self.emit("edit", vec![("label", Value::str(label)), ("structural", Value::Bool(structural)), ("timing", Value::Bool(timing)), ("frameIndices", Value::array(indices))]);
        Ok(true)
    }

    pub(crate) fn apply_native_ranges(&mut self, command: &crate::history::Command, direction: crate::history::Direction) -> Result<(), EditError> {
        let crate::history::CommandKind::Ranges { changes, structural, timing } = &command.kind else { return Ok(()) };
        let after = direction == crate::history::Direction::After;
        self.ensure_counts();
        splice_ranges(&mut self.report, changes, after)?;
        let (previous, current) = if after {
            (changes.iter().flat_map(|change| change.before.iter().cloned()).collect::<Vec<_>>(), changes.iter().flat_map(|change| change.after.iter().cloned()).collect::<Vec<_>>())
        } else {
            (changes.iter().flat_map(|change| change.after.iter().cloned()).collect::<Vec<_>>(), changes.iter().flat_map(|change| change.before.iter().cloned()).collect::<Vec<_>>())
        };
        self.track_frames(&previous, &current, false)?;
        self.apply_attached_layer_changes(&command.layer_changes, direction);
        self.retained = None;
        let (selection, revision) = if after { (&command.after_selection, command.after_revision) } else { (&command.before_selection, command.before_revision) };
        self.restore_selection(selection)?; self.revision = revision;
        self.emit("history", vec![("label", Value::str(&command.label)), ("structural", Value::Bool(*structural)), ("timing", Value::Bool(*timing)), ("definition", Value::Bool(false)), ("animation", Value::Bool(false))]);
        Ok(())
    }
}

fn native_frames_of(report: &Value) -> Vec<Value> {
    report.get("keyframes").and_then(Value::elements).map(|frames| frames.iter().collect()).unwrap_or_default()
}

#[cfg(test)]
#[path = "native_sampling_tests.rs"]
mod native_sampling_tests;
