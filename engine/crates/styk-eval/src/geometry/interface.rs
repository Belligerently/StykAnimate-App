//! The geometry the interface reads of a frame (docs/engine/evaluation.md, "The interface's geometry"):
//! per object its root, joints, every segment and polygon as built (drawn or not), the 3D frames of its
//! bones and what inverse kinematics solved, as JSON for `src/engine/engine-shown.mjs`, which makes
//! them the groups the canvas, the handles and the pickers read.
//!
//! Written from a scene built by [`super::build_shapes`]. Numbers are the exact values the scene holds
//! (non-finite ones are `null`).

use std::collections::HashMap;

use styk_doc::format_number;

use super::{Item, ItemKind, ITEM_CULLED, Scene};
use crate::doc::Figure;
use crate::rig::SpatialFrame;

fn num(out: &mut Vec<u8>, value: f64) {
    format_number(value, out);
}

fn point(out: &mut Vec<u8>, p: [f64; 2]) {
    out.push(b'[');
    num(out, p[0]);
    out.push(b',');
    num(out, p[1]);
    out.push(b']');
}

fn numbers(out: &mut Vec<u8>, values: &[f64]) {
    out.push(b'[');
    for (i, &value) in values.iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        num(out, value);
    }
    out.push(b']');
}

/// Points as `[x0,y0,x1,y1,…]`.
fn flat(out: &mut Vec<u8>, points: &[[f64; 2]]) {
    out.push(b'[');
    for (i, p) in points.iter().enumerate() {
        if i > 0 {
            out.push(b',');
        }
        num(out, p[0]);
        out.push(b',');
        num(out, p[1]);
    }
    out.push(b']');
}

fn key(out: &mut Vec<u8>, name: &str) {
    out.push(b',');
    out.push(b'"');
    out.extend_from_slice(name.as_bytes());
    out.extend_from_slice(b"\":");
}

fn rgb(out: &mut Vec<u8>, c: [u8; 3]) {
    out.extend_from_slice(format!("[{},{},{}]", c[0], c[1], c[2]).as_bytes());
}

fn spatial_frame(out: &mut Vec<u8>, frame: &SpatialFrame) {
    out.extend_from_slice(b"{\"q\":");
    numbers(out, &frame.q);
    key(out, "t");
    numbers(out, &frame.t);
    key(out, "dual");
    numbers(out, &frame.dual);
    key(out, "nativeBend");
    num(out, frame.native_bend);
    key(out, "authoredBend");
    num(out, frame.authored_bend);
    key(out, "motionValue");
    num(out, frame.motion_value);
    key(out, "origin");
    numbers(out, &frame.origin);
    key(out, "axis");
    numbers(out, &frame.axis);
    key(out, "stretch");
    num(out, frame.stretch);
    if let Some(scale) = frame.choice_scale {
        key(out, "choiceScale");
        num(out, scale);
    }
    if let Some(shape) = &frame.shape {
        key(out, "shape");
        out.extend_from_slice(b"{\"width\":");
        num(out, shape.width);
        key(out, "taper");
        num(out, shape.taper);
        out.push(b'}');
        key(out, "restLength");
        num(out, frame.rest_length);
    }
    out.push(b'}');
}

/// One segment of an object: `index` is its place in the figure.
fn segment(out: &mut Vec<u8>, scene: &Scene, slot: usize, fig: &Figure, item: &Item) {
    let object = &scene.objects[slot];
    let index = item.index as usize;
    out.extend_from_slice(b"{\"index\":");
    num(out, index as f64);
    key(out, "start");
    point(out, item.start);
    key(out, "end");
    point(out, item.end);
    key(out, "angle");
    num(out, item.angle);
    key(out, "bend");
    num(out, item.bend);
    key(out, "length");
    num(out, item.length);
    key(out, "thickness");
    num(out, item.thickness);
    key(out, "z");
    num(out, item.z);
    key(out, "rgb");
    rgb(out, item.rgb);
    key(out, "alpha");
    num(out, item.alpha);
    key(out, "opacity");
    num(out, item.opacity);
    let points = scene.points_of(item);
    if !points.is_empty() {
        key(out, "points");
        flat(out, points);
    }
    if let Some(joint) = object.rig.as_ref().and_then(|rig| rig.get(index)) {
        if joint.connector {
            key(out, "handle");
            point(out, object.ends[index]);
            key(out, "connector");
            out.extend_from_slice(b"true");
        }
    }
    if let Some(frame) = object.spatial.as_ref().and_then(|spatial| spatial.frames.get(index)).and_then(Option::as_ref) {
        key(out, "spatialFrame");
        spatial_frame(out, frame);
    }
    let _ = fig;
    out.push(b'}');
}

