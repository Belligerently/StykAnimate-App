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

## The fallback

A module that cannot load (an unbuilt checkout) leaves the JavaScript raster engine in
charge, with one console warning. The JavaScript modules remain the oracle for
`tests/drawing-engine.test.mjs` until they are deleted.

## Exactness

The port follows the JavaScript operation for operation: `Math.round` and the
`Uint8ClampedArray` store conversion, single-precision coverage, grain and soft-falloff
tables where the JavaScript uses `Float32Array`, `Map` insertion order for tiles, undo
records and plans, V8's `sin`, `cos`, `atan2`, `hypot` and `pow` (`styk-math`, D16) in the
brush model, textures and selections. Published tiles are byte-identical: `zlib.rs` writes
the same deflate stream as `zlib-sync.mjs`, `png.rs` the same PNG. `x ** y` for custom
pressure-size and opacity curves is V8's own `pow`, and the fixtures are recorded with Node
pinned to it (`engine/tools/v8-pin.mjs`): Node 24's default is the platform's, which gave
different dab shapes on Windows (MSVC) and Linux (glibc) in the one fixture with such
curves (size curve 1.7, opacity curve 0.6). A browser's `**` is the platform's, so a dab
there can differ in its last digits; the exponents 1, 2 and 0.5 are `x`, `x * x` and a
square root in every engine.

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

## Measurements

Measured on the shared development machine (load average 7–18 during the runs; engines
interleaved in the same runs; numbers are medians unless ranges are given). They are
evidence for these runs, not for other machines.

`node scripts/benchmark-drawing-engine.mjs` (Node, worker requests through both facades,
7 rounds), JavaScript → Rust: enclosed fill 145 → 38.7 ms, whole-stage fill 422 → 138,
magic wand 490 → 150, blur 995 → 407, hue/saturation 225 → 150, selection capture
11.9 → 7.9, rotated transform commit 53.8 → 44.1, resampling to half 92.0 → 81.8 and to
double 526 → 366, publishing 1,069 tiles 1,098 → 429, decoding them 323 → 184 ms. Stroke
worker totals per stroke 0.66–1.01× JavaScript's time and pen-up 0.23–0.39×. Slower in
Rust: preparing 64 tiles 0.43 → 0.88 ms (the copy into the module) and the brush model on
the interface thread 2.6–8.4 → 3.2–9.9 µs per sample (calls across the boundary); undo
chunks encode in 0.27–0.38 ms (JavaScript 0.24–0.36) and decode in 0.20–0.22 ms
(JavaScript 0.26–0.29).

`--memory` (each workload alone in a fresh process; peak resident growth): the module's
linear memory grows to the job's working set, 5–12 MiB for strokes, 40–68 MiB for fills and
the wand, 171 MiB for a whole-stage blur, 34–45 MiB for transforms and publication, where
JavaScript's garbage-collected peaks are 0–22 MiB except blur (149 MiB) and publication
(59 MiB). The instance is replaced after a job once it holds more than 256 MiB.

Live drawing in a browser (`tests/drawing-latency.browser.mjs`, blank 1080p cel at 2×,
Canvas 2D, four interleaved rounds), input-to-visible p50 JavaScript/Rust: small 30.1/29.6,
pencil 40.7/32.3, charcoal 30.1/29.5, soft 160 px 56.1/39.0, oil 26.4/32.9 ms; pen-up to
commit 57.3/52.2, 65.7/52.6, 59.5/52.9, 79.3/79.4, 47.9/57.2 ms. Worker memory after a few
strokes and a fill (`measureUserAgentSpecificMemory`): pixel worker 32.9 MiB (JavaScript
21.5), stroke worker 7.6 (3.6), history worker 6.7 (2.9).

The app benchmark (browser, dense 2,040-tile cel at 2×, large soft
strokes whose undo records are on the disk tier; undo and redo through the app's commands; load average
recorded per run). Before the fix below, undo and
redo back to back took Rust 1,011 and 1,226 ms (688–2,243) against JavaScript 760 and 520
(223–2,235), 4 interleaved rounds, load 14–24. Every undo or redo starts a publication of its
revision (the playback cache's barrier); it ran during the next undo or redo, and when it
finished there, its blob assembly in the pixel worker (150–180 ms, both engines) queued the
replay's tile preparation and the figure update it triggers on the interface thread (the
engine stage syncing the published asset, 0.2–0.7 s, both engines) held up the replay's
replies. The Rust encoder is fast enough to finish that publication inside the next replay;
the JavaScript one was usually superseded first. Publication now pauses while a replay is
prepared (`DrawingHistory.replaying()`, docs/DRAWING.md). Since then, 10 interleaved rounds
(load 8–22): back to back, undo 199 ms (169–499) against JavaScript 222 (140–457), redo 200
(146–373) against 306 (151–460); with publication settled first, undo 634 and redo 659
against 645 and 678 (these include the stage's figure update after the publication the
benchmark awaited, in both engines). An undo or redo is now mostly its 15 history chunk reads
(IndexedDB, decompression, checksum, decoding: 95–113 ms of history-worker time in both engines)
and 7–9 ms of tile preparation. Across the three benchmark runs, fill medians were 96–122 ms
(JavaScript 111–151) and publication 359–394 ms (369–674).
