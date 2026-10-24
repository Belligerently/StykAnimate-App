// Editing inside a movieclip under the engine owner: making a movieclip, entering it, moving a
// figure in it and leaving it is undone back to the document before and redone to the document
// after, as the editor alone does (the gate found redo giving another document).
import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {Editor} from '../src/editor.mjs';
import {installEngineOwner} from '../src/engine/engine-owner.mjs';
import {engineBuilt} from './support/engine-node.mjs';

const text = editor => JSON.stringify(editor.report, (key, value) => key.startsWith('_') && key !== '_styk' ? undefined : value);
const documents = ['startup', 'sheet-layers'];

for (const name of documents) for (const options of [{layerTimeline: true, propertyChannels: true}, {layerTimeline: false, propertyChannels: false}]) {
  test(`${name} (${options.layerTimeline ? 'layer sheet' : 'native'}): a movieclip edit is undone and redone exactly`, {skip: !engineBuilt}, async () => {
    const document = JSON.parse(readFileSync(new URL(`../engine/tests/edit/${name}.document.json`, import.meta.url), 'utf8'));
    const editor = new Editor({demo: false, ...options}).load(structuredClone(document));
    const owner = installEngineOwner(editor);
    try {
      const uid = editor.evaluatedFrame.objects[0].unique_id, before = text(editor), depth = editor._undo.length;
      editor.select(uid, -1);
      const clip = editor.createMovieclip({start: editor.tick, end: editor.tick + 6});
      editor.enterMovieclip(clip.figure_type_id);
      editor.select(editor.evaluatedFrame.objects[0].unique_id, -1);
      editor.moveSelection(25, 10);
      editor.leaveMovieclip();
      const after = text(editor), entries = editor._undo.length - depth;
      assert.notEqual(after, before, 'the edit changed the document');
      for (let i = 0; i < entries; i++) await editor.undo();
      assert.equal(text(editor), before, `undoing the ${entries} entries gives the document before`);
      for (let i = 0; i < entries; i++) await editor.redo();
      assert.equal(text(editor), after, `redoing the ${entries} entries gives the document after`);
      assert.deepEqual(Object.keys(owner.refused ?? {}), [], 'nothing was refused');
    } finally { owner.release(); }
  });
}
