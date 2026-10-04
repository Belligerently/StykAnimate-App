// Thumbnails and library previews from the engine (src/engine/engine-thumbnails.mjs,
// src/engine/engine-previews.mjs): which renderer draws a thumbnail (the stage, the preview
// worker on the CPU where the stage cannot, the JavaScript renderer where the engine does not
// draw), and the preview worker's queue, cancellation, failure and release.
import test from 'node:test';
import assert from 'node:assert/strict';
import {EngineThumbnails} from '../src/engine/engine-thumbnails.mjs';
import {EnginePreviewWorker} from '../src/engine/engine-previews.mjs';

/** An editor as thumbnails see it: a document, its revision, its timeline and change events. */
function editor({frames = 3} = {}) {
  const target = new EventTarget();
  target.report = {keyframes: Array.from({length: frames}, () => ({objects: []})), _styk: {}};
  target.revision = 4;
  target.timeline = Array.from({length: frames}, (_, i) => ({start: i * 10, count: 10}));
  return target;
}

/** The app's stage and playback adapter; `draw` answers the stage's thumbnail requests. */
function app({draw = () => 'drawn', thumbnails, threaded = true, refusal = null} = {}) {
  const calls = [];
  const stage = {
    engineSide: {engine: {threaded, progressSeen: () => 0, whenProgress: async () => {}, handle: {addresses: 0, address() { return 1000 + this.addresses++; }}}, thumbnails},
    prepareExport: (report, revision) => { calls.push(['definitions', revision]); return refusal; },
    renderThumbnail: async (request, canvas) => { calls.push(['thumbnail', request]); return draw(request, canvas); },
  };
  const playback = {exportDocument: async (report, revision) => { calls.push(['frames', revision]); return true; }};
  return {stage, playback, calls};
}

/** A stand-in for the JavaScript renderer, recording what it drew. */
function javascript() {
  const drawn = [];
  const make = () => ({renderThumbnail: async (index, canvas, options) => { drawn.push({index, options}); return true; }, setPaused() {}, destroy() { this.destroyed = true; }, stats: {backend: 'Worker thumbnails'}});
  return {drawn, make};
}

/** A 2D canvas as thumbnails draw on it: what was drawn on it is recorded. */
const canvas = () => {
  const drawn = [];
  return {width: 320, height: 192, drawn, getContext: () => ({setTransform() {}, clearRect() {}, drawImage: image => drawn.push(image)})};
};

/** The engine's preview worker as thumbnails use it: `answer(message)` answers a thumbnail request. */
function previewWorker(answer = message => ({bitmap: {closed: false, close() { this.closed = true; }, tick: message.tick}})) {
  const requests = [];
  return {requests, worker: {usable: true, async request(message, {signal} = {}) {
    if (signal?.aborted) throw new DOMException('aborted', 'AbortError');
    const sent = typeof message === 'function' ? message() : message;
    requests.push(sent);
    return answer(sent);
  }}};
}

test('a document the engine draws gets its thumbnails from the stage, without the JavaScript renderer', async () => {
  const {stage, playback, calls} = app(), js = javascript(), target = editor();
  const thumbnails = new EngineThumbnails(target, {}, {stage: () => stage, playback: () => playback, javascript: js.make});
  assert.equal(await thumbnails.renderThumbnail(1, canvas(), {tick: 10}), true);
  assert.deepEqual(calls.find(call => call[0] === 'thumbnail')[1], {revision: 4, tick: 10, width: 320, height: 192});
  assert.deepEqual(calls.slice(0, 2), [['frames', 4], ['definitions', 4]], 'the engine is brought to the revision first');
  // Without a tick the frame's start is drawn.
  assert.equal(await thumbnails.renderThumbnail(2, canvas()), true);
  assert.equal(calls.filter(call => call[0] === 'thumbnail').at(-1)[1].tick, 20);
  assert.equal(thumbnails.javascript, null, 'no JavaScript worker, so no copy of the document');
  assert.equal(js.drawn.length, 0);
  assert.equal(thumbnails.stats.engine.renders, 2);
});

