//! CPU rasterizer for StykAnimate draw lists ([tiny-skia](https://docs.rs/tiny-skia)).
//!
//! The same draw list the GPU renderer (`styk-render`) draws, rasterized on the
//! CPU: for exports, thumbnails, library previews and devices with neither
//! WebGPU nor WebGL2. tiny-skia is a port of Skia's raster backend, the one
//! Canvas 2D uses, so strokes, fills and their antialiasing follow the editor's
//! Canvas 2D output closely.
//!
//! * Every primitive kind of `styk_eval::drawlist` maps to a tiny-skia path
//!   operation in world units under the view transform.
//! * Isolation groups render into their own pixmap and composite with their
//!   opacity at their place in the parent's painter order.
//! * Custom paints (linear, radial, angular, diamond gradients) are shaded per
//!   pixel through a coverage mask with `styk_draw::sample_paint`, the same
//!   arithmetic as the GPU shader, so both renderers agree on colours.
//! * Consecutive triangles of one colour fill as one path, so glyphs and
//!   polygon fans show no seams between their triangles.
//! * Drawing cels show one level per cel by the same rule as the GPU
//!   (`floor(-log2(pixels per cel pixel))`, 0–8).
//! * Depth runs (3D faces, `K_DEPTH_RESET` then `K_TRI` with `F_DEPTH`) are
//!   rasterized with a depth buffer of four samples per pixel at the GPU's
//!   standard sample positions, resolved, then composited at their place in
//!   painter order, as the GPU draws them.

use std::collections::HashMap;
use std::f64::consts::PI;

use styk_eval::assets::{Assets, DecodedImage, PAGE};
use styk_draw::*;
use tiny_skia::{
    Color, FillRule, FilterQuality, LineCap, LineJoin, Mask, Paint, Path, PathBuilder, Pattern, Pixmap, PixmapPaint, Rect, SpreadMode, Stroke,
    Transform,
};

/// How a frame is rasterized.
#[derive(Clone, Debug)]
pub struct View {
    /// World → output pixels: x' = a·x + c·y + e, y' = b·x + d·y + f.
    pub matrix: [f64; 6],
    pub width: u32,
    pub height: u32,
    /// Output-space artboard rectangle (x0, y0, x1, y1).
    pub artboard: [f64; 4],
    /// Clip everything but the background to the artboard.
    pub clip_to_artboard: bool,
    /// Straight RGBA the frame starts from.
    pub clear: [f32; 4],
    /// Antialiased edges (export "smooth edges"); `false` is the aliased export.
    pub anti_alias: bool,
}

/// Decoded pixels of a document's images, keyed by their atlas place (the draw
/// list addresses images by atlas rectangle, as the GPU samples them).
#[derive(Default)]
pub struct Images {
    /// Premultiplied sprites without their atlas gutter.
    sprites: HashMap<(u32, u32, u32), Pixmap>,
    /// Drawing cel tiles: premultiplied, with their one-texel border of neighbouring texels.
    tiles: HashMap<(u32, u32, u32), Pixmap>,
}

impl Images {
    /// Adds a decoded image (`styk_eval::assets::decode_image` / `decode_cel`).
    pub fn add(&mut self, assets: &Assets, image: &DecodedImage) {
        let entry = if image.drawing { assets.drawing.get(image.index as usize) } else { assets.atlas.get(image.index as usize) };
        let Some(e) = entry.filter(|e| e.valid) else { return };
        let key = (e.layer, e.x, e.y);
        if image.drawing {
            let mut rgba = image.rgba.clone();
            for texel in rgba.chunks_exact_mut(4) {
                let a = texel[3] as u32;
                for c in &mut texel[..3] {
                    *c = ((*c as u32 * a + 127) / 255) as u8;
                }
            }
            if let Some(pixmap) = pixmap_from(image.width, image.height, rgba) {
                self.tiles.insert(key, pixmap);
            }
            return;
        }
        // Sprites are stored with a one-texel gutter repeating their edges: keep the inside.
        let (w, h) = (image.width.saturating_sub(2), image.height.saturating_sub(2));
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for y in 1..=h {
            let row = ((y * image.width + 1) * 4) as usize;
            rgba.extend_from_slice(&image.rgba[row..row + (w * 4) as usize]);
        }
        if let Some(pixmap) = pixmap_from(w, h, rgba) {
            self.sprites.insert(key, pixmap);
        }
    }

