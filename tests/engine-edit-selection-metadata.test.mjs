import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {Editor} from '../src/editor.mjs';
import {installEditShadow} from '../src/engine/engine-edit-shadow.mjs';
const input = JSON.parse(readFileSync(new URL('../engine/tests/edit/selection-metadata-plain-native.document.json', import.meta.url), 'utf8'));
const metadata = state => Object.fromEntries(['polygonSelection', 'segmentIds', 'branch'].filter(key => Object.hasOwn(state, key)).map(key => [key, state[key]]));
function shadow(editor, replies) {
  const states = [], views = [];
  const side = {failed: false, mirror: {framesCurrent: () => true}, engine: {handle: {edit_session(text) {
    states.push(JSON.parse(text));return {revision: () => editor.revision, free() {}, can_undo: () => true, can_redo: () => true,
      call(name, args, view) {views.push(JSON.parse(view));return JSON.stringify(replies(name, args, view));}};
  }}}};
  return {report: installEditShadow(editor, () => side, {mode: 'own'}), states, views};
}
function stateOf(editor, optional = {}) {
  return {tick: editor.tick, frameIndex: editor.frameIndex, selectedUid: editor.selectedUid, selectedUids: [...editor.selectedUids], selectedJoint: editor.selectedJoint,
    layerRevision: editor.layerRevision, nextFrameIdentity: editor._nextFrameIdentity, nextUid: editor._nextUid, ...optional};
}
test('adoption restores metadata-only selection changes through the existing owner', () => {
  const editor = new Editor({demo: false}).load(structuredClone(input));editor.select(1, -1);
  const polygon = {uid: 1, index: 0, typeId: editor.figure.figure_type_id, future: {values: [7, 3]}};
  let optional = {polygonSelection: polygon};
  const {report} = shadow(editor, () => ({status: 'ok', result: false, state: stateOf(editor, optional), events: [], draft: null, drag: null}));
  editor.locateCamera();assert.deepEqual(editor._selectionState().polygonSelection, polygon);assert.equal(report.owned, 1);
  optional = {};editor.locateCamera();assert.equal(report.owned, 1, 'optional selection readiness remains closed');
  assert.deepEqual(editor._selectionState().polygonSelection, polygon);
  assert.equal(editor.canUndo, false);assert.equal(report.mismatches.length, 0);
});
test('ordinary views omit optional fields and optional selection readiness stays closed', () => {
  const editor = new Editor({demo: false}).load(structuredClone(input));editor.select(1, -1);
  const polygon = {uid: 1, index: 0, typeId: editor.figure.figure_type_id, future: {values: [7, 3]}};
  const {states, views} = shadow(editor, () => ({status: 'ok', result: false, state: stateOf(editor), events: [], draft: null, drag: null}));
  editor.locateCamera();assert.deepEqual(metadata(states[0]), {});assert.deepEqual(metadata(views[0]), {});
  editor._restoreSelection({...editor._selectionState(), polygonSelection: polygon});editor.locateCamera();assert.equal(views.length, 1);
  assert.equal(Object.hasOwn(states[0], 'segmentIds'), false);assert.equal(Object.hasOwn(states[0], 'branch'), false);
});
