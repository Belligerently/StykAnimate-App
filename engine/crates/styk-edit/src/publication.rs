//! Worker-prepared publication over the existing Editor/history owner.
//!
//! No API here may run on the interface thread. Preparation and validation can
//! traverse retained metadata. The guarded commit itself only exchanges heads;
//! its returned retirement must stay on the worker until the common packet ACK.
use std::collections::VecDeque;

use styk_doc::Value;

use crate::{EditError, Editor, HistoryLimits, Options, Snapshot, frame, select};

fn quiet(editor: &Editor) -> bool {
    editor.drag.is_none()
        && editor.authored.is_none()
        && editor.property_edit.is_none()
        && editor.vfx_edit.is_none()
        && editor.batch.is_none()
}
fn same_value(left: Option<&Value>, right: Option<&Value>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => left.same_node(right),
        _ => false,
    }
}

/// The existing rollback Snapshot plus the resources its ordinary synchronous
/// rollback deliberately leaves warm. All clones happen on the owning worker.
struct Heads {
    snapshot: Snapshot,
    events: Vec<Value>,
    cache: frame::Cache,
    gate: Option<(Value, Option<&'static str>)>,
    limits: HistoryLimits,
    ready: bool,
    auto_key: bool,
    draws: VecDeque<(String, Value)>,
}
impl Heads {
    fn capture(editor: &Editor) -> Self {
        Self {
            snapshot: editor.snapshot(),
            events: editor.events.clone(),
            cache: editor.eval.borrow().clone(),
            gate: editor.gate.borrow().clone(),
            limits: editor.history_limits,
            ready: editor.worker_history,
            auto_key: editor.auto_key,
            draws: editor.draws.clone(),
        }
    }
    /// Swaps complete prepared heads, retaining every displaced large value.
    /// No cloning, allocation, serialization, dropping or owner callback here.
    fn exchange(&mut self, editor: &mut Editor) {
        editor.exchange_snapshot(&mut self.snapshot);
        std::mem::swap(&mut editor.events, &mut self.events);
        std::mem::swap(editor.eval.get_mut(), &mut self.cache);
        std::mem::swap(editor.gate.get_mut(), &mut self.gate);
        std::mem::swap(&mut editor.history_limits, &mut self.limits);
        std::mem::swap(&mut editor.worker_history, &mut self.ready);
        std::mem::swap(&mut editor.auto_key, &mut self.auto_key);
        std::mem::swap(&mut editor.draws, &mut self.draws);
    }
}

/// Validated host generation/lineage is an additional caller invariant, as for
/// the borrowed SheetPlan API. This guard checks the actual editing owner, not
/// a serialized document or another Editor/evaluator/cache.
struct Guard {
    report: Value,
    counters: [u64; 6],
    selection: select::SelectionState,
    selection_metadata: select::SelectionMetadata,
    options: Options,
    size: u64,
    lengths: [usize; 2],
    limits: HistoryLimits,
    auto_key: bool,
    retained: Option<Value>,
    events: Vec<Value>,
    sheet_clipboard: Option<crate::sheet_clipboard::Clipboard>,
    figure_clipboard: Option<crate::figure_clipboard::FigureClipboard>,
    segment_values_clipboard: Option<String>,
    camera_clipboard: Option<Value>,
    camera_motion_clipboard: Option<Value>,
    vfx_clip_clipboard: Option<(Value, Option<Value>)>,
    draws: VecDeque<(String, Value)>,
}
impl Guard {
    fn capture(editor: &Editor) -> Self {
        Self {
            report: editor.report.clone(),
            counters: [
                editor.revision,
                editor.saved_revision,
                editor.revision_serial,
                editor.layer_revision,
                editor.next_frame_identity,
                editor.next_uid,
            ],
            selection: editor.selection_state(),
            selection_metadata: editor.selection_metadata.clone(),
            options: editor.options,
            size: editor.history_size.to_bits(),
            lengths: [editor.undo.len(), editor.redo.len()],
            limits: editor.history_limits,
            auto_key: editor.auto_key,
            retained: editor.retained.clone(),
            events: editor.events.clone(),
            sheet_clipboard: editor.sheet_clipboard.clone(),
            figure_clipboard: editor.figure_clipboard.clone(),
            segment_values_clipboard: editor.segment_values_clipboard.clone(),
            camera_clipboard: editor.camera_clipboard.clone(),
            camera_motion_clipboard: editor.camera_motion_clipboard.clone(),
            vfx_clip_clipboard: editor.vfx_clip_clipboard.clone(),
            draws: editor.draws.clone(),
        }
    }
    fn matches(&self, editor: &Editor) -> bool {
        quiet(editor)
            && editor.worker_history
            && self.report.same_node(&editor.report)
            && self.counters
                == [
                    editor.revision,
                    editor.saved_revision,
                    editor.revision_serial,
                    editor.layer_revision,
                    editor.next_frame_identity,
                    editor.next_uid,
                ]
            && self.selection == editor.selection_state()
            && self.selection_metadata == editor.selection_metadata
            && self.options.property_channels == editor.options.property_channels
            && self.options.layer_timeline == editor.options.layer_timeline
            && self.options.auto_key == editor.options.auto_key
            && self.size == editor.history_size.to_bits()
            && self.lengths == [editor.undo.len(), editor.redo.len()]
            && self.limits.count == editor.history_limits.count
            && self.limits.bytes.to_bits() == editor.history_limits.bytes.to_bits()
            && self.auto_key == editor.auto_key
            && same_value(self.retained.as_ref(), editor.retained.as_ref())
            && self.events.len() == editor.events.len()
            && self
                .events
                .iter()
                .zip(&editor.events)
                .all(|(a, b)| a.same_node(b))
            && self.sheet_clipboard == editor.sheet_clipboard
            && self.figure_clipboard == editor.figure_clipboard
            && self.segment_values_clipboard == editor.segment_values_clipboard
            && same_value(
                self.camera_clipboard.as_ref(),
                editor.camera_clipboard.as_ref(),
            )
            && same_value(
                self.camera_motion_clipboard.as_ref(),
                editor.camera_motion_clipboard.as_ref(),
            )
            && self.vfx_clip_clipboard == editor.vfx_clip_clipboard
            && self.draws == editor.draws
    }
}

/// Immutable results of the actual recorded publisher/history callback. All
/// large retained state stays on the worker; an interface job owns only IDs.
pub struct PreparedPublication {
    guard: Guard,
    heads: Heads,
    before_state: Value,
    before_history: Value,
    result: Value,
    state: Value,
    history: Value,
    events: Value,
}
impl PreparedPublication {
    pub fn before_report(&self) -> &Value {
        &self.guard.report
    }
    pub fn before_revision(&self) -> u64 { self.guard.counters[0] }
    pub fn report(&self) -> &Value {
        &self.heads.snapshot.report
    }
    pub fn result(&self) -> &Value {
        &self.result
    }
    pub fn state(&self) -> &Value {
        &self.state
    }
    pub fn history(&self) -> &Value {
        &self.history
    }
    pub fn events(&self) -> &Value {
        &self.events
    }
    pub fn before_state(&self) -> &Value {
        &self.before_state
    }
    pub fn before_history(&self) -> &Value {
        &self.before_history
    }

