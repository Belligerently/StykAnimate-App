// StykAnimate primitive shader. One instanced draw per pass covers every
// primitive kind: the vertex stage expands a record into a quad (or triangle),
// the fragment stage shades it analytically in world units. Nothing here
// depends on the zoom level except the anti-aliasing width, so panning and
// zooming only change the `View` uniform.

struct View {
    m: vec4<f32>,        // world -> screen: x' = m.x*x + m.z*y + t.x
    t: vec4<f32>,        //                  y' = m.y*x + m.w*y + t.y ; t.z = screen px per world unit
    inv: vec4<f32>,      // screen -> world (same layout)
    inv_t: vec4<f32>,
    dest: vec4<f32>,     // target width, height, page index, depth
    artboard: vec4<f32>, // screen-space artboard rect (x0, y0, x1, y1)
};

struct Prim {
    kind: u32,
    group: u32,
    color: u32,
    paint: u32,
    a: vec4<f32>,
    b: vec4<f32>,
    c: vec4<f32>,
};

// Per-render placement of an isolation group (effects::record): six texels.
// A group is drawn in a space of its own (the screen's, an effect layer's grid,
// the camera image's) at a place in the atlas, then composited into its parent.
struct Group {
    m: vec4<f32>,       // world -> this group's atlas pixels, linear part (a, b, c, d)
    t: vec2<f32>,       // and its translation
    page: f32,          // atlas layer, or -1 when the group is not drawn
    aa: f32,            // world units per atlas pixel
    clip: vec4<f32>,    // the group's atlas rectangle (x0, y0, x1, y1)
    p: vec4<f32>,       // this group's atlas pixels -> its parent's target, linear part
    pt: vec2<f32>,      // and its translation
    show: vec4<f32>,    // the part of the atlas rectangle its composite shows
};

@group(0) @binding(0) var<uniform> view: View;
// Tables read with textureLoad, so the same shader runs where storage buffers do
// not exist (WebGL2): gradient records, isolation group placements, polyline vertices.
@group(0) @binding(1) var paint_table: texture_2d<f32>;
@group(0) @binding(2) var group_table: texture_2d<f32>;
@group(0) @binding(3) var point_table: texture_2d<f32>;
@group(1) @binding(0) var sprites: texture_2d_array<f32>;
@group(1) @binding(1) var linear_sampler: sampler;
@group(2) @binding(0) var layers: texture_2d_array<f32>;

const TABLE_WIDTH: u32 = 1024u;

fn table_at(i: u32) -> vec2<i32> {
    return vec2<i32>(i32(i % TABLE_WIDTH), i32(i / TABLE_WIDTH));
}

fn paint_row(i: u32) -> vec4<f32> {
    return textureLoad(paint_table, table_at(i), 0);
}

fn polyline_point(i: u32) -> vec2<f32> {
    return textureLoad(point_table, table_at(i), 0).xy;
}

fn group_at(g: u32) -> Group {
    let r0 = textureLoad(group_table, table_at(g * 6u), 0);
    let r1 = textureLoad(group_table, table_at(g * 6u + 1u), 0);
    let r2 = textureLoad(group_table, table_at(g * 6u + 2u), 0);
    let r3 = textureLoad(group_table, table_at(g * 6u + 3u), 0);
    let r4 = textureLoad(group_table, table_at(g * 6u + 4u), 0);
    let r5 = textureLoad(group_table, table_at(g * 6u + 5u), 0);
    return Group(r0, r1.xy, r1.z, r1.w, r2, r3, r4.xy, r5);
}

fn affine(m: vec4<f32>, t: vec2<f32>, q: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(m.x * q.x + m.z * q.y + t.x, m.y * q.x + m.w * q.y + t.y);
}

fn affine_inverse(m: vec4<f32>, t: vec2<f32>, q: vec2<f32>) -> vec2<f32> {
    let d = q - t;
    return vec2<f32>(m.w * d.x - m.z * d.y, -m.y * d.x + m.x * d.y) / (m.x * m.w - m.y * m.z);
}

const K_CAPSULE: u32 = 1u;
const K_ARC: u32 = 2u;
const K_DISC: u32 = 3u;
const K_RING: u32 = 4u;
const K_TRI: u32 = 5u;
const K_IMAGE: u32 = 6u;
const K_COMPOSITE: u32 = 7u;
const K_FILL_VIEW: u32 = 8u;
const K_BG_IMAGE: u32 = 9u;
const K_BOX: u32 = 10u;
const K_POLYLINE: u32 = 11u;
const K_DEPTH_RESET: u32 = 12u;
const F_CLOSED: u32 = 1024u;
const F_DEPTH: u32 = 8192u;
const DEPTH_FAR: f32 = 0.9999;
const F_SMOOTH: u32 = 256u;
const F_RING_FILL: u32 = 512u;
const F_TINT: u32 = 2048u;
const F_DRAWING: u32 = 4096u;
const NO_PAINT: u32 = 0xffffffffu;
const PI: f32 = 3.14159265358979;

