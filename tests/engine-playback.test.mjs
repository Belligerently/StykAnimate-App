import test from 'node:test';
import assert from 'node:assert/strict';
import {createStagePlayback, engineTick} from '../src/engine/engine-playback.mjs';

function playbackSide({maxFrames} = {}) {
  let current = null, generation = 7;
  let readinessReads = 0;
  const requests = [], ready = new Map(), legacyCalls = [];
  const handle = {
    request_samples(playhead, samples) { requests.push({playhead, samples}); },
    // Calling the integer path would truncate the sample before the worker.
    request() { throw new Error('integer binding reached'); },
    frame_ready: tick => { readinessReads++; return ready.has(tick); },
    frame_meta: tick => JSON.stringify(ready.get(tick)),
    generation: () => generation,
    set_hidden() {}, total_ticks: () => 9,
  };
  const side = {engine: {handle, pump() {}}, eligibility: {check: () => null}, drawingUnpublished: () => false,
    mirror: {stats: {}, async syncFrames(report, revision) { current = {report, revision}; },
      framesCurrent: (report, revision) => current?.report === report && current?.revision === revision}};
  const legacy = {report: null, maxFrames, load(...args) { legacyCalls.push(args); }, dispose() {}, invalidate() {}};
  return {playback: createStagePlayback(legacy, {stage: () => ({engineSide: side})}), ready, requests, legacyCalls,
    reads: () => readinessReads,
    nextGeneration() { generation++; }};
}

test('fractional range requests retain Float64 samples and distinct ready packet metadata', async () => {
  const {playback, ready, requests, legacyCalls} = playbackSide();
  playback.load({}, {revision: 1});
  await playback.prepareRange([3, 3.5]);
  assert.equal(requests.length, 1);
  assert.equal(requests[0].playhead, 3);
  assert.ok(requests[0].samples instanceof Float64Array);
  assert.deepEqual([...requests[0].samples], [3, 3.5]);
  assert.equal(playback.get(3.5), null, 'cold readiness does not evaluate or fall through');
  ready.set(3, {tick: 3, camera: {x: 10}, background_id: 1});
  ready.set(3.5, {tick: 3.5, camera: {x: 12}, background_id: 2});
  const integer = playback.get(3), fractional = playback.get(3.5);
  assert.notEqual(integer, fractional);
  assert.equal(fractional, playback.get(3.5), 'same ticket in existing packet map');
  assert.equal(fractional.frame.tick, 3.5);
  assert.equal(fractional.frame.camera.x, 12);
  assert.equal(fractional.frame.background_id, 2);
  assert.equal(engineTick(fractional.key), 3.5);
  assert.notEqual(integer.key, fractional.key);
  assert.equal(playback.cache.size, 2);
  assert.equal(legacyCalls.length, 0);
});

test('native eviction prunes 100,000 distinct fractional metadata tickets', async () => {
  const {playback, ready, reads} = playbackSide();
  playback.load({}, {revision: 1});
  await playback.prepareRange([0]);
  for (let index = 0; index < 100_000; index++) {
    const tick = index / 100_000;
    ready.clear(); ready.set(tick, {tick});
    const before = reads(), packet = playback.get(tick);
    assert.equal(packet.tick, tick);
    assert.equal(playback.cache.size, 1, 'obsolete native ticket is released by the existing packet owner');
    assert.ok(reads() - before <= 121, 'fixed metadata scan bound');
  }
});

test('finite ticket cap preserves current and queued packet identities without limiting native requests', async () => {
  for (const [maxFrames, limit] of [[0, 1], [NaN, 120], [Infinity, 120], [1_000_000, 120], [2.9, 2]]) {
    const {playback, ready, requests, reads} = playbackSide({maxFrames});
    playback.load({}, {revision: 1});
    const ticks = Array.from({length: 500}, (_, index) => index / 1000);
    await playback.prepareRange(ticks);
    assert.equal(requests.at(-1).samples.length, ticks.length, 'native frame preparation is unchanged');
    for (const tick of ticks) ready.set(tick, {tick});
    const current = playback.get(0), queued = limit > 1 ? playback.get(.001) : null;
    for (const tick of ticks) {
      const before = reads();
      assert.equal(playback.get(tick).tick, tick);
      assert.ok(playback.cache.size <= limit);
      assert.ok(reads() - before <= 121, `${maxFrames}: configured maxFrames cannot expand the UI scan`);
    }
    assert.equal(playback.get(-0), current);
    assert.equal(current.tick, 0);
    if (queued) assert.equal(playback.get(.001), queued);
  }
});

test('request replacement, native eviction and cancellation preserve only current valid identities', async () => {
  const {playback, ready, nextGeneration} = playbackSide({maxFrames: 3});
  playback.load({}, {revision: 1});
  await playback.prepareRange([0, .5, 1]);
  for (const tick of [0, .5, 1, 2, 2.5, 3]) ready.set(tick, {tick});
  const current = playback.get(0), queued = playback.get(.5), last = playback.get(1);
  assert.equal(playback.get(2).tick, 2, 'other ready sample remains available without displacing queued tickets');
  assert.equal(playback.get(0), current); assert.equal(playback.get(.5), queued); assert.equal(playback.get(1), last);
  ready.delete(.5);
  assert.equal(playback.get(.5), null);
  assert.equal(playback.cache.has(.5), false);
  await playback.prepareRange([2, 2.5, 3]);
  const replacement = playback.get(2);
  playback.get(2.5); playback.get(3);
  assert.deepEqual([...playback.cache.keys()].sort((a, b) => a - b), [2, 2.5, 3]);
  assert.equal(playback.get(2), replacement);
  nextGeneration();
  assert.notEqual(playback.get(2), replacement, 'new generation cannot retain a prior ticket');
  playback.invalidate();
  assert.equal(playback.cache.size, 0);
});

test('invalid samples never create tickets or replace valid current request priority', async () => {
  const {playback, ready, reads} = playbackSide({maxFrames: 1});
  playback.load({}, {revision: 1});
  await playback.prepareRange([0]); ready.set(0, {tick: 0}); ready.set(1, {tick: 1});
  const current = playback.get(-0);
  for (const tick of [NaN, Infinity, -Infinity, -1, '0', undefined, null]) {
    ready.set(tick, {tick});
    const before = reads();
    assert.equal(playback.get(tick), null);
    assert.equal(reads(), before, 'invalid identity does not cross the native query boundary');
  }
  await playback.prepareRange([NaN, 1]);
  assert.equal(playback.get(1).tick, 1);
  assert.equal(playback.get(0), current);
  await playback.prepareRange([1, Infinity, -1]);
  const next = playback.get(1);
  assert.equal(playback.cache.size, 1);
  assert.equal(playback.get(1), next);
});

test('invalidation cancels fractional requests and a replacement revision drops old generation packets', async () => {
  const {playback, ready, requests, nextGeneration} = playbackSide();
  playback.load({}, {revision: 1});
  await playback.prepareRange([3.5]);
  ready.set(3.5, {tick: 3.5});
  const previous = playback.get(3.5);
  playback.invalidate();
  assert.equal(playback.cache.size, 0);
  assert.ok(requests.at(-1).samples instanceof Float64Array);
  assert.equal(requests.at(-1).samples.length, 0);
  playback.updateDocument({revision: 2});
  assert.equal(playback.get(3.5), null, 'metadata stays unavailable until mirror revision is current');
  nextGeneration();
  await playback.prepareRange([3.5]);
  const current = playback.get(3.5);
  assert.notEqual(current, previous);
  assert.notEqual(current.key, previous.key);
  assert.equal(current.tick, 3.5);
});
