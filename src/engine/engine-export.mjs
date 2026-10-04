/**
 * Export frames drawn by the engine (docs/engine/rendering.md, Exports; stage S4 of
 * docs/RUST-ENGINE-PORT.md).
 *
 * For a document the engine draws (engine-eligibility.mjs), an export's pixels come
 * from the engine's GPU renderer instead of the JavaScript Canvas 2D export renderer
 * (export-frame-renderer.mjs): the frame the stage would show at that tick, through
 * the scene camera, at the export's size and sampling, without onion skins. Encoders
 * (PNG, GIF, APNG, ZIP, WebCodecs, FFmpeg) do not change: they receive the same
 * canvases and PNG frames from the same interface.
 *
 * `EngineExportFrames` is an `ExportFrameRenderer` whose worker is the engine's
 * (engine-export-worker.mjs): a dedicated worker that joins the engine's shared
 * memory and holds a GPU device of its own, so nothing is drawn or read back on the
 * interface thread (D7). Request handling, progress, revision checks, timeouts, the
 * retry ladder and the output canvas are the base class's. A document the engine does
 * not draw, or a device the worker cannot get, uses the JavaScript worker instead,
 * decided before the first frame. SVG stays vector output: `vector` exports always use
 * the JavaScript renderer. `prewarmExportRenderer` starts the worker and its device while
 * the export dialog is open.
 */
import {ExportFrameRenderer} from '../export-frame-renderer.mjs';
import {drawingBarrier} from '../drawing-resources.mjs';
import {EXPORT_MAX_PIXELS} from '../export-sampling.mjs';
import {engineDocument} from './engine-document.mjs';

/** Rendered pixels a side of an export tile at most (styk-export `TILE_SIDE`). */
export const ENGINE_TILE_SIDE = 2048;
/** How long a worker may take to release the engine memory and GPU device it holds. */
const RELEASE_MS = 5000;
/** How long a worker warmed for an export waits for it. */
const WARM_MS = 60000;
const WORKER_NAME = 'StykAnimate export rendering (engine)';

/**
 * Asks an engine export worker to free what it holds in the engine's memory and its GPU
 * device, then end; it is terminated if it does not say so in time (what it held then stays
 * allocated).
 */
function release(worker) {
  const timer = setTimeout(() => worker.terminate(), RELEASE_MS);
  worker.addEventListener('message', ({data}) => { if (data?.released) { clearTimeout(timer); worker.terminate(); } });
  try { worker.postMessage({operation: 'release'}); } catch { clearTimeout(timer); worker.terminate(); }
}

/** An engine export worker started ahead of an export, and when it is given up. */
let warm = null;

/**
 * Starts an engine export worker ahead of an export (the export dialog opens): joining the
 * engine and getting a GPU device take most of a second, which the next export then skips.
 * Given up after a minute without an export. Does nothing where exports would not use the engine.
 */
export function prewarmExportRenderer({stage} = {}) {
  const engine = stage?.engineSide?.engine;
  if (warm || !engine?.threaded || !engineExportEnabled() || typeof Worker !== 'function') return;
  const worker = new Worker(new URL('./engine-export-worker.mjs', import.meta.url), {type: 'module', name: WORKER_NAME});
  const entry = warm = {worker, timer: setTimeout(() => { if (warm === entry) { warm = null; release(worker); } }, WARM_MS)};
  worker.addEventListener('message', ({data}) => { if (data?.warmed === false && warm === entry) { warm = null; clearTimeout(entry.timer); release(worker); } });
  worker.postMessage({operation: 'warm', ...engine.join});
}

/** The warmed worker, for an export that uses the engine. */
function takeWarm() {
  const entry = warm;
  warm = null;
  if (entry) clearTimeout(entry.timer);
  return entry?.worker ?? null;
}

/** `?engineExport=off` (or the stored setting) keeps the JavaScript export renderer, for comparisons. */
export function engineExportEnabled() {
  try {
    const query = new URLSearchParams(globalThis.location?.search ?? '').get('engineExport');
    if (query) return query !== 'off';
    return globalThis.localStorage?.getItem('stykanimate.engine.export') !== 'off';
  } catch { return true; }
}

/**
 * The export frame renderer for an export: the engine's when the app's stage has the
 * engine running with worker threads and the export is not vector output, else the
 * JavaScript one. `stage` is the app's stage renderer (engine-stage.mjs), `playback` its
 * playback adapter (engine-playback.mjs), `revision` the editor's revision, `range` the
 * first and last tick the export draws.
 */
