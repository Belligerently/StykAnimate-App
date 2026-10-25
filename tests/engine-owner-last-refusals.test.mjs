// The calls the engine session used to refuse for a document or a selection (and the editor ran instead) now run in the
// session: each leaves the document, the history entries and what undo and redo give back as the editor alone does
// (tests/support/owned-vs-plain.mjs), and nothing is refused or run in the editor.
import '../engine/tools/v8-pin.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import {engineBuilt} from './support/engine-node.mjs';
import {bothWays, recordedDocument} from './support/owned-vs-plain.mjs';

const opts = {skip: !engineBuilt};
/** The clipboard the interface reads, as text with its keys in order (the owned editor's mirror lists them differently). */
const clipboardText = editor => JSON.stringify(editor._selectionClipboard, (key, value) => {
  if (value instanceof Set || value instanceof Map) return [...value];
  return value && typeof value === 'object' && !Array.isArray(value) ? Object.fromEntries(Object.entries(value).sort(([a], [b]) => a < b ? -1 : 1)) : value;
});

test('copying, cutting or duplicating a selected branch makes a figure of its own', opts, async () => {
  for (const [name, uid, joint] of [['origin-join-layout-sheet', 1, 0], ['origin-join-layout-sheet', 1, 1], ['sheet-layers', 1, 0], ['sheet-layers', 1, 3], ['sheet-layers', 2, 0], ['branch-thickness-origin-sheet', 1, 3]]) {
    for (const how of ['copy', 'cut', 'duplicate']) {
      // A figure keeps at least one segment: cutting all of them is refused (the same in both editors).
      if (how === 'cut' && [['origin-join-layout-sheet', 0], ['sheet-layers', 0], ['sheet-layers', 2]].some(([n, j]) => n === name && j === joint && uid !== 3)) continue;
      const {owned} = await bothWays(recordedDocument(name), editor => {
        const before = editor.report.figure_types.length;
        editor.selectBranch(uid, joint);
        const count = how === 'cut' ? editor.cutSelection() : how === 'copy' ? editor.copySelection() : editor.duplicateSelection().length;
        assert.ok(count >= 1, `${name} ${joint} ${how}: copied something`);
        const held = how === 'duplicate' ? '' : clipboardText(editor);
        if (how !== 'duplicate') editor.pasteSelection();
        const made = editor.report.figure_types.slice(before).find(figure => / branch$/.test(figure.name));
        assert.ok(made, `${name} ${joint} ${how}: the branch is a definition of its own`);
        assert.ok(made.segments.some(segment => segment.parent_id === 0), "its first joint hangs from the figure origin");
        return held;
      }, {expectEntries: how === 'cut' ? 2 : 1});
      assert.ok(owned.result === '' || owned.result.includes('freshTypes'), 'the clipboard names the definition it made');
    }
  }
});

test('a branch whose face is drawn from joints outside it cannot be copied alone', opts, async () => {
  const document = recordedDocument('origin-join-layout-sheet');
  const [fill] = document.figure_types[0].polyfills;
  fill.segment_ids = fill.segment_ids_raw = [44, 55, 66];
  fill._stykSurface.basis = [44, 55, 33];
  const {owned} = await bothWays(document, editor => {
    editor.selectBranch(1, 3);
    try { editor.copySelection(); } catch (error) { return error.messageKey; }
    return 'copied';
  }, {unchanged: true});
  assert.equal(owned.result, 'errors.editorSelection.copyTheWholeFigureToIncludeItsFace');
});

test('the effects of a copied branch go with it to its new definition', opts, async () => {
  const document = recordedDocument('figure-vfx');
  // The figure's pinned limbs name joints the branch may not hold: the editor refuses to paste those.
  for (const frame of document.keyframes) for (const object of frame.objects) delete object._stykIK;
  const {owned} = await bothWays(document, editor => {
    // Effects are kept on a handle ("<definition>/<handle>", "*/<handle>") or on the whole figure.
    const [object] = editor.evaluatedFrame.objects;
    const figure = editor.report.figure_types.find(type => type.figure_type_id === object.figure_type_id);
    const joint = figure.segments.findIndex(segment => Object.keys(object._stykVfx ?? {}).some(key => key.endsWith(`/${segment.segment_id}`)));
    assert.ok(joint >= 0, 'an effect sits on a handle of the figure');
    editor.selectBranch(object.unique_id, joint);
    editor.copySelection();
    editor.pasteSelection();
    const pasted = editor.selectedObjects[0];
    return Object.keys(pasted._stykVfx ?? {}).join(' ');
  }, {expectEntries: 1});
  assert.match(owned.result, /\d+\/\d+/, 'the effect follows to the new definition');
});
