import test from 'node:test';
import assert from 'node:assert/strict';
import {emptyDocument, createFrame, createObject} from '../src/demo.mjs';
import {planFigureFrameOperation as plan} from '../src/range-figure-operations.mjs';
import {applyLayerOrder} from '../src/layers.mjs';

test('numeric ordering resolves mixed depths and ties without relying on changed pose order', () => {
  const report = emptyDocument();
  for (let pattern = 0; pattern < 81; pattern++) {
    const objects = [1, 2, 3, 4].map((uid, index) => ({...createObject(report.figure_types[0], uid, 200, 200),
      _stykZIndex: Math.floor(pattern / 3 ** index) % 3 - 1}));
    const native = createFrame(objects, 10), evaluated = applyLayerOrder(report, native);
    for (let mask = 1; mask < 16; mask++) for (const direction of ['front', 'back', 1, -1]) {
      const selection = objects.filter((_, index) => mask & 1 << index).map(o => o.unique_id);
      const changed = plan(report, evaluated, selection, {kind: 'order', direction, numericOrder: true});
      if (!changed) continue;
      const actual = applyLayerOrder(report, {...native, objects: changed.frame.objects});
      assert.deepEqual(actual.object_draw_order, changed.frame.object_draw_order,
        `depth pattern ${pattern}, selection ${mask}, direction ${direction}`);
    }
  }
});

