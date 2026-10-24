//! Combine connected figures (src/editor-combine.mjs): a figure and everything joined to it become one native figure in the
//! keyframe at the playhead, drawn as they were drawn (curves, circles, fills, colours, paint order) with the root's identity.
//! One `figure-splice` history entry holds the new definition and the keyframe before and after.
//!
//! Ported for figures made of lines, circles and fills, as the editor combines them. Groups with media (images, text), rig
//! relationships, Stick Nodes shapes, surfaces, colour transitions, pinned limbs, 3D meshes, movieclips or origins, and a
//! document with saved animation channels (`authored`), answer `Unported`: the editor combines those.

use std::collections::{HashMap, HashSet};

use styk_doc::{Obj, Value};

use crate::figures::{frame_order, normalize_frame, set_order};
use crate::history::{Command, CommandKind};
use crate::record::{num, uid};
use crate::sheet::items;
use crate::{EditError, Editor};

fn invalid(key: &str) -> EditError {
    EditError::Invalid(key.into())
}

fn unported(what: &str) -> EditError {
    EditError::Unported(format!("combining figures {what}"))
}

/// `value ?? fallback`: neither absent nor `null`.
fn present(value: Option<&Value>) -> Option<&Value> {
    value.filter(|v| !v.is_null())
}

fn truthy(value: &Value, name: &str) -> bool {
    value.get(name).is_some_and(Value::truthy)
}

/// A handle as a key: signed zeros alike, as `Map` has them.
fn key(id: f64) -> u64 {
    if id == 0.0 { 0 } else { id.to_bits() }
}

/// `alphaByte(alpha)`: the transparency byte an opacity is, when it is exactly one.
fn alpha_byte(alpha: f64) -> Result<f64, EditError> {
    let byte = crate::range_plan::js_round((1.0 - alpha) * 255.0);
    if !(0.0..=255.0).contains(&byte) || (1.0 - byte / 255.0 - alpha).abs() > 1e-10 {
        return Err(invalid("errors.editorCombine.thisOpacityCannotBeRepresentedExactlyInA"));
    }
    Ok(byte)
}

/// `joinedGroup(editor, uid)`: the slot of the figure and of everything joined to it, breadth first.
fn joined_group(objects: &[Value], id: u32) -> Result<Vec<usize>, EditError> {
    let Some(root) = objects.iter().position(|o| uid(o) == Some(id)) else { return Err(invalid("errors.editorCombine.selectAFigureWithAttachedChildren")) };
    let mut children: HashMap<u64, Vec<usize>> = HashMap::new();
    for (slot, object) in objects.iter().enumerate() {
        if let Some(parent) = object.get("join").filter(|j| j.truthy()).and_then(|j| num(j, "target_object_slot")) {
            children.entry(key(parent)).or_default().push(slot);
        }
    }
    let mut queue = vec![root];
    let mut taken: HashSet<usize> = HashSet::from([root]);
    let mut at = 0;
    while at < queue.len() {
        for &child in children.get(&key(queue[at] as f64)).map(Vec::as_slice).unwrap_or(&[]) {
            if taken.insert(child) {
                queue.push(child);
            }
        }
        at += 1;
    }
    Ok(queue)
}

/// What a segment of a figure is drawn with in a pose (`segmentAt` of src/pivot/geometry.mjs, without a rig or a pinned limb):
/// its angle, bend and length (scaled).
fn drawn_pose(object: &Value, segment: &Value, index: usize, scale: f64) -> (f64, f64, f64) {
    let held = |name: &str| present(object.get(name).and_then(|v| v.item(index)).as_ref()).and_then(Value::as_f64);
    let runtime = |name: &str| present(object.get("runtime_segments").and_then(|r| r.item(index)).and_then(|r| r.get(name).cloned()).as_ref()).and_then(Value::as_f64);
    let own = |name: &str| present(segment.get(name)).and_then(Value::as_f64);
    let angle = held("angles").or_else(|| runtime("angle_rad")).or_else(|| own("angle_rad")).unwrap_or(0.0);
    let bend = if truthy(object, "bend_flag") { held("bends").unwrap_or(0.0) } else { runtime("bend_rad").or_else(|| own("bend_rad")).unwrap_or(0.0) };
    let length = if truthy(object, "len_flag") { held("lengths").or_else(|| own("length")).unwrap_or(f64::NAN) } else { runtime("length").or_else(|| own("length")).unwrap_or(0.0) };
    (angle, bend, length * scale)
}

