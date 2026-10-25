import test from 'node:test';
import assert from 'node:assert/strict';
import {MAX_ALLOCATED_UID, nextUidAfter} from '../src/object-identity.mjs';


test('nextUidAfter ignores identities that cannot collide with allocations', () => {
  assert.equal(nextUidAfter(1, 5), 6);
  assert.equal(nextUidAfter(9, 5), 9);
  assert.equal(nextUidAfter(1, MAX_ALLOCATED_UID), MAX_ALLOCATED_UID + 1);
  assert.equal(nextUidAfter(3, MAX_ALLOCATED_UID + 1), 3);
  assert.equal(nextUidAfter(3, 0xffffffff), 3);
});
