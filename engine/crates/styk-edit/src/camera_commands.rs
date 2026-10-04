//! Camera commands on a layer sheet (src/editor-camera.mjs): the camera's values copied,
//! pasted, typed, reset, transformed, aligned with the canvas view or taken from the
//! neighbouring key, each a key of the camera track (`addAnimationKeys`); a camera drag that
//! previews its keys and commits them once; a handle moved to where it is on the neighbouring
//! key (`locateHandle`). On a native timeline the editor stores cameras in keyframes: not
//! ported.

use styk_doc::{Obj, Value};

use crate::record::{num, uid};
use crate::{EditError, Editor};

const FIELDS: [&str; 4] = ["x", "y", "scale", "angle_rad"];
const FINITE: &str = "errors.editorCamera.mustBeFinite";

/// A frame's camera as edited (`{index, tick, camera}`).
#[derive(Clone, Debug)]
pub(crate) struct Target {
    index: usize,
    tick: f64,
    camera: Value,
}

/// `_drag` of a camera drag on a layer sheet.
#[derive(Clone, Debug)]
pub(crate) struct CameraDrag {
    /// The keyframes the drag edits (`cameraBefore`'s indices).
    indices: Vec<usize>,
    baseline: Vec<Target>,
    current: Vec<Target>,
    entries: Vec<Value>,
    /// What the frame on show is while the drag previews its keys (`_sheetPropertyPreview`).
    pub preview: Option<Value>,
}

/// `cameraValues(camera)`: every value present, a default camera when none.
fn camera_values(camera: Option<&Value>) -> Value {
    let camera = camera.filter(|c| !c.is_null() && !c.get("is_default").is_some_and(Value::truthy));
    let mut out = match camera.and_then(Value::as_obj) {
        Some(object) => object.clone(),
        None => {
            let mut defaults = Obj::new();
            defaults.set("is_default", Value::Bool(true));
            defaults
        }
    };
    for (key, fallback) in [("x", 0.0), ("y", 0.0), ("scale", 1.0), ("angle_rad", 0.0)] {
        let value = camera.and_then(|c| c.get(key)).filter(|v| !v.is_null()).cloned().unwrap_or(Value::Num(fallback));
        out.set(key, value);
    }
    Value::object(out)
}

fn field(camera: &Value, key: &str) -> f64 {
    camera.get(key).and_then(Value::as_f64).unwrap_or(f64::NAN)
}

/// `checked(camera)`: finite values within the native range and a positive scale.
fn checked(camera: &Value) -> Result<Value, EditError> {
    for key in FIELDS {
        if !camera.get(key).and_then(Value::as_f64).is_some_and(f64::is_finite) {
            return Err(EditError::Invalid(FINITE.into()));
        }
    }
    for key in ["x", "y", "scale"] {
        if !(field(camera, key) as f32).is_finite() {
            return Err(EditError::Invalid("errors.editorCamera.cameraExceedsTheNativeRange".into()));
        }
    }
    let scale = field(camera, "scale");
    if scale <= 0.0 {
        return Err(EditError::Invalid("errors.editorCamera.cameraScaleMustBePositive".into()));
    }
    let inverse = (1.0f32 / scale as f32) as f64;
    if !inverse.is_finite() || inverse <= 0.0 {
        return Err(EditError::Invalid("errors.background.cameraScaleExceedsTheNativeRenderingRange".into()));
    }
    let mut out = camera.as_obj().cloned().unwrap_or_default();
    let default = field(camera, "x") == 0.0 && field(camera, "y") == 0.0 && scale == 1.0 && field(camera, "angle_rad") == 0.0;
    out.set("is_default", Value::Bool(default));
    Ok(Value::object(out))
}

/// `checkedPatch(patch)`: camera values only, each finite.
fn checked_patch(patch: &Value) -> Result<Vec<(String, f64)>, EditError> {
    let Some(object) = patch.as_obj() else { return Err(EditError::Invalid("errors.editorCamera.provideCameraPositionValues".into())) };
    let mut out = Vec::new();
    for (key, value) in object.iter() {
        if !FIELDS.contains(&key.as_ref()) {
            return Err(EditError::Invalid("errors.editorCamera.unknownCameraValue".into()));
        }
        let Some(value) = value.as_f64().filter(|v| v.is_finite()) else { return Err(EditError::Invalid(FINITE.into())) };
        out.push((key.to_string(), value));
    }
    Ok(out)
}

