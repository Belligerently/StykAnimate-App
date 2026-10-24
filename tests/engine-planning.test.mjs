import {readFileSync, readdirSync} from 'node:fs';
import test from 'node:test';
import assert from 'node:assert/strict';
import {planEngineInbetweens, planOnInterface, isUnported} from '../src/engine/engine-planning.mjs';
import {createStagePlayback} from '../src/engine/engine-playback.mjs';
import {documentChunks} from '../src/document-transport.mjs';
import {Editor} from '../src/editor.mjs';
const nativeJson = value => JSON.stringify(value, (key,value) => key === 'runtime_segments' ? undefined : value);
const inbetweenFolder = new URL('../engine/tests/inbetween/', import.meta.url);
const inbetween = name => JSON.parse(readFileSync(new URL(`${name}.json`, inbetweenFolder), 'utf8'));
const inbetweenNames = readdirSync(inbetweenFolder).map(file => file.replace(/\.json$/, '')).sort();
const inbetweenPlans = inbetweenNames.filter(name => inbetween(name).payload), inbetweenRefusals = inbetweenNames.filter(name => inbetween(name).failure);
const loaded = reference => new Editor({demo: false}).load(JSON.parse(reference.document));
/** An engine whose in-between job sends `payload` the way the worker does: one packet per ACK. */
function inbetweenEngine(payload, requests = []) {
  const chunks = documentChunks(payload, {maxBytes: 64 * 1024, maxRecords: 128, timeBudgetMs: 2});
  const engine = {threaded: true, workers: 1, progressSeen: () => 0, whenProgress: async () => {}, handle: {plan_inbetweens(revision, request) {
    assert.equal(typeof revision, 'number'); requests.push(JSON.parse(request));
    let sequence = 0, awaiting;
    return {take() {
      if (awaiting !== undefined) return undefined;
      const next = chunks.next();
      if (next.done) return JSON.stringify({done: true});
      awaiting = sequence++; return JSON.stringify({sequence: awaiting, chunk: next.value, progress: {phase: 'sampling', completed: 1, total: 1}});
    }, ack(seq) { assert.equal(seq, awaiting); awaiting = undefined; return true; }, cancel() {}, free() {}};
  }}};
  return engine;
}

test('the in-between fixtures cover native, authored and refused plans', () => {
  assert.ok(inbetweenPlans.length >= 18); assert.equal(inbetweenRefusals.length, 4);
});
for (const name of inbetweenPlans) test(`engine in-between plan reaches the actual async editor publication and history: ${name}`, async () => {
  const reference = inbetween(name), editor = loaded(reference), requests = [];
  editor._restoreSelection(reference.initialState.selection); editor._layerRevision = reference.initialState.layerRevision;
  const engine = inbetweenEngine(reference.payload, requests), progress = [], events = [];
  editor.addEventListener('change', event => events.push(event.detail));
  const planInbetweens = (request, options) => planEngineInbetweens(engine, editor.revision, request, options);
  const {ratio, indices} = reference.command.args;
  const result = await editor.insertInbetweenFramesAsync(ratio, indices, {planInbetweens, onProgress: value => progress.push(value)});
  assert.deepEqual(requests, [reference.request], 'only the ratio, the ordered keys and the identity cross');
  assert.ok(progress.some(value => value.phase === 'sampling'));
  assert.deepEqual(result, reference.command.result); assert.equal(nativeJson(editor.report), reference.command.after);
  assert.deepEqual(events, reference.command.events); assert.equal(editor.undoLabel, reference.command.state.undoLabel);
  editor.undo(); assert.equal(nativeJson(editor.report), reference.command.undone);
  editor.redo(); assert.equal(nativeJson(editor.report), reference.command.redone);
});
for (const name of inbetweenPlans) test(`the interface plans the same in-between wire as the recorded engine plan: ${name}`, async () => {
  const reference = inbetween(name), editor = loaded(reference), progress = [];
  const wire = await planOnInterface('inbetweens', editor.report, reference.request, {onProgress: value => progress.push(value)});
  assert.equal(nativeJson(wire), nativeJson(reference.payload.plan)); assert.ok(progress.some(value => value.phase === 'sampling'));
});
for (const name of inbetweenRefusals) test(`the interface and the engine adapter refuse an in-between as the editor words it: ${name}`, async () => {
  const reference = inbetween(name), editor = loaded(reference), key = reference.failure.messageKey;
  await assert.rejects(planOnInterface('inbetweens', editor.report, reference.request), error => error.messageKey === key && error.message === reference.failure.message);
  const engine = {threaded: true, workers: 1, progressSeen: () => 0, whenProgress: async () => {}, handle: {plan_inbetweens: () => ({
    take() { throw new Error(key); }, cancel() {}, free() {}})}};
  await assert.rejects(planEngineInbetweens(engine, 1, reference.request), error => error.messageKey === key && error.message === reference.failure.message);
});
test('a retired in-between job reads as the editor words it, and an engine that cannot plan says so', async () => {
  const request = {ratio: .5, indices: [1], nextFrameIdentity: 4};
  const engineOf = (take, extra = {}) => ({threaded: true, workers: 1, progressSeen: () => 0, whenProgress: async () => {}, handle: {plan_inbetweens: () => ({take, cancel() {}, free() {}}), ...extra}});
  const retired = Object.assign(new Error('The document changed while the plan was prepared.'), {name: 'AbortError'});
  await assert.rejects(planEngineInbetweens(engineOf(() => { throw retired; }), 1, request), error => error.name === 'AbortError' && /animation changed while inbetweens/.test(error.message));
  const controller = new AbortController();
  await assert.rejects(planEngineInbetweens(engineOf(() => { controller.abort(); throw retired; }), 1, request, {signal: controller.signal}), error => error.name === 'AbortError' && /cancelled/.test(error.message));
  await assert.rejects(planEngineInbetweens(engineOf(() => { throw Object.assign(new Error('a layer sheet is not ported'), {name: 'EngineUnported'}); }), 1, request), isUnported);
  await assert.rejects(planEngineInbetweens({...engineOf(() => undefined), threaded: false}, 1, request), isUnported);
  // A refusal that names a value carries it after the key; the editor's error holds both.
  await assert.rejects(planEngineInbetweens(engineOf(() => { throw new Error('errors.editorLayerSheet.unlockBeforeEditingIt\u001f{"name":"Destination"}'); }), 1, request),
    error => error.messageKey === 'errors.editorLayerSheet.unlockBeforeEditingIt' && error.messageParams.name === 'Destination' && error.message === 'Unlock Destination before editing it.');
  await assert.rejects(planEngineInbetweens(engineOf(() => undefined), 1, {...request, ratio: 2}), error => error.messageKey === 'errors.editorInbetweenAsync.chooseAnInterpolationRatioFromZeroToOne');
  await assert.rejects(planEngineInbetweens(engineOf(() => undefined), 1, {...request, indices: [-1]}), /Invalid inbetween planning request/);
  // The worker that still disposes the previous job answers busy: the job starts once it has.
  let attempts = 0;
  const busy = engineOf(() => JSON.stringify({done: true}), {plan_inbetweens() { if (++attempts < 3) throw new Error('a planner is busy; wait for worker disposal'); return {take: () => JSON.stringify({done: true}), cancel() {}, free() {}}; }});
  await assert.rejects(planEngineInbetweens(busy, 1, request), /invalid inbetween planning payload/);
  assert.equal(attempts, 3);
});

