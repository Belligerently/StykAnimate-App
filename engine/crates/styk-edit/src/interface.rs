//! What the interface reads of the frame on show (docs/engine/evaluation.md, "The interface's frame"):
//! the evaluated frame record (`editor.evaluatedFrame`), the document's own frame under a gesture
//! (the base a draft is composed on), the motion-path edit's display, the timeline rows
//! (`editor.timeline`) and the geometry of a frame (`editor.geometry`), each as JSON text.
//!
//! Geometry is sent as the groups that changed since the interface's last read: a group is rebuilt
//! when its figure's pose (or a figure joined to it) is another record (another node with other values: a
//! frame evaluated at another tick makes new records of the poses it holds), and every group when a figure
//! definition is another. The interface names the read it holds (`since`).

use std::sync::Arc;

use styk_doc::Value;

use crate::frame::objects_of;
use crate::{EditError, Editor};

/// What the interface holds of one kind of frame's geometry.
#[derive(Clone, Default)]
pub(crate) struct Cache {
    /// The read it holds; zero before the first.
    token: u64,
    /// The figure definitions its groups were built against (identities, and the document's version).
    definitions: Vec<usize>,
    /// The pose records its groups were built from, by slot.
    objects: Vec<Value>,
}

impl Editor {
    /// The frame the interface shows: the gesture in progress, else the document at the playhead
    /// (`kind` `frame`); the document's own frame under any gesture (`document`); the motion-path
    /// edit's display, `null` where it shows the document (`motion`).
    pub(crate) fn interface_frame(&self, kind: &str) -> Result<String, EditError> {
        let record = match kind {
            "frame" => self.shown_frame()?,
            "document" => self.document_frame()?,
            "motion" => match self.motion_display_frame_worker()? {
                Some(record) => record,
                None => return Ok("null".into()),
            },
            other => return Err(EditError::Invalid(format!("the interface has no frame '{other}'"))),
        };
        Ok(styk_doc::json::to_string(&record))
    }

    /// The record of the frame the stage draws: the motion-path edit's display where one is asked for and shown,
    /// else the frame on show.
    pub(crate) fn interface_record(&self, motion: bool) -> Result<Value, EditError> {
        let displayed = if motion { self.motion_display_frame_worker()? } else { None };
        let record = match displayed {
            Some(record) => record,
            None => self.shown_frame()?,
        };
        // The stage samples the effect clips and the camera's motion at the playhead.
        let mut shown = record.as_obj().cloned().unwrap_or_default();
        shown.set("tick", Value::Num(self.tick));
        if self.report.path(&["_styk", "cameraMotion", "clips"]).and_then(Value::array_len).unwrap_or(0) > 0 {
            shown.set("_cameraMotionTick", Value::Num(self.tick));
        }
        Ok(Value::object(shown))
    }

    /// The reads of the editor's own the interface makes by name (`animationValue`, `vfxEffects`, `segmentAnimationTarget`,
    /// `getIKSettings`), answered where the session holds the document: `None` for a name it does not answer, else the
    /// value (`None` inside for what the editor answers `undefined`). A document that is not on a layer sheet has property
    /// channels of its own that these do not read yet: `Unported`.
    pub(crate) fn interface_query(&self, name: &str, args: &Value) -> Option<Result<Option<Value>, EditError>> {
        let arg = |index: usize| args.item(index).filter(|value| !value.is_null());
        Some(match name {
            "getIKSettings" => self.get_ik_settings().map(Some),
            // A clipboard the session holds, as the interface hands it to another session (`@setClipboard`).
            "clipboard" => Ok(arg(0).and_then(|kind| kind.as_str().and_then(|kind| self.clipboard_wire(kind)))),
            "animationValue" | "vfxEffects" | "segmentAnimationTarget" if !self.sheet() => {
                Err(EditError::Unported("property values of a document without a layer sheet".into()))
            }
            "animationValue" => (|| {
                let property = arg(1).and_then(|property| property.as_str().map(str::to_string)).ok_or_else(|| EditError::Invalid("errors.animationChannels.chooseASupportedAnimationProperty".into()))?;
                self.animation_value(&arg(0).unwrap_or(Value::Null), &property, &arg(2).unwrap_or(Value::Num(self.tick)))
            })(),
            "vfxEffects" => self.vfx_effects(&arg(0).unwrap_or(Value::Null)).map(|effects| Some(Value::array(effects))),
            "segmentAnimationTarget" => {
                let mut target = styk_doc::Obj::new();
                target.set("kind", Value::str("segment"));
                target.set("uid", arg(0).unwrap_or(Value::Null));
                target.set("handleId", arg(1).unwrap_or(Value::Null));
                crate::property_edit::normalize_target(&Value::object(target)).map(Some)
            }
            _ => return None,
        })
    }

