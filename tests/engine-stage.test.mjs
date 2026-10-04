// The engine's stage (src/engine/engine-stage.mjs): a still frame drawn while its pictures
// decode is drawn again once they can be shown, without a loop on the interface thread.
import test from 'node:test';
import assert from 'node:assert/strict';
import {waitForPictures, ensureCameraFrames, cancelCameraFrames, recordCameraChange} from '../src/engine/engine-stage.mjs';

/** A stage side whose last frame lacks pictures as `pending` says, and an engine whose progress counter the test advances. */
function side(pending) {
  let progress = 0, wake = null;
  const waits = [];
  const engine = {
    progressSeen: () => progress,
    // Resolves when the counter moves past `seen`, or when the test lets the wait time out.
    whenProgress(seen) {
      waits.push(seen);
      if (progress !== seen) return Promise.resolve();
      return new Promise(resolve => { wake = resolve; });
    },
  };
  return {
    side: {stage: {pictures_pending: () => pending.shift() ?? 0}, engine, pictureWait: 0, stats: {pictureRedraws: 0}},
    waits,
    advance() { progress++; wake?.(); wake = null; },
    timeout() { wake?.(); wake = null; },
  };
}

const settle = () => new Promise(resolve => setTimeout(resolve, 0));

test('a frame with every picture is not drawn again', async () => {
  const {side: s, waits} = side([0]);
  let redraws = 0;
  waitForPictures(s, 0, () => redraws++);
  await settle();
  assert.equal(redraws, 0);
  assert.equal(waits.length, 0, 'nothing waits on the engine');
});

test('pictures decoded and waiting for upload: drawn again on the next frame', async () => {
  const {side: s, waits} = side([1]);
  let redraws = 0;
  waitForPictures(s, 0, () => redraws++);
  assert.equal(redraws, 0, 'not inside the frame being drawn');
  await settle();
  assert.equal(redraws, 1);
  assert.equal(waits.length, 0);
  assert.equal(s.stats.pictureRedraws, 1);
});

test('pictures still decoding: drawn again once the engine finishes something, not on a timeout alone', async () => {
  const engine = side([2]);
  let redraws = 0;
  waitForPictures(engine.side, 0, () => redraws++);
  await settle();
  assert.equal(redraws, 0, 'waits on the progress counter');
  engine.timeout();
  await settle();
  assert.equal(redraws, 0, 'a wait that timed out with nothing finished waits again without drawing');
  assert.deepEqual(engine.waits, [0, 0]);
  engine.advance();
  await settle();
  assert.equal(redraws, 1);
});

test('progress made while the frame was drawn is not missed', async () => {
  const engine = side([2]);
  let redraws = 0;
  // The counter was read before drawing; a picture decoded meanwhile.
  engine.advance();
  waitForPictures(engine.side, 0, () => redraws++);
  await settle();
  assert.equal(redraws, 1);
});

test('another frame drawn meanwhile takes over the wait', async () => {
  const engine = side([2, 0]);
  let first = 0, second = 0;
  waitForPictures(engine.side, 0, () => first++);
  waitForPictures(engine.side, 0, () => second++);
  engine.advance();
  await settle();
  assert.equal(first, 0, 'the superseded wait does not draw');
  assert.equal(second, 0, 'the frame drawn last had its pictures');
});

function cameraSide() {
  const calls = [];
  let current = null;
  const s = {mirror: {
    framesCurrent: (report, revision) => current?.report === report && current.revision === revision,
    syncFrames(report, revision, dirty, {signal}) {
      let resolve;
      const promise = new Promise(done => { resolve = done; });
      calls.push({report, revision, dirty, signal, finish() { current = {report, revision}; resolve(); }});
      return promise;
    },
  }};
  return {side: s, calls, current(report, revision) { current = {report, revision}; }};
}

test('a definitions-only followed pose coalesces one asynchronous sync on the existing mirror', async () => {
  const {side: s, calls} = cameraSide(), report = {}, dirty = {frameIndices: [3]};
  s.cameraDirty = dirty;
  let redraws = 0;
  assert.equal(ensureCameraFrames(s, {report, revision: 4}, () => redraws++), true);
  assert.equal(calls.length, 0, 'no frame lifting occurs inside render_edit');
  ensureCameraFrames(s, {report, revision: 4}, () => redraws++);
  await settle();
  assert.equal(calls.length, 1, 'the cold pose reuses one sync');
  assert.equal(calls[0].dirty, dirty);
  calls[0].finish();
  await settle();
  assert.equal(redraws, 1);
  assert.equal(s.cameraFrames, null);
  assert.equal(s.cameraDirty, null);
  assert.equal(ensureCameraFrames(s, {report, revision: 4}, () => redraws++), false);
});

