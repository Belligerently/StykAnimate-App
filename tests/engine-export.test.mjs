// Export frames from the engine (src/engine/engine-export.mjs): which renderer an export
// gets, how the engine's worker is started, released and replaced, and the JavaScript
// renderer taking over before the first frame when the engine cannot draw.
import test from 'node:test';
import assert from 'node:assert/strict';
import {ExportFrameRenderer} from '../src/export-frame-renderer.mjs';
import {createExportFrameRenderer, EngineExportFrames, ENGINE_TILE_SIDE, prewarmExportRenderer} from '../src/engine/engine-export.mjs';

/** Fake workers, canvases and bitmaps (as tests/export-robustness.test.mjs); `respond(message, worker)` answers. */
function environment(t, respond) {
  const workers = [];
  const bitmap = (width = 32, height = 32) => ({width, height, closed: false, close() { this.closed = true; }});
  class Canvas extends EventTarget {
    constructor() { super(); this.width = this.height = 32;
      this.context = {canvas: this, isContextLost: () => false, setTransform() {}, clearRect() {}, drawImage() {}}; }
    getContext() { return this.context; }
  }
  class Worker extends EventTarget {
    constructor(url, init) { super(); this.url = String(url); this.name = init?.name; this.messages = []; workers.push(this); }
    get engine() { return this.url.endsWith('engine-export-worker.mjs'); }
    postMessage(message) {
      this.messages.push(message);
      const response = respond(message, this, bitmap);
      if (!response) return;
      queueMicrotask(() => { const event = new Event('message'); event.data = response; this.dispatchEvent(event); });
    }
    terminate() { this.terminated = true; }
  }
  for (const [key, value] of Object.entries({Worker, OffscreenCanvas: Canvas, document: {createElement: () => new Canvas()}})) {
    const descriptor = Object.getOwnPropertyDescriptor(globalThis, key);
    Object.defineProperty(globalThis, key, {value, configurable: true, writable: true});
    t.after(() => { if (descriptor) Object.defineProperty(globalThis, key, descriptor); else delete globalThis[key]; });
  }
  return {workers};
}

/** Answers both workers' requests; the engine's worker says when it released what it held. */
const answer = (overrides = {}) => (message, worker, bitmap) => {
  if (message.operation === 'release') return {released: true};
  if (message.operation === 'warm') return overrides.warm?.(message, worker) ?? {warmed: true};
  const custom = overrides[message.operation]?.(message, worker);
  if (custom !== undefined) return custom && {id: message.id, ...custom};
  return {id: message.id, ok: true, ...(message.operation === 'render' ? {bitmap: bitmap()} : {}), ...(message.operation === 'start' && worker.engine ? {engine: {backend: 'test', tiles: 1}} : {})};
};

/** The app's stage renderer and playback adapter, as the export sees them. */
function app({refusal = null, current = true, threaded = true, busy = 0} = {}) {
  const calls = [];
  const stage = {
    engineSide: {engine: {threaded, handle: {address: () => 4096}, join: {module: 'module', memory: 'memory', stack: 65536, progress: 64},
      whenProgress: async () => { calls.push(['progress']); }}},
    prepareExport: (report, revision) => {
      calls.push(['definitions', revision]);
      // A background commit is running: the interface thread is told to come back (D7).
      if (busy-- > 0) throw Object.assign(new Error('The engine is committing the document.'), {code: 'engine_busy'});
      return refusal;
    },
  };
  const playback = {exportDocument: async (report, revision) => { calls.push(['frames', revision]); return current; }};
  return {stage, playback, calls};
}

const report = () => ({figure_types: [], keyframes: [{duration_raw: 1, objects: []}]});

test('exports get the engine renderer only where the engine runs with workers, never for SVG', t => {
  environment(t, answer());
  const {stage, playback} = app();
  const options = {width: 32, height: 32};
  const engine = createExportFrameRenderer(options, {stage, playback, revision: 3, range: [0, 9]});
  assert.ok(engine instanceof EngineExportFrames);
  for (const [why, context] of [
    ['SVG is vector output', {stage, playback, revision: 3, vector: true}],
    ['no engine', {stage: {}, playback, revision: 3}],
    ['the JavaScript renderer is the whole stage', {stage: undefined, playback}],
    ['no worker threads (not cross-origin isolated)', {...app({threaded: false}), revision: 3}],
  ]) {
    const renderer = createExportFrameRenderer(options, context);
    assert.ok(!(renderer instanceof EngineExportFrames), why);
    assert.ok(renderer instanceof ExportFrameRenderer, why);
    renderer.dispose();
  }
  engine.dispose();
});

