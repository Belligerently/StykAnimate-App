// The engine's stage (src/engine/engine-stage.mjs): a still frame drawn while its pictures
// decode is drawn again once they can be shown, without a loop on the interface thread.
import test from 'node:test';
import assert from 'node:assert/strict';
import {waitForPictures} from '../src/engine/engine-stage.mjs';

/** A stage side whose last frame lacks pictures as `pending` says, and an engine whose progress counter the test advances. */
function side(pending) {
  let progress = 0, wake = null;
  const waits = [];
  const engine = {
    progressSeen: () => progress,
    // Resolves when the counter moves past `seen`, or when the test lets the wait time out.
    whenProgress(seen) {
      waits.push(seen);
      if (progress !== seen) return Promise.resolve();
      return new Promise(resolve => { wake = resolve; });
    },
  };
  return {
    side: {stage: {pictures_pending: () => pending.shift() ?? 0}, engine, pictureWait: 0, stats: {pictureRedraws: 0}},
    waits,
    advance() { progress++; wake?.(); wake = null; },
    timeout() { wake?.(); wake = null; },
  };
}

const settle = () => new Promise(resolve => setTimeout(resolve, 0));

test('a frame with every picture is not drawn again', async () => {
  const {side: s, waits} = side([0]);
  let redraws = 0;
  waitForPictures(s, 0, () => redraws++);
  await settle();
  assert.equal(redraws, 0);
  assert.equal(waits.length, 0, 'nothing waits on the engine');
});

test('pictures decoded and waiting for upload: drawn again on the next frame', async () => {
  const {side: s, waits} = side([1]);
  let redraws = 0;
  waitForPictures(s, 0, () => redraws++);
  assert.equal(redraws, 0, 'not inside the frame being drawn');
  await settle();
  assert.equal(redraws, 1);
  assert.equal(waits.length, 0);
  assert.equal(s.stats.pictureRedraws, 1);
});

test('pictures still decoding: drawn again once the engine finishes something, not on a timeout alone', async () => {
  const engine = side([2]);
  let redraws = 0;
  waitForPictures(engine.side, 0, () => redraws++);
  await settle();
  assert.equal(redraws, 0, 'waits on the progress counter');
  engine.timeout();
  await settle();
  assert.equal(redraws, 0, 'a wait that timed out with nothing finished waits again without drawing');
  assert.deepEqual(engine.waits, [0, 0]);
  engine.advance();
  await settle();
  assert.equal(redraws, 1);
});

test('progress made while the frame was drawn is not missed', async () => {
  const engine = side([2]);
  let redraws = 0;
  // The counter was read before drawing; a picture decoded meanwhile.
  engine.advance();
  waitForPictures(engine.side, 0, () => redraws++);
  await settle();
  assert.equal(redraws, 1);
});

test('another frame drawn meanwhile takes over the wait', async () => {
  const engine = side([2, 0]);
  let first = 0, second = 0;
  waitForPictures(engine.side, 0, () => first++);
  waitForPictures(engine.side, 0, () => second++);
  engine.advance();
  await settle();
  assert.equal(first, 0, 'the superseded wait does not draw');
  assert.equal(second, 0, 'the frame drawn last had its pictures');
});