struct VsIn {
    @builtin(vertex_index) vertex: u32,
    // One record per instance: (kind, group, colour, paint), then three vec4 of parameters.
    @location(0) head: vec4<u32>,
    @location(1) a: vec4<f32>,
    @location(2) b: vec4<f32>,
    @location(3) c: vec4<f32>,
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) world: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) @interpolate(flat) aa: f32,
    @location(3) @interpolate(flat) clip: vec4<f32>,
    @location(4) @interpolate(flat) head: vec4<u32>,
    @location(5) @interpolate(flat) a: vec4<f32>,
    @location(6) @interpolate(flat) b: vec4<f32>,
    @location(7) @interpolate(flat) c: vec4<f32>,
};

fn to_screen(p: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(view.m.x * p.x + view.m.z * p.y + view.t.x, view.m.y * p.x + view.m.w * p.y + view.t.y);
}

fn to_world(s: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(view.inv.x * s.x + view.inv.z * s.y + view.inv_t.x, view.inv.y * s.x + view.inv.w * s.y + view.inv_t.y);
}

// World -> this pass's target: the screen for the frame, the group's atlas space otherwise.
fn to_target(world: vec2<f32>, g: u32, gr: Group) -> vec2<f32> {
    if (g == 0u) { return to_screen(world); }
    return affine(gr.m, gr.t, world);
}

fn clip_pos(t: vec2<f32>) -> vec4<f32> {
    return vec4<f32>(t.x / view.dest.x * 2.0 - 1.0, 1.0 - t.y / view.dest.y * 2.0, 0.0, 1.0);
}

fn corner(v: u32) -> vec2<f32> {
    // Two triangles: (0,0) (1,0) (0,1) / (0,1) (1,0) (1,1)
    var c = array<vec2<f32>, 6>(vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0), vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0));
    return c[v];
}

