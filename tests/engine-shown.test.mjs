// The frame on show, its geometry and the timeline come from the engine's session (docs/engine/evaluation.md,
// "The interface's frame"): the groups the canvas reads, kept by identity while a gesture moves other figures.
import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {Editor} from '../src/editor.mjs';
import {installEngineOwner} from '../src/engine/engine-owner.mjs';
import {holdsRevision, registeredEngine} from '../src/engine/engine-open.mjs';
import {engineBuilt} from './support/engine-node.mjs';
import {ownedEditor} from './support/owned-editor.mjs';

const folder = new URL('../engine/crates/styk-eval/tests/fixtures/channels/', import.meta.url);
const read = name => JSON.parse(readFileSync(new URL(name, folder), 'utf8'));
const options = {demo: false, layerTimeline: true, propertyChannels: true};

/** Figures 1 and 2, 3 hanging from 2, and 4 alone, on a layer sheet. */
const poses = () => ownedEditor(options, read('channels-pose.sheet.json'));

test('the frame, geometry and timeline are the session\'s', {skip: !engineBuilt}, async () => {
  const {editor, owner} = await poses();
  try {
    assert.ok(editor._shown.active, 'the engine answers for the open document');
    editor.seek(11);
    const frame = editor.evaluatedFrame, geometry = editor.geometry;
    assert.equal(frame.objects.length, 4);
    assert.deepEqual([...geometry.objects.keys()], [0, 1, 2, 3]);
    for (const [slot, group] of geometry.objects) {
      const figure = editor.report.figure_types.find(type => type.figure_type_id === frame.objects[slot].figure_type_id);
      assert.equal(group.obj, frame.objects[slot], 'a group is of the object of its slot');
      assert.equal(group.segments.length, figure.segments.length, 'a shape for every segment, drawn or not');
      assert.deepEqual([...group.joints.keys()], [0, ...figure.segments.map(segment => segment.segment_id)], 'the root, then a joint for each segment');
      assert.equal(group.joints.get(0), group.root);
      group.segments.forEach((shape, index) => {
        assert.equal(shape.index, index);
        assert.equal(shape.seg, figure.segments[index]);
        assert.deepEqual(group.joints.get(shape.seg.segment_id), shape.end, 'a segment ends where its joint is');
        assert.ok(Number.isFinite(shape.start.x) && Number.isFinite(shape.length));
      });
    }
    assert.equal(geometry.orderedObjects.every(group => group.drawables.length > 0), true, 'the objects in painter order have artwork');
    assert.equal(editor.timeline.at(-1).start + editor.timeline.at(-1).count, editor.totalTicks);
    assert.ok(editor.totalTicks > 11);
    // The same geometry is read again as it was held.
    assert.equal(editor.geometry, geometry);
    editor._invalidate();
    assert.equal(editor.geometry, geometry, 'nothing changed: the groups held are the answer');
  } finally { owner.release(); }
});

test('a gesture rebuilds the groups of the figures it moves and keeps the others', {skip: !engineBuilt}, async () => {
  const {editor, owner} = await poses();
  try {
    editor.seek(11);
    const slotOf = uid => editor.evaluatedFrame.objects.findIndex(object => object.unique_id === uid);
    const before = editor.geometry, alone = slotOf(4);
    editor.select(4, -1);
    editor.beginTranslationDrag();
    editor.moveSelection(12, 0, {fromStart: true});
    const during = editor.geometry;
    assert.notEqual(during, before);
    assert.equal(during.objects.get(alone).root.x, before.objects.get(alone).root.x + 12, 'the moved figure is where the gesture put it');
    for (const [slot, group] of before.objects) if (slot !== alone) assert.equal(during.objects.get(slot), group, `the group of slot ${slot} is the one already held`);
    assert.deepEqual([...during.changedSlots], [alone]);
    editor.cancelDrag();
    assert.equal(editor.geometry.objects.get(alone).root.x, before.objects.get(alone).root.x, 'cancelled: the figure is back');
    // Moving the figure another hangs from moves both.
    editor.select(3, -1);
    const joined = [2, 3].map(uid => slotOf(uid)), held = editor.geometry;
    editor.beginTranslationDrag();
    editor.moveSelection(5, 5, {fromStart: true});
    const moved = editor.geometry;
    for (const slot of joined) assert.notEqual(moved.objects.get(slot), held.objects.get(slot), `slot ${slot} moves with what it hangs from`);
    for (const slot of [1, 4].map(uid => slotOf(uid))) assert.equal(moved.objects.get(slot), held.objects.get(slot), `slot ${slot} is not joined to them`);
    editor.cancelDrag();
  } finally { owner.release(); }
});

