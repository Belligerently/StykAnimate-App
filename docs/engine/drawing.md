# Engine: the drawing raster engine

The raster engine of the Draw workspace (docs/DRAWING.md) in Rust: `engine/crates/styk-drawing`,
run by the drawing workers and the brush model through its own WebAssembly module
(`engine/crates/styk-drawing-wasm` → `src/engine/pkg/styk_drawing.js`), reached from
JavaScript only through `src/engine/engine-drawing.mjs` and `src/engine/engine-drawing-worker.mjs`
([D17](decisions/0017-drawing-raster-engine.md)). Stage S6 of the plan.

## What runs where

| Thread | JavaScript before | Engine now |
| --- | --- | --- |
| Interface (controller) | `BrushStroke` (drawing-brush.mjs): samples → dab shapes | `RustBrushStroke` (`createBrushStroke`): the same shapes, tails, tail versions |
| Stroke worker | `StrokeEngine` (drawing-stroke-engine.mjs, `DrawingStroke`) | `Strokes` (the same `begin`/`dabs`/`end`/`cancel`, batches and undo records) |
| Pixel worker | drawing-worker-operations.mjs, `prepare-tiles`, the preview | `drawingEngineOperation`: fill, wand, filters, fill-mask, select, selection, paste, resample, decode-tiles, decode-legacy, encode-pngs, encode-tiles, copy-selection, thumbnail, prepare-tiles, preview |
| History worker | drawing-history-codec.mjs | `encodeHistoryChunk`/`decodeHistoryChunk` (IndexedDB, hashing and compression stay platform services) |

Each worker instantiates its own module (no threads, no shared memory, stable Rust), so a
worker is still terminated to cancel its job and its memory goes with it. Tiles enter the
module one copy each (`Tiles`, owned buffers) and leave one copy each (`Output`, copied into
fresh, transferable arrays); a commit's full replacement tiles return as the buffers they
arrived in. Every module object is freed after the call. Canvas work (preview bitmaps,
thumbnail scaling) and base64 stay in JavaScript.

Memory: a module's linear memory grows to its largest job's working set and never shrinks,
where JavaScript's garbage collector would release it. So after each job a pixel worker whose
instance holds more than `DRAWING_ENGINE_RETAINED_BYTES` (256 MiB, room for the 64 MiB
encoded-tile cache) replaces it with a fresh one (`releaseDrawingEngineMemory`); the encode
cache is refilled through the publication protocol's resend of missing tiles. The stroke
worker does the same between strokes, the history worker after every chunk. A preview worker
keeps its source cache (it is bounded by the viewport and accounted by its client). Jobs
themselves are bounded by the same working-memory budgets as before (`maxBytes`, the owner's
hard budget), checked by the same code.

Still JavaScript, and why: the authoritative `TileStore` on the interface thread, its
memory accounting (`drawing-tile-memory.mjs`) and history orchestration (`drawing-history.mjs`,
`drawing-preparation.mjs`'s `DrawingPreparation`), which hold the JavaScript buffers the
renderer, GPU residency and saving share; the renderer's presenters and reduced levels for
display (`drawing-raster.mjs`); the GPU brush (`drawing-gpu.mjs`, WGSL on the JavaScript
renderer's device), which moves when the engine draws the workspace (S3 item 6); "sample
all layers" capture (`drawing-composite*.mjs`, the renderer); Pivot flattening for export
(`drawing-flatten.mjs`, the export path); the brush previews in Properties.

## The gate

`?drawingEngine=off` or `?engine=off` (or the stored `stykanimate.drawingEngine` /
`stykanimate.engine` set to `off`) keeps the JavaScript raster engine; the page appends
`?engine=off` to the drawing workers' URLs (`drawingWorkerURL`). A module that cannot load
(an unbuilt checkout) leaves the JavaScript engine in charge, with one console warning.
The JavaScript modules remain the oracle.

## Exactness

The port follows the JavaScript operation for operation: `Math.round` and the
`Uint8ClampedArray` store conversion, single-precision coverage, grain and soft-falloff
tables where the JavaScript uses `Float32Array`, `Map` insertion order for tiles, undo
records and plans, V8's `sin`, `cos`, `atan2` and `hypot` (`styk-math`, D16) in the brush
model, textures and selections. Published tiles are byte-identical: `zlib.rs` writes the
same deflate stream as `zlib-sync.mjs`, `png.rs` the same PNG. One known gap: `x ** y`
for exponents other than 1, 2 and 0.5 (custom pressure-size and opacity curves) is the
platform's `pow`, which in WebAssembly is the `libm` crate's; browsers already differ
from each other there (D16). Measured natively with that `pow`
(`cargo test -p styk-drawing --features libm-pow --test parity`): in the one fixture with
such curves (size curve 1.7, opacity curve 0.6) the 607 dab shapes differ in their last
digits and every pixel, batch and undo record is still identical; every other fixture is
unaffected (all presets use exponent 1).

## Parity

`node engine/tools/drawing-fixtures.mjs` records the JavaScript engine in Node (inputs and
SHA-256 prefixes of every output; byte-stable regeneration, checked by `npm run engine:parity`);
`cargo test -p styk-drawing --test parity` replays them:

| Fixture file | Cases | What must match |
| --- | --- | --- |
| strokes | 60 strokes, 41,146 dabs, 498 worker batches | every dab shape; every batch's seeded tiles, dirty tiles and rectangle; undo records; the `sample()` path with its live tail |
| brush | settings, pen curves, jitter, dab shapes, atlas, stroke paths | values bit for bit |
| fills | 16 (tolerance, gap closing both ways, expansion, clips, sample-all, blank cels, budgets, refusals) | undo records, tiles or error key |
| masks | wand, dense fill, lasso masks, combine, invert, crop, edges, fill-mask | masks and tiles |
| filters | blur, HSL, brightness/contrast, invert, masked and not, refusals | undo records and tiles |
| selections | polygon, mask, whole-cel and over-budget capture | selected and remaining tiles with bounds |
| transforms | moves, rotations, mirror, extreme scales, removal, resampling, affine, singular and out-of-range | undo records and tiles, coverage, support bounds |
| levels | three cels, levels 1–5 | every reduced tile in order |
| codecs | tile PNGs, deflate, other encoders' PNGs, broken PNGs, blobs, the encode cache, undo chunks, preparation, clipboard, paste, stroke region | bytes and error keys |

`tests/drawing-engine.test.mjs` runs every pixel-worker request, the stroke worker's
protocol, the brush model and the undo codec through both engines' facades in Node and
requires identical replies, and that every error key the crate can raise is in the
English catalogue.
