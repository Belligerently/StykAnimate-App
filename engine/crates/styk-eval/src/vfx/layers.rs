//! The rectangles effects are filtered on, as the editor computes them
//! (`objectBounds` in src/renderer-geometry.mjs with `drawableVfxPadding` from
//! src/vfx-canvas.mjs). A layer's pixel grid, and the coordinates spatial
//! effects read (noise cells, gradient positions, the downsampled blur grid),
//! are anchored to these rectangles, so they are computed exactly as the editor
//! does: integer world units, three units beyond every stroke's half width and
//! outline, plus the reach of the effects that apply.

use crate::assets::Assets;
use crate::geometry::{Item, ItemKind, Scene};

/// What `objectBounds` reads from one item: the box around the points it
/// includes, and the padding it adds to each of them before any effect.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Extent {
    /// Smallest and largest x and y of the points (x0, y0, x1, y1).
    pub points: [f64; 4],
    /// Half the stroke width, the figure outline and three units.
    pub pad: f64,
}

/// An accumulating box; empty until something is included.
pub const EMPTY: [f64; 4] = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];

impl Extent {
    /// Adds this item to `acc` with `extra` effect padding.
    pub fn include(&self, acc: &mut [f64; 4], extra: f64) {
        let pad = self.pad + extra;
        acc[0] = acc[0].min(self.points[0] - pad);
        acc[1] = acc[1].min(self.points[1] - pad);
        acc[2] = acc[2].max(self.points[2] + pad);
        acc[3] = acc[3].max(self.points[3] + pad);
    }

    /// Moves the item by `d` (a dragged figure).
    pub fn translate(&mut self, d: [f64; 2]) {
        self.points = [self.points[0] + d[0], self.points[1] + d[1], self.points[2] + d[0], self.points[3] + d[1]];
    }
}

/// `objectBounds`' rectangle of an accumulated box: `[x, y, width, height]`
/// in whole units, at least one wide; `{0, 0, 1, 1}` when empty.
pub fn rounded(acc: [f64; 4]) -> [f64; 4] {
    if !acc[0].is_finite() || !acc[1].is_finite() || !acc[2].is_finite() || !acc[3].is_finite() {
        return [0.0, 0.0, 1.0, 1.0];
    }
    let (x, y) = (acc[0].floor(), acc[1].floor());
    [x, y, (acc[2].ceil() - x).max(1.0), (acc[3].ceil() - y).max(1.0)]
}

/// The item's contribution to `objectBounds`, or `None` for what it skips
/// (zero-width construction lines without an outline).
pub fn item_extent(scene: &Scene, assets: &Assets, it: &Item) -> Option<Extent> {
    let fig = &scene.figures[it.figure as usize];
    let outline = fig.outline.as_ref().map_or(0.0, |o| (o.width * it.scale).abs());
    let line = matches!(it.kind, ItemKind::Line | ItemKind::SquareLine);
    if line && it.thickness == 0.0 && outline == 0.0 {
        return None;
    }
    let pad = it.thickness.max(0.0) / 2.0 + outline + 3.0;
    let mut b = EMPTY;
    let mut add = |p: [f64; 2]| {
        if p[0].is_finite() && p[1].is_finite() {
            b = [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])];
        }
    };
    match it.kind {
        ItemKind::Circle | ItemKind::CircleFill | ItemKind::CircleClear => {
            let c = [(it.start[0] + it.end[0]) / 2.0, (it.start[1] + it.end[1]) / 2.0];
            let r = it.length.abs() / 2.0;
            add([c[0] - r, c[1] - r]);
            add([c[0] + r, c[1] + r]);
        }
        ItemKind::Poly => scene.points_of(it).iter().for_each(|&p| add(p)),
        // `objectBounds` takes the whole mesh's visible bounds for each of its drawables.
        ItemKind::Mesh => {
            if let Some(b) = scene.mesh_of(it).and_then(|mesh| mesh.bounds) {
                add([b[0] as f64, b[1] as f64]);
                add([b[2] as f64, b[3] as f64]);
            }
        }
        _ => {
            // A segment's points: its sampled arc, or its two ends.
            let points = scene.points_of(it);
            if points.len() >= 2 {
                points.iter().for_each(|&p| add(p));
            } else {
                add(it.start);
                add(it.end);
            }
            let seg = &fig.segments[it.index as usize];
            if it.kind == ItemKind::Image
                && let Some(a) = fig.image(seg.asset)
            {
                image_corners(it, seg, a, assets, &mut add);
            }
            if it.kind == ItemKind::Text
                && let Some(t) = fig.text(seg.asset)
            {
                text_corners(it, seg, t, &mut add);
            }
        }
    }
    b[0].is_finite().then_some(Extent { points: b, pad })
}