test('a movieclip edited in place is evaluated by the editor, which runs its commands', {skip: !engineBuilt}, async () => {
  const document = JSON.parse(readFileSync(new URL('../engine/tests/edit/startup.document.json', import.meta.url), 'utf8'));
  const {editor, owner} = await ownedEditor(options, document);
  try {
    editor.select(editor.evaluatedFrame.objects[0].unique_id, -1);
    const clip = editor.createMovieclip({start: editor.tick, end: editor.tick + 6});
    editor.enterMovieclip(clip.figure_type_id);
    assert.ok(editor._movieclipScopes.length, 'the movieclip is open for editing');
    assert.equal(editor._shown.frame(), null, 'the session does not answer for it');
    assert.equal(editor._shown.geometry(editor.evaluatedFrame), null);
    assert.equal(editor._shown.source(false), null, 'the stage gets its poses from the editor');
    editor.select(editor.evaluatedFrame.objects[0].unique_id, -1);
    const before = editor.evaluatedFrame.objects[0].root_x;
    editor.moveSelection(25, 10);
    assert.equal(editor.evaluatedFrame.objects[0].root_x, before + 25, 'the frame follows the edit');
    assert.equal(editor.geometry.objects.get(0).root.x, before + 25);
    editor.leaveMovieclip();
    assert.ok(editor._shown.frame(), 'back in the project the session answers');
  } finally { owner.release(); }
});

test('a detached editor has a session of its own and leaves the open project the engine\'s document', {skip: !engineBuilt}, async () => {
  const {editor, owner} = await poses();
  const engine = registeredEngine();
  try {
    assert.ok(holdsRevision(engine, editor.revision), 'the project is the engine\'s document');
    const test = new Editor({demo: false, layerTimeline: true}), tester = installEngineOwner(test, {detached: true});
    try {
      test.load(read('channels-pose.sheet.json'));
      assert.ok(tester.ready(), 'the test editor has its own session');
      const object = test.evaluatedFrame.objects[0], figure = test.report.figure_types.find(type => type.figure_type_id === object.figure_type_id);
      test.select(object.unique_id, figure.segments.length - 1);
      const joint = test.geometry.objects.get(0).segments[figure.segments.length - 1], end = {...joint.end};
      assert.equal(test.beginDrag('Test pose'), true);
      assert.equal(test.poseJoint(end.x + 40, end.y + 40, {snap: false}), true);
      assert.notDeepEqual(test.geometry.objects.get(0).segments[figure.segments.length - 1].end, end, 'posing moves the joint the test shows');
      test.commitDrag();
      assert.ok(holdsRevision(engine, editor.revision), 'the project is still the engine\'s document');
      assert.equal(editor._shown.source(false)?.session, owner.session(), 'and the stage draws from its session');
    } finally { tester.release(); }
  } finally { owner.release(); }
});

/** Counts the reads the owner makes of the session by name. */
function spy(session) {
  const ask = session.query.bind(session), asked = [];
  session.query = (kind, ...rest) => { asked.push(kind); return ask(kind, ...rest); };
  return asked;
}

