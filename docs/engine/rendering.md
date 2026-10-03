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
artwork holding 3D faces has one too, without effects (`LayerKind::Raster`): the
editor draws it from a raster of its own. 3D faces are **depth runs**: a
`K_DEPTH_RESET`, then `K_TRI` records with `F_DEPTH` and their corner depths, depth
tested against each other and placed as a whole in painter order
([spatial.md](spatial.md#drawing)).
Paint records live in `DrawList::paints`; `sample_paint` is the reference
evaluation every renderer reproduces.

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

Drawing cels: every display level's tiles are in the list; the vertex shader
keeps the level `floor(-log2(pixels per cel pixel))` (0–8) and the fragment
shader premultiplies straight-alpha texels and filters bilinearly across tile
borders (each atlas slot carries its neighbours' edge texels; halo tiles fade
past the last tile), as the editor's GPU drawing display does.

Depth runs: a list with faces gets a multisampled `depth24plus` buffer per pass
and two variants of the one shader's pipeline (painter order writing depth; faces
tested `less-equal`), switched where faces begin and end; the vertex shader maps a
corner's depth to its window depth with the editor's expression.

Invariants: no per-frame allocation in steady state; the no-effect path draws
one pass per depth and round and creates no effect resource; a list without 3D
faces creates no depth buffer and draws one instanced draw per pass; WebGL2 limits (no storage buffers, one sampler
per texture, array textures of at least two layers, [D11](decisions/0011-array-textures-two-layers.md)).

## CPU (`styk-raster`)

The same draw list rasterized with tiny-skia (Skia's raster backend, as Canvas
2D): for exports, thumbnails, library previews and devices without WebGPU or
WebGL2. Groups render into their own pixmaps; custom paints shade per pixel
with `sample_paint`; consecutive triangles of one look fill as one path.
`styk raster` renders a frame like `styk render`; on the compared frames the
two differ by less than 0.2/255 on average, along edges (0.30–0.56 on 3D scenes,
whose raster entries the GPU resamples and the CPU draws at the output's density).
Depth runs rasterize with a four-sample depth buffer compared at 24 bits.
Effects are not drawn on the CPU yet.

## In the app

`src/engine/engine-stage.mjs` puts the engine's canvas (`#scene-engine`)
where the JavaScript renderer's is and draws the edit frame, onion skins and
prepared playback frames for documents the engine supports
([D4](decisions/0004-strangler-gate.md)). The WebGPU surface is configured with
premultiplied alpha so the page shows through the pasteboard. Through the scene
camera (prepared playback frames, and edit frames in the camera preview) parallax
layers move: the frame's per-object offsets ([evaluation.md](evaluation.md#the-scene-camera))
join the drag translations of the draw list. Filter effects are
drawn by the engine; the effects preview switch reaches it through
`Stage::set_effects` and `EngineHandle::set_effects`. Documents with generated
effects keep the JavaScript renderer, as do drawings a spatial bone turns; 3D
meshes, surfaces and poses are drawn by the engine.

## Testing

`cargo test -p styk-render -p styk-raster`; `styk render` / `styk raster` /
`styk diff` for native comparisons; `scripts/engine-app-parity.mjs` for the
running app (Electron WebGPU and Chromium WebGL2, several zoom levels,
[D13](decisions/0013-app-parity-rule.md)).
