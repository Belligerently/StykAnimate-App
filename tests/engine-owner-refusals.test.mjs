// A call the engine session has no command for (here: inserting a blank keyframe, which the app never makes on a layer
// sheet) is refused with the error the interface shows, counted, and changes nothing: the editor does not run it, and the
// calls around it work and undo as they do. The calls with no command are `REFUSED_CALLS` (src/engine/engine-commands.mjs).
import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {Editor} from '../src/editor.mjs';
import {installEngineOwner} from '../src/engine/engine-owner.mjs';
import {engineBuilt} from './support/engine-node.mjs';

const text = editor => JSON.stringify(editor.report, (key, value) => key.startsWith('_') && key !== '_styk' ? undefined : value);
const document = () => JSON.parse(readFileSync(new URL('../engine/tests/edit/startup.document.json', import.meta.url), 'utf8'));

test('a call the session has no command for is refused and changes nothing', {skip: !engineBuilt}, async () => {
  const editor = new Editor({demo: false, layerTimeline: true, propertyChannels: true}).load(document());
  const owner = installEngineOwner(editor, {verify: true});
  try {
    editor.addPaletteColor('#12ab34');
    const colored = text(editor), label = editor.undoLabel;
    assert.throws(() => editor.insertBlankFrame(), error => error.messageKey === 'errors.engineOwner.notSupported' && error.reason === 'insertBlankFrame: a native timeline has no engine command');
    assert.equal(text(editor), colored, 'the refused call changed nothing');
    assert.equal(editor.undoLabel, label);
    assert.deepEqual(owner.refused, {'insertBlankFrame: a native timeline has no engine command': 1});
    for (const name of ['addFrame', 'deleteFrame', 'duplicateFrame', 'reverseFrames', 'insertInbetweenFrames', 'insertInbetweenFramesAsync']) {
      assert.throws(() => editor[name](), error => error.messageKey === 'errors.engineOwner.notSupported', name);
    }
    assert.equal(text(editor), colored);
    await editor.undo();
    await editor.redo();
    assert.equal(text(editor), colored, 'the entries around the refusal undo and redo');
  } finally { owner.release(); }
});