/// `{...camera, ...patch}`.
fn with(camera: &Value, patch: &[(&str, f64)]) -> Value {
    let mut out = camera.as_obj().cloned().unwrap_or_default();
    for (key, value) in patch {
        out.set(key, Value::Num(*value));
    }
    Value::object(out)
}

/// `cameraKeyEntries(editor, before, after)`: a camera key for every value that changed.
fn key_entries(before: &[Target], after: &[Target]) -> Vec<Value> {
    let mut entries = Vec::new();
    for (b, a) in before.iter().zip(after) {
        for key in FIELDS {
            let value = field(&a.camera, key);
            if field(&b.camera, key) == value {
                continue;
            }
            let mut target = Obj::new();
            target.set("kind", Value::str("camera"));
            let mut entry = Obj::new();
            entry.set("target", Value::object(target));
            entry.set("property", Value::str(if key == "angle_rad" { "rotation" } else { key }));
            entry.set("tick", Value::Num(b.tick));
            entry.set("value", Value::Num(value));
            entries.push(Value::object(entry));
        }
    }
    entries
}

/// A JSON list of keyframe indices (`indices = [this.frameIndex]`).
fn indices_of(value: &Value) -> Option<Value> {
    (!value.is_null()).then(|| value.clone())
}

impl Editor {
    fn camera_sheet(&self, method: &str) -> Result<(), EditError> {
        if self.sheet() { Ok(()) } else { Err(EditError::Unported(format!("{method} on a native timeline"))) }
    }

    /// `finishGesture(editor)`: an open property edit or gesture is committed first.
    fn finish_gesture(&mut self) -> Result<(), EditError> {
        if self.property_edit.is_some() {
            self.commit_animation_edit()?;
        }
        if self.drag.is_some() {
            self.commit_drag()?;
        }
        Ok(())
    }

    /// `_frameIndices(indices)`.
    fn frame_indices(&self, indices: Option<&Value>) -> Result<Vec<usize>, EditError> {
        let count = self.report.get("keyframes").and_then(Value::array_len).unwrap_or(0) as f64;
        let list: Vec<Value> = match indices {
            None => vec![Value::Num(self.frame_index as f64)],
            Some(list) => list.elements().map(|e| e.iter().collect()).unwrap_or_default(),
        };
        let mut out: Vec<usize> = Vec::new();
        for value in list {
            let Some(index) = value.as_f64().filter(|i| i.fract() == 0.0 && *i >= 0.0 && *i < count) else {
                return Err(EditError::Invalid("errors.editor.selectExistingKeyframes".into()));
            };
            if !out.contains(&(index as usize)) {
                out.push(index as usize);
            }
        }
        out.sort_unstable();
        Ok(out)
    }

    /// `cameraTargets(editor, indices)` with each frame's camera (`composedFrame`).
    fn camera_targets(&mut self, indices: Option<&Value>) -> Result<Vec<Target>, EditError> {
        let mut options = Obj::new();
        if let Some(indices) = indices {
            options.set("indices", indices.clone());
        }
        let ticks = self.sheet_edit_times(Some(&Value::object(options)))?;
        let rows = crate::timeline::rows(&self.report);
        let mut targets = Vec::with_capacity(ticks.len());
        for tick in ticks {
            let index = rows.iter().rposition(|row| row.start <= tick).unwrap_or(0);
            let frame = self.evaluated(tick, true)?;
            targets.push(Target { index, tick, camera: camera_values(frame.get("camera")) });
        }
        Ok(targets)
    }

    /// `cameraEdit(editor, label, indices, update)`: the changed cameras keyed; returns how many.
    fn camera_edit(&mut self, indices: Option<&Value>, update: impl Fn(&Value) -> Option<Value>) -> Result<Value, EditError> {
        self.finish_gesture()?;
        let (mut before, mut after) = (Vec::new(), Vec::new());
        for entry in self.camera_targets(indices)? {
            let Some(value) = update(&entry.camera) else { continue };
            let next = checked(&value)?;
            if styk_doc::json::to_string(&entry.camera) == styk_doc::json::to_string(&next) {
                continue;
            }
            after.push(Target { index: entry.index, tick: entry.tick, camera: next });
            before.push(entry);
        }
        if after.is_empty() {
            return Ok(Value::Num(0.0));
        }
        let entries = key_entries(&before, &after);
        if entries.is_empty() {
            return Ok(Value::Num(0.0));
        }
        self.add_animation_keys(Some(&Value::array(entries)))?;
        Ok(Value::Num(after.len() as f64))
    }