/// `1 - transparency / 255`, else the stored opacity: `seg.transparency === undefined ? (seg.color_alpha ?? 1) : 1 - seg.transparency / 255`.
fn alpha_of(record: &Value, transparency: &str, opacity: &str) -> f64 {
    match record.get(transparency) {
        None => present(record.get(opacity)).and_then(Value::as_f64).unwrap_or(1.0),
        Some(Value::Null) => 1.0,
        Some(value) => 1.0 - value.as_f64().unwrap_or(f64::NAN) / 255.0,
    }
}

/// The colour a segment or fill shows (`objectColor`; a format 4 figure shows the object's native colour for its segments).
fn shown_rgb(version: f64, figure: &Value, object: &Value, base: Option<&Value>, segment: bool) -> [f64; 3] {
    let raw = crate::recolor::override_raw(object);
    if segment && version < 5.0 && figure.path(&["_styk", "colorMode"]).and_then(Value::as_str) != Some("segments") {
        return if crate::recolor::to_int32(raw) < 0 { [0.0; 3] } else { crate::recolor::native_rgb(raw) };
    }
    crate::recolor::mix_color(styk_eval::records::paint::rgb_of_value(present(base)).unwrap_or([0.0; 3]), raw)
}

fn rgb_value(rgb: [f64; 3]) -> Value {
    styk_eval::records::paint::rgb_value(rgb)
}

/// One drawable of a figure in the draw list: where its record is and where it paints.
struct Drawable {
    z: f64,
    polygon: bool,
    index: usize,
}

