//! World-space geometry of an evaluated frame (`buildGeometrySteps` in
//! src/pivot/geometry.mjs): joints, segment primitives and polygons in the
//! exact painter order the JavaScript editor uses.
//!
//! Objects are resolved level by level along their joins; every level builds in
//! parallel. Points live in one shared buffer per scene, so a frame with 100k
//! primitives performs a handful of allocations, not 100k.

use std::sync::Arc;


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
    /// Depth-tested faces of an evaluated mesh ([`Item::mesh`], [`Item::faces`]).
    Mesh,
}

impl ItemKind {
    pub fn name(self) -> &'static str {
        match self {
            ItemKind::Line | ItemKind::SquareLine => "line",
            ItemKind::Circle | ItemKind::CircleFill | ItemKind::CircleClear => "circle",
            ItemKind::Image => "image",
            ItemKind::Text => "text",
            ItemKind::Poly => "poly",
            ItemKind::Mesh => "mesh",
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
    /// The object's colour transition, which replaces `color` when mixing colours.
    pub transition: Option<Arc<ColorTransition>>,
    pub scale: f64,
    pub image_text_flags: bool,
    /// Movieclip nesting: index into [`Scene::chains`], or `NO_CHAIN`.
    pub chain: u32,
    /// `ITEM_*` drawing flags.
    pub flags: u8,
    /// For `Mesh`: index into [`Scene::meshes`]; `NO_MESH` otherwise.
    pub mesh: u32,
    /// For `Mesh`: the faces of the mesh this drawable holds (a painter-order bucket).
    pub faces: (u32, u32),
    /// For a polygon that is a depth-tested surface: range in [`Scene::depths`],
    /// one depth per point (`depthValues`). Empty otherwise.
    pub depth: (u32, u32),
}

pub const NO_CHAIN: u32 = u32::MAX;
pub const NO_MESH: u32 = u32::MAX;

impl Item {
    /// `depthSurface(item)`: the item is drawn depth tested against the depth
    /// surfaces next to it in its object's painter order. Meshes always are;
    /// a polygon is when it is an opaque triangle or clipped quad with one
    /// depth per point.
    pub fn depth_surface(&self) -> bool {
        match self.kind {
            ItemKind::Mesh => self.faces.1 > self.faces.0,
            ItemKind::Poly => {
                let points = self.points.1 - self.points.0;
                self.depth.1 > self.depth.0 && self.depth.1 - self.depth.0 == points && (3..=4).contains(&points) && self.alpha == 1.0
            }
            _ => false,
        }
    }
}
/// A polygon expanded from a shaped segment: paint it with the segment's brush.
pub const ITEM_SEG_BRUSH: u8 = 1;
/// A fixed colour: ignore the segment's paint and gradient.
pub const ITEM_SOLID: u8 = 2;
/// Stroke `points` as an open polyline (curved Stick Nodes segments).
pub const ITEM_POLYLINE: u8 = 4;

#[derive(Clone, Debug, Default)]
pub struct ObjectGeometry {
    pub slot: u32,
    pub uid: u32,
    pub figure: u32,
    pub root: [f64; 2],
    /// The pose's scale.
    pub scale: f64,
    /// Handle position per segment index (NaN when unresolved).
    pub ends: Vec<[f64; 2]>,
    pub starts: Vec<[f64; 2]>,
    pub opacity: f64,
    pub resolved: bool,
    /// What a spatial rig's joints carry besides their positions; `None` for
    /// a figure without one.
    pub spatial: Option<Arc<ObjectSpatial>>,
    /// Per segment of a figure with rig relationships, what editing reads from its
    /// evaluation (`rigSpace`, `connector`); `None` for a figure without any.
    pub rig: Option<Arc<[RigJoint]>>,
    /// What a follow camera frames of the object (`figureBounds`): `[x0, y0, x1, y1]` over
    /// its visible segments' endpoints, curves and circles with half their stroke, and
    /// every polygon's points (culled surfaces too); a movieclip's handle. `None` when
    /// nothing is finite, and for a figure built without [`build_measured`].
    pub extent: Option<[f64; 4]>,
}

/// `figureBounds` in src/camera-follow.mjs, accumulated point by point.
#[derive(Clone, Copy, Debug)]
struct Bounds {
    rect: [f64; 4],
    found: bool,
}

impl Bounds {
    fn new() -> Bounds {
        Bounds { rect: [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], found: false }
    }

