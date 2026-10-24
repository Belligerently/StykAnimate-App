// The stage (src/engine/engine-stage.mjs) over a stand-in engine: the engine draws every frame, a
// frame it refuses leaves the previous picture (no other renderer exists), and the device, the
// view's turn and the surface limit reach it.
import test from 'node:test';
import assert from 'node:assert/strict';

globalThis.requestAnimationFrame ??= callback => setTimeout(callback, 0);
let instance = 0;
/** A fresh module instance: the engine side is shared by every stage a module creates. */
const freshStage = async () => (await import(`../src/engine/engine-stage.mjs?instance=${++instance}`)).createStage;

/** A stand-in for the engine's `Stage`: it records what the interface asks of it. */
function standInStage(behaviour = {}) {
  const calls = [];
  const record = name => (...args) => { calls.push([name, ...args]); return behaviour[name]?.(...args); };
  const stage = {
    calls,
    backend: () => 'BrowserWebGpu · Test GPU',
    stats: () => JSON.stringify({prims: 12, groups: 3, passes: 2, effectBytes: 1, layerBytes: 2, depthBytes: 3, materialBytes: 4,
      glyphBytes: 5, uploadBytes: 6, pictures: 7, multisampled: true}),
    take_device_errors: () => behaviour.errors?.() ?? '',
    device_lost: () => behaviour.lost?.() ?? '',
    max_texture: () => 4096,
    refining: () => false, camera_pending: () => false, pictures_pending: () => 0, surface_pending: () => behaviour.pending?.() ?? false,
    has_ghost: () => false,
    ...Object.fromEntries(['resize', 'set_effects', 'set_canvas_drawing', 'set_view_orientation', 'set_multisampled', 'edit_frame', 'edit_ghost', 'lose_device', 'free']
      .map(name => [name, record(name)])),
    render_edit: (...args) => { calls.push(['render_edit', ...args]); return behaviour.render_edit ? behaviour.render_edit(...args) : true; },
    render_ticket: (...args) => { calls.push(['render_ticket', ...args]); return behaviour.render_ticket ? behaviour.render_ticket(...args) : true; },
  };
  return stage;
}

/** Creates a stage over a stand-in engine; `mirror` replaces the engine mirror. */
async function open(behaviour = {}, options = {}) {
  const stage = standInStage(behaviour);
  const progress = [];
  const engine = {handle: {frame_ticket_ready: () => true}, threaded: false, progressSeen: () => 0,
    whenProgress: () => new Promise(resolve => progress.push(resolve)), createStage: async () => stage};
  const createStage = await freshStage();
  const canvas = {width: 300, height: 150};
  const events = {invalidations: 0, errors: [], statuses: []};
  const renderer = await createStage(canvas, {onInvalidate: () => events.invalidations++, onError: error => events.errors.push(error),
    onStatus: message => events.statuses.push(message), ...options}, async () => engine);
  const syncs = [];
  renderer.engineSide.mirror = {report: null, stats: {}, framesCurrent: () => true,
    syncDefinitions(report, revision) { syncs.push([report, revision]); if (behaviour.sync) behaviour.sync(); this.report = report; return '{}'; }};
  return {renderer, stage, canvas, events, syncs, progress, engine};
}

const frame = {objects: [], camera: null};
const report = {};
const request = (overrides = {}) => ({report, frame, geometry: null, view: {scale: 1, x: 0, y: 0}, revision: 1, artworkRevision: 1,
  vfxEnabled: true, cameraEnabled: false, hiddenUids: {has: () => false}, onionFrames: [], ...overrides});

test('an edited frame is drawn by the engine, with or without editor geometry', async () => {
  const {renderer, stage} = await open();
  assert.equal(renderer.render(request()), true);
  const call = stage.calls.find(([name]) => name === 'render_edit');
  assert.ok(call, 'the engine drew the frame');
  assert.equal(renderer.engineSide.stats.frames, 1);
  renderer.destroy();
});

