//! Scene → GPU draw list.
//!
//! Every primitive becomes one fixed-size [`Prim`] record that the GPU expands
//! into a quad and shades analytically (signed distance for strokes, arcs and
//! circles; MSAA for triangles). Records are in world units, so a draw list is
//! view independent: panning and zooming never re-encode or re-upload it.
//!
//! Pivot composites a translucent figure (and a translucent outline) as one
//! layer. Those become isolation [`Group`]s: the renderer draws every group of
//! one nesting depth into an atlas in a single pass, then composites each with
//! a `COMPOSITE` record at its place in the parent's painter order.

use std::f64::consts::PI;
use std::sync::Arc;

use crate::assets::Assets;
use crate::doc::*;
use crate::geometry::{mix_color, Item, ItemKind, Scene};
use crate::vfx::layers::{item_extent, rounded, Extent, EMPTY};
use crate::vfx::{Effect, FrameVfx, ObjectVfx};

pub use styk_draw::{
    argb8, CameraPose, EffectLayer, Effects, LayerKind, NO_LAYER, rgba8, sample_paint, window_depth, DrawList, Group, Pass, Prim, SharedDrawList, F_CLOSED, F_DEPTH, F_DRAWING, F_RING_FILL, F_SMOOTH, F_TINT, K_ARC, K_BG_IMAGE, K_BOX, K_CAPSULE,
    K_COMPOSITE, K_DEPTH_RESET, K_DISC, K_FILL_VIEW, K_IMAGE, K_POLYLINE, K_RING, K_TRI, NO_PAINT, P_ANGULAR, P_DIAMOND, P_LINEAR, P_RADIAL, P_RADIAL_RING,
};

/// A local primitive list with world bounds, before groups are assigned.
#[derive(Default)]
struct Out {
    prims: Vec<Prim>,
    paints: Vec<[f32; 4]>,
    points: Vec<[f32; 2]>,
    bounds: [f32; 4],
}

impl Out {
    fn new() -> Out {
        Out { prims: Vec::new(), paints: Vec::new(), points: Vec::new(), bounds: [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY] }
    }
    fn grow(&mut self, x0: f64, y0: f64, x1: f64, y1: f64) {
        self.bounds[0] = self.bounds[0].min(x0 as f32);
        self.bounds[1] = self.bounds[1].min(y0 as f32);
        self.bounds[2] = self.bounds[2].max(x1 as f32);
        self.bounds[3] = self.bounds[3].max(y1 as f32);
    }
    fn push(&mut self, p: Prim, bounds: [f64; 4]) {
        self.grow(bounds[0], bounds[1], bounds[2], bounds[3]);
        self.prims.push(p);
    }
    /// One polyline record (vertex offsets are local and fixed up when merged).
    fn polyline(&mut self, pts: &[[f64; 2]], hw: f64, closed: bool, color: u32, paint: u32) {
        if pts.len() < 2 {
            return;
        }
        // Very long polylines split into overlapping-free runs of 512 vertices.
        let mut start = 0;
        while start + 1 < pts.len() {
            let end = (start + 512).min(pts.len());
            let run = &pts[start..end];
            let at = self.points.len() as u32;
            let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
            for p in run {
                self.points.push([p[0] as f32, p[1] as f32]);
                x0 = x0.min(p[0]);
                y0 = y0.min(p[1]);
                x1 = x1.max(p[0]);
                y1 = y1.max(p[1]);
            }
            let whole = closed && start == 0 && end == pts.len();
            let flags = if whole { F_CLOSED } else { 0 };
            let b = [x0 - hw, y0 - hw, x1 + hw, y1 + hw];
            self.push(
                Prim {
                    kind: K_POLYLINE | flags,
                    group: 0,
                    color,
                    paint,
                    a: [x0 as f32, y0 as f32, x1 as f32, y1 as f32],
                    b: [hw as f32, f32::from_bits(at), f32::from_bits(run.len() as u32), 0.0],
                    c: [0.0; 4],
                },
                b,
            );
            if end == pts.len() {
                break;
            }
            start = end - 1;
        }
    }

    /// Stores a paint record; offsets are local and fixed up when merged.
    fn paint(&mut self, kind: f32, geom: [f32; 4], extra: [f32; 4], stops: &[(f32, [f32; 4])]) -> u32 {
        let at = self.paints.len() as u32;
        self.paints.push([kind, stops.len() as f32, 0.0, 0.0]);
        self.paints.push(geom);
        self.paints.push(extra);
        for &(offset, color) in stops {
            self.paints.push([offset, 0.0, 0.0, 0.0]);
            self.paints.push(color);
        }
        at
    }
}

fn prim(kind: u32, color: u32, paint: u32, a: [f64; 4], b: [f64; 4], c: [f64; 4]) -> Prim {
    let f = |v: [f64; 4]| [v[0] as f32, v[1] as f32, v[2] as f32, v[3] as f32];
    Prim { kind, group: 0, color, paint, a: f(a), b: f(b), c: f(c) }
}

fn stop_color(rgb: [u8; 3], opacity: f64) -> [f32; 4] {
    [rgb[0] as f32 / 255.0, rgb[1] as f32 / 255.0, rgb[2] as f32 / 255.0, opacity.clamp(0.0, 1.0) as f32]
}

/// Colour mapping of a figure's override (`objectColor`).
fn object_color(c: [u8; 3], color: u32) -> [u8; 3] {
    mix_color(c, color)
}

/// `paintGeometry` + record for a paint between two world points.
fn paint_record(out: &mut Out, p: &Paint, stops: &[(f32, [f32; 4])], start: [f64; 2], end: [f64; 2]) -> u32 {
    let length = (end[0] - start[0]).hypot(end[1] - start[1]).max(0.001);
    let angle = (end[1] - start[1]).atan2(end[0] - start[0]) + p.angle * PI / 180.0;
    let (cx, cy) = ((start[0] + end[0]) / 2.0, (start[1] + end[1]) / 2.0);
    let (dx, dy) = (angle.cos() * length / 2.0, angle.sin() * length / 2.0);
    match p.kind {
        PaintKind::Radial => out.paint(P_RADIAL, [cx as f32, cy as f32, (length / 2.0) as f32, 0.0], [0.0; 4], stops),
        PaintKind::Angular => out.paint(P_ANGULAR, [cx as f32, cy as f32, angle as f32, 0.0], [0.0; 4], stops),
        PaintKind::Diamond => out.paint(P_DIAMOND, [cx as f32, cy as f32, angle as f32, (length / 2.0) as f32], [0.0; 4], stops),
        PaintKind::Linear | PaintKind::Solid => {
            out.paint(P_LINEAR, [(cx - dx) as f32, (cy - dy) as f32, (cx + dx) as f32, (cy + dy) as f32], [0.0; 4], stops)
        }
    }
}

struct Ctx<'a> {
    scene: &'a Scene,
    assets: &'a Assets,
    /// An onion skin's colour: every primitive is drawn in it, flat.
    tint: Option<[u8; 3]>,
}

impl<'a> Ctx<'a> {
    fn figure(&self, it: &Item) -> &Figure {
        &self.scene.figures[it.figure as usize]
    }

    /// Brush of a segment: paint, two-colour gradient, or solid colour.
    ///
    /// An onion skin's tint replaces the item's colour and drops its secondary
    /// colour, but a custom gradient keeps its own colours, as the editor's styled
    /// onion drawables do (`onion-skin.mjs` sets `rgb` and clears `secondary` only;
    /// a solid paint is drawn in `rgb`, `paintFor` in renderer-geometry.mjs).
    fn brush(&self, out: &mut Out, it: &Item, outline: bool) -> (u32, u32) {
        let seg = &self.figure(it).segments[it.index as usize];
        let solid = rgba8(self.tint.unwrap_or(it.rgb), it.alpha);
        if outline || it.color >> 24 == 255 || it.flags & crate::geometry::ITEM_SOLID != 0 {
            return (solid, NO_PAINT);
        }
        if self.tint.is_some() && seg.paint.as_ref().is_some_and(|paint| paint.kind == PaintKind::Solid || paint.stops.len() == 1) {
            return (solid, NO_PAINT);
        }
        let secondary = if self.tint.is_some() { None } else { seg.secondary };
        if let Some(paint) = &seg.paint {
            let mut stops: Vec<(f32, [f32; 4])> =
                paint.stops.iter().map(|s| (s.offset as f32, stop_color(object_color(s.rgb, it.color), s.opacity))).collect();
            stops.sort_by(|a, b| a.0.total_cmp(&b.0));
            stops[0].1 = stop_color(object_color(seg.rgb, it.color), seg.alpha);
            if let Some((rgb, a)) = secondary
                && stops.len() > 1
            {
                let last = stops.len() - 1;
                stops[last].1 = stop_color(object_color(rgb, it.color), 1.0 - a as f64 / 255.0);
            }
            if stops.len() == 1 || paint.kind == PaintKind::Solid {
                let c = stops[0].1;
                return (rgba8([(c[0] * 255.0).round() as u8, (c[1] * 255.0).round() as u8, (c[2] * 255.0).round() as u8], c[3] as f64), NO_PAINT);
            }
            return (solid, paint_record(out, paint, &stops, it.start, it.end));
        }
        if let Some((rgb, a)) = secondary {
            let mut end = it.end;
            let (dx, dy) = (it.end[0] - it.start[0], it.end[1] - it.start[1]);
            if it.bend.abs() > 1e-7 && matches!(it.kind, ItemKind::Line | ItemKind::SquareLine) {
                let pts = self.scene.points_of(it);
                let (mut minx, mut miny, mut maxx, mut maxy) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
                for p in pts {
                    minx = minx.min(p[0]);
                    maxx = maxx.max(p[0]);
                    miny = miny.min(p[1]);
                    maxy = maxy.max(p[1]);
                }
                let (w, h) = (maxx - minx, maxy - miny);
                if w > 1e-6 && h > 1e-6 {
                    let d = dx * dx / (w * w) + dy * dy / (h * h);
                    let (gx, gy) = (dx / (w * w * d), dy / (h * h * d));
                    let norm = gx * gx + gy * gy;
                    if norm > 0.0 {
                        end = [it.start[0] + gx / norm, it.start[1] + gy / norm];
                    }
                }
            }
            let stops = [(0.0, stop_color(it.rgb, it.alpha)), (1.0, stop_color(object_color(rgb, it.color), 1.0 - a as f64 / 255.0))];
            let rec = out.paint(P_LINEAR, [it.start[0] as f32, it.start[1] as f32, end[0] as f32, end[1] as f32], [0.0; 4], &stops);
            return (solid, rec);
        }
        (solid, NO_PAINT)
    }

