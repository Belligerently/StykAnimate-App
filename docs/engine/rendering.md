# Engine: rendering

How a draw list becomes pixels: the format (`engine/crates/styk-draw`), the GPU
renderer (`engine/crates/styk-render`), the CPU rasterizer
(`engine/crates/styk-raster`) and the stage in the app (`src/engine/engine-stage.mjs`).

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

## CPU (`styk-raster`)

The same draw list rasterized with tiny-skia (Skia's raster backend, as Canvas
2D): for exports, thumbnails, library previews and devices without WebGPU or
WebGL2. Groups render into their own pixmaps; custom paints shade per pixel
with `sample_paint`; consecutive triangles of one look fill as one path.
`styk raster` renders a frame like `styk render`; on the compared frames the
two differ by less than 0.2/255 on average, along edges (0.30–0.56 on 3D scenes,
whose raster entries the GPU resamples and the CPU draws at the output's density).
Depth runs rasterize with a four-sample depth buffer compared at 24 bits.
Generated effects expand into the GPU's quads and are shaded per pixel by its
fragment arithmetic, additive ones added (`generators.rs`); on the generator
fixtures the CPU and the GPU differ by less than 0.2/255 on average.
Text in raster entries is drawn at the output's density, and lone glyphs by their
own triangles (`F_GLYPH_VECTOR`), not from masks.
Filter effects are not drawn on the CPU yet.

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

## Testing

`cargo test -p styk-render -p styk-raster`; text: `cargo test -p styk-eval --test text`
(raster entries, glyph records and their mask layouts against numbers the editor printed,
finer curves for large text; each fails without its rule), `-p styk-render --test glyphs`
(masks draw the glyphs, are kept across frames, give way to triangles past density 8) and
the `styk-draw` unit test of `GlyphMask::layout`; `styk render` prints the rounds, layer
memory and glyph masks of a frame; `styk render` / `styk raster` /
`styk diff` for native comparisons; `scripts/engine-app-parity.mjs` for the
running app (Electron WebGPU and Chromium WebGL2, several zoom levels,
[D13](decisions/0013-app-parity-rule.md)).
