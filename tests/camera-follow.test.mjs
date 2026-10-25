import test from 'node:test';
import assert from 'node:assert/strict';
import {emptyDocument, createFrame, createObject} from '../src/demo.mjs';
import {validateCameraMotion} from '../src/camera-motion.mjs';
import {createCameraMotion} from './support/camera-motion.mjs';
import {ownedEditorSync, ownedEditorFor} from './support/owned-editor.mjs';
import {ownerReferences} from './support/owner-references.mjs';

const refs = ownerReferences(import.meta.url);

const near = (a, b, tolerance, label = '') => assert.ok(Math.abs(a - b) <= tolerance, `${label} ${a} != ${b}`);
/** Figure 1 walks right by 20px per frame for 30 frames, then stands still; figure 2 stays at x=700. */
function fixture(options = {layerTimeline: true, propertyChannels: true}) {
  const report = emptyDocument(), figure = report.figure_types[0];
  report.keyframes = [createFrame([createObject(figure, 1, 100, 250), createObject(figure, 2, 700, 250)], 30),
    createFrame([createObject(figure, 1, 700, 250), createObject(figure, 2, 700, 250)], 30),
    createFrame([createObject(figure, 1, 700, 250), createObject(figure, 2, 700, 250)], 1)];
  const editor = ownedEditorFor({demo: false, ...options}, report);
  if (editor.report._styk?.layerSheet) editor.sheetSetEasing([editor.layerForUid(1)], 0, 60, {interpolation: 'linear'});
  return editor;
}
const params = patch => ({lag: 0, fadeIn: 0, fadeOut: 0, ...patch});
function follow(editor, targets = [{uid: 1, weight: 1}], patch = {}, timing = {start: 0, duration: 60}) {
  const id = editor.addCameraMotion('follow', {...timing, targets});
  editor.setCameraMotion(id, {params: params(patch)}); return id;
}
/** The camera the engine shoots the frame at `tick` with: the authored camera, follow travel, then the presets' impulses. */
const shot = (editor, tick) => { editor.seek(tick); return editor._shown.shot() ?? editor.evaluatedFrame.camera; };

refs.test('baking follow reproduces the live camera and keeps it as travel', take => {
  // Baking is a command of the engine session: the camera it keys on every frame shows what the follow and the shake showed.
  // The document is the plain editor's (its reference): the fixture's figures walking, with a linear easing set on the layer sheet.
  const document = JSON.parse(take('fixture'));
  const editor = ownedEditorSync({demo: false, layerTimeline: true, propertyChannels: true}, document, {recorded: false}).editor;
  editor.sheetSetEasing([editor.layerForUid(1)], 0, 60, {interpolation: 'linear'});
  follow(editor, undefined, {lag: 4});
  editor.addCameraMotion('shake', {start: 0, duration: 60});
  const live = [3, 22, 50].map(tick => ({tick, camera: shot(editor, tick)}));
  assert.equal(editor.bakeCameraMotion({start: 0, end: 60}), true);
  assert.ok(editor.getCameraMotions().every(clip => !clip.enabled), 'the motion is silenced where it was baked');
  for (const {tick, camera} of live) {
    const baked = shot(editor, tick);
    near(baked.x, camera.x, 1e-6); near(baked.y, camera.y, 1e-6); near(baked.scale, camera.scale, 1e-9);
  }
});

test('validation: follow needs figures; older preset clips stay valid', () => {
  assert.throws(() => createCameraMotion('follow', 'a', 0, 10, {targets: []}));
  assert.throws(() => validateCameraMotion({version: 1, clips: [{...createCameraMotion('follow', 'a', 0, 10, {targets: [{uid: 1, weight: 2}]})}]}));
  const shake = createCameraMotion('shake', 'b', 0, 10);
  validateCameraMotion({version: 1, clips: [shake]});
  assert.throws(() => validateCameraMotion({version: 1, clips: [{...shake, targets: [{uid: 1, weight: 1}]}]}));
  assert.throws(() => validateCameraMotion({version: 1, clips: [{...createCameraMotion('follow', 'c', 0, 10, {targets: [{uid: 1, weight: 1}]}), params: {...shake.params}}]}));
  const empty = fixture(); empty.select(null);
  assert.throws(() => empty.addCameraMotion('follow'), /Select the figures/);
  const selected = fixture(); selected.select(1);
  const added = selected.addCameraMotion('follow'), clip = selected.getCameraMotions().find(c => c.id === added);
  assert.deepEqual([clip.targets, clip.start, clip.duration], [[{uid: 1, weight: 1}], 0, 61], 'selected figures, to the end');
});

