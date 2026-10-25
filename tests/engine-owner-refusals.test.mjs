// A call the engine session refuses (here: copying a selected branch, which the session has no command for yet) runs in
// the editor and reaches the session as a history entry: the user's edit works, undoes and redoes exactly as the editor
// alone did (the frozen references of tests/support/owner-references.mjs), and the refusal is counted.
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

refs.test('a command refused for a document runs in the editor and undoes exactly', {skip: !engineBuilt}, async take => {
  const run = async owned => {
    const editor = new Editor({demo: false, layerTimeline: true, propertyChannels: true}).load(document());
    const owner = owned ? installEngineOwner(editor) : null;
    try {
      const steps = [text(editor)];
      editor.addPaletteColor('#12ab34'); steps.push(text(editor), editor.undoLabel);
      const uid = editor.evaluatedFrame.objects[0].unique_id;
      editor.selectBranch(uid, 2); editor.cutSelection(); steps.push(text(editor), editor.undoLabel);
      await editor.undo(); steps.push(text(editor));
      await editor.undo(); steps.push(text(editor));
      await editor.redo(); await editor.redo(); steps.push(text(editor), editor.undoLabel, editor.dirty);
      return {steps, refused: owner ? Object.keys(owner.refused) : []};
    } finally { owner?.release(); }
  };
  const alone = await take('alone', async () => (await run(false)).steps), owned = await run(true);
  assert.deepEqual(canonical(owned.steps), alone);
  assert.ok(owned.refused.some(key => key.startsWith('cutSelection: ') && key.includes('branch')), `the session refused it: ${owned.refused}`);
  assert.equal(owned.refused.length, 1, 'the palette colour is not refused');
});

// The owner ends the session's own gesture before the editor runs a refused call; a session that refuses to commit
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
    editor.selectBranch(uid, 2); editor.cutSelection();
    assert.equal(editor._drag, null, 'the gesture ended');
    assert.ok(owner.refused['commitDrag: a commit this test refuses'] >= 1);
    assert.ok(owner.refused['cutSelection: cutSelection of a selected branch'] >= 1 || Object.keys(owner.refused).some(key => key.startsWith('cutSelection: ')), 'the cut was refused and ran in the editor');
    assert.notEqual(text(editor), before, 'the cut changed the document');
    await editor.undo();
    assert.equal(text(editor), before, 'undoing the cut gives the document before the cancelled move');
  } finally { owner.release(); }
});
