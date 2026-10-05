import test from 'node:test';
import assert from 'node:assert/strict';
import {captureEditBounds, MAX_BOUNDS_SAMPLES} from '../src/engine/engine-edit-bounds.mjs';
import {readFileSync} from 'node:fs';
import {Editor} from '../src/editor.mjs';
import {appBounds} from '../engine/tools/edit-bounds-callbacks.mjs';
import {installEditShadow} from '../src/engine/engine-edit-shadow.mjs';
const editor = {tick: 3, frameIndex: 0, selectedUids: new Set([7, 2])};
const entry = {obj: {unique_id: 7}};
test('bounds bridge records existing requests once, preserves callback receiver and frame range properties', () => {
  const result = {minX: -4, minY: 3, maxX: 8, maxY: 9}, receiver = {}, range = Object.assign([0], {sheetTicks: [3, 6]});
  let calls = 0;
  const callback = function (given) { calls++; assert.equal(this, receiver); assert.equal(given, entry); return result; };
  const capture = captureEditBounds('centerCamera', ['both', range, {boundsOf: callback}], editor);
  assert.equal(calls, 0); assert.equal(capture.args[2].boundsOf.call(receiver, entry), result); assert.equal(calls, 1);
  const args = JSON.parse(capture.payload()), data = args[2].boundsOf['@measuredBounds'];
  assert.deepEqual(data.input, ['both', {'@array': [0], sheetTicks: [3, 6]}]);
  assert.deepEqual(data.samples, [{uid: 7, bounds: result}]); assert.deepEqual(data.selectedUids, [7, 2]);
});

test('recorded app bounds depend on hidden construction strokes, anchored media and effect reach', () => {
  const document = JSON.parse(readFileSync(new URL('../engine/tests/edit/measured-bounds-media.document.json', import.meta.url), 'utf8'));
  const scene = new Editor({demo: false}).load(document).geometry;
  const hidden = scene.objects.get(0), media = scene.objects.get(2);
  const hiddenBox = appBounds()(hidden), visibleBox = appBounds()({...hidden, segments: hidden.segments.filter(item => item.seg.visible !== false)});
  assert.ok(hiddenBox.maxX > visibleBox.maxX + 100, 'fixture does not depend on the hidden endpoint');
  const full = appBounds()(media), noFx = appBounds({vfxBounds: false})(media);
  assert.ok(full.minX < noFx.minX && full.maxX > noFx.maxX, 'fixture does not depend on effect padding');
  const bare = {...media, segments: media.segments.map(item => item.type === 'image' ? {...item, asset: {...item.asset, display_width: 1, display_height: 1}} : item)};
  assert.notDeepEqual(appBounds()(bare), full, 'fixture does not depend on the anchored media extent');
});
test('nonplain results, getters, callback throws and oversized tapes keep callback behavior and refuse transport', () => {
  let reads = 0;
  const options = {get boundsOf() { reads++; return () => null; }};
  const accessor = captureEditBounds('centerCamera', ['x', [0], options], editor);
  assert.equal(reads, 0); assert.equal(accessor.args[2], options); assert.equal(accessor.payload(), null);
  for (const result of [Object.create({minX: 1}), 3, {get minX() { throw new Error('getter must not be read by capture'); }}]) {
    const capture = captureEditBounds('alignSelection', ['x', 'start', {boundsOf: () => result}], editor);
    assert.equal(capture.args[2].boundsOf(entry), result); assert.equal(capture.payload(), null);
  }
  const thrown = new Error('same callback error'), capture = captureEditBounds('centerCamera', ['x', [0], {boundsOf: () => { throw thrown; }}], editor);
  assert.throws(() => capture.args[2].boundsOf(entry), error => error === thrown); assert.equal(capture.payload(), null);
  const bounded = captureEditBounds('centerCamera', ['x', [0], {boundsOf: () => null}], editor);
  for (let n = 0; n <= MAX_BOUNDS_SAMPLES; n++) assert.equal(bounded.args[2].boundsOf(entry), null);
  assert.equal(bounded.payload(), null);
});

test('unsupported nested option data leaves getters, toJSON and the original call untouched', () => {
  let invoked = 0;
  const callback = () => null;
  for (const nested of [{get ticks() { invoked++; return []; }}, {toJSON() { invoked++; return null; }}, {data: 1n}]) {
    const args = ['both', [0], {boundsOf: callback, extra: nested}];
    const capture = captureEditBounds('centerCamera', args, editor);
    assert.equal(capture.args, args); assert.equal(capture.payload(), null);
  }
  assert.equal(invoked, 0);
});

test('the editing facade keeps callback commands in the editor and sends only their completed measurements', () => {
  const document = JSON.parse(readFileSync(new URL('../engine/tests/edit/measured-bounds.document.json', import.meta.url), 'utf8'));
  const owner = new Editor({demo: false, layerTimeline: true, propertyChannels: true}).load(document);
  owner.selectMany([1, 3]);
  const order = [], queries = appBounds();
  let received;
  const initialRevision = owner.revision;
  const session = {revision: () => initialRevision, free() {}, call(name, args) {
    order.push('engine'); received = {name, args: JSON.parse(args)};
    return JSON.stringify({status: 'unported', reason: 'inspection completed'});
  }};
  const side = {mirror: {framesCurrent: () => true}, engine: {handle: {edit_session: () => session}}};
  const report = installEditShadow(owner, () => side, {mode: 'own'});
  const boundsOf = entry => { order.push(entry.obj.unique_id); return queries(entry); };
  assert.deepEqual(owner.alignFiguresInFrames('x', 'end', [0, 1], {boundsOf}), {changedFrames: 2, changedTicks: [0, 6], insertedUids: []});
  assert.deepEqual(order, [1, 3, 1, 3, 'engine']);
  assert.equal(received.name, 'alignFiguresInFrames');
  assert.deepEqual(received.args[3].boundsOf['@measuredBounds'].samples.map(row => row.uid), [1, 3, 1, 3]);
  assert.equal(report.owned, 0); assert.equal(report.calls, 1);
  assert.equal(report.failedOwned, 0); assert.equal(report.failedOwnedMs, 0);
});
