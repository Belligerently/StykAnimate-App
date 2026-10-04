//! Effects in a figure's, a segment's or the scene's stack on a layer sheet
//! (src/editor-vfx.mjs): added, removed, reset, duplicated, moved, reordered and pasted, each
//! the stack keyed at the playhead through `addAnimationKeys`. New effects take the
//! identities the editor drew (D18).

use styk_doc::{Obj, Value};
use styk_eval::vfx::Kind;

use crate::property_edit::{normalize_target, validate_vfx_value};
use crate::sheet::{fixed, items};
use crate::{EditError, Editor};

fn invalid(key: &str) -> EditError {
    EditError::Invalid(key.into())
}

/// `effectAvailable(type, scene)`: scene stacks take the scene's effects; figures and segments
/// the others but the vignette (an unknown kind passes here and fails in `createVfx`).
fn available(kind: &str, scene: bool) -> bool {
    match Kind::from_name(kind) {
        Some(k) if scene => k.on_scene(),
        None if scene => false,
        Some(k) => !k.scene_only() && kind != "vignette",
        None => kind != "vignette",
    }
}

fn is_scene(target: &Value) -> bool {
    target.get("kind").and_then(Value::as_str) == Some("scene")
}

impl Editor {
    /// `targetFor(editor, target)`: a figure, segment or the scene, on an unlocked layer, present
    /// at the playhead.
    pub(crate) fn vfx_target(&self, target: &Value) -> Result<Value, EditError> {
        let target = normalize_target(target)?;
        let kind = target.get("kind").and_then(Value::as_str).unwrap_or("").to_string();
        if !["scene", "figure", "segment"].contains(&kind.as_str()) {
            return Err(invalid("errors.editorVfx.selectAFigureNodeMovieclipOrScene"));
        }
        let layer = if kind == "scene" { "scene".to_string() } else { self.layer_of(target.get("uid").and_then(Value::as_f64).unwrap_or(0.0) as u32) };
        let layers = self.report.path(&["_styk", "layers"]);
        let state = if fixed(&layer) { layers.and_then(|l| l.path(&["fixed", &layer])) } else { layers.and_then(|l| l.path(&["byId", &layer])) };
        if state.and_then(|s| s.get("locked")).is_some_and(Value::truthy) {
            return Err(invalid("errors.editorVfx.unlockThisLayerBeforeEditingEffects"));
        }
        if self.animation_value(&target, "vfxStack", &Value::Num(self.tick))?.is_none() {
            return Err(invalid("errors.editorVfx.thisTargetIsNotPresentAtThisFrame"));
        }
        Ok(target)
    }

    /// `vfxEffects(target)`: the target's stack on show.
    pub(crate) fn vfx_effects(&self, target: &Value) -> Result<Vec<Value>, EditError> {
        Ok(items(self.animation_value(target, "vfxStack", &Value::Num(self.tick))?.as_ref()))
    }

    /// `createVfx(type, id)`: an effect of `type` with its default parameters.
    fn create_vfx(&self, kind: &str, id: &str) -> Result<Value, EditError> {
        if Kind::from_name(kind).is_none() {
            return Err(invalid("errors.vfx.chooseAnAvailableEffect"));
        }
        let mut effect = Obj::new();
        effect.set("version", Value::Num(2.0));
        effect.set("type", Value::str(kind));
        effect.set("id", Value::str(id));
        effect.set("enabled", Value::Bool(true));
        effect.set("params", Value::object(Obj::new()));
        Ok(validate_vfx_value("vfxStack", Some(&Value::array(vec![Value::object(effect)])))?.item(0).unwrap_or(Value::Null))
    }

    /// `freshId()`: `fx-` and an identity the editor drew, without its dashes.
    fn fresh_effect_id(&mut self) -> Result<String, EditError> {
        Ok(format!("fx-{}", self.draw_uuid()?.replace('-', "")))
    }

