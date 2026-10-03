//! Editing commands on the document store (docs/engine/editing.md, stage S5).
//!
//! An [`Editor`] holds a document (the JavaScript editor's report, as the mirror
//! receives it), the playhead, the selection, the gesture in progress and the
//! history, and runs the editor's commands on them so that they change the same
//! records into the same bytes, leave the same selection and emit the same change
//! events as `src/editor.mjs`. The command-replay harness (`replay`, and
//! `engine/tools/edit-replay.mjs` on the JavaScript side) checks that step by step.
//!
//! Ported so far: selecting, seeking, moving and transforming figures, posing,
//! rotating and bending joints, and undo and redo of what those record, on native
//! timelines (docs/engine/editing.md lists the calls). Every other call
//! answers [`EditError::Unported`], so a facade knows to run it in JavaScript.

mod authored;
mod figures;
mod frame;
mod history;
mod ik;
mod pose;
mod record;
pub mod replay;
mod select;
pub mod session;
mod sheet;
mod sheet_frames;
mod timeline;
mod transform;

use std::cell::RefCell;
use std::collections::HashMap;

use styk_doc::Value;

pub use history::{Command, CommandKind};
pub use select::Selection;

/// What a failed call says. `Unported` means "run this in JavaScript": the call
/// changed nothing.
#[derive(Debug, thiserror::Error)]
pub enum EditError {
    #[error("{0} is not ported to the engine yet")]
    Unported(String),
    #[error("{0}")]
    Invalid(String),
}

/// The editor's construction options (`new Editor({propertyChannels, layerTimeline})`)
/// and the settings the app gives it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Options {
    pub property_channels: bool,
    pub layer_timeline: bool,
    /// `editor.autoKey`: every edited track keys the playhead, held cells included.
    pub auto_key: bool,
}

/// A gesture in progress (`editor._drag`).
#[derive(Clone, Debug)]
pub(crate) struct Drag {
    pub label: String,
    /// `selection` (pose the selected subtrees) or `translation` (whole connected figures).
    pub translation: bool,
    pub index: usize,
    /// The affected objects as they were when the gesture began, in slot order.
    pub before: Vec<Value>,
    pub before_selection: select::SelectionState,
    pub changed: bool,
    /// Where figures detached from a parent outside the gesture were (`detachedOrigins`).
    pub detached: HashMap<u32, [f64; 2]>,
    /// The gesture edits a draft of the frame on show (a layer sheet), not a keyframe.
    pub authored: bool,
}

/// The draft of a gesture on a layer sheet (`editor._authoredPoseEdit`).
#[derive(Clone, Debug)]
pub(crate) struct Authored {
    /// The frame being edited: the evaluated frame with the gesture's changes.
    pub frame: Value,
    /// The evaluated frame the gesture started from.
    pub source: Value,
    pub tick: f64,
    /// A command's draft of the whole frame (`transaction`): figures may come and go.
    pub transaction: bool,
}

