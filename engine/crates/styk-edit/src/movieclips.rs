//! Movieclip definitions (src/editor-movieclips.mjs): `renameMovieclip`, published as one
//! `movieclip-definition` history entry (`publishDefinition`).

use styk_doc::Value;

use crate::history::{Command, CommandKind, Direction};
use crate::{EditError, Editor};

/// White space as `String.prototype.trim` removes it.
fn js_space(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
}

impl Editor {
    /// `publishDefinition(editor, figure, index, label)` replacing the definition at `index`.
    fn publish_definition(&mut self, index: usize, figure: Value, label: &str) -> Result<(), EditError> {
        let figures = self.report.make_obj().and_then(|r| r.get_mut("figure_types")).and_then(Value::make_arr).ok_or_else(|| EditError::Unported("figure definitions that are not a list".into()))?;
        let before = figures.get(index).cloned();
        match figures.get_mut(index) {
            Some(slot) => *slot = figure.clone(),
            None => figures.push(figure.clone()),
        }
        let selection = self.selection_state();
        self.record(Command { kind: CommandKind::MovieclipDefinition { index, before, after: Some(figure) }, label: label.into(), before_selection: selection.clone(), after_selection: selection, before_revision: 0, after_revision: 0 })?;
        self.emit("edit", vec![("structural", Value::Bool(true)), ("timing", Value::Bool(true)), ("definition", Value::Bool(true)), ("label", Value::str(label))]);
        Ok(())
    }

    /// `applyMovieclipCommand` for a `movieclip-definition` entry.
    pub(crate) fn apply_movieclip_definition(&mut self, command: &Command, direction: Direction) -> Result<(), EditError> {
        let CommandKind::MovieclipDefinition { index, before, after } = &command.kind else { return Ok(()) };
        let (value, other) = if direction == Direction::Before { (before, after) } else { (after, before) };
        if let Some(figures) = self.report.make_obj().and_then(|r| r.get_mut("figure_types")).and_then(Value::make_arr) {
            let at = (*index).min(figures.len());
            let removed = if other.is_some() { 1.min(figures.len() - at) } else { 0 };
            figures.splice(at..at + removed, value.iter().cloned());
        }
        let (selection, revision) = match direction {
            Direction::Before => (&command.before_selection, command.before_revision),
            Direction::After => (&command.after_selection, command.after_revision),
        };
        self.revision = revision;
        self.restore_selection(selection)?;
        self.emit("history", vec![("structural", Value::Bool(true)), ("timing", Value::Bool(true)), ("definition", Value::Bool(true)), ("label", Value::str(&command.label))]);
        Ok(())
    }

    /// `renameMovieclip(name)`: the selected movieclip's definition renamed.
    pub(crate) fn rename_movieclip(&mut self, name: &Value) -> Result<Value, EditError> {
        let definition = match self.object()? {
            Some((_, object)) => {
                let figures: Vec<Value> = self.report.get("figure_types").and_then(Value::elements).map(|e| e.iter().collect()).unwrap_or_default();
                figures.iter().position(|f| f.get("figure_type_id") == object.get("figure_type_id")).map(|i| (i, figures[i].clone()))
            }
            None => None,
        };
        let Some((index, figure)) = definition.filter(|(_, f)| f.path(&["_styk", "movieclip"]).is_some_and(Value::truthy)) else {
            return Err(EditError::Invalid("errors.editorMovieclips.selectAMovieclip".into()));
        };
        // `String(name).trim()`
        let text = match name {
            Value::Str(_) | Value::Blob(_) => name.text().unwrap_or_default().into_owned(),
            Value::Num(_) | Value::Bool(_) | Value::Null => styk_doc::json::to_string(name),
            _ => return Err(EditError::Unported("a movieclip name that is not text".into())),
        };
        let trimmed: &str = text.trim_matches(js_space);
        if trimmed.is_empty() || trimmed.encode_utf16().count() > 120 {
            return Err(EditError::Invalid("errors.editorMovieclips.useANameFrom1To120Characters".into()));
        }
        if figure.get("name").and_then(Value::as_str) == Some(trimmed) {
            return Ok(Value::Null);
        }
        let mut renamed = figure.as_obj().cloned().unwrap_or_default();
        renamed.set("name", Value::str(trimmed));
        let renamed = Value::object(renamed);
        self.publish_definition(index, renamed.clone(), "Rename movieclip")?;
        Ok(renamed)
    }
}
