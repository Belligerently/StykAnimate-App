// Editors the engine does not own, for the tests of the JavaScript editor's own bodies: native timelines, working frames and
// the calls the app cannot make on a layer sheet. Under the owner (D19) the session refuses those calls and the editor never
// runs them, so these tests keep their subject here until the editor's bodies are deleted with them.
import {Editor} from '../../src/editor.mjs';

/** Call once at the top of a test file. `open(options, document)` is a new `Editor` (on `document` when given, else on the
 * document it starts with). */
export function plainScope() {
  return (options, document = null) => {
    const editor = new Editor(options);
    if (document) editor.load(document);
    return editor;
  };
}

/** An editor on a copy of `document` (the blank one the Editor opens with when there is none). */
export function plainEditorFor(options, document) {
  const editor = new Editor(options);
  if (document) editor.load(structuredClone(document));
  return editor;
}

/** An editor with no document loaded, before the test opens one. */
export const newPlainEditor = options => new Editor(options);

/** [`plainEditorFor`] in the shape of the owned editors' helper: no owner, nothing draws from a pool. */
export function plainEditorSync(options, document) {
  return {editor: plainEditorFor(options, document), owner: null, setDraws() {}};
}

export async function plainEditor(options, document) {
  return plainEditorSync(options, document);
}
