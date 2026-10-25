// A call the engine session has no command for (here: inserting a blank keyframe, which the app never makes on a layer
// sheet) runs in the editor and reaches the session as a history entry: the user's edit works, undoes and redoes exactly
// as the editor alone did (the frozen references of tests/support/owner-references.mjs), and the call is counted.
import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {Editor} from '../src/editor.mjs';
import {installEngineOwner} from '../src/engine/engine-owner.mjs';
import {engineBuilt} from './support/engine-node.mjs';
import {canonical, ownerReferences} from './support/owner-references.mjs';

const refs = ownerReferences(import.meta.url);
const text = editor => JSON.stringify(editor.report, (key, value) => key.startsWith('_') && key !== '_styk' ? undefined : value);
const document = () => JSON.parse(readFileSync(new URL('../engine/tests/edit/startup.document.json', import.meta.url), 'utf8'));

refs.test('a command the session has no command for runs in the editor and undoes exactly', {skip: !engineBuilt}, async take => {
  const run = async owned => {
    const editor = new Editor({demo: false, layerTimeline: true, propertyChannels: true}).load(document());
    const owner = owned ? installEngineOwner(editor) : null;
    try {
      const steps = [text(editor)];
      editor.addPaletteColor('#12ab34'); steps.push(text(editor), editor.undoLabel);
      editor.insertBlankFrame(); steps.push(text(editor), editor.undoLabel);
      await editor.undo(); steps.push(text(editor));
      await editor.undo(); steps.push(text(editor));
      await editor.redo(); await editor.redo(); steps.push(text(editor), editor.undoLabel, editor.dirty);
      return {steps, refused: owner ? Object.keys(owner.refused) : [], ran: owner ? owner.editor : null};
    } finally { owner?.release(); }
  };
  const alone = await take('alone', async () => (await run(false)).steps), owned = await run(true);
  assert.deepEqual(canonical(owned.steps), alone);
  assert.deepEqual(owned.ran, {insertBlankFrame: 1}, 'the editor ran the one call the session has no command for');
  assert.deepEqual(owned.refused, [], 'the palette colour is not refused');
});

// The owner ends the session's own gesture before the editor runs a call of its own; a session that refuses to commit
// that gesture has it cancelled instead (the editor has no state for it), never sent round to the editor again.
test('a gesture the session refuses to commit is cancelled, not run in the editor', {skip: !engineBuilt}, async () => {
  const {openingSession} = await import('../src/engine/engine-owner.mjs');
  const editor = new Editor({demo: false, layerTimeline: true, propertyChannels: true}).load(document());
  const unported = JSON.stringify({status: 'unported', reason: 'a commit this test refuses'});
  const owner = installEngineOwner(editor, {start: e => {
    const session = openingSession(e);
    if (!session) return session;
    const call = session.call.bind(session);
    return Object.assign(session, {call: (method, args, view) => method === 'commitDrag' ? unported : call(method, args, view)});
  }});
  try {
    const uid = editor.evaluatedFrame.objects[0].unique_id, before = text(editor);
    editor.select(uid, -1); editor.beginTranslationDrag('Move figures'); editor.moveSelection(12, 4);
    assert.equal(editor._drag?.engine, true, 'the session holds the gesture');
    editor.insertBlankFrame();
    assert.equal(editor._drag, null, 'the gesture ended');
    assert.ok(owner.refused['commitDrag: a commit this test refuses'] >= 1);
    assert.equal(owner.editor.insertBlankFrame, 1, 'the call ran in the editor');
    assert.notEqual(text(editor), before, 'the call changed the document');
    await editor.undo();
    assert.equal(text(editor), before, 'undoing the call gives the document before the cancelled move');
  } finally { owner.release(); }
});