export function createExportFrameRenderer(options, {stage, playback, revision, range, vector = false} = {}) {
  const side = stage?.engineSide;
  if (vector || !engineExportEnabled() || !side?.engine?.threaded || typeof stage.prepareExport !== 'function' || typeof playback?.exportDocument !== 'function')
    return new ExportFrameRenderer(options);
  return new EngineExportFrames(options, {stage, playback, revision, range});
}

export class EngineExportFrames extends ExportFrameRenderer {
  constructor(options, engine) {
    super(options);
    /** `engine` while the engine may draw, `javascript` once the JavaScript worker does. */
    this.mode = null;
    this.engine = engine;
    this.stats.renderer = null;
  }

  /** The worker of the current mode (the warmed one when there is one); none before `load` decides. */
  _spawn() {
    if (this.mode === 'javascript') return super._spawn();
    if (this.mode !== 'engine') return;
    this._attach(takeWarm() ?? new Worker(new URL('./engine-export-worker.mjs', import.meta.url), {type: 'module', name: WORKER_NAME}));
  }

  /** The engine's worker frees what it holds in the shared memory and its GPU device, then ends. */
  _release(worker) {
    if (!worker || worker.styk !== 'engine') return super._release(worker);
    release(worker);
  }

  _attach(worker) {
    worker.styk = this.mode;
    super._attach(worker);
  }

  /** What the engine's worker needs to start drawing this export. */
  _start(tileSize) {
    const engine = this.engine.stage.engineSide.engine;
    return {
      ...engine.join, address: engine.handle.address(), revision: this.engine.revision,
      width: this.width, height: this.height, smoothing: this.smoothing, maxPixels: EXPORT_MAX_PIXELS, tileSize,
      last: this.engine.range?.[1] ?? null,
    };
  }

  /** Why the engine cannot draw `report` for this export, or null once it holds it (engine-document.mjs). */
  _engineDocument(report) {
    const {stage, playback, revision} = this.engine;
    return engineDocument({stage, playback, report, revision, check: () => this._check()});
  }

  /** The JavaScript renderer takes the export; nothing was drawn yet. */
  async _javascript(report, reason) {
    this.mode = 'javascript';
    this.stats.renderer = 'javascript';
    this.stats.engineRefusal = reason;
    this._spawn();
    this.busy = false;
    return super.load(report);
  }

  async load(report) {
    this._check();
    await drawingBarrier(report, {signal: this.signal});
    this._check();
    if (this.busy) return super.load(report);
    this.busy = true;
    this.loaded = false;
    try {
      this.revision = this.getRevision?.();
      const began = performance.now();
      const reason = await this._engineDocument(report);
      this._check();
      if (reason) return await this._javascript(report, reason);
      this.mode = 'engine';
      this.tileSize = ENGINE_TILE_SIDE;
      const documented = performance.now();
      this._spawn();
      let started;
      try { started = await this._request('start', this._start(this.tileSize)); }
      catch (error) {
        if (error?.code !== 'engine_unavailable') throw error;
        // No GPU device in a worker here (or the engine moved on): the JavaScript worker draws.
        this.failure = null;
        const worker = this.worker;
        this.worker = null;
        this._release(worker);
        return await this._javascript(report, error.message);
      }
      this.stats.renderer = 'engine';
      this.stats.engine = started.engine;
      // Bringing the engine's document to the export's revision, then starting the worker and its GPU device.
      this.stats.engineLoad = {documentMs: documented - began, startMs: performance.now() - documented};
      this.loaded = true;
      return this;
    } catch (error) { this._fail(error); throw error; } finally { this.busy = false; }
  }

  /** Replaces a failed engine worker (a new GPU device), drawing in tiles of at most `tileSize`. */
  async _rebuild(tileSize) {
    if (this.mode !== 'engine') return super._rebuild(tileSize);
    this.failure = null;
    this.loaded = false;
    this._spawn();
    const started = await this._request('start', this._start(tileSize));
    this.stats.engine = started.engine;
    this.loaded = true;
  }

  // SVG is vector output: createExportFrameRenderer gives vector exports the JavaScript renderer.
  renderSVGTick(tick) { return this.mode === 'engine' ? Promise.reject(new Error('The engine draws pixels, not SVG.')) : super.renderSVGTick(tick); }
  renderSVGFrame(frame) { return this.mode === 'engine' ? Promise.reject(new Error('The engine draws pixels, not SVG.')) : super.renderSVGFrame(frame); }
}