    fn add(&mut self, p: [f64; 2], pad: f64) {
        if !p[0].is_finite() || !p[1].is_finite() {
            return;
        }
        self.found = true;
        let r = &mut self.rect;
        if p[0] - pad < r[0] {
            r[0] = p[0] - pad;
        }
        if p[1] - pad < r[1] {
            r[1] = p[1] - pad;
        }
        if p[0] + pad > r[2] {
            r[2] = p[0] + pad;
        }
        if p[1] + pad > r[3] {
            r[3] = p[1] + pad;
        }
    }

    /// A segment as drawn before Stick Nodes shapes expand it: a circle by its centre and
    /// radius, a curve by its points, a line by its ends; half its stroke around each.
    fn segment(&mut self, it: &Item, prim: Prim, points: &[[f64; 2]]) {
        let pad = if it.thickness.is_nan() { f64::NAN } else { it.thickness.max(0.0) / 2.0 };
        if prim.is_circle() {
            self.add([(it.start[0] + it.end[0]) / 2.0, (it.start[1] + it.end[1]) / 2.0], it.length.abs() / 2.0 + pad);
        } else if !points.is_empty() {
            for &p in points {
                self.add(p, pad);
            }
        } else {
            self.add(it.start, pad);
            self.add(it.end, pad);
        }
    }

    fn extent(&self) -> Option<[f64; 4]> {
        self.found.then_some(self.rect)
    }
}

/// A rigged joint as geometry evaluated it, for editing: the space its authored
/// angle is in (`rigSpace`, which `authoredRigTarget` inverts) and whether it is a
/// connector sliding between two joints.
#[derive(Clone, Copy, Debug, Default)]
pub struct RigJoint {
    pub space: Option<crate::rig::Space>,
    pub connector: bool,
}

/// The 3D side of an evaluated figure: what surfaces, meshes and attached
/// movieclip meshes read from its bones ([`crate::spatial`]).
#[derive(Clone, Debug, Default)]
pub struct ObjectSpatial {
    /// Spatial frame per segment index (bones only).
    pub frames: Vec<Option<crate::rig::SpatialFrame>>,
    /// Depth guide and unprojected position per segment index.
    pub depths: Vec<crate::spatial::JointDepth>,
    /// `animationRotation(figure, object)`.
    pub figure_angle: f64,
    pub mirror: bool,
    /// The pose's perspective.
    pub camera: Option<crate::spatial::Camera>,
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
    /// Scene colour key (ARGB) replacing the native background, if any.
    pub background_color: Option<u32>,
    /// Movieclip owner opacities.
    pub owners: Vec<f64>,
    /// By owner: the instance's absolute scale, times the scales of the
    /// instances it is nested in (the scale its container effects apply at).
    pub owner_scales: Vec<f64>,
    /// Owner chains, outermost first: (owner, child slot inside that clip).
    pub chains: Vec<Vec<(u32, u32)>>,
    /// The effects of the frame this scene was built from ([`Frame::vfx`]).
    pub vfx: Option<Arc<crate::vfx::FrameVfx>>,
    /// By owner (as [`Scene::owners`]): the effects of the clip's own frame,
    /// which its child slots in [`Scene::chains`] index.
    pub clip_vfx: Vec<Option<Arc<crate::vfx::FrameVfx>>>,
    /// Evaluated meshes ([`Item::mesh`]).
    pub meshes: Vec<crate::spatial::SharedMesh>,
    /// Depths of depth-tested surfaces ([`Item::depth`]).
    pub depths: Vec<f64>,
}

impl Scene {
    pub fn points_of(&self, item: &Item) -> &[[f64; 2]] {
        &self.points[item.points.0 as usize..item.points.1 as usize]
    }

    /// `depthValues` of a depth-tested surface.
    pub fn depths_of(&self, item: &Item) -> &[f64] {
        &self.depths[item.depth.0 as usize..item.depth.1 as usize]
    }

