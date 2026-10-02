//! The evaluated frame as the editor's records.
//!
//! A gesture commits by storing the frame the editor shows: `snapshotPoseFrame(
//! editor.evaluatedFrame)` becomes a pose source in the document. To run that commit
//! in the engine, the engine must produce the same record, byte for byte: the same
//! keys in the same order and every number with the same bits.
//!
//! This module mirrors the record-level shape of the editor's evaluation
//! (`sampleLayerSheet` in src/layer-sheet.mjs, `evaluatedNativeFrame` and
//! `evaluatedFrame` in src/pivot/geometry.mjs, `snapshotPoseFrame` in
//! src/pose-snapshot.mjs): which record a pose comes from, which keys a tween writes
//! and in which order. Every interpolated number comes from the typed evaluator
//! ([`crate::eval`]), so there is one implementation of the arithmetic.
//!
//! What it does not cover yet answers [`Error::Unported`] with the reason.
//! `engine/tools/records-reference.mjs` records the editor's output and
//! `styk records` compares.

use std::collections::HashMap;
use std::sync::Arc;

use styk_doc::{Obj, Value};

use crate::eval::{Prepared, Row, Seq, evaluate_native, timeline_rows};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The engine does not produce this frame as records yet.
    #[error("not ported: {0}")]
    Unported(String),
    #[error("{0}")]
    Invalid(String),
}

fn unported<T>(what: &str) -> Result<T, Error> {
    Err(Error::Unported(what.to_string()))
}

/// A sampled frame and what it is, for the identity tests of `sampleLayerSheet`
/// (JavaScript compares frames with `===`).
#[derive(Clone)]
struct Sample {
    frame: Value,
    identity: Identity,
}

/// What one layer takes from a sampled frame: its figures (each with the unique id
/// it joins, if any) in drawing order, and its movieclip continuations.
#[derive(Default)]
struct Group {
    records: Vec<(Value, Option<f64>)>,
    steps: Vec<Value>,
}

#[derive(Clone, PartialEq)]
enum Identity {
    /// `sourceAt(time)`: the imported timeline at a time (bits of the time).
    Native(u64),
    /// A retained pose source, by id.
    Source(Arc<str>),
    /// A frame of a layer's own pose track: never the same object as another track's.
    Track(String),
}

fn num(value: &Value, key: &str) -> Option<f64> {
    value.get(key).and_then(Value::as_f64)
}

fn items(value: Option<&Value>) -> Vec<Value> {
    value.and_then(Value::elements).map(|e| e.iter().collect()).unwrap_or_default()
}

fn truthy(value: Option<&Value>) -> bool {
    value.is_some_and(Value::truthy)
}

fn uid(object: &Value) -> Option<f64> {
    num(object, "unique_id")
}

fn numbers(values: impl IntoIterator<Item = f64>) -> Value {
    Value::array(values.into_iter().map(Value::Num).collect())
}

/// Evaluates frames of one document as records.
pub struct Records<'a> {
    tree: &'a Value,
    prepared: &'a Prepared,
    /// `timeline(legacySheetReport(report))`: the native rows without the sheet's cut.
    rows: Vec<Row>,
}