fn in_sweep(angle: f32, start: f32, sweep: f32) -> bool {
    var d = angle - start;
    if (sweep >= 0.0) {
        d = d - floor(d / (2.0 * PI)) * 2.0 * PI;
        return d <= sweep;
    }
    d = start - angle;
    d = d - floor(d / (2.0 * PI)) * 2.0 * PI;
    return d <= -sweep;
}

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let vi = in.vertex;
    let p = Prim(in.head.x, in.head.y, in.head.z, in.head.w, in.a, in.b, in.c);
    let kind = p.kind & 255u;
    out.head = in.head;
    out.a = in.a;
    out.b = in.b;
    out.c = in.c;
    out.uv = vec2<f32>(0.0, 0.0);
    let g = p.group;
    out.clip = vec4<f32>(-1e9, -1e9, 1e9, 1e9);
    // World units per target pixel.
    var aa = 1.0 / max(view.t.z, 1e-12);
    var gr: Group;
    if (g != 0u) {
        gr = group_at(g);
        if (gr.page != view.dest.z) {
            out.pos = vec4<f32>(2.0, 2.0, 2.0, 1.0);
            return out;
        }
        aa = gr.aa;
        out.clip = gr.clip;
    }
    out.aa = aa;
    let margin = aa * 1.5;
    let c = corner(vi);
    var world = vec2<f32>(0.0, 0.0);
    if (kind == K_CAPSULE || kind == K_BOX) {
        let p0 = p.a.xy;
        let p1 = p.a.zw;
        let hw = max(p.b.x, aa * 0.5);
        var d = p1 - p0;
        let len = length(d);
        if (len > 1e-12) { d = d / len; } else { d = vec2<f32>(1.0, 0.0); }
        let n = vec2<f32>(-d.y, d.x);
        let e = hw + margin;
        let along = mix(-e, len + e, c.x);
        let across = mix(-e, e, c.y);
        world = p0 + d * along + n * across;
    } else if (kind == K_ARC) {
        let center = p.a.xy;
        let r = p.a.z;
        let hw = max(p.a.w, aa * 0.5);
        let start = p.b.x;
        let sweep = p.b.y;
        var lo = center + r * vec2<f32>(cos(start), sin(start));
        var hi = lo;
        let e2 = center + r * vec2<f32>(cos(start + sweep), sin(start + sweep));
        lo = min(lo, e2);
        hi = max(hi, e2);
        for (var k = 0; k < 4; k = k + 1) {
            let a = f32(k) * PI * 0.5;
            if (in_sweep(a, start, sweep)) {
                let q = center + r * vec2<f32>(cos(a), sin(a));
                lo = min(lo, q);
                hi = max(hi, q);
            }
        }
        let e = hw + margin;
        world = mix(lo - vec2<f32>(e), hi + vec2<f32>(e), c);
    } else if (kind == K_POLYLINE) {
        let e = max(p.b.x, aa * 0.5) + margin;
        world = mix(p.a.xy - vec2<f32>(e), p.a.zw + vec2<f32>(e), c);
    } else if (kind == K_DISC) {
        let e = p.a.z + margin;
        world = p.a.xy + mix(vec2<f32>(-e), vec2<f32>(e), c);
    } else if (kind == K_RING) {
        let e = p.a.z + max(p.a.w, aa * 0.5) + margin;
        world = p.a.xy + mix(vec2<f32>(-e), vec2<f32>(e), c);
    } else if (kind == K_TRI) {
        if (vi == 0u) { world = p.a.xy; } else if (vi == 1u) { world = p.a.zw; } else { world = p.b.xy; }
        if ((p.kind & F_DEPTH) != 0u) {
            // A face of a depth run: its corner's window depth, mapped over the run's depth
            // range in this shader as the editor's mesh raster maps it (styk-draw `F_DEPTH`).
            var d = p.c.x;
            if (vi == 0u) { d = p.b.z; } else if (vi == 1u) { d = p.b.w; }
            out.world = world;
            out.pos = clip_pos(to_target(world, g, gr));
            out.pos.z = 0.9995 - 0.999 * (d - p.c.y) / p.c.z;
            return out;
        }
    } else if (kind == K_DEPTH_RESET) {
        // The run's rectangle at the far depth, before its faces are drawn.
        world = mix(p.a.xy - vec2<f32>(margin), p.a.zw + vec2<f32>(margin), c);
        out.world = world;
        out.pos = clip_pos(to_target(world, g, gr));
        out.pos.z = DEPTH_FAR;
        return out;
    } else if (kind == K_IMAGE) {
        if ((p.kind & F_DRAWING) != 0u) {
            // A drawing tile of display level b.w: the view shows one level per cel,
            // floor(-log2(target pixels per cel pixel)) clamped to 0..8 (drawing-gpu.mjs).
            let level = p.b.w;
            let cel_pixel = length(p.a.zw) / (64.0 * exp2(level));
            let shown = clamp(floor(-log2(max(cel_pixel / aa, 1e-30))), 0.0, 8.0);
            if (shown != level) {
                out.pos = vec4<f32>(2.0, 2.0, 2.0, 1.0);
                return out;
            }
        }
        world = p.a.xy + p.a.zw * c.x + p.b.xy * c.y;
        out.uv = c;
    } else if (kind == K_COMPOSITE) {
        // The child's shown rectangle, carried into this target by the child's own map.
        let child = group_at(bitcast<u32>(p.b.x));
        if (child.page < 0.0) {
            out.pos = vec4<f32>(2.0, 2.0, 2.0, 1.0);
            return out;
        }
        let a = mix(child.show.xy, child.show.zw, c);
        out.uv = a;
        out.pos = clip_pos(affine(child.p, child.pt, a));
        return out;
    } else if (kind == K_FILL_VIEW || kind == K_BG_IMAGE) {
        // The artboard on screen; a group's whole layer (the camera image's background).
        if (g == 0u) {
            let s = mix(view.artboard.xy, view.artboard.zw, c);
            out.world = to_world(s);
            out.pos = clip_pos(s);
        } else {
            let s = mix(gr.clip.xy, gr.clip.zw, c);
            out.world = affine_inverse(gr.m, gr.t, s);
            out.pos = clip_pos(s);
        }
        return out;
    }
    out.world = world;
    out.pos = clip_pos(to_target(world, g, gr));
    return out;
}

fn coverage(d: f32, aa: f32) -> f32 {
    return clamp(0.5 - d / aa, 0.0, 1.0);
}

fn unpack(c: u32) -> vec4<f32> {
    return unpack4x8unorm(c);
}

