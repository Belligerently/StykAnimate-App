import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {prepareFreshEditWorker} from '../src/engine/engine-session-worker.mjs';
import {open} from '../engine/tools/fixture-kit.mjs';

function fixture(mode = 'native', family = 'fresh') {
  const value = JSON.parse(readFileSync(new URL(`../engine/tests/session-handoff/${mode}-${family}.json`, import.meta.url)));
  const editor = open(JSON.parse(value.steps[0].document), value.options);
  for (const step of value.steps.slice(1)) {
    if (step.boundary) break;
    if (step.call) editor[step.call[0]](...step.call.slice(1));
  }
  return editor;
}
const owner = editor => ({report: editor.report, document: JSON.stringify(editor.report), revision: editor.revision,
  serial: editor._revisionSerial, saved: editor._savedRevision, size: editor.historySize,
  undo: JSON.stringify(editor._undo), redo: JSON.stringify(editor._redo)});
function sideFor(editor, onProgress = () => {}) {
  let progress = 0, native = null, calls = 0, state, values;
  const identity = [5, 4, editor.revision], epochs = [71, 4, 0];
  const side = {mirror: {framesCurrent: (report, revision) => report === editor.report && revision === editor.revision},
    engine: {threaded: true, progressSeen: () => progress,
      async whenProgress() { progress++; native?.advance(); onProgress(progress, native); },
      handle: {try_record_identity: () => identity,
        record_identity() { throw Error('bootstrap must not take the spinning identity read'); },
        request_edit_worker(given, numbers) {
          calls++; state = JSON.parse(given); values = [...numbers];
          native = {phase: 'preparing', closing: false, activating: false, freed: 0, activated: 0,
            advance() { if (this.closing) this.phase = 'closed'; else if (this.activating) { this.phase = 'idle'; this.activated++; } else this.phase = 'attached'; },
            poll() { return JSON.stringify({request: this.phase === 'idle' ? 0 : 1, phase: this.phase,
              ...(this.phase === 'attached' ? {stamp: [...identity, 0], epochs} : {})}); },
            activate(request, givenEpochs) { assert.equal(request, 1);assert.deepEqual([...givenEpochs], epochs);this.activating = true;return true; },
            close() { this.closing = true; }, free() { this.freed++; }};
          return native;
        }}}};
  return {side, epochs: () => epochs, get native() { return native; }, get calls() { return calls; }, get state() { return state; }, get values() { return values; }};
}

test('actual native and sheet fresh counters reach bounded worker init and explicit writer-transfer ACK', async () => {
  for (const mode of ['native', 'sheet']) {
    const editor = fixture(mode), before = owner(editor), host = sideFor(editor);
    const prepared = await prepareFreshEditWorker(host.side, editor, {epochs: host.epochs});
    assert.deepEqual(owner(editor), before);
    assert.equal(host.native.phase, 'attached');
    assert.equal(host.native.activated, 0);
    assert.deepEqual(host.values.slice(7, 10), [editor.revision, editor._revisionSerial, editor._savedRevision]);
    assert.ok(editor.revision > 0);
    assert.deepEqual(host.state.selectedUids, [...editor.selectedUids]);
    assert.equal(host.state.layerTimeline, mode === 'sheet');
    let accepted = 0, rolledBack = 0;
    const authority = await prepared.activate(() => ({accept() { accepted++; }, rollback() { rolledBack++; }}));
    assert.equal(accepted, 1);assert.equal(rolledBack, 0);assert.equal(host.native.activated, 1);
    assert.equal(authority.worker, host.native);
    assert.deepEqual(authority.stamp, [5, 4, editor.revision, 0]);
    assert.deepEqual(owner(editor), before);
    await assert.rejects(prepared.cancel(), /canonical owner must be retained/);
    await assert.rejects(prepared.activate(() => {}), /already consumed/);
  }
});

