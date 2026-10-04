import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {projectWorker} from '../src/engine/engine-project-worker.mjs';
import {EngineMirror} from '../src/engine/engine-mirror.mjs';
import {waitForEngineProgress} from '../src/engine/engine.mjs';
import {documentChunks} from '../src/document-transport.mjs';
import {open} from '../engine/tools/fixture-kit.mjs';

const read = mode => JSON.parse(readFileSync(new URL(`../engine/tests/session-quiet/${mode}.json`, import.meta.url)));
// Only transport/scheduling is mocked here. Responses use real editor-built
// Fields/state/history records; native actor tests prove the owning Rust heads,
// serialization, final commit and retained lifetime against these same traces.
function host(mode = 'native', onProgress = () => {}) {
  const reference = read(mode), first = reference.steps[0];
  const editor = open(JSON.parse(first.document), reference.options), report = editor.report;
  const steps = reference.steps.slice(1).filter(step => step.fields), identity = [5, 4, first.history.revision], epochs = [71, 4, 0];
  let progress = 0, cursor = 0, next = 2, packets = [], packet, stagedStep, sequence = 0, request = 0, stamp = [...identity, 0];
  const worker = {phase: 'idle', closing: false, cancelled: false, advanceRequested: false, committed: 0, disposed: 0, terminalCount: 0, submitted: 0,
    submit_project_fields(numbers, method, args, view) {
      this.submitted++;assert.equal(this.phase, 'idle');assert.deepEqual([...numbers], [...stamp, ...epochs]);assert.equal(view, 'null');
      stagedStep = steps[cursor++];assert.equal(method, stagedStep.call[0]);assert.deepEqual(JSON.parse(args), stagedStep.call.slice(1));
      const {history, state, error, fields, result, events} = stagedStep;
      const payload = {kind: 'project-fields', answer: {...(error ? {status: 'error', error: error.key} : {status: 'ok', result}), state, events}, fields,
        history: {size: history.size, revision: history.revision, revisionSerial: history.revisionSerial, savedRevision: history.savedRevision,
          undo: history.undo.length, redo: history.redo.length, limit: reference.options.historyLimit, byteLimit: reference.options.historyBytes}};
      packets = [...documentChunks(payload, {maxBytes: 65536, maxRecords: 256})].map(chunk => JSON.stringify(chunk));
      assert.ok(packets.every(text => Buffer.byteLength(text) <= 65536));
      sequence = 0;request = next++;this.cancelled = false;this.phase = 'preparing';this.advanceRequested = true;
      return request;
    },
    advance() {
      if (this.closing) { this.phase = 'closed';this.disposed++;return; }
      if (this.cancelled && !['success', 'failed', 'idle'].includes(this.phase)) { cursor--;packet = undefined;packets = [];this.phase = 'failed';return; }
      if (!this.advanceRequested) return;this.advanceRequested = false;
      if (this.phase === 'preparing' || this.phase === 'streaming') {
        packet = packets.shift();this.phase = packet === undefined ? 'ready' : 'streaming';
      } else if (this.phase === 'ready') {
        this.phase = 'success';this.committed++;identity[0]++;identity[2] = stagedStep.history.revision;
        stamp = [...identity, request];
      } else if (this.phase === 'success' || this.phase === 'failed') { this.phase = 'idle';request = 0; }
    },
    poll() { return JSON.stringify({request, phase: this.phase, ...(packet !== undefined ? {packetSequence: sequence} : {}),
      ...(this.phase === 'success' ? {stamp, epochs} : {}), ...(this.phase === 'failed' ? {error: 'Editing cancelled.'} : {})}); },
    packet() { const text = packet;packet = undefined;return text; },
    ack_packet(id, given) { assert.equal(id, request);assert.equal(given, sequence++);this.advanceRequested = true;return true; },
    ready(id, given) { assert.equal(id, request);assert.deepEqual([...given], epochs);this.advanceRequested = true;return true; },
    terminal(id) { assert.equal(id, request);this.terminalCount++;this.advanceRequested = true;return true; },
    cancel(id) { assert.equal(id, request || id);this.cancelled = true; },
    close() { this.closing = true; },
  };
  const engine = {threaded: true, handle: {try_record_identity: () => identity,
    record_identity() { throw Error('project controls must not take the spinning identity read'); }}, progressSeen: () => progress,
    async whenProgress() { progress++;worker.advance();onProgress(progress, worker, epochs); }};
  const mirror = new EngineMirror(engine);mirror.report = report;mirror.framesRevision = identity[2];
  const sent = mirror.sent, sources = mirror.sources;
  const authority = {worker, stamp, epochs}, publications = [], barriers = [];
  const route = projectWorker({engine, mirror}, authority, {report, epochs: () => epochs,
    pending(value) { barriers.push(value); }, publish(value) { publications.push(value); }});
  return {reference, steps, editor, report, route, worker, authority, publications, barriers, mirror, sent, sources, epochs};
}
function invoke(route, call, signal) {
  const options = {signal};
  if (call[0] === 'updateProject') return route.updateProject(Object.entries(call[1]), options);
  return route[call[0]](...call.slice(1), options);
}