    /// `copyCamera()`.
    pub(crate) fn copy_camera(&mut self) -> Result<Value, EditError> {
        self.camera_sheet("copyCamera")?;
        let camera = checked(&camera_values(self.shown_frame()?.get("camera")))?;
        self.camera_clipboard = Some(camera.clone());
        Ok(camera)
    }

    /// `pasteCamera(indices)`.
    pub(crate) fn paste_camera(&mut self, indices: &Value) -> Result<Value, EditError> {
        self.camera_sheet("pasteCamera")?;
        let Some(values) = self.camera_clipboard.clone() else { return Ok(Value::Num(0.0)) };
        let patch: Vec<(&str, Value)> = FIELDS.iter().map(|key| (*key, values.get(key).cloned().unwrap_or(Value::Null))).collect();
        self.camera_edit(indices_of(indices).as_ref(), |camera| {
            let mut out = camera.as_obj().cloned().unwrap_or_default();
            for (key, value) in &patch {
                out.set(key, value.clone());
            }
            Some(Value::object(out))
        })
    }

    /// `setCameraValues(patch, indices)`.
    pub(crate) fn set_camera_values(&mut self, patch: &Value, indices: &Value) -> Result<Value, EditError> {
        self.camera_sheet("setCameraValues")?;
        let patch = checked_patch(patch)?;
        if patch.is_empty() {
            return Ok(Value::Num(0.0));
        }
        let patch: Vec<(&str, f64)> = patch.iter().map(|(k, v)| (k.as_str(), *v)).collect();
        self.camera_edit(indices_of(indices).as_ref(), |camera| Some(with(camera, &patch)))
    }

    /// `resetCamera(indices)`.
    pub(crate) fn reset_camera(&mut self, indices: &Value) -> Result<Value, EditError> {
        self.camera_sheet("resetCamera")?;
        self.camera_edit(indices_of(indices).as_ref(), |camera| {
            let mut out = camera.as_obj().cloned().unwrap_or_default();
            out.set("is_default", Value::Bool(true));
            for (key, value) in [("x", 0.0), ("y", 0.0), ("scale", 1.0), ("angle_rad", 0.0)] {
                out.set(key, Value::Num(value));
            }
            Some(Value::object(out))
        })
    }

    /// `transformCamera(delta, indices)`: offsets for position and angle, a factor for scale.
    pub(crate) fn transform_camera(&mut self, delta: &Value, indices: &Value) -> Result<Value, EditError> {
        self.camera_sheet("transformCamera")?;
        let delta = checked_patch(delta)?;
        let get = |key: &str, fallback: f64| delta.iter().find(|(k, _)| k == key).map_or(fallback, |(_, v)| *v);
        let (dx, dy, da, ds) = (get("x", 0.0), get("y", 0.0), get("angle_rad", 0.0), get("scale", 1.0));
        self.camera_edit(indices_of(indices).as_ref(), |camera| {
            Some(with(camera, &[("x", field(camera, "x") + dx), ("y", field(camera, "y") + dy), ("angle_rad", field(camera, "angle_rad") + da), ("scale", field(camera, "scale") * ds)]))
        })
    }