test('the engine renderer can be switched off for comparisons', t => {
  environment(t, answer());
  const stored = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
  Object.defineProperty(globalThis, 'localStorage', {value: {getItem: key => key === 'stykanimate.engine.export' ? 'off' : null}, configurable: true});
  t.after(() => { if (stored) Object.defineProperty(globalThis, 'localStorage', stored); else delete globalThis.localStorage; });
  const renderer = createExportFrameRenderer({width: 32, height: 32}, {...app(), revision: 1});
  assert.ok(!(renderer instanceof EngineExportFrames));
  renderer.dispose();
});

test('the engine draws a document it holds: its worker starts with the export settings', async t => {
  const {workers} = environment(t, (message, worker, bitmap) => answer({render: () => ({ok: true, bitmap: bitmap(640, 360)})})(message, worker, bitmap));
  const {stage, playback, calls} = app();
  const renderer = createExportFrameRenderer({width: 640, height: 360, smoothing: 'sprites'}, {stage, playback, revision: 7, range: [5, 20]});
  assert.equal(workers.length, 0, 'no worker before the renderer knows which one draws');
  await renderer.load(report());
  assert.deepEqual(calls, [['frames', 7], ['definitions', 7]], 'frames through the playback adapter, then what only the stage knows, at the export revision');
  assert.equal(workers.length, 1);
  assert.ok(workers[0].engine);
  const start = workers[0].messages[0];
  assert.equal(start.operation, 'start');
  assert.deepEqual({revision: start.revision, width: start.width, height: start.height, smoothing: start.smoothing, tileSize: start.tileSize, last: start.last, address: start.address},
    {revision: 7, width: 640, height: 360, smoothing: 'sprites', tileSize: ENGINE_TILE_SIDE, last: 20, address: 4096});
  assert.equal(start.module, 'module');
  assert.equal(renderer.stats.renderer, 'engine');
  const canvas = await renderer.renderTick(5);
  assert.equal(canvas.width, 640);
  assert.equal(workers[0].messages.at(-1).operation, 'render');
  // Disposal asks the worker to free the engine memory and GPU device it holds; it ends once it has.
  renderer.dispose();
  assert.equal(workers[0].messages.at(-1).operation, 'release');
  await new Promise(resolve => setTimeout(resolve, 0));
  assert.equal(workers[0].terminated, true);
});

test('the export waits for a background commit instead of falling back', async t => {
  const {workers} = environment(t, answer());
  const {stage, playback, calls} = app({busy: 2});
  const renderer = createExportFrameRenderer({width: 32, height: 32}, {stage, playback, revision: 2});
  await renderer.load(report());
  assert.deepEqual(calls.map(call => call[0]), ['frames', 'definitions', 'progress', 'definitions', 'progress', 'definitions']);
  assert.equal(renderer.stats.renderer, 'engine');
  assert.ok(workers[0].engine);
  renderer.dispose();
});

test('a document the engine does not draw keeps the JavaScript renderer', async t => {
  const {workers} = environment(t, answer());
  const renderer = createExportFrameRenderer({width: 32, height: 32}, {...app({refusal: 'property channels'}), revision: 1});
  await renderer.load(report());
  assert.equal(renderer.stats.renderer, 'javascript');
  assert.equal(renderer.stats.engineRefusal, 'property channels');
  assert.equal(workers.length, 1);
  assert.ok(!workers[0].engine, 'the JavaScript export worker');
  assert.equal((await renderer.renderTick(0)).width, 32);
  renderer.dispose();
  assert.equal(workers[0].terminated, true);
});