    /// Emits one drawable. `outline` is the outline half-extension (0 = normal pass).
    fn item(&self, out: &mut Out, it: &Item, outline: f64, outline_rgb: [u8; 3]) {
        let fig = self.figure(it);
        match it.kind {
            // Depth-tested faces are encoded as runs (`Ctx::depth_run`), never one by one.
            ItemKind::Mesh => {}
            ItemKind::Poly => self.poly(out, it, outline, outline_rgb),
            ItemKind::Line | ItemKind::SquareLine => {
                let width = it.thickness + 2.0 * outline;
                if width <= 0.0 {
                    return;
                }
                let (color, paint) = if outline > 0.0 { (rgba8(outline_rgb, 1.0), NO_PAINT) } else { self.brush(out, it, false) };
                let hw = width / 2.0;
                let square = it.kind == ItemKind::SquareLine;
                if it.flags & crate::geometry::ITEM_POLYLINE != 0 {
                    // Curved Stick Nodes strokes: one polyline, round caps and joins.
                    let pts = self.scene.points_of(it);
                    out.polyline(pts, hw, false, color, paint);
                } else if it.bend.abs() > 1e-7 {
                    self.arc(out, it, hw, color, paint);
                } else {
                    let (s, e) = (it.start, it.end);
                    let pad = if square { hw * std::f64::consts::SQRT_2 } else { hw };
                    let b = [s[0].min(e[0]) - pad, s[1].min(e[1]) - pad, s[0].max(e[0]) + pad, s[1].max(e[1]) + pad];
                    let kind = if square { K_BOX } else { K_CAPSULE };
                    out.push(prim(kind, color, paint, [s[0], s[1], e[0], e[1]], [hw, 0.0, 0.0, 0.0], [0.0; 4]), b);
                }
            }
            ItemKind::Circle | ItemKind::CircleFill | ItemKind::CircleClear => {
                let width = it.thickness + 2.0 * outline;
                let filled = it.kind == ItemKind::CircleFill || width >= it.length.abs();
                let radius = (it.length.abs() + if filled { width } else { 0.0 }) / 2.0;
                if radius == 0.0 {
                    return;
                }
                let c = [(it.start[0] + it.end[0]) / 2.0, (it.start[1] + it.end[1]) / 2.0];
                let (color, paint) = if outline > 0.0 { (rgba8(outline_rgb, 1.0), NO_PAINT) } else { self.brush(out, it, false) };
                if filled {
                    out.push(prim(K_DISC, color, paint, [c[0], c[1], radius, 0.0], [0.0; 4], [0.0; 4]), [c[0] - radius, c[1] - radius, c[0] + radius, c[1] + radius]);
                    return;
                }
                let hw = width / 2.0;
                let outer = radius + hw.max(0.0);
                let bounds = [c[0] - outer, c[1] - outer, c[0] + outer, c[1] + outer];
                if it.kind == ItemKind::Circle {
                    // White interior (or the brush itself when outlining), then the ring.
                    let fill = if outline > 0.0 { color } else { rgba8([255, 255, 255], it.alpha) };
                    let flags = F_RING_FILL;
                    let kind = if width > 0.0 { K_RING } else { K_DISC };
                    if kind == K_DISC {
                        out.push(prim(K_DISC, fill, NO_PAINT, [c[0], c[1], radius, 0.0], [0.0; 4], [0.0; 4]), bounds);
                    } else {
                        out.push(
                            prim(K_RING | flags, color, paint, [c[0], c[1], radius, hw], [f32::from_bits(fill) as f64, 0.0, 0.0, 0.0], [0.0; 4]),
                            bounds,
                        );
                    }
                } else if width > 0.0 {
                    out.push(prim(K_RING, color, paint, [c[0], c[1], radius, hw], [0.0; 4], [0.0; 4]), bounds);
                }
            }
            ItemKind::Image => {
                if outline > 0.0 {
                    return;
                }
                let seg = &fig.segments[it.index as usize];
                let Some(asset) = fig.image(seg.asset) else { return };
                self.image(out, it, seg, asset);
            }
            ItemKind::Text => {
                let seg = &fig.segments[it.index as usize];
                let Some(text) = fig.text(seg.asset) else { return };
                if outline > 0.0 {
                    if let Some(figure_outline) = &fig.outline {
                        self.text_outline(out, it, seg, text, crate::assets::text_outline_width(figure_outline), outline_rgb);
                    }
                    return;
                }
                self.text(out, it, seg, text);
            }
        }
    }

    fn arc(&self, out: &mut Out, it: &Item, hw: f64, color: u32, paint: u32) {
        let r = it.length / it.bend;
        let (s, c) = it.angle.sin_cos();
        let center = [it.start[0] - r * s, it.start[1] + r * c];
        let radius = r.abs();
        let theta0 = if r > 0.0 { it.angle - PI / 2.0 } else { it.angle + PI / 2.0 };
        let sweep = it.bend.clamp(-2.0 * PI, 2.0 * PI);
        // Bounds: the points of the sampled arc, padded by the half width.
        let pts = self.scene.points_of(it);
        let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        for p in pts {
            b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
        }
        // Chords stay inside the arc's own bulge; add the sagitta of one step.
        let pad = hw + radius * (1.0 - (sweep.abs() / 16.0).cos()).abs() + 0.5;
        b = [b[0] - pad, b[1] - pad, b[2] + pad, b[3] + pad];
        out.push(prim(K_ARC, color, paint, [center[0], center[1], radius, hw], [theta0, sweep, 0.0, 0.0], [0.0; 4]), b);
    }

    fn poly(&self, out: &mut Out, it: &Item, outline: f64, outline_rgb: [u8; 3]) {
        let pts = self.scene.points_of(it);
        if pts.len() < 3 {
            return;
        }
        if outline > 0.0 {
            // Stroke of the closed outline path, width 2*outline, round joins.
            out.polyline(pts, outline, true, rgba8(outline_rgb, 1.0), NO_PAINT);
            return;
        }
        let (color, paint) = if it.flags & crate::geometry::ITEM_SEG_BRUSH != 0 { self.brush(out, it, false) } else { (rgba8(self.tint.unwrap_or(it.rgb), it.alpha), NO_PAINT) };
        if convex(pts) {
            for k in 1..pts.len() - 1 {
                tri(out, color, paint, pts[0], pts[k], pts[k + 1]);
            }
            return;
        }
        let mut builder = lyon_path::Path::builder();
        builder.begin(lyon_path::math::point(pts[0][0] as f32, pts[0][1] as f32));
        for p in &pts[1..] {
            builder.line_to(lyon_path::math::point(p[0] as f32, p[1] as f32));
        }
        builder.end(true);
        let tris = crate::assets::triangulate(&builder.build(), lyon_tessellation::FillRule::EvenOdd, 0.05);
        for t in tris.chunks_exact(3) {
            let f = |v: [f32; 2]| [v[0] as f64, v[1] as f64];
            tri(out, color, paint, f(t[0]), f(t[1]), f(t[2]));
        }
    }