    /// `alignCameraToView({x, y, scale, width, height}, indices)`: the camera framing what the
    /// canvas view shows.
    pub(crate) fn align_camera_to_view(&mut self, view: &Value, indices: &Value) -> Result<Value, EditError> {
        self.camera_sheet("alignCameraToView")?;
        let mut values = [0.0; 5];
        for (slot, key) in values.iter_mut().zip(["x", "y", "scale", "width", "height"]) {
            *slot = view.get(key).and_then(Value::as_f64).filter(|v| v.is_finite()).ok_or_else(|| EditError::Invalid(FINITE.into()))?;
        }
        let [x, y, scale, width, height] = values;
        if scale <= 0.0 || width <= 0.0 || height <= 0.0 {
            return Err(EditError::Invalid("errors.editorCamera.canvasDimensionsAndZoomMustBePositive".into()));
        }
        let (w, h) = (num(&self.report, "canvas_width").unwrap_or(f64::NAN), num(&self.report, "canvas_height").unwrap_or(f64::NAN));
        let patch = [("x", (width / 2.0 - x) / scale - w / 2.0), ("y", (height / 2.0 - y) / scale - h / 2.0), ("scale", (width / w).min(height / h) / scale)];
        self.camera_edit(indices_of(indices).as_ref(), |camera| Some(with(camera, &patch)))
    }

    /// `neighborIndex(editor, direction)`.
    fn neighbour_index(&self, direction: f64) -> Result<Option<usize>, EditError> {
        if direction != -1.0 && direction != 1.0 {
            return Err(EditError::Invalid("errors.editorCamera.chooseThePrevious1OrNext1Frame".into()));
        }
        let index = self.frame_index as i64 + direction as i64;
        let count = self.report.get("keyframes").and_then(Value::array_len).unwrap_or(0) as i64;
        Ok((index >= 0 && index < count).then_some(index as usize))
    }

    /// `locateCamera(direction, channel, indices)`: the neighbouring key's camera values.
    pub(crate) fn locate_camera(&mut self, direction: f64, channel: &str, indices: &Value) -> Result<Value, EditError> {
        self.camera_sheet("locateCamera")?;
        let keys: &[&str] = match channel {
            "position" => &["x", "y"],
            "rotation" => &["angle_rad"],
            "scale" => &["scale"],
            "pose" => &FIELDS,
            _ => return Err(EditError::Invalid("errors.editorCamera.chooseCameraPositionRotationScaleOrPose".into())),
        };
        let Some(index) = self.neighbour_index(direction)? else { return Ok(Value::Num(0.0)) };
        let Some(row) = crate::timeline::rows(&self.report).get(index).copied() else { return Ok(Value::Num(0.0)) };
        let camera = camera_values(self.evaluated(row.start, true)?.get("camera"));
        let mut patch = Obj::new();
        for key in keys {
            patch.set(key, camera.get(key).cloned().unwrap_or(Value::Null));
        }
        self.set_camera_values(&Value::object(patch), indices)
    }

    /// `locateHandle(direction, {uid, jointIndex, handleId, editMode})`: the handle's figure
    /// moved by where the handle is on the neighbouring key.
    pub(crate) fn locate_handle(&mut self, direction: f64, options: &Value) -> Result<bool, EditError> {
        let id = match options.get("uid") {
            Some(value) => value.as_f64().filter(|v| v.fract() == 0.0 && *v >= 0.0).map(|v| v as u32),
            None => self.selection.uid,
        };
        let joint = options.get("jointIndex").and_then(Value::as_f64).map_or(self.selection.joint, |j| j as i64);
        if options.get("editMode").is_some_and(Value::truthy) {
            let mut edit = Obj::new();
            edit.set("uid", id.map_or(Value::Null, |u| Value::Num(u as f64)));
            edit.set("jointIndex", Value::Num(joint as f64));
            if let Some(handle) = options.get("handleId").filter(|h| !h.is_null()) {
                edit.set("handleId", handle.clone());
            }
            return self.locate_edited_handle(direction, &Value::object(edit));
        }
        self.finish_gesture()?;
        let Some(index) = self.neighbour_index(direction)? else { return Ok(false) };
        let current = self.selection_frame()?;
        let Some(object) = id.and_then(|id| current.iter().find(|o| uid(o) == Some(id))).cloned() else { return Ok(false) };
        let Some(row) = crate::timeline::rows(&self.report).get(index).copied() else { return Ok(false) };
        let neighbour = self.evaluated(row.start, true)?;
        let id = id.unwrap();
        if !crate::frame::objects_of(&neighbour).iter().any(|o| uid(o) == Some(id)) {
            return Ok(false);
        }
        let segments: Vec<Value> = self.figure_record(&object).and_then(|f| f.get("segments").and_then(Value::elements).map(|e| e.iter().collect())).unwrap_or_default();
        let handle = match options.get("handleId").filter(|h| !h.is_null()) {
            Some(value) => value.as_f64().unwrap_or(f64::NAN),
            None if joint < 0 => 0.0,
            None => segments.get(joint as usize).and_then(|s| num(s, "segment_id")).unwrap_or(f64::NAN),
        };
        if !(handle.fract() == 0.0 && handle >= 0.0) {
            return Ok(false);
        }
        // `buildGeometry(report, selectionFrame)`: the frame on show.
        let source = self.joint_in_frame(&self.shown_frame()?, id, handle)?;
        let target = self.joint_in_frame(&neighbour, id, handle)?;
        let (Some(source), Some(target)) = (source, target) else { return Ok(false) };
        let (dx, dy) = (target[0] - source[0], target[1] - source[1]);
        if dx == 0.0 && dy == 0.0 {
            return Ok(false);
        }
        if !self.selection.uids.contains(&id) {
            self.select(Some(id), joint, false, false)?;
        }
        self.begin_drag(if direction < 0.0 { "Locate handle in previous frame" } else { "Locate handle in next frame" }, true)?;
        let result = self.move_selection(dx, dy, &Value::Null).and_then(|_| self.commit_drag());
        if result.is_err() {
            self.cancel_drag()?;
        }
        result
    }

