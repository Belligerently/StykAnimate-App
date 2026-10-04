# Engine: WebAssembly, threads and scheduling

How the engine runs in the app (`engine/crates/styk-engine`,
`engine/crates/styk-wasm`, `src/engine/*.mjs`), and why
([D3](decisions/0003-threaded-webassembly.md), [D6](decisions/0006-pinned-nightly-wasm.md),
[D7](decisions/0007-interface-thread-never-blocks.md)).

## Pieces

- `Engine` (`styk-engine`): the open document as a `Generation` (lineage,
  revision, prepared document, assets), a task queue, the frame cache and
  image decoding. Every change publishes a new generation; results computed
  for an older one are dropped.
- Frame scheduler (`frames.rs`): the interface asks for ticks around the
  playhead (`request`); workers evaluate and encode them (`FrameData`: draw
  list, camera, background) into a cache bounded by `Budgets::for_device`
  (48–384 MiB by `navigator.deviceMemory`), evicting what is farthest ahead
  of the playhead first. `set_hidden` re-keys frames by what is hidden.
- Images: decoded on workers (`take_images` hands them to the renderer a few
  megabytes per frame); drawing cels decode one task each.
- `styk-wasm`: `EngineHandle` (commands and queries over wasm-bindgen),
  `Stage` (a wgpu surface on a canvas, edit frames and ghosts, prepared
  frames), `worker_run` (a worker's loop).
- `src/engine/engine.mjs`: loads the module with shared memory, starts
  `navigator.hardwareConcurrency - 1` workers, at most 8 (`engine-worker.mjs`), and
  without cross-origin isolation runs the same module without workers through
  `pump()`. `join` hands another thread what it needs to join the engine (the
  compiled module, the memory, a stack size, the progress counter's address).
- The export worker (`src/engine/engine-export-worker.mjs`, one per export, see
  [rendering.md](rendering.md#exports)): joins the shared memory like a preparation
  worker but keeps its event loop, because its GPU device (WebGPU in the worker, else
  WebGL2 on an `OffscreenCanvas`) answers through promises. The export dialog starts
  one when it opens (`warm`: joining the engine and getting an `ExportDevice`);
  `ExportRenderer` (`styk-wasm`) takes the device, pins the export's revision
  (`styk_engine::export::ExportFrames`),
  asks the preparation workers for the next frames and its pictures, draws and reads
  back. It ends with `release`: the device is destroyed, the read-backs it rejects
  settle (their answers run through the module), then the renderer is freed and the
  thread's stack and thread-local storage given back (`__wbindgen_thread_destroy`)
  before the worker closes, with no turn of its event loop in between (a call into the
  module after the destructor writes through a stack the shared memory has taken back:
  cancelling an export of URIEL 3 crashed the page that way until the answers were
  let through first); a worker that does not answer within 5 s is terminated
  (what it held then stays allocated in the shared memory). Pages that are not
  cross-origin isolated export with the JavaScript renderer.
- The preview worker (`src/engine/engine-preview-worker.mjs`, one per page,
  `engine-previews.mjs`; see [rendering.md](rendering.md#thumbnails-library-previews-and-the-camera-monitor)):
  joins the shared memory like the export worker and keeps its event loop for a GPU
  device of its own, on which it draws library files' posters and hover animations
  (`styk_wasm::preview::Previews`). It starts with the first request, runs requests one
  at a time and is released after five idle minutes, as the export worker is (device
  destroyed, pending answers let through, then the thread destroyed); it is never
  terminated to cancel a preview (a cancelled request's answer is dropped), because a
  thread of the shared memory terminated mid-call could hold the allocator or a lock.
- Frame thumbnails and the camera monitor are drawn by the stage on the interface
  thread (`styk-wasm/src/views.rs`), the only work done there being the draw itself:
  a thumbnail's frame is prepared on the preparation workers (`ExportFrames::prefetch`)
  and the interface waits for it on the progress counter; `ExportFrames::ready` only
  looks.

## Rules

- The interface thread never blocks: shared state is taken with a spinning
  lock around queue pushes and pointer swaps; waiting is `Atomics.waitAsync` on
  the engine's progress counter, which Rust bumps with `Atomics.notify`. Read the
  counter (`progressSeen()`) before checking for a result and wait with that value
  (`whenProgress(seen)`): work that finishes in between is otherwise only noticed at
  the 250 ms timeout. The mirror's commit on the interface thread only tries the
  mirror's lock (`Engine::try_commit`, [document.md](document.md)).
- No rayon in WebAssembly: `styk_eval::par::map` is sequential there;
  parallelism is whole frames on workers.
- Library code has no clock, file or thread of its own: the host injects a
  clock (`styk_wasm::clock`), and native tools use rayon through `par`.

## Building

`npm run engine:build` (`scripts/engine-build.mjs`): nightly `nightly-2026-09-15`,
`-Z build-std=std,panic_abort`, `+atomics,+bulk-memory,+mutable-globals`,
imported shared memory (maximum 2 GiB), wasm-bindgen 0.2.129 `--target web`,
output in `src/engine/pkg/` ([D9](decisions/0009-generated-engine-build-output.md)).
Never strip the profile ([D10](decisions/0010-keep-wasm-symbols.md)).

## Testing

`cargo test -p styk-engine` (scheduler, mirror, definitions-only commits, a commit
the interface thread finds busy, export frames prepared ahead, thumbnail frames never
evaluated by the thread asking); `node --test tests/engine-export.test.mjs` (the export
worker's start, release and fallbacks); `node --test tests/engine-previews.test.mjs`
(the preview worker's queue, cancellation, failure and idle release);
`engine/viewer/` (a bare page over the engine with `window.viewer` benchmarks);
the app parity tool.