    /// Sprite placement (`drawPrimitives` image branch).
    fn image(&self, out: &mut Out, it: &Item, seg: &Segment, a: &ImageAsset) {
        if a.anchor_length == 0.0 {
            return;
        }
        let mirror = seg.mirror != it.image_text_flags;
        let start = if seg.reverse { it.end } else { it.start };
        let theta = (if mirror { it.angle + a.anchor_angle - PI } else { it.angle - a.anchor_angle }) + if seg.reverse { PI } else { 0.0 };
        let k = (it.end[0] - it.start[0]).hypot(it.end[1] - it.start[1]) / a.anchor_length;
        let sx = if mirror { -k } else { k };
        let (sin, cos) = theta.sin_cos();
        let place = |anchor: [f64; 2], px: f64, py: f64| -> [f64; 2] {
            let (lx, ly) = (sx * (px - anchor[0]), k * (py - anchor[1]));
            [start[0] + cos * lx - sin * ly, start[1] + sin * lx + cos * ly]
        };
        // A tinted sprite keeps its coverage and takes the tint as its colour.
        let color = rgba8(self.tint.unwrap_or([255, 255, 255]), it.alpha);
        let tint = if self.tint.is_some() { F_TINT } else { 0 };
        let flags = if a.smooth { F_SMOOTH } else { 0 } | tint;
        // One atlas rectangle drawn over the cel-space rectangle (x, y, w, h); `extra` is the record's last parameter.
        let quad = |out: &mut Out, kind: u32, e: &crate::assets::AtlasEntry, anchor: [f64; 2], x: f64, y: f64, w: f64, h: f64, extra: f64| {
            let o = place(anchor, x, y);
            let u = place(anchor, x + w, y);
            let v = place(anchor, x, y + h);
            let (ux, uy, vx, vy) = (u[0] - o[0], u[1] - o[1], v[0] - o[0], v[1] - o[1]);
            let far = [o[0] + ux + vx, o[1] + uy + vy];
            let xs = [o[0], u[0], v[0], far[0]];
            let ys = [o[1], u[1], v[1], far[1]];
            let b = [
                xs.iter().copied().fold(f64::INFINITY, f64::min),
                ys.iter().copied().fold(f64::INFINITY, f64::min),
                xs.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                ys.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            ];
            let uv = e.uv();
            out.push(prim(kind, color, NO_PAINT, [o[0], o[1], ux, uy], [vx, vy, e.layer as f64, extra], [uv[0] as f64, uv[1] as f64, uv[2] as f64, uv[3] as f64]), b);
        };
        if let Some(tiles) = &a.tiles {
            // Every level's tiles; the shader keeps the level the view calls for (`crate::drawing`).
            let Some(starts) = self.assets.cel_levels(tiles) else { return };
            for (level, drawn) in tiles.levels.iter().enumerate() {
                let span = (crate::drawing::TILE << level) as f64;
                for (i, &(tx, ty)) in drawn.iter().enumerate() {
                    let Some(e) = self.assets.drawing.get((starts[level] + i as u32) as usize).filter(|e| e.valid) else { continue };
                    quad(out, K_IMAGE | F_DRAWING | tint, e, tiles.anchor, tx as f64 * span, ty as f64 * span, span, span, level as f64);
                }
            }
            return;
        }
        let Some(image) = a.image else { return };
        let Some(e) = self.assets.image(image) else { return };
        let [w, h] = a.display_size.unwrap_or([e.source_w as f64, e.source_h as f64]);
        quad(out, K_IMAGE | flags, e, a.anchor_start, 0.0, 0.0, w, h, 0.0);
    }

    /// Where a text's glyph-space point lands (the transform of `drawPrimitives`' text branch).
    fn text_place(it: &Item, seg: &Segment, t: &TextAsset) -> impl Fn(f64, f64) -> [f64; 2] {
        let mirror = seg.mirror != it.image_text_flags;
        let start = if seg.reverse { it.end } else { it.start };
        let theta = it.angle + if seg.reverse { PI } else { 0.0 } - if mirror { PI } else { 0.0 };
        let xs = (it.end[0] - it.start[0]).hypot(it.end[1] - it.start[1]) / t.width;
        let sx = if mirror { -xs } else { xs };
        let sy = it.scale;
        let (sin, cos) = theta.sin_cos();
        move |x: f64, y: f64| -> [f64; 2] {
            let (lx, ly) = (sx * x, sy * y);
            [start[0] + cos * lx - sin * ly, start[1] + sin * lx + cos * ly]
        }
    }

    /// A figure outline around text: every visible glyph stroked `width` wide in
    /// glyph space with round joins, in the outline colour (the editor strokes the
    /// glyph paths under the text's transform, so the outline scales with the text).
    fn text_outline(&self, out: &mut Out, it: &Item, seg: &Segment, t: &TextAsset, width: f32, rgb: [u8; 3]) {
        if t.width == 0.0 || t.placements.is_empty() || width <= 0.0 {
            return;
        }
        let place = Self::text_place(it, seg, t);
        let color = rgba8(rgb, 1.0);
        for &(g, dx, dy) in &t.placements {
            let Some(glyph) = t.glyphs.get(g as usize) else { continue };
            let tris = self.assets.glyph_stroke(glyph, width);
            for tr in tris.chunks_exact(3) {
                let p = |v: [f32; 2]| place(v[0] as f64 + dx as f64, v[1] as f64 + dy as f64);
                let (a, b, c) = (p(tr[0]), p(tr[1]), p(tr[2]));
                let bounds = [a[0].min(b[0]).min(c[0]), a[1].min(b[1]).min(c[1]), a[0].max(b[0]).max(c[0]), a[1].max(b[1]).max(c[1])];
                out.push(prim(K_TRI, color, NO_PAINT, [a[0], a[1], b[0], b[1]], [c[0], c[1], 0.0, 0.0], [0.0; 4]), bounds);
            }
        }
    }

    /// Vector text (`drawPrimitives` text branch), even-odd filled glyphs.
    fn text(&self, out: &mut Out, it: &Item, seg: &Segment, t: &TextAsset) {
        if t.width == 0.0 || t.placements.is_empty() {
            return;
        }
        let mirror = seg.mirror != it.image_text_flags;
        let start = if seg.reverse { it.end } else { it.start };
        let theta = it.angle + if seg.reverse { PI } else { 0.0 } - if mirror { PI } else { 0.0 };
        let xs = (it.end[0] - it.start[0]).hypot(it.end[1] - it.start[1]) / t.width;
        let sx = if mirror { -xs } else { xs };
        let sy = it.scale;
        let (sin, cos) = theta.sin_cos();
        let place = |x: f64, y: f64| -> [f64; 2] {
            let (lx, ly) = (sx * x, sy * y);
            [start[0] + cos * lx - sin * ly, start[1] + sin * lx + cos * ly]
        };
        let (color, paint) = if let Some(tint) = self.tint {
            (rgba8(tint, it.alpha), NO_PAINT)
        } else if it.color >> 24 != 255 && seg.secondary.is_some() {
            // Vertical gradient from the baseline to the top of the text.
            let (rgb, a) = seg.secondary.unwrap();
            let stops = [(0.0, stop_color(it.rgb, it.alpha)), (1.0, stop_color(object_color(rgb, it.color), 1.0 - a as f64 / 255.0))];
            let p0 = place(0.0, 0.0);
            let p1 = place(0.0, -t.height);
            (rgba8(it.rgb, it.alpha), out.paint(P_LINEAR, [p0[0] as f32, p0[1] as f32, p1[0] as f32, p1[1] as f32], [0.0; 4], &stops))
        } else {
            (rgba8(it.rgb, it.alpha), NO_PAINT)
        };
        for &(g, dx, dy) in &t.placements {
            let Some(tris) = t.glyphs.get(g as usize).and_then(|glyph| self.assets.glyph(glyph)) else { continue };
            for tr in tris.chunks_exact(3) {
                let p = |v: [f32; 2]| place(v[0] as f64 + dx as f64, v[1] as f64 + dy as f64);
                let (a, b, c) = (p(tr[0]), p(tr[1]), p(tr[2]));
                let bounds = [a[0].min(b[0]).min(c[0]), a[1].min(b[1]).min(c[1]), a[0].max(b[0]).max(c[0]), a[1].max(b[1]).max(c[1])];
                out.push(prim(K_TRI, color, paint, [a[0], a[1], b[0], b[1]], [c[0], c[1], 0.0, 0.0], [0.0; 4]), bounds);
            }
        }
    }
}