impl Editor {
    /// `combineJoinedFigure(uid, {name})` on the keyframe at the playhead.
    pub(crate) fn combine_joined_figure(&mut self, requested: Option<&Value>, options: Option<&Value>) -> Result<Value, EditError> {
        let id = match present(requested) {
            None => self.selection.uid.ok_or_else(|| invalid("errors.editorCombine.selectAFigureWithAttachedChildren"))?,
            Some(Value::Num(n)) if n.fract() == 0.0 && *n >= 0.0 && *n <= f64::from(u32::MAX) => *n as u32,
            Some(_) => return Err(unported("of an identity that is not a number")),
        };
        let name = match options.and_then(|o| present(o.get("name"))) {
            None => None,
            Some(Value::Str(_)) => options.and_then(|o| o.get("name")).and_then(Value::as_str).map(str::to_string),
            Some(_) => return Err(unported("with a name that is not text")),
        };
        if options.is_some_and(|o| truthy(o, "authored")) {
            return Err(unported("inside an authored transaction"));
        }
        let index = self.frame_index;
        let frame = self.report.get("keyframes").and_then(|k| k.item(index)).ok_or_else(|| unported("without a keyframe"))?;
        let objects = items(frame.get("objects"));
        let slots = joined_group(&objects, id)?;
        let definitions = items(self.report.get("figure_types"));
        let figure_of = |object: &Value| definitions.iter().find(|f| num(f, "figure_type_id") == num(object, "figure_type_id")).cloned().unwrap_or(Value::Null);
        let segments_of = |figure: &Value| items(figure.get("segments"));
        if slots.iter().any(|&slot| segments_of(&figure_of(&objects[slot])).iter().any(|s| s.path(&["_stykRig", "spatial"]).is_some_and(Value::truthy))) {
            return Err(invalid("errors.editorCombine.keepSpatialRigsJoinedToPreserveTheirSkin"));
        }
        if self.drag.is_some() {
            self.commit_drag()?;
        }
        if self.report.path(&["_styk", "animation", "channels"]).and_then(Value::array_len).unwrap_or(0) > 0 {
            return Err(unported("of a document with saved animation channels"));
        }
        if slots.len() < 2 {
            return Err(invalid("errors.editorCombine.joinAtLeastTwoFiguresBeforeCombining"));
        }
        let members: HashSet<u32> = slots.iter().filter_map(|&slot| uid(&objects[slot])).collect();
        if members.iter().any(|&member| self.layer_locked(member)) {
            return Err(invalid("errors.editorCombine.unlockEveryConnectedFigureBeforeCombining"));
        }
        let order = frame_order(frame.as_obj().ok_or_else(|| unported("without a keyframe"))?);
        let places: Vec<usize> = order.iter().enumerate().filter(|(_, member)| members.contains(&(**member as u32))).map(|(i, _)| i).collect();
        let first = places.first().copied().unwrap_or(0);
        if places.last().copied().unwrap_or(0) + 1 - first != places.len() {
            return Err(invalid("errors.editorCombine.moveTheJoinedFiguresToAdjacentLayersBefore"));
        }
        let (mut segment_total, mut polygon_total) = (0usize, 0usize);
        for &slot in &slots {
            let (object, figure) = (&objects[slot], figure_of(&objects[slot]));
            // What the editor combines with a rig, media, Stick Nodes shapes, surfaces, colour transitions or pinned limbs is its own.
            if segments_of(&figure).iter().any(|s| truthy(s, "_stykRig") || truthy(s, "_sticknodes") || matches!(s.get("segment_type").and_then(Value::as_str), Some("image" | "text")))
                || items(figure.get("polyfills")).iter().any(|p| truthy(p, "_stykSurface"))
                || ["spatialMesh", "movieclip", "drawing", "animationOrigin"].iter().any(|k| figure.path(&["_styk", k]).is_some_and(Value::truthy))
                || ["_sticknodesShapes", "_sticknodesSprite", "color_transition", "_stykAnimationOrigin"].iter().any(|k| truthy(object, k))
                || crate::ik::has_pinned(object)
            {
                return Err(unported("with media, a rig, Stick Nodes shapes, surfaces, colour transitions, pinned limbs or an origin"));
            }
            if truthy(object, "_stykVfx") {
                return Err(invalid("errors.editorCombine.combineCannotPreserveSeparateEffectStacksConvertThe"));
            }
            if present(object.get("field28")).or_else(|| present(object.get("transparency"))).and_then(Value::as_f64).unwrap_or(0.0) != 0.0 {
                return Err(invalid("errors.editorCombine.resetFigureOpacityTo100BeforeCombiningPer"));
            }
            let style = figure.path(&["segment_decode", "global_style"]);
            let outline = match style.and_then(|s| present(s.get("outline_thickness"))).and_then(Value::as_f64) {
                Some(thickness) => thickness,
                None => f64::from(f32::from_bits(style.and_then(|s| present(s.get("u28"))).and_then(Value::as_f64).unwrap_or(0.0) as u32)),
            };
            if outline > 0.0 {
                return Err(invalid("errors.editorCombine.removeFigureOutlinesBeforeCombiningSeparateOutlinePasses"));
            }
            segment_total += segments_of(&figure).len();
            polygon_total += items(figure.get("polyfills")).len();
        }
        if segment_total > 30000 || polygon_total > 20000 {
            return Err(invalid("errors.editorCombine.theCombinedFigureExceedsTheSupported30000"));
        }
        // The figures' world positions are the root's: only the root figure's origin is kept.
        let root_point = {
            let record = frame.clone();
            let (_, _, scene) = self.posed_scene(&record)?;
            scene.objects.iter().find(|g| Some(g.uid) == uid(&objects[slots[0]]) && g.resolved).map(|g| g.root).ok_or_else(|| unported("of a figure the engine does not draw"))?
        };

        let version = num(&self.report, "version").unwrap_or(f64::NAN);
        let type_id = definitions.iter().filter_map(|f| num(f, "figure_type_id")).fold(0.0f64, f64::max) + 1.0;
        let title = name.unwrap_or_else(|| format!("{} combined", figure_of(&objects[slots[0]]).get("name").filter(|n| n.truthy()).and_then(Value::as_str).unwrap_or("Figure")));

        // The new handles, figure by figure; a figure's origin is the joint of its parent it hangs from.
        let mut handles: HashMap<u32, HashMap<u64, f64>> = HashMap::new();
        let mut next_handle = 1.0;
        for &slot in &slots {
            let mut map = HashMap::new();
            for segment in segments_of(&figure_of(&objects[slot])) {
                map.insert(key(num(&segment, "segment_id").unwrap_or(f64::NAN)), next_handle);
                next_handle += 1.0;
            }
            handles.insert(uid(&objects[slot]).unwrap_or(0), map);
        }
        for &slot in &slots {
            let object = &objects[slot];
            let member = uid(object).unwrap_or(0);
            let origin = if member == id {
                0.0
            } else {
                let join = object.get("join").cloned().unwrap_or(Value::Null);
                let parent = num(&join, "target_object_slot").and_then(|s| objects.get(s as usize)).and_then(uid).unwrap_or(0);
                handles.get(&parent).and_then(|m| m.get(&key(num(&join, "target_handle_id").unwrap_or(f64::NAN)))).copied().unwrap_or(f64::NAN)
            };
            handles.get_mut(&member).expect("every member has handles").insert(0, origin);
        }
        let mapped = |member: u32, handle: Option<&Value>| handles.get(&member).and_then(|m| m.get(&key(handle.and_then(Value::as_f64).unwrap_or(f64::NAN)))).map_or(Value::Null, |h| Value::Num(*h));

        let mut segments: Vec<Value> = Vec::with_capacity(segment_total);
        let mut polygons: Vec<Value> = Vec::with_capacity(polygon_total);
        // Where each segment and fill of each member went, and how it paints.
        let mut drawn: HashMap<usize, Vec<Drawable>> = HashMap::new();
        for &slot in &slots {
            let object = &objects[slot];
            let member = uid(object).unwrap_or(0);
            let figure = figure_of(object);
            let scale = present(object.get("scale")).and_then(Value::as_f64).unwrap_or(1.0);
            let raw = crate::recolor::to_uint32(crate::recolor::override_raw(object));
            let mut list = Vec::new();
            for (i, original) in segments_of(&figure).iter().enumerate() {
                let (angle, bend, length) = drawn_pose(object, original, i, scale);
                let thickness = present(original.get("thickness")).and_then(Value::as_f64).unwrap_or(0.0) * scale.abs();
                let rgb = shown_rgb(version, &figure, object, original.get("color_rgb"), true);
                let alpha = alpha_of(original, "transparency", "color_alpha");
                let mut segment = original.as_obj().cloned().unwrap_or_default();
                segment.remove("_logical_segment_id");
                segment.remove("z_order_runtime_raw");
                segment.set("segment_id", mapped(member, original.get("segment_id")));
                segment.set("parent_id", mapped(member, original.get("parent_id")));
                segment.set("length", Value::Num(length));
                segment.set("thickness", Value::Num(thickness));
                segment.set("angle_rad", Value::Num(angle));
                segment.set("bend_rad", Value::Num(bend));
                segment.set("color_rgb", rgb_value(rgb));
                segment.set("transparency", Value::Num(alpha_byte(alpha)?));
                segment.set("color_alpha", Value::Num(alpha));
                segment.set("z_order", Value::Num(0.0));
                if segment.get("secondary").is_some_and(Value::truthy) {
                    if raw >> 24 == 255 {
                        segment.set("secondary", Value::Null);
                    } else if raw >> 24 != 0
                        && let Some(mut secondary) = segment.get("secondary").and_then(Value::as_obj).cloned()
                    {
                        if let Some(base) = styk_eval::records::paint::rgb_of_value(secondary.get("rgb")) {
                            secondary.set("rgb", rgb_value(crate::recolor::mix_color(base, f64::from(raw))));
                        }
                        segment.set("secondary", Value::object(secondary));
                    }
                }
                if !matches!(original.get("visible"), Some(Value::Bool(false))) {
                    list.push(Drawable { z: num(original, "z_order").unwrap_or((i + 1) as f64), polygon: false, index: segments.len() });
                }
                segments.push(Value::object(segment));
            }
            let mut fills = Vec::new();
            for original in items(figure.get("polyfills")) {
                let rgb = shown_rgb(version, &figure, object, original.get("color_rgb"), false);
                let alpha = alpha_of(&original, "alpha_raw", "color_alpha");
                let mut polygon = original.as_obj().cloned().unwrap_or_default();
                polygon.set("polyfill_id", Value::Num((polygons.len() + 1) as f64));
                let ids: Vec<Value> = items(original.get("segment_ids")).iter().map(|handle| mapped(member, Some(handle))).collect();
                polygon.set("segment_ids", Value::array(ids.clone()));
                polygon.set("segment_ids_raw", Value::array(ids));
                polygon.set("color_rgb", rgb_value(rgb));
                polygon.set("alpha_raw", Value::Num(alpha_byte(alpha)?));
                polygon.set("color_alpha", Value::Num(alpha));
                polygon.set("z_order", Value::Num(0.0));
                polygon.remove("z_order_runtime_raw");
                fills.push(Drawable { z: num(&original, "z_order").unwrap_or(0.0), polygon: true, index: polygons.len() });
                polygons.push(Value::object(polygon));
            }
            // Fills draw first, then the visible segments; each by its place (a stable sort).
            fills.extend(list);
            fills.sort_by(|a, b| a.z.partial_cmp(&b.z).unwrap_or(std::cmp::Ordering::Equal));
            drawn.insert(slot, fills);
        }
        // Paint order: the drawables of the members, figure by figure in the drawing order of the keyframe.
        let slot_of_uid = |member: f64| objects.iter().position(|o| num(o, "unique_id") == Some(member));
        let mut rank = 1.0;
        for &member in &order {
            let Some(slot) = slot_of_uid(member).filter(|slot| slots.contains(slot)) else { continue };
            for drawable in drawn.get(&slot).map(Vec::as_slice).unwrap_or(&[]) {
                let list = if drawable.polygon { &mut polygons } else { &mut segments };
                if let Some(record) = list.get_mut(drawable.index).and_then(Value::make_obj) {
                    record.set("z_order", Value::Num(rank));
                    record.set("z_order_runtime_raw", Value::Num(rank - 1.0));
                }
                rank += 1.0;
            }
        }

        let angles: Vec<f64> = segments.iter().map(|s| num(s, "angle_rad").unwrap_or(f64::NAN)).collect();
        let mut figure = Obj::new();
        figure.set("figure_type_id", Value::Num(type_id));
        figure.set("name", Value::str(&title.chars().scan(0usize, |units, c| { *units += c.len_utf16(); (*units <= 255).then_some(c) }).collect::<String>()));
        figure.set("segments", Value::array(segments));
        figure.set("polyfills", Value::array(polygons));
        figure.set("image_assets", Value::array(Vec::new()));
        figure.set("text_assets", Value::array(Vec::new()));
        figure.set("segment_count", Value::Num(segment_total as f64));
        let mut styk = Obj::new();
        styk.set("colorMode", Value::str("segments"));
        figure.set("_styk", Value::object(styk));
        let figure = Value::object(figure);
        crate::topology::validate_definition(&figure)?;

        // The keyframe with the group replaced by the one figure, at the group's place in the drawing order; every join follows by identity.
        let mut combined = objects[slots[0]].as_obj().cloned().unwrap_or_default();
        combined.set("scale", Value::Num(1.0));
        combined.set("figure_type_id", Value::Num(type_id));
        combined.set("root_x", Value::Num(root_point[0]));
        combined.set("root_y", Value::Num(root_point[1]));
        combined.set("color_override_raw", Value::Num(0.0));
        combined.set("field28", Value::Num(0.0));
        combined.set("image_text_flags", Value::Num(0.0));
        combined.set("angles", Value::nums(&angles));
        combined.set("len_flag", Value::Num(0.0));
        combined.set("lengths", Value::array(Vec::new()));
        combined.set("bend_flag", Value::Num(0.0));
        combined.set("bends", Value::array(Vec::new()));
        combined.remove("runtime_segments");
        let by_identity = |record: &mut Obj| {
            if let Some(mut join) = record.get("join").filter(|j| j.truthy()).and_then(Value::as_obj).cloned() {
                let target = num(&Value::object(join.clone()), "target_object_slot").and_then(|s| objects.get(s as usize)).and_then(|o| o.get("unique_id").cloned());
                join.set("target_unique_id", target.unwrap_or(Value::Null));
                record.set("join", Value::object(join));
            }
        };
        let mut kept: Vec<Obj> = Vec::with_capacity(objects.len());
        for object in &objects {
            let mut record = object.as_obj().cloned().unwrap_or_default();
            by_identity(&mut record);
            if !uid(object).is_some_and(|member| members.contains(&member)) {
                kept.push(record);
            }
        }
        by_identity(&mut combined);
        kept.push(combined);
        let slot_by_uid: HashMap<u32, usize> = kept.iter().enumerate().filter_map(|(i, o)| o.get("unique_id").and_then(Value::as_f64).map(|u| (u as u32, i))).collect();
        let renumbered: Vec<Value> = kept
            .into_iter()
            .enumerate()
            .map(|(i, mut record)| {
                record.set("object_slot", Value::Num(i as f64));
                if let Some(mut join) = record.get("join").filter(|j| j.truthy()).and_then(Value::as_obj).cloned() {
                    let target = join.get("target_unique_id").and_then(Value::as_f64).and_then(|u| slot_by_uid.get(&(u as u32)));
                    join.set("target_object_slot", target.map_or(Value::Null, |slot| Value::Num(*slot as f64)));
                    record.set("join", Value::object(join));
                }
                Value::object(record)
            })
            .collect();
        let mut next_order: Vec<f64> = order.iter().copied().filter(|member| !members.contains(&(*member as u32))).collect();
        next_order.insert(first.min(next_order.len()), f64::from(id));
        let mut after_frame = frame.as_obj().cloned().unwrap_or_default();
        after_frame.set("objects", Value::array(renumbered));
        set_order(&mut after_frame, &next_order);
        normalize_frame(&mut after_frame);
        let after_frame = Value::object(after_frame);

        self.ensure_counts();
        let before_selection = self.selection_state();
        let figure_index = definitions.len();
        if let Some(list) = self.report.make_obj().and_then(|r| r.get_mut("figure_types")).and_then(Value::make_arr) {
            list.push(figure.clone());
        }
        if let Some(frames) = self.report.make_obj().and_then(|r| r.get_mut("keyframes")).and_then(Value::make_arr)
            && let Some(slot) = frames.get_mut(index)
        {
            *slot = after_frame.clone();
        }
        self.select_ids(vec![id]);
        let layers = self.track_frames(std::slice::from_ref(&frame), std::slice::from_ref(&after_frame), true)?;
        let side = |figures: Vec<Value>, frame: Value| {
            let mut side = Obj::new();
            side.set("figures", Value::array(figures));
            side.set("frame", frame);
            Value::object(side)
        };
        self.retained = None;
        self.record(Command {
            kind: CommandKind::FigureSplice { index, figure_index, before: side(Vec::new(), frame), after: side(vec![figure], after_frame) },
            label: "Combine joined figures".into(),
            before_selection,
            after_selection: self.selection_state(),
            before_revision: 0,
            after_revision: 0,
            layer_changes: Vec::new(),
            captured_source_bytes: 0,
            layers,
        })?;
        self.emit("edit", vec![("label", Value::str("Combine joined figures")), ("structural", Value::Bool(true)), ("definition", Value::Bool(true))]);
        Ok(self.object()?.map(|(_, o)| o).unwrap_or(Value::Null))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Value {
        styk_doc::json::parse(text.as_bytes()).unwrap()
    }

    #[test]
    fn the_group_is_the_figure_and_everything_joined_below_it_breadth_first() {
        // 0 root; 1 and 2 hang from it; 3 from 1; 4 hangs from nothing.
        let objects: Vec<Value> = [
            r#"{"unique_id":10}"#,
            r#"{"unique_id":11,"join":{"target_object_slot":0}}"#,
            r#"{"unique_id":12,"join":{"target_object_slot":0}}"#,
            r#"{"unique_id":13,"join":{"target_object_slot":1}}"#,
            r#"{"unique_id":14}"#,
        ].iter().map(|text| parse(text)).collect();
        assert_eq!(joined_group(&objects, 10).unwrap(), vec![0, 1, 2, 3]);
        assert_eq!(joined_group(&objects, 11).unwrap(), vec![1, 3]);
        assert_eq!(joined_group(&objects, 14).unwrap(), vec![4]);
        assert!(matches!(joined_group(&objects, 99), Err(EditError::Invalid(key)) if key == "errors.editorCombine.selectAFigureWithAttachedChildren"));
    }

    #[test]
    fn an_opacity_is_a_transparency_byte_only_when_it_is_exactly_one() {
        assert_eq!(alpha_byte(1.0).unwrap(), 0.0);
        assert_eq!(alpha_byte(1.0 - 128.0 / 255.0).unwrap(), 128.0);
        assert_eq!(alpha_byte(0.0).unwrap(), 255.0);
        // Half is not a byte of 255.
        assert!(alpha_byte(0.5).is_err());
        assert!(alpha_byte(f64::NAN).is_err());
    }

    #[test]
    fn a_segment_is_drawn_with_its_pose_then_its_runtime_values_then_its_definition() {
        let segment = parse(r#"{"angle_rad":0.25,"bend_rad":0.5,"length":40}"#);
        let plain = parse(r#"{"angles":[1.5],"len_flag":0,"bend_flag":0}"#);
        assert_eq!(drawn_pose(&plain, &segment, 0, 2.0), (1.5, 0.5, 80.0));
        let stretched = parse(r#"{"angles":[null],"len_flag":1,"lengths":[10],"bend_flag":1,"bends":[0.75]}"#);
        assert_eq!(drawn_pose(&stretched, &segment, 0, 0.5), (0.25, 0.75, 5.0));
        let runtime = parse(r#"{"runtime_segments":[{"angle_rad":2,"bend_rad":3,"length":4}]}"#);
        assert_eq!(drawn_pose(&runtime, &segment, 0, 1.0), (2.0, 3.0, 4.0));
    }

    #[test]
    fn opacity_is_read_from_the_transparency_byte_before_the_stored_opacity() {
        assert_eq!(alpha_of(&parse(r#"{"transparency":51}"#), "transparency", "color_alpha"), 0.8);
        assert_eq!(alpha_of(&parse(r#"{"color_alpha":0.25}"#), "transparency", "color_alpha"), 0.25);
        assert_eq!(alpha_of(&parse("{}"), "transparency", "color_alpha"), 1.0);
        assert_eq!(alpha_of(&parse(r#"{"transparency":null}"#), "transparency", "color_alpha"), 1.0);
    }
}
