//! Render-ready assets derived once per document: where every image lives in
//! the GPU sprite atlas (computed from image headers, so encoding never waits
//! for pixel decoding) and triangulated glyph outlines.

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::Arc;

use lyon_path::math::point;
use lyon_path::Path;
use lyon_tessellation::{BuffersBuilder, FillOptions, FillRule, FillTessellator, FillVertex, VertexBuffers};

use crate::doc::{Document, Glyph, ImageData, PathCmd};

pub const PAGE: u32 = 4096;
const GUTTER: u32 = 1;

#[derive(Clone, Copy, Debug, Default)]
pub struct AtlasEntry {
    pub layer: u32,
    /// Stored pixel rectangle inside the page (excluding the gutter).
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    /// Source pixel size (before any downscale to fit a page).
    pub source_w: u32,
    pub source_h: u32,
    pub valid: bool,
}

impl AtlasEntry {
    /// UV rectangle (u0, v0, u1, v1) in page coordinates.
    pub fn uv(&self) -> [f32; 4] {
        let p = PAGE as f32;
        [self.x as f32 / p, self.y as f32 / p, (self.x + self.w) as f32 / p, (self.y + self.h) as f32 / p]
    }
}

#[derive(Default)]
pub struct Assets {
    pub atlas: Vec<AtlasEntry>,
    pub pages: u32,
    /// Triangles (glyph space) per glyph, keyed by the glyph's address.
    pub glyphs: HashMap<usize, Arc<[[f32; 2]]>>,
}

fn dimensions(data: &ImageData) -> Option<(u32, u32)> {
    image::ImageReader::new(Cursor::new(&data.bytes[..])).with_guessed_format().ok()?.into_dimensions().ok()
}

impl Assets {
    pub fn build(doc: &Document) -> Assets {
        let sizes: Vec<Option<(u32, u32)>> = crate::par::map(&doc.images, 8, dimensions);
        let (atlas, pages) = pack(&sizes);
        let mut glyph_list: Vec<Arc<Glyph>> = Vec::new();
        collect_glyphs(doc, &mut glyph_list);
        let glyphs = crate::par::map(&glyph_list, 8, |g| (Arc::as_ptr(g) as usize, tessellate(&g.path))).into_iter().collect();
        Assets { atlas, pages, glyphs }
    }

    pub fn image(&self, index: u32) -> Option<&AtlasEntry> {
        self.atlas.get(index as usize).filter(|e| e.valid)
    }

    pub fn glyph(&self, glyph: &Arc<Glyph>) -> Option<&Arc<[[f32; 2]]>> {
        self.glyphs.get(&(Arc::as_ptr(glyph) as usize))
    }
}

fn collect_glyphs(doc: &Document, out: &mut Vec<Arc<Glyph>>) {
    for f in &doc.figures {
        for t in &f.texts {
            out.extend(t.glyphs.iter().cloned());
        }
        if let Some(m) = &f.movieclip {
            collect_glyphs(&m.document, out);
        }
    }
}

/// Shelf packing into `PAGE`-sized layers, tallest images first.
fn pack(sizes: &[Option<(u32, u32)>]) -> (Vec<AtlasEntry>, u32) {
    let mut entries = vec![AtlasEntry::default(); sizes.len()];
    let max = PAGE - 2 * GUTTER;
    let mut order: Vec<usize> = (0..sizes.len()).filter(|&i| sizes[i].is_some()).collect();
    let fit = |w: u32, h: u32| -> (u32, u32) {
        if w <= max && h <= max {
            return (w.max(1), h.max(1));
        }
        let s = (max as f64 / w as f64).min(max as f64 / h as f64);
        (((w as f64 * s).floor() as u32).max(1), ((h as f64 * s).floor() as u32).max(1))
    };
    order.sort_by_key(|&i| {
        let (w, h) = sizes[i].unwrap();
        std::cmp::Reverse(fit(w, h).1)
    });
    let (mut layer, mut x, mut y, mut shelf) = (0u32, 0u32, 0u32, 0u32);
    let mut used = false;
    for i in order {
        let (sw, sh) = sizes[i].unwrap();
        let (w, h) = fit(sw, sh);
        let (cw, ch) = (w + 2 * GUTTER, h + 2 * GUTTER);
        if x + cw > PAGE {
            x = 0;
            y += shelf;
            shelf = 0;
        }
        if y + ch > PAGE {
            layer += 1;
            x = 0;
            y = 0;
            shelf = 0;
        }
        entries[i] = AtlasEntry { layer, x: x + GUTTER, y: y + GUTTER, w, h, source_w: sw, source_h: sh, valid: true };
        used = true;
        x += cw;
        shelf = shelf.max(ch);
    }
    (entries, if used { layer + 1 } else { 0 })
}