test('actual present/null settings preserve raw root, unknown order and references without JS history rerun', async () => {
  for (const mode of ['native', 'sheet', 'native-null', 'sheet-null', 'native-zero', 'sheet-zero']) {
    const h = host(mode), root = h.report, figures = root.figure_types, frames = root.keyframes, unknown = root.__proto__, future = root.futureTop;
    let expected = h.reference.steps[0].document;
    const records = h.reference.steps.slice(1);
    for (const method of ['setFPS', 'setCanvasSize', 'updateProject', 'undo', 'redo']) h.editor[method] = () => { throw Error('synchronous shadow/history rerun'); };
    h.editor._undo.push = () => { throw Error('foreign JS history append'); };
    for (const step of records) {
      if (step.document) expected = step.document;
      if (!step.fields) continue;
      if (step.error) await assert.rejects(invoke(h.route, step.call), new RegExp(step.error.key.replace(/[.]/g, '\\.')));
      else assert.deepEqual(await invoke(h.route, step.call), step.result);
      assert.equal(root, h.report);assert.equal(root.figure_types, figures);assert.equal(root.keyframes, frames);
      assert.equal(root.__proto__, unknown);assert.equal(root.futureTop, future);
      assert.equal(JSON.stringify(root, (key, value) => key === 'runtime_segments' ? undefined : value), expected);
      assert.equal(h.mirror.sent, h.sent);assert.equal(h.mirror.sources, h.sources);
      assert.equal(h.mirror.framesRevision, step.history.revision);
      assert.deepEqual(h.publications.at(-1).answer.state, step.state);
      assert.deepEqual(h.publications.at(-1).answer.events, step.events);
      assert.equal(h.editor._undo.length, 0);assert.equal(h.editor._redo.length, 0);
      assert.equal(h.route.busy, false);assert.equal(h.worker.phase, 'idle');assert.equal(h.worker.closing, false);
    }
    assert.equal(h.worker.terminalCount, h.steps.length);
    assert.ok(h.barriers.every((value, index) => value === (index % 2 === 0)));
  }
});

test('UI controls never enumerate or serialize dense unknown report fields', async () => {
  const h = host(), original = h.report.futureTop;
  h.report.futureTop = {toJSON() { throw Error('dense raw JSON walk'); }};
  const root = h.report;
  const controlled = new Proxy(root, {ownKeys() { throw Error('dense raw own-key walk'); }});
  h.mirror.report = controlled;
  const route = projectWorker({engine: h.mirror.engine, mirror: h.mirror}, h.authority,
    {report: controlled, epochs: () => h.epochs, pending() {}, publish() {}});
  assert.equal(await route.setFPS(31), true);
  assert.equal(root.fps, 31);assert.equal(root.futureTop.toJSON instanceof Function, true);
  root.futureTop = original;
});

test('cancel before commit and stale readiness leave root unchanged; dependent calls stay behind barrier', async () => {
  for (const kind of ['cancel', 'stale']) {
    const controller = new AbortController();
    const h = host('native', (_progress, worker, epochs) => {
      if (worker.phase === 'ready') { if (kind === 'cancel') controller.abort();else epochs[1]++; }
    });
    const before = JSON.stringify(h.report), action = h.route.setFPS(31, {signal: controller.signal});
    await assert.rejects(h.route.setFPS(31), /busy/);
    await assert.rejects(action, kind === 'cancel' ? {name: 'AbortError'} : /stale/);
    assert.equal(JSON.stringify(h.report), before);assert.equal(h.worker.committed, 0);
    assert.equal(h.publications.length, 0);assert.equal(h.worker.phase, 'idle');assert.equal(h.route.busy, false);
  }
});