test('without a GPU device in a worker the JavaScript renderer takes over before the first frame', async t => {
  const {workers} = environment(t, answer({
    start: (message, worker) => worker.engine ? {ok: false, code: 'engine_unavailable', error: 'no GPU adapter in this worker'} : undefined,
  }));
  const renderer = createExportFrameRenderer({width: 32, height: 32}, {...app(), revision: 1});
  await renderer.load(report());
  assert.equal(renderer.stats.renderer, 'javascript');
  assert.match(renderer.stats.engineRefusal, /no GPU adapter/);
  assert.equal(workers.length, 2);
  assert.equal(workers[0].messages.at(-1).operation, 'release', 'the engine worker is released, not killed');
  await new Promise(resolve => setTimeout(resolve, 0));
  assert.equal(workers[0].terminated, true);
  assert.equal((await renderer.renderTick(0)).width, 32);
  renderer.dispose();
});

test('a GPU memory failure retries the frame in smaller tiles', async t => {
  let renders = 0;
  const {workers} = environment(t, answer({
    render: () => ++renders === 1 ? {ok: false, error: 'out of memory', code: 'export_gpu_memory', recoverable: true} : undefined,
  }));
  const renderer = createExportFrameRenderer({width: 32, height: 32}, {...app(), revision: 1});
  await renderer.load(report());
  assert.equal((await renderer.renderTick(0)).width, 32);
  const retile = workers[0].messages.find(message => message.operation === 'retile');
  assert.equal(retile.tileSize, ENGINE_TILE_SIDE / 2);
  renderer.dispose();
});

test('a worker that does not release in time is terminated, and cancellation rejects at once', async t => {
  const {workers} = environment(t, (message, worker, bitmap) => message.operation === 'release' || message.operation === 'render' ? null
    : answer()(message, worker, bitmap));
  t.mock.timers.enable({apis: ['setTimeout']});
  const controller = new AbortController();
  const renderer = createExportFrameRenderer({width: 32, height: 32, signal: controller.signal}, {...app(), revision: 1});
  const loading = renderer.load(report());
  await t.mock.timers.runAll?.();
  await loading;
  const pending = renderer.renderTick(0);
  controller.abort();
  await assert.rejects(pending, {name: 'AbortError'});
  assert.equal(workers[0].messages.at(-1).operation, 'release');
  assert.equal(workers[0].terminated, undefined, 'given time to free its memory');
  t.mock.timers.tick(5000);
  assert.equal(workers[0].terminated, true);
  renderer.dispose();
});

test('the export dialog warms a worker that the export then takes', async t => {
  const {workers} = environment(t, answer());
  const {stage, playback} = app();
  prewarmExportRenderer({stage});
  prewarmExportRenderer({stage});
  assert.equal(workers.length, 1, 'one warm worker at a time');
  assert.equal(workers[0].messages[0].operation, 'warm');
  assert.equal(workers[0].messages[0].module, 'module', 'it joins the engine');
  const renderer = createExportFrameRenderer({width: 32, height: 32}, {stage, playback, revision: 4});
  await renderer.load(report());
  assert.equal(workers.length, 1, 'the export takes the warm worker instead of starting one');
  assert.deepEqual(workers[0].messages.map(message => message.operation), ['warm', 'start']);
  renderer.dispose();
  await new Promise(resolve => setTimeout(resolve, 0));
  assert.equal(workers[0].terminated, true);
});

test('a warm worker no export takes is released after a minute, and one without a device at once', async t => {
  const {workers} = environment(t, answer({warm: (message, worker) => workers.indexOf(worker) === 1 ? {warmed: false, error: 'no GPU adapter'} : undefined}));
  t.mock.timers.enable({apis: ['setTimeout']});
  const {stage} = app();
  // Nothing is warmed where exports would not use the engine.
  prewarmExportRenderer({stage: {}});
  prewarmExportRenderer({stage: app({threaded: false}).stage});
  assert.equal(workers.length, 0);
  prewarmExportRenderer({stage});
  t.mock.timers.tick(59999);
  assert.equal(workers[0].messages.at(-1).operation, 'warm');
  t.mock.timers.tick(1);
  assert.equal(workers[0].messages.at(-1).operation, 'release', 'given up after a minute');
  await new Promise(resolve => queueMicrotask(resolve));
  // A worker that finds no GPU device says so and is released at once; the next dialog warms again.
  prewarmExportRenderer({stage});
  await new Promise(resolve => queueMicrotask(resolve));
  assert.equal(workers[1].messages.at(-1).operation, 'release');
  prewarmExportRenderer({stage});
  assert.equal(workers.length, 3);
  t.mock.timers.tick(60000);
});
