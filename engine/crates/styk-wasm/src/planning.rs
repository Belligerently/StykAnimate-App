//! Small request/poll boundary for worker-owned planning (layer moves).
//! No document input: the engine plans against the document its mirror holds.
use crate::engine::EngineHandle;
use styk_doc::{Value, json};
use styk_engine::planning::{LayerJob, LayerRequest, PlanError, TRANSFER_BYTES};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct LayerPlanJob {
    job: LayerJob,
}

/// A planning error as the host tells them apart by name: `AbortError` for a job that was
/// cancelled, replaced by a newer document or timed out; `EngineUnported` for a plan the
/// engine does not make yet (the host plans it itself); an ordinary error, whose message is
/// the editor's message key, for a refusal.
fn plan_error(error: PlanError) -> JsValue {
    let name = match &error {
        PlanError::Cancelled | PlanError::Stale | PlanError::TimedOut => "AbortError",
        PlanError::Unported(_) => "EngineUnported",
        PlanError::Invalid(_) => "Error",
    };
    let failure = js_sys::Error::new(&error.to_string());
    failure.set_name(name);
    failure.into()
}
fn bad_request(message: &str) -> JsValue {
    js_sys::Error::new(message).into()
}
fn revision_of(revision: f64, request_json: &str) -> Result<u64, JsValue> {
    if !revision.is_finite()
        || revision < 0.0
        || revision.fract() != 0.0
        || revision > 9_007_199_254_740_991.0
        || request_json.len() > 64 * 1024
    {
        return Err(bad_request("invalid planning request budget/revision"));
    }
    Ok(revision as u64)
}
fn identity_of(request: &Value) -> Result<u64, JsValue> {
    request
        .get("nextFrameIdentity")
        .and_then(Value::as_i64)
        .filter(|id| *id > 0 && *id <= 9_007_199_254_740_991)
        .map(|id| id as u64)
        .ok_or_else(|| bad_request("invalid planning frame identity"))
}

#[wasm_bindgen]
impl EngineHandle {
    /// Only IDs enter here. Hosts must use real engine worker threads (D7).
    pub fn plan_layers(&self, revision: f64, request_json: &str) -> Result<LayerPlanJob, JsValue> {
        let revision = revision_of(revision, request_json)?;
        let request = json::parse(request_json.as_bytes()).map_err(|error| bad_request(&error.to_string()))?;
        let uids = request
            .get("uids")
            .and_then(Value::elements)
            .ok_or_else(|| bad_request("layer planning requires figure IDs"))?;
        if uids.len() > 10_000 {
            return Err(bad_request("layer planning figure ID budget exceeded"));
        }
        let uids = uids
            .iter()
            .map(|uid| {
                uid.as_i64()
                    .filter(|uid| *uid > 0 && *uid <= u32::MAX as i64)
                    .map(|uid| uid as u32)
                    .ok_or_else(|| bad_request("invalid layer planning figure ID"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let target_layer = request
            .get("targetLayerId")
            .and_then(Value::as_str)
            .ok_or_else(|| bad_request("layer planning requires destination"))?
            .to_string();
        let deleted_layer = request
            .get("deletedLayerId")
            .filter(|value| !matches!(value, Value::Null))
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_string)
                    .ok_or_else(|| bad_request("invalid deleted layer ID"))
            })
            .transpose()?;
        let request = LayerRequest {
            uids,
            target_layer,
            deleted_layer,
            next_frame_identity: identity_of(&request)?,
            max_source_bytes: TRANSFER_BYTES,
        };
        self.engine
            .plan_layers(revision, request)
            .map(|job| LayerPlanJob { job })
            .map_err(plan_error)
    }
}
#[wasm_bindgen]
impl LayerPlanJob {
    /// One packet at most; no waiting and no complete payload serialization.
    pub fn take(&self) -> Result<Option<String>, JsValue> {
        let result = self.job.poll();
        if let Some(error) = result.error {
            return Err(plan_error(error));
        }
        if result.progress.is_none() && result.chunk.is_none() && !result.done {
            return Ok(None);
        }
        let mut fields = Vec::new();
        if let Some(p) = result.progress {
            fields.push(if p.phase == "sampling" {
                format!("\"progress\":{{\"phase\":\"sampling\",\"completed\":{},\"total\":{},\"sourceFrames\":{},\"objectPoses\":{},\"segmentValues\":{}}}",p.completed,p.total,p.source_frames,p.object_poses,p.segment_values)
            } else {
                format!("\"progress\":{{\"phase\":\"planning\",\"completed\":{},\"total\":{}}}",p.completed,p.total)
            });
        }
        if let Some(chunk) = result.chunk {
            fields.push(format!(
                "\"sequence\":{},\"chunk\":{}",
                chunk.sequence, chunk.json
            ));
        }
        if result.done {
            fields.push("\"done\":true".into());
        }
        Ok(Some(format!("{{{}}}", fields.join(","))))
    }
    pub fn ack(&self, sequence: u32) -> Result<bool, JsValue> {
        self.job.ack(sequence).map_err(plan_error)
    }
    pub fn cancel(&self) {
        self.job.cancel();
    }
}
