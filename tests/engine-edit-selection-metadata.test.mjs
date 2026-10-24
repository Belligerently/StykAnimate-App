import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {Editor} from '../src/editor.mjs';
import {installEngineOwner, appSessionStart} from '../src/engine/engine-owner.mjs';
import {engineBuilt, nodeEngine} from './support/engine-node.mjs';
const input = JSON.parse(readFileSync(new URL('../engine/tests/edit/selection-metadata-plain-native.document.json', import.meta.url), 'utf8'));
const metadata = state => Object.fromEntries(['polygonSelection', 'segmentIds', 'branch'].filter(key => Object.hasOwn(state, key)).map(key => [key, state[key]]));
/** The editor owned by the real engine, started as the app starts it (from the mirror). It records the state each
 * session starts with and the view each call carries. */
async function owned(editor) {
  const {engine, mirror} = await nodeEngine();
  mirror.reset();
  await mirror.syncFrames(editor.report, editor.revision, {allFigures: true, allFrames: true, allSources: true});
  const states = [], views = [];
  const side = {failed: false, mirror, engine: {handle: {edit_session(text) {
    states.push(JSON.parse(text));
    const session = engine.handle.edit_session(text);
    return new Proxy(session, {get: (target, name) => name === 'call'
      ? (method, args, view) => { views.push(JSON.parse(view)); return target.call(method, args, view); }
      : typeof target[name] === 'function' ? target[name].bind(target) : target[name]});
  }}}};
  return {owner: installEngineOwner(editor, {start: appSessionStart(() => side)}), states, views};
}
test('a call made with optional selection open is refused and keeps the selection', {skip: !engineBuilt}, async () => {
  const editor = new Editor({demo: false}).load(structuredClone(input));editor.select(1, -1);
  const polygon = {uid: 1, index: 0, typeId: editor.figure.figure_type_id, future: {values: [7, 3]}};
  editor._restoreSelection({...editor._selectionState(), polygonSelection: polygon});
  const {owner, views} = await owned(editor);
  const refused = () => assert.throws(() => editor.locateCamera(), error => error.messageKey === 'errors.engineOwner.notSupported');
  refused();assert.deepEqual(editor._selectionState().polygonSelection, polygon);
  assert.deepEqual(metadata(views[0]), {polygonSelection: polygon}, 'the view carries the selection metadata');
  assert.equal(owner.native, 0, 'optional selection readiness stays closed');assert.equal(owner.fallback, 0, 'a refusal never runs in the editor');
  assert.deepEqual(owner.refused, {'locateCamera: locateCamera with optional selection metadata': 1});
  refused();assert.deepEqual(editor._selectionState().polygonSelection, polygon);
  assert.equal(editor.canUndo, false);
  owner.release();
});
test('ordinary views omit optional selection fields', {skip: !engineBuilt}, async () => {
  const editor = new Editor({demo: false}).load(structuredClone(input));editor.select(1, -1);
  const {owner, states, views} = await owned(editor);
  // A call the session answers on this document: the figures move as they would anywhere.
  editor.moveSelection(2, 1);assert.deepEqual(metadata(states[0]), {});assert.deepEqual(metadata(views[0]), {});
  assert.equal(Object.hasOwn(states[0], 'segmentIds'), false);assert.equal(Object.hasOwn(states[0], 'branch'), false);
  assert.deepEqual(editor._selectionState().polygonSelection ?? null, null);
  owner.release();
});