test('busy identity or canonical slot preserves fresh owner and exposes typed retry without fallback', async () => {
  for (const at of ['identity', 'admission', 'during', 'activation']) {
    const editor = fixture(), before = owner(editor), host = sideFor(editor);
    if (at === 'identity') host.side.engine.handle.try_record_identity = () => undefined;
    if (at === 'admission') host.side.engine.handle.request_edit_worker = () => undefined;
    let reads = 0;
    if (at === 'during') {
      const read = host.side.engine.handle.try_record_identity;
      host.side.engine.handle.try_record_identity = () => ++reads >= 2 ? undefined : read();
    }
    let accepted = 0, rolledBack = 0;
    if (at === 'activation') {
      const prepared = await prepareFreshEditWorker(host.side, editor, {epochs: host.epochs});
      await assert.rejects(prepared.activate(() => {
        host.side.engine.handle.try_record_identity = () => undefined;
        return {accept() { accepted++; }, rollback() { assert.equal(host.native.phase, 'closed'); rolledBack++; }};
      }), {code: 'engine_busy'});
      assert.equal(rolledBack, 1);
    } else await assert.rejects(prepareFreshEditWorker(host.side, editor, {epochs: host.epochs}), {code: 'engine_busy'});
    assert.deepEqual(owner(editor), before);
    assert.equal(accepted, 0);
    if (at === 'identity' || at === 'admission') assert.equal(host.native, null);
    else { assert.equal(host.native.phase, 'closed'); assert.equal(host.native.freed, 1); assert.equal(host.native.activated, 0); }
  }
});

test('all recorded foreign warm owners refuse before selected-ID reads, history export or init', async () => {
  for (const mode of ['native', 'sheet']) for (const family of ['undo', 'redo', 'saved', 'trimmed', 'pose']) {
    const editor = fixture(mode, family), before = owner(editor), host = sideFor(editor);
    Object.defineProperty(editor, 'selectedUids', {get() { throw Error('must not read selection after warm refusal'); }});
    await assert.rejects(prepareFreshEditWorker(host.side, editor, {epochs: host.epochs}), /warm JavaScript history/);
    assert.equal(host.calls, 0);
    assert.deepEqual(owner(editor), before);
  }
});

test('abort before/during init disposes on worker and preserves actual original JS owner', async () => {
  for (const when of ['before', 'during']) {
    const editor = fixture('sheet'), before = owner(editor), controller = new AbortController();
    const host = sideFor(editor, progress => { if (when === 'during' && progress === 1) controller.abort(); });
    if (when === 'before') controller.abort();
    await assert.rejects(prepareFreshEditWorker(host.side, editor, {epochs: host.epochs, signal: controller.signal}), {name: 'AbortError'});
    assert.deepEqual(owner(editor), before);
    if (when === 'before') assert.equal(host.calls, 0);
    else { assert.equal(host.native.phase, 'closed');assert.equal(host.native.freed, 1);assert.equal(host.native.activated, 0); }
  }
});

test('selection drift without revision or event-epoch movement refuses unchanged newer selection', async () => {
  const editor = fixture(), host = sideFor(editor, progress => { if (progress === 1) editor.selectMany([]); });
  const originalRevision = editor.revision;
  await assert.rejects(prepareFreshEditWorker(host.side, editor, {epochs: host.epochs}), /original editor or actual shown head changed/);
  assert.equal(editor.revision, originalRevision);
  assert.equal(editor.selectedUid, null);assert.deepEqual([...editor.selectedUids], []);
  assert.equal(editor.historySize, 0);
  assert.equal(host.native.phase, 'closed');assert.equal(host.native.activated, 0);
});

test('mutable host epoch tuples cannot erase a stale shown-head handoff guard', async () => {
  let tuple;
  const editor = fixture(), before = owner(editor), host = sideFor(editor, progress => { if (progress === 1) tuple[0]++; });
  tuple = host.epochs();
  await assert.rejects(prepareFreshEditWorker(host.side, editor, {epochs: host.epochs}), /original editor or actual shown head changed/);
  assert.deepEqual(owner(editor), before);
  assert.equal(host.native.phase, 'closed');assert.equal(host.native.activated, 0);
});

test('activation freezes writer before revalidation and rollback waits for worker disposal', async () => {
  const editor = fixture(), host = sideFor(editor);
  const prepared = await prepareFreshEditWorker(host.side, editor, {epochs: host.epochs});
  let accepted = 0, rolledBack = 0;
  await assert.rejects(prepared.activate(() => {
    editor.selectMany([]);
    return {accept() { accepted++; }, rollback() { assert.equal(host.native.phase, 'closed');rolledBack++; }};
  }), /original editor or actual shown head changed/);
  assert.equal(accepted, 0);assert.equal(rolledBack, 1);assert.equal(host.native.activated, 0);
  assert.equal(editor.selectedUid, null);assert.equal(editor.canUndo, false);
});