    /// The mesh a `Mesh` item draws faces of.
    pub fn mesh_of(&self, item: &Item) -> Option<&crate::spatial::MeshGeometry> {
        self.meshes.get(item.mesh as usize).map(|mesh| &**mesh)
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

/// `objectColor`: a colour of a figure as the object draws it, mixed with its override, or
/// with both overrides of its colour transition and taken `t` of the way between them.
pub fn object_rgb(base: [u8; 3], color: u32, transition: Option<&ColorTransition>) -> [u8; 3] {
    let Some(c) = transition else { return mix_color(base, color) };
    let (a, b) = (mix_color(base, c.from), mix_color(base, c.to));
    // `Math.round(lerp(a, b, t))`; a NaN channel is drawn as 0, as the GPU renderer clamps it.
    std::array::from_fn(|i| (a[i] as f64 + (b[i] as f64 - a[i] as f64) * c.t + 0.5).floor() as u8)
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
    /// Movieclip content: its own figure table, owners and chains (local ids).
    figures: Vec<Arc<Figure>>,
    owners: Vec<f64>,
    owner_scales: Vec<f64>,
    chains: Vec<Vec<(u32, u32)>>,
    clip_vfx: Vec<Option<Arc<crate::vfx::FrameVfx>>>,
    /// Evaluated meshes and surface depths (local indices).
    meshes: Vec<crate::spatial::SharedMesh>,
    depths: Vec<f64>,
}

impl Built {
    fn new(geometry: ObjectGeometry) -> Built {
        Built { geometry, items: Vec::new(), points: Vec::new(), figures: Vec::new(), owners: Vec::new(), owner_scales: Vec::new(), chains: Vec::new(), clip_vfx: Vec::new(), meshes: Vec::new(), depths: Vec::new() }
    }
}

/// `ik` is the object's derived pose when a pinned IK controller reaches it:
/// its angles (and stretched lengths) replace the authored channels.
#[allow(clippy::too_many_arguments)]
/// A segment's pose as the object stores it, before inverse kinematics, rigs and
/// scale: angle, bend and length, with the editor's fallbacks (`segmentAt`).
pub fn segment_pose(obj: &Object, seg: &crate::doc::Segment, i: usize) -> (f64, f64, f64) {
    let angle = obj.angles.get(i).copied().unwrap_or(seg.angle);
    let bend = match &obj.bends {
        Some(b) => b.get(i).copied().unwrap_or(0.0),
        None => seg.bend,
    };
    let length = match &obj.lengths {
        Some(l) => l.get(i).copied().unwrap_or(seg.length),
        None => seg.length,
    };
    (angle, bend, length)
}

#[allow(clippy::too_many_arguments)]
fn build_object(doc: &Document, version: u32, frame: &Frame, slot: usize, figure_slot: u32, fig: &Figure, root: [f64; 2], ik: Option<&crate::ik::Solution>, measure: bool) -> Built {
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
    // Rigged figures evaluate in dependency order with their rig context.
    let rig_plan = plan.rig.clone();
    let mut rig_states: Vec<Option<crate::rig::State>> = if rig_plan.is_some() { vec![None; n] } else { Vec::new() };
    let figure_angle = crate::origin::rotation(fig, obj);
    let order: &[u32] = match &rig_plan {
        Some(r) => &r.order,
        None => &plan.order,
    };
    // 3D: the pose's perspective, and per joint what surfaces and meshes read.
    let has_spatial = rig_plan.is_some() && plan.spatial;
    let camera = if has_spatial { crate::spatial::Camera::of(obj, root) } else { None };
    let shape = obj.spatial.as_ref().and_then(|s| s.shape.as_ref());
    let mut frames: Vec<Option<crate::rig::SpatialFrame>> = if has_spatial { vec![None; n] } else { Vec::new() };
    let mut depths: Vec<crate::spatial::JointDepth> = if has_spatial { vec![Default::default(); n] } else { Vec::new() };
    for &i in order {
        let i = i as usize;
        let seg = &fig.segments[i];
        let parent = plan.parent[i];
        let start = if parent == u32::MAX { root } else { ends[parent as usize] };
        let mut start = if start[0].is_nan() { root } else { start };
        let (posed_angle, mut bend, posed_length) = segment_pose(obj, seg, i);
        let mut angle = ik.map(|s| s.angles[i]).unwrap_or(posed_angle);
        let mut length = ik.and_then(|s| s.lengths.as_ref()).map(|l| l[i]).unwrap_or(posed_length) * scale;
        let mut width = 1.0;
        let mut rig_handle: Option<([f64; 2], bool)> = None;
        if let Some(rp) = &rig_plan {
            let ctx = crate::rig::Context {
                segments: &fig.segments,
                index_of: &plan.index_of,
                plan: rp,
                root,
                scale,
                figure_angle,
                mirror: obj.image_text_flags,
                len_flag: obj.lengths.is_some(),
                lengths: obj.lengths.as_deref(),
                camera,
                shape,
            };
            let joint = |id: u32| -> Option<[f64; 2]> {
                if id == 0 {
                    return Some(root);
                }
                let k = *plan.index_of.get(&id)? as usize;
                (!ends[k][0].is_nan()).then_some(ends[k])
            };
            let st = crate::rig::evaluate(&ctx, &mut rig_states, i, crate::rig::Input { start, angle, length, bend }, &joint);
            if has_spatial {
                frames[i] = st.spatial;
                depths[i] = crate::spatial::JointDepth { offset: st.depth_offset, raw: st.raw_point };
            }
            start = st.start;
            angle = st.angle;
            length = st.length;
            bend = st.bend;
            width = st.width;
            let derived = seg.rig.as_ref().is_some_and(|r| r.spatial.is_some() || r.weighted.is_some());
            rig_handle = st.handle.map(|h| (h, derived));
        }
        let p0 = points.len() as u32;
        let end;
        if let Some((h, true)) = rig_handle {
            end = [fround(h[0]), fround(h[1])];
        } else if bend.abs() < 1e-7 && start[0].is_finite() && start[1].is_finite() && angle.is_finite() && length.is_finite() {
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
        // Children and joins attach at the rig handle (a connector's slider), drawing ends at `end`.
        ends[i] = match rig_handle {
            Some((h, false)) => [fround(h[0]), fround(h[1])],
            _ => end,
        };
        starts[i] = start;
        let rgb = if black_override {
            if (obj.color as i32) < 0 { [0, 0, 0] } else { [(obj.color & 255) as u8, (obj.color >> 8 & 255) as u8, (obj.color >> 16 & 255) as u8] }
        } else {
            object_rgb(seg.rgb, obj.color, obj.color_transition.as_deref())
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
            thickness: seg.thickness * scale.abs() * width,
            rgb,
            alpha: seg.alpha,
            opacity,
            z: seg.z,
            points: (p0, p1),
            color: obj.color,
            transition: obj.color_transition.clone(),
            scale,
            image_text_flags: obj.image_text_flags,
            chain: NO_CHAIN,
            flags: 0,
            mesh: NO_MESH,
            faces: (0, 0),
            depth: (0, 0),
        });
        // A curved Stick Nodes segment draws along its sagitta curve.
        if let Some(shape) = crate::sticknodes::shape_for(seg, obj)
            && shape.sagitta != 0.0
            && bend.abs() < 1e-7
        {
            let item = segment_items.last_mut().unwrap();
            let q0 = points.len() as u32;
            crate::sticknodes::curve(shape, shape.reversed, item.start, item.angle, item.length, scale, &mut points);
            item.points = (q0, points.len() as u32);
            item.bend = 1e-6;
            item.flags |= ITEM_POLYLINE;
        }
    }
    let joint = |handle: u32| -> Option<[f64; 2]> {
        if handle == 0 {
            return Some(root);
        }
        let i = *plan.index_of.get(&handle)? as usize;
        (!ends[i][0].is_nan()).then_some(ends[i])
    };
    // What a joint is before perspective (`rawPoint`); the root has none.
    let raw_of = |handle: u32| -> Option<[f64; 3]> {
        let i = *plan.index_of.get(&handle)? as usize;
        depths.get(i)?.raw
    };
    let joint_depth = |handle: u32| -> crate::spatial::JointDepth { plan.index_of.get(&handle).and_then(|&i| depths.get(i as usize)).copied().unwrap_or_default() };
    let mut surface_depths: Vec<f64> = Vec::new();
    let mut raws: Vec<Option<[f64; 3]>> = Vec::new();
    // What a follow camera frames (`figureBounds`), only when asked for ([`build_measured`]).
    let mut bounds = Bounds::new();
    if measure {
        for it in &segment_items {
            let seg = &fig.segments[it.index as usize];
            if seg.visible {
                bounds.segment(it, seg.prim, &points[it.points.0 as usize..it.points.1 as usize]);
            }
        }
    }
    for (pi, poly) in fig.polyfills.iter().enumerate() {
        let pp = &plan.polygons[pi];
        let p0 = points.len() as u32;
        // A depth-tested face of a perspective pose is clipped before projection,
        // which needs every point's depth.
        let projecting = camera.is_some() && poly.surface.as_ref().is_some_and(|s| s.depth_test);
        raws.clear();
        if let Some(first) = pp.first
            && let Some(p) = joint(first)
        {
            points.push(p);
            if projecting {
                raws.push(raw_of(first));
            }
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
                        if projecting {
                            raws.resize(points.len() - p0 as usize, None);
                        }
                    }
                }
                _ => {
                    if let Some(p) = joint(edge.end) {
                        points.push(p);
                        if projecting {
                            raws.push(raw_of(edge.end));
                        }
                    }
                }
            }
        }
        let p1 = points.len() as u32;
        let mut range = (p0, p1);
        if p1 - p0 > 1 && points[p0 as usize] == points[p1 as usize - 1] {
            points.pop();
            raws.pop();
            range.1 -= 1;
        }
        // One-sided surfaces: culled when they look away, ordered by their depth
        // guides, and depth tested against their neighbours (`surfaceEvaluation`).
        let mut z = poly.z;
        let mut depth = (0u32, 0u32);
        if let Some(surface) = &poly.surface {
            let mut projected = None;
            if projecting {
                match crate::spatial::project_face(&points[range.0 as usize..range.1 as usize], &raws, camera.as_ref()) {
                    Some(Ok(depths)) => projected = Some(depths),
                    Some(Err((clipped, depths))) => {
                        points.truncate(p0 as usize);
                        points.extend_from_slice(&clipped);
                        range = (p0, points.len() as u32);
                        projected = Some(depths);
                    }
                    None => {}
                }
            }
            if measure {
                for &p in &points[range.0 as usize..range.1 as usize] {
                    bounds.add(p, 0.0);
                }
            }
            let result = crate::spatial::evaluate_surface(surface, poly.z, &points[range.0 as usize..range.1 as usize], projected, root, &joint, &joint_depth);
            if result.culled {
                points.truncate(p0 as usize);
                continue;
            }
            z = result.z;
            if let Some(values) = result.depth_values {
                let d0 = surface_depths.len() as u32;
                surface_depths.extend_from_slice(&values);
                depth = (d0, surface_depths.len() as u32);
            }
        } else {
            if measure {
                for &p in &points[range.0 as usize..range.1 as usize] {
                    bounds.add(p, 0.0);
                }
            }
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
            rgb: object_rgb(poly.rgb, obj.color, obj.color_transition.as_deref()),
            alpha: poly.alpha,
            opacity,
            z,
            points: range,
            color: obj.color,
            transition: obj.color_transition.clone(),
            scale,
            image_text_flags: obj.image_text_flags,
            chain: NO_CHAIN,
            flags: 0,
            mesh: NO_MESH,
            faces: (0, 0),
            depth,
        });
    }
    // The figure's mesh: evaluated once after its bones, then split where the
    // figure's other visible primitives lie in painter order (`spatialMeshDrawables`).
    let mut meshes: Vec<crate::spatial::SharedMesh> = Vec::new();
    if let Some(mesh_plan) = fig.mesh.as_ref().filter(|_| rig_plan.is_some()) {
        let context = crate::spatial::MeshContext {
            root,
            scale,
            figure_angle,
            mirror: obj.image_text_flags,
            camera,
            joint: &joint,
            colors: fig.mesh_colors.iter().map(|&c| object_rgb(c, obj.color, obj.color_transition.as_deref())).collect(),
        };
        if let Some(mut mesh) = crate::spatial::evaluate_mesh(mesh_plan, &|i| frames.get(i).copied().flatten(), context) {
            let mut levels: Vec<f64> = items.iter().map(|it| it.z).collect();
            // Zero-width lines paint nothing and must not split the depth-tested faces.
            levels.extend(segment_items.iter().filter(|it| fig.segments[it.index as usize].visible && !(matches!(it.kind, ItemKind::Line | ItemKind::SquareLine) && (it.thickness.is_nan() || it.thickness <= 0.0))).map(|it| it.z));
            for drawable in mesh.split(&levels) {
                items.push(Item {
                    slot: slot as u32,
                    kind: ItemKind::Mesh,
                    index: drawable.index,
                    figure: figure_slot,
                    start: root,
                    end: root,
                    angle: 0.0,
                    bend: 0.0,
                    length: 0.0,
                    thickness: 0.0,
                    rgb: [0, 0, 0],
                    alpha: 1.0,
                    opacity,
                    z: drawable.z,
                    points: (0, 0),
                    color: obj.color,
                    transition: obj.color_transition.clone(),
                    scale,
                    image_text_flags: obj.image_text_flags,
                    chain: NO_CHAIN,
                    flags: 0,
                    mesh: 0,
                    faces: drawable.faces,
                    depth: (0, 0),
                });
            }
            meshes.push(Arc::new(mesh));
        }
    }
    // Drawables: polygons, mesh faces, then visible segments in record order, stably by z.
    segment_items.sort_by_key(|it| it.index);
    for it in segment_items {
        let seg = &fig.segments[it.index as usize];
        if !seg.visible {
            continue;
        }
        match crate::sticknodes::shape_for(seg, obj) {
            Some(shape) => {
                let own: Vec<[f64; 2]> = points[it.points.0 as usize..it.points.1 as usize].to_vec();
                crate::sticknodes::expand(&it, shape, &own, &mut points, &mut items);
            }
            None => items.push(it),
        }
    }
    items.sort_by(|a, b| a.z.total_cmp(&b.z));
    let _ = doc;
    let spatial = has_spatial.then(|| Arc::new(ObjectSpatial { frames, depths, figure_angle, mirror: obj.image_text_flags, camera }));
    let rig = rig_plan.is_some().then(|| {
        rig_states.iter().zip(&fig.segments).map(|(state, seg)| RigJoint { space: state.and_then(|s| s.space), connector: seg.rig.as_ref().is_some_and(|r| r.connector.is_some()) }).collect()
    });
    let extent = bounds.extent();
    let mut built = Built::new(ObjectGeometry { slot: slot as u32, uid: obj.uid, figure: fig.id, root, scale, ends, starts, opacity, resolved: true, spatial, rig, extent });
    built.items = items;
    built.points = points;
    built.meshes = meshes;
    built.depths = surface_depths;
    built
}

