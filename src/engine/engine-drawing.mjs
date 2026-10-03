/**
 * The drawing raster engine in the app (engine/crates/styk-drawing, docs/engine/drawing.md,
 * docs/engine/decisions/0017-drawing-raster-engine.md): the one module that loads its
 * WebAssembly module (`./pkg/styk_drawing.js`, built by scripts/engine-build.mjs).
 *
 * The module is its own, without threads or shared memory: every drawing worker and the
 * page load their own instance, so a worker can still be terminated to cancel its job.
 * The facades here keep the interfaces the drawing workers and the controller already
 * use: the stroke worker's `StrokeEngine`, the brush model's `BrushStroke`, the history
 * codec. `src/engine/engine-drawing-worker.mjs` answers the pixel worker's operations.
 *
 * Strangler gate (docs/RUST-ENGINE-PORT.md, S6): `?drawingEngine=off` (or `?engine=off`,
 * or the stored `stykanimate.drawingEngine` / `stykanimate.engine` setting) keeps the
 * JavaScript raster engine, which stays the oracle; so does a module that fails to load.
 */
import {messageError} from '../localized-error.mjs';
import {BrushStroke as JavaScriptBrushStroke, pressureMap} from '../drawing-brush.mjs';

/** The call set this facade was written for (`drawingApiVersion()` in the module). */
const API_VERSION = 1;
const DAB = 11;

/** Whether the drawing raster engine runs in Rust: `location` is the page's or, in a
 * worker, the worker script's URL (the page appends `?engine=off` when it is off). */
export function drawingEngineEnabled(location = globalThis.location) {
  try {
    const query = new URLSearchParams(location?.search ?? '');
    for (const name of ['drawingEngine', 'engine']) { const value = query.get(name); if (value) return value !== 'off'; }
    const stored = globalThis.localStorage?.getItem('stykanimate.drawingEngine') ?? globalThis.localStorage?.getItem('stykanimate.engine');
    return stored !== 'off';
  } catch { return true; }
}

/** A drawing worker's script URL, carrying the page's gate decision. */
export function drawingWorkerURL(url) {
  if (drawingEngineEnabled()) return url;
  const next = new URL(url);
  next.searchParams.set('engine', 'off');
  return next;
}

let loading = null, glue = null, failure = null, exports = null;

async function load() {
  const module = new URL('./pkg/styk_drawing.js', import.meta.url);
  const bindings = await import(module.href);
  if (globalThis.process?.versions?.node && !globalThis.WorkerGlobalScope) {
    // Node (tests, tools): read the module from disk; fetch() has no file URLs there.
    const {readFile} = await import('node:fs/promises');
    exports = await bindings.default({module_or_path: await readFile(new URL('./pkg/styk_drawing_bg.wasm', import.meta.url))});
  } else exports = await bindings.default();
  if (bindings.drawingApiVersion() !== API_VERSION) throw new Error(`The drawing engine module has API ${bindings.drawingApiVersion()}, this page expects ${API_VERSION}. Reload the page.`);
  return bindings;
}

/** Loads the module once per thread. Resolves to its bindings, or null when the gate is
 * off or the module cannot load (the JavaScript raster engine then answers). */
export function loadDrawingEngine({enabled = drawingEngineEnabled()} = {}) {
  if (!enabled) return Promise.resolve(null);
  return loading ??= load().then(bindings => (glue = bindings), error => {
    failure = error;
    console.warn?.(`The drawing engine is unavailable; drawing uses the JavaScript engine (${error?.message ?? error}).`);
    return null;
  });
}

/** The loaded bindings, or null (not loaded, off, or failed). */
export function drawingEngine() { return glue; }
/** Why the module did not load, if it failed. */
export function drawingEngineFailure() { return failure; }
/** Bytes of linear memory the module holds in this thread (it grows, it never shrinks). */
export function drawingEngineMemory() { return exports?.memory?.buffer?.byteLength ?? 0; }

/** Runs `call` on the module; a module error becomes the editor's localized error. A trap
 * (a panic aborts) leaves the instance unusable: it is replaced before the error is thrown. */