test('the values the interface shows are the session\'s reads', {skip: !engineBuilt}, async () => {
  const {editor, owner} = await poses();
  try {
    editor.seek(11);
    const asked = spy(owner.session());
    const object = editor.evaluatedFrame.objects.find(candidate => candidate.unique_id === 4);
    assert.equal(editor.animationValue({kind: 'figure', uid: 4}, 'x'), object.root_x);
    assert.equal(editor.animationValue({kind: 'figure', uid: 999}, 'x'), undefined, 'a figure that is not there has no value');
    assert.deepEqual(editor.segmentAnimationTarget(4, 2, 'x'), {kind: 'segment', uid: 4, handleId: 2});
    assert.deepEqual(editor.vfxEffects({kind: 'figure', uid: 4}), []);
    editor.select(4, 0);
    assert.equal(editor.getIKSettings().enabled, false);
    assert.ok(editor.getIKSettings().maxLength >= 1);
    assert.deepEqual(asked.filter(kind => !['frame', 'document', 'geometry', 'timeline', 'motion'].includes(kind)),
      ['animationValue', 'animationValue', 'segmentAnimationTarget', 'vfxEffects', 'getIKSettings', 'getIKSettings']);
    assert.throws(() => editor.animationValue({kind: 'figure', uid: 4}, 'x', -1), /keyframeTimeMustBeAnIntegerFrom0|integer from 0/i);
  } finally { owner.release(); }
});

test('a document the session does not answer property values for is read by the editor', {skip: !engineBuilt}, async () => {
  const document = JSON.parse(readFileSync(new URL('../engine/tests/edit/startup.document.json', import.meta.url), 'utf8'));
  const {editor, owner} = await ownedEditor({demo: false}, document);
  try {
    const asked = spy(owner.session());
    const object = editor.evaluatedFrame.objects[0];
    assert.equal(editor.animationValue({kind: 'figure', uid: object.unique_id}, 'x'), object.root_x);
    assert.ok(asked.includes('animationValue'), 'the session was asked and did not answer');
  } finally { owner.release(); }
});

test('what the session says it does not evaluate yet is evaluated by the editor; any other refusal is raised', {skip: !engineBuilt}, async () => {
  const {editor, owner} = await poses();
  try {
    editor.seek(11);
    const expected = editor._evaluateHere();
    const session = owner.session(), ask = session.query.bind(session);
    session.query = () => { throw new Error('unported: geometry with a feature of its own'); };
    editor._invalidate();
    assert.deepEqual(editor.evaluatedFrame.objects.map(object => object.root_x), expected.objects.map(object => object.root_x));
    assert.equal(editor.geometry.objects.size, expected.objects.length);
    session.query = () => { throw new Error('errors.something.invalid'); };
    editor._invalidate();
    assert.throws(() => editor.evaluatedFrame, /errors\.something\.invalid/);
    session.query = ask;
    editor._invalidate();
    assert.equal(editor.evaluatedFrame.objects.length, expected.objects.length);
  } finally { owner.release(); }
});

test('a read made while the editor\'s edits are being written to the session is the editor\'s own', {skip: !engineBuilt}, async () => {
  const {editor, owner} = await poses();
  try {
    editor.seek(11);
    editor.select(4, -1);
    // A selected polygon is checked against the figure on show whenever the view is taken.
    editor._polygonSelection = {uid: 4, typeId: editor.object.figure_type_id, index: 0};
    // An edit made in place and announced, not yet written to the session: the read writes it first, and the view the
    // write takes reads the frame on show in turn, which must not write it again.
    editor.report._styk.name = 'Renamed in place';
    editor._emit('edit', {label: 'Project metadata'});
    editor._invalidate();
    const calls = [];
    const session = owner.session(), call = session.call.bind(session);
    session.call = (...parts) => { calls.push(parts[0]); return call(...parts); };
    assert.ok(editor.evaluatedFrame.objects.length > 0);
    session.call = call;
    assert.deepEqual(calls, ['@sync'], 'written once');
    assert.equal(JSON.parse(owner.session().report_json())._styk.name, 'Renamed in place');
  } finally { owner.release(); }
});