test('in-between previews, refused documents and devices without a second canvas are the JavaScript renderer\'s', async () => {
  const js = javascript(), target = editor();
  // An in-between preview samples a pose that is not in the document.
  const {stage, playback, calls} = app();
  const thumbnails = new EngineThumbnails(target, {}, {stage: () => stage, playback: () => playback, javascript: js.make});
  assert.equal(await thumbnails.renderThumbnail(0, canvas(), {inbetween: {ratio: .5}}), true);
  assert.equal(calls.length, 0, 'the engine is not asked');
  // The gate refuses the document.
  const refused = app({refusal: 'a frame draft'});
  const second = new EngineThumbnails(target, {}, {stage: () => refused.stage, playback: () => refused.playback, javascript: js.make});
  assert.equal(await second.renderThumbnail(0, canvas()), true);
  assert.equal(second.stats.engine.refusal, 'a frame draft');
  // WebGL2 and no preview worker either: the stage could not give thumbnails a canvas; later
  // requests go straight to JavaScript.
  const gl = app({draw: () => 'refused'});
  const third = new EngineThumbnails(target, {}, {stage: () => gl.stage, playback: () => gl.playback, javascript: js.make, previews: async () => null});
  assert.equal(await third.renderThumbnail(0, canvas()), true);
  gl.stage.engineSide.thumbnails = false;
  const before = gl.calls.length;
  assert.equal(await third.renderThumbnail(1, canvas()), true);
  assert.equal(gl.calls.length, before, 'not asked again');
  assert.match(third.stats.engine.refusal, /no preview worker/);
  // No engine with worker threads.
  const plain = app({threaded: false});
  const fourth = new EngineThumbnails(target, {}, {stage: () => plain.stage, playback: () => plain.playback, javascript: js.make});
  assert.equal(await fourth.renderThumbnail(0, canvas()), true);
  assert.equal(plain.calls.length, 0);
  assert.equal(js.drawn.length, 5);
});

test('thumbnails the stage cannot draw are drawn by the preview worker on the CPU, from the engine\'s document', async () => {
  const js = javascript(), target = editor();
  // A WebGL2 stage: it refuses thumbnails and says it never will.
  const {stage, playback, calls} = app({draw: () => { stage.engineSide.thumbnails = false; return 'refused'; }});
  const cpu = previewWorker();
  const thumbnails = new EngineThumbnails(target, {}, {stage: () => stage, playback: () => playback, javascript: js.make, previews: async () => cpu.worker});
  const card = canvas();
  assert.equal(await thumbnails.renderThumbnail(1, card, {tick: 10}), true);
  assert.deepEqual(calls.slice(0, 2), [['frames', 4], ['definitions', 4]], 'the engine is brought to the revision first');
  assert.equal(cpu.requests.length, 1);
  assert.deepEqual({...cpu.requests[0], address: undefined}, {operation: 'thumbnail', address: undefined, revision: 4, tick: 10, width: 320, height: 192});
  assert.equal(typeof cpu.requests[0].address, 'number', 'an engine address, made when the request is sent');
  assert.equal(card.drawn.length, 1);
  assert.equal(card.drawn[0].closed, true, 'the picture is copied onto the card and let go');
  // Later thumbnails go straight to the worker: the stage is not asked again.
  const asked = calls.filter(call => call[0] === 'thumbnail').length;
  assert.equal(await thumbnails.renderThumbnail(2, canvas()), true);
  assert.equal(calls.filter(call => call[0] === 'thumbnail').length, asked);
  assert.equal(cpu.requests.at(-1).tick, 20);
  assert.equal(cpu.requests[1].address !== cpu.requests[0].address, true, 'each request gets an address of its own');
  assert.equal(thumbnails.javascript, null, 'no JavaScript worker, so no copy of the document');
  assert.equal(thumbnails.stats.engine.cpu, 2);
  // Another revision in the engine: not drawn, asked again later.
  const stale = previewWorker(() => ({refused: 'stale'}));
  const later = new EngineThumbnails(target, {}, {stage: () => stage, playback: () => playback, javascript: js.make, previews: async () => stale.worker});
  assert.equal(await later.renderThumbnail(0, canvas()), false);
  assert.equal(later.stats.engine.stale, 1);
  // A frame the worker could not draw goes to the JavaScript renderer.
  const timeout = previewWorker(() => ({refused: 'timeout', message: 'the frame was not prepared in time'}));
  const slow = new EngineThumbnails(target, {}, {stage: () => stage, playback: () => playback, javascript: js.make, previews: async () => timeout.worker});
  assert.equal(await slow.renderThumbnail(0, canvas()), true);
  assert.equal(js.drawn.length, 1);
  // A cancelled thumbnail is not drawn and makes no address.
  const never = previewWorker();
  const cancelled = new EngineThumbnails(target, {}, {stage: () => stage, playback: () => playback, javascript: js.make, previews: async () => never.worker});
  const controller = new AbortController();
  controller.abort();
  assert.equal(await cancelled.renderThumbnail(0, canvas(), {signal: controller.signal}), false);
  assert.equal(never.requests.length, 0);
});