    /// Worker-only validation; refusal returns the payload for worker disposal.
    /// Once validated, safe Rust's exclusive borrow prevents an intervening edit.
    pub fn guard_worker<'a>(
        self,
        editor: &'a mut Editor,
    ) -> Result<GuardedPublication<'a>, (EditError, Self)> {
        self.guard_worker_checked(editor, || Ok(()))
    }

    pub fn guard_worker_checked<'a>(
        self,
        editor: &'a mut Editor,
        mut check: impl FnMut() -> Result<(), EditError>,
    ) -> Result<GuardedPublication<'a>, (EditError, Self)> {
        if let Err(error) = check() {
            return Err((error, self));
        }
        let matches = self.guard.matches(editor);
        if let Err(error) = check() {
            return Err((error, self));
        }
        if !matches {
            return Err((
                EditError::Invalid(
                    "errors.editorLayerAsync.theAnimationChangedWhileTheLayerMoveWas".into(),
                ),
                self,
            ));
        }
        Ok(GuardedPublication {
            editor,
            prepared: self,
        })
    }
}

/// Holds the same editing owner exclusively after its worker guard succeeded.
pub struct GuardedPublication<'a> {
    editor: &'a mut Editor,
    prepared: PreparedPublication,
}
impl GuardedPublication<'_> {
    /// Canonical roots of this already-guarded transaction, borrowed on its worker.
    pub fn before_report(&self) -> &Value { self.prepared.before_report() }
    pub fn report(&self) -> &Value { self.prepared.report() }
    pub fn revision(&self) -> u64 { self.prepared.heads.snapshot.counters[0] }
    /// Constant head exchange; retains all displaced payloads in the return.
    /// The host must keep this retirement on its durable worker through ACK.
    pub fn commit(mut self) -> RetiredPublication {
        self.prepared.heads.exchange(self.editor);
        RetiredPublication {
            prepared: self.prepared,
        }
    }
}