// Straight-alpha colour of a paint record at a world position.
fn paint_at(offset: u32, w: vec2<f32>) -> vec4<f32> {
    let head = paint_row(offset);
    let g = paint_row(offset + 1u);
    let kind = u32(head.x);
    let count = u32(head.y);
    var t = 0.0;
    if (kind == 0u) {
        let d = g.zw - g.xy;
        t = dot(w - g.xy, d) / max(dot(d, d), 1e-12);
    } else if (kind == 1u) {
        t = length(w - g.xy) / max(g.z, 1e-12);
    } else if (kind == 2u) {
        let v = w - g.xy;
        let a = atan2(v.y, v.x) - g.z;
        t = fract(a / (2.0 * PI));
    } else if (kind == 3u) {
        let uv = (w - g.xy) / max(g.w, 1e-12);
        let cs = cos(g.z);
        let sn = sin(g.z);
        t = abs(uv.x * cs + uv.y * sn) + abs(-uv.x * sn + uv.y * cs);
    } else {
        t = (length(w - g.xy) - g.z) / max(g.w - g.z, 1e-12);
    }
    t = clamp(t, 0.0, 1.0);
    let base = offset + 3u;
    var prev_offset = paint_row(base).x;
    var prev = paint_row(base + 1u);
    if (t <= prev_offset) { return prev; }
    for (var i = 1u; i < count; i = i + 1u) {
        let o = paint_row(base + i * 2u).x;
        let col = paint_row(base + i * 2u + 1u);
        if (t < o) {
            let f = (t - prev_offset) / max(o - prev_offset, 1e-12);
            return mix(prev, col, f);
        }
        prev_offset = o;
        prev = col;
    }
    return prev;
}

// Nearest-neighbour sampling through the linear sampler: at a texel's centre the
// filter returns that texel alone. WebGL2 binds one sampler per texture, so the
// sprite atlas cannot have a second, nearest one.
fn texel_centre(uv: vec2<f32>, size: vec2<f32>) -> vec2<f32> {
    return (floor(uv * size) + vec2<f32>(0.5)) / size;
}

fn premul(c: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(c.rgb * c.a, c.a);
}

fn sd_segment(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let pa = p - a;
    let ba = b - a;
    let h = clamp(dot(pa, ba) / max(dot(ba, ba), 1e-20), 0.0, 1.0);
    return length(pa - ba * h);
}

fn sd_box(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>, hw: f32) -> f32 {
    var d = b - a;
    let len = length(d);
    if (len > 1e-12) { d = d / len; } else { d = vec2<f32>(1.0, 0.0); }
    let n = vec2<f32>(-d.y, d.x);
    let q = p - (a + b) * 0.5;
    let local = abs(vec2<f32>(dot(q, d), dot(q, n))) - vec2<f32>(len * 0.5 + hw, hw);
    return length(max(local, vec2<f32>(0.0))) + min(max(local.x, local.y), 0.0);
}