test('cancel racing final success accepts exactly once and keeps warm canonical owner', async () => {
  const controller = new AbortController();
  const h = host('native', (_progress, worker) => { if (worker.phase === 'success') controller.abort(); });
  assert.equal(await h.route.setFPS(31, {signal: controller.signal}), true);
  assert.equal(h.report.fps, 31);assert.equal(h.worker.committed, 1);
  assert.equal(h.publications.length, 1);assert.equal(h.worker.terminalCount, 1);
  assert.equal(h.worker.closing, false);assert.equal(h.worker.phase, 'idle');
});

test('bootstrap reservation and cancelled requests preserve actual tagged request sequence gaps', async () => {
  const controller = new AbortController();
  let cancelOnce = true;
  const h = host('native', (_progress, worker) => {
    if (worker.phase === 'ready' && cancelOnce) { cancelOnce = false;controller.abort(); }
  });
  // Fresh native bootstrap reserved request1. Request2 is cancelled, leaving
  // actual owning stamp0 unchanged; the next committed request is3, not1.
  await assert.rejects(h.route.setFPS(31, {signal: controller.signal}), {name: 'AbortError'});
  assert.equal(h.authority.stamp[3], 0);assert.equal(h.worker.committed, 0);
  assert.equal(await h.route.setFPS(31), true);
  assert.equal(h.authority.stamp[3], 3);assert.equal(h.worker.committed, 1);
  assert.equal(h.worker.closing, false);assert.equal(h.report.fps, 31);
  assert.equal(await h.route.setFPS(31), false);
  assert.equal(h.authority.stamp[3], 4);assert.equal(h.worker.committed, 2);
  assert.equal(h.worker.phase, 'idle');assert.equal(h.publications.length, 2);
});

test('a throwing publication callback retires the committed worker before releasing the host barrier', async () => {
  const h = host(), barriers = [];
  let calls = 0;
  const route = projectWorker({engine: h.mirror.engine, mirror: h.mirror}, h.authority,
    {report: h.report, epochs: () => h.epochs,
      pending(value) {
        if (!value) assert.equal(h.worker.phase, 'closed');
        barriers.push(value);
      },
      publish() { calls++;throw Error('host publication failed'); }});
  await assert.rejects(route.setFPS(31), /host publication failed/);
  assert.equal(calls, 1);assert.equal(h.worker.committed, 1);
  assert.equal(h.report.fps, 31);assert.equal(h.worker.disposed, 1);
  assert.equal(h.worker.terminalCount, 0);assert.deepEqual(barriers, [true, false]);
  await assert.rejects(route.setFPS(31), /closed/);
  assert.equal(h.worker.committed, 1);
});

test('wide control arrays and arbitrary toJSON values refuse before serialization/submission', () => {
  const h = host(), wide = Array.from({length: 4}, () => ['fps', 31]);
  wide[Symbol.iterator] = () => { throw Error('unbounded argument iteration'); };
  assert.throws(() => h.route.updateProject(wide), /control budget/);
  assert.throws(() => h.route.updateProject([['fps', {toJSON() { throw Error('unbounded toJSON'); }}]]), /scalar controls/);
  assert.throws(() => h.route.setFPS({toJSON() { throw Error('unbounded toJSON'); }}), /numeric control/);
  assert.equal(h.worker.phase, 'idle');assert.equal(h.barriers.length, 0);
});