export function engineCall(call) {
  try { return call(glue); }
  catch (error) {
    const key = typeof error?.message === 'string' && error.message.startsWith('errors.') ? error.message : null;
    if (!key) { try { glue.__wbg_reset_state?.(); } catch { /* the next call reports its own failure */ } throw error; }
    let localized;
    try { localized = messageError(new Error(''), key, {}); } catch { localized = new Error(key); }
    throw localized;
  }
}

// ---------------------------------------------------------------- conversions

/** Tiles in: `[{tx, ty, pixels}]` (ArrayBuffer or typed array; null for none; png for trusted PNGs). */
export function tilesIn(list, {png = false} = {}) {
  const tiles = new glue.Tiles();
  for (const t of list ?? []) {
    if (png && t.png) tiles.pushPng(t.tx, t.ty, bytesOf(t.png));
    else if (t.pixels == null) tiles.pushEmpty(t.tx, t.ty);
    else tiles.push(t.tx, t.ty, bytesOf(t.pixels));
  }
  return tiles;
}
/** A worker's tile store in (its tiles in `Map` order). */
export function storeIn(store) {
  const tiles = new glue.Tiles();
  for (const t of store) tiles.push(t.tx, t.ty, t.pixels);
  return tiles;
}
export function bytesOf(value) {
  if (value instanceof Uint8Array || value instanceof Uint8ClampedArray) return value;
  if (value instanceof ArrayBuffer) return new Uint8Array(value);
  if (ArrayBuffer.isView(value)) return new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
  return new Uint8Array(0);
}
/** A selection in: `{x, y, width, height, mask, max?}` or `{…, points: [{x, y}]}`. */
export function maskIn(mask) {
  if (!mask) return undefined;
  if (mask.points) return glue.MaskIn.points(mask.x, mask.y, mask.width, mask.height, Float64Array.from(mask.points.flatMap(p => [p.x, p.y])));
  return glue.MaskIn.dense(mask.x, mask.y, mask.width, mask.height, mask.mask, mask.max ?? 1);
}
const clamped = bytes => bytes == null ? null : new Uint8ClampedArray(bytes.buffer, bytes.byteOffset, bytes.byteLength);
const rectOf = values => values ? {x: values[0], y: values[1], width: values[2], height: values[3]} : null;
/** Undo records out: `[{tx, ty, before, after}]` with `Uint8ClampedArray` sides. */
export function changesOut(output) {
  const out = [];
  for (let i = 0; i < output.length; i++) out.push({tx: output.tx(i), ty: output.ty(i), before: clamped(output.a(i)), after: clamped(output.b(i))});
  output.free();
  return out;
}
/** Tiles out with their alpha bounds: `[{tx, ty, pixels: Uint8ClampedArray, alphaBounds}]`. */
export function tilesOut(output) {
  const out = [];
  for (let i = 0; i < output.length; i++) out.push({tx: output.tx(i), ty: output.ty(i), pixels: clamped(output.a(i)), alphaBounds: rectOf(output.alpha(i))});
  return out;
}
export {rectOf};
const packDabs = dabs => {
  const out = new Float64Array(dabs.length * DAB);
  for (let i = 0; i < dabs.length; i++) {
    const d = dabs[i], o = i * DAB;
    out[o] = d.x; out[o + 1] = d.y; out[o + 2] = d.radius; out[o + 3] = d.opacity; out[o + 4] = d.flow; out[o + 5] = d.cos; out[o + 6] = d.sin;
    out[o + 7] = d.left; out[o + 8] = d.top; out[o + 9] = d.right; out[o + 10] = d.bottom;
  }
  return out;
};
const unpackDabs = (values, paint) => {
  for (let o = 0; o < values.length; o += DAB)
    paint({x: values[o], y: values[o + 1], radius: values[o + 2], opacity: values[o + 3], flow: values[o + 4], cos: values[o + 5], sin: values[o + 6],
      left: values[o + 7], top: values[o + 8], right: values[o + 9], bottom: values[o + 10]});
};
/** Brush settings as the module reads them: JSON without the selection. */
const settingsJSON = settings => JSON.stringify(settings ?? {}, (key, value) => key === 'mask' ? undefined : value);

// ---------------------------------------------------------------- the stroke worker