fn poly(out: &mut Vec<u8>, scene: &Scene, item: &Item) {
    out.extend_from_slice(b"{\"index\":");
    num(out, item.index as f64);
    key(out, "points");
    flat(out, scene.points_of(item));
    key(out, "z");
    num(out, item.z);
    key(out, "rgb");
    rgb(out, item.rgb);
    key(out, "alpha");
    num(out, item.alpha);
    key(out, "opacity");
    num(out, item.opacity);
    if item.flags & ITEM_CULLED != 0 {
        key(out, "culled");
        out.extend_from_slice(b"true");
    } else if item.depth.1 > item.depth.0 {
        key(out, "depthValues");
        numbers(out, scene.depths_of(item));
    }
    out.push(b'}');
}

/// The groups of `slots` of a scene built by [`super::build_shapes`], each as JSON text (`null` for
/// an object the scene could not resolve). `bounds` is [`crate::drawlist::app_bounds`] of the scene,
/// by slot.
pub fn write_groups(scene: &Scene, slots: &[usize], bounds: &[Option<[f64; 4]>]) -> Vec<String> {
    let mut by_slot: HashMap<u32, Vec<&Item>> = HashMap::new();
    let wanted: std::collections::HashSet<u32> = slots.iter().map(|&slot| slot as u32).collect();
    for item in &scene.measured {
        if wanted.contains(&item.slot) {
            by_slot.entry(item.slot).or_default().push(item);
        }
    }
    // The objects that have artwork to draw (the others keep their handles only).
    let drawn: std::collections::HashSet<u32> = scene.items.iter().map(|item| item.slot).filter(|slot| wanted.contains(slot)).collect();
    slots.iter().map(|&slot| {
        let Some(object) = scene.objects.get(slot).filter(|object| object.resolved) else { return "null".to_string() };
        let Some(fig) = scene.figure_for(slot) else { return "null".to_string() };
        let items = by_slot.get(&(slot as u32)).map(Vec::as_slice).unwrap_or(&[]);
        let mut out: Vec<u8> = Vec::with_capacity(256 + items.len() * 220);
        out.extend_from_slice(b"{\"root\":");
        point(&mut out, object.root);
        key(&mut out, "joints");
        out.push(b'[');
        let mut first = true;
        for (i, end) in object.ends.iter().enumerate() {
            let Some(seg) = fig.segments.get(i) else { break };
            if end[0].is_nan() {
                continue;
            }
            if !first {
                out.push(b',');
            }
            first = false;
            out.extend_from_slice(format!("[{},", seg.id).as_bytes());
            num(&mut out, end[0]);
            out.push(b',');
            num(&mut out, end[1]);
            out.push(b']');
        }
        out.push(b']');
        key(&mut out, "segments");
        out.push(b'[');
        if fig.movieclip.is_some() {
            // A movieclip instance is its handle: forty units from the root along its angle.
            let end = object.ends.first().copied().unwrap_or(object.root);
            out.extend_from_slice(b"{\"index\":0");
            key(&mut out, "start");
            point(&mut out, object.starts.first().copied().unwrap_or(object.root));
            key(&mut out, "end");
            point(&mut out, end);
            key(&mut out, "angle");
            num(&mut out, object.angles.first().copied().unwrap_or(0.0));
            key(&mut out, "length");
            num(&mut out, object.lengths.first().copied().unwrap_or(0.0));
            out.push(b'}');
        } else {
            let mut shapes: Vec<&Item> = items.iter().copied().filter(|item| !matches!(item.kind, ItemKind::Poly | ItemKind::Mesh)).collect();
            shapes.sort_by_key(|item| item.index);
            for (n, item) in shapes.into_iter().enumerate() {
                if n > 0 {
                    out.push(b',');
                }
                segment(&mut out, scene, slot, fig, item);
            }
        }
        out.push(b']');
        key(&mut out, "polys");
        out.push(b'[');
        let mut first = true;
        for item in items.iter().filter(|item| item.kind == ItemKind::Poly) {
            if !first {
                out.push(b',');
            }
            first = false;
            poly(&mut out, scene, item);
        }
        out.push(b']');
        if let Some(solution) = &object.solution {
            key(&mut out, "solved");
            out.extend_from_slice(b"{\"angles\":");
            numbers(&mut out, &solution.angles);
            if let Some(lengths) = &solution.lengths {
                key(&mut out, "lengths");
                numbers(&mut out, lengths);
            }
            out.push(b'}');
        }
        if drawn.contains(&(slot as u32)) {
            key(&mut out, "drawn");
            out.extend_from_slice(b"true");
        }
        if let Some(Some(rect)) = bounds.get(slot) {
            key(&mut out, "bounds");
            numbers(&mut out, rect);
        }
        out.push(b'}');
        // The writer emits ASCII punctuation, numbers and the text of identifiers only.
        unsafe { String::from_utf8_unchecked(out) }
    }).collect()
}