impl Ctx<'_> {
    /// A depth run: consecutive depth-tested faces of one figure (`paintDepthSurfaces`).
    /// Their depths are normalized over the run's depth range: each mesh's whole range
    /// and each surface's corner depths, as the editor's raster does.
    fn depth_run(&self, out: &mut Out, items: &[Item]) {
        let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        let (mut z0, mut z1) = (f64::INFINITY, f64::NEG_INFINITY);
        for it in items {
            if it.kind == ItemKind::Mesh {
                let Some(mesh) = self.scene.mesh_of(it) else { continue };
                if let Some(b) = mesh.bounds {
                    (x0, y0, x1, y1) = (x0.min(b[0] as f64), y0.min(b[1] as f64), x1.max(b[2] as f64), y1.max(b[3] as f64));
                    (z0, z1) = (z0.min(mesh.z_min as f64), z1.max(mesh.z_max as f64));
                }
                continue;
            }
            for (p, &z) in self.scene.points_of(it).iter().zip(self.scene.depths_of(it)) {
                (x0, y0, x1, y1) = (x0.min(p[0]), y0.min(p[1]), x1.max(p[0]), y1.max(p[1]));
                (z0, z1) = (z0.min(z), z1.max(z));
            }
        }
        // `if(!(maxX>minX&&maxY>minY))return`: a run without area draws nothing.
        if !(x1 > x0 && y1 > y0) {
            return;
        }
        let (z_min, range) = (z0 as f32, (z1 - z0).max(1e-8) as f32);
        out.push(prim(K_DEPTH_RESET, 0, NO_PAINT, [x0, y0, x1, y1], [0.0; 4], [0.0; 4]), [x0, y0, x1, y1]);
        let face = |out: &mut Out, color: [u8; 3], p: [[f32; 2]; 3], d: [f32; 3]| {
            let bounds = [p[0][0].min(p[1][0]).min(p[2][0]) as f64, p[0][1].min(p[1][1]).min(p[2][1]) as f64, p[0][0].max(p[1][0]).max(p[2][0]) as f64, p[0][1].max(p[1][1]).max(p[2][1]) as f64];
            out.grow(bounds[0], bounds[1], bounds[2], bounds[3]);
            out.prims.push(Prim {
                kind: K_TRI | F_DEPTH,
                group: 0,
                color: rgba8(self.tint.unwrap_or(color), 1.0),
                paint: NO_PAINT,
                a: [p[0][0], p[0][1], p[1][0], p[1][1]],
                b: [p[2][0], p[2][1], d[0], d[1]],
                c: [d[2], z_min, range, 0.0],
            });
        };
        for it in items {
            if it.kind == ItemKind::Mesh {
                let Some(mesh) = self.scene.mesh_of(it) else { continue };
                for t in it.faces.0 as usize..it.faces.1 as usize {
                    let v = [mesh.triangles[t * 3] as usize, mesh.triangles[t * 3 + 1] as usize, mesh.triangles[t * 3 + 2] as usize];
                    let color = mesh.colors.get(mesh.materials[t] as usize).copied().unwrap_or([0, 0, 0]);
                    face(out, color, v.map(|v| [mesh.positions[v * 2], mesh.positions[v * 2 + 1]]), v.map(|v| mesh.depth[v]));
                }
                continue;
            }
            // A face is a triangle, or a quad a near clip made: a fan from its first corner.
            let (points, depths) = (self.scene.points_of(it), self.scene.depths_of(it));
            for k in 1..points.len() - 1 {
                let v = [0, k, k + 1];
                face(out, it.rgb, v.map(|v| [points[v][0] as f32, points[v][1] as f32]), v.map(|v| depths[v] as f32));
            }
        }
    }
}

fn tri(out: &mut Out, color: u32, paint: u32, a: [f64; 2], b: [f64; 2], c: [f64; 2]) {
    let bounds = [a[0].min(b[0]).min(c[0]), a[1].min(b[1]).min(c[1]), a[0].max(b[0]).max(c[0]), a[1].max(b[1]).max(c[1])];
    out.push(prim(K_TRI, color, paint, [a[0], a[1], b[0], b[1]], [c[0], c[1], 0.0, 0.0], [0.0; 4]), bounds);
}

/// Strictly convex simple polygon (same turn direction everywhere, one winding).
fn convex(p: &[[f64; 2]]) -> bool {
    let n = p.len();
    if n < 3 {
        return false;
    }
    let mut sign = 0.0;
    let mut turn = 0.0;
    for i in 0..n {
        let (a, b, c) = (p[i], p[(i + 1) % n], p[(i + 2) % n]);
        let (ux, uy, vx, vy) = (b[0] - a[0], b[1] - a[1], c[0] - b[0], c[1] - b[1]);
        let cross = ux * vy - uy * vx;
        if cross.abs() < 1e-12 {
            continue;
        }
        if sign == 0.0 {
            sign = cross.signum();
        } else if cross.signum() != sign {
            return false;
        }
        turn += (ux * vy - uy * vx).atan2(ux * vx + uy * vy);
    }
    sign != 0.0 && (turn.abs() - 2.0 * PI).abs() < 1e-3
}

/// One object's primitives with its isolation structure.
struct ObjectOut {
    opacity: f64,
    outline_alpha: f64,
    outline: Out,
    items: Out,
    /// Nodes with a stack of their own, filtered on their own layer, each
    /// inserted before item primitive `at` of `items`, in order.
    nodes: Vec<Node>,
    /// The object's own effects (`figure` stack), over its outline and items.
    figure: Option<Box<Fx>>,
    /// Per item: what `objectBounds` reads, and the padding of the object's
    /// own stacks on it (figure and node), for the layers that enclose the
    /// object. Empty when the frame has no effects.
    extents: Vec<(Extent, f64)>,
    /// The object carries effects of its own (figure or node stacks).
    has_fx: bool,
    /// The object holds depth-tested faces, which the editor draws from a raster of
    /// the object's own (`requiresCompatibilityTexture`).
    depth: bool,
    /// `objectBounds` of the object (x, y, width, height), with effect padding;
    /// measured when the frame has effects or depth-tested faces.
    bounds: [f64; 4],
}

/// A node with its own stack.
struct Node {
    at: usize,
    out: Out,
    fx: Fx,
}

/// A stack and the rectangles the editor filters it on.
#[derive(Clone, Debug)]
struct Fx {
    stack: Vec<Arc<Effect>>,
    /// `objectBounds` with effect padding, and without: `[x, y, width, height]`.
    bounds: [f64; 4],
    domain: [f64; 4],
    /// The scale effect sizes are multiplied by.
    scale: f64,
    /// The largest padding of one item.
    padding: f64,
}

impl Fx {
    fn translate(&mut self, d: [f64; 2]) {
        self.bounds[0] += d[0];
        self.bounds[1] += d[1];
        self.domain[0] += d[0];
        self.domain[1] += d[1];
    }
}

/// The background layers (`drawBackground`), as view-filling primitives.
fn background(doc: &Document, assets: &Assets, scene: &Scene, editor_color: Option<u32>, out: &mut Out) {
    // `sceneBackgroundColor`: a hidden Scene layer is transparent; a colour key replaces the fill.
    // The editor's own canvas colour comes before both.
    let scene_hidden = doc.layers.as_ref().is_some_and(|l| !l.scene_visible);
    let key = editor_color.or(if scene_hidden { Some(0) } else { scene.background_color });
    if let Some(argb) = key {
        let everywhere = [f64::NEG_INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::INFINITY];
        out.push(prim(K_FILL_VIEW, argb8(argb), NO_PAINT, [0.0; 4], [0.0; 4], [0.0; 4]), everywhere);
        return;
    }
    let (a, b, mix) = scene.background;
    let (w, h) = (doc.canvas[0] as f64, doc.canvas[1] as f64);
    let mut layer = |id: u32, alpha: f64| {
        let bg = doc.backgrounds.iter().find(|x| x.id == id);
        let fill = |color: u32, paint: u32| prim(K_FILL_VIEW, color, paint, [0.0; 4], [0.0; 4], [0.0; 4]);
        let everywhere = [f64::NEG_INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::INFINITY];
        let with_alpha = |c: u32| -> u32 {
            let a = ((c >> 24) as f64 * alpha).round() as u32;
            (c & 0x00ff_ffff) | a.min(255) << 24
        };
        let argb_stop = |raw: u32| -> [f32; 4] {
            [(raw >> 16 & 255) as f32 / 255.0, (raw >> 8 & 255) as f32 / 255.0, (raw & 255) as f32 / 255.0, ((raw >> 24) as f64 / 255.0 * alpha) as f32]
        };
        match bg.map(|b| &b.kind) {
            None => out.push(fill(with_alpha(0xffff_ffff), NO_PAINT), everywhere),
            Some(BackgroundKind::Solid { argb }) => out.push(fill(with_alpha(argb8(*argb)), NO_PAINT), everywhere),
            Some(BackgroundKind::Linear { a, b, x1, y1, x2, y2 }) => {
                let rec = out.paint(P_LINEAR, [(x1 * w) as f32, (y1 * h) as f32, (x2 * w) as f32, (y2 * h) as f32], [0.0; 4], &[(0.0, argb_stop(*a)), (1.0, argb_stop(*b))]);
                out.push(fill(0, rec), everywhere);
            }
            Some(BackgroundKind::Radial { a, b, x1, y1, x2, y2 }) => {
                let outer = y2 * h;
                if outer <= 0.0 {
                    out.push(fill(with_alpha(argb8(*a)), NO_PAINT), everywhere);
                } else {
                    let inner = (x2 * h).max(0.0).min(0.99 * outer);
                    let rec = out.paint(
                        P_RADIAL_RING,
                        [(x1 * w) as f32, (y1 * h) as f32, inner as f32, outer as f32],
                        [0.0; 4],
                        &[(0.0, argb_stop(*b)), (1.0, argb_stop(*a))],
                    );
                    out.push(fill(0, rec), everywhere);
                }
            }
            Some(BackgroundKind::Paint(p)) => {
                let stops: Vec<(f32, [f32; 4])> = p.stops.iter().map(|s| (s.offset as f32, stop_color(s.rgb, s.opacity * alpha))).collect();
                if stops.len() == 1 || p.kind == PaintKind::Solid {
                    let c = stops[0].1;
                    out.push(fill(rgba8([(c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8], c[3] as f64), NO_PAINT), everywhere);
                } else {
                    let rec = paint_record(out, p, &stops, [0.0, h / 2.0], [w, h / 2.0]);
                    out.push(fill(0, rec), everywhere);
                }
            }
            Some(BackgroundKind::Image { image }) => {
                let Some(e) = image.and_then(|i| assets.image(i)) else { return };
                let (iw, ih) = (e.source_w as f64, e.source_h as f64);
                let s = (w / iw).max(h / ih);
                let uv = e.uv();
                out.push(
                    prim(K_BG_IMAGE, rgba8([255, 255, 255], alpha), NO_PAINT, [iw * s, ih * s, e.layer as f64, 0.0], [0.0; 4], [uv[0] as f64, uv[1] as f64, uv[2] as f64, uv[3] as f64]),
                    everywhere,
                );
            }
        }
    };
    layer(a, 1.0);
    if mix > 0.0 {
        layer(b, mix / 255.0);
    }
}

/// What the editor changes about a frame without changing the document: figures
/// hidden for this session, figures being dragged, and its own canvas colour.
#[derive(Clone, Copy, Debug, Default)]
pub struct Overrides<'a> {
    /// Object slots that are not drawn (`true` = hidden). Shorter than the frame = the rest are shown.
    pub hidden: &'a [bool],
    /// World-space offsets of objects being dragged, by slot.
    pub translations: &'a [(u32, [f64; 2])],
    /// An ARGB colour replacing the scene background (Build mode's canvas colour).
    pub background: Option<u32>,
    /// Draw without effects (the editor's effects preview is off).
    pub bypass_effects: bool,
}