fn sd_arc(p: vec2<f32>, center: vec2<f32>, r: f32, start: f32, sweep: f32) -> f32 {
    let v = p - center;
    let a = atan2(v.y, v.x);
    if (in_sweep(a, start, sweep) || abs(sweep) >= 2.0 * PI - 1e-6) {
        return abs(length(v) - r);
    }
    let e0 = center + r * vec2<f32>(cos(start), sin(start));
    let e1 = center + r * vec2<f32>(cos(start + sweep), sin(start + sweep));
    return min(length(p - e0), length(p - e1));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let pos = in.pos.xy;
    if (pos.x < in.clip.x || pos.y < in.clip.y || pos.x > in.clip.z || pos.y > in.clip.w) {
        discard;
    }
    let p = Prim(in.head.x, in.head.y, in.head.z, in.head.w, in.a, in.b, in.c);
    let kind = p.kind & 255u;
    let w = in.world;
    let aa = in.aa;
    var base = unpack(p.color);
    if (p.paint != NO_PAINT) {
        base = paint_at(p.paint, w);
    }
    if (kind == K_CAPSULE || kind == K_BOX || kind == K_ARC || kind == K_POLYLINE) {
        var hw = p.b.x;
        if (kind == K_ARC) { hw = p.a.w; }
        // Hairlines keep a one-pixel footprint and fade with their width.
        var fade = 1.0;
        if (hw < aa * 0.5) {
            fade = max(hw, 0.0) * 2.0 / aa;
            hw = aa * 0.5;
        }
        var d = 0.0;
        if (kind == K_CAPSULE) {
            d = sd_segment(w, p.a.xy, p.a.zw) - hw;
        } else if (kind == K_BOX) {
            d = sd_box(w, p.a.xy, p.a.zw, hw);
        } else if (kind == K_POLYLINE) {
            let start = bitcast<u32>(p.b.y);
            let count = bitcast<u32>(p.b.z);
            var best = 1e30;
            for (var k = 0u; k + 1u < count; k = k + 1u) {
                best = min(best, sd_segment(w, polyline_point(start + k), polyline_point(start + k + 1u)));
            }
            if ((p.kind & F_CLOSED) != 0u && count > 2u) {
                best = min(best, sd_segment(w, polyline_point(start + count - 1u), polyline_point(start)));
            }
            d = best - hw;
        } else {
            d = sd_arc(w, p.a.xy, p.a.z, p.b.x, p.b.y) - hw;
        }
        let cov = coverage(d, aa) * fade;
        if (cov <= 0.0) { discard; }
        return premul(base) * cov;
    }
    if (kind == K_DISC) {
        let cov = coverage(length(w - p.a.xy) - p.a.z, aa);
        if (cov <= 0.0) { discard; }
        return premul(base) * cov;
    }
    if (kind == K_RING) {
        let dist = length(w - p.a.xy);
        var hw = p.a.w;
        var fade = 1.0;
        if (hw < aa * 0.5) {
            fade = max(hw, 0.0) * 2.0 / aa;
            hw = aa * 0.5;
        }
        let ring = coverage(abs(dist - p.a.z) - hw, aa) * fade;
        var out = premul(base) * ring;
        if ((p.kind & F_RING_FILL) != 0u) {
            let fill = premul(unpack(bitcast<u32>(p.b.x))) * coverage(dist - p.a.z, aa);
            out = out + fill * (1.0 - out.a);
        }
        if (out.a <= 0.0) { discard; }
        return out;
    }
    if (kind == K_TRI) {
        return premul(base);
    }
    if (kind == K_IMAGE) {
        let rect = p.c;
        let size = vec2<f32>(textureDimensions(sprites).xy);
        let texel = 1.0 / size;
        let raw = mix(rect.xy, rect.zw, in.uv);
        var tex: vec4<f32>;
        if ((p.kind & F_DRAWING) != 0u) {
            // Straight-alpha texels, premultiplied, then filtered bilinearly; the slot's
            // border holds the neighbouring tiles' texels, so tiles join without seams.
            let t = in.uv * 64.0 - vec2<f32>(0.5);
            let cell = floor(t);
            let f = t - cell;
            let origin = vec2<i32>(round(rect.xy * size)) + vec2<i32>(cell);
            let layer = i32(p.b.z);
            let t00 = premul(textureLoad(sprites, origin, layer, 0));
            let t10 = premul(textureLoad(sprites, origin + vec2<i32>(1, 0), layer, 0));
            let t01 = premul(textureLoad(sprites, origin + vec2<i32>(0, 1), layer, 0));
            let t11 = premul(textureLoad(sprites, origin + vec2<i32>(1, 1), layer, 0));
            tex = mix(mix(t00, t10, f.x), mix(t01, t11, f.x), f.y);
        } else if ((p.kind & F_SMOOTH) != 0u) {
            tex = textureSampleLevel(sprites, linear_sampler, raw, i32(p.b.z), 0.0);
        } else {
            let q = clamp(raw, rect.xy + texel * 0.5, rect.zw - texel * 0.5);
            tex = textureSampleLevel(sprites, linear_sampler, texel_centre(q, size), i32(p.b.z), 0.0);
        }
        if ((p.kind & F_TINT) != 0u) {
            // An onion skin's sprite: its coverage in the tint colour.
            return vec4<f32>(base.rgb * tex.a, tex.a) * base.a;
        }
        return tex * base.a;
    }
    if (kind == K_BG_IMAGE) {
        let rect = p.c;
        let size = vec2<f32>(textureDimensions(sprites).xy);
        let texel = 1.0 / size;
        let uv = clamp(w / p.a.xy, vec2<f32>(0.0), vec2<f32>(1.0));
        let q = clamp(mix(rect.xy, rect.zw, uv), rect.xy + texel * 0.5, rect.zw - texel * 0.5);
        return textureSampleLevel(sprites, linear_sampler, texel_centre(q, size), i32(p.a.z), 0.0) * base.a;
    }
    if (kind == K_COMPOSITE) {
        // Bilinear, clamped to the child's own rectangle as an exact texture's edge would be.
        let child = group_at(bitcast<u32>(p.b.x));
        let size = vec2<f32>(textureDimensions(layers).xy);
        let uv = clamp(in.uv, child.clip.xy + vec2<f32>(0.5), child.clip.zw - vec2<f32>(0.5));
        let tex = textureSampleLevel(layers, linear_sampler, uv / size, i32(child.page), 0.0);
        return tex * base.a;
    }
    if (kind == K_FILL_VIEW) {
        return premul(base);
    }
    return vec4<f32>(0.0);
}