    /// `beginCameraDrag({indices, label})`.
    pub(crate) fn begin_camera_drag(&mut self, options: &Value) -> Result<bool, EditError> {
        self.camera_sheet("beginCameraDrag")?;
        self.finish_gesture()?;
        let indices = options.get("indices").filter(|i| !i.is_null()).cloned();
        let label = options.get("label").and_then(Value::as_str).unwrap_or("Move camera").to_string();
        let selected = self.frame_indices(indices.as_ref())?;
        if selected.is_empty() {
            return Ok(false);
        }
        let baseline = self.camera_targets(indices.as_ref())?;
        self.drag = Some(crate::Drag {
            label,
            translation: false,
            index: self.frame_index,
            before: Vec::new(),
            before_selection: self.selection_state(),
            changed: false,
            detached: Default::default(),
            authored: false,
            rotation_targets: Default::default(),
            spatial_gestures: Default::default(),
            spatial_kinds: Default::default(),
            ik_captures: Default::default(),
            spatial_ik_captures: Default::default(),
            ik_resume: Default::default(),
            handle_edit: None,
            figure_handle: None,
            segment_value_options: None,
            camera: Some(CameraDrag { indices: selected, current: baseline.clone(), baseline, entries: Vec::new(), preview: None }),
        });
        self.retained = None;
        Ok(true)
    }

    /// `updateCameraDrag(delta, {fromStart})`: the keys previewed on the frame on show.
    pub(crate) fn update_camera_drag(&mut self, delta: &Value, options: &Value) -> Result<bool, EditError> {
        self.camera_sheet("updateCameraDrag")?;
        let delta = match checked_patch(delta) {
            Ok(delta) => delta,
            Err(error) => {
                if self.camera_drag().is_some() {
                    self.cancel_camera_drag()?;
                }
                return Err(error);
            }
        };
        if self.camera_drag().is_none() && !self.begin_camera_drag(&Value::Null)? {
            return Ok(false);
        }
        if self.drag.as_ref().is_some_and(|d| d.index != self.frame_index) {
            self.cancel_camera_drag()?;
            return Err(EditError::Invalid("errors.editorCamera.theDocumentOrFrameChangedDuringTheCamera".into()));
        }
        let from_start = options.get("fromStart").is_none_or(Value::truthy);
        let get = |key: &str, fallback: f64| delta.iter().find(|(k, _)| k == key).map_or(fallback, |(_, v)| *v);
        let (dx, dy, da, ds) = (get("x", 0.0), get("y", 0.0), get("angle_rad", 0.0), get("scale", 1.0));
        let result = (|| {
            let drag = self.camera_drag().unwrap();
            let mut values = Vec::with_capacity(drag.baseline.len());
            for (i, entry) in drag.baseline.iter().enumerate() {
                let camera = camera_values(Some(if from_start { &entry.camera } else { &drag.current[i].camera }));
                let next = checked(&with(&camera, &[("x", field(&camera, "x") + dx), ("y", field(&camera, "y") + dy), ("angle_rad", field(&camera, "angle_rad") + da), ("scale", field(&camera, "scale") * ds)]))?;
                values.push(Target { index: entry.index, tick: entry.tick, camera: next });
            }
            let entries = key_entries(&drag.baseline, &values);
            let preview = self.camera_preview(&drag.baseline, &entries)?;
            Ok::<_, EditError>((values, entries, preview))
        })();
        let (values, entries, preview) = match result {
            Ok(done) => done,
            Err(error) => {
                self.cancel_camera_drag()?;
                return Err(error);
            }
        };
        let frames: Vec<Value> = values.iter().map(|v| Value::Num(v.index as f64)).collect();
        let drag = self.drag.as_mut().unwrap();
        drag.changed = !entries.is_empty();
        let camera = drag.camera.as_mut().unwrap();
        camera.current = values;
        camera.entries = entries;
        camera.preview = Some(preview);
        self.retained = None;
        self.emit("edit", vec![("live", Value::Bool(true)), ("camera", Value::Bool(true)), ("animation", Value::Bool(true)), ("changedFrames", Value::array(frames))]);
        Ok(true)
    }