/// A movieclip instance's extent for a follow camera: its handle, from the root 40 units along its angle.
fn handle_extent(root: [f64; 2], end: [f64; 2]) -> Option<[f64; 4]> {
    let mut bounds = Bounds::new();
    bounds.add(root, 0.0);
    bounds.add(end, 0.0);
    bounds.extent()
}

/// The object a movieclip instance is joined to, for an attached mesh: its
/// pose, figure and geometry.
struct JoinedParent<'a> {
    object: &'a Object,
    figure: &'a Figure,
    geometry: &'a ObjectGeometry,
}

/// `buildMovieclipMeshSteps`: an attached mesh. Joined to a spatial bone, the
/// bone alone owns its orientation and the instance's root is the bone's start.
#[allow(clippy::too_many_arguments)]
fn build_clip_mesh(frame: &Frame, slot: usize, figure_slot: u32, fig: &Figure, clip: &Movieclip, meshes: &[Arc<crate::spatial::ClipMesh>], mut root: [f64; 2], parent: Option<JoinedParent<'_>>) -> Built {
    let obj = &frame.objects[slot];
    let local_angle = obj.angles.first().copied().unwrap_or(0.0);
    let scale = obj.scale;
    let opacity = 1.0 - obj.transparency / 255.0;
    // The bone the instance hangs from, if its join target is one.
    let bone = parent.as_ref().and_then(|p| {
        let handle = obj.join?.handle;
        let index = *p.figure.plan.index_of.get(&handle)? as usize;
        let spatial = p.geometry.spatial.as_ref()?;
        Some((spatial, spatial.frames.get(index)?.as_ref()?, p.geometry.starts[index]))
    });
    let attachment = match (&parent, &bone) {
        (Some(p), Some((spatial, bone_frame, start))) => {
            root = *start;
            Some(crate::spatial::Attachment { frame: bone_frame, figure_angle: spatial.figure_angle, root: p.geometry.root, scale: p.object.scale, mirror: p.object.image_text_flags, camera: crate::spatial::Camera::of(p.object, p.geometry.root) })
        }
        _ => None,
    };
    let angle = attachment.as_ref().map_or(local_angle, |a| a.figure_angle);
    let end = [root[0] + 40.0 * scale * styk_math::cos(angle), root[1] + 40.0 * scale * styk_math::sin(angle)];
    let mut built = Built::new(ObjectGeometry { slot: slot as u32, uid: obj.uid, figure: fig.id, root, scale, ends: vec![end], starts: vec![root], opacity, resolved: true, spatial: None, rig: None, extent: handle_extent(root, end) });
    let Some(local) = obj.clip_time else { return built };
    // `Math.floor(local)` clamped to the forms the clip has.
    let form = (local.floor().max(0.0) as usize).min(meshes.len() - 1);
    let mesh = &meshes[form];
    let pose = crate::spatial::ClipPose {
        root,
        angle: local_angle,
        scale,
        mirror: obj.image_text_flags,
        pivot: clip.pivot,
        attachment,
        colors: mesh.colors.iter().map(|&c| object_rgb(c, obj.color, obj.color_transition.as_deref())).collect(),
    };
    let mut geometry = crate::spatial::evaluate_clip_mesh(mesh, pose);
    for drawable in geometry.split(&[]) {
        built.items.push(Item {
            slot: slot as u32,
            kind: ItemKind::Mesh,
            index: drawable.index,
            figure: figure_slot,
            start: root,
            end: root,
            angle: 0.0,
            bend: 0.0,
            length: 0.0,
            thickness: 0.0,
            rgb: [0, 0, 0],
            alpha: 1.0,
            opacity,
            z: drawable.z,
            points: (0, 0),
            color: obj.color,
            transition: obj.color_transition.clone(),
            scale,
            image_text_flags: obj.image_text_flags,
            chain: NO_CHAIN,
            flags: 0,
            mesh: 0,
            faces: drawable.faces,
            depth: (0, 0),
        });
    }
    built.meshes.push(Arc::new(geometry));
    built
}

