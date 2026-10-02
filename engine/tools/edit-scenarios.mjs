import {writeFileSync} from 'node:fs';
// Builds the first command-replay scenarios (engine/tests/edit/) with the editor's own fixture kit.
//   node engine/tools/edit-scenarios.mjs && node engine/tools/edit-replay.mjs --all
import {documentOf, limb, open, nextKey, prune} from './fixture-kit.mjs';
const out = new URL('../tests/edit', import.meta.url).pathname;
// A native-timeline document with two limbs (one joined to the other) over two keyframes.
const doc = documentOf([limb(true), limb()], i => [250 + i * 150, 160]);
writeFileSync(`${out}/two-limbs.document.json`, JSON.stringify(doc) + '\n');
// A chain of three figures (3 joined to 2's shin, 2 joined to 1's foot) over two keyframes.
const chain = open(documentOf([limb(true), limb(), limb()], i => [200 + i * 140, 170]));
chain.select(2); chain.joinTo(1, 33); chain.select(3); chain.joinTo(2, 22);
nextKey(chain, 6); chain.select(1); chain.moveSelection(40, 0);
writeFileSync(`${out}/joined-chain.document.json`, JSON.stringify(prune(chain.report), (key, value) => key === 'runtime_segments' ? undefined : value) + '\n');
const scenario = (name, editor, steps, document = 'two-limbs.document.json') => writeFileSync(`${out}/${name}.scenario.json`, JSON.stringify({name, document, editor, steps}, null, 1) + '\n');
scenario('move-native', {}, [
  ['@select', 1, -1], ['moveSelection', 30, -12], ['moveSelection', -5, 4], ['undo'], ['redo'],
  ['@select', 2, -1], ['beginDrag', 'Move figures'], ['moveSelection', 10, 10], ['moveSelection', 2, 0], ['commitDrag'], ['undo'],
]);
scenario('rotate-native', {}, [
  ['@select', 1, -1], ['rotateSelection', 0.4], ['rotateSelection', -0.1], ['undo'], ['undo'], ['redo'],
]);
scenario('pose-native', {}, [
  ['@select', 1, 1], ['poseJoint', 330, 220], ['@select', 1, 2], ['poseJoint', 360, 260, {stretch: true}], ['undo'], ['redo'],
]);
scenario('move-sheet', {layerTimeline: true, propertyChannels: true}, [
  ['@select', 1, -1], ['moveSelection', 20, 5], ['@seek', 6], ['moveSelection', 40, -10], ['@seek', 3], ['undo'], ['undo'], ['redo'],
]);
scenario('move-joined', {}, [
  ['@seek', 0], ['@select', 3, -1], ['moveSelection', 15, 5], ['undo'], ['redo'], ['@select', 2, -1],
  ['moveSelection', 10, 0, {detachExternal: true}], ['undo'], ['@seek', 6], ['@select', 1, -1], ['moveSelection', -20, 8], ['undo'], ['undo'], ['redo'],
], 'joined-chain.document.json');
scenario('drag-cancel', {}, [
  ['@select', 2, -1], ['beginTranslationDrag'], ['moveSelection', 10, 0, {fromStart: true}], ['moveSelection', 25, -5, {fromStart: true}], ['cancelDrag'],
  ['beginDrag', 'Pose figure'], ['moveSelection', 0, 0], ['commitDrag'], ['beginDrag', 'Pose figure', {scope: 'translation'}], ['moveSelection', 4, 4], ['@seek', 6], ['undo'],
  ['@select', 3, -1], ['beginDrag'], ['moveSelection', 5, 5], ['undo'], ['redo'],
], 'joined-chain.document.json');
scenario('selection-native', {}, [
  ['@select', 99, -1], ['moveSelection', 5, 5], ['undo'], ['redo'], ['@select', 1, 2], ['@select', 2, {add: true}], ['moveSelection', 3, 3],
  ['@select', 1, {toggle: true}], ['selectMany', [2, 1, 7]], ['moveSelection', -3, 1], ['@seek', -4], ['@seek', 100], ['@seek', 2.5], ['moveSelection', 1, 1],
  ['moveSelection', null, 1], ['undo'], ['undo'], ['undo'], ['undo'],
]);
scenario('transform-native', {}, [
  ['@select', 1, -1], ['scaleSelection', 1.5], ['mirrorSelection', 'vertical'], ['mirrorSelection', 'horizontal', {center: {x: 300, y: 200}}],
  ['rotateFigure', 0.3], ['centerFigure'], ['resetPose'], ['selectMany', [1, 2]], ['rotateSelection', 0.2], ['scaleSelected', 0.5],
  ['scaleSelection', 1.25, {selectedOnly: true}], ['rotateFigure', -0.2], ['centerFigure'], ['scaleSelection', -1], ['mirrorSelection', 'diagonal'],
  ['undo'], ['undo'], ['undo'], ['redo'], ['@select', 1, 1], ['mirrorSelection'],
]);
scenario('transform-joined', {}, [
  ['@seek', 3], ['@select', 2, -1], ['rotateSelection', 0.5], ['selectMany', [1, 3]], ['scaleSelection', 2], ['mirrorSelection', 'horizontal'],
  ['@select', 3, -1], ['rotateFigure', 0.7], ['scaleSelection', 0.5], ['centerFigure'], ['@seek', 6], ['@select', 2, -1], ['resetPose'],
  ['beginDrag', 'Pose figure'], ['rotateSelection', 0.1], ['rotateSelection', 0.1], ['cancelDrag'], ['undo'], ['undo'], ['redo'],
], 'joined-chain.document.json');
scenario('pose-joints', {}, [
  ['@select', 1, 0], ['poseJoint', 300, 260], ['rotateJoint', 0.3], ['rotateJoint', -0.1, {isolate: true}], ['@select', 1, 1],
  ['bendJoint', 320, 250], ['bendJoint', 330, 240, {stretch: true}], ['poseJoint', 340, 250, {stretch: true, snap: false}],
  ['beginDrag', 'Pose figure'], ['poseJoint', 330, 270], ['poseJoint', 320, 280, {stretch: true}], ['poseJoint', 318, 279, {stretch: true}], ['commitDrag'],
  ['@select', 1, 1], ['poseJoint', 300, 300, {stretch: true}], ['poseJoint', 305, 300, {stretch: true, snapTolerance: 40}], ['bendJoint', 310, 300, {snapTolerance: 50}],
  ['poseJoint', 1, 2, {snapTolerance: -1}], ['@select', 1, -1], ['poseJoint', 300, 300], ['bendJoint', 300, 300], ['rotateJoint', 1],
  ['@select', 2, 2], ['bendJoint', 420, 260], ['poseJoint', 440, 250], ['undo'], ['undo'], ['undo'], ['redo'],
]);
scenario('pose-joined', {}, [
  ['@select', 1, 2], ['rotateJoint', 0.4], ['poseJoint', 250, 300], ['rotateJoint', 0.2, {isolate: true}], ['@select', 2, 1], ['poseJoint', 300, 380],
  ['bendJoint', 290, 390, {stretch: true}], ['@seek', 3], ['@select', 1, 1], ['poseJoint', 230, 310], ['@select', 3, 0], ['poseJoint', 260, 420, {stretch: true}],
  ['undo'], ['undo'], ['undo'], ['redo'], ['@seek', 6], ['@select', 1, 0], ['rotateJoint', -0.5], ['undo'],
], 'joined-chain.document.json');
// Snapping back to the definition: pointers placed from the editor's own geometry, step by step.
{
  const probe = open(structuredClone(doc)), steps = [];
  const run = step => { steps.push(step); const [method, ...args] = step; probe[method === '@select' ? 'select' : method](...args); };
  const shin = () => probe.geometry.objects.get(0).segments[1];
  const along = (length, turn = 0, side = 0) => {
    const s = shin(), a = s.angle + turn;
    return [s.start.x + length * Math.cos(a) - side * Math.sin(a), s.start.y + length * Math.sin(a) + side * Math.cos(a)];
  };
  run(['@select', 1, 1]);
  run(['poseJoint', ...along(120), {stretch: true}]);          // stretched: the next gesture may snap back
  run(['poseJoint', ...along(93), {stretch: true}]);           // 3 px from 90: snaps to the definition
  run(['poseJoint', ...along(120), {stretch: true}]);
  run(['poseJoint', ...along(93), {stretch: true, snap: false}]); // no snapping
  run(['poseJoint', ...along(99), {stretch: true}]);           // 6 px away after the last: within tolerance? recorded either way
  run(['bendJoint', ...along(90, 0.4)]);                        // bent
  run(['bendJoint', ...along(probe.object.lengths?.[1] ?? 90, 0, 2)]); // 2 px from the straight end: snaps to 0
  run(['bendJoint', ...along(90, 0.3)]);
  run(['bendJoint', ...along(140, 0.02, 3), {stretch: true}]);  // stretching bend, near the straight line at the pointer's distance
  run(['beginDrag', 'Pose figure']);
  run(['poseJoint', ...along(130), {stretch: true}]);
  run(['poseJoint', ...along(91.5), {stretch: true, snapTolerance: 2}]);
  run(['commitDrag']);
  run(['undo']); run(['undo']); run(['redo']);
  scenario('pose-snap', {}, steps);
}