test('a new report or revision cancels the cold camera sync and discards its late redraw', async () => {
  const {side: s, calls} = cameraSide(), report = {};
  let old = 0, latest = 0;
  ensureCameraFrames(s, {report, revision: 1}, () => old++);
  await settle();
  ensureCameraFrames(s, {report, revision: 2}, () => latest++);
  await settle();
  assert.equal(calls[0].signal.aborted, true);
  calls[0].finish();
  await settle();
  assert.equal(old, 0);
  assert.equal(s.cameraFrames.revision, 2, 'the stale finally keeps the newest owner');
  calls[1].finish();
  await settle();
  assert.equal(latest, 1);
});

test('disposing a pending camera prerequisite prevents late work from redrawing the stage', async () => {
  const {side: s, calls} = cameraSide();
  let redraws = 0;
  ensureCameraFrames(s, {report: {}, revision: 1}, () => redraws++);
  await settle();
  cancelCameraFrames(s);
  assert.equal(calls[0].signal.aborted, true);
  calls[0].finish();
  await settle();
  assert.equal(redraws, 0);
  assert.equal(s.cameraFrames, null);
});

test('camera dirty identities stay bounded by live records while the preview is off', () => {
  const s = {frameReport: {keyframes: [{}], figure_types: [{}], _styk: {frameSources: {one: {}}}}};
  for (let i = 0; i < 100_000; i++) recordCameraChange(s, {frameIndices: [0, 100], figureIndices: [0, 100], sourceIds: ['one', `removed-${i}`]});
  assert.deepEqual([...s.cameraDirty.frameIndices], [0]);
  assert.deepEqual([...s.cameraDirty.figureIndices], [0]);
  assert.deepEqual([...s.cameraDirty.sourceIds], ['one']);
  delete s.frameReport._styk.frameSources.one;
  recordCameraChange(s, {sourceIds: ['one']});
  assert.equal(s.cameraDirty.sourceIds.size, 0, 'removed sources leave no retained identity');
});

test('a large dirty identity stream conservatively resends through the existing mirror with bounded state', () => {
  const s = {frameReport: {keyframes: Array(1024).fill({}), figure_types: Array(1024).fill({}), _styk: {frameSources: Object.fromEntries(Array.from({length: 1024}, (_, i) => [`source-${i}`, {}]))}}};
  for (let i = 0; i < 100_000; i++) recordCameraChange(s, {frameIndices: [i % 1024], figureIndices: [i % 1024], sourceIds: [`source-${i % 1024}`]});
  assert.equal(s.cameraDirty.allFrames, true);
  assert.equal(s.cameraDirty.allFigures, true);
  assert.equal(s.cameraDirty.allSources, true);
  assert.equal(s.cameraDirty.frameIndices.size + s.cameraDirty.figureIndices.size + s.cameraDirty.sourceIds.size, 0);
});

test('a canceled camera revision keeps the newest dirty records until their own mirror ACK', async () => {
  const {side: s, calls} = cameraSide(), report = {keyframes: [{}, {}], figure_types: [], _styk: {frameSources: {latest: {}}}};
  s.frameReport = report;
  recordCameraChange(s, {frameIndices: [0]});
  ensureCameraFrames(s, {report, revision: 1}, () => {});
  await settle();
  cancelCameraFrames(s);
  recordCameraChange(s, {frameIndices: [1], sourceIds: ['latest']});
  ensureCameraFrames(s, {report, revision: 2}, () => {});
  await settle();
  calls[0].finish();
  await settle();
  assert.deepEqual([...s.cameraDirty.frameIndices], [0, 1]);
  assert.deepEqual([...calls[1].dirty.sourceIds], ['latest']);
  calls[1].finish();
  await settle();
  assert.equal(s.cameraDirty, null);
});

test('one dense dirty event is conservatively bounded even when every identity repeats', () => {
  let visited = 0;
  const items = {*[Symbol.iterator]() { for (let i = 0; i < 100_000; i++) { visited++; yield 0; } }};
  const s = {frameReport: {keyframes: [{}]}};
  recordCameraChange(s, {frameIndices: items});
  assert.equal(s.cameraDirty.allFrames, true);
  assert.equal(s.cameraDirty.frameIndices.size, 0);
  assert.equal(visited, 257, 'the rest is covered by a worker-side full resend, not a dense UI walk');
});
