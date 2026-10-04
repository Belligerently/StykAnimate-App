//! `projectNativePropertyChannel`: worker-only materialization through the same
//! native pose marker/source and typed Records property/composition owners.
use super::{channel_id, easing, items, normalize_target, own_channel};
use crate::{EditError, Editor};
use styk_doc::{Obj, Value};

impl Editor {
    /// Read-only projection. The caller owns worker readiness and forwards its
    /// existing cancellation check; no draft/cache/index/callback state is kept.
    pub(crate) fn native_property_channel_checked(
        &self,
        target: &Value,
        property: &str,
        check: &mut dyn FnMut() -> Result<(), EditError>,
    ) -> Result<Value, EditError> {
        check()?;
        if self.sheet()
            || target.get("kind").and_then(Value::as_str) != Some("figure")
            || !["x", "y", "scale", "rotation"].contains(&property)
        {
            return Err(EditError::Unported(
                "unrecorded native property projection target".into(),
            ));
        }
        let target = normalize_target(target)?;
        let id = channel_id(&target, property)?;
        if let Some(channel) = own_channel(self, &id) {
            return Ok(channel);
        }
        // Layer-pose inheritance is a separately recorded owner boundary.
        if items(self.report.path(&["_styk", "animation", "channels"]))
            .iter()
            .any(|channel| {
                channel.path(&["target", "kind"]).and_then(Value::as_str) == Some("layer")
                    && channel.get("property").and_then(Value::as_str) == Some("pose")
                    && channel.get("keys").and_then(Value::array_len).unwrap_or(0) > 0
            })
        {
            return Err(EditError::Unported(
                "native scalar projection from an inherited layer pose".into(),
            ));
        }
        let uid = target.get("uid").and_then(Value::as_f64).unwrap() as u32;
        let pose = self.native_baseline_pose(uid)?;
        let native = pose.get("source").and_then(Value::as_str) == Some("native");
        let raw_frames = if native {
            items(self.report.get("keyframes"))
        } else {
            Vec::new()
        };
        let dependency = items(self.report.path(&["_styk", "animation", "channels"]))
            .iter()
            .any(|channel| {
                channel.get("property").and_then(Value::as_str) == Some(property)
                    && channel.path(&["target", "kind"]).and_then(Value::as_str) == Some("figure")
                    && (["x", "y"].contains(&property)
                        || channel.path(&["target", "uid"]) == target.get("uid"))
            });
        self.with_records(|records| {
            let mut keys = Vec::new();
            let mut cursor = 0;
            for key in items(pose.get("keys")) {
                check()?;
                if key.get("hidden").is_some_and(Value::truthy) {
                    continue;
                }
                let Some(source_id) = key.path(&["value", "frameId"]).and_then(Value::as_str)
                else {
                    continue;
                };
                let hint = if native {
                    while cursor < raw_frames.len()
                        && raw_frames[cursor]
                            .path(&["_styk", "frameId"])
                            .and_then(Value::as_str)
                            != Some(source_id)
                    {
                        check()?;
                        cursor += 1;
                    }
                    (cursor < raw_frames.len()).then(|| {
                        let index = cursor;
                        cursor += 1;
                        index
                    })
                } else {
                    None
                };
                let Some(source) = records.pose_source_frame(source_id, hint) else {
                    continue;
                };
                let Some(base) = records.property_value(&source, &target, property)? else {
                    continue;
                };
                let object = items(source.get("objects"))
                    .into_iter()
                    .find(|object| object.get("unique_id") == target.get("uid"));
                let needs_rig = ["x", "y", "rotation"].contains(&property)
                    && object.as_ref().is_some_and(|object| {
                        object.get("join").is_some_and(Value::truthy)
                            || object.get("_stykIK").is_some_and(Value::truthy)
                    });
                let tick = key
                    .get("tick")
                    .and_then(Value::as_f64)
                    .ok_or_else(|| EditError::Unported("a native marker without a tick".into()))?;
                let value = if needs_rig || dependency {
                    check()?;
                    let frame = records.evaluated_editor_with(tick, true)?;
                    records
                        .property_value(&frame, &target, property)?
                        .unwrap_or(Value::Null)
                } else {
                    base
                };
                let curve = key.get("interpolation").is_none()
                    && self.report.get("auto_easing").is_some_and(Value::truthy);
                let mut timing = key.as_obj().cloned().unwrap();
                timing.set(
                    "interpolation",
                    Value::str(if curve {
                        "linear"
                    } else {
                        key.get("interpolation")
                            .and_then(Value::as_str)
                            .unwrap_or("linear")
                    }),
                );
                let (interpolation, bezier) = easing(&Value::object(timing))?;
                let mut out = Obj::new();
                let marker_id = key.get("id").and_then(Value::as_str).ok_or_else(|| {
                    EditError::Unported("a native marker without an identity".into())
                })?;
                out.set("id", Value::str(&format!("native:{id}:{marker_id}")));
                out.set("tick", Value::Num(tick));
                out.set("value", value);
                out.set("interpolation", Value::str(&interpolation));
                if let Some(bezier) = bezier {
                    out.set("bezier", bezier);
                }
                if curve {
                    out.set("nativeCurve", Value::str("pchip"));
                }
                if property == "rotation" {
                    out.set("nativeRotation", Value::Bool(true));
                }
                // Map.set(tick,key) replaces at the original position.
                let value = Value::object(out);
                if let Some(index) = keys
                    .iter()
                    .position(|prior: &Value| prior.get("tick") == value.get("tick"))
                {
                    keys[index] = value;
                } else {
                    keys.push(value);
                }
            }
            check()?;
            let mut channel = Obj::new();
            channel.set("id", Value::str(&id));
            channel.set("target", target);
            channel.set("property", Value::str(property));
            channel.set("keys", Value::array(keys));
            Ok(Value::object(channel))
        })
    }
}