impl<'a> Records<'a> {
    /// `tree` is the document `prepared` was built from.
    pub fn new(tree: &'a Value, prepared: &'a Prepared) -> Result<Records<'a>, Error> {
        let styk = tree.get("_styk");
        let clips = |path: &[&str]| items(styk.and_then(|s| s.path(path)));
        if items(tree.get("figure_types")).iter().any(|f| truthy(f.path(&["_styk", "animationOrigin"]))) {
            return unported("animation origins");
        }
        if styk.and_then(|s| s.get("movieclipSource")).is_some() {
            return unported("a movieclip's own document");
        }
        let legacy_channels = match styk.and_then(|s| s.get("layerSheet")) {
            Some(sheet) => sheet.path(&["legacyAnimation", "channels"]),
            None => styk.and_then(|s| s.path(&["animation", "channels"])),
        };
        if legacy_channels.and_then(Value::array_len).unwrap_or(0) > 0 {
            return unported("property channels");
        }
        let doc = &prepared.doc;
        let mut rows = timeline_rows(doc.version, doc.keyframes.iter().map(|k| k.duration));
        // The last row lasts as long as the camera and effect clips do.
        let ends = clips(&["cameraMotion", "clips"])
            .iter()
            .map(|c| num(c, "start").unwrap_or(0.0) + num(c, "duration").unwrap_or(0.0))
            .chain(clips(&["vfxClips", "clips"]).iter().map(|c| match c.get("duration") {
                Some(Value::Null) => 0.0,
                _ => num(c, "start").unwrap_or(0.0) + num(c, "duration").unwrap_or(0.0),
            }))
            .fold(0.0, f64::max);
        if let Some(last) = rows.last_mut() {
            last.count = last.count.max((ends - last.start as f64).max(0.0) as u32);
        }
        Ok(Records { tree, prepared, rows })
    }

    /// `snapshotPoseFrame(editor.evaluatedFrame)` at `tick`.
    pub fn snapshot(&self, tick: f64) -> Result<Value, Error> {
        Ok(snapshot_pose_frame(&self.evaluated(tick)?))
    }

    /// `evaluatedFrame(report, …, tick)` without the runtime clock fields.
    pub fn evaluated(&self, tick: f64) -> Result<Value, Error> {
        let frame = match self.tree.path(&["_styk", "layerSheet"]) {
            Some(sheet) => self.sample_sheet(sheet, tick)?,
            None => self.native_at(tick)?,
        };
        let frame = self.sample_clips(frame, tick)?;
        Ok(self.layer_order(frame))
    }

    /// `sampleMovieclipObjects(report, frame, tick)`: movieclips outside their span
    /// leave the frame. (The sample time each shown one gets, `_movieclipSample`, is a
    /// runtime field a snapshot drops.)
    fn sample_clips(&self, frame: Value, tick: f64) -> Result<Value, Error> {
        if !self.prepared.doc.figures.iter().any(|f| f.movieclip.is_some()) {
            return Ok(frame);
        }
        // Which instances are hidden: the typed evaluator's clocks, by identity.
        let typed = self.prepared.raw(tick);
        let hidden: Vec<u32> = typed.objects.iter().filter(|o| self.prepared.clip_local(o, tick) == Some(None)).map(|o| o.uid).collect();
        if hidden.is_empty() {
            return Ok(frame);
        }
        let objects = items(frame.get("objects"));
        let shown = |o: &Value| !uid(o).is_some_and(|id| hidden.contains(&(id as u32)));
        let mut slots: HashMap<usize, usize> = HashMap::new();
        for (slot, object) in objects.iter().enumerate() {
            if shown(object) {
                slots.insert(slot, slots.len());
            }
        }
        let mut visible = Vec::with_capacity(slots.len());
        for (slot, object) in objects.iter().enumerate() {
            let Some(&at) = slots.get(&slot) else { continue };
            let mut next = object.as_obj().cloned().unwrap_or_default();
            next.set("object_slot", Value::Num(at as f64));
            if let Some(join) = object.get("join").filter(|j| j.truthy()) {
                let target = num(join, "target_object_slot").map(|s| s as usize);
                let Some(&parent) = target.and_then(|t| slots.get(&t)) else { return unported("figures joined to a hidden movieclip") };
                let mut join = join.as_obj().cloned().unwrap_or_default();
                join.set("target_object_slot", Value::Num(parent as f64));
                next.set("join", Value::object(join));
            }
            visible.push(Value::object(next));
        }
        let order: Vec<usize> = match frame.get("object_draw_order").filter(|o| !o.is_null()) {
            Some(order) => order.numbers().unwrap_or_default().into_iter().map(|s| s as usize).collect(),
            None => (0..objects.len()).collect(),
        };
        let order: Vec<usize> = order.iter().filter_map(|slot| slots.get(slot).copied()).collect();
        let ids: Vec<Value> = order.iter().map(|&slot| visible[slot].get("unique_id").cloned().unwrap_or(Value::Null)).collect();
        let mut out = frame.as_obj().cloned().unwrap_or_default();
        let count = visible.len();
        out.set("objects", Value::array(visible));
        out.set("object_count", Value::Num(count as f64));
        out.set("object_draw_order", numbers(order.iter().map(|&s| s as f64)));
        out.set("object_draw_order_unique_ids", Value::array(ids));
        Ok(Value::object(out))
    }

    /// `sampleLegacy(report, time)`: the native timeline at a time, in layer order.
    fn native_at(&self, time: f64) -> Result<Value, Error> {
        let index = self.rows.partition_point(|r| r.start as f64 <= time).saturating_sub(1);
        let t = match self.rows.get(index) {
            Some(row) if row.count > 0 => (time - row.start as f64) / row.count as f64,
            _ => 0.0,
        };
        Ok(self.layer_order(self.native(index, t)?))
    }

    /// `evaluatedNativeFrame(report, index, t)` on the document's own keyframes.
    fn native(&self, index: usize, t: f64) -> Result<Value, Error> {
        let frames = items(self.tree.get("keyframes"));
        self.interval(&self.prepared.native, &|i| Ok(frames.get(i).cloned()), index, t)
    }

    /// `evaluatedNativeFrame` on a timeline: the document's keyframes, or a pose
    /// track's virtual ones. `frame_at` gives the record of each key of `seq`.
    fn interval(&self, seq: &Seq, frame_at: &dyn Fn(usize) -> Result<Option<Value>, Error>, index: usize, t: f64) -> Result<Value, Error> {
        let (Some(a), Some(key)) = (frame_at(index)?, seq.keys.get(index)) else { return Err(Error::Invalid(format!("no key {index}"))) };
        let Some(b) = frame_at(index + 1)? else { return Ok(a) };
        if t <= 0.0 || (seq.version < 5 && key.easing.is_none()) {
            return Ok(a);
        }
        if t >= 1.0 {
            return Ok(b);
        }
        let auto = key.easing.map(|e| e.mode) == Some(crate::doc::EasingMode::Auto);
        let eased = if auto { t } else { crate::eval::eased_progress(key.easing.as_ref(), t) };
        if eased <= 0.0 {
            return Ok(a);
        }
        if eased >= 1.0 {
            return Ok(b);
        }
        // The numbers: the typed evaluator's tween of the same interval.
        let typed = evaluate_native(&self.prepared.doc, seq, index, t);
        let before = &key.frame.objects;
        let next: HashMap<u64, Value> = items(b.get("objects")).into_iter().filter_map(|o| Some((uid(&o)?.to_bits(), o))).collect();
        let mut objects = Vec::new();
        for (slot, o) in items(a.get("objects")).into_iter().enumerate() {
            let tweened = typed.objects.get(slot).zip(before.get(slot)).filter(|(out, source)| !Arc::ptr_eq(out, source)).map(|(out, _)| out);
            let Some(out) = tweened else {
                objects.push(o);
                continue;
            };
            let other = uid(&o).and_then(|id| next.get(&id.to_bits())).ok_or_else(|| Error::Invalid("a tweened figure has no next pose".into()))?;
            objects.push(tween_record(&o, other, out)?);
        }
        let mut frame = a.as_obj().cloned().unwrap_or_default();
        frame.set("objects", Value::array(objects));
        if truthy(a.get("camera")) && truthy(b.get("camera")) {
            let parallax = |f: &Value| truthy(f.path(&["camera", "_parallaxMotionOffset"]));
            if parallax(&a) || parallax(&b) {
                return unported("camera parallax offsets");
            }
            let camera = typed.camera.unwrap_or_default();
            let mut record = Obj::new();
            record.set("is_default", Value::Bool(false));
            record.set("x", Value::Num(camera.x));
            record.set("y", Value::Num(camera.y));
            record.set("scale", Value::Num(camera.scale));
            record.set("angle_rad", Value::Num(camera.angle));
            frame.set("camera", Value::object(record));
        }
        frame.set("background_id", Value::Num(typed.background as f64));
        frame.set("background_id_b", Value::Num(typed.background_b as f64));
        frame.set("background_mix", Value::Num(typed.background_mix));
        if truthy(a.get("_stykVfx")) {
            return unported("scene effects between keys");
        }
        Ok(Value::object(frame))
    }

    /// One frame of `buildVirtualPoseTrack(report, channel)`: the key's pose source
    /// with the key's easing and the time to the next key.
    fn virtual_frame(&self, keys: &[Value], index: usize, fixed: bool) -> Result<Option<Value>, Error> {
        let Some(key) = keys.get(index) else { return Ok(None) };
        let frame_id = key.path(&["value", "frameId"]);
        let blank = matches!(key.get("value"), Some(Value::Null)) || matches!(frame_id, Some(Value::Null));
        let source = match (blank, frame_id.and_then(Value::as_str)) {
            (true, _) => None,
            (false, Some(id)) => Some(self.source(id).ok_or_else(|| Error::Invalid(format!("the pose source {id} is missing")))?),
            (false, None) => return Err(Error::Invalid("a pose key names no source".into())),
        };
        let mut frame = match &source {
            Some(source) => source.as_obj().cloned().unwrap_or_default(),
            None => {
                // `blankPoseFrame(report.keyframes[0])`.
                let mut blank = self.tree.get("keyframes").and_then(|k| k.item(0)).and_then(|f| f.as_obj().cloned()).unwrap_or_default();
                for key in ["background_color", "_stykBackgroundColor", "_stykVfx"] {
                    blank.remove(key);
                }
                empty_objects(&mut blank);
                for key in ["background_id", "background_id_b", "background_mix"] {
                    blank.set(key, Value::Num(0.0));
                }
                blank.set("camera", default_camera());
                blank
            }
        };
        let source_styk = source.as_ref().and_then(|s| s.get("_styk"));
        let mut styk = source_styk.and_then(Value::as_obj).cloned().unwrap_or_default();
        let easing = match key.get("interpolation") {
            None => source_styk.and_then(|s| s.get("easing")).cloned(),
            Some(interpolation) => {
                // `keyEasing(key)`: `hold` is read as `constant`.
                let mode = match interpolation.as_str() {
                    Some("hold") => "constant",
                    Some(mode) => mode,
                    None => "linear",
                };
                let mut easing = Obj::new();
                easing.set("interpolation", Value::str(mode));
                if mode == "bezier" {
                    easing.set("bezier", key.get("bezier").cloned().unwrap_or_else(|| numbers([0.42, 0.0, 0.58, 1.0])));
                }
                Some(Value::object(easing))
            }
        };
        match easing {
            Some(easing) => styk.set("easing", easing),
            None => {
                styk.remove("easing");
            }
        }
        frame.set("_styk", Value::object(styk));
        let tick = |k: &Value| num(k, "tick").unwrap_or(0.0);
        let duration = if truthy(key.get("hidden")) { 0.0 } else { keys.get(index + 1).map_or(1.0, |next| (tick(next) - tick(key)).max(0.0)) };
        frame.set("duration_raw", Value::Num(duration));
        if fixed {
            empty_objects(&mut frame);
        }
        Ok(Some(Value::object(frame)))
    }

    /// `samplePoseTrack(report, channel, tick)` for a layer's own keys.
    fn pose_track(&self, id: &str, keys: &[Value], tick: f64) -> Result<Option<Sample>, Error> {
        let fixed = id == "scene" || id == "camera";
        let Some(seq) = self.prepared.track(id) else { return Err(Error::Invalid(format!("no pose track {id}"))) };
        if keys.iter().any(|k| truthy(k.get("hidden"))) {
            return unported("hidden pose keys");
        }
        let ticks: Vec<f64> = keys.iter().map(|k| num(k, "tick").unwrap_or(0.0)).collect();
        if ticks.is_empty() || tick < ticks[0] {
            return Ok(None);
        }
        let low = ticks.partition_point(|&t| t <= tick) - 1;
        let fraction = match ticks.get(low + 1) {
            Some(&next) if next > ticks[low] => ((tick - ticks[low]) / (next - ticks[low])).clamp(0.0, 1.0),
            _ => 0.0,
        };
        let frame = self.interval(seq, &|i| self.virtual_frame(keys, i, fixed), low, fraction)?;
        Ok(Some(Sample { frame, identity: Identity::Track(id.to_string()) }))
    }

    /// `layerIdForUid(report, uid)`.
    fn layer_of(&self, uid: f64) -> String {
        let Some(state) = self.tree.path(&["_styk", "layers"]) else { return "layer-1".into() };
        let key = styk_doc::json::to_string(&Value::Num(uid));
        let assigned = state.get("assignments").and_then(|a| a.get(&key));
        assigned.or_else(|| state.get("defaultLayerId")).and_then(Value::as_str).unwrap_or("").to_string()
    }

    /// `applyLayerOrder(report, frame)`: figures drawn layer by layer; slots stay.
    fn layer_order(&self, frame: Value) -> Value {
        let state = self.tree.path(&["_styk", "layers"]);
        let objects = items(frame.get("objects"));
        let depth = |o: &Value| o.get("_stykZIndex").and_then(Value::as_f64).unwrap_or(0.0);
        let has_depth = objects.iter().any(|o| truthy(o.get("_stykZIndex")));
        let layers: Vec<String> = match state {
            Some(state) => items(state.get("order")).iter().filter_map(|id| id.as_str().map(str::to_string)).collect(),
            None => vec!["layer-1".into()],
        };
        if (state.is_none() || layers.len() < 2) && !has_depth {
            return frame;
        }
        let order: Vec<usize> = match frame.get("object_draw_order").filter(|o| !o.is_null()) {
            Some(order) => order.numbers().unwrap_or_default().into_iter().map(|s| s as usize).collect(),
            None => (0..objects.len()).collect(),
        };
        let mut groups: Vec<Vec<usize>> = vec![Vec::new(); layers.len()];
        for &slot in &order {
            let Some(object) = objects.get(slot) else { return frame };
            let id = self.layer_of(uid(object).unwrap_or(f64::NAN));
            let Some(group) = layers.iter().position(|layer| *layer == id) else { return frame };
            groups[group].push(slot);
        }
        let mut next = Vec::with_capacity(order.len());
        for mut group in groups {
            if has_depth {
                group.sort_by(|&a, &b| depth(&objects[a]).total_cmp(&depth(&objects[b])));
            }
            next.extend(group);
        }
        if next == order {
            return frame;
        }
        let mut out = frame.as_obj().cloned().unwrap_or_default();
        out.set("object_draw_order", numbers(next.iter().map(|&s| s as f64)));
        out.set("object_draw_order_unique_ids", Value::array(next.iter().map(|&s| objects[s].get("unique_id").cloned().unwrap_or(Value::Null)).collect()));
        Value::object(out)
    }

    /// `poseSourceFrame(report, id)`.
    fn source(&self, id: &str) -> Option<Value> {
        let retained = self.tree.path(&["_styk", "frameSources"]).and_then(|sources| sources.get(id)).filter(|frame| frame.truthy());
        // Else the native keyframe that carries the id.
        retained.cloned().or_else(|| items(self.tree.get("keyframes")).into_iter().find(|frame| frame.path(&["_styk", "frameId"]).and_then(Value::as_str) == Some(id)))
    }

    /// `baseSample(id, track)`: a layer's pose from its keys.
    fn base_sample(&self, id: &str, track: &Value, tick: f64) -> Result<Option<Sample>, Error> {
        let fixed = id == "scene" || id == "camera";
        let keys: Vec<Value> = items(track.get("keys")).into_iter().filter(|k| !truthy(k.get("patchOnly"))).collect();
        if !fixed && tick >= num(track, "end").unwrap_or(0.0) {
            return Ok(None);
        }
        let at = keys.partition_point(|k| num(k, "tick").unwrap_or(0.0) <= tick);
        let Some(key) = at.checked_sub(1).map(|i| &keys[i]) else { return Ok(None) };
        let next = keys.get(at);
        let frame_id = |k: &Value| k.path(&["value", "frameId"]).cloned();
        let source_tick = |k: &Value| num(k, "sourceTick");
        let native = |time: f64| -> Result<Option<Sample>, Error> { Ok(Some(Sample { frame: self.native_at(time)?, identity: Identity::Native(time.to_bits()) })) };
        let held = |k: &Value| -> Result<Option<Sample>, Error> {
            match (source_tick(k), frame_id(k)) {
                (Some(time), _) => native(time),
                (None, Some(Value::Str(id))) => Ok(self.source(&id).map(|frame| Sample { frame, identity: Identity::Source(id.clone()) })),
                _ => Ok(None),
            }
        };
        if matches!(frame_id(key), Some(Value::Null)) {
            return Ok(None);
        }
        if next.is_some_and(|n| matches!(frame_id(n), Some(Value::Null))) {
            return held(key);
        }
        if let Some(start) = source_tick(key)
            && next.is_none_or(|n| source_tick(n).is_some())
        {
            let (fraction, end) = match next {
                Some(n) => {
                    let (a, b) = (num(key, "tick").unwrap_or(0.0), num(n, "tick").unwrap_or(0.0));
                    (((tick - a) / (b - a)).clamp(0.0, 1.0), source_tick(n).unwrap_or(start))
                }
                None => (0.0, start),
            };
            return native(start + fraction * (end - start));
        }
        if keys.iter().any(|k| source_tick(k).is_some()) {
            return unported("layers that mix imported and authored keys");
        }
        self.pose_track(id, &keys, tick)
    }

    /// `sample(id)`: the base pose, then the movieclip frames its key replaces.
    fn sample(&self, sheet: &Value, id: &str, tick: f64) -> Result<Option<Sample>, Error> {
        let Some(track) = sheet.path(&["tracks", id]) else { return Ok(None) };
        let keys = items(track.get("keys"));
        let at = keys.partition_point(|k| num(k, "tick").unwrap_or(0.0) <= tick);
        let patched = at.checked_sub(1).is_some_and(|i| keys[i].get("movieclipPatches").and_then(Value::array_len).unwrap_or(0) > 0);
        if patched && tick < num(track, "end").unwrap_or(0.0) {
            return unported("movieclip frame replacements");
        }
        self.base_sample(id, track, tick)
    }

    /// `sampleLayerSheet(report, tick)`.
    fn sample_sheet(&self, sheet: &Value, tick: f64) -> Result<Value, Error> {
        let layers: Vec<String> = items(self.tree.path(&["_styk", "layers", "order"])).iter().filter_map(|id| id.as_str().map(str::to_string)).collect();
        let scene = self.sample(sheet, "scene", tick)?;
        let camera = self.sample(sheet, "camera", tick)?;
        let mut frames: Vec<(String, Option<Sample>)> = Vec::new();
        for id in &layers {
            frames.push((id.clone(), self.sample(sheet, id, tick)?));
        }
        let continuations = |frame: &Value| items(frame.path(&["_styk", "movieclipContinuations"]));
        let frame_of = |layer: &str| frames.iter().find(|(id, _)| id == layer).and_then(|(_, sample)| sample.as_ref());
        // Imported layers that share one native pose are that pose, whole.
        if let (Some(scene), Some(camera)) = (&scene, &camera)
            && camera.identity == scene.identity
        {
            let same = |sample: Option<&Sample>| sample.is_some_and(|s| s.identity == scene.identity);
            let objects = items(scene.frame.get("objects"));
            let steps = continuations(&scene.frame);
            if objects.iter().all(|o| same(frame_of(&self.layer_of(uid(o).unwrap_or(f64::NAN)))))
                && steps.iter().all(|step| same(frame_of(&self.layer_of(num(step, "uid").unwrap_or(f64::NAN)))))
                && frames.iter().all(|(_, sample)| match sample {
                    None => true,
                    Some(s) => s.identity == scene.identity || (items(s.frame.get("objects")).is_empty() && continuations(&s.frame).is_empty()),
                })
            {
                return Ok(scene.frame.clone());
            }
        }
        // Otherwise each layer contributes its own figures from its own sample.
        let mut records: Vec<(Value, Option<f64>)> = Vec::new();
        let mut steps: Vec<Value> = Vec::new();
        let mut seen: Vec<&Sample> = Vec::new();
        let mut partitions: Vec<HashMap<String, Group>> = Vec::new();
        for (id, sample) in &frames {
            let Some(sample) = sample else { continue };
            let at = match seen.iter().position(|s| s.identity == sample.identity) {
                Some(at) => at,
                None => {
                    let objects = items(sample.frame.get("objects"));
                    let order: Vec<usize> = match sample.frame.get("object_draw_order").filter(|o| !o.is_null()) {
                        Some(order) => order.numbers().unwrap_or_default().into_iter().map(|s| s as usize).collect(),
                        None => (0..objects.len()).collect(),
                    };
                    let mut groups: HashMap<String, Group> = HashMap::new();
                    for object in order.iter().filter_map(|&slot| objects.get(slot)) {
                        let parent = object.get("join").filter(|j| j.truthy()).map(|join| {
                            let slot = num(join, "target_object_slot").unwrap_or(f64::NAN);
                            objects.get(slot as usize).filter(|_| slot >= 0.0).and_then(uid)
                        });
                        let owner = self.layer_of(uid(object).unwrap_or(f64::NAN));
                        groups.entry(owner).or_default().records.push((object.clone(), parent.flatten()));
                    }
                    for step in continuations(&sample.frame) {
                        groups.entry(self.layer_of(num(&step, "uid").unwrap_or(f64::NAN))).or_default().steps.push(step);
                    }
                    seen.push(sample);
                    partitions.push(groups);
                    seen.len() - 1
                }
            };
            if let Some(group) = partitions[at].get(id) {
                records.extend(group.records.iter().cloned());
                steps.extend(group.steps.iter().cloned());
            }
        }
        let slots: HashMap<u64, usize> = records.iter().enumerate().filter_map(|(slot, (object, _))| Some((uid(object)?.to_bits(), slot))).collect();
        let objects: Vec<Value> = records
            .iter()
            .enumerate()
            .map(|(slot, (object, parent))| {
                let join = object.get("join").filter(|j| j.truthy());
                let parent_slot = parent.and_then(|p| slots.get(&p.to_bits()).copied());
                let joined = join.is_some() && parent_slot.is_some();
                let slot_matches = num(object, "object_slot") == Some(slot as f64);
                let join_matches = join.is_none() || (joined && join.and_then(|j| num(j, "target_object_slot")) == parent_slot.map(|s| s as f64));
                if slot_matches && join_matches {
                    return object.clone();
                }
                let mut out = object.as_obj().cloned().unwrap_or_default();
                out.set("object_slot", Value::Num(slot as f64));
                out.set(
                    "join",
                    match (joined, join) {
                        (true, Some(join)) => {
                            let mut join = join.as_obj().cloned().unwrap_or_default();
                            join.set("target_object_slot", Value::Num(parent_slot.unwrap() as f64));
                            join.set("target_unique_id", Value::Num(parent.unwrap()));
                            Value::object(join)
                        }
                        _ => Value::Null,
                    },
                );
                Value::object(out)
            })
            .collect();
        let template = self.tree.get("keyframes").and_then(|k| k.item(0)).unwrap_or(Value::Null);
        let mut result = template.as_obj().cloned().unwrap_or_default();
        let count = objects.len();
        let ids: Vec<Value> = objects.iter().map(|o| o.get("unique_id").cloned().unwrap_or(Value::Null)).collect();
        result.set("objects", Value::array(objects));
        result.set("object_count", Value::Num(count as f64));
        result.set("object_draw_order", numbers((0..count).map(|i| i as f64)));
        result.set("object_draw_order_unique_ids", Value::array(ids));
        result.set(
            "camera",
            camera.as_ref().and_then(|c| c.frame.get("camera").filter(|v| !v.is_null()).cloned()).unwrap_or_else(default_camera),
        );
        // `setContinuations`: `_styk` is copied, with or without the list.
        let mut styk = result.get("_styk").and_then(Value::as_obj).cloned().unwrap_or_default();
        if steps.is_empty() {
            styk.remove("movieclipContinuations");
        } else {
            styk.set("movieclipContinuations", Value::array(steps));
        }
        result.set("_styk", Value::object(styk));
        for field in ["background_id", "background_id_b", "background_mix", "background_color", "_stykBackgroundColor", "_stykVfx"] {
            match scene.as_ref().map(|s| s.frame.get(field)) {
                Some(Some(value)) => result.set(field, value.clone()),
                Some(None) => {
                    result.remove(field);
                }
                // No scene pose: `{background_id: 0, background_id_b: 0, background_mix: 0}`.
                None if field.starts_with("background_") && field != "background_color" => result.set(field, Value::Num(0.0)),
                None => {
                    result.remove(field);
                }
            }
        }
        Ok(Value::object(result))
    }
}

