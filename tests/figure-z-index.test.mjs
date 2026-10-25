import test from 'node:test';
import assert from 'node:assert/strict';
import {plainEditorFor} from './support/plain-editor.mjs';
import {emptyDocument, createFrame, createObject} from '../src/demo.mjs';
import {composeAnimationChannels, validateAnimationDocument} from '../src/animation-channels.mjs';
import {evaluatedFrame} from '../src/pivot/geometry.mjs';
import {timeline} from '../src/pivot/read-view.mjs';
import {pivotDocument} from './support/engine-export.mjs';

const target = {kind: 'figure', uid: 1};
/** `edit(report)` changes the document before the engine opens it. */
function fixture(edit = null) {
  const report = emptyDocument(), figure = report.figure_types[0];
  report.keyframes = [0, 100, 0].map(x => createFrame([1, 2, 3].map(uid => createObject(figure, uid, x, 200)), 10));
  edit?.(report);
  return plainEditorFor({demo: false}, report);
}
function sample(report, tick) {
  const row = timeline(report).findLast(row => row.start <= tick);
  return evaluatedFrame(report, row.index, row.count ? (tick - row.start) / row.count : 0, tick);
}
const order = frame => frame.object_draw_order.map(slot => frame.objects[slot].unique_id);

test('Z-index is a sparse held channel; ties retain pose order and source objects remain intact', () => {
  const e = fixture(), before = structuredClone(e.report.keyframes);
  e.addAnimationKey(target, 'zIndex', 5, 10);
  e.addAnimationKey(target, 'zIndex', 10, -10);
  assert.deepEqual(order(sample(e.report, 4)), [1, 2, 3]);
  assert.deepEqual(order(sample(e.report, 9.999)), [2, 3, 1]);
  assert.deepEqual(order(sample(e.report, 10)), [1, 2, 3]);
  assert.equal(e.animationChannel(target, 'zIndex').keys[0].interpolation, 'constant');
  assert.deepEqual(e.report.keyframes, before);
  const base = e.report.keyframes[0];
  const composed = composeAnimationChannels(e.report, base, 6);
  assert.equal(composed.objects[1], base.objects[1]);
  assert.equal(composed.objects[0].angles, base.objects[0].angles);
  e.addAnimationKey({kind: 'figure', uid: 2}, 'zIndex', 5, 10);
  assert.deepEqual(order(sample(e.report, 5)), [3, 1, 2]);
});

test('Z-index respects layer boundaries and never renumbers attachment slots', () => {
  const e = fixture(report => { for (const frame of report.keyframes) frame.objects[1].join = {target_object_slot: 0, target_unique_id: 1, target_handle_id: 1}; }), layer = e.createLayer('Front');
  const root = {...e.geometry.objects.get(1).root};
  e.assignFiguresToLayer([3], layer.id);
  e.addAnimationKey(target, 'zIndex', 0, 1000000);
  assert.deepEqual(order(e.evaluatedFrame), [2, 1, 3]);
  assert.deepEqual(e.evaluatedFrame.objects.map(o => o.unique_id), [1, 2, 3]);
  assert.equal(e.evaluatedFrame.objects[1].join.target_object_slot, 0);
  assert.deepEqual(e.geometry.objects.get(1).root, root);
});

test('invalid depth and continuous easing reject atomically, including persisted documents', () => {
  const e = fixture();
  for (const value of [NaN, Infinity, 1.5, 1000001, -1000001]) assert.throws(() => e.addAnimationKey(target, 'zIndex', 0, value));
  assert.equal(e.animationChannels.length, 0);
  const key = e.addAnimationKey(target, 'zIndex', 0, 1), channel = e.animationChannel(target, 'zIndex');
  const before = e.report._styk.animation;
  assert.throws(() => e.setAnimationKey(channel.id, key.id, {interpolation: 'linear'}), /Constant/);
  assert.equal(e.report._styk.animation, before);
  const report = structuredClone(e.serialize()); report._styk.animation.channels[0].keys[0].interpolation = 'linear';
  assert.throws(() => validateAnimationDocument(report), /Constant/);
});

test('depth is identical in materialized Pivot output', () => {
  const e = fixture(); e.addAnimationKey(target, 'zIndex', 0, 2); e.addAnimationKey(target, 'zIndex', 10, -2);
  const exported = pivotDocument(e.serialize());
  for (const tick of [0, 5, 9, 10, 20]) {
    const expected = order(sample(e.report, tick));
    assert.deepEqual(order(sample(exported, tick)), expected);
  }
  assert.ok(exported.keyframes.every(frame => frame.objects.every(object => object._stykZIndex === undefined)));
});