test('a turned and mirrored view reaches the engine before an edited or a prepared frame is drawn', async () => {
  const {renderer, stage} = await open();
  const view = {scale: 2, x: 3, y: 4, rotation: Math.PI / 2, flip: true};
  renderer.render(request({view}));
  renderer.renderPrepared('engine:7:2:3', {view, cameraEnabled: true});
  const names = stage.calls.map(([name]) => name);
  const edit = names.indexOf('render_edit'), ticket = names.indexOf('render_ticket');
  assert.ok(edit > 0 && ticket > edit);
  const orientations = stage.calls.filter(([name]) => name === 'set_view_orientation');
  assert.deepEqual(orientations.map(([, rotation, flip]) => [rotation, flip]), [[Math.PI / 2, true], [Math.PI / 2, true]]);
  assert.ok(names.indexOf('set_view_orientation') < edit, 'the turn is set before the edited frame is drawn');
  assert.ok(names.indexOf('set_view_orientation', edit) < ticket, 'and again before the prepared one');
  renderer.render(request({view: {scale: 1, x: 0, y: 0}}));
  assert.deepEqual(stage.calls.filter(([name]) => name === 'set_view_orientation').at(-1).slice(1), [0, false], 'an upright view turns nothing');
  renderer.destroy();
});

test('an edited frame is drawn in device pixels at the size the page asked for', async () => {
  const {renderer, stage, canvas} = await open();
  renderer.resize(100, 50, 2);
  assert.equal(renderer.dpr, 2);
  assert.deepEqual([canvas.width, canvas.height], [200, 100]);
  renderer.render(request({view: {scale: 1.5, x: 10, y: 20}}));
  const [, zoom, panX, panY] = stage.calls.find(([name]) => name === 'render_edit');
  assert.deepEqual([zoom, panX, panY], [3, 20, 40]);
  renderer.destroy();
});

test('a surface larger than the device allows is clamped to it', async () => {
  const {renderer, stage, canvas} = await open();
  renderer.resize(10000, 6000, 1);
  assert.ok(canvas.width <= 4096 && canvas.height <= 4096);
  assert.equal(stage.calls.filter(([name]) => name === 'resize').length, 1);
  assert.equal(renderer.dpr < 1, true, 'the scale shrinks to fit');
  renderer.destroy();
});

test('a frame the engine refuses leaves the previous picture, is reported once, and the next revision is tried again', async () => {
  let refuse = true;
  const {renderer, stage, events, syncs} = await open({sync() { if (refuse) throw new Error('the engine refused the figure'); }});
  assert.equal(renderer.render(request({revision: 1})), false);
  assert.equal(events.errors.length, 1);
  assert.match(events.errors[0].message, /refused the figure/);
  assert.equal(renderer.render(request({revision: 1})), false);
  assert.equal(syncs.length, 1, 'the same revision of the same document is not tried again');
  assert.equal(events.errors.length, 1, 'and not reported again');
  assert.equal(stage.calls.some(([name]) => name === 'render_edit'), false, 'no picture was drawn meanwhile');
  refuse = false;
  assert.equal(renderer.render(request({revision: 2})), true, 'the next revision is tried afresh');
  assert.equal(renderer.engineSide.failed, null);
  renderer.destroy();
});

test('a commit running in the engine delays the frame without refusing it', async () => {
  let busy = true;
  const {renderer, events, progress, syncs} = await open({sync() { if (busy) throw Object.assign(new Error('The engine is committing the document.'), {code: 'engine_busy'}); }});
  assert.equal(renderer.render(request()), false);
  assert.equal(events.errors.length, 0, 'a busy engine is not an error');
  assert.equal(progress.length, 1, 'one waiter for the engine to publish');
  assert.equal(renderer.render(request()), false);
  assert.equal(progress.length, 1, 'never more than one');
  busy = false;
  progress.shift()();
  await new Promise(resolve => setTimeout(resolve, 0));
  assert.equal(events.invalidations, 1, 'the page draws again once the engine moved on');
  assert.equal(renderer.render(request()), true);
  assert.equal(syncs.length, 3);
  renderer.destroy();
});

test('the new-cel preview of a drawing is not a document the engine holds', async () => {
  const {renderer, stage, syncs} = await open();
  assert.equal(renderer.render(request({drawingPreview: true})), false);
  assert.equal(syncs.length, 0);
  assert.equal(stage.calls.length, 0);
  renderer.destroy();
});

