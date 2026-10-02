//! World-space geometry of an evaluated frame (`buildGeometrySteps` in
//! src/pivot/geometry.mjs): joints, segment primitives and polygons in the
//! exact painter order the JavaScript editor uses.
//!
//! Objects are resolved level by level along their joins; every level builds in
//! parallel. Points live in one shared buffer per scene, so a frame with 100k
//! primitives performs a handful of allocations, not 100k.

use std::sync::Arc;

use rayon::prelude::*;

use crate::doc::*;
use crate::eval::Frame;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemKind {
    Line,
    SquareLine,
    Circle,
    CircleFill,
    CircleClear,
    Image,
    Text,
    Poly,
}

impl ItemKind {
    pub fn name(self) -> &'static str {
        match self {
            ItemKind::Line | ItemKind::SquareLine => "line",
            ItemKind::Circle | ItemKind::CircleFill | ItemKind::CircleClear => "circle",
            ItemKind::Image => "image",
            ItemKind::Text => "text",
            ItemKind::Poly => "poly",
        }
    }
}

/// One drawable primitive.
#[derive(Clone, Debug)]
pub struct Item {
    pub slot: u32,
    pub kind: ItemKind,
    /// Segment index, or polygon index for `Poly`.
    pub index: u32,
    /// Index into [`Scene::figures`].
    pub figure: u32,
    pub start: [f64; 2],
    pub end: [f64; 2],
    pub angle: f64,
    pub bend: f64,
    pub length: f64,
    pub thickness: f64,
    pub rgb: [u8; 3],
    pub alpha: f64,
    pub opacity: f64,
    pub z: f64,
    /// Range in [`Scene::points`] (bent lines and polygons).
    pub points: (u32, u32),
    /// The object's colour override (for gradient end colours).
    pub color: u32,
    pub scale: f64,
    pub image_text_flags: bool,
}

#[derive(Clone, Debug, Default)]
pub struct ObjectGeometry {
    pub slot: u32,
    pub uid: u32,
    pub figure: u32,
    pub root: [f64; 2],
    /// Handle position per segment index (NaN when unresolved).
    pub ends: Vec<[f64; 2]>,
    pub starts: Vec<[f64; 2]>,
    pub opacity: f64,
    pub resolved: bool,
}

impl ObjectGeometry {
    pub fn joint(&self, fig: &Figure, handle: u32) -> Option<[f64; 2]> {
        if handle == 0 {
            return Some(self.root);
        }
        let i = *fig.plan.index_of.get(&handle)? as usize;
        let p = self.ends[i];
        (!p[0].is_nan()).then_some(p)
    }
}

#[derive(Clone, Debug, Default)]
pub struct Scene {
    pub objects: Vec<ObjectGeometry>,
    pub items: Vec<Item>,
    pub points: Vec<[f64; 2]>,
    pub figures: Vec<Arc<Figure>>,
    pub camera: Option<Camera>,
    pub background: (u32, u32, f64),
    pub warnings: Vec<String>,
}

impl Scene {
    pub fn points_of(&self, item: &Item) -> &[[f64; 2]] {
        &self.points[item.points.0 as usize..item.points.1 as usize]
    }
}

/// `arc()` in geometry.mjs: a bent segment sampled as a circular arc.
pub fn arc_points(start: [f64; 2], angle: f64, length: f64, bend: f64, out: &mut Vec<[f64; 2]>) {
    if !start[0].is_finite() || !start[1].is_finite() || !angle.is_finite() || !length.is_finite() || !bend.is_finite() {
        return;
    }
    if bend.abs() < 1e-7 {
        out.push(start);
        out.push([start[0] + length * angle.cos(), start[1] + length * angle.sin()]);
        return;
    }
    let steps = 8f64.max(160f64.min((bend.abs() * 16.0).ceil()).min((length.abs() / 4.0).ceil())) as usize;
    let r = length / bend;
    let (s0, c0) = angle.sin_cos();
    for i in 0..=steps {
        let a = angle + (bend * i as f64) / steps as f64;
        out.push([start[0] + r * (a.sin() - s0), start[1] + r * (c0 - a.cos())]);
    }
}

pub fn fround(v: f64) -> f64 {
    v as f32 as f64
}

pub fn mix_color(base: [u8; 3], raw: u32) -> [u8; 3] {
    let k = raw >> 24;
    if k == 0 {
        return base;
    }
    let c = [(raw & 255) as u8, (raw >> 8 & 255) as u8, (raw >> 16 & 255) as u8];
    if k == 255 {
        return c;
    }
    let mut out = [0u8; 3];
    for i in 0..3 {
        let v = base[i] as i64;
        out[i] = ((v * 256 + k as i64 * (c[i] as i64 - v)) as f64 / 256.0).floor() as u8;
    }
    out
}

/// Per-object build output before it is spliced into the scene buffers.
struct Built {
    geometry: ObjectGeometry,
    items: Vec<Item>,
    points: Vec<[f64; 2]>,
}

