import test from 'node:test';
import assert from 'node:assert/strict';
import {easedProgress, cubicBezierProgress, keyEasing} from '../src/keyframe-easing.mjs';
import {sampleAnimationChannel} from '../src/animation-channels.mjs';

const near = (actual, expected, epsilon = 1e-7) => assert.ok(Math.abs(actual - expected) < epsilon, `${actual} ≠ ${expected}`);
const modes = ['auto', 'bezier', 'ease-in', 'ease-out', 'linear', 'constant'];
test('Bezier inverts time, presets are distinct, constant switches only at the next key', () => {
  // x(u)=u^3, y(u)=3u-3u²+u³: at x=.125, u=.5 and y=.875.
  near(cubicBezierProgress(.125, [0,1,0,1]), .875);
  near(easedProgress({interpolation:'ease-in'}, .5), .3153568125725393);
  near(easedProgress({interpolation:'ease-out'}, .5), .6846431874274608);
  near(easedProgress({interpolation:'bezier'}, .25), .1291619310473198);
  for (const interpolation of modes) {
    const key = {interpolation};
    assert.equal(easedProgress(key, -1), 0); assert.equal(easedProgress(key, 2), 1);
    let previous = 0;
    for (let i = 0; i <= 100; i++) { const value = easedProgress(key, i / 100); assert.ok(value >= previous && value <= 1); previous = value; }
  }
  assert.equal(easedProgress({interpolation:'constant'}, .999999), 0);
  assert.deepEqual(keyEasing({interpolation:'hold'}), {interpolation:'constant'});
});

test('Auto shares velocity at unevenly spaced keys, stops at extrema and never overshoots', () => {
  const keys = [0, 4, 17, 20, 31].map((tick, index) => ({tick, value:[0, 12, 40, 20, 20][index], interpolation:'auto'}));
  const channel = {property:'x', keys}, at = tick => sampleAnimationChannel(channel, tick), h = 1e-5;
  for (let i = 0; i < keys.length - 1; i++) for (let step = 0; step <= 100; step++) {
    const a = keys[i], b = keys[i + 1], value = at(a.tick + (b.tick - a.tick) * step / 100);
    assert.ok(value >= Math.min(a.value,b.value) - 1e-9 && value <= Math.max(a.value,b.value) + 1e-9);
  }
  const leftVelocity = (at(4) - at(4-h))/h, rightVelocity = (at(4+h) - at(4))/h;
  assert.ok(leftVelocity > 2); near(leftVelocity, rightVelocity, 3e-5);
  near((at(17+h)-at(17))/h,0,1e-4); near((at(17)-at(17-h))/h,0,1e-4);
  near(at(h)/h,0,1e-4); near(at(25),20);
  keys[1].interpolation = 'constant';
  near(at(4-h),12,1e-7); near(at(16.9),12); near(at(17),40);
  keys[1].interpolation = 'auto'; keys[2].passthrough = true;
  near(at(10),12); assert.equal(at(17),undefined);
});