/// The clip's own scene at a local time, shared by every instance.
fn clip_scene(clip: &Movieclip, local: f64) -> Arc<Scene> {
    let key = local.to_bits();
    if let Some(s) = clip.cache.lock().unwrap().get(&key) {
        return s.clone();
    }
    let frame = clip.prepared.evaluate_clip(clip.source_start + local);
    let scene = Arc::new(build(&clip.document, &frame));
    let mut cache = clip.cache.lock().unwrap();
    if cache.len() >= 64 {
        cache.clear();
    }
    cache.insert(key, scene.clone());
    scene
}

/// `buildMovieclipGeometrySteps`: the clip's scene placed at its pivot.
fn build_clip(frame: &Frame, slot: usize, fig: &Figure, clip: &Movieclip, root: [f64; 2]) -> Built {
    let obj = &frame.objects[slot];
    let angle = obj.angles.first().copied().unwrap_or(0.0);
    let scale = obj.scale;
    let flip = if obj.image_text_flags { -1.0 } else { 1.0 };
    let (a, b) = (angle.cos() * scale, angle.sin() * scale);
    let end = [root[0] + 40.0 * angle.cos() * scale, root[1] + 40.0 * angle.sin() * scale];
    let opacity = 1.0 - obj.transparency / 255.0;
    let mut built = Built::new(ObjectGeometry { slot: slot as u32, uid: obj.uid, figure: fig.id, root, scale, ends: vec![end], starts: vec![root], opacity, resolved: true, spatial: None, rig: None, extent: handle_extent(root, end) });
    let Some(local) = obj.clip_time else { return built };
    let source = clip_scene(clip, local);
    // Meshes inside the clip keep their evaluated positions: the editor places a
    // clip's points, starts and ends, and a mesh drawable has none of them.
    built.meshes = source.meshes.clone();
    let pivot = clip.pivot;
    let point = |p: [f64; 2]| {
        let (dx, dy) = (p[0] - pivot[0], (p[1] - pivot[1]) * flip);
        [root[0] + a * dx - b * dy, root[1] + b * dx + a * dy]
    };
    built.figures = source.figures.clone();
    built.owners.push(opacity);
    built.owners.extend_from_slice(&source.owners);
    built.owner_scales.push(scale.abs());
    built.owner_scales.extend(source.owner_scales.iter().map(|s| s * scale.abs()));
    built.clip_vfx.push(source.vfx.clone());
    built.clip_vfx.extend_from_slice(&source.clip_vfx);
    // Chain 0..n of the source shift by one owner and gain this clip as outermost.
    let hidden = clip.document.layers.as_ref().map(|l| l.hidden.clone()).unwrap_or_default();
    let mut chain_ids: std::collections::HashMap<(u32, u32), u32> = std::collections::HashMap::new();
    for it in &source.items {
        let child = &source.objects[it.slot as usize];
        if !hidden.is_empty()
            && let Some(layers) = &clip.document.layers
            && hidden.iter().any(|h| h == layers.layer_of(child.uid))
        {
            continue;
        }
        let chain = *chain_ids.entry((it.slot, it.chain)).or_insert_with(|| {
            let mut c = vec![(0u32, it.slot)];
            if it.chain != NO_CHAIN {
                c.extend(source.chains[it.chain as usize].iter().map(|&(o, p)| (o + 1, p)));
            }
            built.chains.push(c);
            built.chains.len() as u32 - 1
        });
        let p0 = built.points.len() as u32;
        for p in source.points_of(it) {
            built.points.push(point(*p));
        }
        let p1 = built.points.len() as u32;
        let d0 = built.depths.len() as u32;
        built.depths.extend_from_slice(source.depths_of(it));
        built.items.push(Item {
            slot: slot as u32,
            points: (p0, p1),
            depth: (d0, built.depths.len() as u32),
            start: point(it.start),
            end: point(it.end),
            angle: it.angle * flip + angle,
            bend: it.bend * flip,
            length: it.length * scale,
            thickness: it.thickness * scale.abs(),
            scale: it.scale * scale,
            image_text_flags: it.image_text_flags != obj.image_text_flags,
            chain,
            ..it.clone()
        });
    }
    built
}