impl Out {
    /// Moves every primitive, paint and polyline vertex by `d`.
    fn translate(&mut self, d: [f64; 2]) {
        let (dx, dy) = (d[0] as f32, d[1] as f32);
        for p in &mut self.prims {
            match p.kind & 255 {
                K_CAPSULE | K_BOX | K_POLYLINE | K_DEPTH_RESET => {
                    p.a = [p.a[0] + dx, p.a[1] + dy, p.a[2] + dx, p.a[3] + dy];
                }
                K_TRI => {
                    p.a = [p.a[0] + dx, p.a[1] + dy, p.a[2] + dx, p.a[3] + dy];
                    p.b[0] += dx;
                    p.b[1] += dy;
                }
                K_ARC | K_DISC | K_RING | K_IMAGE => {
                    p.a[0] += dx;
                    p.a[1] += dy;
                }
                _ => {}
            }
        }
        for point in &mut self.points {
            point[0] += dx;
            point[1] += dy;
        }
        // Paint records: [kind, stops, …], geometry, extra, then two rows per stop.
        let mut at = 0;
        while at + 2 < self.paints.len() {
            let (kind, stops) = (self.paints[at][0], self.paints[at][1] as usize);
            let g = &mut self.paints[at + 1];
            g[0] += dx;
            g[1] += dy;
            if kind == P_LINEAR {
                g[2] += dx;
                g[3] += dy;
            }
            at += 3 + stops * 2;
        }
        if self.bounds[0] <= self.bounds[2] {
            self.bounds = [self.bounds[0] + dx, self.bounds[1] + dy, self.bounds[2] + dx, self.bounds[3] + dy];
        }
    }
}

/// Encodes a scene. Objects encode in parallel; groups are merged in order.
pub fn encode(doc: &Document, assets: &Assets, scene: &Scene) -> DrawList {
    encode_with(doc, assets, scene, &Overrides::default())
}

/// [`encode`] with the editor's session overrides applied.
pub fn encode_with(doc: &Document, assets: &Assets, scene: &Scene, overrides: &Overrides<'_>) -> DrawList {
    encode_composed(doc, assets, scene, overrides, &[], None)
}

/// An onion skin: another tick's scene, drawn faded and optionally in one colour.
#[derive(Clone, Copy)]
pub struct Ghost<'a> {
    pub scene: &'a Scene,
    /// Opacity of the whole ghost frame, applied once after its figures composite.
    pub opacity: f64,
    pub tint: Option<[u8; 3]>,
    /// Object slots of the ghost scene that are not drawn.
    pub hidden: &'a [bool],
}

/// One step of the painter order of a composed frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    /// The object in this slot of the frame being edited.
    Current(u32),
    /// The object in this slot of a ghost.
    Ghost(u32, u32),
}

/// One scene's objects, encoded and ready to be placed in any order.
struct Encoded {
    /// Item ranges of consecutive items of one figure instance (same slot and clip chain).
    runs: Vec<(usize, usize)>,
    objects: Vec<Option<ObjectOut>>,
    /// Run range per object slot.
    of_slot: std::collections::HashMap<u32, (usize, usize)>,
    /// The scene's effects are drawn.
    fx: bool,
}

/// The effects of the object an item belongs to: the frame's own object, or
/// the innermost object of a movieclip chain (from that clip's frame).
fn object_fx<'s>(scene: &'s Scene, it: &Item) -> Option<&'s ObjectVfx> {
    if it.chain == crate::geometry::NO_CHAIN {
        return scene.vfx.as_deref()?.object(it.slot);
    }
    let &(owner, part) = scene.chains[it.chain as usize].last()?;
    scene.clip_vfx.get(owner as usize)?.as_deref()?.object(part)
}

/// A node's own stack (`segmentVfx`): segments have node stacks, polygons do not.
fn node_stack<'s>(fx: &'s ObjectVfx, fig: &Figure, it: &Item) -> Option<&'s [Arc<Effect>]> {
    if fx.segments.is_empty() || matches!(it.kind, ItemKind::Poly | ItemKind::Mesh) || it.flags & crate::geometry::ITEM_SEG_BRUSH != 0 {
        return None;
    }
    let handle = fig.segments.get(it.index as usize)?.id;
    fx.segments.iter().find(|(h, _)| *h == handle).map(|(_, stack)| &stack[..]).filter(|s| !s.is_empty())
}

fn encode_scene(assets: &Assets, scene: &Scene, hidden: &[bool], translations: &[(u32, [f64; 2])], tint: Option<[u8; 3]>, bypass: bool) -> Encoded {
    let ctx = Ctx { scene, assets, tint };
    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut of_slot: std::collections::HashMap<u32, (usize, usize)> = std::collections::HashMap::new();
    let mut i = 0;
    while i < scene.items.len() {
        let (slot, chain) = (scene.items[i].slot, scene.items[i].chain);
        let mut j = i + 1;
        while j < scene.items.len() && scene.items[j].slot == slot && scene.items[j].chain == chain {
            j += 1;
        }
        let entry = of_slot.entry(slot).or_insert((runs.len(), runs.len()));
        entry.1 = runs.len() + 1;
        runs.push((i, j));
        i = j;
    }
    // Translations by slot (drags and parallax layers can move every object of a frame).
    let mut moved: Vec<Option<[f64; 2]>> = Vec::new();
    for &(slot, delta) in translations {
        if moved.len() <= slot as usize {
            moved.resize(slot as usize + 1, None);
        }
        moved[slot as usize] = Some(delta);
    }
    let fx = !bypass && (scene.vfx.is_some() || scene.clip_vfx.iter().any(Option::is_some));
    // Objects with depth-tested faces are drawn on rasters of their own, sized by their bounds.
    let measured = fx || !scene.meshes.is_empty() || !scene.depths.is_empty();
    let encode_run = |&(a, b): &(usize, usize)| -> Option<ObjectOut> {
        let first = &scene.items[a];
        let opacity = first.opacity;
        if opacity <= 0.0 || hidden.get(first.slot as usize).copied().unwrap_or(false) {
            return None;
        }
        let fig = &scene.figures[first.figure as usize];
        let mut outline = Out::new();
        let mut outline_alpha = 1.0;
        if let Some(o) = &fig.outline {
            outline_alpha = o.alpha;
            if outline_alpha > 0.0 {
                for it in &scene.items[a..b] {
                    if it.kind != ItemKind::Image {
                        ctx.item(&mut outline, it, o.width * it.scale.abs(), o.rgb);
                    }
                }
            }
        }
        let own = if fx { object_fx(scene, first) } else { None };
        let figure_stack = own.map_or(&[][..], |o| &o.figure[..]);
        let scale = first.scale.abs();
        let figure_pad = if figure_stack.is_empty() { 0.0 } else { crate::vfx::padding(figure_stack, scale, 0.0) };
        let mut items = Out::new();
        let mut nodes: Vec<Node> = Vec::new();
        let mut extents: Vec<(Extent, f64)> = Vec::new();
        let (mut with, mut without, mut largest) = (EMPTY, EMPTY, 0.0f64);
        let mut run_end = a;
        for (k, it) in scene.items[a..b].iter().enumerate().map(|(k, it)| (a + k, it)) {
            // Consecutive depth-tested faces are drawn as one depth run.
            if k >= run_end && it.depth_surface() {
                run_end = k + scene.items[k..b].iter().take_while(|it| it.depth_surface()).count();
                ctx.depth_run(&mut items, &scene.items[k..run_end]);
            }
            let node = if k < run_end { None } else { own.and_then(|o| node_stack(o, &scene.figures[it.figure as usize], it)) };
            let pad = figure_pad + node.map_or(0.0, |stack| crate::vfx::padding(stack, scale, 0.0));
            let extent = if measured { item_extent(scene, assets, it) } else { None };
            if let Some(stack) = node {
                let mut out = Out::new();
                ctx.item(&mut out, it, 0.0, [0, 0, 0]);
                let (mut bounds, mut domain) = (EMPTY, EMPTY);
                if let Some(e) = &extent {
                    e.include(&mut bounds, pad);
                    e.include(&mut domain, 0.0);
                }
                let fx = Fx { stack: stack.to_vec(), bounds: rounded(bounds), domain: rounded(domain), scale, padding: pad };
                nodes.push(Node { at: items.prims.len(), out, fx });
            } else if k >= run_end {
                ctx.item(&mut items, it, 0.0, [0, 0, 0]);
            }
            if let Some(e) = extent {
                e.include(&mut with, pad);
                e.include(&mut without, 0.0);
                largest = largest.max(pad);
                extents.push((e, pad));
            }
        }
        let has_fx = !figure_stack.is_empty() || !nodes.is_empty();
        let depth = run_end > a;
        let mut bounds = rounded(with);
        let mut figure = has_fx.then(|| Box::new(Fx { stack: figure_stack.to_vec(), bounds, domain: rounded(without), scale, padding: largest }));
        if let Some(delta) = moved.get(first.slot as usize).copied().flatten() {
            outline.translate(delta);
            items.translate(delta);
            for node in &mut nodes {
                node.out.translate(delta);
                node.fx.translate(delta);
            }
            if let Some(f) = &mut figure {
                f.translate(delta);
            }
            for (e, _) in &mut extents {
                e.translate(delta);
            }
            bounds[0] += delta[0];
            bounds[1] += delta[1];
        }
        Some(ObjectOut { opacity, outline_alpha, outline, items, nodes, figure, extents, has_fx, depth, bounds })
    };
    let objects: Vec<Option<ObjectOut>> = crate::par::map(&runs, 16, encode_run);
    Encoded { runs, objects, of_slot, fx }
}

