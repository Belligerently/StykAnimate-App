import test from 'node:test';
import assert from 'node:assert/strict';
import { emptyDocument, createFrame, createObject } from '../src/demo.mjs';
import {buildVirtualPoseTrack} from '../src/animation-pose.mjs';

function fixture() {
  const report = emptyDocument(), figure = report.figure_types[0];
  report._styk.layers = { order: ['layer-1', 'layer-2'], defaultLayerId: 'layer-1', assignments: { 1: 'layer-1', 2: 'layer-2' } };
  report.keyframes = [0, 100, 0].map((x, index) => ({
    ...createFrame([createObject(figure, 1, x, 200), createObject(figure, 2, 10 + index * 10, 300)], 10),
    _styk: { frameId: `f${index}` }, camera: { is_default: false, x: index * 20, y: 0, scale: 1, angle_rad: 0 },
  }));
  return report;
}
const channel = (target, keys) => ({ id: `${target.kind}:${target.id || ''}:pose`, target, property: 'pose',
  keys: keys.map(([tick, frameId], i) => ({ id: `k${i}`, tick, value: frameId === null ? null : { frameId }, interpolation: 'linear' })) });

test('pose wrappers share native object storage and preserve source-specific unknown data', () => {
  const report = fixture(); report.keyframes[1].unknown_native = { value: 17 };
  const track = buildVirtualPoseTrack(report, channel({ kind: 'layer', id: 'layer-1' }, [[0, 'f0'], [30, 'f1']]));
  assert.equal(track.frames[0].objects, report.keyframes[0].objects);
  assert.equal(track.frames[1].unknown_native, report.keyframes[1].unknown_native);
  assert.equal(track.frames[0].duration_raw, 30);
  assert.equal(report.keyframes[0].duration_raw, 10);
});