test('a thumbnail waits for the stage, and one superseded by an edit is not drawn', async () => {
  const js = javascript(), target = editor();
  let current = null;
  const {stage, playback} = app({draw: () => 'stale'});
  const thumbnails = new EngineThumbnails(target, {}, {stage: () => current, playback: () => playback, javascript: js.make});
  // The app is still creating its stage: the filmstrip asks again later.
  assert.equal(await thumbnails.renderThumbnail(0, canvas()), false);
  current = stage;
  // The engine holds another revision: not drawn, asked again later.
  assert.equal(await thumbnails.renderThumbnail(0, canvas()), false);
  assert.equal(thumbnails.stats.engine.stale, 1);
  // An edit while the engine catches up: the thumbnail is dropped.
  const slow = app({draw: () => 'drawn'});
  slow.playback.exportDocument = async () => { target.revision++; target.dispatchEvent(Object.assign(new Event('change'), {detail: {kind: 'edit'}})); return true; };
  const edited = new EngineThumbnails(target, {}, {stage: () => slow.stage, playback: () => slow.playback, javascript: js.make});
  assert.equal(await edited.renderThumbnail(0, canvas()), false);
  assert.equal(slow.calls.filter(call => call[0] === 'thumbnail').length, 0);
  // Paused (playback, a gesture) and destroyed renderers draw nothing.
  const {stage: idle, playback: idlePlayback} = app();
  const paused = new EngineThumbnails(target, {}, {stage: () => idle, playback: () => idlePlayback, javascript: js.make});
  paused.setPaused(true);
  assert.equal(await paused.renderThumbnail(0, canvas()), false);
  paused.destroy();
  assert.equal(await paused.renderThumbnail(0, canvas()), false);
  assert.equal(js.drawn.length, 0);
});

/** A fake preview worker: `respond(message)` answers, `null` never does. */
function fakeWorker(respond) {
  const workers = [];
  const spawn = () => {
    const worker = new EventTarget();
    worker.messages = [];
    worker.postMessage = message => {
      worker.messages.push(message);
      const data = message.operation === 'release' ? {released: true} : respond(message);
      if (data) queueMicrotask(() => worker.dispatchEvent(Object.assign(new Event('message'), {data})));
    };
    worker.terminate = () => { worker.terminated = true; };
    workers.push(worker);
    return worker;
  };
  return {workers, spawn};
}
const engine = {join: {module: 'module', memory: 'memory', stack: 1, progress: 2}};

test('the preview worker answers requests in order, drops cancelled ones and is released when idle', async () => {
  const held = [];
  const {workers, spawn} = fakeWorker(message => message.operation === 'start' ? {id: 0, ok: true} : message.operation === 'slow' ? (held.push(message), null) : {id: message.id, ok: true, echo: message.operation});
  const previews = new EnginePreviewWorker(engine, {spawn, idleMs: 20});
  assert.equal((await previews.request({operation: 'library-poster'})).echo, 'library-poster');
  assert.equal(workers.length, 1, 'started with the first request');
  assert.equal(workers[0].messages[0].operation, 'start');
  // One request at a time: a later one waits; cancelled before it is sent, it never is.
  const controller = new AbortController();
  const slow = previews.request({operation: 'slow'});
  const queued = previews.request({operation: 'library-frame'}, {signal: controller.signal});
  controller.abort();
  await assert.rejects(queued, {name: 'AbortError'});
  assert.equal(workers[0].messages.filter(m => m.operation === 'library-frame').length, 0);
  workers[0].dispatchEvent(Object.assign(new Event('message'), {data: {id: held[0].id, ok: true, echo: 'slow'}}));
  assert.equal((await slow).echo, 'slow');
  // Idle: released (the worker frees what it holds, then ends).
  await new Promise(resolve => setTimeout(resolve, 60));
  assert.equal(workers[0].messages.at(-1).operation, 'release');
  assert.equal(workers[0].terminated, true);
  assert.equal(previews.worker, null);
  // The next request starts a new worker.
  await previews.request({operation: 'library-poster'});
  assert.equal(workers.length, 2);
  previews.release();
});

test('a request\'s message can be made when it is sent', async () => {
  const {workers, spawn} = fakeWorker(message => message.operation === 'start' ? {id: 0, ok: true} : {id: message.id, ok: true, address: message.address});
  const previews = new EnginePreviewWorker(engine, {spawn});
  let made = 0;
  const controller = new AbortController();
  const first = previews.request(() => ({operation: 'thumbnail', address: ++made}));
  const dropped = previews.request(() => ({operation: 'thumbnail', address: ++made}), {signal: controller.signal});
  controller.abort();
  await assert.rejects(dropped, {name: 'AbortError'});
  assert.equal((await first).address, 1);
  assert.equal(made, 1, 'a request cancelled before it was sent made nothing');
  assert.equal(workers[0].messages.filter(m => m.operation === 'thumbnail').length, 1);
  previews.release();
});

test('a preview worker that cannot start leaves every request to the JavaScript workers', async () => {
  const {workers, spawn} = fakeWorker(message => message.operation === 'start' ? {id: 0, ok: false, error: 'the GPU canvas gives no pixels in this worker'} : null);
  const previews = new EnginePreviewWorker(engine, {spawn});
  await assert.rejects(previews.request({operation: 'library-poster'}), /could not start/);
  assert.equal(previews.usable, false);
  await assert.rejects(previews.request({operation: 'library-poster'}), /could not start/);
  // It answered, so it is asked to free its thread's memory before it ends (not just terminated).
  await new Promise(resolve => setTimeout(resolve, 10));
  assert.equal(workers[0].messages.at(-1).operation, 'release');
  assert.equal(workers[0].terminated, true);
  assert.equal(workers.length, 1, 'no second worker is started');
});