pub struct Editor {
    pub(crate) report: Value,
    pub(crate) options: Options,
    pub(crate) tick: f64,
    pub(crate) frame_index: usize,
    pub(crate) selection: Selection,
    pub(crate) drag: Option<Drag>,
    pub(crate) authored: Option<Authored>,
    /// The draft of the last committed gesture, which the editor goes on showing as
    /// the evaluated frame until a seek, an edit or history invalidates it
    /// (`_authoredCommitPreview` kept in `_evaluated`). Usually equal to evaluating
    /// again; not after editing a movieclip's replaced frame between its keys.
    pub(crate) retained: Option<Value>,
    pub(crate) undo: Vec<Command>,
    pub(crate) redo: Vec<Command>,
    pub(crate) revision: u64,
    pub(crate) saved_revision: u64,
    pub(crate) revision_serial: u64,
    /// `editor.layerRevision`: named by the events of layer-sheet commands.
    pub(crate) layer_revision: u64,
    /// `editor._nextFrameIdentity`: the number of the next pose source (`frame-N`).
    pub(crate) next_frame_identity: u64,
    /// `editor._nextUid`: the unique id of the next new figure.
    pub(crate) next_uid: u64,
    /// How many poses hold each figure, over the keyframes and the pose sources
    /// (`_layerUidCounts`), and every figure seen so far (`_layerSeenUids`). Read
    /// from the document when first needed, then kept with every command.
    pub(crate) uid_counts: Option<(HashMap<u32, i64>, std::collections::HashSet<u32>)>,
    /// `auto_key` of [`Options`], which the app may change between calls.
    pub(crate) auto_key: bool,
    /// Pose sources this session captured, in order (`_generatedFrameSources`).
    pub(crate) generated_sources: Vec<String>,
    /// Whether the document came with a pose-source table (`_sourceTableWasPresent`).
    pub(crate) source_table_was_present: bool,
    /// The layer of each figure in the last published poses (`_newLayerHints`).
    pub(crate) new_layer_hints: HashMap<u32, String>,
    /// Change events since the last [`Editor::take_events`], as the editor's `change` details.
    pub(crate) events: Vec<Value>,
    /// Typed views and evaluated frames of the current document.
    pub(crate) eval: RefCell<frame::Cache>,
    /// The last document checked for unported features, and the answer.
    gate: RefCell<Option<(Value, Option<&'static str>)>>,
}

/// What the interface reads after a call (`engine/tools/edit-replay.mjs` records the same).
#[derive(Clone, Debug, PartialEq)]
pub struct State {
    pub tick: f64,
    pub frame_index: usize,
    pub selected_uid: Option<u32>,
    pub selected_uids: Vec<u32>,
    pub selected_joint: i64,
    pub can_undo: bool,
    pub can_redo: bool,
    pub undo_label: String,
    pub redo_label: String,
    pub dirty: bool,
    pub layer_revision: u64,
    pub next_frame_identity: u64,
    pub next_uid: u64,
}

/// Everything a call may change, to put back when it turns out not to be ported.
struct Snapshot {
    report: Value,
    tick: f64,
    frame_index: usize,
    selection: Selection,
    drag: Option<Drag>,
    authored: Option<Authored>,
    retained: Option<Value>,
    undo: Vec<Command>,
    redo: Vec<Command>,
    counters: [u64; 6],
    uid_counts: Option<(HashMap<u32, i64>, std::collections::HashSet<u32>)>,
    generated_sources: Vec<String>,
    new_layer_hints: HashMap<u32, String>,
    events: usize,
}

impl Editor {
    /// An editor over `report`, a document the JavaScript editor has loaded (its
    /// normalization on load is not ported: phase 1 starts from the loaded report).
    pub fn new(report: Value, options: Options) -> Editor {
        let source_table_was_present = report.path(&["_styk", "frameSources"]).is_some();
        let next_frame_identity = next_frame_identity(&report);
        Editor {
            report,
            options,
            tick: 0.0,
            frame_index: 0,
            selection: Selection::default(),
            drag: None,
            authored: None,
            retained: None,
            undo: Vec::new(),
            redo: Vec::new(),
            revision: 0,
            saved_revision: 0,
            revision_serial: 0,
            layer_revision: 0,
            next_frame_identity,
            next_uid: 1,
            uid_counts: None,
            auto_key: options.auto_key,
            generated_sources: Vec::new(),
            source_table_was_present,
            new_layer_hints: HashMap::new(),
            events: Vec::new(),
            eval: RefCell::default(),
            gate: RefCell::new(None),
        }
    }

    /// Puts the playhead and selection where the JavaScript editor has them (after
    /// loading, the editor selects and seeks on its own; phase 1 starts from there).
    pub fn set_view(&mut self, tick: f64, frame_index: usize, selection: Selection) {
        self.tick = tick;
        self.frame_index = frame_index;
        self.selection = selection;
    }

    /// The editor's counters that the document does not carry: the layer revision
    /// its events name, the number of the next pose source and the next figure identity.
    pub fn set_counters(&mut self, layer_revision: u64, next_frame_identity: u64, next_uid: u64) {
        self.layer_revision = layer_revision;
        self.next_frame_identity = next_frame_identity;
        self.next_uid = next_uid;
    }

    pub fn report(&self) -> &Value {
        &self.report
    }

    /// The report as the mirror sends it: `JSON.stringify(report)` without `runtime_segments`.
    pub fn report_json(&self) -> String {
        styk_doc::json::to_string(&record::without_runtime(&self.report))
    }

    pub fn take_events(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.events)
    }