test('actual nonfinite settings reject unchanged and never serialize to a legal null or queue work', () => {
  for (const mode of ['native', 'sheet']) {
    for (const value of [NaN, Infinity, -Infinity]) {
      const h = host(mode), before = JSON.stringify(h.report);
      const size = h.editor.historySize, revision = h.editor.revision;
      for (const patch of [{canvas_width: value}, {canvas_height: value}, {fps: value}]) {
        assert.throws(() => h.editor.updateProject(patch));
        assert.throws(() => h.route.updateProject(Object.entries(patch)), /scalar controls/);
      }
      assert.throws(() => h.editor.setFPS(value));
      assert.throws(() => h.route.setFPS(value), /finite numeric control/);
      for (const args of [[value, 480], [720, value]]) {
        assert.throws(() => h.editor.setCanvasSize(...args));
        assert.throws(() => h.route.setCanvasSize(...args), /finite numeric controls/);
      }
      assert.equal(JSON.stringify(h.report), before);
      assert.equal(h.editor.historySize, size);assert.equal(h.editor.revision, revision);
      assert.equal(h.worker.phase, 'idle');assert.equal(h.worker.committed, 0);
      assert.equal(h.worker.cancelled, false);assert.equal(h.publications.length, 0);
      assert.equal(h.barriers.length, 0);
    }
  }
});


test('busy identity at submission or ready ACK preserves the canonical head and permits an explicit retry', async () => {
  for (const at of ['submission', 'ready']) {
    let hostSide, denyReady = true;
    const h = host('native', (_progress, worker) => {
      if (at === 'ready' && worker.phase === 'ready' && denyReady) {
        denyReady = false;hostSide.handle.try_record_identity = () => undefined;
      }
    });
    hostSide = h.mirror.engine;
    const readIdentity = hostSide.handle.try_record_identity;
    const before = JSON.stringify(h.report), stamp = h.authority.stamp, framesRevision = h.mirror.framesRevision;
    if (at === 'submission') hostSide.handle.try_record_identity = () => undefined;
    await assert.rejects(h.route.setFPS(31), {code: 'engine_busy'});
    assert.equal(JSON.stringify(h.report), before);
    assert.equal(h.authority.stamp, stamp);assert.equal(h.mirror.framesRevision, framesRevision);
    assert.equal(h.worker.committed, 0);assert.equal(h.publications.length, 0);
    assert.equal(h.worker.submitted, at === 'submission' ? 0 : 1);
    assert.equal(h.worker.cancelled, at === 'ready');
    assert.equal(h.worker.phase, 'idle');assert.equal(h.route.busy, false);
    assert.deepEqual(h.barriers, at === 'submission' ? [] : [true, false]);
    hostSide.handle.try_record_identity = readIdentity;
    // Only the first request receives the temporary readiness Busy.
    h.worker.advance = ((advance) => function () {
      advance.call(this);hostSide.handle.try_record_identity = readIdentity;
    })(h.worker.advance);
    assert.equal(await h.route.setFPS(31), true);
    assert.equal(h.report.fps, 31);assert.equal(h.worker.committed, 1);
    assert.equal(h.publications.length, 1);assert.equal(h.worker.phase, 'idle');
  }
});


test('postcommit identity Busy without progress retains the old head and accepts exactly once after lock release', async () => {
  let h, hold = true, observed = 0;
  h = host('native', (_progress, worker) => {
    if (worker.phase === 'success' && hold) {
      hold = false;
      const read = h.mirror.engine.handle.try_record_identity, seen = h.mirror.engine.progressSeen();
      h.mirror.engine.handle.try_record_identity = () => undefined;
      const wait = h.mirror.engine.whenProgress;
      h.mirror.engine.whenProgress = function (...args) {
        if (worker.closing) return wait.apply(this, args);
        if (worker.phase === 'success' && worker.terminalCount === 0) {
          assert.equal(args[1], 16);return waitForEngineProgress(new Int32Array(new SharedArrayBuffer(4)), false, 0, args[1]);
        }
        return wait.apply(this, args);
      };
      setTimeout(() => {
        assert.equal(h.mirror.engine.progressSeen(), seen);
        assert.equal(h.report.fps, JSON.parse(h.reference.steps[0].document).fps);
        assert.equal(h.publications.length, 0);assert.equal(h.barriers.at(-1), true);
        observed++;h.mirror.engine.handle.try_record_identity = read;
      }, 0);
    }
  });
  assert.equal(await h.route.setFPS(31), true);
  assert.equal(observed, 1);assert.equal(h.report.fps, 31);
  assert.equal(h.worker.submitted, 1);assert.equal(h.worker.committed, 1);
  assert.equal(h.publications.length, 1);assert.equal(h.worker.terminalCount, 1);
  assert.equal(h.worker.phase, 'idle');assert.deepEqual(h.barriers, [true, false]);
});

