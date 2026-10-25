import test from 'node:test';
import assert from 'node:assert/strict';
import {emptyDocument, createFrame, createObject} from '../src/demo.mjs';
import {validateAnimationDocument} from '../src/animation-channels.mjs';
import {applyLayerOrder} from '../src/layers.mjs';
import {plainScope} from './support/plain-editor.mjs';

const open = plainScope();

function fixture(propertyChannels = true) {
  const report = emptyDocument();
  report.keyframes = [0, 1, 2].map(() => createFrame([1, 2, 3, 4].map(uid => createObject(report.figure_types[0], uid, 200, 200)), 10));
  const e = open({demo: false, propertyChannels}, report);
  for (const uid of [1, 2, 3, 4]) e.adoptNativePoseTrack({kind: 'figure', uid});
  e.seek(5); return e;
}
const order = e => e.evaluatedFrame.object_draw_order_unique_ids;
const commands = [
  ['forward', e => e.moveSelectionLayer(1), [2, 1, 3, 4]],
  ['backward', e => e.moveSelectionLayer(-1), [1, 2, 4, 3]],
  ['front', e => e.bringToFront(), [2, 3, 4, 1]],
  ['back', e => e.sendToBack(), [4, 1, 2, 3]],
];
for (const propertyChannels of [false, true]) for (const [name, run, expected] of commands)
  test(`${name} works with tied depths and independent figure poses (propertyChannels=${propertyChannels})`, () => {
    const e = fixture(propertyChannels), poses = [...e.animationChannels], source = structuredClone(e.report.keyframes);
    e.select(['back', 'backward'].includes(name) ? 4 : 1);
    const history = e._undo.length; run(e);
    assert.deepEqual(order(e), expected); assert.equal(e._undo.length, history + 1);
    for (const pose of poses) assert.equal(e.animationChannel(pose.target, 'pose'), pose);
    assert.deepEqual(e.report.keyframes, source);
    assert.ok(e.animationChannels.filter(c => !poses.includes(c)).every(c => c.property === 'zIndex'));
    e.seek(4); assert.deepEqual(order(e), [1, 2, 3, 4]);
    e.seek(5); e.undo(); assert.deepEqual(order(e), [1, 2, 3, 4]);
    e.redo(); assert.deepEqual(order(e), expected);
    validateAnimationDocument(e.report);
  });

test('repeated ordering preserves selection order and becomes a no-op at the boundary', () => {
  const e = fixture(); e.selectMany([1, 3]);
  e.moveSelectionLayer(1); assert.deepEqual(order(e), [2, 1, 4, 3]);
  e.bringToFront(); assert.deepEqual(order(e), [2, 4, 1, 3]);
  const history = e._undo.length; e.bringToFront(); e.moveSelectionLayer(1);
  assert.equal(e._undo.length, history); assert.deepEqual([...e.selectedUids], [1, 3]);
  e.sendToBack(); assert.deepEqual(order(e), [1, 3, 2, 4]);
});

test('depth rebalancing retains all changed values, including stationary figures and numeric limits', () => {
  const e = fixture();
  for (const uid of [1, 2, 3, 4]) e.addAnimationKey({kind: 'figure', uid}, 'zIndex', 0, uid === 1 ? -1000000 : 1000000);
  e.select(2); e.moveSelectionLayer(1);
  assert.deepEqual(order(e), [1, 3, 2, 4]);
  const depths = e.evaluatedFrame.objects.map(o => o._stykZIndex);
  assert.ok(depths.every(value => value >= -1000000 && value <= 1000000));
  assert.equal(new Set(depths).size, 4);
  validateAnimationDocument(e.report);
});

test('multi-selection across layers reorders each layer independently', () => {
  const e = fixture(), layer = e.createLayer('Front layer'); e.assignFiguresToLayer([3, 4], layer.id);
  e.selectMany([1, 3]); e.bringToFront();
  assert.deepEqual(order(e), [2, 1, 4, 3]);
  assert.deepEqual(e.evaluatedFrame.objects.map(o => o.unique_id), [1, 2, 3, 4]);
});

test('range ordering and Simple draft discard use the same depth commands', () => {
  const e = fixture(); e.select(1); e.orderFiguresInFrames('front', [0, 1]);
  for (const tick of [0, 10]) { e.seek(tick); assert.deepEqual(order(e), [2, 3, 4, 1]); }
  e.seek(20); assert.deepEqual(order(e), [1, 2, 3, 4]);
  e.enableFrameDrafts({mode: 'update'}); e.select(1); e.bringToFront();
  assert.deepEqual(order(e), [2, 3, 4, 1]);
  e.discardFrameDraft(); assert.deepEqual(order(e), [1, 2, 3, 4]);
});
