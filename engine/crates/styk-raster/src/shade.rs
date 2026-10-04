//! The fragment arithmetic of styk-render's `prim.wgsl`, in single precision as the GPU
//! computes it: coverage by distance, the distance functions of strokes, and texture
//! sampling (nearest and bilinear, as the GPU's samplers filter, clamped to the edge).

use crate::target::Px;

pub(crate) const PI: f32 = std::f32::consts::PI;

/// Coverage of a pixel whose centre lies `d` world units outside a shape's edge, `aa`
/// world units a pixel (`coverage` in prim.wgsl); crisp edges cover fully or not at all.
#[inline]
pub(crate) fn coverage(d: f32, aa: f32, crisp: bool) -> f32 {
    if crisp {
        return if d <= 0.0 { 1.0 } else { 0.0 };
    }
    (0.5 - d / aa).clamp(0.0, 1.0)
}

#[inline]
pub(crate) fn premul(c: [f32; 4]) -> [f32; 4] {
    [c[0] * c[3], c[1] * c[3], c[2] * c[3], c[3]]
}

#[inline]
pub(crate) fn scale(c: [f32; 4], k: f32) -> [f32; 4] {
    [c[0] * k, c[1] * k, c[2] * k, c[3] * k]
}

/// Straight RGBA of a packed RGBA8 colour (`unpack4x8unorm`).
#[inline]
pub(crate) fn unpack(color: u32) -> [f32; 4] {
    [(color & 255) as f32 / 255.0, (color >> 8 & 255) as f32 / 255.0, (color >> 16 & 255) as f32 / 255.0, (color >> 24) as f32 / 255.0]
}

#[inline]
fn length(x: f32, y: f32) -> f32 {
    (x * x + y * y).sqrt()
}

pub(crate) fn sd_segment(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let (pa, ba) = ([p[0] - a[0], p[1] - a[1]], [b[0] - a[0], b[1] - a[1]]);
    let h = ((pa[0] * ba[0] + pa[1] * ba[1]) / (ba[0] * ba[0] + ba[1] * ba[1]).max(1e-20)).clamp(0.0, 1.0);
    length(pa[0] - ba[0] * h, pa[1] - ba[1] * h)
}

pub(crate) fn sd_box(p: [f32; 2], a: [f32; 2], b: [f32; 2], hw: f32) -> f32 {
    let mut d = [b[0] - a[0], b[1] - a[1]];
    let len = length(d[0], d[1]);
    d = if len > 1e-12 { [d[0] / len, d[1] / len] } else { [1.0, 0.0] };
    let n = [-d[1], d[0]];
    let q = [p[0] - (a[0] + b[0]) * 0.5, p[1] - (a[1] + b[1]) * 0.5];
    let local = [(q[0] * d[0] + q[1] * d[1]).abs() - (len * 0.5 + hw), (q[0] * n[0] + q[1] * n[1]).abs() - hw];
    length(local[0].max(0.0), local[1].max(0.0)) + local[0].max(local[1]).min(0.0)
}

pub(crate) fn in_sweep(angle: f32, start: f32, sweep: f32) -> bool {
    let tau = 2.0 * PI;
    if sweep >= 0.0 {
        let d = angle - start;
        return d - (d / tau).floor() * tau <= sweep;
    }
    let d = start - angle;
    d - (d / tau).floor() * tau <= -sweep
}

pub(crate) fn sd_arc(p: [f32; 2], center: [f32; 2], r: f32, start: f32, sweep: f32) -> f32 {
    let v = [p[0] - center[0], p[1] - center[1]];
    let a = v[1].atan2(v[0]);
    if in_sweep(a, start, sweep) || sweep.abs() >= 2.0 * PI - 1e-6 {
        return (length(v[0], v[1]) - r).abs();
    }
    let e0 = [center[0] + r * start.cos(), center[1] + r * start.sin()];
    let e1 = [center[0] + r * (start + sweep).cos(), center[1] + r * (start + sweep).sin()];
    length(p[0] - e0[0], p[1] - e0[1]).min(length(p[0] - e1[0], p[1] - e1[1]))
}

