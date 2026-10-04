import test from 'node:test';
import assert from 'node:assert/strict';
import {EngineMirror} from '../src/engine/engine-mirror.mjs';

function mirrorWith({record, commit, source, definitionsCommit} = {}) {
  const staged = [], sources = [], discarded = [], responses = [];
  const handle = {
    stage_reset() {}, stage_properties() {}, stage_layout() {}, stage_source(batch, id) { sources.push({batch, id}); source?.(batch, id); },
    stage_record(batch, table, index) { staged.push({batch, table, index}); record?.(batch, table, index); },
    commit_now: revision => JSON.stringify(definitionsCommit?.(revision) ?? {revision, ms: 0}),
    commit(revision) { responses.push(JSON.stringify({revision, ms: 0})); commit?.(); },
    take_committed: () => responses.shift(), discard_batch: batch => discarded.push(batch),
  };
  const mirror = new EngineMirror({handle, pump() {}, progressSeen: () => 0, whenProgress: () => Promise.resolve()});
  return {mirror, staged, sources, discarded, responses};
}

test('an already aborted sync stages nothing', async () => {
  const {mirror, staged, discarded} = mirrorWith(), abort = new AbortController();
  abort.abort();
  await assert.rejects(mirror.syncFrames({keyframes: [{objects: []}]}, 1, {}, {signal: abort.signal}), {name: 'AbortError'});
  assert.equal(mirror.report, null);
  assert.deepEqual(staged, []);
  assert.deepEqual(discarded, []);
});

test('aborting after staging discards the partial batch and resends frame identities next time', async () => {
  const abort = new AbortController();
  const {mirror, staged, discarded} = mirrorWith({record: batch => { if (batch === 1) abort.abort(); }});
  const report = {keyframes: [{objects: []}, {objects: []}]};
  await assert.rejects(mirror.syncFrames(report, 1, {}, {signal: abort.signal}), {name: 'AbortError'});
  assert.deepEqual(discarded, [1]);
  assert.equal(mirror.framesCurrent(report, 1), false);
  assert.deepEqual(mirror.sent.keyframes, []);
  await mirror.syncFrames(report, 2);
  assert.equal(mirror.framesCurrent(report, 2), true);
  assert.equal(staged.filter(item => item.batch === 2).length, 2);
});

test('a reset during staging preserves the new report bookkeeping', async () => {
  const next = {figure_types: [{name: 'new'}], keyframes: []};
  let mirror;
  const state = mirrorWith({record: batch => { if (batch === 1) mirror.syncDefinitions(next, 9); }});
  mirror = state.mirror;
  await assert.rejects(mirror.syncFrames({keyframes: [{objects: []}, {objects: []}]}, 1), {name: 'AbortError'});
  assert.equal(mirror.report, next);
  assert.equal(mirror.sent.figure_types[0], next.figure_types[0]);
  assert.deepEqual(state.discarded, [1]);
});

test('canceling a commit already in flight drains its response without a false current ACK', async () => {
  const abort = new AbortController();
  const {mirror, responses, discarded} = mirrorWith({commit: () => abort.abort()});
  const report = {keyframes: [{objects: []}]};
  await assert.rejects(mirror.syncFrames(report, 1, {}, {signal: abort.signal}), {name: 'AbortError'});
  assert.equal(mirror.framesCurrent(report, 1), false);
  assert.equal(responses.length, 0, 'the next serialized sync owns no stale response');
  assert.deepEqual(discarded, [1]);
});

test('conservative flags resend unchanged identities whose records were edited in place', async () => {
  const {mirror, staged, sources} = mirrorWith();
  const report = {keyframes: [{objects: []}, {objects: []}], _styk: {frameSources: {one: {objects: []}}}};
  await mirror.syncFrames(report, 1);
  report.keyframes[1].objects.push({root_x: 5});
  report._styk.frameSources.one.objects.push({root_x: 8});
  await mirror.syncFrames(report, 2, {allFrames: true, allSources: true});
  assert.equal(staged.filter(item => item.batch === 2).length, 2);
  assert.deepEqual(sources.filter(item => item.batch === 2).map(item => item.id), ['one']);
  assert.equal(mirror.framesCurrent(report, 2), true);
});


test('definition stage refusal discards the attempted batch and retries every optimistic definition identity', () => {
  let refuse = true;
  const failure = new Error('the record receiver is full');
  const {mirror, staged, discarded} = mirrorWith({record: (batch, table) => { if (table === 2 && refuse) throw failure; }});
  const report = {figure_types: [{name: 'figure'}], backgrounds: [{name: 'background'}], keyframes: []};
  assert.throws(() => mirror.syncDefinitions(report, 1), error => error === failure);
  assert.deepEqual(discarded, [0]);
  assert.equal(mirror.report, null);
  assert.deepEqual(mirror.sent, {figure_types: [], keyframes: [], backgrounds: []});
  assert.equal(mirror.stats.errors, 1);
  refuse = false;
  mirror.syncDefinitions(report, 1);
  assert.equal(staged.filter(item => item.table === 0).length, 2, 'the next attempt resends a figure staged before refusal');
  assert.equal(staged.filter(item => item.table === 2).length, 2);
  assert.equal(mirror.report, report);
});

test('definition busy preserves its partial batch for a later commit', () => {
  let busy = true;
  const {mirror, staged, discarded} = mirrorWith({definitionsCommit: revision => busy ? {busy: true} : {revision, ms: 0}});
  const report = {figure_types: [{name: 'figure'}], keyframes: []};
  assert.throws(() => mirror.syncDefinitions(report, 1), {code: 'engine_busy'});
  assert.deepEqual(discarded, []);
  assert.equal(mirror.report, report);
  busy = false;
  mirror.syncDefinitions(report, 1);
  assert.equal(staged.filter(item => item.table === 0).length, 1);
  assert.equal(mirror.stats.errors, 0);
});

test('source stage refusal discards frames and optimistic sources before exact retry', async () => {
  let refuse = true;
  const failure = new Error('the record receiver is full');
  const {mirror, sources, staged, discarded} = mirrorWith({source: (batch, id) => { if (id === 'second' && refuse) throw failure; }});
  const report = {keyframes: [{objects: []}], _styk: {frameSources: {first: {objects: []}, second: {objects: []}}}};
  await assert.rejects(mirror.syncFrames(report, 1), error => error === failure);
  assert.deepEqual(discarded, [1]);
  assert.deepEqual(mirror.sent.keyframes, []);
  assert.equal(mirror.sources.size, 0);
  assert.equal(mirror.framesCurrent(report, 1), false);
  refuse = false;
  await mirror.syncFrames(report, 2);
  assert.equal(staged.filter(item => item.batch === 2 && item.table === 1).length, 1);
  assert.deepEqual(sources.filter(item => item.batch === 2).map(item => item.id), ['first', 'second']);
  assert.equal(mirror.framesCurrent(report, 2), true);
});