test('close without progress during committed Busy acceptance retires before the barrier releases', async () => {
  let h, closeOnce = true;
  h = host('native', (_progress, worker) => {
    if (worker.phase === 'success' && closeOnce) {
      closeOnce = false;
      const seen = h.mirror.engine.progressSeen();
      h.mirror.engine.handle.try_record_identity = () => {
        if (worker.phase === 'closed') throw Error('closed worker must not accept identity');
        return undefined;
      };
      h.mirror.engine.whenProgress = (_seen, timeout) => {
        assert.equal(timeout, 16);return waitForEngineProgress(new Int32Array(new SharedArrayBuffer(4)), false, 0, timeout);
      };
      setTimeout(() => {
        assert.equal(h.mirror.engine.progressSeen(), seen);assert.equal(h.barriers.at(-1), true);
        worker.closing = true;worker.phase = 'closed';worker.disposed++;
      }, 0);
    }
  });
  const before = JSON.stringify(h.report), stamp = h.authority.stamp;
  await assert.rejects(h.route.setFPS(31), /closed before host acceptance/);
  assert.equal(JSON.stringify(h.report), before);assert.equal(h.authority.stamp, stamp);
  assert.equal(h.worker.submitted, 1);assert.equal(h.worker.committed, 1);
  assert.equal(h.worker.disposed, 1);assert.equal(h.worker.terminalCount, 0);
  assert.equal(h.publications.length, 0);assert.equal(h.worker.phase, 'closed');
  assert.deepEqual(h.barriers, [true, false]);
  await assert.rejects(h.route.setFPS(31), /closed/);
});

test('terminal ACK Busy can release without progress and never resubmits the committed command', async () => {
  const h = host(), terminal = h.worker.terminal, wait = h.mirror.engine.whenProgress;
  let busy = true, attempts = 0;
  h.worker.terminal = function (id) {
    attempts++;
    if (busy) { setTimeout(() => { busy = false; }, 0);return false; }
    return terminal.call(this, id);
  };
  h.mirror.engine.whenProgress = function (...args) {
    if (h.worker.closing) return wait.apply(this, args);
    if (h.worker.phase === 'success' && busy) {
      assert.equal(args[1], 16);return waitForEngineProgress(new Int32Array(new SharedArrayBuffer(4)), false, 0, args[1]);
    }
    return wait.apply(this, args);
  };
  assert.equal(await h.route.setFPS(31), true);
  assert.equal(attempts, 2);assert.equal(h.worker.terminalCount, 1);
  assert.equal(h.worker.submitted, 1);assert.equal(h.worker.committed, 1);
  assert.equal(h.publications.length, 1);assert.equal(h.worker.phase, 'idle');
  assert.deepEqual(h.barriers, [true, false]);
});


test('abort during Ready Busy wait never ACKs readiness or commits after cancellation', async () => {
  const controller = new AbortController(), h = host();
  const before = JSON.stringify(h.report), stamp = h.authority.stamp, frameRevision = h.mirror.framesRevision;
  const ready = h.worker.ready, wait = h.mirror.engine.whenProgress;
  let attempts = 0, acknowledged = 0, waits = 0;
  h.worker.ready = function (...args) {
    attempts++;
    if (attempts === 1) return false;
    acknowledged++;return ready.apply(this, args);
  };
  h.mirror.engine.whenProgress = function (...args) {
    if (h.worker.phase === 'ready' && attempts === 1 && waits++ === 0) {
      assert.equal(args[1], 16);controller.abort();
    }
    return wait.apply(this, args);
  };
  await assert.rejects(h.route.setFPS(31, {signal: controller.signal}), {name: 'AbortError'});
  assert.equal(attempts, 1);assert.equal(acknowledged, 0);
  assert.equal(h.worker.submitted, 1);assert.equal(h.worker.committed, 0);
  assert.equal(JSON.stringify(h.report), before);assert.equal(h.authority.stamp, stamp);
  assert.equal(h.mirror.framesRevision, frameRevision);assert.equal(h.editor.historySize, 0);
  assert.equal(h.worker.cancelled, true);assert.equal(h.worker.phase, 'idle');
  assert.equal(h.publications.length, 0);assert.deepEqual(h.barriers, [true, false]);
});