function routedPlayback({threaded = true, plan = null, refuse = null} = {}) {
  let published = 0; const calls = [];
  const engine = {threaded, workers: 1, progressSeen: () => 0, whenProgress: async () => {}, pump() {}, handle: {
    try_set_hidden: () => 1, try_set_effects: () => 1, generation: () => 1, try_document_identity: () => [1, 1, published],
    plan_inbetweens(revision, request) { calls.push({revision, request: JSON.parse(request)}); if (refuse) throw refuse; return plan(); }}};
  const side = {engine};
  const playback = createStagePlayback({stage: () => ({engineSide: side})}), load = playback.load;
  // The engine's session has published the revision the playback is loaded at.
  playback.load = (next, options) => { published = options?.revision ?? published; return load(next, options); };
  return {playback, calls};
}
const planJob = payload => () => {
  const chunks = documentChunks(payload, {maxBytes: 64 * 1024, maxRecords: 128, timeBudgetMs: 2}); let sequence = 0, awaiting;
  return {take() { if (awaiting !== undefined) return undefined; const next = chunks.next(); if (next.done) return JSON.stringify({done: true}); awaiting = sequence++; return JSON.stringify({sequence: awaiting, chunk: next.value}); },
    ack() { awaiting = undefined; return true; }, cancel() {}, free() {}};
};

test('the stage playback plans on the engine for the document it holds', async () => {
  const reference = inbetween('authored-independent'), editor = loaded(reference);
  const {playback, calls} = routedPlayback({plan: planJob(reference.payload)});
  playback.load(editor.report, {revision: 7});
  const wire = await playback.planInsertInbetweens(reference.request, {isCurrent: () => true});
  assert.equal(nativeJson(wire), nativeJson(reference.payload.plan));
  assert.deepEqual(calls, [{revision: 7, request: reference.request}]);
});
test('what the engine does not plan, a host without worker threads and a stale document are planned or refused on the interface', async () => {
  const reference = inbetween('authored-easing'), editor = loaded(reference);
  const unported = routedPlayback({refuse: Object.assign(new Error('a layer sheet is not ported'), {name: 'EngineUnported'})});
  unported.playback.load(editor.report, {revision: 1});
  assert.equal(nativeJson(await unported.playback.planInsertInbetweens(reference.request, {})), nativeJson(reference.payload.plan));
  assert.equal(unported.calls.length, 1, 'the engine was asked first');
  const plain = routedPlayback({threaded: false});
  plain.playback.load(editor.report, {revision: 1});
  assert.equal(nativeJson(await plain.playback.planInsertInbetweens(reference.request, {})), nativeJson(reference.payload.plan));
  assert.equal(plain.calls.length, 0);
  const stale = routedPlayback({threaded: false});
  stale.playback.load(editor.report, {revision: 1});
  await assert.rejects(stale.playback.planInsertInbetweens(reference.request, {isCurrent: () => false}), {name: 'AbortError'});
  const controller = new AbortController(); controller.abort();
  await assert.rejects(stale.playback.planInsertInbetweens(reference.request, {signal: controller.signal}), {name: 'AbortError'});
  // A refusal is the user's, not a reason to plan again.
  const refused = routedPlayback({refuse: new Error('errors.editorInbetween.thisInbetweenNeedsMoreThanTwoBackgroundImages')});
  refused.playback.load(editor.report, {revision: 1});
  await assert.rejects(refused.playback.planInsertInbetweens(reference.request, {}), error => error.messageKey === 'errors.editorInbetween.thisInbetweenNeedsMoreThanTwoBackgroundImages');
});
