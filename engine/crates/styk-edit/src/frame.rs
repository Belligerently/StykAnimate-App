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
#[derive(Default)]
pub(crate) struct Cache {
    pub loader: styk_eval::load::Loader,
    /// The document `prepared` and `frames` belong to (a shared node: any change to
    /// the editor's report makes it a different one).
    report: Option<Value>,
    prepared: Option<Arc<Prepared>>,
    /// Whether `prepared` has 3D poses, meshes or surfaces (`document_has_3d`), found once.
    has_3d: Option<bool>,
    /// Evaluated frames by tick and by whether movieclips were sampled; a few recent ones.
    frames: Vec<(u64, bool, Value)>,
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
        cache.has_3d = None;
        cache.report = Some(self.report.clone());
        cache.frames.clear();
    }

    /// The typed views of the current document.
    pub(crate) fn prepared(&self) -> Result<Arc<Prepared>, EditError> {
        let mut cache = self.eval.borrow_mut();
        if cache.prepared.is_none() || !cache.report.as_ref().is_some_and(|report| report.same_node(&self.report)) {
            let document = cache.loader.document(&self.report, "").map_err(|error| EditError::Invalid(error.to_string()))?;
            cache.prepared = Some(Arc::new(Prepared::new(Arc::new(document))));
            cache.has_3d = None;
            cache.report = Some(self.report.clone());
            cache.frames.clear();
        }
        Ok(cache.prepared.clone().unwrap())
    }

    /// `evaluatedFrame(report, …, tick)` as records; without `clips`, the frame a
    /// key's pose is captured from (`at(report, tick)`: movieclips not sampled).
    pub(crate) fn evaluated(&self, tick: f64, clips: bool) -> Result<Value, EditError> {
        let prepared = self.prepared()?;
        if let Some((_, _, frame)) = self.eval.borrow().frames.iter().find(|(at, with, _)| *at == tick.to_bits() && *with == clips) {
            return Ok(frame.clone());
        }
        let frame = Records::new(&self.report, &prepared)?.evaluated_with(tick, clips)?;
        let mut cache = self.eval.borrow_mut();
        if cache.frames.len() >= 4 {
            cache.frames.remove(0);
        }
        cache.frames.push((tick.to_bits(), clips, frame.clone()));
        Ok(frame)
    }

    /// `editor.evaluatedFrame`: the draft of the gesture in progress, else the
    /// document at the playhead.
    pub(crate) fn shown_frame(&self) -> Result<Value, EditError> {
        match (&self.authored, &self.retained) {
            (Some(edit), _) => Ok(edit.frame.clone()),
            (None, Some(frame)) => Ok(frame.clone()),
            (None, None) => self.evaluated(self.tick, true),
        }
    }

    /// `editor.selectionFrame.objects`: what selection reads. On a layer sheet the
    /// frame on show; on a native timeline the keyframe at the playhead.
    pub(crate) fn selection_frame(&self) -> Result<Vec<Value>, EditError> {
        if self.authored.is_some() || self.sheet() {
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
        let prepared = self.prepared()?;
        if let Some(feature) = prepared.doc.unsupported.first() {
            return Err(EditError::Unported(format!("geometry with {feature}")));
        }
        // The engine evaluates 3D, but no command replay covers editing it yet.
        let has_3d = *self.eval.borrow_mut().has_3d.get_or_insert_with(|| styk_eval::spatial::document_has_3d(&prepared.doc));
        if has_3d {
            return Err(EditError::Unported("geometry with 3D".into()));
        }
        let record = self.shown_frame()?;
        let frame = self.eval.borrow_mut().loader.edit_frame(&record, None, &prepared.doc).map_err(|error| EditError::Invalid(error.to_string()))?;
        let scene = styk_eval::geometry::build(&prepared.doc, &frame);
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