    /// One `vfxStack` key entry at the playhead.
    fn stack_entry(&self, target: &Value, stack: Vec<Value>) -> Result<Value, EditError> {
        let mut entry = Obj::new();
        entry.set("target", target.clone());
        entry.set("property", Value::str("vfxStack"));
        entry.set("tick", Value::Num(self.tick));
        entry.set("value", validate_vfx_value("vfxStack", Some(&Value::array(stack)))?);
        entry.set("exactTarget", Value::Bool(true));
        Ok(Value::object(entry))
    }

    /// `addVfx(targets, type)`: a new effect at the end of each target's stack. Returns the new
    /// effects' identities.
    pub(crate) fn add_vfx(&mut self, targets: &Value, kind: &Value) -> Result<Value, EditError> {
        let list = if targets.is_array() { items(Some(targets)) } else { vec![targets.clone()] };
        let targets: Vec<Value> = list.iter().map(|t| self.vfx_target(t)).collect::<Result<_, _>>()?;
        let kind = kind.as_str().unwrap_or("").to_string();
        if targets.iter().any(|t| !available(&kind, is_scene(t))) {
            return Err(invalid("errors.editorVfx.thisEffectIsNotAvailableForTheSelected"));
        }
        let (mut entries, mut ids) = (Vec::new(), Vec::new());
        for target in &targets {
            let mut stack = self.vfx_effects(target)?;
            let id = self.fresh_effect_id()?;
            let effect = self.create_vfx(&kind, &id)?;
            ids.push(Value::str(&id));
            stack.push(effect);
            entries.push(self.stack_entry(target, stack)?);
        }
        self.add_animation_keys(Some(&Value::array(entries)))?;
        Ok(Value::array(ids))
    }

    /// `changeVfxStack(target, mutate)`: the target's stack changed and keyed at the playhead.
    fn change_vfx_stack(&mut self, target: &Value, mutate: impl FnOnce(&mut Editor, &Value, &mut Vec<Value>) -> Result<(), EditError>) -> Result<Value, EditError> {
        let target = self.vfx_target(target)?;
        let mut stack = self.vfx_effects(&target)?;
        mutate(self, &target, &mut stack)?;
        let entry = self.stack_entry(&target, stack)?;
        self.add_animation_keys(Some(&Value::array(vec![entry]))).map(|keys| keys.item(0).unwrap_or(Value::Null))
    }

    /// `removeVfx(target, id)`.
    pub(crate) fn remove_vfx(&mut self, target: &Value, id: &Value) -> Result<Value, EditError> {
        self.change_vfx_stack(target, |_, _, stack| {
            if let Some(at) = stack.iter().position(|e| e.get("id") == Some(id)) {
                stack.remove(at);
            }
            Ok(())
        })
    }

    /// `resetVfx(target, id)`: the effect's defaults back.
    pub(crate) fn reset_vfx(&mut self, target: &Value, id: &Value) -> Result<Value, EditError> {
        self.change_vfx_stack(target, |editor, _, stack| {
            let Some(at) = stack.iter().position(|e| e.get("id") == Some(id)) else { return Err(invalid("errors.editorVfx.theEffectNoLongerExists")) };
            let kind = stack[at].get("type").and_then(Value::as_str).unwrap_or("").to_string();
            stack[at] = editor.create_vfx(&kind, id.as_str().unwrap_or(""))?;
            Ok(())
        })
    }

    /// `duplicateVfx(target, id)`: a copy after the effect. Returns the copy's identity.
    pub(crate) fn duplicate_vfx(&mut self, target: &Value, id: &Value) -> Result<Value, EditError> {
        let mut new_id = Value::Null;
        self.change_vfx_stack(target, |editor, _, stack| {
            let Some(at) = stack.iter().position(|e| e.get("id") == Some(id)) else { return Err(invalid("errors.editorVfx.theEffectNoLongerExists")) };
            let fresh = editor.fresh_effect_id()?;
            let mut copy = stack[at].as_obj().cloned().unwrap_or_default();
            copy.set("id", Value::str(&fresh));
            stack.insert(at + 1, Value::object(copy));
            new_id = Value::str(&fresh);
            Ok(())
        })?;
        Ok(new_id)
    }