    pub fn len(&self) -> usize {
        self.sprites.len() + self.tiles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn pixmap_from(width: u32, height: u32, rgba: Vec<u8>) -> Option<Pixmap> {
    Pixmap::from_vec(rgba, tiny_skia::IntSize::from_wh(width.max(1), height.max(1))?)
}

/// The atlas place a primitive's uv rectangle starts at.
fn atlas_key(layer: f32, uv: [f32; 4]) -> (u32, u32, u32) {
    let p = PAGE as f32;
    (layer.round() as u32, (uv[0] * p).round() as u32, (uv[1] * p).round() as u32)
}

fn unpack(color: u32) -> [f32; 4] {
    [(color & 255) as f32 / 255.0, (color >> 8 & 255) as f32 / 255.0, (color >> 16 & 255) as f32 / 255.0, (color >> 24) as f32 / 255.0]
}

fn solid(color: [f32; 4]) -> Color {
    Color::from_rgba(color[0].clamp(0.0, 1.0), color[1].clamp(0.0, 1.0), color[2].clamp(0.0, 1.0), color[3].clamp(0.0, 1.0)).unwrap_or(Color::TRANSPARENT)
}

/// An arc as cubic Béziers (at most a quarter turn each).
fn push_arc(pb: &mut PathBuilder, center: [f32; 2], r: f32, start: f32, sweep: f32) {
    let pieces = (sweep.abs() / (PI as f32 / 2.0)).ceil().max(1.0) as u32;
    let step = sweep / pieces as f32;
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    let at = |a: f32| [center[0] + r * a.cos(), center[1] + r * a.sin()];
    let p0 = at(start);
    pb.move_to(p0[0], p0[1]);
    for i in 0..pieces {
        let (a0, a1) = (start + step * i as f32, start + step * (i + 1) as f32);
        let (s, e) = (at(a0), at(a1));
        let c1 = [s[0] - k * r * a0.sin(), s[1] + k * r * a0.cos()];
        let c2 = [e[0] + k * r * a1.sin(), e[1] - k * r * a1.cos()];
        pb.cubic_to(c1[0], c1[1], c2[0], c2[1], e[0], e[1]);
    }
}

fn circle(center: [f32; 2], r: f32) -> Option<Path> {
    PathBuilder::from_circle(center[0], center[1], r.max(1e-6))
}

/// One frame being rasterized.
struct Raster<'a> {
    list: &'a DrawList,
    images: &'a Images,
    view: &'a View,
    transform: Transform,
    /// Output → world, for per-pixel paints.
    inverse: [f64; 6],
    /// Prim index range of each group (its run in the list).
    runs: Vec<(usize, usize)>,
    /// Output pixels per world unit.
    scale: f64,
}

impl Raster<'_> {
    fn paint_for(&self, color: u32) -> Paint<'static> {
        let mut paint = Paint::default();
        paint.set_color(solid(unpack(color)));
        paint.anti_alias = self.view.anti_alias;
        paint
    }

    /// Fills or strokes `path` (world units) with a primitive's colour or paint.
    fn shade(&self, target: &mut Pixmap, path: &Path, stroke: Option<&Stroke>, color: u32, paint: u32, clip: Option<&Mask>) {
        if paint == NO_PAINT {
            let p = self.paint_for(color);
            match stroke {
                Some(stroke) => target.stroke_path(path, &p, stroke, self.transform, clip),
                None => target.fill_path(path, &p, FillRule::Winding, self.transform, clip),
            }
            return;
        }
        // A custom paint: coverage first, then the paint's colour per covered pixel.
        let filled;
        let area = match stroke {
            Some(stroke) => {
                let Some(outline) = path.stroke(stroke, self.scale as f32) else { return };
                filled = outline;
                &filled
            }
            None => path,
        };
        let Some(mut mask) = Mask::new(target.width(), target.height()) else { return };
        mask.fill_path(area, FillRule::Winding, self.view.anti_alias, self.transform);
        let width = target.width() as usize;
        let [a, b, c, d, e, f] = self.inverse;
        let pixels = target.pixels_mut();
        for (i, &coverage) in mask.data().iter().enumerate() {
            let clipped = clip.map_or(255u32, |clip| clip.data()[i] as u32);
            let coverage = coverage as u32 * clipped / 255;
            if coverage == 0 {
                continue;
            }
            let (x, y) = ((i % width) as f64 + 0.5, (i / width) as f64 + 0.5);
            let w = [(a * x + c * y + e) as f32, (b * x + d * y + f) as f32];
            let s = sample_paint(&self.list.paints, paint, w);
            let alpha = s[3] * coverage as f32 / 255.0;
            let dst = pixels[i];
            let blend = |src: f32, dst: u8| ((src * alpha * 255.0) + dst as f32 * (1.0 - alpha)).round().clamp(0.0, 255.0) as u8;
            let out_a = ((alpha * 255.0) + dst.alpha() as f32 * (1.0 - alpha)).round().clamp(0.0, 255.0) as u8;
            let (r, g, bl) = (blend(s[0], dst.red()), blend(s[1], dst.green()), blend(s[2], dst.blue()));
            pixels[i] = tiny_skia::PremultipliedColorU8::from_rgba(r.min(out_a), g.min(out_a), bl.min(out_a), out_a).unwrap_or(dst);
        }
    }

    fn stroke(&self, width: f32, cap: LineCap, join: LineJoin) -> Stroke {
        Stroke { width: width.max(0.0), line_cap: cap, line_join: join, ..Stroke::default() }
    }

    /// Draws group `g` into `target` in painter order.
    fn group(&self, g: usize, target: &mut Pixmap, clip: Option<&Mask>) {
        let (start, end) = self.runs[g];
        let prims = &self.list.prims;
        let mut i = start;
        // The frame's background prims are not clipped to the artboard.
        let background = if g == 0 { self.list.background_prims as usize } else { 0 };
        while i < end {
            let p = &prims[i];
            let clip = if g == 0 && i - start < background { None } else { clip };
            let kind = p.kind & 255;
            match kind {
                K_DEPTH_RESET => {
                    let faces = prims[i + 1..end].iter().take_while(|q| q.kind & 255 == K_TRI && q.kind & F_DEPTH != 0).count();
                    self.depth_run(target, p, &prims[i + 1..i + 1 + faces], clip);
                    i += 1 + faces;
                    continue;
                }
                K_TRI => {
                    // A run of triangles of one look fills as one path.
                    let mut pb = PathBuilder::new();
                    let mut j = i;
                    while j < end && prims[j].kind == p.kind && prims[j].color == p.color && prims[j].paint == p.paint {
                        let q = &prims[j];
                        pb.move_to(q.a[0], q.a[1]);
                        pb.line_to(q.a[2], q.a[3]);
                        pb.line_to(q.b[0], q.b[1]);
                        pb.close();
                        j += 1;
                    }
                    if let Some(path) = pb.finish() {
                        self.shade(target, &path, None, p.color, p.paint, clip);
                    }
                    i = j;
                    continue;
                }
                K_CAPSULE | K_BOX => {
                    let (hw, square) = (p.b[0], kind == K_BOX);
                    let length = (p.a[2] - p.a[0]).hypot(p.a[3] - p.a[1]);
                    if length < 1e-6 && !square {
                        if let Some(path) = circle([p.a[0], p.a[1]], hw) {
                            self.shade(target, &path, None, p.color, p.paint, clip);
                        }
                    } else {
                        let mut pb = PathBuilder::new();
                        pb.move_to(p.a[0], p.a[1]);
                        pb.line_to(p.a[2], p.a[3]);
                        if let Some(path) = pb.finish() {
                            let cap = if square { LineCap::Square } else { LineCap::Round };
                            self.shade(target, &path, Some(&self.stroke(2.0 * hw, cap, LineJoin::Round)), p.color, p.paint, clip);
                        }
                    }
                }
                K_ARC => {
                    let mut pb = PathBuilder::new();
                    push_arc(&mut pb, [p.a[0], p.a[1]], p.a[2], p.b[0], p.b[1]);
                    if let Some(path) = pb.finish() {
                        self.shade(target, &path, Some(&self.stroke(2.0 * p.a[3], LineCap::Round, LineJoin::Round)), p.color, p.paint, clip);
                    }
                }
                K_POLYLINE => {
                    let (hw, first, count) = (p.b[0], p.b[1].to_bits() as usize, p.b[2].to_bits() as usize);
                    let points = self.list.points.get(first..first + count).unwrap_or(&[]);
                    if let Some((head, rest)) = points.split_first() {
                        let mut pb = PathBuilder::new();
                        pb.move_to(head[0], head[1]);
                        for q in rest {
                            pb.line_to(q[0], q[1]);
                        }
                        if p.kind & F_CLOSED != 0 && points.len() > 2 {
                            pb.close();
                        }
                        if let Some(path) = pb.finish() {
                            self.shade(target, &path, Some(&self.stroke(2.0 * hw, LineCap::Round, LineJoin::Round)), p.color, p.paint, clip);
                        }
                    }
                }
                K_DISC => {
                    if let Some(path) = circle([p.a[0], p.a[1]], p.a[2]) {
                        self.shade(target, &path, None, p.color, p.paint, clip);
                    }
                }
                K_RING => {
                    if p.kind & F_RING_FILL != 0
                        && let Some(path) = circle([p.a[0], p.a[1]], p.a[2])
                    {
                        self.shade(target, &path, None, p.b[0].to_bits(), NO_PAINT, clip);
                    }
                    if let Some(path) = circle([p.a[0], p.a[1]], p.a[2]) {
                        self.shade(target, &path, Some(&self.stroke(2.0 * p.a[3], LineCap::Butt, LineJoin::Miter)), p.color, p.paint, clip);
                    }
                }
                K_IMAGE => self.image(target, p, clip),
                K_COMPOSITE => {
                    let child = p.b[0].to_bits() as usize;
                    if child < self.runs.len()
                        && let Some(mut layer) = Pixmap::new(target.width(), target.height())
                    {
                        self.group(child, &mut layer, None);
                        let paint = PixmapPaint { opacity: unpack(p.color)[3], ..PixmapPaint::default() };
                        target.draw_pixmap(0, 0, layer.as_ref(), &paint, Transform::identity(), clip);
                    }
                }
                K_FILL_VIEW => {
                    let [x0, y0, x1, y1] = self.view.artboard.map(|v| v as f32);
                    if let Some(rect) = Rect::from_ltrb(x0, y0, x1, y1) {
                        let path = PathBuilder::from_rect(rect);
                        // The fill is in output pixels: draw it with an identity transform.
                        let raster = Raster { transform: Transform::identity(), ..self.clone_shallow() };
                        raster.shade(target, &path, None, p.color, p.paint, None);
                    }
                }
                K_BG_IMAGE => self.background(target, p),
                _ => {}
            }
            i += 1;
        }
    }

    /// A depth run: its faces depth tested against each other (smaller window depth is
    /// nearer, an equal depth keeps the later face) in a buffer of four samples per
    /// pixel, resolved, then drawn over the target.
    fn depth_run(&self, target: &mut Pixmap, reset: &Prim, faces: &[Prim], clip: Option<&Mask>) {
        // The four sample positions of 4× multisampling (D3D, Vulkan and Metal standard).
        const SAMPLES: [[f32; 2]; 4] = [[0.375, 0.125], [0.875, 0.375], [0.125, 0.625], [0.625, 0.875]];
        let samples: &[[f32; 2]] = if self.view.anti_alias { &SAMPLES } else { &[[0.5, 0.5]] };
        let [a, b, c, d, e, f] = self.view.matrix;
        // Device coordinates and depth planes in double precision: faces in one plane then
        // agree on their depth to far below the buffer's resolution and tie.
        let device = |x: f32, y: f32| [a * x as f64 + c * y as f64 + e, b * x as f64 + d * y as f64 + f];
        let corners = [device(reset.a[0], reset.a[1]), device(reset.a[2], reset.a[1]), device(reset.a[0], reset.a[3]), device(reset.a[2], reset.a[3])];
        let (w, h) = (target.width() as i64, target.height() as i64);
        let x0 = (corners.iter().map(|p| p[0]).fold(f64::INFINITY, f64::min).floor() as i64 - 1).clamp(0, w);
        let y0 = (corners.iter().map(|p| p[1]).fold(f64::INFINITY, f64::min).floor() as i64 - 1).clamp(0, h);
        let x1 = (corners.iter().map(|p| p[0]).fold(f64::NEG_INFINITY, f64::max).ceil() as i64 + 1).clamp(0, w);
        let y1 = (corners.iter().map(|p| p[1]).fold(f64::NEG_INFINITY, f64::max).ceil() as i64 + 1).clamp(0, h);
        if x1 <= x0 || y1 <= y0 || faces.is_empty() {
            return;
        }
        let (bw, bh, n) = ((x1 - x0) as usize, (y1 - y0) as usize, samples.len());
        // Depths are compared as the GPU's 24-bit depth buffer stores them: faces that lie
        // in one plane then tie, and the later one shows, as on the GPU.
        const EMPTY: u32 = u32::MAX;
        let mut depth = vec![EMPTY; bw * bh * n];
        let mut color = vec![0u32; bw * bh * n];
        for face in faces {
            let p = [device(face.a[0], face.a[1]), device(face.a[2], face.a[3]), device(face.b[0], face.b[1])];
            let z = [face.b[2], face.b[3], face.c[0]].map(|d| window_depth(d, face.c[1], face.c[2]) as f64);
            let area = (p[1][0] - p[0][0]) * (p[2][1] - p[0][1]) - (p[1][1] - p[0][1]) * (p[2][0] - p[0][0]);
            if area == 0.0 || !area.is_finite() {
                continue;
            }
            let fx0 = (p.iter().map(|q| q[0]).fold(f64::INFINITY, f64::min).floor() as i64).max(x0);
            let fy0 = (p.iter().map(|q| q[1]).fold(f64::INFINITY, f64::min).floor() as i64).max(y0);
            let fx1 = (p.iter().map(|q| q[0]).fold(f64::NEG_INFINITY, f64::max).ceil() as i64).min(x1);
            let fy1 = (p.iter().map(|q| q[1]).fold(f64::NEG_INFINITY, f64::max).ceil() as i64).min(y1);
            // Edge functions, positive inside whatever the winding.
            let edge = |a: [f64; 2], b: [f64; 2], q: [f64; 2]| ((b[0] - a[0]) * (q[1] - a[1]) - (b[1] - a[1]) * (q[0] - a[0])) / area;
            for y in fy0..fy1 {
                for x in fx0..fx1 {
                    let pixel = ((y - y0) as usize * bw + (x - x0) as usize) * n;
                    for (k, s) in samples.iter().enumerate() {
                        let q = [x as f64 + s[0] as f64, y as f64 + s[1] as f64];
                        let (u, v, t) = (edge(p[1], p[2], q), edge(p[2], p[0], q), edge(p[0], p[1], q));
                        if u < 0.0 || v < 0.0 || t < 0.0 {
                            continue;
                        }
                        let value = ((u * z[0] + v * z[1] + t * z[2]).clamp(0.0, 1.0) * 16_777_215.0).round() as u32;
                        if value <= depth[pixel + k] {
                            depth[pixel + k] = value;
                            color[pixel + k] = face.color;
                        }
                    }
                }
            }
        }
        // Resolve: the average of the samples' premultiplied colours.
        let mut rgba = vec![0u8; bw * bh * 4];
        for (i, pixel) in rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let mut sum = [0f32; 4];
            for k in 0..n {
                if depth[i * n + k] != EMPTY {
                    let c = unpack(color[i * n + k]);
                    sum = [sum[0] + c[0] * c[3], sum[1] + c[1] * c[3], sum[2] + c[2] * c[3], sum[3] + c[3]];
                }
            }
            for (out, value) in pixel.iter_mut().zip(sum) {
                *out = (value / n as f32 * 255.0).round().clamp(0.0, 255.0) as u8;
            }
        }
        if let Some(layer) = pixmap_from(bw as u32, bh as u32, rgba) {
            target.draw_pixmap(x0 as i32, y0 as i32, layer.as_ref(), &PixmapPaint::default(), Transform::identity(), clip);
        }
    }

    fn clone_shallow(&self) -> Raster<'_> {
        Raster { list: self.list, images: self.images, view: self.view, transform: self.transform, inverse: self.inverse, runs: self.runs.clone(), scale: self.scale }
    }

    /// A sprite or a drawing tile over its world quad (o, u, v).
    fn image(&self, target: &mut Pixmap, p: &Prim, clip: Option<&Mask>) {
        let (o, u, v) = ([p.a[0], p.a[1]], [p.a[2], p.a[3]], [p.b[0], p.b[1]]);
        let key = atlas_key(p.b[2], p.c);
        let drawing = p.kind & F_DRAWING != 0;
        let (pixmap, inner, border) = if drawing {
            // One level per cel: the one the view calls for (`crate` docs).
            let level = p.b[3];
            let cel_pixel = (u[0].hypot(u[1]) as f64) / (64.0 * 2f64.powf(level as f64));
            let shown = (-(cel_pixel * self.scale).max(1e-30).log2()).floor().clamp(0.0, 8.0);
            if shown != level as f64 {
                return;
            }
            let Some(tile) = self.images.tiles.get(&key) else { return };
            (tile, 64.0, 1.0)
        } else {
            let Some(sprite) = self.images.sprites.get(&key) else { return };
            (sprite, 0.0, 0.0)
        };
        let (w, h) = if drawing { (inner, inner) } else { (pixmap.width() as f32, pixmap.height() as f32) };
        // Image texels (shifted past a tile's border) → world quad → output.
        let to_world = Transform::from_row(u[0] / w, u[1] / w, v[0] / h, v[1] / h, o[0], o[1]).pre_translate(-border, -border);
        let quality = if drawing || p.kind & F_SMOOTH != 0 { FilterQuality::Bilinear } else { FilterQuality::Nearest };
        let tint = p.kind & F_TINT != 0;
        let base = unpack(p.color);
        let tinted;
        let source = if tint {
            // An onion skin's image: its coverage in the tint colour.
            let mut copy = pixmap.clone();
            for texel in copy.pixels_mut() {
                let a = texel.alpha() as f32 / 255.0;
                let c = |k: usize| (base[k] * a * 255.0).round() as u8;
                *texel = tiny_skia::PremultipliedColorU8::from_rgba(c(0), c(1), c(2), texel.alpha()).unwrap_or(*texel);
            }
            tinted = copy;
            &tinted
        } else {
            pixmap
        };
        let mut paint = Paint::default();
        paint.anti_alias = self.view.anti_alias;
        // The fill's transform applies to the shader too: the pattern maps texels to world units.
        paint.shader = Pattern::new(source.as_ref(), SpreadMode::Pad, quality, base[3], to_world);
        let mut pb = PathBuilder::new();
        pb.move_to(o[0], o[1]);
        pb.line_to(o[0] + u[0], o[1] + u[1]);
        pb.line_to(o[0] + u[0] + v[0], o[1] + u[1] + v[1]);
        pb.line_to(o[0] + v[0], o[1] + v[1]);
        pb.close();
        if let Some(path) = pb.finish() {
            target.fill_path(&path, &paint, FillRule::Winding, self.transform, clip);
        }
    }

    /// A background picture: covers the canvas rectangle [0, a.x] × [0, a.y] (world), edges extended.
    fn background(&self, target: &mut Pixmap, p: &Prim) {
        let key = atlas_key(p.a[2], p.c);
        let Some(pixmap) = self.images.sprites.get(&key) else { return };
        let (w, h) = (pixmap.width() as f32, pixmap.height() as f32);
        let to_world = Transform::from_scale(p.a[0] / w, p.a[1] / h);
        let mut paint = Paint::default();
        paint.anti_alias = self.view.anti_alias;
        paint.shader = Pattern::new(pixmap.as_ref(), SpreadMode::Pad, FilterQuality::Nearest, unpack(p.color)[3], self.transform.pre_concat(to_world));
        let [x0, y0, x1, y1] = self.view.artboard.map(|v| v as f32);
        if let Some(rect) = Rect::from_ltrb(x0, y0, x1, y1) {
            target.fill_rect(rect, &paint, Transform::identity(), None);
        }
    }
}

