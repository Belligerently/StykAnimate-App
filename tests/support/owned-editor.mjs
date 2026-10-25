// An editor the engine owns, for tests (D19): its document opened on the engine, which also holds the session that edits it.
import assert from 'node:assert/strict';
import {afterEach} from 'node:test';
import {Editor} from '../../src/editor.mjs';
import {installEngineOwner} from '../../src/engine/engine-owner.mjs';

/** The owners of the editors the running test made: released when it ends, so no session outlives its test. */
const open = new Set();
afterEach(() => { for (const owner of open) owner.release(); open.clear(); });

/** The editor on `document` (the blank one the Editor opens with when there is none) with the engine owning it (the engine must be
 * registered: `tests/support/register-engine.mjs`). The owner is released when the test ends.
 * `verify`: the session's document is compared with the editor's after every call.
 * `recorded`: the identities a call draws come from the test (`setDraws`), else from the editor's own random ones. */
export function ownedEditorSync(options, document, {verify = true, recorded = true} = {}) {
  const editor = new Editor(options);
  if (document) editor.load(structuredClone(document));
  let pool = [];
  const owner = installEngineOwner(editor, {verify, draws: recorded ? () => pool : null});
  open.add(owner);
  // The session exists from the moment the document opens: the app starts it as it loads the document.
  assert.ok(owner.ready(), 'the document has its session');
  return {editor, owner, setDraws: draws => { pool = draws; }};
}

/** [`ownedEditorSync`] for a test that acts on the editor alone: the identities a call draws are the editor's own random ones. */
export const ownedEditorFor = (options, document) => ownedEditorSync(options, document, {recorded: false}).editor;

/** [`ownedEditorSync`] for tests that wait for it. */
export async function ownedEditor(options, document, settings) {
  return ownedEditorSync(options, document, settings);
}

/** An editor the engine does not own, on a document with drawing layers (one per name) that the engine session made on the way: the
 * structure of a drawing layer is the session's, and the tests of what stays the editor's own (its history, inspectors, focus,
 * painting) start from the result. The last layer is the active one, as after creating it. */
export function plainEditorWithDrawings(options, document, names = [undefined]) {
  const owned = new Editor(options);
  if (document) owned.load(structuredClone(document));
  const owner = installEngineOwner(owned);
  assert.ok(owner.ready(), 'the document has its session');
  const made = names.map(name => owned.createDrawingLayer(name));
  const editor = new Editor(options).load(structuredClone(owned.report));
  owner.release();
  const layers = made.map(layer => editor.getLayer(layer.id));
  editor.selectSheetLayer(layers.at(-1).id);
  return {editor, layers};
}

/** The editor the engine owns, for the tests of what an editor does with its calls: the identities a call draws are the engine's own. */
export const engineEditor = (options, document, settings) => ownedEditorSync(options, document, {recorded: false, ...settings}).editor;
