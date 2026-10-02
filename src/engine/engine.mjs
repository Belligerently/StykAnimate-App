/**
 * The StykAnimate engine (docs/ARCHITECTURE.md): the one module that loads the
 * WebAssembly engine and its worker threads. Nothing else imports `./pkg/`.
 *
 * The engine is one WebAssembly module instantiated once per thread over one
 * shared memory. This thread holds the handle and draws; the workers prepare
 * frames and decode files and images. Shared memory needs a cross-origin
 * isolated page (COOP/COEP headers); without it the engine still runs, doing
 * its background work here between frames.
 */
import init, {EngineHandle, Stage, api_version} from './pkg/styk_engine.js';

/** The command/query API this loader was written for. */
const API_VERSION = 1;
/** Each worker's stack: evaluation recurses through nested movieclips. */
const WORKER_STACK_BYTES = 2 * 1024 * 1024;

/** How many preparation threads a device gets: its cores minus the interface thread, at most eight. */
export function defaultWorkerCount(cores = globalThis.navigator?.hardwareConcurrency || 4) {
  return Math.max(1, Math.min(8, cores - 1));
}

let loading = null;

/**
 * Loads the engine once per page.
 * @param {{workers?: number, deviceMemory?: number}} [options]
 * @returns {Promise<{handle: EngineHandle, workers: number, threaded: boolean, createStage(canvas: HTMLCanvasElement): Promise<Stage>,
 *   open(bytes: Uint8Array, extension: string, name: string): Promise<object>, whenProgress(): Promise<void>, destroy(): void}>}
 */
export function loadEngine(options = {}) {
  return loading ??= load(options).catch(error => { loading = null; throw error; });
}

async function load({workers = defaultWorkerCount(), deviceMemory = globalThis.navigator?.deviceMemory} = {}) {
  const source = new URL('./pkg/styk_engine_bg.wasm', import.meta.url);
  const module = await compile(source);
  const wasm = await init({module_or_path: module});
  if (api_version() !== API_VERSION) throw new Error(`The engine module has API ${api_version()}, this page expects ${API_VERSION}. Reload the page.`);
  const memory = wasm.memory;
  const handle = new EngineHandle(deviceMemory ?? undefined);
  // Threads need the memory to be shareable with workers, which a page that is not cross-origin isolated cannot do.
  const threaded = workers > 0 && globalThis.crossOriginIsolated !== false && typeof Worker === 'function';
  const pool = [];
  if (threaded) {
    await Promise.all(Array.from({length: workers}, () => new Promise((resolve, reject) => {
      const worker = new Worker(new URL('./engine-worker.mjs', import.meta.url), {type: 'module', name: 'styk-engine'});
      worker.onmessage = () => resolve();
      worker.onerror = event => reject(new Error(`An engine worker failed to start: ${event.message}`));
      worker.postMessage({module, memory, address: handle.address(), stack: WORKER_STACK_BYTES});
      pool.push(worker);
    })));
  }
  const progress = new Int32Array(memory.buffer, handle.progress_address(), 1);
  let pumping = false;
  /** Without workers, background work runs here in slices that leave the frame its time. */
  const pump = () => {
    if (threaded || pumping) return;
    pumping = true;
    const slice = () => {
      const started = performance.now();
      let more = true;
      while (more && performance.now() - started < 6) more = handle.step();
      if (more) setTimeout(slice, 0); else pumping = false;
    };
    setTimeout(slice, 0);
  };
  /** Resolves when background work has finished something since the call. */
  const whenProgress = () => {
    pump();
    const seen = Atomics.load(progress, 0);
    if (threaded && typeof Atomics.waitAsync === 'function') {
      const waiting = Atomics.waitAsync(progress, 0, seen, 250);
      return waiting.async ? waiting.value.then(() => {}) : Promise.resolve();
    }
    return new Promise(resolve => {
      const check = () => Atomics.load(progress, 0) !== seen ? resolve() : setTimeout(check, 4);
      setTimeout(check, 4);
    });
  };
  return {
    handle, workers: pool.length, threaded, pump, whenProgress,
    createStage: canvas => Stage.create(canvas, handle),
    /** Opens a file in the background and resolves with its description. */
    async open(bytes, extension, name) {
      handle.open(bytes, extension, name);
      for (;;) {
        const opened = handle.take_opened();
        if (opened !== undefined) return JSON.parse(opened);
        await whenProgress();
      }
    },
    destroy() {
      handle.stop();
      for (const worker of pool) worker.terminate();
      loading = null;
    },
  };
}

async function compile(source) {
  // Node (unit tests) reads the file; browsers compile while downloading.
  if (source.protocol === 'file:') {
    const {readFile} = await import('node:fs/promises');
    return WebAssembly.compile(await readFile(source));
  }
  const response = fetch(source, {credentials: 'same-origin'});
  try { return await WebAssembly.compileStreaming(response); }
  catch { return WebAssembly.compile(await (await fetch(source, {credentials: 'same-origin'})).arrayBuffer()); }
}