test('a lost device is reported once and draws nothing more', async () => {
  let lost = '';
  const {renderer, stage, events} = await open({lost: () => lost});
  assert.equal(renderer.render(request()), true);
  lost = 'Destroyed: the driver reset';
  assert.equal(renderer.render(request()), false);
  assert.equal(renderer.render(request()), false);
  await Promise.resolve();
  assert.equal(events.statuses.length, 1);
  assert.equal(events.statuses[0].backend, 'WebGPU device lost');
  assert.equal(events.statuses[0].messageKey, 'rendererStatus.deviceDisconnected');
  assert.equal(events.statuses[0].messageParams.detail, 'Destroyed: the driver reset');
  assert.equal(renderer.stats.backend, 'WebGPU device lost');
  assert.equal(stage.calls.filter(([name]) => name === 'render_edit').length, 1);
  renderer.destroy();
});

test('errors the device reports are kept as warnings, not raised', async () => {
  let pending = 'Validation Error: bad bind group';
  const warn = console.warn;
  console.warn = () => {};
  const {renderer} = await open({errors: () => { const out = pending; pending = ''; return out; }});
  renderer.render(request());
  console.warn = warn;
  assert.deepEqual(renderer.stats.warnings, ['Validation Error: bad bind group']);
  renderer.destroy();
});

test('the counters the status label and telemetry read come from the engine', async () => {
  const {renderer} = await open();
  renderer.render(request());
  const stats = renderer.stats;
  assert.equal(stats.backend, 'WebGPU');
  assert.equal(stats.adapter, 'Test GPU');
  assert.deepEqual([stats.primitives, stats.objects, stats.drawCalls, stats.gpuBytes, stats.uploadBytes, stats.textureObjects, stats.multisampled], [12, 3, 2, 15, 6, 7, true]);
  renderer.destroy();
});

test('the quality ladder sets the engine\'s edge sampling', async () => {
  const {renderer, stage} = await open();
  renderer.setMsaa(1);
  renderer.setMsaa(4);
  assert.deepEqual(stage.calls.filter(([name]) => name === 'set_multisampled').map(([, on]) => on), [false, true]);
  renderer.destroy();
});

test('a destroyed stage frees its device, draws nothing, and a new stage keeps the engine\'s document', async () => {
  const first = await open();
  first.renderer.render(request());
  const document = first.renderer.engineSide.mirror;
  first.renderer.destroy();
  assert.equal(first.stage.calls.filter(([name]) => name === 'free').length, 1);
  assert.equal(first.renderer.render(request()), false);
  assert.equal(first.renderer.renderPrepared('engine:7:2:3'), false);
  const second = standInStage();
  const createStage = await freshStage();
  const engine = {handle: {frame_ticket_ready: () => true}, threaded: false, progressSeen: () => 0, whenProgress: () => new Promise(() => {}), createStage: async () => second};
  const again = await createStage({width: 1, height: 1}, {}, async () => engine);
  assert.equal(again.engineSide.stage, second);
  again.destroy();
  void document;
});

test('a stage that cannot start rejects with a localized error', async () => {
  const createStage = await freshStage();
  const engine = {createStage: async () => { throw new Error('no adapter'); }};
  await assert.rejects(createStage({width: 1, height: 1}, {}, async () => engine), error => {
    assert.equal(error.messageKey, 'errors.engineStage.theDrawingEngineCouldNotStart');
    assert.match(error.message, /no adapter/);
    return true;
  });
  await assert.rejects((await freshStage())({width: 1, height: 1}, {}, async () => { throw new Error('no WebAssembly threads'); }), error => {
    assert.equal(error.messageKey, 'errors.engineStage.theDrawingEngineCouldNotStart');
    return true;
  });
});

test('after an error the poses are sent again: the engine may not hold what the stage believes it sent', async () => {
  let fail = true;
  const {renderer, stage, events} = await open({render_edit() { if (fail) throw new Error('the draw failed'); return true; }});
  assert.equal(renderer.render(request({revision: 1})), false);
  assert.equal(events.errors.length, 1);
  const sent = () => stage.calls.filter(([name]) => name === 'edit_frame').length;
  assert.equal(sent(), 1);
  fail = false;
  assert.equal(renderer.render(request({revision: 2})), true);
  assert.equal(sent(), 2, 'the unchanged frame went to the engine again');
  assert.equal(renderer.render(request({revision: 2})), true);
  assert.equal(sent(), 2, 'and only once');
  renderer.destroy();
});
