// The layer-move planning job through the real WebAssembly module (npm run engine:build), in Node:
// the request and poll boundary of styk-wasm (JSON request in, bounded packets out, errors by
// name), not only the Rust planners the cargo tests compare. Node has no engine worker threads,
// so the test pumps the engine's queue itself (the app refuses such a host: D7).
import {readFileSync} from 'node:fs';
import test from 'node:test';
import assert from 'node:assert/strict';
import {Editor} from '../src/editor.mjs';
import {applyDocumentChunk} from '../src/document-transport.mjs';
import {engineBuilt, nodeEngine} from './support/engine-node.mjs';
import {openingSession} from '../src/engine/engine-owner.mjs';

const skip = engineBuilt ? false : 'the engine is not built (npm run engine:build)';
const nativeJson = value => JSON.stringify(value, (key, item) => key === 'runtime_segments' ? undefined : item);
const read = path => JSON.parse(readFileSync(new URL(`../engine/tests/${path}.json`, import.meta.url), 'utf8'));

/** The engine holding `reference`'s document, as the app's session leaves it at `revision`. */
async function engineHolding(reference, options, revision = 5) {
  const {engine} = await nodeEngine();
  const editor = new Editor({demo: false, ...options}).load(JSON.parse(reference.document));
  const session = openingSession(editor);
  session.activate(revision);
  assert.equal(engine.handle.revision(), revision, 'the engine holds the document');
  return {engine, revision, session};
}
/** Runs a job to its end: every packet acknowledged, progress kept. */
async function drive(engine, job) {
  let payload; const progress = [];
  try {
    for (let spins = 0; spins < 200_000; spins++) {
      engine.pump();
      const text = job.take();
      if (text === undefined || text === null) { await new Promise(resolve => setTimeout(resolve, 0)); continue; }
      const packet = JSON.parse(text);
      if (packet.progress) progress.push(packet.progress);
      if (packet.chunk) { payload = applyDocumentChunk(payload, packet.chunk); assert.equal(job.ack(packet.sequence), true); }
      if (packet.done) return {payload, progress};
    }
    throw new Error('The planning job did not finish.');
  } finally { job.cancel(); engine.pump(); job.free(); engine.pump(); }
}
const failure = async action => { try { await action(); } catch (error) { return error; } assert.fail('expected a planning failure'); };

test('the engine module words a stale revision as aborted and a malformed request as an error', {skip}, async () => {
  const combined = read('layer-sheet-migration/combined'), {engine, revision} = await engineHolding(combined, {layerTimeline: true, propertyChannels: false});
  const stale = await failure(async () => engine.handle.plan_layers(revision + 1, JSON.stringify(combined.request)));
  assert.equal(stale.name, 'AbortError');
  for (const bad of ['{}', JSON.stringify({...combined.request, nextFrameIdentity: 0}), JSON.stringify({...combined.request, uids: [-1]})])
    assert.equal((await failure(async () => engine.handle.plan_layers(revision, bad))).name, 'Error');
});

for (const name of ['native-only', 'independent', 'joined-scalars', 'delete-default', 'dense-single-figure']) test(`the engine module plans the layer move: ${name}`, {skip}, async () => {
  const reference = read(`layer-migration/${name}`), {engine, revision} = await engineHolding(reference);
  const {payload, progress} = await drive(engine, engine.handle.plan_layers(revision, JSON.stringify(reference.request)));
  assert.equal(nativeJson(payload), nativeJson(reference.payload));
  for (const step of progress) assert.deepEqual(Object.keys(step).sort(), ['completed', 'objectPoses', 'phase', 'segmentValues', 'sourceFrames', 'total']);
});
test('the engine module plans a layer move on a layer sheet and words a refusal by its key', {skip}, async () => {
  const combined = read('layer-sheet-migration/combined'), options = {layerTimeline: true, propertyChannels: false};
  const {engine, revision} = await engineHolding(combined, options);
  const {payload} = await drive(engine, engine.handle.plan_layers(revision, JSON.stringify(combined.request)));
  assert.equal(nativeJson(payload), nativeJson(combined.payload));
  const missing = read('layer-sheet-migration/missing-layer'), held = await engineHolding(missing, options);
  const error = await failure(async () => { await drive(held.engine, held.engine.handle.plan_layers(held.revision, JSON.stringify(missing.request))); });
  assert.equal(error.message, missing.failure.messageKey);
});