/** `StrokeEngine` (drawing-stroke-engine.mjs) in Rust, the same calls and answers. */
export class StrokeEngine {
  constructor() { this.engine = engineCall(g => new g.Strokes()); }
  begin(id, settings) { engineCall(() => this.engine.begin(id, settingsJSON(settings), maskIn(settings?.mask))); }
  dabs(id, seq, dabs, tiles, tail) {
    const seeds = tilesIn(tiles);
    let output;
    try { output = engineCall(() => this.engine.dabs(id, seq, packDabs(dabs ?? []), seeds, tail ? packDabs(tail) : undefined)); }
    finally { seeds.free(); }
    if (!output) return null;
    const changed = [];
    for (let i = 0; i < output.length; i++) changed.push({tx: output.tx(i), ty: output.ty(i), pixels: output.a(i).buffer});
    const dirty = rectOf(output.rect);
    output.free();
    return {tiles: changed, dirty};
  }
  end(id) {
    const output = engineCall(() => this.engine.end(id));
    return output ? changesOut(output) : null;
  }
  cancel(id) { engineCall(() => this.engine.cancel(id)); }
}

// ---------------------------------------------------------------- the brush model

/** `BrushStroke` (drawing-brush.mjs) in Rust: input samples to final dab shapes, the
 * provisional tail and the pen curve, as the drawing controller uses them. */
export class RustBrushStroke {
  constructor(settings, paint, {seed} = {}) {
    this.paint = paint;
    this.path = engineCall(g => new g.BrushPath(JSON.stringify(settings), seed ?? undefined));
    this.closed = false;
    this.tailCache = null;
    this.final = null;
    // The wet-ink preview sizes its marks with the pen curve; pixels never depend on it.
    this.pressure = pressureMap(settings.pressureCurve);
  }
  get tailVersion() { return this.final?.tailVersion ?? this.path.tailVersion; }
  get hasTail() { return this.final?.hasTail ?? this.path.hasTail; }
  sample(input) {
    const ok = Number.isFinite;
    const values = engineCall(() => this.path.sample(input.x, input.y, input.pressure ?? undefined, input.time ?? undefined, ok(input.ax) ? input.ax : undefined, ok(input.ay) ? input.ay : undefined));
    unpackDabs(values, this.paint);
  }
  tailShapes() {
    const version = this.tailVersion;
    if (this.final) return this.tailCache?.version === version ? this.tailCache.shapes : (this.tailCache = {version, shapes: []}).shapes;
    if (this.tailCache?.version === version) return this.tailCache.shapes;
    const shapes = [];
    unpackDabs(engineCall(() => this.path.tail()), shape => shapes.push(shape));
    this.tailCache = {version, shapes};
    return shapes;
  }
  /** Ends the stroke and releases its state in the module; the tail is empty afterwards. */
  finish() {
    if (this.closed) return;
    this.closed = true;
    const values = engineCall(() => this.path.finish());
    this.final = {tailVersion: this.path.tailVersion, hasTail: this.path.hasTail};
    this.path.free();
    unpackDabs(values, this.paint);
  }
}

/** A brush stroke on the engine when it is loaded in this thread, otherwise the
 * JavaScript brush model (the same shapes: tests/parity in styk-drawing). */
export function createBrushStroke(settings, paint, options) {
  return glue ? new RustBrushStroke(settings, paint, options) : new JavaScriptBrushStroke(settings, paint, options);
}

// ---------------------------------------------------------------- undo chunks

/** `encodeHistoryChunk(tiles)` in Rust. */
export function encodeHistoryChunk(tiles) {
  const list = new glue.Tiles();
  try {
    for (const t of tiles) {
      for (const side of [t.before, t.after]) side == null ? list.pushEmpty(t.tx, t.ty) : list.push(t.tx, t.ty, bytesOf(side));
    }
    return engineCall(g => g.encodeHistoryChunk(list));
  } finally { list.free(); }
}
/** `decodeHistoryChunk(bytes)` in Rust. */
export function decodeHistoryChunk(bytes) {
  return changesOut(engineCall(g => g.decodeHistoryChunk(bytesOf(bytes))));
}