test('abort after native activation but before authority return cannot restore JS until worker closed', async () => {
  const editor = fixture(), before = owner(editor), controller = new AbortController();
  const host = sideFor(editor, (progress, native) => { if (native?.activated && progress === 2) controller.abort(); });
  const prepared = await prepareFreshEditWorker(host.side, editor, {epochs: host.epochs, signal: controller.signal});
  let accepted = 0, rolledBack = 0;
  await assert.rejects(prepared.activate(() => ({accept() { accepted++; }, rollback() { assert.equal(host.native.phase, 'closed');rolledBack++; }})), {name: 'AbortError'});
  assert.equal(accepted, 0);assert.equal(rolledBack, 1);
  assert.equal(host.native.phase, 'closed');assert.equal(host.native.freed, 1);
  assert.deepEqual(owner(editor), before);
});

test('control budget, optional metadata and missing worker readiness refuse before serialization/init', async () => {
  for (const kind of ['selection', 'optional', 'threads', 'mirror']) {
    const editor = fixture(), host = sideFor(editor);
    if (kind === 'selection') {
      editor.selectedUids = new Set(Array.from({length: 1025}, (_, i) => i + 1));
      editor.selectedUids[Symbol.iterator] = () => { throw Error('unbounded selected-ID iteration'); };
    } else if (kind === 'optional') editor._polygonSelection = {toJSON() { throw Error('unbounded optional metadata'); }};
    else if (kind === 'threads') host.side.engine.threaded = false;
    else host.side.mirror.framesCurrent = () => false;
    await assert.rejects(prepareFreshEditWorker(host.side, editor, {epochs: host.epochs}), /control budget|Selection metadata|worker threads|acknowledged/);
    assert.equal(host.calls, 0);
  }
});

test('actual batch owner refuses and readonly bootstrap never normalizes hidden legacy UID', async () => {
  const editor = fixture(), host = sideFor(editor);
  const refusal = editor.movieclipBatch('Pending fresh handoff', () => prepareFreshEditWorker(host.side, editor, {epochs: host.epochs}));
  await assert.rejects(refusal, /live editor requires/);
  assert.equal(host.calls, 0);
  editor._selectedUid = 9999;
  const prepared = await prepareFreshEditWorker(host.side, editor, {epochs: host.epochs});
  assert.equal(host.state.selectedUid, 1);
  assert.equal(editor._selectedUid, 9999);
  await prepared.cancel();
  assert.equal(editor._selectedUid, 9999);
  assert.equal(host.native.phase, 'closed');
});

test('actual copied clipboard survives empty-history load and handoff refusal without export', async () => {
  for (const mode of ['native', 'sheet']) {
    const editor = fixture(mode);
    editor.copySelection();
    const clipboard = editor._selectionClipboard;
    assert.ok(clipboard);
    assert.equal(editor.historySize, 0);assert.equal(editor.canUndo, false);
    editor.load(structuredClone(editor.report));
    assert.equal(editor._selectionClipboard, clipboard, 'actual load retains its clipboard owner');
    const before = owner(editor), host = sideFor(editor);
    clipboard.toJSON = () => { throw Error('must not export a foreign clipboard'); };
    await assert.rejects(prepareFreshEditWorker(host.side, editor, {epochs: host.epochs}), /Existing clipboards require/);
    assert.equal(host.calls, 0);assert.equal(editor._selectionClipboard, clipboard);
    assert.deepEqual(owner(editor), before);
  }
});

test('retained display payloads refuse a nominally empty history without export', async () => {
  for (const key of ['_authoredCommitPreview', '_sheetPropertyPreview']) {
    const editor = fixture('sheet'), host = sideFor(editor);
    const retained = {toJSON() { throw Error('must not export an existing display'); }};
    editor[key] = retained;
    await assert.rejects(prepareFreshEditWorker(host.side, editor, {epochs: host.epochs}), /retained display/);
    assert.equal(host.calls, 0);assert.equal(editor[key], retained);assert.equal(editor.historySize, 0);
  }
});
