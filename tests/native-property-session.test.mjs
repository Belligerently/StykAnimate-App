// The inspector's native-field gestures on an editor the engine owns: each is a session call or a gesture of the session's, and what the
// session has no command for is refused, never written into the document by the inspector.
import test from 'node:test';
import assert from 'node:assert/strict';
import {emptyDocument, createFrame, createObject} from '../src/demo.mjs';
import {beginNativePropertyEdit, previewNativePropertyEdit, commitNativePropertyEdit, cancelNativePropertyEdit, getNativePropertyValue} from '../src/native-property-edit.mjs';
import {engineBuilt} from './support/engine-node.mjs';
import {ownedEditorSync} from './support/owned-editor.mjs';

const sheet = {demo: false, layerTimeline: true, propertyChannels: true};
function twoFigures(options = sheet) {
  const report = emptyDocument(), figure = report.figure_types[0];
  report.keyframes = [0, 10].map(x => createFrame([createObject(figure, 1, 150 + x, 200), createObject(figure, 2, 350 + x, 200)], 10));
  const {editor, owner} = ownedEditorSync(options, report, {recorded: false});
  return {editor, owner};
}
const notSupported = error => error.messageKey === 'errors.engineOwner.notSupported';

test('the project fields are one command of the session when the edit ends', {skip: !engineBuilt}, () => {
  const {editor} = twoFigures();
  for (const [key, read, value] of [['fps', () => editor.report.fps, 30], ['canvas_width', () => editor.report.canvas_width, 800]]) {
    const before = read(), entries = editor._undo.length;
    const token = beginNativePropertyEdit(editor, key);
    previewNativePropertyEdit(token, value + 1); previewNativePropertyEdit(token, value);
    assert.equal(read(), before, 'nothing shows in the document until the edit ends');
    assert.equal(editor._undo.length, entries);
    assert.equal(commitNativePropertyEdit(token) !== false, true);
    assert.equal(read(), value);
    assert.equal(editor._undo.length, entries + 1, 'one entry');
    editor.undo();
    assert.equal(read(), before);
  }
  const token = beginNativePropertyEdit(editor, 'fps');
  previewNativePropertyEdit(token, 12);
  cancelNativePropertyEdit(token);
  assert.equal(editor.report.fps, 24, 'a cancelled edit changes nothing');
  assert.equal(editor.canUndo, false);
  assert.throws(() => { const next = beginNativePropertyEdit(editor, 'fps'); previewNativePropertyEdit(next, 0); }, /from 1 to 1000/);
});

test('a selection size is scaled once when the edit ends, and the opacity of several figures follows the slider', {skip: !engineBuilt}, () => {
  const {editor} = twoFigures();
  editor.selectMany([1, 2]);
  const start = getNativePropertyValue(editor, 'group_width'), objects = () => editor.evaluatedFrame.objects.map(object => object.scale);
  const token = beginNativePropertyEdit(editor, 'group_width');
  for (let n = 0; n < 40; n++) previewNativePropertyEdit(token, start + n);
  previewNativePropertyEdit(token, start * 2);
  assert.deepEqual(objects(), [1, 1], 'the size shows when the edit ends');
  const entries = editor._undo.length;
  commitNativePropertyEdit(token);
  assert.equal(editor._undo.length, entries + 1);
  assert.ok(objects().every(scale => Math.abs(scale - 2) < 1e-9), `doubled: ${objects()}`);
  editor.undo();
  assert.deepEqual(objects(), [1, 1]);

  const opacity = beginNativePropertyEdit(editor, 'figure_opacity');
  previewNativePropertyEdit(opacity, 40);
  assert.ok(editor.evaluatedFrame.objects.every(object => object.field28 === Math.round(0.6 * 255)), 'the preview shows on every selected figure');
  assert.equal(editor.canUndo, false, 'a preview is no entry');
  commitNativePropertyEdit(opacity);
  assert.equal(editor.undoLabel, 'Figure opacity');
  editor.undo();
  assert.ok(editor.evaluatedFrame.objects.every(object => object.field28 === 0));
});

test('what the session has no command for is refused when the edit begins, and changes nothing', {skip: !engineBuilt}, () => {
  const {editor, owner} = twoFigures();
  editor.select(1, -1);
  const text = JSON.stringify(editor.report);
  // The stacking number of a figure, and a pose at one value across several frames.
  assert.throws(() => beginNativePropertyEdit(editor, 'figure_z_order'), notSupported);
  const frames = [editor.frameIndex]; frames.sheetTicks = [0, 10];
  assert.throws(() => beginNativePropertyEdit(editor, 'root_x', {indices: frames}), notSupported);
  assert.equal(editor.dragging, false, 'no gesture was left open');
  assert.equal(JSON.stringify(editor.report), text);
  assert.deepEqual(owner.refused, {});
  // The definition of a figure: written by the commands of the engine, never in place.
  const native = twoFigures({demo: false});
  native.editor.select(1, 0);
  const before = JSON.stringify(native.editor.report);
  assert.throws(() => beginNativePropertyEdit(native.editor, 'outline_width'), notSupported);
  assert.throws(() => beginNativePropertyEdit(native.editor, 'thickness'), notSupported);
  assert.equal(JSON.stringify(native.editor.report), before);
});