/// A frame (the scene's own, `TOP`) or a movieclip owner's frame.
const TOP: u32 = u32::MAX;

/// One step of the nesting a run of primitives is drawn in, outermost first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Level {
    /// A movieclip instance: the slot it occupies at its level and its owner.
    Owner { key: u32, owner: u32 },
    /// A connected group (`vfxScope`): the frame that holds it (`TOP` or an
    /// owner) and its root's slot in that frame.
    Connected { clip: u32, root: u32 },
}

/// A level with what it applies.
struct Step<'s> {
    level: Level,
    opacity: f64,
    stack: &'s [Arc<Effect>],
    scale: f64,
    /// `vfxPadding` of the stack in world units.
    pad: f64,
}

/// Which connected groups each slot of a frame belongs to, outermost first,
/// built once per frame that has any.
#[derive(Default)]
struct Memberships {
    tables: Vec<(*const FrameVfx, Vec<Vec<u32>>)>,
    /// Absolute world scale of a clip's root object, by (owner, slot).
    scales: std::collections::HashMap<(u32, u32), f64>,
}

impl Memberships {
    fn of(&mut self, fx: &FrameVfx, slot: u32) -> &[u32] {
        if fx.roots.is_empty() {
            return &[];
        }
        let key = fx as *const FrameVfx;
        let at = match self.tables.iter().position(|(k, _)| *k == key) {
            Some(at) => at,
            None => {
                let mut table: Vec<Vec<u32>> = Vec::new();
                // Roots composite smaller groups first: the reverse is outermost first.
                for &root in fx.roots.iter().rev() {
                    let Some(attached) = fx.object(root).and_then(|o| o.attached.as_ref()) else { continue };
                    for &member in &attached.members {
                        if table.len() <= member as usize {
                            table.resize(member as usize + 1, Vec::new());
                        }
                        table[member as usize].push(root);
                    }
                }
                self.tables.push((key, table));
                self.tables.len() - 1
            }
        };
        self.tables[at].1.get(slot as usize).map_or(&[], Vec::as_slice)
    }

    /// The world scale of the object in `slot` of owner `owner`'s clip frame.
    fn clip_scale(&mut self, scene: &Scene, owner: u32, slot: u32) -> f64 {
        *self.scales.entry((owner, slot)).or_insert_with(|| {
            scene
                .items
                .iter()
                .find(|it| it.chain != crate::geometry::NO_CHAIN && scene.chains[it.chain as usize].last() == Some(&(owner, slot)))
                .map_or(1.0, |it| it.scale.abs())
        })
    }
}

/// The levels a run is drawn in: the connected groups of its frame holding it,
/// then each movieclip instance of its chain with the connected groups of
/// that clip's frame holding the next instance (`connectedVfxGeometry` and
/// `buildMovieclipGeometrySteps`).
fn path<'s>(scene: &'s Scene, members: &mut Memberships, it: &Item, hidden: &[bool], fx: bool) -> Vec<Step<'s>> {
    let mut steps: Vec<Step<'s>> = Vec::new();
    let top = if fx { scene.vfx.as_deref() } else { None };
    let connected = |steps: &mut Vec<Step<'s>>, members: &mut Memberships, frame: &'s FrameVfx, clip: u32, slot: u32| {
        let roots: Vec<u32> = members.of(frame, slot).to_vec();
        for root in roots {
            // A group whose owner is hidden is not drawn as a group (only the frame's own figures can be hidden here).
            if clip == TOP && hidden.get(root as usize).copied().unwrap_or(false) {
                continue;
            }
            let Some(attached) = frame.object(root).and_then(|o| o.attached.as_ref()) else { continue };
            let scale = if clip == TOP { scene.objects.get(root as usize).map_or(1.0, |o| o.scale.abs()) } else { members.clip_scale(scene, clip, root) };
            let pad = crate::vfx::padding(&attached.stack, scale, 0.0);
            steps.push(Step { level: Level::Connected { clip, root }, opacity: 1.0, stack: &attached.stack, scale, pad });
        }
    };
    if let Some(frame) = top {
        connected(&mut steps, members, frame, TOP, it.slot);
    }
    if it.chain != crate::geometry::NO_CHAIN {
        let chain = &scene.chains[it.chain as usize];
        let (mut above, mut above_slot) = (top, it.slot);
        for (level, &(owner, part)) in chain.iter().enumerate() {
            let key = if level == 0 { it.slot } else { chain[level - 1].1 };
            let stack = above.and_then(|f| f.object(above_slot)).map_or(&[][..], |o| &o.figure[..]);
            let scale = scene.owner_scales.get(owner as usize).copied().unwrap_or(1.0);
            let pad = if stack.is_empty() { 0.0 } else { crate::vfx::padding(stack, scale, 0.0) };
            steps.push(Step { level: Level::Owner { key, owner }, opacity: scene.owners[owner as usize], stack, scale, pad });
            let inner = if fx { scene.clip_vfx.get(owner as usize).and_then(|v| v.as_deref()) } else { None };
            if let Some(frame) = inner {
                connected(&mut steps, members, frame, owner, part);
            }
            above = inner;
            above_slot = part;
        }
    }
    steps
}

/// An open level while placing.
struct Open {
    level: Level,
    /// Group receiving this level's content (its own group, or the parent's).
    target: usize,
    isolated: bool,
    skipped: bool,
    opacity: f64,
    /// The first run inside it, and its position in that run's path.
    first: usize,
    index: usize,
    /// It has a layer whose rectangle is measured when it closes.
    measured: bool,
}