/// Retains old/rollback/history/cache views for ACK, cancellation and disposal.
/// Never give this strong payload to an interface handle or drop it on the UI.
pub struct RetiredPublication {
    prepared: PreparedPublication,
}
impl RetiredPublication {
    pub fn previous_report(&self) -> &Value {
        &self.prepared.heads.snapshot.report
    }
    pub fn result(&self) -> &Value {
        &self.prepared.result
    }
    pub fn state(&self) -> &Value {
        &self.prepared.state
    }
    pub fn history(&self) -> &Value {
        &self.prepared.history
    }
    pub fn events(&self) -> &Value {
        &self.prepared.events
    }
}

impl Editor {
    /// Runs a recorded publication/history operation on this same owning worker,
    /// builds immutable views, then restores the live owner until guarded commit.
    /// Errors/cancel/stale restore all captured state; no second Editor is built.
    /// The caller must use the same validated generation/lineage as the Plan and
    /// retain all returned strong payloads on its durable worker until packet ACK.
    pub fn prepare_worker_publication(
        &mut self,
        operation: impl FnOnce(
            &mut Editor,
            &mut dyn FnMut() -> Result<(), EditError>,
        ) -> Result<Value, EditError>,
        mut check: impl FnMut() -> Result<(), EditError>,
    ) -> Result<PreparedPublication, EditError> {
        check()?;
        if !self.worker_history {
            return Err(EditError::Unported(
                "worker history has not been prepared".into(),
            ));
        }
        if !quiet(self) {
            return Err(EditError::Unported(
                "publication with a live editing gesture or batch".into(),
            ));
        }
        let guard = Guard::capture(self);
        let mut heads = Heads::capture(self);
        let before_state = self.state_value();
        let before_history = self.history_value();
        let result = operation(self, &mut check).and_then(|result| {
            check()?;
            if !quiet(self) {
                return Err(EditError::Unported(
                    "publication left a live editing gesture or batch".into(),
                ));
            }
            if self.events.len() < guard.events.len()
                || !self
                    .events
                    .iter()
                    .zip(&guard.events)
                    .all(|(a, b)| a.same_node(b))
            {
                return Err(EditError::Unported(
                    "publication consumed previously queued change events".into(),
                ));
            }
            Ok(result)
        });
        match result {
            Err(error) => {
                heads.exchange(self);
                Err(error)
            }
            Ok(result) => {
                let state = self.state_value();
                let history = self.history_value();
                let events =
                    Value::array(self.events[guard.events.len().min(self.events.len())..].to_vec());
                let final_check = check();
                heads.exchange(self);
                final_check?;
                Ok(PreparedPublication {
                    guard,
                    heads,
                    before_state,
                    before_history,
                    result,
                    state,
                    history,
                    events,
                })
            }
        }
    }
}
