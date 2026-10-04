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

/// `a === b` for two values: numbers by value, text by content, containers by identity.
pub(crate) fn js_strict_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Num(x), Value::Num(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Null, Value::Null) => true,
        (x @ (Value::Str(_) | Value::Blob(_)), y @ (Value::Str(_) | Value::Blob(_))) => x.text() == y.text(),
        (x, y) => x.same_node(y),
    }
}

/// What a history entry changed.
#[derive(Clone, Debug)]
pub enum CommandKind {
    /// Whole records of one native keyframe's objects, replaced by unique id.
    Objects { index: usize, before: Vec<Value>, after: Vec<Value> },
    /// The layer sheet and layer state before and after, and the pose sources the
    /// command captured (`kind: 'layer-sheet'`).
    LayerSheet { before: Value, after: Value, before_layers: Value, after_layers: Value, sources: Vec<(String, Value)>, definitions: Option<(usize, Vec<Value>)> },
    /// Paths of the layer state before and after, and the sheet when the layers' tracks
    /// changed with them (`kind: 'layers'`).
    Layers { changes: Vec<crate::layers::LayerChange>, sheet: Option<(Value, Value)>, structural: bool, timing: bool },
    /// A figure definition before and after (`kind: 'figure'`).
    Figure { index: usize, before: Value, after: Value },
    /// The camera motion table before (none when there was none) and after, and the sheet
    /// when the clips lengthened it (`kind: 'camera-motion'`).
    CameraMotion { before: Option<Value>, after: Value, sheet: Option<(Value, Value)> },
    /// The project palette before and after, absent when there is none (`kind: 'palette'`).
    Palette { before: Option<Value>, after: Option<Value> },
    /// The audio before (the empty audio when there was none) and after, and whether the
    /// document had audio before (`kind: 'audio'`).
    Audio { before: Value, after: Value, before_present: bool },
    /// The effect clips before (none when there were none) and after, and the sheet when
    /// the clips lengthened it (`kind: 'vfx-clips'`).
    VfxClips { before: Option<Value>, after: Value, sheet: Option<(Value, Value)> },
    /// Fields of the document before (absent ones as none) and after (`kind: 'fields'`).
    Fields { before: Vec<(String, Option<Value>)>, after: Vec<(String, Value)>, timing: bool },
    /// A movieclip definition before (none when it was added) and after (none when removed)
    /// at its place in the definitions (`kind: 'movieclip-definition'`).
    MovieclipDefinition { index: usize, before: Option<Value>, after: Option<Value> },
    /// Entries recorded inside `movieclipBatch`, undone and redone together (`kind: 'movieclip-batch'`).
    Batch { commands: Vec<Command> },
    /// Native keyframes patched by a figure operation across frames (`kind: 'figure-range'`).
    FigureRange { changes: Vec<crate::range::FrameChange>, structural: bool },
    /// Paths of a figure definition before and after (`kind: 'native-properties'`, `scope: 'figure'`).
    NativeProperties { index: usize, before: crate::builder::Patch, after: crate::builder::Patch },
}

/// `editor._movieclipBatch` in progress: its entries, label, the revision before it and the
/// selection when it began.
#[derive(Clone, Debug)]
pub(crate) struct Batch {
    commands: Vec<Command>,
    label: String,
    revision: u64,
    before_selection: SelectionState,
    /// Batches opened inside this one, which only run their operation.
    nested: u32,
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
        if let Some(batch) = self.batch.as_mut() {
            batch.commands.push(command);
            return Ok(());
        }
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

    /// `movieclipBatch(label, operation)` begins: entries recorded until it ends become one.
    pub(crate) fn begin_batch(&mut self, label: &str) -> Result<(), EditError> {
        match self.batch.as_mut() {
            Some(batch) => batch.nested += 1,
            None => self.batch = Some(Batch { commands: Vec::new(), label: label.into(), revision: self.revision, before_selection: self.selection_state(), nested: 0 }),
        }
        Ok(())
    }

    /// The batch's operation returned: its entries as one, and the batch's change event.
    pub(crate) fn end_batch(&mut self) -> Result<(), EditError> {
        let Some(batch) = self.batch.as_mut() else { return Ok(()) };
        if batch.nested > 0 {
            batch.nested -= 1;
            return Ok(());
        }
        let batch = self.batch.take().unwrap();
        if !batch.commands.is_empty() {
            let after_selection = self.selection_state();
            self.record(Command { kind: CommandKind::Batch { commands: batch.commands }, label: batch.label.clone(), before_selection: batch.before_selection, after_selection, before_revision: 0, after_revision: 0 })?;
            if let Some(command) = self.undo.last_mut() {
                command.before_revision = batch.revision;
            }
            if self.sheet() {
                self.prune_frame_sources();
            }
        }
        self.emit("edit", vec![("structural", Value::Bool(true)), ("timing", Value::Bool(true)), ("label", Value::str(&batch.label))]);
        Ok(())
    }

    /// The batch's operation failed: its entries undone, the revision as before it.
    pub(crate) fn abort_batch(&mut self) -> Result<(), EditError> {
        let Some(batch) = self.batch.take() else { return Ok(()) };
        for command in batch.commands.iter().rev() {
            self.apply(command, Direction::Before)?;
        }
        self.revision = batch.revision;
        Ok(())
    }

    pub(crate) fn undo(&mut self) -> Result<bool, EditError> {
        self.cancel_vfx_clip_edit()?;
        if self.drag.is_some() {
            self.cancel_drag()?;
        }
        let Some(command) = self.undo.pop() else { return Ok(false) };
        self.apply(&command, Direction::Before)?;
        self.redo.push(command);
        Ok(true)
    }

    pub(crate) fn redo(&mut self) -> Result<bool, EditError> {
        self.cancel_vfx_clip_edit()?;
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
            CommandKind::CameraMotion { .. } => self.apply_camera_command(command, direction)?,
            CommandKind::Palette { .. } => self.apply_palette_command(command, direction)?,
            CommandKind::Audio { .. } => self.apply_audio_command(command, direction)?,
            CommandKind::VfxClips { .. } => self.apply_vfx_command(command, direction)?,
            CommandKind::Fields { .. } => self.apply_fields_command(command, direction)?,
            CommandKind::MovieclipDefinition { .. } => self.apply_movieclip_definition(command, direction)?,
            CommandKind::FigureRange { .. } => self.apply_figure_range_command(command, direction)?,
            CommandKind::NativeProperties { .. } => self.apply_native_properties(command, direction)?,
            CommandKind::Batch { commands } => {
                let ordered: Vec<&Command> = if direction == Direction::Before { commands.iter().rev().collect() } else { commands.iter().collect() };
                for child in ordered {
                    self.apply(child, direction)?;
                }
                self.restore_selection(selection)?;
                self.revision = revision;
            }
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