/// Builds the scene for an evaluated frame.
pub fn build(doc: &Document, frame: &Frame) -> Scene {
    build_filtered(doc, frame, None, false)
}

/// [`build`] with every figure's [`ObjectGeometry::extent`]: what a follow camera samples.
pub fn build_measured(doc: &Document, frame: &Frame) -> Scene {
    build_filtered(doc, frame, None, true)
}

/// [`build`] for the objects in `slots` and the objects they are joined to, the others
/// left unresolved: what an editing call reads of one figure's geometry, without the
/// whole frame's.
pub fn build_only(doc: &Document, frame: &Frame, slots: &[usize]) -> Scene {
    let count = frame.objects.len();
    let mut needed = vec![false; count];
    for &slot in slots {
        let mut at = Some(slot);
        while let Some(i) = at.filter(|&i| i < count && !needed[i]) {
            needed[i] = true;
            at = frame.objects[i].join.map(|j| j.slot as usize);
        }
    }
    build_filtered(doc, frame, Some(&needed), false)
}

fn build_filtered(doc: &Document, frame: &Frame, needed: Option<&[bool]>, measure: bool) -> Scene {
    let count = frame.objects.len();
    let mut scene = Scene {
        objects: vec![ObjectGeometry::default(); count],
        camera: frame.camera,
        background: (frame.background, frame.background_b, frame.background_mix),
        background_color: frame.background_color,
        vfx: frame.vfx.clone(),
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
    // IK across joined figures solves once per assembly, before its objects build (`solveJoinedIK`).
    let joined_ik = crate::ik::joined::solve(frame, &|slot| fig_of.get(slot).filter(|&&s| s != u32::MAX).map(|&s| &scene.figures[s as usize]));
    let mut built: Vec<Option<Built>> = (0..count).map(|_| None).collect();
    for slots in &order {
        let filtered: Vec<usize>;
        let slots = match needed {
            Some(needed) => {
                filtered = slots.iter().copied().filter(|&i| needed[i]).collect();
                &filtered
            }
            None => slots,
        };
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
            if let Some(clip) = &fig.movieclip {
                if let Some(meshes) = &clip.meshes {
                    // An attached mesh reads the bone it is joined to from its parent's geometry.
                    let parent = frame.objects[i].join.and_then(|j| {
                        let geometry = &built.get(j.slot as usize)?.as_ref()?.geometry;
                        let figure = scene.figures.get(*fig_of.get(j.slot as usize)? as usize)?;
                        Some(JoinedParent { object: &frame.objects[j.slot as usize], figure, geometry })
                    });
                    return Some((i, build_clip_mesh(frame, i, fig_of[i], fig, clip, meshes, root, parent)));
                }
                return Some((i, build_clip(frame, i, fig, clip, root)));
            }
            // IK: a pinned object draws its derived pose, from its assembly or solved on its own (`evaluateIKPose`).
            let local;
            let ik = match joined_ik.get(&i) {
                Some(solution) => Some(solution),
                None => {
                    local = crate::ik::solve_local(fig, &frame.objects[i], root);
                    local.as_ref()
                }
            };
            Some((i, build_object(doc, doc.version, frame, i, fig_of[i], fig, root, ik, measure)))
        };
        let results: Vec<(usize, Built)> = crate::par::map(&slots, 32, make).into_iter().flatten().collect();
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
        // Movieclip content brings its own figures, owners and chains.
        let figure_map: Vec<u32> = b
            .figures
            .iter()
            .map(|f| {
                let key = Arc::as_ptr(f) as usize;
                let len = scene.figures.len() as u32;
                *figure_slot.entry(key).or_insert_with(|| {
                    scene.figures.push(f.clone());
                    len
                })
            })
            .collect();
        let owner_base = scene.owners.len() as u32;
        scene.owners.extend_from_slice(&b.owners);
        scene.owner_scales.extend_from_slice(&b.owner_scales);
        scene.clip_vfx.extend(b.clip_vfx);
        let chain_base = scene.chains.len() as u32;
        for c in &b.chains {
            scene.chains.push(c.iter().map(|&(o, p)| (o + owner_base, p)).collect());
        }
        let mesh_base = scene.meshes.len() as u32;
        scene.meshes.extend(b.meshes);
        let depth_base = scene.depths.len() as u32;
        scene.depths.extend_from_slice(&b.depths);
        for mut it in b.items {
            it.points = (it.points.0 + base, it.points.1 + base);
            it.depth = (it.depth.0 + depth_base, it.depth.1 + depth_base);
            if it.mesh != NO_MESH {
                it.mesh += mesh_base;
            }
            if !figure_map.is_empty() {
                it.figure = figure_map[it.figure as usize];
            }
            if it.chain != NO_CHAIN {
                it.chain += chain_base;
            }
            scene.items.push(it);
        }
        scene.objects[slot as usize] = b.geometry;
    }
    scene
}
