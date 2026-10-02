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

mod history;
mod pose;
mod record;
pub mod replay;
mod select;
mod timeline;
mod transform;

use styk_doc::Value;

pub use history::{Command, CommandKind};
pub use select::Selection;

/// What a failed call says. `Unported` means "run this in JavaScript".
#[derive(Debug, thiserror::Error)]
pub enum EditError {
    #[error("{0} is not ported to the engine yet")]
    Unported(String),
    #[error("{0}")]
    Invalid(String),
}

/// The editor's construction options (`new Editor({propertyChannels, layerTimeline})`).
#[derive(Clone, Copy, Debug, Default)]
pub struct Options {
    pub property_channels: bool,
    pub layer_timeline: bool,
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
    pub detached: std::collections::HashMap<u32, [f64; 2]>,
}

pub struct Editor {
    pub(crate) report: Value,
    pub(crate) options: Options,
    pub(crate) tick: f64,
    pub(crate) frame_index: usize,
    pub(crate) selection: Selection,
    pub(crate) drag: Option<Drag>,
    pub(crate) undo: Vec<Command>,
    pub(crate) redo: Vec<Command>,
    pub(crate) revision: u64,
    pub(crate) saved_revision: u64,
    pub(crate) revision_serial: u64,
    /// Change events since the last [`Editor::take_events`], as the editor's `change` details.
    pub(crate) events: Vec<Value>,
    /// Typed views of the report for geometry, converted again only where records changed.
    pub(crate) loader: styk_eval::load::Loader,
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
}

impl Editor {
    /// An editor over `report`, a document the JavaScript editor has loaded (its
    /// normalization on load is not ported: phase 1 starts from the loaded report).
    pub fn new(report: Value, options: Options) -> Editor {
        Editor {
            report,
            options,
            tick: 0.0,
            frame_index: 0,
            selection: Selection::default(),
            drag: None,
            undo: Vec::new(),
            redo: Vec::new(),
            revision: 0,
            saved_revision: 0,
            revision_serial: 0,
            events: Vec::new(),
            loader: styk_eval::load::Loader::new(),
        }
    }

    /// Puts the playhead and selection where the JavaScript editor has them (after
    /// loading, the editor selects and seeks on its own; phase 1 starts from there).
    pub fn set_view(&mut self, tick: f64, frame_index: usize, selection: Selection) {
        self.tick = tick;
        self.frame_index = frame_index;
        self.selection = selection;
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
        }
    }

    /// Runs one editor call by its JavaScript name, with JSON arguments; returns what
    /// the JavaScript method returns.
    pub fn call(&mut self, method: &str, args: &[Value]) -> Result<Value, EditError> {
        let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Null);
        // A JSON argument that is not a number is NaN to the finite checks, as `Number.isFinite` sees it.
        let number = |i: usize| arg(i).as_f64().unwrap_or(f64::NAN);
        if let Some(reason) = self.unported_document() {
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
            "undo" => self.undo().map(Value::Bool),
            "redo" => self.redo().map(Value::Bool),
            other => Err(EditError::Unported(other.to_string())),
        }
    }

    /// Why the document's commands still run in JavaScript, if they do: what phase 1
    /// has not ported yet (layer sheets, property channels, animation origins,
    /// movieclip sources, inverse kinematics).
    fn unported_document(&self) -> Option<&'static str> {
        let styk = self.report.get("_styk");
        if self.options.layer_timeline || styk.and_then(|s| s.get("layerSheet")).is_some() {
            return Some("on a layer sheet");
        }
        if self.options.property_channels || styk.and_then(|s| s.path(&["animation", "channels"])).and_then(Value::array_len).unwrap_or(0) > 0 {
            return Some("with property channels");
        }
        if styk.and_then(|s| s.get("movieclipSource")).is_some() {
            return Some("inside a movieclip");
        }
        let figures = self.report.get("figure_types").and_then(Value::elements);
        if figures.is_some_and(|f| f.iter().any(|figure| figure.path(&["_styk", "animationOrigin"]).is_some_and(Value::truthy))) {
            return Some("with animation origins");
        }
        let frames = self.report.get("keyframes").and_then(Value::elements);
        if frames.is_some_and(|f| f.iter().any(|frame| frame.get("objects").and_then(Value::elements).is_some_and(|o| o.iter().any(|object| object.get("_stykIK").is_some())))) {
            return Some("with inverse kinematics");
        }
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