/// `{is_default: true, x: 0, y: 0, scale: 1, angle_rad: 0}`.
fn default_camera() -> Value {
    let mut camera = Obj::new();
    camera.set("is_default", Value::Bool(true));
    camera.set("x", Value::Num(0.0));
    camera.set("y", Value::Num(0.0));
    camera.set("scale", Value::Num(1.0));
    camera.set("angle_rad", Value::Num(0.0));
    Value::object(camera)
}

/// `objects: [], object_count: 0, object_draw_order: [], object_draw_order_unique_ids: []`.
fn empty_objects(frame: &mut Obj) {
    frame.set("objects", Value::array(Vec::new()));
    frame.set("object_count", Value::Num(0.0));
    frame.set("object_draw_order", Value::array(Vec::new()));
    frame.set("object_draw_order_unique_ids", Value::array(Vec::new()));
}

/// The record of a tweened figure: `out` in `evaluatedNativeFrame`, with the keys
/// in the order the editor writes them and the typed evaluator's values.
fn tween_record(o: &Value, next: &Value, out: &crate::doc::Object) -> Result<Value, Error> {
    for (key, what) in [("_stykSpatial", "3D poses between keys"), ("_stykVfx", "figure effects between keys"), ("_stykIK", "inverse kinematics between keys")] {
        if truthy(o.get(key)) || (key == "_stykSpatial" && truthy(next.get(key))) {
            return unported(what);
        }
    }
    let mut record = o.as_obj().cloned().unwrap_or_default();
    record.remove("runtime_segments");
    record.set("root_x", Value::Num(out.root[0]));
    record.set("root_y", Value::Num(out.root[1]));
    record.set("scale", Value::Num(out.scale));
    record.set("field28", Value::Num(out.transparency));
    if let Some(lengths) = &out.lengths {
        record.set("len_flag", Value::Num(1.0));
        record.set("lengths", numbers(lengths.iter().copied()));
    }
    if let Some(bends) = &out.bends {
        record.set("bend_flag", Value::Num(1.0));
        record.set("bends", numbers(bends.iter().copied()));
    }
    record.set("angles", numbers(out.angles.iter().copied()));
    record.set("color_override_raw", Value::Num(out.color as f64));
    Ok(Value::object(record))
}

