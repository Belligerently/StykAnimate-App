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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandKind {
    /// Whole records of one keyframe's objects, replaced by unique id.
    Objects,
}

/// One history entry.
#[derive(Clone, Debug)]
pub struct Command {
    pub kind: CommandKind,
    pub label: String,
    pub index: usize,
    pub before: Vec<Value>,
    pub after: Vec<Value>,
    pub(crate) before_selection: SelectionState,
    pub(crate) after_selection: SelectionState,
    pub before_revision: u64,
    pub after_revision: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Direction {
    Before,
    After,
}

impl Editor {
    /// `_record`: a new revision, pushed to undo; redo is cleared.
    pub(crate) fn record(&mut self, mut command: Command) {
        command.before_revision = self.revision;
        self.revision_serial += 1;
        command.after_revision = self.revision_serial;
        self.revision = command.after_revision;
        self.undo.push(command);
        self.redo.clear();
        while self.undo.len() > HISTORY_LIMIT {
            self.undo.remove(0);
        }
    }

    pub(crate) fn undo(&mut self) -> Result<bool, EditError> {
        if self.drag.is_some() {
            self.cancel_drag()?;
        }
        let Some(command) = self.undo.pop() else { return Ok(false) };
        self.apply(&command, Direction::Before);
        self.redo.push(command);
        Ok(true)
    }

    pub(crate) fn redo(&mut self) -> Result<bool, EditError> {
        let Some(command) = self.redo.pop() else { return Ok(false) };
        self.apply(&command, Direction::After);
        self.undo.push(command);
        Ok(true)
    }

    fn apply(&mut self, command: &Command, direction: Direction) {
        let (value, selection, revision) = match direction {
            Direction::Before => (&command.before, &command.before_selection, command.before_revision),
            Direction::After => (&command.after, &command.after_selection, command.after_revision),
        };
        match command.kind {
            CommandKind::Objects => self.replace_objects(command.index, value),
        }
        self.restore_selection(selection);
        self.revision = revision;
        let uids = Value::array(value.iter().filter_map(uid).map(|u| Value::Num(u as f64)).collect());
        self.emit(
            "history",
            vec![
                ("label", Value::str(&command.label)),
                ("definition", Value::Bool(false)),
                ("animation", Value::Bool(false)),
                ("frameIndices", Value::array(vec![Value::Num(command.index as f64)])),
                ("changedUids", uids),
            ],
        );
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
