//! Bounded controls for the one canonical worker Session. No Editor, report,
//! cursor or retirement is owned by this interface binding.
use crate::{engine::EngineHandle, js_error};
use styk_edit::{HistoryLimits, session::RevisionCounters};
use styk_engine::session::{
    Epochs, FreshHistory, Handle, MAX_SESSION_CONTROL_BYTES, Packet, Phase, Stamp,
};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct EditWorker {
    handle: Handle,
    incoming: Option<Packet>,
}
fn counter(value: f64) -> Result<u64, JsError> {
    if !value.is_finite() || value < 0.0 || value.fract() != 0.0 || value > 9_007_199_254_740_991.0
    {
        return Err(js_error("invalid editing control counter"));
    }
    Ok(value as u64)
}
fn identity(values: &js_sys::Float64Array, count: u32) -> Result<(Stamp, Epochs), JsError> {
    if values.length() != count {
        return Err(js_error("invalid editing control identity"));
    }
    let value = |at| counter(values.get_index(at));
    Ok((
        Stamp {
            generation: value(0)?,
            lineage: value(1)?,
            revision: value(2)?,
            sequence: value(3)?,
        },
        Epochs {
            shown: value(4)?,
            view: value(5)?,
            gesture: value(6)?,
        },
    ))
}
fn epochs(values: &js_sys::Float64Array) -> Result<Epochs, JsError> {
    if values.length() != 3 {
        return Err(js_error("invalid editing view epochs"));
    }
    Ok(Epochs {
        shown: counter(values.get_index(0))?,
        view: counter(values.get_index(1))?,
        gesture: counter(values.get_index(2))?,
    })
}
fn text(value: &js_sys::JsString) -> Result<String, JsError> {
    value
        .as_string()
        .ok_or_else(|| js_error("editing controls require strings"))
}
fn phase(value: Phase) -> &'static str {
    match value {
        Phase::Unavailable => "unavailable",
        Phase::Idle => "idle",
        Phase::Preparing => "preparing",
        Phase::Attached => "attached",
        Phase::Streaming => "streaming",
        Phase::Ready => "ready",
        Phase::Success => "success",
        Phase::Failed => "failed",
        Phase::Closed => "closed",
    }
}
#[wasm_bindgen]
impl EngineHandle {
    /// Identity7, original revision/serial/saved3, limit count/bytes2,
    /// checkpoint/undo length/redo length3. Validate foreign warmth before
    /// copying even bounded state. Session construction happens on a worker.
    pub fn request_edit_worker(
        &self,
        state: js_sys::JsString,
        values: js_sys::Float64Array,
    ) -> Result<Option<EditWorker>, JsError> {
        let (stamp, epochs) = identity(&values, 15)?;
        let undo = counter(values.get_index(13))?;
        let redo = counter(values.get_index(14))?;
        let checkpoint = values.get_index(12);
        if undo != 0 || redo != 0 || checkpoint != 0.0 {
            return Err(js_error(
                "a warm JavaScript history must keep its existing owner",
            ));
        }
        // UTF-8 occupies at most three bytes per UTF-16 code unit. Check before
        // as_string; no unbounded JS string is copied into owned wasm memory.
        if state.length() as usize > MAX_SESSION_CONTROL_BYTES / 3 {
            return Err(js_error(
                "editing bootstrap state exceeds its control budget",
            ));
        }
        let history = FreshHistory {
            undo: undo as usize,
            redo: redo as usize,
            checkpoint,
            counters: RevisionCounters {
                revision: counter(values.get_index(7))?,
                serial: counter(values.get_index(8))?,
                saved: counter(values.get_index(9))?,
            },
            limits: HistoryLimits {
                count: usize::try_from(counter(values.get_index(10))?).map_err(js_error)?,
                bytes: values.get_index(11),
            },
        };
        let handle = match self.engine.request_fresh_session(stamp, epochs, &text(&state)?, history) {
            Ok(handle) => handle,
            Err(styk_engine::session::Error::Busy) => return Ok(None),
            Err(error) => return Err(js_error(error)),
        };
        Ok(Some(EditWorker {
            handle,
            incoming: None,
        }))
    }
}
#[wasm_bindgen]
impl EditWorker {
    /// The host validates its known small argument/view shapes BEFORE JSON.
    /// This binding checks string lengths again before copying UTF-8; parsing
    /// and the existing command callbacks run on the owning worker only.
    pub fn submit(
        &self,
        values: js_sys::Float64Array,
        method: js_sys::JsString,
        args: js_sys::JsString,
        view: js_sys::JsString,
    ) -> Result<f64, JsError> {
        self.submit_inner(values, method, args, view, false)
    }
    /// Explicit asynchronous project-settings route, not a replacement of the
    /// synchronous JavaScript Editor method/return contract.
    pub fn submit_project_fields(
        &self, values: js_sys::Float64Array, method: js_sys::JsString,
        args: js_sys::JsString, view: js_sys::JsString,
    ) -> Result<f64, JsError> {
        self.submit_inner(values, method, args, view, true)
    }
}
impl EditWorker {
    fn submit_inner(
        &self, values: js_sys::Float64Array, method: js_sys::JsString,
        args: js_sys::JsString, view: js_sys::JsString, project_fields: bool,
    ) -> Result<f64, JsError> {
        let (stamp, epochs) = identity(&values, 7)?;
        if method.length() > 128
            || (method.length() as usize)
                .saturating_add(args.length() as usize)
                .saturating_add(view.length() as usize)
                > MAX_SESSION_CONTROL_BYTES / 3
        {
            return Err(js_error("editing call exceeds its control budget"));
        }
        let (method, args, view) = (text(&method)?, text(&args)?, text(&view)?);
        if project_fields { self.handle.submit_project_fields(stamp, epochs, &method, &args, &view) }
        else { self.handle.submit(stamp, epochs, &method, &args, &view) }
            .map(|request| request as f64).map_err(js_error)
    }
}
#[wasm_bindgen]
impl EditWorker {
    /// Scalar header only. Packet JSON is taken separately so no wrapping or
    /// escaping can inflate the <=64KiB payload. An unavailable mailbox is
    /// undefined, never mistaken for an idle/ready Session.
    pub fn poll(&mut self) -> Result<Option<String>, JsError> {
        if self.incoming.is_some() {
            return Err(js_error(
                "the editing packet must be taken before polling again",
            ));
        }
        let poll = self.handle.poll();
        if poll.phase == Phase::Unavailable {
            return Ok(None);
        }
        let mut fields = vec![format!(
            "\"request\":{},\"phase\":\"{}\"",
            poll.request,
            phase(poll.phase)
        )];
        if let Some(packet) = poll.packet {
            fields.push(format!("\"packetSequence\":{}", packet.sequence));
            self.incoming = Some(packet);
        }
        if let Some(success) = poll.success {
            fields.push(format!(
                "\"stamp\":[{},{},{},{}],\"epochs\":[{},{},{}]",
                success.stamp.generation,
                success.stamp.lineage,
                success.stamp.revision,
                success.stamp.sequence,
                success.epochs.shown,
                success.epochs.view,
                success.epochs.gesture
            ));
        }
        if let Some(error) = poll.error {
            let message = error.to_string();
            if message.len() > 4096 {
                return Err(js_error("the editing refusal exceeds its control budget"));
            }
            fields.push(format!(
                "\"error\":{}",
                styk_doc::json::to_string(&styk_doc::Value::str(&message))
            ));
        }
        Ok(Some(format!("{{{}}}", fields.join(","))))
    }
    pub fn packet(&mut self) -> Option<String> {
        self.incoming.take().map(|packet| packet.json)
    }
    pub fn ack_packet(&self, request: f64, sequence: u32) -> Result<bool, JsError> {
        if self.incoming.is_some() {
            return Err(js_error(
                "an editing packet was acknowledged before being taken",
            ));
        }
        self.handle
            .ack_packet(counter(request)?, sequence)
            .map_err(js_error)
    }
    pub fn activate(&self, request: f64, values: js_sys::Float64Array) -> Result<bool, JsError> {
        self.handle
            .activate(counter(request)?, epochs(&values)?)
            .map_err(js_error)
    }
    pub fn ready(&self, request: f64, values: js_sys::Float64Array) -> Result<bool, JsError> {
        self.handle
            .ready(counter(request)?, epochs(&values)?)
            .map_err(js_error)
    }
    pub fn terminal(&self, request: f64) -> Result<bool, JsError> {
        self.handle.terminal(counter(request)?).map_err(js_error)
    }
    pub fn cancel(&self, request: f64) -> Result<(), JsError> {
        self.handle.cancel(counter(request)?);
        Ok(())
    }
    pub fn close(&self) {
        self.handle.close();
    }
}