/// The corners of a sprite or drawing cel, placed as `drawPrimitives` places it.
fn image_corners(it: &Item, seg: &crate::doc::Segment, a: &crate::doc::ImageAsset, assets: &Assets, add: &mut impl FnMut([f64; 2])) {
    let ([bx, by], [w, h]) = match (&a.tiles, a.image.and_then(|i| assets.image(i))) {
        // A drawing cel: its level-0 tiles, in cel pixels.
        (Some(tiles), _) if !tiles.tiles.is_empty() => {
            let t = crate::drawing::TILE as f64;
            let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
            for &(tx, ty, _) in &tiles.tiles {
                let (x, y) = (tx as f64 * t, ty as f64 * t);
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + t);
                y1 = y1.max(y + t);
            }
            ([x0, y0], [x1 - x0, y1 - y0])
        }
        (_, image) => ([0.0, 0.0], a.display_size.unwrap_or_else(|| image.map_or([1.0, 1.0], |e| [e.source_w as f64, e.source_h as f64]))),
    };
    let mirror = seg.mirror != it.image_text_flags;
    let start = if seg.reverse { it.end } else { it.start };
    let angle = (if mirror { it.angle + a.anchor_angle - std::f64::consts::PI } else { it.angle - a.anchor_angle }) + if seg.reverse { std::f64::consts::PI } else { 0.0 };
    let length = if a.anchor_length != 0.0 { a.anchor_length } else { 1.0 };
    let scale = (it.end[0] - it.start[0]).hypot(it.end[1] - it.start[1]) / length;
    let (sin, cos) = angle.sin_cos();
    for [x, y] in [[bx, by], [bx + w, by], [bx + w, by + h], [bx, by + h]] {
        let px = (x - a.anchor_start[0]) * scale * if mirror { -1.0 } else { 1.0 };
        let py = (y - a.anchor_start[1]) * scale;
        add([start[0] + px * cos - py * sin, start[1] + px * sin + py * cos]);
    }
}

/// The corners of a text's glyphs and of its box (glyph boxes from their outlines).
fn text_corners(it: &Item, seg: &crate::doc::Segment, t: &crate::doc::TextAsset, add: &mut impl FnMut([f64; 2])) {
    let mirror = seg.mirror != it.image_text_flags;
    let start = if seg.reverse { it.end } else { it.start };
    let angle = it.angle + if seg.reverse { std::f64::consts::PI } else { 0.0 } - if mirror { std::f64::consts::PI } else { 0.0 };
    let width = if t.width != 0.0 { t.width } else { 1.0 };
    let xs = (it.end[0] - it.start[0]).hypot(it.end[1] - it.start[1]) / width * if mirror { -1.0 } else { 1.0 };
    let ys = it.scale;
    let (sin, cos) = angle.sin_cos();
    let mut place = |x: f64, y: f64| add([start[0] + x * xs * cos - y * ys * sin, start[1] + x * xs * sin + y * ys * cos]);
    for &(g, dx, dy) in &t.placements {
        let Some(glyph) = t.glyphs.get(g as usize) else { continue };
        let (mut x0, mut y0, mut x1, mut y1) = (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
        let mut point = |x: f32, y: f32| {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        };
        for cmd in &glyph.path {
            match *cmd {
                crate::doc::PathCmd::Move(x, y) | crate::doc::PathCmd::Line(x, y) => point(x, y),
                crate::doc::PathCmd::Cubic(a, b, c, d, e, f) => {
                    point(a, b);
                    point(c, d);
                    point(e, f);
                }
                crate::doc::PathCmd::Close => {}
            }
        }
        if x0.is_finite() {
            for (x, y) in [(x0, y0), (x1, y0), (x0, y1), (x1, y1)] {
                place((x + dx) as f64, (y + dy) as f64);
            }
        }
    }
    for (x, y) in [(0.0, -t.height), (t.width, -t.height), (0.0, 0.0), (t.width, 0.0)] {
        place(x, y);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounding_matches_object_bounds() {
        assert_eq!(rounded([10.2, -3.5, 20.1, 4.0]), [10.0, -4.0, 11.0, 8.0]);
        assert_eq!(rounded([1.0, 1.0, 1.0, 1.0]), [1.0, 1.0, 1.0, 1.0], "at least one unit wide");
        assert_eq!(rounded(EMPTY), [0.0, 0.0, 1.0, 1.0]);
    }

    #[test]
    fn padding_adds_to_every_point() {
        let e = Extent { points: [0.0, 0.0, 10.0, 5.0], pad: 4.0 };
        let mut acc = EMPTY;
        e.include(&mut acc, 6.5);
        assert_eq!(acc, [-10.5, -10.5, 20.5, 15.5]);
    }
}