/// Pixels in rows of `width`: premultiplied, or straight where `straight` (drawing cel
/// tiles, which the GPU premultiplies texel by texel before filtering).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Texture<'a> {
    pub width: usize,
    pub height: usize,
    pub data: &'a [Px],
    pub straight: bool,
}

impl Texture<'_> {
    /// Texel (`x`, `y`), edges clamped, premultiplied.
    #[inline]
    pub fn texel(&self, x: i64, y: i64) -> [f32; 4] {
        let x = x.clamp(0, self.width as i64 - 1) as usize;
        let y = y.clamp(0, self.height as i64 - 1) as usize;
        let p = self.data[y * self.width + x];
        let c = [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0, p[3] as f32 / 255.0];
        if self.straight { premul(c) } else { c }
    }

    /// Bilinear filtering at texel coordinates `t` (texel centres at halves), edges clamped.
    #[inline]
    pub fn bilinear(&self, t: [f32; 2]) -> [f32; 4] {
        let (x, y) = (t[0] - 0.5, t[1] - 0.5);
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (ix, iy) = (x0 as i64, y0 as i64);
        let (a, b, c, d) = (self.texel(ix, iy), self.texel(ix + 1, iy), self.texel(ix, iy + 1), self.texel(ix + 1, iy + 1));
        [0, 1, 2, 3].map(|k| {
            let top = a[k] + (b[k] - a[k]) * fx;
            let bottom = c[k] + (d[k] - c[k]) * fx;
            top + (bottom - top) * fy
        })
    }

    /// The texel holding texel coordinates `t`, edges clamped.
    #[inline]
    pub fn nearest(&self, t: [f32; 2]) -> [f32; 4] {
        self.texel(t[0].floor() as i64, t[1].floor() as i64)
    }
}

/// A one-channel coverage mask (glyph masks), sampled bilinearly.
pub(crate) struct Mask {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

impl Mask {
    #[inline]
    fn texel(&self, x: i64, y: i64) -> f32 {
        let x = x.clamp(0, self.width as i64 - 1) as usize;
        let y = y.clamp(0, self.height as i64 - 1) as usize;
        self.data[y * self.width + x] as f32 / 255.0
    }

    #[inline]
    pub fn bilinear(&self, t: [f32; 2]) -> f32 {
        let (x, y) = (t[0] - 0.5, t[1] - 0.5);
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (ix, iy) = (x0 as i64, y0 as i64);
        let top = self.texel(ix, iy) + (self.texel(ix + 1, iy) - self.texel(ix, iy)) * fx;
        let bottom = self.texel(ix, iy + 1) + (self.texel(ix + 1, iy + 1) - self.texel(ix, iy + 1)) * fx;
        top + (bottom - top) * fy
    }
}

/// An affine map `[a, b, c, d, e, f]`: x' = a·x + c·y + e, y' = b·x + d·y + f.
pub(crate) type Affine = [f64; 6];

#[inline]
pub(crate) fn apply(m: Affine, p: [f64; 2]) -> [f64; 2] {
    [m[0] * p[0] + m[2] * p[1] + m[4], m[1] * p[0] + m[3] * p[1] + m[5]]
}

/// `m` in single precision applied to a single-precision point, as a vertex shader does.
#[inline]
pub(crate) fn apply32(m: &[f32; 6], p: [f32; 2]) -> [f32; 2] {
    [m[0] * p[0] + m[2] * p[1] + m[4], m[1] * p[0] + m[3] * p[1] + m[5]]
}

pub(crate) fn invert(m: Affine) -> Affine {
    let det = m[0] * m[3] - m[1] * m[2];
    let (a, b, c, d) = (m[3] / det, -m[1] / det, -m[2] / det, m[0] / det);
    [a, b, c, d, -(a * m[4] + c * m[5]), -(b * m[4] + d * m[5])]
}

#[inline]
pub(crate) fn single(m: Affine) -> [f32; 6] {
    m.map(|v| v as f32)
}