/// Places the runs `range` of an encoded scene under `base` (the root, a ghost's
/// group or the camera image), opening and closing the groups of movieclip
/// owners and connected effect groups along the way.
fn place(b: &mut Builder, scene: &Scene, encoded: &Encoded, range: (usize, usize), base: usize, hidden: &[bool]) {
    let mut members = Memberships::default();
    let steps: Vec<Vec<Step<'_>>> = (range.0..range.1).map(|k| path(scene, &mut members, &scene.items[encoded.runs[k].0], hidden, encoded.fx)).collect();
    let steps_of = |k: usize| &steps[k - range.0];
    // Top-level entries: runs sharing their outermost level, or one object. An
    // entry holding effects anywhere is drawn on its own raster, like the editor's.
    let mut entry_fx = vec![false; range.1 - range.0];
    let mut connected_domains: std::collections::HashMap<Level, [f64; 4]> = std::collections::HashMap::new();
    if encoded.fx {
        let mut start = range.0;
        while start < range.1 {
            let mut end = start + 1;
            if let Some(first) = steps_of(start).first() {
                while end < range.1 && steps_of(end).first().is_some_and(|s| s.level == first.level) {
                    end += 1;
                }
            }
            let any = (start..end).any(|k| encoded.objects[k].as_ref().is_some_and(|o| o.has_fx || steps_of(k).iter().any(|s| !s.stack.is_empty())));
            entry_fx[start - range.0..end - range.0].fill(any);
            start = end;
        }
        // Connected runs share one domain: every drawn member's artwork.
        for k in range.0..range.1 {
            let Some(o) = encoded.objects[k].as_ref() else { continue };
            for s in steps_of(k).iter().filter(|s| matches!(s.level, Level::Connected { .. })) {
                let acc = connected_domains.entry(s.level).or_insert(EMPTY);
                for (e, _) in &o.extents {
                    e.include(acc, 0.0);
                }
            }
        }
    }
    // An entry without effects that holds depth-tested faces is drawn on a raster of
    // its own too (`requiresCompatibilityTexture`): docs/engine/spatial.md.
    let mut entry_depth = vec![false; range.1 - range.0];
    let mut start = range.0;
    while start < range.1 {
        let mut end = start + 1;
        if let Some(first) = steps_of(start).first() {
            while end < range.1 && steps_of(end).first().is_some_and(|s| s.level == first.level) {
                end += 1;
            }
        }
        let any = !entry_fx[start - range.0] && (start..end).any(|k| encoded.objects[k].as_ref().is_some_and(|o| o.depth));
        entry_depth[start - range.0..end - range.0].fill(any);
        start = end;
    }
    let measure = |b: &mut Builder, open: &Open, end: usize| {
        let (mut with, mut without, mut largest) = (EMPTY, EMPTY, 0.0f64);
        for k in open.first..end {
            let Some(o) = encoded.objects[k].as_ref() else { continue };
            let outer: f64 = steps_of(k).get(open.index..).map_or(0.0, |s| s.iter().map(|s| s.pad).sum());
            for (e, own) in &o.extents {
                e.include(&mut with, outer + own);
                e.include(&mut without, 0.0);
                largest = largest.max(outer + own);
            }
        }
        let domain = match open.level {
            Level::Connected { .. } => connected_domains.get(&open.level).copied().unwrap_or(without),
            Level::Owner { .. } => without,
        };
        b.measure(open.target, rounded(with), rounded(domain), largest);
    };
    let mut stack: Vec<Open> = Vec::new();
    for k in range.0..range.1 {
        let path = steps_of(k);
        let common = stack.iter().zip(path.iter()).take_while(|(o, s)| o.level == s.level).count();
        while stack.len() > common {
            let open = stack.pop().unwrap();
            if open.measured {
                measure(b, &open, k);
            }
            b.close(open.target, open.opacity, open.isolated);
        }
        for (index, s) in path.iter().enumerate().skip(common) {
            let parent = stack.last().map(|o| o.target).unwrap_or(base);
            let skipped = stack.last().is_some_and(|o| o.skipped) || s.opacity <= 0.0;
            let entry = index == 0 && entry_fx[k - range.0];
            let raster = index == 0 && entry_depth[k - range.0];
            let filtered = !s.stack.is_empty();
            let (target, measured) = if skipped || !(s.opacity < 1.0 || filtered || entry || raster) {
                (parent, false)
            } else if raster && !filtered {
                (b.open_layer(parent, s.opacity, Vec::new(), LayerKind::Raster { bounds: [0.0; 4] }), true)
            } else if entry {
                (b.open_layer(parent, s.opacity, s.stack.to_vec(), LayerKind::Entry { bounds: [0.0; 4], domain: [0.0; 4], scale: s.scale, padding: 0.0 }), true)
            } else if filtered {
                (b.open_layer(parent, s.opacity, s.stack.to_vec(), LayerKind::Nested { bounds: [0.0; 4], domain: [0.0; 4], scale: s.scale }), true)
            } else {
                (b.open(parent, s.opacity), false)
            };
            stack.push(Open { level: s.level, target, isolated: target != parent, skipped, opacity: s.opacity, first: k, index, measured });
        }
        if stack.last().is_some_and(|o| o.skipped) {
            continue;
        }
        let Some(o) = encoded.objects[k].as_ref() else { continue };
        let parent = stack.last().map(|o| o.target).unwrap_or(base);
        b.object(parent, o, path.is_empty() && entry_fx[k - range.0], path.is_empty() && entry_depth[k - range.0]);
    }
    while let Some(open) = stack.pop() {
        if open.measured {
            measure(b, &open, range.1);
        }
        b.close(open.target, open.opacity, open.isolated);
    }
}

/// The scene's camera as the camera image applies it (`effectiveCamera`).
fn scene_camera(doc: &Document, scene: &Scene) -> Option<CameraPose> {
    if doc.layers.as_ref().is_some_and(|l| !l.camera_visible) {
        return None;
    }
    scene.camera.map(|c| CameraPose { x: c.x, y: c.y, scale: c.scale, angle: c.angle, is_default: c.is_default })
}

/// The effects of a layer as the encoder resolved them ([`Layer::effects`]).
pub fn layer_effects(layer: &EffectLayer) -> &[Arc<Effect>] {
    layer.effects.as_ref().and_then(|e| e.downcast_ref::<Vec<Arc<Effect>>>()).map_or(&[], Vec::as_slice)
}

/// Encodes the frame being edited together with its onion skins.
///
/// `order` is the painter order (see [`crate::onion::compose`]); `None` draws the
/// scene alone, in its own order. Consecutive steps of one ghost form one isolation
/// group, faded once at the ghost's opacity, so a ghost never shows through itself.
/// With a scene effect stack the camera image is drawn over the frame and the
/// onion skins over the camera image, as the editor's overlays.
pub fn encode_composed(doc: &Document, assets: &Assets, scene: &Scene, overrides: &Overrides<'_>, ghosts: &[Ghost<'_>], order: Option<&[Layer]>) -> DrawList {
    let bypass = overrides.bypass_effects;
    let current = encode_scene(assets, scene, overrides.hidden, overrides.translations, None, bypass);
    let ghost_scenes: Vec<Encoded> = ghosts.iter().map(|ghost| encode_scene(assets, ghost.scene, ghost.hidden, &[], ghost.tint, bypass)).collect();
    let mut b = Builder::new();
    let mut root = Out::new();
    background(doc, assets, scene, overrides.background, &mut root);
    let background_prims = root.prims.len() as u32;
    b.add(0, root);
    let camera_stack = if bypass { None } else { scene.vfx.as_ref().map(|v| &v.scene).filter(|s| !s.is_empty()) };
    let whole = (0, current.runs.len());
    let mut deferred: Vec<(u32, u32)> = Vec::new();
    match order {
        None => place(&mut b, scene, &current, whole, 0, overrides.hidden),
        Some(order) => {
            // The ghost whose group is open, and that group.
            let mut open: Option<(u32, usize)> = None;
            // Consecutive steps of the current frame are placed together (a connected run spans them).
            let mut pending: Option<(usize, usize)> = None;
            let flush = |b: &mut Builder, pending: &mut Option<(usize, usize)>| {
                if let Some(range) = pending.take() {
                    place(b, scene, &current, range, 0, overrides.hidden);
                }
            };
            for layer in order {
                match *layer {
                    Layer::Current(slot) => {
                        close_ghost(&mut b, &mut open, ghosts);
                        if let Some(&range) = current.of_slot.get(&slot) {
                            match &mut pending {
                                Some((_, end)) if *end == range.0 => *end = range.1,
                                _ => {
                                    flush(&mut b, &mut pending);
                                    pending = Some(range);
                                }
                            }
                        }
                    }
                    Layer::Ghost(ghost, slot) => {
                        if camera_stack.is_some() {
                            deferred.push((ghost, slot));
                            continue;
                        }
                        flush(&mut b, &mut pending);
                        place_ghost(&mut b, &mut open, ghosts, &ghost_scenes, ghost, slot);
                    }
                }
            }
            flush(&mut b, &mut pending);
            close_ghost(&mut b, &mut open, ghosts);
        }
    }
    if let Some(stack) = camera_stack {
        let camera = b.open_layer(0, 1.0, stack.clone(), LayerKind::Camera { canvas: doc.canvas, camera: scene_camera(doc, scene) });
        let mut fill = Out::new();
        background(doc, assets, scene, None, &mut fill);
        b.add(camera, fill);
        place(&mut b, scene, &current, whole, camera, overrides.hidden);
        b.close(camera, 1.0, true);
        let mut open: Option<(u32, usize)> = None;
        for &(ghost, slot) in &deferred {
            place_ghost(&mut b, &mut open, ghosts, &ghost_scenes, ghost, slot);
        }
        close_ghost(&mut b, &mut open, ghosts);
    }
    let (prims, paints, points, groups, passes, layers) = b.finish();
    DrawList { prims, paints, points, groups, passes, background_prims, layers }
}