    /// `previewCameraKeys` on a layer sheet: the frame on show with the drag's camera at the playhead.
    fn camera_preview(&self, baseline: &[Target], entries: &[Value]) -> Result<Value, EditError> {
        let shown = self.shown_frame()?;
        let mut camera = camera_values(shown.get("camera")).as_obj().cloned().unwrap_or_default();
        if let Some(entry) = baseline.iter().find(|e| e.tick == self.tick)
            && let Some(object) = entry.camera.as_obj()
        {
            for (key, value) in object.iter() {
                camera.set(key, value.clone());
            }
        }
        for entry in entries {
            if entry.get("tick").and_then(Value::as_f64) == Some(self.tick) {
                let property = entry.get("property").and_then(Value::as_str).unwrap_or("");
                camera.set(if property == "rotation" { "angle_rad" } else { property }, entry.get("value").cloned().unwrap_or(Value::Null));
            }
        }
        let mut frame = shown.as_obj().cloned().unwrap_or_default();
        frame.set("camera", checked(&Value::object(camera))?);
        Ok(Value::object(frame))
    }

    pub(crate) fn camera_drag(&self) -> Option<&CameraDrag> {
        self.drag.as_ref().and_then(|d| d.camera.as_ref())
    }

    /// `commitCameraDrag()`: the previewed keys added as one edit.
    pub(crate) fn commit_camera_drag(&mut self) -> Result<bool, EditError> {
        let Some(camera) = self.drag.take().and_then(|d| d.camera) else { return Ok(false) };
        self.retained = None;
        if camera.entries.is_empty() {
            return Ok(false);
        }
        self.add_animation_keys(Some(&Value::array(camera.entries)))?;
        Ok(true)
    }

    /// `cancelCameraDrag()`.
    pub(crate) fn cancel_camera_drag(&mut self) -> Result<bool, EditError> {
        let Some(camera) = self.drag.take().and_then(|d| d.camera) else { return Ok(false) };
        self.retained = None;
        let frames: Vec<Value> = camera.indices.iter().map(|i| Value::Num(*i as f64)).collect();
        self.emit("edit", vec![("label", Value::str("Cancel camera move")), ("camera", Value::Bool(true)), ("changedFrames", Value::array(frames))]);
        Ok(true)
    }

    /// While a camera drag is open only its own calls and camera commands run here.
    pub(crate) fn camera_gate(&self, method: &str) -> Result<(), EditError> {
        if self.camera_drag().is_none() {
            return Ok(());
        }
        const ALLOWED: [&str; 14] = ["updateCameraDrag", "commitCameraDrag", "cancelCameraDrag", "beginCameraDrag", "commitDrag", "cancelDrag", "copyCamera", "pasteCamera", "setCameraValues", "resetCamera", "transformCamera", "alignCameraToView", "locateCamera", "locateHandle"];
        if ALLOWED.contains(&method) { Ok(()) } else { Err(EditError::Unported(format!("{method} during a camera drag"))) }
    }
}