    pub fn state(&self) -> State {
        State {
            tick: self.tick,
            frame_index: self.frame_index,
            selected_uid: self.selection.uid,
            selected_uids: self.selection.uids.clone(),
            selected_joint: self.selection.joint,
            can_undo: !self.undo.is_empty(),
            can_redo: !self.redo.is_empty(),
            undo_label: self.undo.last().map(|c| c.label.clone()).unwrap_or_default(),
            redo_label: self.redo.last().map(|c| c.label.clone()).unwrap_or_default(),
            dirty: self.revision != self.saved_revision || self.drag.as_ref().is_some_and(|d| d.changed),
            layer_revision: self.layer_revision,
            next_frame_identity: self.next_frame_identity,
            next_uid: self.next_uid,
        }
    }

    /// `editor.autoKey`, which the app sets from its settings.
    pub fn set_auto_key(&mut self, auto_key: bool) {
        self.auto_key = auto_key;
    }

    /// `editor.activeLayerId = id` when it differs (selection sets it in the editor
    /// without a new revision, so a session's document may be behind).
    pub fn set_active_layer_id(&mut self, id: &str) {
        if self.report.path(&["_styk", "layers", "activeId"]).and_then(Value::as_str) != Some(id) && self.report.path(&["_styk", "layers"]).is_some() {
            self.set_active_layer(id);
        }
    }

    /// The state as JSON, in the key order `engine/tools/edit-replay.mjs` records it.
    pub fn state_value(&self) -> Value {
        let state = self.state();
        let mut object = styk_doc::Obj::new();
        object.set("tick", Value::Num(state.tick));
        object.set("frameIndex", Value::Num(state.frame_index as f64));
        object.set("selectedUid", state.selected_uid.map_or(Value::Null, |u| Value::Num(u as f64)));
        object.set("selectedUids", Value::array(state.selected_uids.iter().map(|&u| Value::Num(u as f64)).collect()));
        object.set("selectedJoint", Value::Num(state.selected_joint as f64));
        object.set("canUndo", Value::Bool(state.can_undo));
        object.set("canRedo", Value::Bool(state.can_redo));
        object.set("undoLabel", Value::str(&state.undo_label));
        object.set("redoLabel", Value::str(&state.redo_label));
        object.set("dirty", Value::Bool(state.dirty));
        object.set("layerRevision", Value::Num(state.layer_revision as f64));
        object.set("nextFrameIdentity", Value::Num(state.next_frame_identity as f64));
        object.set("nextUid", Value::Num(state.next_uid as f64));
        Value::object(object)
    }

    /// The figures the gesture in progress took, as they are in its draft (without
    /// the clip sample times evaluation adds); `None` outside a gesture on a sheet.
    pub fn draft_figures(&self) -> Option<Vec<Value>> {
        let (drag, edit) = (self.drag.as_ref()?, self.authored.as_ref()?);
        let taken: Vec<u32> = drag.before.iter().filter_map(record::uid).collect();
        let figures = frame::objects_of(&edit.frame).into_iter().filter(|o| record::uid(o).is_some_and(|u| taken.contains(&u))).map(|figure| {
            let mut figure = figure.as_obj().cloned().unwrap_or_default();
            figure.remove("_movieclipSample");
            Value::object(figure)
        });
        Some(figures.collect())
    }

