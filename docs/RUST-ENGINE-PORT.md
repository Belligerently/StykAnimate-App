# Rust engine port: the living plan

The single source of truth for moving StykAnimate's engine from JavaScript and Python to
Rust. It is updated at every milestone. Read this page and `git log engine-rust` to resume;
nothing here should need re-deriving.

- **Branch:** `engine-rust` (from `main` at `96ebce85`). Not pushed, no PR, nothing released.
- **Rule:** the interface does not change. Every feature stays. A visible change that is not
  strictly faster is a bug.
- **Status in one line:** see [Milestone log](#milestone-log), last entry.

## 1. What "the engine" is

Everything that is not interface or a platform service: file formats, the document, commands
and history, evaluation, geometry, rigs and IK, movieclips, effects, drawing raster work,
playback preparation, rendering, export frame production. The inventory lists every module:

- [`docs/engine/INVENTORY.md`](engine/INVENTORY.md), generated from `engine/inventory.tsv` by
  `node scripts/engine-inventory.mjs --render`. The check (`node scripts/engine-inventory.mjs`)
  fails when a source file has no row, so a new module cannot skip the classification.
- At the start: **355 engine modules, 4,056 KiB of JavaScript and Python** (JS engine 320
  modules; Python codec and service 20; C++ scanner 1), against 165 interface modules, 47
  platform services and 17 application-service modules that stay.

| Rust owner | Holds | Modules at start | Source at start |
| --- | --- | ---: | ---: |
| `styk-doc` | lossless document store, validation, structure | 2 | 28 KiB |
| `styk-formats` | `.styk`/`.stkz`/`.stykmov` containers, PIV/STK codec, Stick Nodes, save | 57 | 728 KiB |
| `styk-eval` | timeline, tweening, layer sheet, movieclips, rigs/IK, camera, effect sampling, geometry, picking | 83 | 774 KiB |
| `styk-render` | wgpu renderer (WebGPU, WebGL2, native), effects, generators, images, CPU rasterizer | 52 | 708 KiB |
| `styk-engine` | the facade: scheduler, thread pool, frame cache, budgets, thumbnails, playback | 24 | 216 KiB |
| `styk-edit` | commands, history, selection, clipboard, sessions | 86 | 1,212 KiB |
| `styk-drawing` | raster drawing: tiles, brushes, strokes, fill, selection, history | 36 | 270 KiB |
| `styk-export` | export frames, PNG/GIF/APNG/SVG/ZIP, audio mix | 15 | 122 KiB |

## 2. Engine boundary

```
 JS (interface only)                         Rust (engine/, one WebAssembly module + native tools)
 ───────────────────                         ────────────────────────────────────────────────────
 app.mjs, panels, timeline, inspector   ──►  commands (batched)            styk-edit
 Editor facade (src/engine/…)           ◄──  versioned query results       styk-engine (facade)
 overlay canvas (handles, guides)       ◄──  handle positions, picking     styk-eval
 platform services (files, codecs,      ──►  bytes in / bytes out          styk-formats, styk-export
   WebCodecs, Web Audio, IPC)
 <canvas>                               ◄──  presents prepared frames      styk-render (wgpu)
```

- The engine owns the document. JS holds no second copy of animation data on interactive paths.
- Commands go in as small messages; small, versioned query results come out (inspector values,
  timeline rows, handle positions, selection). No per-object calls inside frame loops.
- Background work (evaluation, preparation, decoding, effects, export) runs on a WebAssembly
  thread pool over one shared memory; results carry the document revision and stale ones are
  dropped. The UI thread never evaluates or rasterizes during playback or gestures.
- Draw lists are view independent (world units); pan and zoom rewrite one uniform.

The JS side of the boundary lives in `src/engine/` only. Nothing else imports the wasm module.

## 3. Method: strangler stages

The JavaScript engine stays as the oracle until a subsystem is proven, then it is deleted.
Each stage leaves the app fully working, with its tests, parity results and docs.

| Stage | Subsystem | Replaces | Gate while both exist |
| --- | --- | --- | --- |
| S0 | Plan, inventory, baseline measurements, "before" screenshots, workspace, wasm build | — | — |
| S1 | Formats: container reader/writer, PIV/STK codec, Stick Nodes | `native-container.mjs`, `zlib-sync.mjs`, vendored zstd, Python codec + Pyodide + PyInstaller sidecar, C++ scanner | Python codec remains a test oracle only |
| S2 | Document store + evaluation + geometry (all features: IK, channels, meshes, follow camera, text) | `src/pivot/`, evaluation modules | Rust evaluation checked against JS per primitive |
| S3 | Renderer (WebGPU, WebGL2, CPU), effects, generators, images; playback on the thread pool | `renderer*.mjs`, `playback*.mjs`, `prepared-frames.mjs`, `vfx-*` executors, Canvas 2D renderer | per document: a scene using a feature the new renderer lacks keeps the old renderer |
| S4 | Export frames, thumbnails, library previews | `export-frame-*`, `thumbnail-*`, `library-preview-*`, image codecs | per format |
| S5 | Editing: gestures first (pose, move, IK), then commands by group, history, clipboard | `editor*.mjs` and friends | per command group: unported commands run in JS on checked-out frames and write back |
| S6 | Drawing raster engine | `drawing-*` (not the controller/view) | Draw workspace keeps the old path until parity |
| S7 | Remove dead machinery, final docs, final measurements | stand-ins, crop caches, density buckets, settle repaints | — |

Parity bar (never loosened to pass): geometry within float tolerance; pixels equal except at
anti-aliased edges; unchanged `.styk` and PIV saves byte-identical; exports within the
tolerance the current export tests use.

### Parity harness

| Tool | Checks |
| --- | --- |
| `engine/tools/reference.mjs` + `styk compare` | geometry, primitive by primitive, against the JS editor |
| `engine/tools/reference-render.mjs` + `styk diff` | pixels against the app's renderer |
| `styk parity formats` (S1) | decoded documents and written bytes against the JS and Python codecs |
| `scripts/ui-snapshots.mjs --touch --compare … --chrome-only` | the interface, pixel by pixel |

Corpus: every file in `~/Downloads`, `~/Downloads/pivs`, `~/Downloads/WafflesPack` (127 files,
with its `.stk` folders), the repository fixtures and asset packs. `Jay on a new app.styk`
(unrepaired) is rejected by the current editor ("Pose array does not match its figure"); the
engine must reject it the same way.

## 4. Decisions

Each gets an ADR under `docs/engine/decisions/` when it lands.

| # | Decision | Why | Status |
| --- | --- | --- | --- |
| D1 | The engine workspace is `engine/` at the repository root, started from `experiments/styk-native` (kept as the measured reference). | One owner per concern; the prototype's numbers stay reproducible. | done |
| D2 | The document store is a lossless, copy-on-write tree (ordered keys, exact numbers, packed numeric arrays) with typed evaluation views derived per record. | Saves must be byte-identical and unknown fields must survive; the prototype's typed model is read-only and lossy. An edit clones only the records it touches. | planned (S2) |
| D3 | One WebAssembly module with threads (SharedArrayBuffer, COOP/COEP on the web build, Cloudflare and the Electron host). | One shared document for every worker instead of one copy per worker (today each playback worker holds its own). Synchronous queries from the UI thread. | planned (S0/S3) |
| D4 | Strangler gate per document for the renderer, per command group for editing. | The app must work at every commit. | planned |
| D5 | zstd from the C library (`zstd-sys` 1.5.7) compiled to WebAssembly; byte parity with the vendored build to be measured. | Container bytes depend on the compressor version. | open |

## 5. Baseline (current app, before any engine change)

To be filled by `scripts/engine-benchmark.mjs` on commit `96ebce85` code. The final comparison
re-runs the baseline from a worktree, interleaved with the candidate.

### Tests at the baseline commit

To be filled from `tmp/engine-port/baseline-*.log` (`npm run check`, `npm test`, Python tests).
Known before the port: `tests/hand-face-pack.test.mjs` and `tests/impact-fx-pack.test.mjs` fail
at `2ad8edd3`; the Linux-only autopose stall; the timeline browser tests named in
`docs/LINUX.md`; `tests/app.browser.mjs` needs WebGPU.

## 6. Open problems

- Whether zstd 1.5.7 reproduces the vendored build's bytes at level 3 (D5).
- Threaded WebAssembly needs a nightly toolchain (`-Z build-std` with atomics). Pinned in
  `engine/rust-toolchain.toml` once the first threaded build works.
- Playwright's Chromium does not display a WebGPU canvas on the development machine; the new
  renderer's WebGL2 and CPU paths are what browser tests will see.

## Milestone log

| Date | Milestone | What landed | Measured |
| --- | --- | --- | --- |
| 2026-10-02 | S0 (in progress) | Branch, this plan, inventory (355 engine modules), `engine/` workspace from the prototype (builds natively and to `wasm32`), UI snapshot tool extended with touch layouts, more screens and chrome-only comparison. | — |
