//! History entries and undo/redo (`_record`, `_applyCommand`, `undo`, `redo`).
//!
//! Phase 1 keeps the app's history in JavaScript (docs/engine/editing.md); this one
//! exists so the command-replay harness can check undo and redo record for record.
//! It trims by count (`historyLimit`, 80 entries) but not by the editor's byte budget.

use styk_doc::Value;

use crate::record::{frame_mut, uid};
use crate::select::SelectionState;
use crate::{EditError, Editor};

const HISTORY_LIMIT: usize = 80;

/// What a history entry changed.
#[derive(Clone, Debug)]
pub enum CommandKind {
    /// Whole records of one native keyframe's objects, replaced by unique id.
    Objects { index: usize, before: Vec<Value>, after: Vec<Value> },
    /// The layer sheet and layer state before and after, and the pose sources the
    /// command captured (`kind: 'layer-sheet'`).
    LayerSheet { before: Value, after: Value, before_layers: Value, after_layers: Value, sources: Vec<(String, Value)> },
    /// Paths of the layer state before and after, and the sheet when the layers' tracks
    /// changed with them (`kind: 'layers'`).
    Layers { changes: Vec<crate::layers::LayerChange>, sheet: Option<(Value, Value)>, structural: bool, timing: bool },
    /// A figure definition before and after (`kind: 'figure'`).
    Figure { index: usize, before: Value, after: Value },
}

/// One history entry.
#[derive(Clone, Debug)]
pub struct Command {
    pub kind: CommandKind,
    pub label: String,
    pub(crate) before_selection: SelectionState,
    pub(crate) after_selection: SelectionState,
    pub before_revision: u64,
    pub after_revision: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction {
    Before,
    After,
}

impl Editor {
    /// `_record`: a new revision, pushed to undo; redo is cleared, and pose sources
    /// only a dropped redo branch used are collected.
    pub(crate) fn record(&mut self, mut command: Command) -> Result<(), EditError> {
        if let CommandKind::LayerSheet { sources, .. } = &command.kind {
            for (id, _) in sources {
                if !self.generated_sources.contains(id) {
                    self.generated_sources.push(id.clone());
                }
            }
            let frames: Vec<Value> = sources.iter().map(|(_, frame)| frame.clone()).collect();
            self.track_frames(&[], &frames, true)?;
        }
        let collect = !self.redo.is_empty();
        command.before_revision = self.revision;
        self.revision_serial += 1;
        command.after_revision = self.revision_serial;
        self.revision = command.after_revision;
        self.undo.push(command);
        self.redo.clear();
        let mut trimmed = false;
        while self.undo.len() > HISTORY_LIMIT {
            self.undo.remove(0);
            trimmed = true;
        }
        if collect || trimmed {
            self.prune_frame_sources();
        }
        Ok(())
    }

    pub(crate) fn undo(&mut self) -> Result<bool, EditError> {
        if self.drag.is_some() {
            self.cancel_drag()?;
        }
        let Some(command) = self.undo.pop() else { return Ok(false) };
        self.apply(&command, Direction::Before)?;
        self.redo.push(command);
        Ok(true)
    }

    pub(crate) fn redo(&mut self) -> Result<bool, EditError> {
        let Some(command) = self.redo.pop() else { return Ok(false) };
        self.apply(&command, Direction::After)?;
        self.undo.push(command);
        Ok(true)
    }

    fn apply(&mut self, command: &Command, direction: Direction) -> Result<(), EditError> {
        let (selection, revision) = match direction {
            Direction::Before => (&command.before_selection, command.before_revision),
            Direction::After => (&command.after_selection, command.after_revision),
        };
        match &command.kind {
            CommandKind::Objects { index, before, after } => {
                let value = if direction == Direction::Before { before } else { after };
                self.replace_objects(*index, value);
                self.restore_selection(selection)?;
                self.revision = revision;
                let uids = Value::array(value.iter().filter_map(uid).map(|u| Value::Num(u as f64)).collect());
                self.emit(
                    "history",
                    vec![
                        ("label", Value::str(&command.label)),
                        ("definition", Value::Bool(false)),
                        ("animation", Value::Bool(false)),
                        ("frameIndices", Value::array(vec![Value::Num(*index as f64)])),
                        ("changedUids", uids),
                    ],
                );
            }
            CommandKind::LayerSheet { .. } => self.apply_sheet_command(command, direction)?,
            CommandKind::Layers { .. } => self.apply_layers_command(command, direction)?,
            CommandKind::Figure { index, before, after } => {
                if let Some(figures) = self.report.make_obj().and_then(|r| r.get_mut("figure_types")).and_then(Value::make_arr)
                    && let Some(slot) = figures.get_mut(*index)
                {
                    *slot = if direction == Direction::Before { before.clone() } else { after.clone() };
                }
                self.restore_selection(selection)?;
                self.revision = revision;
                self.emit("history", vec![("label", Value::str(&command.label)), ("definition", Value::Bool(true)), ("animation", Value::Bool(false))]);
            }
        }
        Ok(())
    }

    /// `_replaceObjects`: puts `objects` in place of the records with the same unique ids.
    pub(crate) fn replace_objects(&mut self, index: usize, objects: &[Value]) {
        let Some(frame) = frame_mut(&mut self.report, index) else { return };
        let Some(list) = frame.get_mut("objects").and_then(Value::make_arr) else { return };
        for slot in list.iter_mut() {
            if let Some(replacement) = uid(slot).and_then(|id| objects.iter().find(|o| uid(o) == Some(id))) {
                *slot = replacement.clone();
            }
        }
    }
}