fn close_ghost(b: &mut Builder, open: &mut Option<(u32, usize)>, ghosts: &[Ghost<'_>]) {
    if let Some((ghost, target)) = open.take() {
        b.close(target, ghosts[ghost as usize].opacity, true);
    }
}

/// One ghost object, in the ghost's group (opened when another ghost's was).
fn place_ghost(b: &mut Builder, open: &mut Option<(u32, usize)>, ghosts: &[Ghost<'_>], scenes: &[Encoded], ghost: u32, slot: u32) {
    let Some(source) = ghosts.get(ghost as usize) else { return };
    if source.opacity <= 0.0 {
        return;
    }
    let Some(&range) = scenes[ghost as usize].of_slot.get(&slot) else { return };
    if open.map(|(index, _)| index) != Some(ghost) {
        close_ghost(b, open, ghosts);
        *open = Some((ghost, b.open(0, source.opacity)));
    }
    place(b, source.scene, &scenes[ghost as usize], range, open.expect("opened above").1, source.hidden);
}

/// Collects prims per isolation group, then orders them deepest first.
struct Builder {
    groups: Vec<Group>,
    prims: Vec<Vec<Prim>>,
    paints: Vec<[f32; 4]>,
    points: Vec<[f32; 2]>,
    layers: Vec<EffectLayer>,
}

impl Builder {
    fn new() -> Builder {
        let empty = [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY];
        Builder {
            groups: vec![Group { parent: 0, depth: 0, opacity: 1.0, bounds: empty, layer: NO_LAYER }],
            prims: vec![Vec::new()],
            paints: Vec::new(),
            points: Vec::new(),
            layers: Vec::new(),
        }
    }

    fn open(&mut self, parent: usize, opacity: f64) -> usize {
        let id = self.groups.len();
        let empty = [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY];
        self.groups.push(Group { parent: parent as u32, depth: self.groups[parent].depth + 1, opacity: opacity as f32, bounds: empty, layer: NO_LAYER });
        self.prims.push(Vec::new());
        id
    }

    /// A group drawn on its own raster and filtered by `stack` (when it holds any effect).
    fn open_layer(&mut self, parent: usize, opacity: f64, stack: Vec<Arc<Effect>>, kind: LayerKind) -> usize {
        let id = self.open(parent, opacity);
        self.groups[id].layer = self.layers.len() as u32;
        let effects = (!stack.is_empty()).then(|| Arc::new(stack) as Effects);
        self.layers.push(EffectLayer { effects, kind });
        id
    }

    /// Sets the rectangles of a layer whose content is known only once it closes.
    fn measure(&mut self, group: usize, bounds: [f64; 4], domain: [f64; 4], largest: f64) {
        let Some(layer) = self.layers.get_mut(self.groups[group].layer as usize) else { return };
        match &mut layer.kind {
            LayerKind::Entry { bounds: b, domain: d, padding, .. } => {
                *b = bounds;
                *d = domain;
                *padding = largest;
            }
            LayerKind::Nested { bounds: b, domain: d, .. } => {
                *b = bounds;
                *d = domain;
            }
            LayerKind::Raster { bounds: b } => *b = bounds,
            LayerKind::Camera { .. } => {}
        }
    }

    /// Finishes a group: its composite lands at this point of the parent's order.
    /// A filtered group covers its layer's rectangle, effects reaching beyond its artwork.
    fn close(&mut self, target: usize, opacity: f64, isolated: bool) {
        if !isolated {
            return;
        }
        let g = self.groups[target];
        if g.bounds[0] > g.bounds[2] {
            return;
        }
        let bounds = match self.layers.get(g.layer as usize).map(|l| l.kind) {
            Some(LayerKind::Entry { bounds: r, .. } | LayerKind::Nested { bounds: r, .. } | LayerKind::Raster { bounds: r }) => [r[0] as f32, r[1] as f32, (r[0] + r[2]) as f32, (r[1] + r[3]) as f32],
            _ => g.bounds,
        };
        self.groups[target].bounds = bounds;
        let parent = g.parent as usize;
        self.prims[parent].push(composite(target as u32, opacity, bounds));
        self.grow(parent, bounds);
    }

    fn grow(&mut self, group: usize, b: [f32; 4]) {
        let g = &mut self.groups[group].bounds;
        *g = union(*g, b);
    }

    fn add(&mut self, target: usize, out: Out) {
        self.add_ref(target, &out);
    }

    fn add_ref(&mut self, target: usize, out: &Out) {
        let (paint_base, point_base) = self.tables(out);
        self.add_part(target, out, 0..out.prims.len(), paint_base, point_base);
    }

    /// Appends the paint and polyline tables of `out`; returns where they start.
    fn tables(&mut self, out: &Out) -> (u32, u32) {
        let base = (self.paints.len() as u32, self.points.len() as u32);
        self.paints.extend_from_slice(&out.paints);
        self.points.extend_from_slice(&out.points);
        base
    }

    /// Adds primitives `range` of `out`, whose tables start at the given bases.
    fn add_part(&mut self, target: usize, out: &Out, range: std::ops::Range<usize>, paint_base: u32, point_base: u32) {
        // A part of an object grows its group by the whole object's bounds, which cover it.
        if (!range.is_empty() || out.prims.is_empty()) && out.bounds[0] <= out.bounds[2] {
            self.grow(target, out.bounds);
        }
        for p in &out.prims[range] {
            let mut p = *p;
            if p.paint != NO_PAINT {
                p.paint += paint_base;
            }
            if p.kind & 255 == K_POLYLINE {
                p.b[1] = f32::from_bits(p.b[1].to_bits() + point_base);
            }
            self.prims[target].push(p);
        }
    }

    /// One figure instance: object opacity layer (with its own effects), outline
    /// layer, then its items with filtered nodes in their place. An `entry`
    /// holding effects is drawn on a raster of its own even when opaque, and so is
    /// a `raster` entry, which holds depth-tested faces.
    fn object(&mut self, parent: usize, o: &ObjectOut, entry: bool, raster: bool) {
        let fx = o.figure.as_deref();
        let filtered = fx.is_some_and(|f| !f.stack.is_empty());
        let layered = filtered || (entry && fx.is_some());
        let raster = raster && !layered;
        let isolate = o.opacity < 1.0 || layered || raster;
        let target = match fx {
            Some(f) if layered => {
                let kind = if entry {
                    LayerKind::Entry { bounds: f.bounds, domain: f.domain, scale: f.scale, padding: f.padding }
                } else {
                    LayerKind::Nested { bounds: f.bounds, domain: f.domain, scale: f.scale }
                };
                self.open_layer(parent, o.opacity, f.stack.clone(), kind)
            }
            _ if raster => self.open_layer(parent, o.opacity, Vec::new(), LayerKind::Raster { bounds: o.bounds }),
            _ if isolate => self.open(parent, o.opacity),
            _ => parent,
        };
        if !o.outline.prims.is_empty() {
            if o.outline_alpha < 1.0 {
                let id = self.open(target, o.outline_alpha);
                self.add_ref(id, &o.outline);
                self.close(id, o.outline_alpha, true);
            } else {
                self.add_ref(target, &o.outline);
            }
        }
        if o.nodes.is_empty() {
            self.add_ref(target, &o.items);
        } else {
            let (paint_base, point_base) = self.tables(&o.items);
            let mut from = 0;
            for node in &o.nodes {
                self.add_part(target, &o.items, from..node.at, paint_base, point_base);
                let f = &node.fx;
                let id = self.open_layer(target, 1.0, f.stack.clone(), LayerKind::Nested { bounds: f.bounds, domain: f.domain, scale: f.scale });
                self.add_ref(id, &node.out);
                self.close(id, 1.0, true);
                from = node.at;
            }
            self.add_part(target, &o.items, from..o.items.prims.len(), paint_base, point_base);
        }
        if isolate {
            self.close(target, o.opacity, true);
        }
    }

    #[allow(clippy::type_complexity)]
    fn finish(mut self) -> (Vec<Prim>, Vec<[f32; 4]>, Vec<[f32; 2]>, Vec<Group>, Vec<Pass>, Vec<EffectLayer>) {
        let max_depth = self.groups.iter().map(|g| g.depth).max().unwrap_or(0);
        let mut prims = Vec::with_capacity(self.prims.iter().map(Vec::len).sum());
        let mut passes = Vec::new();
        for depth in (0..=max_depth).rev() {
            let start = prims.len() as u32;
            for (gi, list) in self.prims.iter_mut().enumerate() {
                if self.groups[gi].depth != depth {
                    continue;
                }
                for mut p in list.drain(..) {
                    p.group = gi as u32;
                    prims.push(p);
                }
            }
            passes.push(Pass { depth, start, end: prims.len() as u32 });
        }
        (prims, self.paints, self.points, self.groups, passes, self.layers)
    }
}

fn union(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]
}

fn composite(group: u32, opacity: f64, bounds: [f32; 4]) -> Prim {
    Prim {
        kind: K_COMPOSITE,
        group: 0,
        color: rgba8([255, 255, 255], opacity),
        paint: NO_PAINT,
        a: bounds,
        b: [f32::from_bits(group), 0.0, 0.0, 0.0],
        c: [0.0; 4],
    }
}