    /// `editor.timeline` as `[[start, count], …]`, one pair per row.
    pub(crate) fn interface_timeline(&self) -> String {
        let rows = crate::timeline::rows(&self.report);
        let mut out = Vec::with_capacity(rows.len() * 12 + 2);
        out.push(b'[');
        for (i, row) in rows.iter().enumerate() {
            if i > 0 {
                out.push(b',');
            }
            out.push(b'[');
            styk_doc::format_number(row.start, &mut out);
            out.push(b',');
            styk_doc::format_number(row.count, &mut out);
            out.push(b']');
        }
        out.push(b']');
        String::from_utf8(out).unwrap_or_default()
    }

    /// `editor.geometry` of the frame on show (or of the motion-path display with `motion`), the groups
    /// that changed since the read `since` named: `{token, full, count, slots: [[slot, group|null], …],
    /// warnings}`. `null` where a motion-path edit shows the document.
    pub(crate) fn interface_geometry(&self, args: &Value) -> Result<String, EditError> {
        let motion = args.get("motion").is_some_and(Value::truthy);
        let since = args.get("since").and_then(Value::as_f64).map(|token| token as u64);
        let record = if motion {
            match self.motion_display_frame_worker()? {
                Some(record) => record,
                None => return Ok("null".into()),
            }
        } else {
            self.shown_frame()?
        };
        let prepared = self.prepared()?;
        if let Some(feature) = prepared.doc.unsupported.first() {
            return Err(EditError::Unported(format!("geometry with {feature}")));
        }
        let mut frame = self.eval.borrow_mut().edit_frame(&record, &prepared).map_err(|error| EditError::Invalid(error.to_string()))?;
        let fields = frame.pose_vfx.is_some() || frame.objects.iter().any(|object| object.vfx.is_some() || object.vfx_connected.is_some());
        if fields || prepared.has_vfx {
            frame.vfx = styk_eval::vfx::frame::composed(&prepared.doc, &frame);
        }
        let objects = objects_of(&record);
        let count = frame.objects.len();
        let mut caches = self.shown.borrow_mut();
        let cache = &mut caches[usize::from(motion)];
        // Selecting another layer is a new document node but the same figures: their groups stay.
        let definitions: Vec<usize> = std::iter::once(prepared.doc.version as usize).chain(prepared.doc.figures.iter().map(|figure| Arc::as_ptr(figure) as usize)).collect();
        let reuse = since.is_some() && since == Some(cache.token) && cache.definitions == definitions && cache.objects.len() == objects.len() && count == objects.len();
        let mut dirty: Vec<bool> = if reuse { objects.iter().zip(&cache.objects).map(|(now, before)| !(now.same_node(before) || now == before)).collect() } else { vec![true; count] };
        if reuse && dirty.iter().any(|dirty| *dirty) {
            // A figure's pose moves the figures joined to it (and an assembly's inverse kinematics reaches across them).
            let mut root: Vec<usize> = (0..count).collect();
            for (i, object) in frame.objects.iter().enumerate() {
                if let Some(join) = object.join.filter(|join| (join.slot as usize) < count) {
                    root[i] = join.slot as usize;
                }
            }
            let top = |mut i: usize| {
                let mut steps = 0;
                while root[i] != i && steps <= count {
                    i = root[i];
                    steps += 1;
                }
                i
            };
            let mut component = vec![false; count];
            for i in 0..count {
                if dirty[i] {
                    component[top(i)] = true;
                }
            }
            for i in 0..count {
                dirty[i] = component[top(i)];
            }
        }
        let slots: Vec<usize> = (0..count).filter(|&i| dirty[i]).collect();
        let full = slots.len() == count;
        let groups = if slots.is_empty() {
            (Vec::new(), Vec::new())
        } else {
            let scene = styk_eval::geometry::build_shapes(&prepared.doc, &frame, (!full).then_some(&slots[..]));
            let bounds = styk_eval::drawlist::app_bounds(&scene, prepared.doc.as_ref());
            (styk_eval::geometry::write_groups(&scene, &slots, &bounds), scene.warnings)
        };
        cache.token += 1;
        cache.definitions = definitions;
        cache.objects = objects;
        let mut out = Vec::with_capacity(64 + groups.0.iter().map(|group| group.len() + 8).sum::<usize>());
        out.extend_from_slice(format!("{{\"token\":{},\"full\":{},\"count\":{},\"slots\":[", cache.token, full, count).as_bytes());
        for (n, (slot, group)) in slots.iter().zip(&groups.0).enumerate() {
            if n > 0 {
                out.push(b',');
            }
            out.extend_from_slice(format!("[{slot},").as_bytes());
            out.extend_from_slice(group.as_bytes());
            out.push(b']');
        }
        out.extend_from_slice(b"],\"warnings\":");
        styk_doc::json::write(&Value::array(groups.1.iter().map(|warning| Value::str(warning)).collect()), &mut out);
        out.push(b'}');
        String::from_utf8(out).map_err(|error| EditError::Invalid(error.to_string()))
    }
}