    /// `moveVfx(target, id, direction)`: one place up or down.
    pub(crate) fn move_vfx(&mut self, target: &Value, id: &Value, direction: &Value) -> Result<Value, EditError> {
        let step = direction.as_f64().filter(|d| *d == -1.0 || *d == 1.0).ok_or_else(|| invalid("errors.editorVfxClips.chooseAnEffectOrder"))?;
        self.change_vfx_stack(target, |_, _, stack| {
            let Some(at) = stack.iter().position(|e| e.get("id") == Some(id)) else { return Ok(()) };
            let to = at as i64 + step as i64;
            if to >= 0 && (to as usize) < stack.len() {
                stack.swap(at, to as usize);
            }
            Ok(())
        })
    }

    /// `reorderVfx(target, id, index)`.
    pub(crate) fn reorder_vfx(&mut self, target: &Value, id: &Value, index: &Value) -> Result<Value, EditError> {
        self.change_vfx_stack(target, |_, _, stack| {
            let from = stack.iter().position(|e| e.get("id") == Some(id));
            let to = index.as_f64().filter(|i| i.fract() == 0.0 && *i >= 0.0 && (*i as usize) < stack.len());
            let (Some(from), Some(to)) = (from, to) else { return Err(invalid("errors.editorVfx.invalidEffectPosition")) };
            let effect = stack.remove(from);
            stack.insert(to as usize, effect);
            Ok(())
        })
    }

    /// `pasteVfx(targets, stack, {includeAttached})` on a layer sheet: the copied effects merged
    /// into each target's stack (an effect of the same identity and kind replaced, one of
    /// another kind under a new identity), and the scope set when given.
    pub(crate) fn paste_vfx(&mut self, targets: &Value, stack: &Value, options: &Value) -> Result<Value, EditError> {
        let list = if targets.is_array() { items(Some(targets)) } else { vec![targets.clone()] };
        let effects = items(Some(&validate_vfx_value("vfxStack", Some(stack))?));
        let include = match options.get("includeAttached") {
            None => None,
            Some(Value::Bool(include)) => Some(*include),
            Some(_) => return Err(invalid("errors.editorVfx.chooseWhetherToIncludeAttachedParts")),
        };
        if include == Some(true) {
            return Err(EditError::Unported("pasting effects that include attached parts".into()));
        }
        let mut entries = Vec::new();
        for given in &list {
            let target = self.vfx_target(given)?;
            let mut next = self.vfx_effects(&target)?;
            for effect in &effects {
                let kind = effect.get("type").and_then(Value::as_str).unwrap_or("").to_string();
                if !available(&kind, is_scene(&target)) {
                    return Err(invalid("errors.editorVfx.thisEffectIsNotAvailableForTheSelected"));
                }
                let index = next.iter().position(|e| e.get("id") == effect.get("id"));
                let same = index.is_some_and(|i| next[i].get("type") == effect.get("type"));
                let mut value = effect.as_obj().cloned().unwrap_or_default();
                if index.is_some() && !same {
                    value.set("id", Value::str(&self.fresh_effect_id()?));
                }
                match index.filter(|_| same) {
                    Some(i) => next[i] = Value::object(value),
                    None => next.push(Value::object(value)),
                }
            }
            entries.push(self.stack_entry(&target, next)?);
            if let (Some(include), Some("figure")) = (include, target.get("kind").and_then(Value::as_str)) {
                let mut scope = Obj::new();
                scope.set("target", target.clone());
                scope.set("property", Value::str("vfxScope"));
                scope.set("tick", Value::Num(self.tick));
                scope.set("exactTarget", Value::Bool(true));
                scope.set("value", Value::Num(if include { 1.0 } else { 0.0 }));
                entries.push(Value::object(scope));
            }
        }
        if !self.sheet() {
            return Err(EditError::Unported("pasteVfx on a native timeline".into()));
        }
        self.add_animation_keys(Some(&Value::array(entries)))
    }
}
