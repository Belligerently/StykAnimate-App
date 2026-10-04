# Engine: rendering

How a draw list becomes pixels: the format (`engine/crates/styk-draw`), the GPU
renderer (`engine/crates/styk-render`), the CPU rasterizer
(`engine/crates/styk-raster`), the stage in the app (`src/engine/engine-stage.mjs`),
thumbnails, library previews and the camera monitor (`engine/crates/styk-wasm/src/views.rs`,
`preview.rs`, `src/engine/engine-thumbnails.mjs`, `engine-previews.mjs`) and export frames
(`engine/crates/styk-export`, `src/engine/engine-export.mjs`).

## The draw list (`styk-draw`)

One `Prim` record (64 bytes) per primitive, in world units, so a list is view
independent: pan and zoom change one uniform. Kinds (`K_*`) and flags (`F_*`)
are documented on their constants. Isolation `Group`s (translucent figures,
outlines, onion skins) draw into layers composited by a `K_COMPOSITE` record
at their place in the parent's painter order; group ids are given depth first.
A group whose artwork is filtered has a raster **layer** (`Group::layer` into
`DrawList::layers`, an `EffectLayer`): the rectangles the editor filters on and
the effect stack, opaque to the format ([vfx.md](vfx.md#rendering)). Top-level
artwork holding 3D faces or text has one too, without effects (`LayerKind::Raster`):
the editor draws it from a raster of its own ([Text](#text)). 3D faces are **depth runs**: a
`K_DEPTH_RESET`, then `K_TRI` records with `F_DEPTH` and their corner depths, depth
tested against each other and placed as a whole in painter order
([spatial.md](spatial.md#drawing)).
Paint records live in `DrawList::paints`; `sample_paint` is the reference
evaluation every renderer reproduces. Generated effects are `K_PARTICLE` records
(one per particle, in the shape bits of its kind, in its draw's local units) and
`K_BEAM` records; each points at its draw's rows in `paints` (its transform to the
world and its constants), `F_ADD` marks those blended additively, and
`DrawList::materials` names the particle material rows sprites sample
([vfx.md](vfx.md#generated-effects-on-screen)). An object that is one solid glyph
is a `K_GLYPH` record drawn from a mask of the glyph, followed by the glyph's own
triangles flagged `F_GLYPH_VECTOR`; `DrawList::glyphs` holds each record's
`GlyphMask` and triangles ([Text](#text)).

## GPU (`styk-render`)

| Module | Holds |
| --- | --- |
| `pipeline.rs` | the one WGSL pipeline (`prim.wgsl`) for every kind, bind layouts ([D8](decisions/0008-renderer-without-storage-buffers.md)) |
| `frame.rs` | one frame: isolation layer passes by round and depth, each depth's effect stacks after its passes, then the frame pass (MSAA ×4, resolved once) |
| `isolation.rs` | the layer atlas (8 pages of at least the frame's size, grown for larger effect layers, two for depth parity) and `plan_rounds`: layers that do not fit render in full-resolution rounds, the frame drawn between rounds |
| `images.rs` | the sprite atlas (texture array of 4,096² pages laid out by `styk_eval::assets`), uploads of decoded images and drawing tiles |
| `resources.rs` | GPU record layouts, lookup tables in float textures, buffers |
| `present.rs` | copy into the host's surface, read-back |
| `view.rs` | world → output transform, artboard |
| `effects/` | effect layers ([vfx.md](vfx.md#rendering)): `plan.rs` places each group for the view (density, crop, composite), `mod.rs` writes its group record, `executor.rs` and `passes.wgsl` run effect stacks on pooled slots |
| `materials.rs` | the particle material atlas of generated effects: a placeholder texel until a list names a row, then the editor's procedural rows with their reduced levels (5.9 MB), sampled trilinearly |
| `glyphs.rs`, `glyph.wgsl` | the masks of lone glyphs ([Text](#text)): each glyph filled once per mask size into an atlas of one-channel pages kept across frames (at most 64 MiB), and the per-frame table that tells each `K_GLYPH` record where its mask is |

Drawing cels: every display level's tiles are in the list; the vertex shader
keeps the level `floor(-log2(pixels per cel pixel))` (0–8) and the fragment
shader premultiplies straight-alpha texels and filters bilinearly across tile
borders (each atlas slot carries its neighbours' edge texels; halo tiles fade
past the last tile), as the editor's GPU drawing display does.

Depth runs: a list with faces gets a multisampled `depth24plus` buffer per pass
and two variants of the one shader's pipeline (painter order writing depth; faces
tested `less-equal`), switched where faces begin and end; the vertex shader maps a
corner's depth to its window depth with the editor's expression.

Generated effects: the vertex shader expands each `K_PARTICLE` or `K_BEAM` record
exactly as the editor's generator shader does, in its draw's local units with the
target's pixel in those units, then places it by the draw's transform; the fragment
shader shades it as the editor's does. Runs of `F_ADD` records draw with an additive
pipeline variant (colour and alpha added), created with the first list that has one.

Lone glyphs: before the passes, `GlyphMasks::prepare` lays out each `K_GLYPH`
record's mask for the view's raster density and writes the glyph table (three
texels per record: the texel axes in the world, the origin and mask size, the
mask's place in the atlas); a mask no frame has filled yet is filled then, in the
same command buffer, so no frame waits for one. The vertex shader places the
record's quad from the table and collapses its `F_GLYPH_VECTOR` triangles, or the
other way round where the view gives it no mask; the fragment shader samples the
mask bilinearly, clamped to its own texels, times the record's colour and opacity.

Invariants: no per-frame allocation in steady state; the no-effect path draws
one pass per depth and round and creates no effect resource; a list without 3D
faces creates no depth buffer and a list without additive records no additive
pipeline, and either draws one instanced draw per pass; the material atlas is a
placeholder texel until a list names a row, and the glyph atlas until a list has a
lone glyph; glyph masks are filled only when first needed; WebGL2 limits (no storage buffers, one sampler
per texture, array textures of at least two layers, [D11](decisions/0011-array-textures-two-layers.md)).

## Text

The editor's WebGPU renderer (`src/renderer.mjs`) never draws text as vectors
below a raster density of 8; the engine draws it the same two ways.

- **Text in an object raster.** Text is one of the things `requiresCompatibilityTexture`
  names: the object holding it (or the movieclip instance, or the connected run) is
  painted by Canvas 2D on a raster of its own (`rasterObject`) on a world-aligned grid
  at `rasterResolution` of the view (a quarter octave at or above the screen's density,
  at least 0.5), then drawn as a texture, sampled bilinearly. The engine gives such an
  entry a `LayerKind::Raster` group on the editor's `objectBounds` (`styk_eval::drawlist`,
  `raster`), drawn on the editor's grid for the view (`effects::plan`, as for 3D
  artwork) and composited bilinearly.
- **A lone glyph from a glyph mask.** An object that is one text segment with one
  visible character, solid, without effects, figure outline or movieclip
  (`glyphTextureDescriptor`; Pivot imports have many, one character per figure) is
  drawn from a mask of the glyph alone: its path filled in glyph space on a grid of
  power-of-two sides, at least 1.25 times the view's raster density, with two texels of
  padding, then drawn as a quad, tinted and faded. `GlyphMask::layout` (styk-draw) is
  that arithmetic, in the editor's order; the test
  `glyph_masks_are_laid_out_as_the_editor_lays_them_out` holds numbers the editor's own
  function printed. The object's opacity goes into the record's colour, as the editor's
  shader applies it, so a lone glyph needs no isolation layer. Above density 8, or for a
  mask above 4,096 texels, the editor rasterizes the object instead; the engine then
  draws the glyph's triangles.

**Masks** (`glyphs.rs`): one mask per glyph tessellation and mask size, shared by every
instance, filled with four samples a texel and resolved, in square one-channel pages of
1,024 texels a side, grown to the frame's largest mask (up to 4,096; growing drops the
masks, which are filled again as they are drawn), at most 64 MiB of pages. They are kept
across frames; when a mask does not fit, the least recently used page that no record of
the frame samples is emptied; a record whose mask cannot be placed is drawn from its
triangles for that frame. The masks
are filled on the GPU in the frame's own command buffer: nothing is rasterized on the
interface thread and nothing is waited for (D7). Entries hold their triangles, so a key
(the triangles' allocation) is never reused while kept; a new document's glyphs age the
old ones out.

**Curves.** Glyph triangles flatten curves within 0.02 glyph units, and finer by halves
for text drawn large (`styk_eval::assets::glyph_level`), so that they stay within 0.1
world units of the outline the editor fills; Pivot text is often drawn 50 to 60 times its
glyph size, where 0.02 glyph units put curves a pixel off at the fitted view. Finer
tessellations are made when first drawn (on the thread drawing, also the interface thread
for edit frames) and kept per document up to 64 MiB, behind a spinning lock held only for
a lookup or an insertion.

**What differs.** Inside a raster, the editor's Canvas 2D in Chromium is GPU-accelerated
at those densities: its text coverage takes nine levels (eighths), and the engine's four
samples five. Measured on two captions of `Impact-FX-Fight.styk` filled by the editor's
own scratch canvas in Electron (accelerated, as `rasterObject` uses it) against the same
glyphs filled by other rules: the edge pixels differ by 22.9 and 26.1 levels on average
from four standard samples; 13.0 and 19.4 from eight standard samples (65 % and 45 % of
edge pixels equal); 16.5 and 17.7 from tiny-skia, a CPU rasterizer; and the editor's own
software canvas (which `rasterObject` uses when zoomed in) differs from the accelerated
one by 13.6 and 17.5. No rule reproduces the accelerated canvas, which depends on the
browser and the GPU, and WebGPU offers one or four samples, not eight; the engine keeps
four samples and the GPU, with no CPU raster or worker job.
Devices without WebGPU: the editor's Canvas 2D fallback draws text as vector paths at the
screen's density, the engine as the WebGPU renderer does, as for polygon seams and
generated effects.

**Measured** (2026-10-03, RTX 4070 SUPER, machine shared with other work at a load
average of 5 to 40). App parity (`scripts/engine-app-parity.mjs`, D13 limits unchanged,
onion skins on, 1,440×900 window, Electron under Xvfb with the Linux Vulkan switches),
the engine on WebGPU against the editor's WebGPU renderer, before (engine-rust at
`904cad9e`) and after:

| Document (steps) | Before: passing, mean beyond 2, solid pixels | After |
| --- | --- | --- |
| 15-Guns-Pivot-3D.styk (5 ticks) | 0/5, 0.387–0.747, 9 | 4/5, 0.185–0.294, 0 |
| Hand-and-Face-Pivot-3D.styk (5 ticks) | 2/5, 0.166–0.431, 4 | 5/5, 0.070–0.174, 0 |
| Vehicles-Pivot-3D.styk (5 ticks at fit, 0.3× and 3×, after playback) | 7/16, 0.039–0.566, 9 | 16/16, 0.017–0.211, 0 |
| Impact-FX-Fight.styk (ticks 30, 108, 157, 185, 192, 205) | 0/6, 0.321–0.382, 0 | 6/6, 0.168–0.217, 0 |
| Impact-FX-Fight.styk (ticks 30, 185 at 0.3× and 3×, after playback) | 2/5, 0.047–0.380, 4 | 5/5, 0.047–0.217, 0 |
| Uriel 19-2-2026.piv (private corpus, lone glyphs; ticks 20, 210, 280, 370) | 2/4, 0.071–0.670, 0 | 4/4, 0.049–0.232, 0 |
| Karate-Fighter-Movement.styk, Katana-Turntable.styk (no text; 16 and 5 steps) | 21/21, ≤ 0.005 | 21/21, unchanged |

Before, the text objects' rectangles held 69–100 % of each step's sum at the fitted view
(Impact: 5–6 % of the stage held 69–79 %, and the stage without them stays at
0.073–0.116, so figure edges are no failing cause of their own); a text pixel differed by
3.2–5.3 levels on average (8.9–10.6 at 0.3×), after by 1.4–2.3. The step still failing,
15 Guns tick 0 (mean 0.294), is the cover page, 13.9 % of it text, which holds 99 % of the
sum: the coverage rule inside the rasters above. Measured while building it, Uriel tick 210
needed the finer curves (0.358 with masks alone, 0.101 with them) and tick 370 the opacity
in the glyph's colour (0.304 with an isolation layer per translucent glyph: 325 groups and
a downscaled round; 0.232 without).

Web target (headless Chromium, the engine on WebGL2 against the editor's Canvas 2D
fallback), before and after: Impact (ticks 30, 108, 185) 0/3 at 0.348–0.434 and 0/3 at
0.414–0.506; Hand & Face (0, 252, 504) 2/3 and 1/3; 15 Guns (0, 3) 0/2 at 0.510–0.845 and
0/2 at 0.610–1.089; Uriel (210, 370) 0/2 at 0.491–0.787 and 0/2 at 0.269–0.780; Katana 2/2
and 2/2. The editor's own two renderers differ more than either from the engine: its
WebGPU pictures against its Canvas 2D pictures 0.491, 0.668, 1.165 and 0.876 (Impact 30,
Hand & Face 0, 15 Guns 0, Uriel 370), the engine on WebGL2 against them 0.414, 0.582,
1.089 and 0.780, and against the WebGPU pictures 0.298, 0.311, 0.475 and 0.348.

Cost. GPU frame (`styk gpubench`, 1,440×900 over 200 zoom levels, p50 of three
interleaved rounds, before → after): Uriel tick 370 1.87 → 0.65 ms and tick 210 1.76 →
1.11 ms (lone glyphs need no layer); 15 Guns 0.25 → 0.24, Impact 0.13 → 0.13, Vehicles
0.28 → 0.27, Karate 0.24 → 0.25, Jay 0.25 → 0.24 ms. Draw-list encoding over every tick
(`styk bench`, three interleaved rounds): Uriel 2.07–2.17 → 2.13–2.71 ms a frame (31,808
→ 34,926 records, the finer curves), 15 Guns 1.50–2.40 → 1.09–1.65, Impact 0.22–0.27 →
0.27–0.30, Jay unchanged (0.37–0.40). Masks: filling those of a new density takes 0.2 to
1.9 ms of a frame (4 to 8 masks, Uriel at 2.2 times the fitted view); in the app the atlas
held 9 masks in 2 MiB at Uriel tick 370's fitted view, at most 64 MiB. Edit frames in the
app (the stage's timing of `render_edit`, Uriel ticks 360 to 380 with onion skins, three
interleaved rounds at loads of 12 to 41): medians 86–412 ms before and 104–408 ms after;
the quietest round 86–96 against 104–108 ms (the draw list holds 26 % more records:
330,238 → 417,744 with the onion skins). Not measured: other GPUs, browsers' frame times.

## CPU (`styk-raster`)

The GPU renderer's pipeline carried out on the CPU, rule for rule, so a frame drawn here is
the frame the stage draws: for devices with neither WebGPU nor WebGL2 and for tools
(`styk raster`, `styk export --cpu`). It replaced a tiny-skia rasterizer that covered edges
as Skia does (a few partial-coverage levels at thumbnail sizes), drew no filter effects,
drew raster entries at the output's density and resolved each depth run to its nearest
faces before compositing it.

| Module | Holds |
| --- | --- |
| `frame.rs` | one frame: each group placed by styk-render's plan, drawn into a target of its own, filtered, composited; every primitive kind |
| `target.rs` | a target with the GPU's samples: four RGBA8 samples a pixel (one for crisp edges), blended one by one, averaged when resolved |
| `coverage.rs` | the GPU's rasterization rules: vertices snapped to 1/256 pixel, the standard four sample positions, the top-left rule |
| `shade.rs` | `prim.wgsl`'s fragment arithmetic: coverage by distance, the strokes' distance functions, texture sampling |
| `effects.rs` | effect recipes run pass by pass with the arithmetic of `passes.wgsl` |
| `glyphs.rs` | masks of lone glyphs, kept across frames |
| `generators.rs` | generated effects: the GPU's quads, its fragment arithmetic |

**One plan for both renderers.** What does not need a GPU lives in styk-render without its
`gpu` feature, which styk-raster depends on: `ViewParams`, `Edges`, where each group is
drawn for a view (`effects::plan`: a translucent figure on its parent's grid, a raster entry
or filtered artwork on the editor's raster grid at its density, the camera image through the
scene camera) and its group record (`effects::record`, here with every layer at the origin
of a target of its own), and effect recipes (`effects::recipe`: the passes `VfxWebGPU.run`
records, with their parameters). The GPU's atlas, rounds and pooled slots are resource
management and have no CPU counterpart: a layer is drawn at full resolution into a target of
its size.

**Rules followed.**

- Samples: a target keeps one colour per pixel until a write covers only some of its
  samples, then four; a write that leaves them equal folds them back. Blending is the
  pipeline's (`One, OneMinusSrcAlpha`; `One, One` for `F_ADD`), each result rounded to 8
  bits as an `Rgba8Unorm` target stores it; resolving averages the samples.
- Strokes, arcs, polylines, discs and rings are covered by distance at each pixel's centre
  (`clamp(0.5 − d / aa, 0, 1)`, hairlines faded to a one-pixel footprint; crisp: in or out),
  over the pixels of the quad the vertex shader draws. A polyline takes each pixel's nearest
  segment.
- Triangles, image and glyph quads, composites, particles and the artboard fill are covered
  by the samples, so adjacent triangles share their edges' samples: no seams between faces,
  glyph triangles or polygons of different colours, and none between a cel's tiles.
- Paints are `sample_paint` at the pixel's centre; sprites bilinear (`F_SMOOTH`) or nearest
  within their gutter; cel tiles premultiplied texel by texel, then bilinear across their
  borders, at the level the layer's density shows; composites bilinear, clamped to the child
  (nearest with crisp edges); backgrounds nearest.
- Depth runs: the reset's quad sets the far depth, then each face is drawn over what is
  there, sample by sample, where its depth (the vertex shader's window depth, interpolated
  at the sample, stored at 24 bits) is not greater than what the run drew there: translucent
  faces blend in drawing order, an equal depth passes.
- Lone glyphs: masks laid out by `GlyphMask::layout` for the view's raster density, filled at
  four samples a texel and kept across frames by glyph and size (at most 64 MiB, the least
  recently used first out); an instance without a mask draws its triangles.
- Filter effects: after a filtered layer is drawn and resolved, its recipe runs on its
  pixels, each pass written to an RGBA8 slot of its size and sampled bilinearly with edges
  clamped, as on the GPU. Box passes and alpha dilations of a source as large as their
  target take whole texels, so they run with prefix sums and a monotonic queue (equal to
  shading each pixel within a level, `windows_equal_the_shader`).

**What still differs.** Near-coplanar 3D faces: two faces nearly in one plane (a seat on a
tank, a decal on a receiver; measured 7 and 16 steps of the 24-bit buffer apart) resolve by
the last bits of each GPU's vertex arithmetic (where its layer sits in the atlas, fused
multiply-adds, its depth interpolation), which differ between GPUs too; the CPU interpolates
exactly from the snapped vertices. The GPU draws layers too large for its atlas pages at a
lower scale in a lone round (`plan_rounds`); the CPU never does. Texture filtering and
resolves round in the last level.

**Memory.** A frame holds the view-sized target, then one target per isolation group on the
path being drawn (a layer's size: its rectangle on its grid), each four bytes a pixel plus
sixteen per pixel whose samples differ; a filtered layer adds its working slots (at most
eight of its size). Glyph masks stay within 64 MiB. Images are the decoded images the
engine holds, not copied (`Images::add_shared`). Nothing is pooled between frames; exports
bound a frame by drawing it in tiles.

### Measured

`styk raster-parity` draws the same draw lists and views with both renderers (frames as the
app's thumbnails and exports take them from `ExportFrames`; library files as the asset library
opens them) and compares them by D13, limits unchanged, alpha counted. 2026-10-04, RTX 4070
SUPER (Vulkan) for the GPU, the machine shared with other work (load averages 6–21). Before:
the tiny-skia rasterizer (`86124552`); after: this one. Repository corpus: the 388 tracked
`.styk`, `.stkz` and `.stykmov` files and 47 editor-built fixtures (effects, generated
effects, 3D, drawings, text; 2 have no keyframe), thumbnails at up to five frame starts,
exports at three; library previews of the 193 `.stkz` and `.stykmov` files (poster and three
hover frames). Passing steps, both rules (no solid pixel / mean):

| Steps | Before | After |
| --- | --- | --- |
| Thumbnail 320×192 (899) | 435 (857 / 438) | 899 (899 / 899) |
| Thumbnail 512×288 (899) | 607 (857 / 614) | 899 (899 / 899) |
| Thumbnail 96×54 (899) | 144 (790 / 144) | 899 (899 / 899) |
| Export, canvas size, Smooth edges (677) | 645 (645 / 665) | 677 (677 / 677) |
| Export, No smoothing (677) | 629 (645 / 649) | 677 (677 / 677) |
| Library poster (193) | 36 (179 / 36) | 193 (193 / 193) |
| Library hover frame (579) | 52 (520 / 52) | 579 (579 / 579) |
| Private corpus (8 files: URIEL 3 with its blur, glow, bloom and tint, Jay, Pistol-3D, four Pivot imports): thumbnails at three sizes (93) | 49 | 93 |
| Private corpus: exports, both samplings (38) | 33 | 38 |

Twelve frame starts of each of the 180 3D documents (scenes, turntables, demos, packs and
spatial fixtures): 1,244 of 1,244 thumbnails at 320×192 pass, 1,242 at 512×288 and 1,231
exports, every failure by solid pixels where two near-coplanar faces meet (above); no mean
fails. Which effects the documents use (`--census`, every tick): the repository's documents
and asset packs draw no filter effect (9 hold generated effects); the effect fixtures draw all
17 kinds; the private corpus draws blur (99 frames of URIEL 3), glow (54), bloom (47), tint
(36), RGB split (2) and outline (1).

**Time per frame** (native, one thread; `styk raster-parity --repeat 5`, one CPU renderer per
document kept across its frames; three rounds alternating the two builds, the median over three
frame starts per round, the range over rounds; 2026-10-04 07:44–07:47, load averages 9–12 on 12
threads; the tiny-skia rasterizer drew no filter effect, so the effects fixture and URIEL 3 are not
the same work):

| Document | Thumbnail 320×192, tiny-skia → now | Export at the canvas size, tiny-skia → now |
| --- | --- | --- |
| 15-Guns-Pivot-3D.styk (3D, text; 1,920×1,080) | 14.1–25.8 → 9.5–9.8 ms | 386–578 → 95–101 ms |
| Vehicles-Pivot-3D.styk (3D, text; 1,920×1,080) | 12.8–21.4 → 10.2–17.7 ms | 369–544 → 98–157 ms |
| Impact-FX-Fight.styk (captions; 1,920×1,080) | 7.3–9.1 → 3.5–6.0 ms | 219–235 → 79–126 ms |
| Bamboo-Training-Grounds-Demo.styk (800×600) | 16.5–26.8 → 14.9–26.6 ms | 109–179 → 55–101 ms |
| hitoshi vs suzu tomiee ssj.piv (private; 760×420) | 11.2–21.5 → 7.5–13.3 ms | 33–61 → 32–58 ms |
| Karate-Fighter-Movement.styk (3D rig; 800×900) | 2.3–3.5 → 4.4–9.3 ms | 20–34 → 15–32 ms |
| drawing-cel fixture (960×540) | 1.2–1.4 → 2.5–3.9 ms | 2.0–4.2 → 14.1–24.9 ms |
| explosions fixture (generated effects; 960×540) | 0.3–0.7 → 0.6–1.2 ms | 2.2–6.5 → 5.4–9.9 ms |
| TDD2 URIEL NEMESIS 3.styk (private; now with its effects; 800×400) | 6.0–12.6 → 8.0–13.2 ms | 23–44 → 55–88 ms |
| effects fixture (fourteen effects on one figure; 960×540) | 2.3–4.3 → 90–152 ms | 20–29 → 531–858 ms |

Text and 3D scenes draw faster (tiny-skia filled each glyph and face path at the output's density);
drawing cels and plain figures slower (each pixel of a cel's quads is sampled bilinearly, where
tiny-skia's pattern fill is vectorised); filter effects cost what their passes cost, a few to tens
of milliseconds a pass at export sizes. In the app's export, frames are drawn in tiles of at most
1,024 pixels (`styk export --cpu`, 1,920×1,080, median of 5 frames, load 9–12: Impact 83 ms,
15 Guns 97 ms, URIEL 3 145 ms; hitoshi 336 ms before the stroke interiors were filled without the
square root, then 417–422 ms against 553–568 ms without it, interleaved at load 15–17). The
WebAssembly engine module grew by 109 KiB (7,980,628 → 8,092,410 bytes without wasm-opt, the
build before this work against this one).

## In the app

`src/engine/engine-stage.mjs` puts the engine's canvas (`#scene-engine`)
where the JavaScript renderer's is and draws the edit frame, onion skins and
prepared playback frames for documents the engine supports
([D4](decisions/0004-strangler-gate.md)). The WebGPU surface is configured with
premultiplied alpha so the page shows through the pasteboard. Through the scene
camera (prepared playback frames, and edit frames in the camera preview) the scene is
seen through `cameraAtTime` (follow clips, camera-motion impulses) and parallax layers
move: the frame's per-object offsets ([evaluation.md](evaluation.md#the-scene-camera))
join the drag translations of the draw list. Filter effects are
drawn by the engine; the effects preview switch reaches it through
`Stage::set_effects` and `EngineHandle::set_effects`. Generated effects are drawn
by the engine (an edit frame whose trails are still being sampled asks the app for
another frame, `Stage::refining`); so are 3D meshes, surfaces, poses and drawings a
spatial bone turns.

**Pictures that arrive after the frame.** Pictures and drawing cels decode on the engine's
workers after a document opens, and the stage uploads them a slice at a time (8 MiB a
frame) when it draws. A still frame drawn before its pictures arrived used to stay without
them until something else drew (URIEL 3 opened at tick 44: its drawing cel missing, 23,783
solid pixels in app parity). The stage now notes the atlas places it has uploaded
(`styk_engine::Uploaded`) and, after each frame, whether that frame samples a place not
filled yet while pictures are still on their way (`Stage::pictures_pending`: looked up only
then); the interface draws again on the next frame when the missing pictures are decoded and
wait for upload, or once the engine's progress counter advances when they are still decoding
(`waitForPictures` in `engine-stage.mjs`: one `Atomics.waitAsync` at a time, dropped when
another frame is drawn, a timeout without progress waiting again without drawing, at most a
minute). A picture that cannot be decoded ends the waiting once nothing else is pending.
Measured opening URIEL 3 at tick 44 in Electron: 9 and 13 extra frames in two runs, all within
the first 1.3 s, none afterwards; the cel is there.

## Thumbnails, library previews and the camera monitor

The other places the editor draws frames, for documents the engine draws, use the same GPU
renderer as the stage, so they show what the stage shows. Where no GPU path serves them (a stage
on WebGL2 for thumbnails, a worker without a GPU device or whose canvas gives no pixels for library
previews and exports), the CPU rasterizer draws the same pixels in a worker ([CPU](#cpu-styk-raster),
[The CPU paths](#the-cpu-paths)). Where the engine does not draw (documents the gate refuses, pages
without the engine's worker threads, devices where the stage gets no GPU device at all), the
JavaScript paths draw as before.

**Frame thumbnails** (the timeline's filmstrip cards at 320×192, the export dialog's stage and
strip at up to 512×288 and 96×60): `src/engine/engine-thumbnails.mjs` keeps the interface of
`ThumbnailRenderer` (`src/thumbnail-renderer.mjs`). For a thumbnail it brings the engine to the
editor's revision (`src/engine/engine-document.mjs`, the synchronisation exports use), then asks
the stage (`renderThumbnail` in `engine-stage.mjs`, `Stage::prepare_thumbnail` and
`render_thumbnail` in `styk-wasm/src/views.rs`): the tick's frame is prepared on the engine's
workers by an `ExportFrames` pinned to the revision (the interface thread only checks
`ExportFrames::ready`, never evaluates, D7), every picture of the document is uploaded first,
then a renderer of the thumbnails' own, sampling the stage's sprite atlas
(`Renderer::share_sprites`), draws it on the stage's device into an `OffscreenCanvas`, framed as
the filmstrip frames it (`styk_engine::thumbnails::card`: the canvas fitted and centred on a
`#f5f5f5` card, through the scene camera, effects drawn, the document's hidden layers hidden),
and the interface copies that canvas onto the card in the same task. Where the stage cannot draw
thumbnails (a WebGL2 stage cannot present one device to a second canvas; a WebGPU canvas failing the
magenta probe), the preview worker below draws them on the CPU (`Previews::thumbnail`): it holds an
engine handle (an address made when the request is sent, consumed in the worker), pins the revision
with an `ExportFrames`, waits on the engine's progress counter for the frame (prepared on the engine's
workers) and every picture (decoded there once per document lineage: a later revision decodes only
the images and drawing cels the earlier one lacked, `ExportFrames::decode_images_since`), rasterizes
the card with styk-raster and hands it over as an `ImageBitmap`, which the interface draws onto the
card. No copy of the document goes to a thumbnail worker; the JavaScript worker is created only for
what the engine does not draw: in-between previews (`docs/INSERT-INBETWEEN.md`, a sampled pose that
is not in the document), refused documents, pages without the engine's worker threads, and
thumbnails neither the stage nor the preview worker drew.
Thumbnails show parallax layers moved by the camera, as playback does; the JavaScript worker
drew them unmoved on frames without scene effects and was fixed first (it is the oracle).

**Library previews** (`src/library-files.mjs`, `src/library-hover-preview.mjs`): one preview
worker per page (`src/engine/engine-previews.mjs`, `engine-preview-worker.mjs`) joins the
engine's shared memory and gets a GPU device of its own (WebGPU in the worker, else WebGL2 on its
`OffscreenCanvas`). It opens a library file apart from the open document
(`styk_engine::thumbnails::library`, `styk_wasm::preview::Previews`) and draws it with the stage's
renderer: a saved asset's poster from its saved bytes (tick 0, or the first of fifteen evenly
spaced ticks with artwork, fitted to its bounds with a tenth of margin, on transparency), and the
hover animation (fitted to the union of sixteen sampled frames, grown by every frame drawn). Bounds
are the editor's: `objectBounds` with each item's effect padding
(`styk_eval::drawlist::artwork_bounds`) joined with the generated effects' reach
(`Batch::bounds`, `generatorBatchBounds`). Each picture leaves the canvas as an `ImageBitmap`
(`transferToImageBitmap`); nothing is read back. The card's backdrop is chosen in the worker from
sixteen 40-pixel samples by `libraryPreviewMatte` (`src/library-matte.mjs`, shared with the
JavaScript worker). Requests run one at a time; one cancelled before it starts is never sent. The
worker stays between hovers and is released after five idle minutes (its device destroyed, its
memory and thread stack freed, then it ends; terminated if it does not answer within 5 s). A worker
that gets no GPU device, or whose canvas fails the probe, draws the same pictures with the CPU
rasterizer (`Previews::create_cpu`) and puts them on a 2D `OffscreenCanvas` to hand them over the
same way; a file the engine cannot read leaves previews to the JavaScript worker. The worker's
first answer says which (`EnginePreviewWorker.stats.backend`, `cpu`, `gpuRefusal`).

**The camera monitor** (`src/camera-monitor.mjs`, in the Camera inspector): where the engine drew
the stage's frame, `drawCameraMonitor` on the stage (`Stage::render_monitor`) draws that frame
again through the scene camera, on the stage's device with a renderer of the monitor's own: a
prepared playback frame from the cache, or the edit frame encoded again with the camera's parallax
and without onion skins (kept while the frame does not change). WebGL2 stages draw the monitor with
the JavaScript Canvas 2D path, as before.

**Home and project pictures** are read from the stage's canvas (`stagePicture` in `src/app.mjs`):
the engine's canvas where the engine draws the scene. They need no path of their own.

**The GPU first, then the CPU.** A thumbnail on the stage's device takes about a millisecond and
samples the stage's own atlas; the CPU takes 3–40 ms in the worker (below) and needs the pictures
decoded for it. So the GPU draws wherever it can, and the CPU where no GPU path exists. The CPU
rasterizer of the time (tiny-skia, since replaced by the GPU's pipeline on the CPU,
[CPU](#cpu-styk-raster)) was tried first for thumbnails and library previews (no device, no shared
atlas, works without a GPU) and dropped, measured in the app: its
edges take three partial-coverage levels where the editor's Canvas 2D takes about 220 and the GPU
about 250 (a library poster: mean difference 0.52 from Canvas 2D on a plain figure, edges only); it
draws no filter effects (such frames had to fall back); and near-coplanar 3D faces resolve
differently from the GPU (15 Guns tick 5 at 512×288: 21 solid pixels against the GPU's picture,
where a receiver face showed through a stock). Drawing on the GPU gives the stage's pixels: an engine
thumbnail equals the engine's export frame of that tick at the canvas's size (`--export-check`
below: 89 of 121 identical, the rest within 2 levels but one pixel at 24, mean ≤ 0.008), and export
frames equal the stage's.

**What differs from the JavaScript paths.** The JavaScript thumbnail and library workers draw with
Canvas 2D in a worker, the editor's fallback renderer: polygons and 3D faces antialiased one by one
(seams), text and 3D artwork as vectors at the output's density, filter effects with the software
filters, generated effects as tinted sprites. The engine draws as the WebGPU renderer does: 3D
artwork and text from object rasters at the editor's raster density (at least half a pixel a unit,
`rasterResolution`) resampled bilinearly, which at thumbnail sizes looks blockier than Canvas 2D
draws small text; seamless faces; the GPU's effects. At 96×54 every stroke is one or two pixels wide,
so edge coverage alone exceeds the app-parity mean (D13).

### Measured

`scripts/engine-preview-parity.mjs` (2026-10-04, RTX 4070 SUPER, the machine shared with other work
at load averages 18–23). Electron under Xvfb with `--use-angle=vulkan` (WebGPU on the NVIDIA
adapter, whose canvases can then be copied); the JavaScript paths in the same page.

**Thumbnails against the JavaScript worker** (D13, limits unchanged; five frame starts per document,
three sizes): 43 steps a size, all drawn by the engine. Passing at 320×192 / 512×288 / 96×54: 10 / 12 /
1. The fixtures of figures, drawings and effects pass at 320×192 and 512×288 (the drawing-cel,
parallax, follow-camera, colour-transition, Stick Nodes sprite and figure-effect fixtures, at most
0.20), Karate-Fighter-Movement (a 3D rig) 9 of 10 (one step with 2 solid pixels); the
generated-effects fixture passes at 512×288 (0.199) and fails at 320×192 (0.293). Scenes with text
and 3D artwork fail at both sizes (Impact-FX-Fight 0.53–1.42, Vehicles 1.09–4.40 with up to 30 solid
pixels, 15 Guns 0.98–3.46, Hand & Face 0.59–1.90, Everyday Props 0.81–3.31, Bamboo 2.39–3.71), the
waterfall plane by the Canvas 2D seams (0.41–0.60, 1–2 solid pixels). At 96×54 only the drawing cel
passes (figures 0.25–1.14, text, 3D and polygon scenes 1.4–15.5). Difference pictures inspected:
edges, small text, 3D faces and particles; no moved, missing or recoloured shape. Against the
engine's own export frames: 121 of 121 steps pass (above).

**Library previews against the JavaScript worker** (Electron, 12 asset-pack files, poster and three
hover frames each): the backdrop chosen is the same for all 12. Posters and frames of 3D figures fail
by the mean (sedan 0.80, helicopter 1.05, Atlas gun 0.50, bullet 0.37, left hand 1.82, face 0.99,
mascots 0.49–0.52; no solid pixel), the dojo wall by the Canvas 2D seams (1.94, 12 solid pixels), the
movieclips' animated frames by their glow and particles (0.50–2.18, at most 10 solid pixels); the
movieclips' posters are blank in both (their clips begin on an empty frame). The fitted bounds of the
three movieclips' hover animations equal the editor's at every compared tick.

**Camera monitor** against the JavaScript monitor (`?engine=off`, Canvas 2D while paused; one tick
each): the figure-effect fixture passes (0.234); the others fail by the mean (parallax and follow
fixtures 0.42–0.45, Karate 0.50 with 2 solid pixels, Impact 1.30, Vehicles 6.68 with 27 solid
pixels, its captions and vehicles drawn from rasters as on the stage).

**Speed and memory** (`--timing`, three interleaved rounds, each renderer created fresh; 320×192
thumbnails of every frame start, at most 48; the app's memory and the GPU memory `nvidia-smi`
attributes to it before and after each renderer):

| Document (thumbnails) | Engine: total, first, median of the rest | JavaScript: total, first, median of the rest |
| --- | --- | --- |
| Karate-Fighter-Movement.styk (44) | 134–158 ms, 3.0–5.5 ms, 2.9–3.0 ms | 1,387–1,476 ms, 857–871 ms, 10.5–12.6 ms |
| Impact-FX-Fight.styk (48) | 85–136 ms, 1.8–5.8 ms, 1.7–2.5 ms | 1,481–2,660 ms, 648–1,128 ms, 12.2–27.2 ms |
| Bamboo-Training-Grounds-Demo.styk (48) | 334–362 ms, 7.6–8.8 ms, 6.8–7.3 ms | 6,502–11,295 ms, 1,186–1,836 ms, 112–221 ms |
| TDD2 URIEL NEMESIS 3.styk (private corpus, 44) | 121–187 ms, 1.9–2.5 ms, 2.7–4.2 ms | 2,171–4,227 ms, 1,563–3,158 ms, 9.2–15.4 ms |
| Jay on a new app (repaired).styk (private corpus, 1) | 6–12 ms | 1,001–1,372 ms |

The JavaScript renderer's first thumbnail starts its worker and copies the document into it; the
engine's draws from the document it holds. Memory around each renderer's first round (all processes
of the app, before → after, the renderer still alive): JavaScript Karate +29, Impact +119, Bamboo
−11, URIEL 3 +61, Jay +9 MiB; engine +11, +1, −1, −5, −5 MiB. GPU memory: the engine's thumbnail
canvas and renderer added up to 42 MiB the first time (URIEL 3: 478 → 520 MiB; Impact +18 MiB), the
JavaScript worker's canvas up to 109 MiB (URIEL 3: 520 → 629 MiB). Later rounds move by tens of MiB
either way (collection, other GPU users of the process).

| Library file | Engine: poster (worker starting), poster, hover start, hover frame | JavaScript: poster, hover start, hover frame |
| --- | --- | --- |
| Vehicles 01-sedan.stkz | 383–488 ms, 12–15 ms, 59–106 ms, 0.5–2.0 ms | 689–899 ms, 796–997 ms, 1.3–2.3 ms |
| Impact 12-hit-spark-fan.stykmov | 281–325 ms, 11–22 ms, 206–276 ms, 0.9–1.6 ms | 706–848 ms, 857–1,001 ms, 9.9–14.0 ms |
| Mascot 01-classic.stkz | 410–438 ms, 14–17 ms, 75–93 ms, 1.7–2.0 ms | 846–876 ms, 942–1,125 ms, 2.8–3.7 ms |
| Dojo 01-dojo-back-wall.stkz | 300–803 ms, 24–34 ms, 75–145 ms, 1.7–2.0 ms | 747–909 ms, 958–1,034 ms, 6.1–9.8 ms |

The camera monitor's time on the interface thread (`drawCameraMonitor`, 278×139, 60 paused frames
after a seek each, two interleaved rounds, load averages 20–27): Bamboo-Training-Grounds-Demo.styk
engine median 0.49–0.66 ms (90th percentile 0.71–1.10, worst 1.69) against JavaScript 75–127 ms
(132–196, worst 260); Vehicles-Pivot-3D.styk engine 0.46–0.53 ms (0.69–0.77, worst 3.85) against
JavaScript 19–40 ms (43–79, worst 106). The JavaScript monitor redraws the scene with Canvas 2D on
the interface thread; the engine's submits a draw on the stage's device and copies its canvas.

Not measured: other GPUs, Windows, macOS, a real display, the web build in browsers with WebGPU,
packaged builds.

**Testing**: `cargo test -p styk-engine --test thumbnails` (thumbnail frames prepared on workers and
never evaluated by asking, the library fit and wrap-around, a file's own pictures),
`cargo test -p styk-engine --lib` (the card's framing), `cargo test -p styk-eval --test vfx_layers`
(`artwork_bounds` with effect padding) and `--test generators` (`Batch::bounds` holds every live
particle), `node --test tests/engine-previews.test.mjs` (which renderer draws a thumbnail, the
preview worker's CPU thumbnails from the engine's document, the fallbacks, the preview worker's
queue, messages made when sent, cancellation, failure and release),
`tests/thumbnail-renderer.browser.mjs` (the JavaScript worker's parallax), and
`scripts/engine-preview-parity.mjs [--target electron --electron-arg --use-angle=vulkan]
[--file …] [--library …] [--monitor] [--export-check] [--cpu-check] [--timing]`.

### The CPU paths

Where no GPU path serves them, thumbnails, library previews and exports are drawn by styk-raster
in their workers, never on the interface thread (D7), with nothing to wait for but the engine's
workers: thumbnails and library previews in the preview worker (`Previews::thumbnail`,
`Previews::create_cpu`), exports in the export worker (`ExportRenderer::create_cpu`,
`styk_export::CpuExport`: tiles of at most 1,024 pixels a side, each drawn when asked for, at most
four waiting, so nothing is drawn ahead). Pictures are the decoded images the engine's workers give
them, shared rather than copied; the preview worker keeps those of the open document's lineage
between thumbnails and lets them go with its idle release (five minutes) or a new document; an
export's go with its worker. `?engineCpu=on` (or `localStorage['stykanimate.engine.cpu'] = 'on'`)
draws all three on the CPU even where a GPU device exists, as devices without one do, for
comparisons. Devices whose stage gets no GPU device at all keep the JavaScript thumbnails and
exports: the engine then holds no document (library previews still use the CPU there).

**Measured** (`scripts/engine-preview-parity.mjs --cpu-check`, Electron under Xvfb with
`--use-angle=vulkan`, 2026-10-04, load averages 8–18): each engine thumbnail drawn by the stage's
GPU and again by the preview worker's CPU, each library poster and hover frame by a GPU preview
worker and a CPU one, and an export frame of each tick at the canvas size by a GPU export worker and
a CPU one, compared by D13. Seven repository documents (15 Guns, Vehicles, Impact, Karate, the
effects, explosions and drawing-cel fixtures) and URIEL 3 (private corpus), five frame starts each,
three library files: 140 steps, 139 pass; the one failure is 15 Guns tick 1's export (9 solid
pixels where near-coplanar faces meet, as natively). Times as the interface sees them (the request
until the picture is on the card), medians over the ticks, GPU / CPU: thumbnails at 320×192
2.3–3.8 / 8.9–27.7 ms for the scenes (15 Guns 27.7, URIEL 3 22.8, Vehicles 15.5, Karate 10.2,
Impact 8.9), 1.4–1.9 / 3.5–7.9 ms for the explosions and drawing-cel fixtures, and 1.4 / 153.7 ms
for the effects fixture (fourteen effects on one figure); at 512×288 0.7–4.5 / 6.3–38.1 ms
(effects fixture 272.2 ms); at 96×54 0.7–6.1 / 1.0–20.7 ms (effects fixture 115.7 ms). An export's
first frame, the worker's start included: GPU 420–1,086 ms (its device), CPU 244–768 ms (effects
fixture 1,035 ms).

## Exports

For a document the engine draws, export frames (PNG, PNG and JPEG sequences, GIF, APNG and
video frames) come from the engine: the frame the stage would show at that tick, through the
scene camera, at the export's size and sampling, without onion skins. The encoders (the PNG,
GIF, APNG and ZIP writers, WebCodecs, FFmpeg, the audio mix) are unchanged and receive the
same canvases and PNG frames as before. SVG stays vector output from the JavaScript exporter,
and so do documents the gate refuses (`engine-eligibility.mjs`).

**Why SVG stays JavaScript.** An SVG export (`src/svg-export.mjs`) is editable vectors: one group
per object (`data-object-id`, `data-movieclip-id`) with its opacity, strokes with their caps and
joins, circles, polygon paths, gradient definitions, text as glyph outline paths, and pictures and
drawing cels embedded at their source resolution (cels flattened from the tile store). The draw
list is the renderer's tessellated form of the same frame, so an SVG built from it would not be
equivalent: polygons and glyphs are triangles, curves polylines, pictures atlas rectangles of
possibly reduced copies, cels 64-pixel tiles of one display level, and objects are not grouped.
Counted on tick 0 (a draw-list census of `styk_eval::drawlist::encode` against the exporter's
elements): Impact-FX-Fight.styk 27 scene items become 4,883 records, 4,855 of them triangles (its
captions), against 141 SVG elements; the drawing-cel fixture's one cel 403 tile quads, against one
embedded PNG; Karate-Fighter-Movement.styk 12,295 3D face triangles, which the exporter embeds as one
rendered picture (frames with 3D, filter or generated effects take that raster branch). Porting
the exporter would mean a second, vector-level output from `styk_eval::geometry` with
JavaScript-exact number formatting, for an export that is not slow: one frame took 11–20 ms
(Impact, 141 elements), 29–39 ms (the waterfall plane, 249 elements) and 117–176 ms (Karate, the
raster branch) in the web build, after 0.52–0.89 s for the JavaScript export worker to start and
receive the document (three rounds each, load average 14–15). The raster branch could take the
engine's export frame later; it is unchanged here.

**Where it runs.** `src/engine/engine-export.mjs` gives the export a renderer:
`EngineExportFrames`, an `ExportFrameRenderer` (`src/export-frame-renderer.mjs`) whose worker
is `src/engine/engine-export-worker.mjs`. The worker joins the engine's shared memory like a
preparation worker but keeps its event loop, holds a GPU device of its own (WebGPU in the
worker, else WebGL2 on an `OffscreenCanvas`) and runs `ExportRenderer` (`styk-wasm`): nothing
of an export is evaluated, drawn or read back on the interface thread (D7). Requests, progress,
revision checks, timeouts, the retry ladder and the output canvas are the base class's, so the
export dialog and the encoders see the same object. Before the worker starts, the document is
brought to the export's revision through the playback adapter's synchronisation
(`exportDocument`, the one playback uses and the app keeps current with every edit) and then
what only the stage knows (figures edited in place, drawings just published: `prepareExport`).
Starting the worker (its modules, joining the engine, the GPU device and the renderer's
pipelines) took 400–500 ms in Electron, so the export dialog starts one when it opens
(`prewarmExportRenderer`, `ExportDevice`); the export takes it, and a worker no export takes
within a minute is released. A worker that cannot get a GPU device draws the export with the CPU
rasterizer instead ([The CPU paths](#the-cpu-paths)). A document the engine does not draw, a page
without worker threads (not cross-origin isolated) or an engine worker that cannot start the export
exports with the JavaScript worker, decided before the first frame; an export never mixes the two. `?engineExport=off` (or
`localStorage['stykanimate.engine.export'] = 'off'`) keeps the JavaScript renderer for
comparisons.

**Frames.** `styk_engine::export::ExportFrames` pins the generation of the export's revision
and evaluates ticks as playback does: effects drawn (exports ignore the editor's effects
preview), the session's hidden figures shown, the document's hidden layers hidden, generated
effects with every earlier anchor evaluated. Up to eight following ticks are prepared on the
engine's workers. The pose the editor shows (a PNG of the current frame) crosses as document
chunks, like the JavaScript worker's, and is drawn by an `EditScene` without onion skins. Every
picture of the document is decoded for the export's device before the first frame, so an
export never shows a frame without one.

**Settings** (`styk-export`):

| Export setting | Engine |
| --- | --- |
| Size | The canvas scaled to the output: `scale(output / canvas)` then the scene camera (`view::tile_view`); an output of another shape stretches (`ViewParams::stretch`), as `drawExportScene` does. |
| Smooth edges | `Edges::Smooth`: four samples a pixel, analytic shapes covered by distance; sprites to the nearest texel. |
| No smoothing | `Edges::Crisp`: one sample a pixel at its centre, every shape fully in or out (hairlines one solid pixel), lone glyphs from their outlines, layers and drawing cels to the nearest texel; opacity kept. Unlike the JavaScript exporter, which draws frames with effects, generated effects or 3D smooth (its vector path cannot carry them), crisp edges apply to every frame; filtered pixels stay filtered. |
| Smooth sprites | Drawn at twice the width and height, each 2×2 block averaged in premultiplied colour (`pixels::resolve`, the rule of `downsampleExportPixels`). |
| Background | The scene's, from the draw list; transparent where the scene has none. |
| Video without alpha | Composited over black by the engine before any rounding (`matte`). |
| Scene effects | The camera image follows the output's density (`ViewParams::camera_limit` unbounded) within the effect surface (3,968 pixels); the stage caps it at 2,048. |

**Memory** (D11). A frame is drawn in equal tiles of at most 2,048 rendered pixels a side
(`tiles::TILE_SIDE`), each the same view shifted by whole pixels: the renderer's targets (frame,
samples, isolation atlas pages, depth) stay those of one tile whatever the output, about the
stage's own. Tiles are read back into at most eight slots of 64 MiB together; the worker keeps
one tile's pixels and the output canvas. The export's device holds its own sprite atlas, the
size of the stage's. Frames prepared ahead are at most eight draw lists. A GPU out-of-memory
error or a lost device retries the frame with half the tile side, then once with a new worker
and device (the base class's ladder), then fails; nothing is written.

**Read-back.** A browser hands a mapped buffer over only when its GPU process gets to it. In
Electron under Xvfb that took 3–5 ms a frame while the page was still and 60–320 ms while the
page repainted (the export dialog's progress repaints every frame), against well under a
millisecond of drawing. So the worker draws the export's next frames ahead into free read-back
slots (at most six frames) while earlier ones are mapped, and puts a frame on its canvas only
when the encoder asks for it; frames drawn ahead for another order (a second GIF pass, the
current pose) are given back unread. WebGL2 (a worker without WebGPU, as headless Chromium here)
completes a read-back only when its device is polled: the worker polls between turns of its event
loop until it does.

**Cancellation and failure.** Cancelling, closing the dialog or a revision change rejects the
pending request at once (the base class); the worker is asked to `release`: it destroys the
device, lets the read-backs that rejects settle, frees the renderer and gives back its thread's
stack, then closes ([threading.md](threading.md)). A worker that does
not answer within 5 s is terminated. The save layer is unchanged: a cancelled or failed export
leaves an existing file untouched.

**What differs from the JavaScript exporter.** It draws with Canvas 2D in a worker: its
antialiasing, text, 3D (faces filled one by one, with seams), filter effects and generated
effects (tinted sprites, colour ramps) are the editor's Canvas 2D fallback's, not its WebGPU
renderer's. Engine export frames are the stage's pixels (D13 on the stage at the same frame,
below); against the JavaScript export they differ along edges, in text, effects and particles,
and in No smoothing on frames the JavaScript exporter drew smooth.

**Why the GPU, measured** (`styk export`, native, RTX 4070 SUPER, 1,920×1,080, Smooth edges, the
median of 10 frames each, the same draw lists, the GPU time including the read-back of the
frame; 2026-10-03, load average 6–7; the CPU column is the tiny-skia rasterizer of the time):

| Document | GPU | CPU (`styk-raster`, tiny-skia) |
| --- | ---: | ---: |
| The start document | 12.6 ms | 14.3 ms |
| Impact-FX-Fight.styk (effects, generated effects, captions) | 13.1 ms | 186.9 ms, without its effects |
| Vehicles-Pivot-3D.styk (3D, text) | 13.6 ms | 409.0 ms |
| 15-Guns-Pivot-3D.styk (3D, text) | 13.7 ms | 444.8 ms |
| generators/explosions.json (generated effects) | 13.3 ms | 258.8 ms |
| hitoshi vs suzu tomiee ssj.piv (private corpus, a Pivot import) | 12.3 ms | 179.8 ms |
| TDD2 URIEL NEMESIS 3.styk (private corpus; effects, drawing cels) | 12.2 ms | 97.6 ms, without its effects |

The CPU rasterizer also draws no filter effects and no camera image, and runs without SIMD in
WebAssembly; WebGPU in a worker draws everything the stage draws with the stage's own renderer. A
native renderer in Electron's main process would have added a second build per platform and an
IPC copy of every frame for what WebGPU in a worker already gives.

### Measured

`scripts/engine-export-parity.mjs` in Electron under Xvfb (`--use-angle=vulkan`, so WebGPU is the
NVIDIA adapter), 2026-10-03, the machine shared with other work (load averages 1.6–18 during the
runs; each run notes its own). Frames are the lossless PNGs the APNG, PNG-sequence and native video
encoders receive.

**Pixels.** Engine export frames against the engine's stage drawing the same tick through the
camera at the same size: **identical, every pixel**, at Smooth edges on all seven documents (21
frames; 1,280×720 for the 16:9 documents and the 960×540 ones, 1,280×640 for URIEL 3, 1,140×630
for hitoshi, each the canvas's own shape). Against today's JavaScript exports (mean |Δ| of the
largest channel, alpha included; D13 mean not counting differences of two levels or less; solid
pixels as D13 counts them; three ticks each):

| Document | Smooth edges | No smoothing | Smooth sprites |
| --- | --- | --- | --- |
| The start document | 0.015, 0.014, 0 | 0.010, 0.010, 0 | 0.004, 0.002, 0 |
| Impact-FX-Fight.styk | 0.85–0.95, 0.37–0.47, 0 | 1.55–1.78, 1.06–1.30, 0–5 | 0.48–0.52, 0.14–0.18, 0 |
| Vehicles-Pivot-3D.styk | 1.09–1.65, 0.61–1.23, 0–5 | 2.10–3.50, 1.62–3.09, 0–359 | 0.54–0.73, 0.21–0.42, 0–7 |
| 15-Guns-Pivot-3D.styk (two ticks) | 1.09–1.56, 0.63–1.11, 0–27 | 1.96–2.99, 1.49–2.53, 3–4 | 0.54–0.71, 0.20–0.38, 0 |
| generators/explosions.json | 1.24–1.83, 0.93–1.29, 14–57 | 0.82–1.21, 0.47–0.78, 16–73 | 1.08–1.67, 0.77–1.11, 1–47 |
| hitoshi vs suzu tomiee ssj.piv | 0.32–0.77, 0.11–0.18, 0 | 0.02–0.09, 0.01–0.06, 0 | 0.27–0.63, 0.03–0.05, 0 |
| TDD2 URIEL NEMESIS 3.styk | 0.07–0.32, 0.02–0.07, 0 | 0.04–0.46, 0.02–0.44, 0 | 0.04–0.22, 0.00–0.02, 0 |

Where they differ (difference pictures inspected): along figure and text edges (Canvas 2D against
four samples), in captions (Canvas 2D text against the stage's text, [Text](#text)), in 3D (the
JavaScript exporter's faces resampled from its own raster, softer, with the stage's crisper), in
generated effects (the Canvas 2D fallback's tinted sprites and colour ramps against the WebGPU
shading, the solid pixels of the explosions fixture), and in No smoothing on frames with effects
or 3D, which the JavaScript exporter draws smooth and the engine crisp (the solid pixels of the
Vehicles and Guns rows).

**Speed and memory of the renderers** (each renderer in turn, two rounds, the order alternating;
PNG frames at about 1,920×1,080, the canvas's shape; the time includes starting the renderer and
the first frame; peaks of every process of the app, and the GPU memory `nvidia-smi` attributes to
it; no dialog open):

| Document (frames) | Engine: total, median frame | JavaScript: total, median frame | Peak memory, engine / JavaScript | GPU memory, engine / JavaScript |
| --- | --- | --- | --- | --- |
| The start document (10) | 712–716 ms, 20.2–20.8 ms | 603–649 ms, 15.9–17.2 ms | 1,257 / 1,243 MiB | 263 / 211 MiB |
| Impact-FX-Fight.styk (60) | 2,111–2,131 ms, 24.6–24.7 ms | 6,359–6,799 ms, 80.7–85.6 ms | 1,865 / 1,681 MiB | 447 / 261 MiB |
| Vehicles-Pivot-3D.styk (7) | 759–764 ms, 23.6 ms | 1,299–1,318 ms, 91.0–92.9 ms | 1,335 / 1,406 MiB | 406 / 273 MiB |
| 15-Guns-Pivot-3D.styk (6) | 719–753 ms, 24.1–25.0 ms | 1,210–1,216 ms, 98.7–99.0 ms | 1,424 / 1,483 MiB | 412 / 279 MiB |
| generators/explosions.json (16) | 1,136–1,213 ms, 40.8–42.3 ms | 4,094–4,626 ms, 247.6–298.1 ms | 1,392 / 1,273 MiB | 399 / 340 MiB |
| hitoshi vs suzu tomiee ssj.piv (60, 1,900×1,050) | 1,970–2,027 ms, 23.2–23.4 ms | 5,171–5,217 ms, 66.9–67.4 ms | 1,999 / 1,821 MiB | 718 / 477 MiB |
| TDD2 URIEL NEMESIS 3.styk (60, 1,920×960) | 2,590–3,697 ms, 21.6–31.9 ms | 82,782–86,412 ms, 243.4–252.8 ms | 2,158 / 2,287 MiB | 1,262 / 901 MiB |

The engine's first frame waits for every picture (URIEL 3: 0.8–0.9 s for its drawing cels, where
the JavaScript renderer decodes them frame by frame). Its own GPU device costs GPU memory the
JavaScript renderer does not use (52–361 MiB more here): the read-back slots (up to 64 MiB), the
frame's samples and layer pages, and a second copy of the document's sprite atlas, the stage
keeping its own. Process memory moved both ways (−129 to +184 MiB): the JavaScript renderer's document
copy and raster caches in its worker against the engine worker's tile buffers.

**Exports through the app** (`--formats … --dialog`: the export dialog opens 1.5 s before each
export, then the app's own command exports today's defaults, the canvas size and the whole
animation, through the real encoders and the desktop save; two rounds, the order alternating):

| Format | The start document (10 frames, 960×540): engine / JavaScript | Impact-FX-Fight.styk (240 frames, 1,920×1,080): engine / JavaScript |
| --- | --- | --- |
| PNG of the current frame | 417–495 / 568–1,238 ms | 526–1,031 / 926–981 ms |
| PNG sequence | 1,020–1,342 / 1,008–1,806 ms | 16.4–18.6 / 67.2–102.3 s |
| GIF | 1,349 ms (one round) / 1,405–1,734 ms | 52.7–64.5 / 85.9–121.4 s |
| APNG | 1,063–1,179 / 1,537–1,925 ms | 21.7–24.5 / 94.2–102.7 s |
| MP4 (FFmpeg) | 859–1,177 / 871–1,519 ms | 13.8–19.1 / 57.6–63.7 s |
| WebM (FFmpeg) | 691–1,299 / 1,071–1,590 ms | 14.6–20.0 / 43.2–52.5 s |

Load averages 10–18 during the start document's runs, 3–17 during Impact's. Peak memory of every
process: 1,058–1,164 / 1,044–1,098 MiB and 1,354–1,731 / 1,296–1,672 MiB; GPU memory 233–274 /
231–236 MiB and 262–490 / 253–314 MiB. Before the dialog started the worker, the start document's
exports took 0.06–1.2 s longer with the engine than with the JavaScript renderer (worker start
400–500 ms, now 11–57 ms). The outputs, decoded by `scripts/engine-export-compare.py` (Pillow, the
bundled FFmpeg), engine against JavaScript, mean |Δ| over all frames (worst frame): the start
document PNG, PNG sequence, GIF and APNG 0.016, MP4 0.099 (0.100), WebM 0.123 (0.129); Impact PNG
0.572, PNG sequence and APNG 0.599 (0.669), GIF 1.287 (1.443), MP4 0.983 (1.113), WebM 0.729
(1.933). Impact's PNG frames are much smaller from the engine (240 frames: 83.5 against 473.9 MB):
Canvas 2D dithers its gradients (48.5 % of a frame's pixels differ by exactly one level, the
background's) and the engine does not; GIF 57.7 against 52.0 MB, MP4 1.76 against 1.92 MB.
TDD2 URIEL NEMESIS 3.styk (private corpus, 232 frames at its 800×400 canvas, one round, load
averages 5–9), engine / JavaScript: PNG of the current frame 1,168 / 1,340 ms, PNG sequence 13.9 /
23.0 s, GIF 29.7 / 49.6 s, MP4 13.9 / 22.5 s; peaks 1,809–2,076 / 1,729–1,955 MiB, GPU memory
626–786 / 426–658 MiB; outputs 0.346 (PNG), 0.306 (PNG sequence, worst frame 2.391), 1.205 (GIF,
3.502), 0.685 (MP4, 3.521). Its worst frames are the gaps the stage has too (plan section 6): tick
46, where the JavaScript renderer draws a drawing layer's onion skin and the engine does not, and
ticks 61–62, a drawing cel under blur, bloom and glow sampled bilinearly.

**Cancellation and failure** (`--cancel`, `--integrity`): on Impact-FX-Fight.styk and TDD2 URIEL
NEMESIS 3.styk, ten exports aborted after their first frame left no export worker, every worker
said it had released its memory and device, and the engine's memory stayed where it was (87 and
364 MiB); the app's PNG-sequence export over an existing file, cancelled from the progress dialog
after two frames and failed in the engine's worker (a frame outside the animation), left the file
byte for byte as it was and no export worker. The GPU memory `nvidia-smi` attributes to the app
went back down on URIEL 3 (393 → 384 → 383 MiB); on Impact, in a later run, the first cancelled
export left it 42 MiB higher (249 → 291 MiB) and the failed one after it did not raise it (291 →
290 MiB). The engine's memory grew with the first export of a session (Impact 79 → 87 and 83 → 90
MiB, the allocator's high-water mark) and not afterwards. Before the
release order was fixed ([threading.md](threading.md)), cancelling URIEL 3's export crashed the
page, twice in two runs; it did not in the two runs since.

## Testing

`cargo test -p styk-export` (settings and their limits, tiles covering every pixel once, the
output view, the 2×2 average against the JavaScript rule, and on a GPU: tiles against the whole
frame by D13 with no seam along tile edges, crisp edges without partly covered pixels, Smooth
sprites as the average of a frame drawn twice as large, the black matte);
`cargo test -p styk-engine --test export`; `cargo test -p styk-render --test glyphs` (crisp
edges fill no glyph mask); `node --test tests/engine-export.test.mjs`; `styk export <file>
<ticks> <out> [--size WxH] [--smoothing edges|none|sprites] [--tile N] [--cpu]`;
`scripts/engine-export-parity.mjs` (frames from both renderers and the stage, timings and memory,
the app's own exports per format with `--formats`, cancellation and failure with `--cancel` and
`--integrity`) and `scripts/engine-export-compare.py` (decodes and compares the exports of a
format).

`cargo test -p styk-raster` (the target's samples, the rasterization rules, effect windows
against per-pixel shading; strokes covered by distance, no seams between adjacent triangles,
translucent faces blending in drawing order, filtered layers; each fails on the tiny-skia
rasterizer; on a GPU, six repository documents by D13 against styk-render, and the generator
fixture), `styk raster-parity [--export] [--crisp] [--library] [--census] [--ticks N]
[--out dir] --list …` (the CPU against the GPU over a corpus, above);
`cargo test -p styk-render -p styk-raster`; text: `cargo test -p styk-eval --test text`
(raster entries, glyph records and their mask layouts against numbers the editor printed,
finer curves for large text; each fails without its rule), `-p styk-render --test glyphs`
(masks draw the glyphs, are kept across frames, give way to triangles past density 8) and
the `styk-draw` unit test of `GlyphMask::layout`; `styk render` prints the rounds, layer
memory and glyph masks of a frame; `styk render` / `styk raster` /
`styk diff` for native comparisons; `scripts/engine-app-parity.mjs` for the
running app (Electron WebGPU and Chromium WebGL2, several zoom levels,
[D13](decisions/0013-app-parity-rule.md)).
