//! Scene backgrounds (src/editor-scene.mjs): a background brought into the document and one given
//! a name. Choosing which one a frame shows (`setSceneBackground`, `setSceneColor`) is a property
//! edit (`property_edit.rs`).

use styk_doc::{Obj, Value};

use crate::{EditError, Editor};

fn invalid(key: &str) -> EditError {
    EditError::Invalid(key.into())
}

/// `Number.isSafeInteger`.
fn safe_integer(value: f64) -> bool {
    value.fract() == 0.0 && value.abs() <= 9_007_199_254_740_991.0
}

impl Editor {
    /// `editable(editor)`: the Scene track must be shown and unlocked, and an open edit ends first.
    fn scene_editable(&mut self) -> Result<(), EditError> {
        let scene = self.report.path(&["_styk", "layers", "fixed", "scene"]);
        if scene.and_then(|s| s.get("locked")).is_some_and(Value::truthy) {
            return Err(invalid("errors.editorScene.unlockTheSceneTrackBeforeEditingItsBackground"));
        }
        if matches!(scene.and_then(|s| s.get("visible")), Some(Value::Bool(false))) {
            return Err(invalid("errors.editorScene.showTheSceneTrackBeforeEditingItsBackground"));
        }
        if self.property_edit.is_some() {
            self.commit_animation_edit()?;
        }
        if self.drag.is_some() {
            self.commit_drag()?;
        }
        Ok(())
    }

    /// `nextBackgroundId(report)`: one past the largest identity in the table.
    fn next_background_id(&self) -> Result<f64, EditError> {
        let backgrounds: Vec<Value> = self.report.get("backgrounds").and_then(Value::elements).map(|e| e.iter().collect()).unwrap_or_default();
        if backgrounds.len() >= 65534 {
            return Err(invalid("errors.editorScene.theNativePivotBackgroundTableIsFull"));
        }
        let id = backgrounds.iter().fold(0.0f64, |largest, asset| {
            let id = asset.get("background_id").and_then(Value::as_f64).unwrap_or(f64::NAN);
            if largest.is_nan() || id.is_nan() { f64::NAN } else { largest.max(id) }
        });
        if !safe_integer(id + 1.0) {
            return Err(invalid("errors.editorScene.backgroundIdentityCapacityExceeded"));
        }
        Ok(id + 1.0)
    }

    /// `importSceneBackground(background, {indices})`: the background joins the table and the chosen frames show it,
    /// as one history entry. Answers the asset it stored, or nothing when no frame is chosen.
    pub(crate) fn import_scene_background(&mut self, background: &Value, options: &Value) -> Result<Value, EditError> {
        if !self.sheet() {
            return Err(EditError::Unported("importSceneBackground on a native timeline".into()));
        }
        self.scene_editable()?;
        let indices = crate::range::Indices::parse(options.get("indices"));
        if self.frame_indices(indices.as_ref())?.is_empty() {
            return Ok(Value::Null);
        }
        let source = background.get("background").filter(|inner| !inner.is_null()).unwrap_or(background);
        let id = self.next_background_id()?;
        let mut imported = source.as_obj().cloned().unwrap_or_default();
        imported.set("background_id", Value::Num(id));
        if !imported.get("name").is_some_and(Value::truthy) {
            imported.set("name", Value::str("Background"));
        }
        let imported = Value::object(imported);
        crate::load::validate_background(&self.report, &imported)?;
        let mut chosen = Obj::new();
        if let Some(indices) = options.get("indices") {
            chosen.set("indices", indices.clone());
        }
        let chosen = Value::object(chosen);
        self.movieclip_batch("Import scene background", move |editor| {
            let mut table: Vec<Value> = editor.report.get("backgrounds").and_then(Value::elements).map(|e| e.iter().collect()).unwrap_or_default();
            table.push(imported.clone());
            editor.set_background_table(table, "Import background asset")?;
            editor.set_scene_background(&Value::Num(id), Some(&chosen))?;
            Ok(editor.report.get("backgrounds").and_then(Value::elements).and_then(|e| e.iter().find(|asset| asset.get("background_id").and_then(Value::as_f64) == Some(id))).unwrap_or(Value::Null))
        })
    }

    /// `renameSceneBackground(id, name)`: whether the name changed.
    pub(crate) fn rename_scene_background(&mut self, id: &Value, name: &Value) -> Result<bool, EditError> {
        let backgrounds: Vec<Value> = self.report.get("backgrounds").and_then(Value::elements).map(|e| e.iter().collect()).unwrap_or_default();
        let Some(index) = backgrounds.iter().position(|asset| asset.get("background_id") == Some(id) && id.as_f64().is_some()) else {
            return Err(invalid("errors.editorScene.chooseAnExistingBackground"));
        };
        let name = name.text().map(|text| js_trim(&text).to_string()).filter(|text| !text.is_empty() && text.encode_utf16().count() <= 160);
        let Some(name) = name else { return Err(invalid("errors.editorScene.backgroundNamesMustContain1160Characters")) };
        if backgrounds[index].get("name").and_then(Value::as_str) == Some(name.as_str()) {
            return Ok(false);
        }
        if !self.sheet() {
            return Err(EditError::Unported("renameSceneBackground on a native timeline".into()));
        }
        if self.property_edit.is_some() {
            self.commit_animation_edit()?;
        }
        if self.drag.is_some() {
            self.commit_drag()?;
        }
        let mut renamed = backgrounds;
        let mut asset = renamed[index].as_obj().cloned().unwrap_or_default();
        asset.set("name", Value::str(&name));
        renamed[index] = Value::object(asset);
        self.set_background_table(renamed, "Rename background")?;
        Ok(true)
    }

    /// The background table replaced, as one history entry that touches no frame.
    fn set_background_table(&mut self, table: Vec<Value>, label: &str) -> Result<bool, EditError> {
        self.set_fields(label, vec![("backgrounds", Value::array(table))], false)
    }
}

/// `String.prototype.trim`: the white space and line terminators of ECMAScript.
fn js_trim(text: &str) -> &str {
    text.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}')
}