fn build_object(doc: &Document, version: u32, frame: &Frame, slot: usize, figure_slot: u32, fig: &Figure, root: [f64; 2]) -> Built {
    let obj = &frame.objects[slot];
    let n = fig.segments.len();
    let scale = obj.scale;
    let opacity = 1.0 - obj.transparency / 255.0;
    let nan = [f64::NAN, f64::NAN];
    let mut ends = vec![nan; n];
    let mut starts = vec![nan; n];
    let mut items: Vec<Item> = Vec::with_capacity(n + fig.polyfills.len());
    let mut points: Vec<[f64; 2]> = Vec::new();
    // Segment item index per segment (for polygon edges).
    let mut seg_item = vec![u32::MAX; n];
    let plan = &fig.plan;
    let black_override = version < 5 && !fig.segment_colors;
    let mut segment_items: Vec<Item> = Vec::with_capacity(n);
    for &i in &plan.order {
        let i = i as usize;
        let seg = &fig.segments[i];
        let parent = plan.parent[i];
        let start = if parent == u32::MAX { root } else { ends[parent as usize] };
        let start = if start[0].is_nan() { root } else { start };
        let angle = obj.angles.get(i).copied().unwrap_or(seg.angle);
        let bend = match &obj.bends {
            Some(b) => b.get(i).copied().unwrap_or(0.0),
            None => seg.bend,
        };
        let length = match &obj.lengths {
            Some(l) => l.get(i).copied().unwrap_or(seg.length),
            None => seg.length,
        } * scale;
        let p0 = points.len() as u32;
        let end;
        if bend.abs() < 1e-7 && start[0].is_finite() && start[1].is_finite() && angle.is_finite() && length.is_finite() {
            end = [fround(start[0] + length * angle.cos()), fround(start[1] + length * angle.sin())];
        } else {
            arc_points(start, angle, length, bend, &mut points);
            let last = points.last().copied().unwrap_or(start);
            end = [fround(last[0]), fround(last[1])];
            if let Some(l) = points.last_mut() {
                *l = end;
            }
        }
        let p1 = points.len() as u32;
        ends[i] = end;
        starts[i] = start;
        let rgb = if black_override {
            if (obj.color as i32) < 0 { [0, 0, 0] } else { [(obj.color & 255) as u8, (obj.color >> 8 & 255) as u8, (obj.color >> 16 & 255) as u8] }
        } else {
            mix_color(seg.rgb, obj.color)
        };
        let kind = match seg.prim {
            Prim::Line => ItemKind::Line,
            Prim::SquareLine => ItemKind::SquareLine,
            Prim::Circle => ItemKind::Circle,
            Prim::CircleFill => ItemKind::CircleFill,
            Prim::CircleClear => ItemKind::CircleClear,
            Prim::Image => ItemKind::Image,
            Prim::Text => ItemKind::Text,
        };
        seg_item[i] = segment_items.len() as u32;
        segment_items.push(Item {
            slot: slot as u32,
            kind,
            index: i as u32,
            figure: figure_slot,
            start,
            end,
            angle,
            bend,
            length,
            thickness: seg.thickness * scale.abs(),
            rgb,
            alpha: seg.alpha,
            opacity,
            z: seg.z,
            points: (p0, p1),
            color: obj.color,
            scale,
            image_text_flags: obj.image_text_flags,
        });
    }
    let joint = |handle: u32| -> Option<[f64; 2]> {
        if handle == 0 {
            return Some(root);
        }
        let i = *plan.index_of.get(&handle)? as usize;
        (!ends[i][0].is_nan()).then_some(ends[i])
    };
    for (pi, poly) in fig.polyfills.iter().enumerate() {
        let pp = &plan.polygons[pi];
        let p0 = points.len() as u32;
        if let Some(first) = pp.first
            && let Some(p) = joint(first)
        {
            points.push(p);
        }
        for edge in &pp.edges {
            let item = if edge.index == u32::MAX { None } else { segment_items.get(seg_item[edge.index as usize] as usize) };
            match item {
                Some(e) if e.bend.abs() > 1e-7 => {
                    let (a, b) = (e.points.0 as usize, e.points.1 as usize);
                    if b > a {
                        if edge.reverse {
                            for k in (a..b - 1).rev() {
                                points.push(points[k]);
                            }
                        } else {
                            for k in a + 1..b {
                                points.push(points[k]);
                            }
                        }
                    }
                }
                _ => {
                    if let Some(p) = joint(edge.end) {
                        points.push(p);
                    }
                }
            }
        }
        let p1 = points.len() as u32;
        let mut range = (p0, p1);
        if p1 - p0 > 1 && points[p0 as usize] == points[p1 as usize - 1] {
            points.pop();
            range.1 -= 1;
        }
        items.push(Item {
            slot: slot as u32,
            kind: ItemKind::Poly,
            index: pi as u32,
            figure: figure_slot,
            start: root,
            end: root,
            angle: 0.0,
            bend: 0.0,
            length: 0.0,
            thickness: 0.0,
            rgb: mix_color(poly.rgb, obj.color),
            alpha: poly.alpha,
            opacity,
            z: poly.z,
            points: range,
            color: obj.color,
            scale,
            image_text_flags: obj.image_text_flags,
        });
    }
    // Drawables: polygons, then visible segments in record order, stably by z.
    segment_items.sort_by_key(|it| it.index);
    for it in segment_items {
        if fig.segments[it.index as usize].visible {
            items.push(it);
        }
    }
    items.sort_by(|a, b| a.z.total_cmp(&b.z));
    let _ = doc;
    Built {
        geometry: ObjectGeometry { slot: slot as u32, uid: obj.uid, figure: fig.id, root, ends, starts, opacity, resolved: true },
        items,
        points,
    }
}