/// Even-odd fill triangulation of one glyph (glyph space).
pub fn tessellate(path: &[PathCmd]) -> Arc<[[f32; 2]]> {
    let mut builder = Path::builder();
    let mut open = false;
    for cmd in path {
        match *cmd {
            PathCmd::Move(x, y) => {
                if open {
                    builder.end(true);
                }
                builder.begin(point(x, y));
                open = true;
            }
            PathCmd::Line(x, y) => {
                if !open {
                    builder.begin(point(x, y));
                    open = true;
                } else {
                    builder.line_to(point(x, y));
                }
            }
            PathCmd::Cubic(a, b, c, d, e, f) => {
                if !open {
                    builder.begin(point(a, b));
                    open = true;
                }
                builder.cubic_bezier_to(point(a, b), point(c, d), point(e, f));
            }
            PathCmd::Close => {
                if open {
                    builder.end(true);
                    open = false;
                }
            }
        }
    }
    if open {
        builder.end(true);
    }
    let path = builder.build();
    triangulate(&path, FillRule::EvenOdd, 0.02)
}

pub fn triangulate(path: &Path, rule: FillRule, tolerance: f32) -> Arc<[[f32; 2]]> {
    let mut buffers: VertexBuffers<[f32; 2], u32> = VertexBuffers::new();
    let mut tess = FillTessellator::new();
    let options = FillOptions::tolerance(tolerance).with_fill_rule(rule);
    let ok = tess
        .tessellate_path(path, &options, &mut BuffersBuilder::new(&mut buffers, |v: FillVertex| v.position().to_array()))
        .is_ok();
    if !ok {
        return Arc::from(Vec::new());
    }
    buffers.indices.iter().map(|&i| buffers.vertices[i as usize]).collect::<Vec<_>>().into()
}

/// Premultiplied RGBA pixels of one image, sized for its atlas rectangle with a
/// one-pixel gutter that repeats its edges (so filtering never reads a neighbour).
pub struct DecodedImage {
    pub index: u32,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Decodes image `index` of `doc` for the atlas `assets` laid out. `None` when the
/// image has no place in the atlas or cannot be decoded.
pub fn decode_image(doc: &Document, assets: &Assets, index: usize) -> Option<DecodedImage> {
    let data = doc.images.get(index)?;
    let entry = assets.atlas.get(index).filter(|entry| entry.valid)?;
    let mut rgba = image::load_from_memory(&data.bytes).ok()?.to_rgba8();
    if let Some(key) = data.color_key {
        for pixel in rgba.pixels_mut() {
            if pixel[0] == key[0] && pixel[1] == key[1] && pixel[2] == key[2] {
                pixel[3] = 0;
            }
        }
    }
    if rgba.width() != entry.w || rgba.height() != entry.h {
        rgba = image::imageops::resize(&rgba, entry.w, entry.h, image::imageops::FilterType::Triangle);
    }
    let (w, h) = (entry.w as usize, entry.h as usize);
    let (gw, gh) = (w + 2, h + 2);
    let mut out = vec![0u8; gw * gh * 4];
    let source = rgba.as_raw();
    for gy in 0..gh {
        let sy = gy.saturating_sub(1).min(h - 1);
        for gx in 0..gw {
            let sx = gx.saturating_sub(1).min(w - 1);
            let p = &source[(sy * w + sx) * 4..(sy * w + sx) * 4 + 4];
            let a = p[3] as u32;
            let o = (gy * gw + gx) * 4;
            out[o] = ((p[0] as u32 * a + 127) / 255) as u8;
            out[o + 1] = ((p[1] as u32 * a + 127) / 255) as u8;
            out[o + 2] = ((p[2] as u32 * a + 127) / 255) as u8;
            out[o + 3] = a as u8;
        }
    }
    Some(DecodedImage { index: index as u32, width: gw as u32, height: gh as u32, rgba: out })
}