/// `snapshotPoseFrame(frame)` for every figure of the frame.
pub fn snapshot_pose_frame(frame: &Value) -> Value {
    let objects: Vec<Value> = items(frame.get("objects"))
        .iter()
        .enumerate()
        .map(|(slot, object)| {
            let mut out = object.as_obj().cloned().unwrap_or_default();
            out.remove("runtime_segments");
            out.remove("_movieclipSample");
            out.set("object_slot", Value::Num(slot as f64));
            Value::object(out)
        })
        .collect();
    let count = objects.len();
    let order: Vec<usize> = match frame.get("object_draw_order").filter(|o| !o.is_null()) {
        Some(order) => order.numbers().unwrap_or_default().into_iter().map(|s| s as usize).filter(|&s| s < count).collect(),
        None => (0..count).collect(),
    };
    let ids: Vec<Value> = order.iter().map(|&slot| objects[slot].get("unique_id").cloned().unwrap_or(Value::Null)).collect();
    let mut saved = frame.as_obj().cloned().unwrap_or_default();
    for key in ["_cameraMotionTick", "_vfxTimelineTick", "_vfxTimelineScene"] {
        saved.remove(key);
    }
    saved.set("objects", Value::array(objects));
    saved.set("object_count", Value::Num(count as f64));
    saved.set("object_draw_order", numbers(order.iter().map(|&s| s as f64)));
    saved.set("object_draw_order_unique_ids", Value::array(ids));
    Value::object(saved)
}