/// Rasterizes `list` with `images` under `view`.
pub fn render(list: &DrawList, images: &Images, view: &View) -> Option<Pixmap> {
    let mut target = Pixmap::new(view.width.max(1), view.height.max(1))?;
    target.fill(solid(view.clear));
    let [a, b, c, d, e, f] = view.matrix;
    let transform = Transform::from_row(a as f32, b as f32, c as f32, d as f32, e as f32, f as f32);
    let det = a * d - b * c;
    let (ia, ib, ic, id) = (d / det, -b / det, -c / det, a / det);
    let inverse = [ia, ib, ic, id, -(ia * e + ic * f), -(ib * e + id * f)];
    let mut runs = vec![(usize::MAX, 0usize); list.groups.len().max(1)];
    for (i, p) in list.prims.iter().enumerate() {
        if let Some(run) = runs.get_mut(p.group as usize) {
            run.0 = run.0.min(i);
            run.1 = i + 1;
        }
    }
    for run in &mut runs {
        if run.0 == usize::MAX {
            *run = (0, 0);
        }
    }
    let raster = Raster { list, images, view, transform, inverse, runs, scale: det.abs().sqrt() };
    let clip = if view.clip_to_artboard {
        let [x0, y0, x1, y1] = view.artboard.map(|v| v as f32);
        let mut mask = Mask::new(target.width(), target.height())?;
        if let Some(rect) = Rect::from_ltrb(x0, y0, x1, y1) {
            mask.fill_path(&PathBuilder::from_rect(rect), FillRule::Winding, false, Transform::identity());
        }
        Some(mask)
    } else {
        None
    };
    raster.group(0, &mut target, clip.as_ref());
    Some(target)
}

/// Straight RGBA bytes of a rendered frame (encoders take straight alpha).
pub fn straight_rgba(pixmap: &Pixmap) -> Vec<u8> {
    let mut out = Vec::with_capacity(pixmap.data().len());
    for texel in pixmap.pixels() {
        let c = texel.demultiply();
        out.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    out
}

/// Every image of `assets` decoded for the CPU (tools and exports).
pub fn decode_all(doc: &styk_eval::doc::Document, assets: &Assets) -> Images {
    let mut images = Images::default();
    for index in 0..doc.images.len() {
        if let Some(image) = styk_eval::assets::decode_image(doc, assets, index) {
            images.add(assets, &image);
        }
    }
    for (cel, _) in assets.cels.values() {
        for image in styk_eval::assets::decode_cel(assets, cel) {
            images.add(assets, &image);
        }
    }
    images
}
