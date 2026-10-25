import test from 'node:test';
import assert from 'node:assert/strict';
import {snapshotPoseObject} from '../src/pose-snapshot.mjs';

test('pose snapshots own numeric storage and metadata without retaining render caches', () => {
  const pose = {unique_id:1, angles:[0,-0,Math.PI], lengths:[1,2,3], bends:[],
    _stykStyle:{segments:{1:{color:[1,2,3]}}}, runtime_segments:[{uncloneable:()=>{}}], _movieclipSample:{uncloneable:()=>{}}};
  const snapshot = snapshotPoseObject(pose);
  assert.equal('runtime_segments' in snapshot,false);assert.equal('_movieclipSample' in snapshot,false);
  for (const field of ['angles','lengths','bends']) {assert.deepEqual(snapshot[field],pose[field]);assert.notEqual(snapshot[field],pose[field]);}
  pose.angles[0]=15;pose._stykStyle.segments[1].color[0]=255;
  assert.equal(snapshot.angles[0],0);assert.equal(snapshot._stykStyle.segments[1].color[0],1);
});

