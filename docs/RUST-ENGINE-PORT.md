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
| `engine/tools/container-parity.mjs` | decoded documents (and `styk container roundtrip` the written bytes) against `src/native-container.mjs` |
| `engine/tools/ik-fixtures.mjs`, `engine/tools/eval-fixtures.mjs` → `cargo test -p styk-eval` | small projects the editor builds with its own commands, and what it evaluates (IK, movieclip replacements); regeneration is byte-stable |
| `scripts/engine-app-parity.mjs` | the running app with and without the engine (`?engine=off`): stage pixels per tick, with onion skins and after playback |
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
| D2 | The document store is a lossless, copy-on-write tree (ordered keys, exact numbers, packed numeric arrays) with typed evaluation views derived per record. | Saves must be byte-identical and unknown fields must survive; the prototype's typed model is read-only and lossy. An edit clones only the records it touches. | done (store, typed views per changed record) |
| D3 | One WebAssembly module with threads (SharedArrayBuffer, COOP/COEP on the web build, Cloudflare and the Electron host). | One shared document for every worker instead of one copy per worker (today each playback worker holds its own). Synchronous queries from the UI thread. | done (headers on every host; playback of supported documents runs on it) |
| D4 | Strangler gate per document for the renderer (`src/engine/engine-eligibility.mjs` names the first feature the engine lacks), per command group for editing. | The app must work at every commit. | renderer gate in use |
| D5 | zstd from the C library (`zstd-sys` 1.5.7) compiled to WebAssembly and natively. | Measured: at level 3 it reproduces the bytes of the vendored zstd build on all 448 version 2 files of the corpus. | done |
| D6 | One threaded WebAssembly build, on a pinned nightly (`nightly-2026-09-15`, `-Z build-std` with atomics, 2 GiB shared memory). A page that is not cross-origin isolated runs the same module without workers, in slices on the interface thread. | Stable Rust ships no atomics-enabled standard library for `wasm32-unknown-unknown`. One build keeps one bundle. | done |
| D7 | The interface thread never blocks: shared state is taken with a spinning lock (critical sections are a queue push or a pointer swap), and no rayon in WebAssembly (parallelism there is whole frames on the engine's workers). | `Atomics.wait` throws on a browser's interface thread; rayon parks the calling thread. Found the hard way: the first frame prepared on the interface thread trapped. | done |
| D8 | The renderer uses instance attributes for primitive records and float textures for its lookup tables (gradients, isolation group placements, polyline vertices), one sampler per texture. | WebGL2 has no storage buffers and binds one sampler per texture. One pipeline serves WebGPU, native and WebGL2; output is identical to the storage-buffer pipeline (mean difference 0.000/255 on the compared frames). | done |
| D9 | The engine build output is `src/engine/pkg/` (git-ignored), built by `node scripts/engine-build.mjs`; CI builds it. | A 4 MiB binary per engine change would bloat history; the web and desktop builds already copy `src/`. | done |
| D10 | Never strip the WebAssembly profile (`strip = false`); wasm-bindgen removes the name section itself. | wasm-bindgen's thread transform finds each thread's stack pointer and thread-local base by symbol name; a stripped module picked the wrong globals and corrupted memory on the first allocation. | done |
| D11 | Array textures always have at least two layers. | WebGL2 creates a one-layer array texture as a plain 2D texture; isolation groups and onion composites sampled black. | done |
| D12 | While commands run in JavaScript, the engine holds a mirror of the document (`src/engine/engine-mirror.mjs`): changed records staged per batch, committed atomically by revision. An edit frame crosses as the pose on screen with `null` for every object whose geometry the editor did not rebuild; onion ghosts are poses kept by an id. | No whole-document transfer on interactive paths; a gesture never waits for a commit. | in use until S5 |
| D13 | App-level pixel parity (`scripts/engine-app-parity.mjs`) fails a step when any pixel differs by more than 32 levels together with all eight neighbours, or the mean difference exceeds 0.25 levels. | The two renderers cover edge pixels by different rules: differences one or two pixels thick along outlines are expected, a moved, missing or recoloured shape differs over an area. The threshold is fixed; it is not tuned to pass. | done |
| D14 | The page creates the engine's shared memory with the largest maximum the browser reserves (2 GiB, else 1 GiB, else 512 MiB; override `stykanimate.engine.memory`), and budgets caches to it. | Mobile WebKit may refuse a 2 GiB reservation. Desktop Chromium and WebKit accept 2 GiB and grow it; the fallback path is exercised by refusing the larger maxima. Real iOS devices are unverified. | done |
| D15 | Editing moves to the engine command by command (`docs/engine/editing.md`): a ported facade method runs in `styk-edit` and returns the changed records and event detail, which the JavaScript document and history adopt; unported commands run in JavaScript and reach the engine through the mirror. Every port is checked by the command-replay harness (`engine/tools/edit-replay.mjs`): documents byte for byte, selection, history and events after every step. | The app keeps working at every commit while about 800 editor methods move; the harness turns each port into a mechanical comparison. | in use: selection, seeking, moving and transforming figures ported ([ADR](engine/decisions/0015-commands-record-for-record.md)) |
| D16 | Engine arithmetic whose results are stored in documents uses `styk-math`: V8's fdlibm 5.3 `sin`, `cos`, `atan2` and its `hypot`, bit for bit. | Rust's `f64::sin` differed from V8 in the last bit for 2.7 % of two million inputs, `f64::atan2` 17 %, the `libm` crate 0.7–0.8 % for `sin` and `cos`; `styk-math` matched on 2.8 million. Byte-identical editing (D15) needs it. | in use ([ADR](engine/decisions/0016-v8-exact-math.md)) |

## 5. Baseline (current app, before any engine change)

Measured on the code of `96ebce85`: a worktree (`tmp/baseline-worktree`, detached at `96ebce85`,
with this checkout's `node_modules`, `.venv` and native binaries linked in) driven by
`scripts/engine-bench-matrix.sh baseline-0 tmp/baseline-worktree <first round> <last round>`,
which runs `scripts/engine-benchmark.mjs` once per configuration and file. Results:
`artifacts/engine-rust/bench/baseline-0/`; tables: `node scripts/engine-bench-summary.mjs baseline-0`,
written to [`docs/performance/RUST-ENGINE-BASELINE.md`](performance/RUST-ENGINE-BASELINE.md).

**Protocol.** Each run starts a fresh profile, opens the file once and never saves it; window
1440×900 (Electron applies its page zoom of 1.25), default settings, light theme, English.
Before each run the driver waits until the 1-minute load average is below 3 (12 threads),
for at most 10 minutes, and records the load at start and end and the busiest processes
(runs that never saw an idle machine are listed as busy). Three rounds; each round runs every
configuration of every file once, the file order reversed on even rounds, so slow changes in
machine load spread over all cells. Cells are the median with the range of the three runs.
Other work on the machine during the rounds ran under `nice -n 19`.

**Configurations:** Electron with WebGPU; Electron without WebGPU (Canvas 2D fallback); the web
build in Chromium and in WebKit; Electron at 4× CPU throttle (WebGPU and Canvas 2D); the web
build in Chromium at phone size with touch. **Files:** `TDD2 URIEL NEMESIS 3.styk`,
`Jay on a new app (repaired).styk`, `svr_uriel.piv`, `tdd2 uriel nemesis.piv` (41 MB).
**Workloads:** open and first frame, a full playback pass, scrubbing, zoom fit→11× and pan at
8× as wheel input, a joint drag in the densest frame, brush strokes on a new drawing layer,
onion skin toggles, PNG sequence, GIF and MP4/WebM export of 120 frames, resident memory of
the whole process tree and GPU memory; cold start (5 runs) and web build size separately.

**Status (2026-10-02 20:20):** round 1 complete for six configurations of all four files
(URIEL 3 and Jay measured from this checkout before any change to `src/`, the code being
identical to `96ebce85`; the other two from the worktree); WebKit round 1 and rounds 2–3 are
running. Headline numbers from round 1 (Electron with WebGPU, one run each, to be replaced by
the medians of three):

| File | Open | Playback gaps over 2 intervals | Longest gap | Joint drag p95 | Peak resident |
| --- | ---: | ---: | ---: | ---: | ---: |
| TDD2 URIEL NEMESIS 3.styk | 2.71 s | 1 | 146 ms | 10.3 ms | 2,360 MiB |
| Jay on a new app (repaired).styk | 0.67 s | 1 | 132 ms | 914 ms | 2,046 MiB |
| svr_uriel.piv | 11.3 s | 0 | 83 ms | 7.1 ms | 3,275 MiB |
| tdd2 uriel nemesis.piv | 21.4 s | 1 | 1,244 ms | 12.2 ms | 3,632 MiB |

Cold start (Electron, WebGPU, editor view, 5 runs): ready in a median of 4,246 ms (4,122–4,515). Web build:
64.2 MB raw, 18.2 MB at Brotli 5, of which the Python runtime is 14.0 / 6.1 MB, JavaScript
24.2 / 4.9 MB and WebAssembly 6.9 / 2.1 MB.

### Tests at the baseline commit

Run on the branch before any engine change to `src/` (code identical to `96ebce85`); logs in
`artifacts/engine-rust/baseline/`.

| Suite | Result |
| --- | --- |
| `npm run check` | passes |
| `npm test` | 4,374 tests: 4,341 pass, 6 fail, 26 skipped, 1 todo |
| Python (`unittest discover -s backend`) | 161 tests pass, 47 skipped |

Failures that are not the port's (same on `main`):

- `tests/hand-face-pack.test.mjs`: 5 tests (lines 41, 80, 127, 185, 222).
- `tests/impact-fx-pack.test.mjs`: 1 test (line 160).
- `tests/autopose-sole-gesture.test.mjs:82` is the documented Linux-only stall, marked todo.
- Browser suites: the timeline tests named in `docs/LINUX.md` and `tests/app.browser.mjs`
  (needs WebGPU) fail the same way before the port; to be re-recorded when the browser
  suites are run for the final comparison.

### Interface before the port

`artifacts/engine-rust/ui/before/`: 292 images, every one settled (three identical captures):
desktop at 1440×900 and 1024×700 and the four touch layouts, light and dark, 40 states.
Command: `node scripts/ui-snapshots.mjs --label before --out artifacts/engine-rust/ui --sizes 1440x900,1024x700 --touch`.
Two states the current interface no longer has are skipped on both sides (`polygon-rows`), and
`recent-files` logs a page error in a browser (it needs the desktop bridge) but is captured.

## 6. Open problems

- **Documents the engine does not draw yet** keep the JavaScript renderer whole (gate D4). In
  the corpus of 457 native files, 39 need nothing more; the others use 3D meshes (387), 3D
  poses (58), 3D surfaces (29), effects (9 with effect clips), text outlines, a follow camera
  or property channels. Drawings are gated
  too (both `.styk` files of the benchmark set have drawing layers): the engine draws committed drawing tiles, but a stroke not yet published lives only in
  the JavaScript drawing resources. Order of work: effects (agent in progress), drawings,
  meshes and surfaces, 3D poses, text outlines, channels, follow camera, parallax.
- **Onion skins of figures with custom paints** differ: the editor keeps a segment's custom
  paint (gradient) under the onion tint and drops only its secondary colour; the engine tints
  everything. Found by `scripts/engine-app-parity.mjs` on Jay (drawings enabled
  experimentally). Fix in `drawlist.rs` `Ctx::brush`.
- **No-WebGPU devices lose the Canvas 2D fallback's seams.** The JavaScript Canvas 2D
  fallback antialiases each triangle of a polygon fill on its own, so light seams show between
  adjacent faces (asset-pack backgrounds such as `04-waterfall.styk` fail the web-target parity
  bar on this alone: 4 solid pixels, mean 0.265). The default WebGPU renderer uses multisampling
  and has no seams; the engine draws like it on every backend (in Electron the engine's picture
  of `04-waterfall.styk` is identical, pixel for pixel, to the WebGPU renderer's). Recorded as a
  deliberate difference: where a device lacks WebGPU the picture now matches what WebGPU
  devices show.
- **The JavaScript modules cannot be deleted yet**: commands, history and the editor's own
  evaluation (handles, picking, the pose it sends to the engine) still run in JavaScript and
  call the evaluation modules the engine now covers. Deleting them needs stage S5 (the engine
  owns the document and evaluates for the editor); until then they are the oracle.
- **The document is held twice until S5** (D12): the editor's JavaScript objects and the
  engine's store and typed views. Measured 2026-10-02 in Electron (WebGPU), one run each after
  opening the file and letting the mirror commit, machine load 4–5 from the baseline rounds
  (`node scripts/engine-memory.mjs <file>`):

  | File | Engine | WebAssembly memory | JS heap | Renderer process | All processes |
  | --- | --- | ---: | ---: | ---: | ---: |
  | Jay on a new app (repaired).styk | on | 153 MiB | 86 MiB | 523 MiB | 1,161 MiB |
  | Jay on a new app (repaired).styk | off | — | 92 MiB | 568 MiB | 1,182 MiB |
  | svr_uriel.piv | on | 268 MiB | 248 MiB | 762 MiB | 1,390 MiB |
  | svr_uriel.piv | off | — | 262 MiB | 1,255 MiB | 1,866 MiB |

  The engine's own memory (store, typed views, assets, decoded images awaiting upload, frame
  cache) is the cost of the mirror; the totals are already lower with the engine because the
  JavaScript renderer's raster caches and the playback workers' document copies are not used
  for documents the engine draws. The JS heap's copy goes when the engine owns the document
  (S5); the final report tracks the trajectory with the same measurement.
- **First use is slow in WebAssembly.** The first file opened after the module loads takes
  about 2.2 s where the same file then takes 0.66 s (URIEL 3). Suspects: baseline-compiler
  code before the optimizing tier is ready, and growing the shared memory. To measure.
- **wasm-opt is not installed** on the development machine: the module is 4,376 KiB without
  it. Bundle budget to be set once it is.
- **The WebGL2 path is only verified on SwiftShader** (headless Chromium). Playwright's
  Chromium does not display a WebGPU canvas here; Electron does.
- **Real iOS and iPadOS devices are unverified** for the engine's shared memory: the page now falls back from 2 GiB to 1 GiB and 512 MiB maxima (D14), but the actual limits of Apple devices were not measured.
- **Some evaluation trigonometry is still the platform's** (D16): tweens now go through
  `styk_math::shortest`, but geometry, the IK solver and 3D quaternions use `f64::sin`,
  `cos`, `atan2` and `acos`. Drawn geometry is rounded to single precision and matches;
  before the engine stores those results in documents (baking IK poses, 3D poses between
  keys), those paths move to `styk-math` (which needs `acos`).
- **Running out of engine memory is not recovered**: an allocation beyond the reserved maximum traps the module, and the stage does not yet drop the engine for the session safely (a trap inside a held spin lock would block the interface).
- **A lone UTF-16 surrogate in a JSON string** is replaced with U+FFFD by the store
  (JavaScript keeps it). No corpus file has one.
- **Clippy is not clean in `styk-eval`**: three `approx_constant` errors in `piv.rs` (code
  carried over from the prototype; `piv.rs` goes when `styk-piv` lands).
- **Small features look crisper than in the WebGPU renderer** on `hitoshi vs suzu tomiee ssj.piv`
  (section 7). Suspected cause: the JavaScript renderer draws figures from cached rasters at
  bucketed densities and resamples them. To confirm with the raster cache disabled, then
  record as a deliberate difference or fix.
- **A joint drag on Jay on a new app (repaired) stalls presentation for about 0.7 s** once
  per drag (benchmark `drag` workload: latency p95 914 ms at the baseline, 651 ms with the
  engine drawing; median 419 → 50 ms). No long task runs meanwhile: the app holds the frame
  while asynchronous editor work completes. Not the old renderer's scene preparation (skipped
  since `1d9e30ad`, unchanged latency). To trace in S5, where the engine evaluates for the
  editor.
- **Edit frames rebuild the whole scene's geometry** when any object changes, and every commit
  resets the whole frame cache (tick-range invalidation, `playback-retention.mjs`, is not
  ported). Fine at the sizes measured so far; the dense-drag benchmark will decide.

Resolved since the last update: WebKit runs here (the missing `libavif`, `libevent`,
`libmanette`, `libgav1`, `libhidapi` and `libyuv` are unpacked from Ubuntu packages into
`~/.cache/styk-webkit-libs` and linked into Playwright's WebKit `sys/lib`; nothing installed
system-wide); pinned IK and layer-sheet movieclip replacements (geometry parity is complete on
URIEL 3).

## 7. Parity results

Current numbers; each milestone that changes them updates this section.

**Containers** (`engine/tools/container-parity.mjs`, `styk container roundtrip`): 461 native
files decode to the same document as `src/native-container.mjs`; all 448 version 2 files are
written back byte-identical.

**Pivot codec** (`engine/tools/piv/parity.sh`, replayed by `styk-piv replay` on the final
binary): 13,369 requests of 679 scenarios answer as the Python codec does: 3,041 imports
(as documents), 121 `STYKPIV1` frame transports (bytes), 5,935 exports (bytes), 4,056 error
responses (status, code and message), 3 native containers, 213 other routes.

**Geometry** (`styk compare` against `engine/tools/reference.mjs`, every primitive and joint):

| File | Ticks compared | Primitives | Joints | Mismatches |
| --- | ---: | ---: | ---: | ---: |
| TDD2 URIEL NEMESIS 3.styk | all 232 | 113,319 | 111,056 | 0 |
| Jay on a new app (repaired).styk | all 26 | 17,238 | 22,698 | 0 |
| hitoshi vs suzu tomiee ssj.piv | 65 (every 4th) | 122,977 | 135,944 | 0 |
| finisher tdd2.piv | 95 (every 4th) | 150,474 | 174,547 | 0 |
| Valthim - Yuki.piv | 2 (every 4th) | 1,487 | 1,801 | 0 |

`svr_uriel.piv` cannot be dumped by the reference tool: its decoded JSON (588 MB) exceeds the
largest string Node can hold. To do: stream the reference from the frame transport.

**Fixtures** (`cargo test -p styk-eval`): 27 IK fixtures built by the editor, 0 mismatches,
and 122 controller outcomes and 599 solved angles equal to the editor's within 4.5e-11 rad
with the same iteration counts; each fixture with a pin fails without the solver. 4 movieclip
replacement fixtures, 0 mismatches, each failing without its replacements.

**In the app** (`scripts/engine-app-parity.mjs`, rule D13). Web target (headless Chromium,
the JavaScript renderer being its Canvas 2D fallback, WebGL2 for the engine), 29 documents
(the repository's eligible test projects and asset packs, `svr_uriel.piv`, `hitoshi vs suzu
tomiee ssj.piv`, `finisher tdd2.piv`, `tdd2 uriel nemesis.piv`), each at five ticks, with onion
skins and after playback: the engine drew 23 documents (6 refused: text outlines 5, follow
camera 1); 160 of its 189 steps pass. The 29 failing steps are six asset-pack backgrounds
(the Canvas 2D seams, see section 6) and `hitoshi vs suzu tomiee ssj.piv`. Electron target
(the WebGPU renderer): the six backgrounds pass (`04-waterfall.styk` identical pixel for
pixel; the others differ only along the document frame's right edge, at most 25 levels);
`hitoshi vs suzu tomiee ssj.piv` passes 2 of 5 ticks. Its failures are by the mean rule only
(no solid areas): edge antialiasing along thousands of spike edges, and small features the
engine draws crisper than the WebGPU renderer, which presents cached figure rasters (on one
row at tick 129, a green spot peaks at 63,212,138 in the engine and 51,158,105 in the
WebGPU renderer; an outline at 255,127,127 against 243,125,124). Not yet classified beyond
that; open in section 6. The start-up document (Blue, IK controllers) and the IK fixtures
`startup-pinned` and `joined-stretch` pass every step.

## 8. Work queue

What remains, in dependency order. Each item ends with its parity results, docs and the
deletion of what it replaces once nothing calls it. "Agent" marks items run by a subagent
(at most two at a time); everything else is done in the main thread.

**S3, the engine draws every document** (then the JavaScript renderer, playback workers and
effect executors are deleted):

1. Effects rendering: every filter type as GPU passes on WebGPU and WebGL2 (agent, running).
2. Generators (particle effects): simulation (`vfx-generator*.mjs`) and drawing.
3. 3D: spatial poses (`_stykSpatial`), perspective projection, surfaces (culling, depth
   order, depth-tested faces), figure meshes (skinning, view targets), movieclip meshes,
   depth-tested rendering with painter-order splitting.
4. Property channels on native timelines (scalar, colour, style, effect parameters, camera,
   scene) and pose channels, with animation origins.
5. Small gates: rotated and mirrored views (renderer), follow camera, layer parallax, colour
   transitions, Stick Nodes sprites, frame drafts, retained movieclip sources,
   `runtime_segments` fallbacks.
6. Every other drawing path through the engine: thumbnails, library previews, the camera
   monitor, the drawing workspace (live strokes), export frames (S4).
7. A CPU rasterizer for exports, thumbnails and devices without WebGPU or WebGL2.
8. Delete the JavaScript renderer, raster caches, stand-ins and density buckets, playback
   workers, effect executors; the eligibility gate disappears.

**S1, formats:** the Rust codec in the app in place of Pyodide and the Python sidecar (agent,
running); then the Python codec stays only as the development oracle. Later, opening and
saving `.styk` through Rust once the engine owns the document (S5).

**S4, export:** frames, PNG sequence, GIF, APNG, SVG, ZIP and video frames from the engine;
the audio mix and WebCodecs encoders stay platform services.

**S5, the engine owns the document:** `styk-edit` with history; commands ported group by
group, each checked by a command-replay harness (the same calls run in the JavaScript
editor and in Rust; documents compared byte for byte after every step, undo and redo
included); handle positions, picking and snapping as engine queries; the JavaScript
`Editor` becomes a facade whose unported commands run on a checked-out JSON copy and write
back through the mirror until none remain. Then the JavaScript evaluation, geometry,
IK, rig, movieclip and effect-model modules are deleted.

**S6, drawing raster engine** (brush, fill, selection, tiles, history) in Rust.

**S7:** remove dead machinery, final interface screenshots and comparison, final interleaved
benchmarks, `docs/performance/RUST-ENGINE-PERFORMANCE.md`, documentation set (§5 of the
task), `CHANGELOG.md`, Windows CI.

## Milestone log

| Date | Milestone | What landed | Measured |
| --- | --- | --- | --- |
| 2026-10-02 | S0 | Branch, this plan, inventory (355 engine modules), `engine/` workspace from the prototype, UI snapshot tool with touch layouts and chrome-only comparison, "before" screenshots (292), baseline test results, benchmark driver (`scripts/engine-benchmark.mjs`). | Baseline suites as in section 5. |
| 2026-10-02 | S1a containers | `styk-doc` (lossless store, JavaScript-exact JSON), `styk-formats` (container reader and writer, native document rules). | 461 native files: Rust and JS readers agree on every document; all 448 version 2 files are written back byte-identical. URIEL 3 (43 MiB skeleton) decodes into the store in 0.40 s natively, 63 MiB resident. |
| 2026-10-02 | S2 start | `styk-eval` reads the store; `styk-engine` (generations, scheduler, cache, image decoding); threaded WebAssembly build; renderer without storage buffers; `src/engine/engine.mjs`; `engine/viewer`. | In headless Chromium (WebGL2 on SwiftShader, 8 workers) URIEL 3 opens and draws; frames prepare in 3.5 ms on average in WebAssembly. In Node the module opens URIEL 3 in 0.66 s and `svr_uriel.piv` in 0.19 s once warm. Geometry parity unchanged from the prototype (Jay exact, URIEL 3 21 of 24 sampled frames; pinned IK missing). |
| 2026-10-02 18:32 | S2 incremental views | The `Loader` lives as long as a document lineage and keeps the typed figure, keyframe and pose source of every record whose tree node is unchanged; images interned by payload; style overlays resolved once per base figure. | An edit that replaces one keyframe converts one keyframe (`tests/incremental.rs`). |
| 2026-10-02 18:37 | S3 mirror | `src/engine/engine-mirror.mjs` and `styk-engine` `mirror.rs`: changed records staged per batch (properties, table layouts by old index, lifted records, pose sources) and committed atomically by revision; an invalid batch changes nothing. `EditScene` draws the pose on screen from the changed objects only (D12). | 8 mirror tests (`cargo test -p styk-engine --test mirror`). |
| 2026-10-02 19:03 | S3 editing view in the app | `src/engine/engine-stage.mjs` wraps the JavaScript renderer: for a supported document the engine draws into its own canvas, stacked where `#scene` is; the gate (`engine-eligibility.mjs`) names the first missing feature. Cross-origin isolation headers on the dev server, the desktop host and the web build. D10. | Demo document, engine against the Canvas 2D renderer: 3.4–4.4 k differing pixels at 1440×900, mean 0.055/255, largest 53–79, all on edges. A background commit is seen about 10 ms after it lands instead of 253 ms (the engine now calls `Atomics.notify`). |
| 2026-10-02 19:09 | S3 playback in the app | `src/engine/engine-playback.mjs`: for a supported document a playback packet is a ticket for a tick the engine prepared on its own threads; the playback workers get no copy of the document. | Demo (47 ticks at 24 fps) in headless Chromium (WebGL2 on SwiftShader), measured at 19:50 while the baseline benchmark ran: all 47 ticks presented in order, all by the engine; tick gap median 38.9 ms, largest 77.2 ms (the JavaScript renderer in the same conditions: 49.5 and 101.1 ms). |
| 2026-10-02 19:22 | S2 inverse kinematics | `styk-eval/src/ik/` (solver, follower and attachment tables, rig projection and refinement, analytic spatial limb, joined assemblies, `interpolateIK`), `parity.rs` shared by `styk compare` and the tests and now comparing joints too, 27 editor-built fixtures (`engine/tools/ik-fixtures.mjs`), [`docs/engine/rigs-ik.md`](engine/rigs-ik.md). Written by a subagent; reviewed, merged and re-verified here. | URIEL 3 frames with differences: 31 → 4 of 232 (every tick). Fixtures: 0 mismatches; 122 controller outcomes and 599 solved angles agree with the editor within 4.5e-11 rad with the same iteration counts. Jay: 0 mismatches. Cost on Jay +0.09 ms per frame (0.75 → 0.84). |
| 2026-10-02 19:25 | S3 onion skins | Ghost poses kept by id with their geometry; tint, fade and the placement order of `composeSceneGroups` (`onion.rs`). D11. | Demo with onion skins, engine against the JavaScript renderer, ticks 6, 13, 30: 3,634 / 4,373 / 3,734 differing pixels, largest 79 / 65 / 53, mean 0.057 / 0.054 / 0.060, edges only. |
| 2026-10-02 19:33 | S2 movieclip replacements | Layer keys that replace a movieclip with other figures (capture, break apart, make unique) are applied as `replaceMovieclipFrame` does, tweened along their own pose track; `engine/tools/eval-fixtures.mjs` and the shared `fixture-kit.mjs`. | URIEL 3, every tick: 232 of 232 frames, 113,319 primitives and 111,056 joints, 0 mismatches. Jay: 26 of 26 frames, 17,238 primitives, 22,698 joints, 0. Four fixtures, each failing without its replacements. |
| 2026-10-02 20:05 | S3 IK in the app; tools | Documents with IK controllers are drawn by the engine (the gate no longer refuses them). `scripts/engine-app-parity.mjs` (D13), `scripts/engine-bench-matrix.sh`, `scripts/engine-bench-summary.mjs`; the inventory gains a **partial** status and coverage shares. WebKit runs here. | App parity: see section 7. Inventory: 3 modules covered by Rust, 32 partly (1 % and 17 % of the engine source), none deleted yet. |
| 2026-10-02 20:10 | S1 Pivot codec (agent, reviewed and verified) | `styk-piv`: Pivot 2-5 and `.stk` reading into the store, Pivot 5 and `.stk` writing (bit-exact zlib 1.3.1), the codec service (import JSON and `STYKPIV1`, export, source cache, leases, cancellation, Python's errors); `docs/engine/formats-piv.md`. Not wired into the app yet. | Replay of the Python recording: 13,369 of 13,369 requests identical (section 7). One run each under load: `svr_uriel.piv` imports as JSON in 0.96 s (Python 2.54 s), `tdd2 uriel nemesis.piv` in 0.91 s (2.19 s). 37 unit and 59 integration tests. Inventory: 20 modules covered, 35 partly (7 % and 21 % of the engine source), none deleted. |
| 2026-10-02 20:15 | S3 renderer fixes found by app parity | Under WebGPU the engine presented the pasteboard black (wgpu lists only opaque alpha for WebGPU canvases; the stage now asks for premultiplied). Isolation layers that overflow the atlas are drawn in full-resolution rounds instead of at half resolution (`plan_rounds`, three unit tests). `engine-app-parity.mjs` gains an Electron target. | `04-waterfall.styk` in Electron: identical to the WebGPU renderer. `hitoshi vs suzu tomiee ssj.piv` tick 257: 2 rounds, no downscaling; mean difference 1.71 → 0.70 levels. |
| 2026-10-02 20:24 | S3 onion paints | Onion skins keep a segment's custom paint under the tint, as the editor's do (found by app parity). | `tests/onion.rs` fails without the fix; a painted test figure matches the editor's renderer with onion skins, edges only. |
| 2026-10-02 20:30 | S3 text outlines | Figure outlines around text: each visible glyph stroked with round joins, tessellated once per glyph and width. The last feature that kept the large Pivot files of the corpus on the JavaScript renderer. | `finisher tdd2.piv` matches the WebGPU renderer at five ticks; `tests/text.rs`. |
| 2026-10-02 20:33 | S2 effect evaluation (agent, reviewed and verified) | `styk-eval/src/vfx/`: per-pose stacks, effect clips, movieclip container stacks, connected groups, generated effects as resolved parameters; `reference.mjs --vfx`; nine editor-built fixtures; [`docs/engine/vfx.md`](engine/vfx.md). | URIEL 3: 232 of 232 ticks, effects included, 0 mismatches; the nine fixtures match. |
| 2026-10-02 20:36 | S3 edit-frame fields; parity rule | A frame whose own fields change (background, camera, order) is resent even with no changed object. The app-parity mean ignores differences of two levels or less (D13). | `svr_uriel.piv`'s last tick now shows the background alone, as the editor does. |
| 2026-10-02 20:53 | S3 drawing layers | Drawing cels: exact box-reduced levels 1–8, per-frame level choice, premultiplied bilinear filtering across tiles with halo tiles, view-independent draw lists; cels decode in the background. Unpublished strokes and drawing previews stay with the JavaScript renderer. `engine-app-parity.mjs --zoom`. | Electron, WebGPU: Jay on a new app (repaired) passes at every compared tick, with onion skins and at 0.3× and 3×; its largest cel alone is identical at 1×, 2× and 4×. |
| 2026-10-02 21:04 | S3 return-to-canvas preparation | The stage no longer asks the JavaScript renderer to prepare a scene for documents the engine draws. | Same frame as before on return from playback; the ~0.7 s joint-drag stall is unchanged and recorded in section 6. |
| 2026-10-02 21:22 | Structure; S1 Pivot in the engine; CPU raster | `styk-draw` (the draw-list format, shared); `styk-render` split into pipeline, frame, isolation, images, resources, present, view and effects; `styk_eval::open` reads `.piv`/`.stk` through `styk-piv` and the prototype reader is deleted; `styk-raster` (tiny-skia) draws draw lists on the CPU (`styk raster`). | Geometry parity unchanged on hitoshi, finisher and Valthim (0 mismatches). CPU against GPU: Jay's largest cel identical; URIEL 3 tick 60 0.17/255 mean, Jay tick 13 0.03, hitoshi tick 129 0.18, along edges. `svr_uriel.piv` opens in the CLI in 461 ms (prototype reader ~190 ms). |
| 2026-10-02 21:28 | Scaffold and docs | ADRs D1–D13 in `docs/engine/decisions/`, subsystem pages, `AGENTS.md` and `docs/ARCHITECTURE.md` updated; `npm run engine:build`, `engine:test`, `engine:parity`, `engine:bench`. | `engine:parity` repository tier passes. |
| 2026-10-02 21:31 | CI | `.github/actions/engine-build` (pinned nightly, wasm-bindgen 0.2.129, caches) before the web build, packaging and releases; an engine job runs the Rust tests on Windows and Linux; `web:build`, `pack` and `dist:*` refuse to run without `src/engine/pkg`. | Not run on GitHub Actions from here. |
| 2026-10-02 21:34 | Mobile memory (D14) | The page reserves the engine's shared memory itself: 2 GiB, else 1 GiB, else 512 MiB; `stykanimate.engine.memory` forces one. | Playwright Chromium and WebKit, isolated and not: all three maxima accepted and growable; refusal falls back to 512 MiB. Real Apple devices unverified (section 6). |
| 2026-10-02 21:36 | Mirror memory | `scripts/engine-memory.mjs`; the doubled document (D12) measured with the engine on and off (section 6). | Jay: engine WebAssembly memory 153 MiB; all processes 1,161 MiB on, 1,182 off. `svr_uriel.piv`: 268 MiB; 1,390 on, 1,866 off. |
| 2026-10-02 21:41 | S5 design and JavaScript harness | [`docs/engine/editing.md`](engine/editing.md) (D15): commands ported one at a time record for record, then engine queries, then the engine owns the document. `engine/tools/edit-replay.mjs` and `edit-scenarios.mjs` record the editor's report, state, results and change events after every step. | Recordings are byte-stable. |
| 2026-10-02 21:55 | S5 phase 1 start | `styk-edit`: selection (with add, toggle and `selectMany`), seeking, gestures (begin, commit, cancel, translation scope), moving figures (`fromStart`, `detachExternal`, joined roots refreshed from the engine's geometry), undo and redo; `styk edit-replay`; a cargo test replays every recording. | 7 recordings, 81 steps: the four scenarios of ported calls (61 steps) match the editor at every step (document bytes, state, results, events); the other three match up to their first unported call. Inventory: 22 modules covered, 40 partly (7 % and 25 % of the engine source), none deleted. |
| 2026-10-02 22:07 | S5 transforms; V8-exact maths | `styk-edit`: `rotateSelection`, `scaleSelection`, `scaleSelected`, `mirrorSelection` (whole figures), `rotateFigure`, `centerFigure`, `resetPose`, pivots from the engine's geometry. `styk-math` (D16): V8's fdlibm `sin`, `cos`, `atan2` and `hypot`, now also the IK solver's `hypot`. ADRs D15 and D16. | 9 recordings, 122 steps: six fully ported scenarios (87 steps) match at every step; transform-native matches 20 of 21 steps, up to its deliberately unported branch flip; the other two up to their first unported call. `styk-math` against V8: 0 of 2.8 million inputs differ (Rust's `f64::sin` 2.7 %, `f64::atan2` 17 %). |
| 2026-10-02 22:13 | S5 joint gestures | `styk-edit`: `poseJoint`, `rotateJoint`, `bendJoint` with stretching, isolation and snapping back to the definition; rotations carried into joined figures (`rotationEdits`); `styk_eval::geometry::segment_pose` shared by the geometry and the editor. Rigs, IK, 3D bones and Stick Nodes curves answer `Unported`. | 12 recordings, 188 steps: ten fully ported scenarios (159 steps) match at every step, including every snap branch; transform-native matches 20 of 21 steps up to its unported branch flip; move-sheet waits for layer sheets. |
| 2026-10-02 23:39 | S5 evaluated frame as records | `styk_eval::records`: what a gesture's commit stores (`snapshotPoseFrame(editor.evaluatedFrame)`), for imported layers, authored pose keys, the layer composition, movieclips leaving the frame and layer order; numbers from the typed evaluator. `engine/tools/records-reference.mjs`, `records-fixtures.mjs`, `styk records`, `tests/records.rs`. | Byte-identical to the editor at every tick it produces: Jay on a new app (repaired) 26 of 26, TDD2 URIEL NEMESIS 3 228 of 232 (4 need movieclip frame replacements), 158 of 322 ticks of the 42 editor-built documents (the rest: IK, effect and movieclip-replacement tweens, not ported yet). No produced frame differs. |
| 2026-10-02 23:45 | S5 evaluated records, complete on the corpus | Tweened IK controllers, figure and scene effect stacks and movieclip frame replacements as records; evaluation tweens angles through `styk_math::shortest` (D16: the platform's trigonometry gave `-0.06` for a stored `-0.05999999999999999`). | Byte-identical at every tick: TDD2 URIEL NEMESIS 3 232 of 232, Jay on a new app (repaired) 26 of 26, the 42 editor-built documents 322 of 322. Geometry parity unchanged (URIEL 3: 113,319 primitives, 111,056 joints, 337 effects, 0 mismatches; Jay 0). |
| 2026-10-02 23:50 | S5 evaluated records on Pivot imports | Records follow the editor's unwrapping of Pivot-eased angles along the whole run of keys (`evaluate_native_exact`; the drawing path keeps its constant-time unwrap). Fixture `eval-legacy-winding`. | Pivot files, every 7th tick: hitoshi vs suzu tomiee ssj 37 of 37, finisher tdd2 55 of 55, AyameDemo 105 of 105, Uriel 19-2-2026 60 of 60 identical (hitoshi tick 182 differed by 2π in one angle before). Fixtures 343 of 343; the new one fails without the change. |
| 2026-10-02 23:56 | S2 layers mixing imported and authored keys | After an edit, an imported layer has both kinds of keys; the editor tweens it through a window of the keys around the playhead. The typed evaluator held the authored pose instead (a wrong picture between such keys during playback); it now builds the same window (`mixed_window`), and the records follow. Fixture `sheet-mixed-keys`. | The fixture's 13 ticks: 81 of 117 primitives and 99 of 143 joints differed before, 0 now; its records are identical (fixtures 356 of 356). |
| 2026-10-03 00:07 | S5 gestures on a layer sheet | `styk-edit` runs every ported call the way the app does: a draft of the evaluated frame (`authored.rs`), the commit through the sheet planner (`sheet.rs`: `_commitSheetPose`, `write`, `capture`, `materialize`, `publish`), `layer-sheet` history with undo, redo and the collection of sources from a dropped redo branch; an `Unported` call restores everything it touched. The replay state gained the layer revision and the next frame identity. | 16 recordings, 279 steps: the 15 scenarios of ported calls (258 steps, five of them on layer sheets: 99 steps) match the editor at every step in document bytes, state, results and events; transform-native 20 of 21 (a branch flip is not ported). |
| 2026-10-03 00:16 | S5 movieclips on a layer sheet; cost of a gesture | Gestures on documents with movieclips: an instance is edited in the frame replacement that places it, other figures bake the replacement into their new pose, patch-only boundaries are fixed first; the committed draft stays on show until invalidated, as in the editor (it differs from a fresh evaluation after editing a replacement between its keys). `styk edit-bench`. | 18 recordings, 325 steps: the 17 scenarios of ported calls (304 steps) match at every step. On Pivot imports of about 100 MB (native, one run each, busy machine): a move step 0.01 ms, a joint-pose step up to 0.56 ms, a commit 2–7 ms, the first call on a document 68–102 ms. |

In progress: the Rust codec in the app in place of Pyodide and the Python sidecar (agent);
effects rendering as GPU passes (agent); baseline round 3 (section 5). Next in the main
thread: the facade (the app's gestures run in the engine, `docs/engine/editing.md`), then
the remaining gesture paths (IK targets, rigs, movieclips on a sheet) and the next command
groups (frames and keys, layers).