    /// Runs one editor call by its JavaScript name, with JSON arguments; returns what
    /// the JavaScript method returns.
    pub fn call(&mut self, method: &str, args: &[Value]) -> Result<Value, EditError> {
        let snapshot = self.snapshot();
        let result = self.dispatch(method, args);
        if matches!(result, Err(EditError::Unported(_))) {
            self.restore(snapshot);
        }
        result
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            report: self.report.clone(),
            tick: self.tick,
            frame_index: self.frame_index,
            selection: self.selection.clone(),
            drag: self.drag.clone(),
            authored: self.authored.clone(),
            retained: self.retained.clone(),
            undo: self.undo.clone(),
            redo: self.redo.clone(),
            counters: [self.revision, self.saved_revision, self.revision_serial, self.layer_revision, self.next_frame_identity, self.next_uid],
            uid_counts: self.uid_counts.clone(),
            generated_sources: self.generated_sources.clone(),
            new_layer_hints: self.new_layer_hints.clone(),
            events: self.events.len(),
        }
    }

    fn restore(&mut self, snapshot: Snapshot) {
        self.report = snapshot.report;
        self.tick = snapshot.tick;
        self.frame_index = snapshot.frame_index;
        self.selection = snapshot.selection;
        self.drag = snapshot.drag;
        self.authored = snapshot.authored;
        self.retained = snapshot.retained;
        self.undo = snapshot.undo;
        self.redo = snapshot.redo;
        [self.revision, self.saved_revision, self.revision_serial, self.layer_revision, self.next_frame_identity, self.next_uid] = snapshot.counters;
        self.uid_counts = snapshot.uid_counts;
        self.generated_sources = snapshot.generated_sources;
        self.new_layer_hints = snapshot.new_layer_hints;
        self.events.truncate(snapshot.events);
    }

    fn dispatch(&mut self, method: &str, args: &[Value]) -> Result<Value, EditError> {
        let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Null);
        // A JSON argument that is not a number is NaN to the finite checks, as `Number.isFinite` sees it.
        let number = |i: usize| arg(i).as_f64().unwrap_or(f64::NAN);
        if let Some(reason) = self.unported_document() {
            return Err(EditError::Unported(format!("{method} {reason}")));
        }
        if let Some(reason) = self.ik_gate(method)? {
            return Err(EditError::Unported(format!("{method} {reason}")));
        }
        match method {
            "select" | "@select" => {
                // `select(uid, {toggle, add})` is `select(uid, -1, {toggle, add})`.
                let (joint, options) = if arg(1).is_object() { (-1, arg(1)) } else { (arg(1).as_f64().unwrap_or(-1.0) as i64, arg(2)) };
                let flag = |key| options.get(key).is_some_and(Value::truthy);
                self.select(arg(0).as_f64().filter(|v| v.fract() == 0.0 && *v >= 0.0).map(|v| v as u32), joint, flag("toggle"), flag("add"))
            }
            "selectMany" => self.select_many(&arg(0).numbers().unwrap_or_default().into_iter().map(|v| v as u32).collect::<Vec<_>>()),
            "seek" | "@seek" => self.seek(arg(0).as_f64().unwrap_or(0.0)),
            "beginDrag" => {
                let translation = match arg(1).get("scope").and_then(Value::as_str) {
                    None | Some("selection") => false,
                    Some("translation") => true,
                    Some(other) => return Err(EditError::Unported(format!("beginDrag with scope {other}"))),
                };
                self.begin_drag(arg(0).as_str().unwrap_or("Pose figure"), translation).map(Value::Bool)
            }
            "commitDrag" => self.commit_drag().map(Value::Bool),
            "cancelDrag" => self.cancel_drag().map(Value::Bool),
            "beginTranslationDrag" => self.begin_drag(arg(0).as_str().unwrap_or("Move figures"), true).map(Value::Bool),
            "moveSelection" => self.move_selection(arg(0).as_f64().unwrap_or(f64::NAN), arg(1).as_f64().unwrap_or(f64::NAN), &arg(2)).map(Value::Bool),
            "rotateSelection" => self.rotate_selection(number(0), &arg(1)).map(Value::Bool),
            "scaleSelection" => self.scale_selection(number(0), &arg(1), false).map(Value::Bool),
            "scaleSelected" => self.scale_selection(number(0), &arg(1), true).map(Value::Bool),
            "mirrorSelection" => self.mirror_selection(&arg(0), &arg(1)).map(Value::Bool),
            "rotateFigure" => self.rotate_figure(number(0)).map(Value::Bool),
            "centerFigure" => self.center_figure().map(Value::Bool),
            "resetPose" => self.reset_pose().map(Value::Bool),
            "poseJoint" => self.pose_joint(number(0), number(1), &arg(2)).map(Value::Bool),
            "rotateJoint" => self.rotate_joint(number(0), &arg(1)).map(Value::Bool),
            "bendJoint" => self.bend_joint(number(0), number(1), &arg(2)).map(Value::Bool),
            "sheetInsertKey" => self.sheet_insert_key(&arg(0), &arg(1), &arg(2)).map(Value::Bool),
            "sheetDuplicateFrameAfter" => self.sheet_duplicate_frame_after(&arg(0), &arg(1)).map(Value::Bool),
            "sheetSetEasing" => self.sheet_set_easing(&arg(0), number(1), number(2), &arg(3)).map(Value::Bool),
            "sheetClearKeys" => self.sheet_clear_keys(&arg(0), number(1), &arg(2)).map(Value::Bool),
            "sheetClearFrames" => self.sheet_clear_frames(&arg(0), number(1), &arg(2)).map(Value::Bool),
            "sheetInsertFrames" => self.sheet_insert_frames(&arg(0), number(1), &arg(2)).map(Value::Bool),
            "sheetRemoveFrames" => self.sheet_remove_frames(&arg(0), number(1), &arg(2)).map(Value::Bool),
            "sheetSetDuration" => self.sheet_set_duration(number(0), &arg(1), &arg(2)).map(Value::Bool),
            "sheetAppendFrame" => self.sheet_append_frame(&arg(0)),
            "joinTo" => self.join_to(arg(0).as_f64(), &arg(1)).map(Value::Bool),
            "unjoinFigure" => self.unjoin_figure().map(Value::Bool),
            "joinFigure" => if arg(0).is_null() { self.unjoin_figure() } else { self.join_to(arg(0).as_f64(), &arg(1)) }.map(Value::Bool),
            "deleteSelection" | "deleteFigure" => self.delete_selection().map(Value::Bool),
            "duplicateFigure" => self.duplicate_figure(),
            "undo" => self.undo().map(Value::Bool),
            "redo" => self.redo().map(Value::Bool),
            other => Err(EditError::Unported(other.to_string())),
        }
    }

    /// Why the document's commands still run in JavaScript, if they do. The answer
    /// is kept for as long as the document does not change.
    fn unported_document(&self) -> Option<&'static str> {
        if let Some((report, reason)) = self.gate.borrow().as_ref()
            && report.same_node(&self.report)
        {
            return *reason;
        }
        let reason = self.scan_unported();
        *self.gate.borrow_mut() = Some((self.report.clone(), reason));
        reason
    }

    /// What phase 1 has not ported yet, found by reading the whole document.
    fn scan_unported(&self) -> Option<&'static str> {
        let styk = self.report.get("_styk");
        let sheet = styk.and_then(|s| s.get("layerSheet"));
        let list = |value: Option<&Value>| -> Vec<Value> { value.and_then(Value::elements).map(|e| e.iter().collect()).unwrap_or_default() };
        if sheet.is_none() && (self.options.property_channels || self.options.layer_timeline) {
            // The app's editor gives every document a layer sheet when it loads it.
            return Some("with property channels on a native timeline");
        }
        if styk.and_then(|s| s.path(&["animation", "channels"])).and_then(Value::array_len).unwrap_or(0) > 0 {
            return Some("with property channels");
        }
        if styk.and_then(|s| s.get("movieclipSource")).is_some() {
            return Some("inside a movieclip");
        }
        let figures = list(self.report.get("figure_types"));
        if figures.iter().any(|figure| figure.path(&["_styk", "animationOrigin"]).is_some_and(Value::truthy)) {
            return Some("with animation origins");
        }
        // IK controllers are gated per call, on the frame on show (`ik_gate`).
        None
    }

    /// Emits a change event: `{kind, frameIndex, tick, structural: false, timing: false, ...detail}`.
    pub(crate) fn emit(&mut self, kind: &str, detail: Vec<(&str, Value)>) {
        let mut object = styk_doc::Obj::new();
        object.set("kind", Value::str(kind));
        object.set("frameIndex", Value::Num(self.frame_index as f64));
        object.set("tick", Value::Num(self.tick));
        object.set("structural", Value::Bool(false));
        object.set("timing", Value::Bool(false));
        for (key, value) in detail {
            object.set(key, value);
        }
        self.events.push(Value::object(object));
    }
}

/// `validateFrameIdentitySteps`: one past the largest `frame-N` among the keyframes
/// and the pose sources.
fn next_frame_identity(report: &Value) -> u64 {
    let number = |id: Option<&str>| id.and_then(|id| id.strip_prefix("frame-")).filter(|n| !n.starts_with('0')).and_then(|n| n.parse::<u64>().ok());
    let frames = report.get("keyframes").and_then(Value::elements).map(|e| e.iter().collect::<Vec<_>>()).unwrap_or_default();
    let keyed = frames.iter().filter_map(|frame| number(frame.path(&["_styk", "frameId"]).and_then(Value::as_str)));
    let sources = report.path(&["_styk", "frameSources"]).and_then(Value::as_obj).map(|table| table.keys().filter_map(|id| number(Some(id))).collect::<Vec<_>>()).unwrap_or_default();
    keyed.chain(sources).max().map_or(1, |largest| largest + 1)
}