/// Builds the scene for an evaluated frame.
pub fn build(doc: &Document, frame: &Frame) -> Scene {
    let count = frame.objects.len();
    let mut scene = Scene {
        objects: vec![ObjectGeometry::default(); count],
        camera: frame.camera,
        background: (frame.background, frame.background_b, frame.background_mix),
        ..Default::default()
    };
    // Join levels: roots first, then children of resolved parents.
    let mut level = vec![u32::MAX; count];
    let mut order: Vec<Vec<usize>> = Vec::new();
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); count];
    let mut current: Vec<usize> = Vec::new();
    for (i, o) in frame.objects.iter().enumerate() {
        match o.join {
            Some(j) if (j.slot as usize) < count => children[j.slot as usize].push(i),
            _ => current.push(i),
        }
    }
    let mut depth = 0;
    while !current.is_empty() {
        let mut next = Vec::new();
        for &i in &current {
            level[i] = depth;
            next.extend(children[i].iter().copied());
        }
        order.push(std::mem::take(&mut current));
        current = next;
        depth += 1;
        if depth as usize > count {
            break;
        }
    }
    // Figure table for items (styled instances resolve to their own figure).
    let mut figure_slot = std::collections::HashMap::new();
    let mut fig_of = vec![u32::MAX; count];
    for (i, o) in frame.objects.iter().enumerate() {
        let fig = match &o.figure_override {
            Some(f) => Some(f),
            None => doc.figure(o.figure),
        };
        if let Some(fig) = fig {
            let len = scene.figures.len() as u32;
            let s = *figure_slot.entry(Arc::as_ptr(fig) as usize).or_insert_with(|| {
                scene.figures.push(fig.clone());
                len
            });
            fig_of[i] = s;
        }
    }
    let mut built: Vec<Option<Built>> = (0..count).map(|_| None).collect();
    for slots in &order {
        let resolve_root = |i: usize, built: &[Option<Built>]| -> Option<[f64; 2]> {
            let o = &frame.objects[i];
            let mut root = [o.root[0], o.root[1]];
            if let Some(j) = o.join {
                let parent = built.get(j.slot as usize)?.as_ref()?;
                let pfig = doc.figure(frame.objects[j.slot as usize].figure)?;
                if let Some(p) = parent.geometry.joint(pfig, j.handle) {
                    root = p;
                }
            }
            Some(root)
        };
        let make = |&i: &usize| -> Option<(usize, Built)> {
            let fig = &scene.figures[*fig_of.get(i)? as usize];
            let root = resolve_root(i, &built).unwrap_or(frame.objects[i].root);
            if fig.movieclip.is_some() {
                // Movieclip instances are expanded by the movieclip module.
                return None;
            }
            Some((i, build_object(doc, doc.version, frame, i, fig_of[i], fig, root)))
        };
        let results: Vec<(usize, Built)> = if slots.len() >= 32 {
            slots.par_iter().filter_map(make).collect()
        } else {
            slots.iter().filter_map(make).collect()
        };
        for (i, b) in results {
            built[i] = Some(b);
        }
    }
    for (i, l) in level.iter().enumerate() {
        if *l == u32::MAX {
            scene.warnings.push(format!("Cyclic join or unresolved target at object {i}"));
        }
    }
    // Splice in draw order.
    let total_items: usize = built.iter().flatten().map(|b| b.items.len()).sum();
    let total_points: usize = built.iter().flatten().map(|b| b.points.len()).sum();
    scene.items.reserve(total_items);
    scene.points.reserve(total_points);
    for &slot in &frame.draw_order {
        let Some(b) = built[slot as usize].take() else { continue };
        let base = scene.points.len() as u32;
        scene.points.extend_from_slice(&b.points);
        for mut it in b.items {
            it.points = (it.points.0 + base, it.points.1 + base);
            scene.items.push(it);
        }
        scene.objects[slot as usize] = b.geometry;
    }
    scene
}
