// A call the engine session refuses for a document (here: importing a figure that carries an animation origin) runs in
// the editor and reaches the session as a history entry: the user's edit works, undoes and redoes exactly as the editor
// alone does, and the refusal is counted.
import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {Editor} from '../src/editor.mjs';
import {installEngineOwner} from '../src/engine/engine-owner.mjs';
import {engineBuilt} from './support/engine-node.mjs';

const text = editor => JSON.stringify(editor.report, (key, value) => key.startsWith('_') && key !== '_styk' ? undefined : value);
const document = () => JSON.parse(readFileSync(new URL('../engine/tests/edit/branch-thickness-origin-sheet.document.json', import.meta.url), 'utf8'));

test('a command refused for a document runs in the editor and undoes exactly', {skip: !engineBuilt}, async () => {
  const run = async owned => {
    const editor = new Editor({demo: false, layerTimeline: true, propertyChannels: true}).load(document());
    const owner = owned ? installEngineOwner(editor) : null;
    try {
      const steps = [text(editor)];
      editor.addPaletteColor('#12ab34'); steps.push(text(editor), editor.undoLabel);
      editor.importFigure(document(), {position: {x: 300, y: 200}}); steps.push(text(editor), editor.undoLabel);
      await editor.undo(); steps.push(text(editor));
      await editor.undo(); steps.push(text(editor));
      await editor.redo(); await editor.redo(); steps.push(text(editor), editor.undoLabel, editor.dirty);
      return {steps, refused: owner ? Object.keys(owner.refused) : []};
    } finally { owner?.release(); }
  };
  const alone = await run(false), owned = await run(true);
  assert.deepEqual(owned.steps, alone.steps);
  assert.ok(owned.refused.some(key => key.startsWith('importFigure: ') && key.includes('animation origins')), `the session refused it: ${owned.refused}`);
  assert.equal(owned.refused.length, 1, 'the palette colour is not refused');
});
