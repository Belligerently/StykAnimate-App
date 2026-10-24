// The editing commands that used to run in the editor now run in the engine session (D19): what the interface reads of
// each after the call, the errors it shows and what undo takes back. The expectations are stated here; nothing is
// compared with the editor alone.
import '../engine/tools/v8-pin.mjs';
import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {engineBuilt} from './support/engine-node.mjs';
import {ownedEditor} from './support/owned-editor.mjs';

const folder = new URL('../engine/tests/edit/', import.meta.url);
/** An owned editor on the first document of a recorded tape. */
async function owned(name) {
  const scenario = JSON.parse(readFileSync(new URL(`${name}.scenario.json`, folder), 'utf8'));
  const document = JSON.parse(readFileSync(new URL(scenario.document, folder), 'utf8'));
  const made = await ownedEditor({demo: false, ...scenario.editor}, document, {recorded: false});
  made.editor.autoKey = Boolean(scenario.editor?.autoKey);
  return made;
}
const shown = (editor, uid = 1) => editor.evaluatedFrame.objects.find(object => object.unique_id === uid);

test('perspective and part proportions of a 3D figure run in the session and undo with it', {skip: !engineBuilt}, async () => {
  const {editor, owner} = await owned('recolor-mesh-sheet');
  editor.select(1, -1);
  assert.equal(editor.setSpatialCamera({strength: 0.4}), true);
  assert.equal(shown(editor)._stykSpatial.camera.strength, 0.4);
  assert.equal(editor.undoLabel, 'Adjust perspective');
  assert.equal(editor.setSpatialCamera({strength: 0.4}), false, 'the same value changes nothing');
  assert.throws(() => editor.setSpatialCamera({strength: 2}), error => error.messageKey === 'errors.spatialProjection.perspectiveNeedsStrength01AndDistance10');
  assert.equal(shown(editor)._stykSpatial.camera.strength, 0.4, 'a refused value leaves the figure alone');
  editor.select(1, 5);
  assert.equal(editor.setSpatialProportions({width: 2, taper: 0.5}), true);
  assert.deepEqual(shown(editor)._stykSpatial.shape[6], {width: 2, taper: 0.5});
  assert.equal(editor.undoLabel, 'Adjust rig proportions');
  assert.throws(() => editor.setSpatialProportions({width: 9}), error => error.messageKey === 'errors.editorSpatialShape.chooseProportionsWithinTheControlRange');
  assert.equal(editor.resetSpatialProportions(), true);
  assert.deepEqual(shown(editor)._stykSpatial.shape[6], {width: 1, taper: 0});
  editor.undo();
  assert.deepEqual(shown(editor)._stykSpatial.shape[6], {width: 2, taper: 0.5});
  editor.undo(); editor.undo();
  assert.equal(shown(editor)._stykSpatial?.camera?.strength, 0.5, 'back to the perspective the document had');
  assert.equal(owner.fallback, 0, 'no call ran in the editor');
});

test('a scene background is imported and named in the session; each is one history entry', {skip: !engineBuilt}, async () => {
  const {editor, owner} = await owned('add-figure-sheet');
  const before = editor.report.backgrounds?.length ?? 0;
  const asset = editor.importSceneBackground({kind: 2, color: 0xff3366ff, name: ''});
  assert.equal(asset.name, 'Background', 'an unnamed background is called Background');
  assert.equal(editor.report.backgrounds.length, before + 1);
  assert.equal(editor.undoLabel, 'Import scene background');
  assert.equal(editor.evaluatedFrame.background_id, asset.background_id, 'the chosen frame shows it');
  assert.throws(() => editor.importSceneBackground({kind: 9}), error => error.messageKey === 'errors.editorScene.expectedADecodedImageColorOrGradientBackground');
  assert.equal(editor.renameSceneBackground(asset.background_id, '  Sky  '), true);
  assert.equal(editor.report.backgrounds.at(-1).name, 'Sky', 'the name is trimmed');
  assert.equal(editor.undoLabel, 'Rename background');
  assert.equal(editor.renameSceneBackground(asset.background_id, 'Sky'), false, 'the same name changes nothing');
  assert.throws(() => editor.renameSceneBackground(asset.background_id, ''), error => error.messageKey === 'errors.editorScene.backgroundNamesMustContain1160Characters');
  assert.throws(() => editor.renameSceneBackground(99, 'x'), error => error.messageKey === 'errors.editorScene.chooseAnExistingBackground');
  editor.undo();
  assert.equal(editor.report.backgrounds.at(-1).name, 'Background');
  editor.undo();
  assert.equal(editor.report.backgrounds.length, before, 'one undo takes the background and the frame showing it');
  assert.equal(editor.evaluatedFrame.background_id, 0);
  editor.redo(); editor.redo();
  assert.equal(editor.report.backgrounds.at(-1).name, 'Sky');
  assert.equal(owner.fallback, 0);
});

test('the exposure under the playhead is set in the session', {skip: !engineBuilt}, async () => {
  const {editor, owner} = await owned('add-figure-sheet');
  const length = () => { const {start, end} = editor.sheetSpan(); return end - start; };
  const first = length();
  assert.equal(editor.setFrameDuration(first + 3), true);
  assert.equal(length(), first + 3);
  assert.equal(editor.setFrameDuration(first + 3), false, 'the same length changes nothing');
  assert.throws(() => editor.setFrameDuration(0), error => error.messageKey === 'errors.editorLayerSheet.exposureMustBe165535Frames');
  editor.undo();
  assert.equal(length(), first);
  assert.equal(owner.fallback, 0);
});
