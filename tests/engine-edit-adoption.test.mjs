// Owned response adoption against the real editor, including compact drafts and
// retained previews. Rust's session tests independently verify the wire deltas.
import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {Editor} from '../src/editor.mjs';
import {installEditShadow} from '../src/engine/engine-edit-shadow.mjs';
import {withDraws} from '../src/editor-draws.mjs';

const json = value => JSON.stringify(value, (key, item) => key === 'runtime_segments' || key === '_movieclipSample' ? undefined : item);
const clean = value => JSON.parse(json(value));
const state = editor => ({tick: editor.tick, frameIndex: editor.frameIndex, selectedUid: editor.selectedUid ?? null,
  selectedUids: [...editor.selectedUids], selectedJoint: editor.selectedJoint ?? -1, canUndo: editor.canUndo, canRedo: editor.canRedo,
  undoLabel: editor.undoLabel, redoLabel: editor.redoLabel, dirty: editor.dirty, layerRevision: editor.layerRevision,
  nextFrameIdentity: editor._nextFrameIdentity, nextUid: editor._nextUid});
const runtimeClock = key => ['_cameraMotionTick', '_vfxTimelineTick', '_vfxTimelineScene'].includes(key);
const decode = value => Array.isArray(value) ? value.map(decode) : value === null || typeof value !== 'object' ? value
  : Object.hasOwn(value, '@array') ? Object.assign(decode(value['@array']), Object.fromEntries(Object.entries(value).filter(([key]) => key !== '@array')))
  : Object.fromEntries(Object.entries(value).map(([key, item]) => [key, decode(item)]));

function sessionFor(oracle) {
  const events = [];
  oracle.addEventListener('change', event => events.push(clean(event.detail)));
  return {
    revision: () => oracle.revision, can_undo: () => oracle.canUndo, can_redo: () => oracle.canRedo, free() {},
    call(name, argsText, viewText) {
      const view = JSON.parse(viewText), beforeSheet = oracle.report._styk.layerSheet, beforeLayers = oracle.report._styk.layers;
      const beforeDefinitions = oracle.report.figure_types.length;
      const beforeFrame = clean(oracle.evaluatedFrame);
      const beforeObjects = new Map(beforeFrame.objects.map(object => [object.unique_id, json(object)]));
      events.length = 0;
      let result, error;
      const draws = view.draws?.slice() ?? [];
      try { ({result} = withDraws(() => oracle[name](...JSON.parse(argsText).map(decode)), () => draws.shift()[1])); }
      catch (caught) { error = caught.messageKey ?? caught.message; }
      const answer = {status: error ? 'error' : 'ok', ...(error ? {error} : {result: result ?? null}), state: state(oracle), events: events.slice(),
        draft: null, drag: oracle._drag ? {before: oracle._drag.before.map(object => object.unique_id), translation: oracle._drag.scope === 'translation', label: oracle._drag.label} : null};
      // View return values have no effect on owned adoption and can carry runtime state.
      if (['select', 'selectMany', 'seek', 'duplicateFigure', 'addFigure', 'importFigure', 'sheetAppendFrame', 'copySegmentValues', 'sheetCopyFrames', 'beginAnimationEdit'].includes(name)) answer.result = null;
      const frame = oracle._authoredPoseEdit?.frame;
      if (frame && oracle._drag) {
        const taken = new Set(oracle._drag.before.map(object => object.unique_id));
        answer.draft = frame.objects.filter(object => taken.has(object.unique_id) && (!view.changesOnly || beforeObjects.get(object.unique_id) !== json(object))).map(clean);
      }
      if (events.some(event => event.layerSheet)) {
        const sheet = oracle.report._styk.layerSheet;
        answer.sheet = view.changesOnly ? Object.fromEntries(Object.entries(sheet).filter(([key, value]) => key !== 'tracks' && beforeSheet[key] !== value)) : {...sheet};
        answer.sheet.tracks = Object.fromEntries(Object.entries(sheet.tracks).filter(([id, track]) => beforeSheet.tracks[id] !== track));
        if (!view.changesOnly || json(Object.keys(beforeSheet.tracks)) !== json(Object.keys(sheet.tracks))) answer.trackOrder = Object.keys(sheet.tracks);
        if (view.changesOnly) {
          const removed = Object.keys(beforeSheet).filter(key => !Object.hasOwn(sheet, key));
          if (removed.length) answer.sheetRemoved = removed;
          if (json(Object.keys(beforeSheet)) !== json(Object.keys(sheet))) answer.sheetOrder = Object.keys(sheet);
        }
        if (!view.changesOnly || beforeLayers !== oracle.report._styk.layers) answer.layers = oracle.report._styk.layers;
        answer.sources = Object.fromEntries(events.flatMap(event => event.sourceIds ?? []).map(id => [id, oracle.report._styk.frameSources[id] ?? null]));
        if (oracle.report.figure_types.length > beforeDefinitions) answer.definitions = oracle.report.figure_types.slice(beforeDefinitions);
        const retained = oracle._evaluated;
        if (retained?.objects) {
          answer.retainedFields = Object.fromEntries(Object.entries(retained).filter(([key, value]) => key !== 'objects' && !runtimeClock(key) && json(beforeFrame[key]) !== json(value)));
          answer.retainedRemoved = Object.keys(beforeFrame).filter(key => !Object.hasOwn(retained, key));
          answer.retainedFieldOrder = Object.keys(retained).filter(key => !runtimeClock(key));
          answer.retained = retained.objects.filter(object => !view.changesOnly || beforeObjects.get(object.unique_id) !== json(object)).map(clean);
          if (view.changesOnly) {
            answer.retainedChanges = true;
            const order = retained.objects.map(object => object.unique_id);
            if (json([...beforeObjects.keys()]) !== json(order)) answer.retainedOrder = order;
          }
        }
      }
      if (view.changesOnly) answer.changesOnly = true;
      if ((name === 'previewAnimationEdit' || name === 'updateCameraDrag') && oracle._sheetPropertyPreview) answer.preview = oracle._sheetPropertyPreview;
      return json(answer);
    },
  };
}

for (const name of ['gestures-sheet', 'layers-sheet', 'figures-layers-sheet', 'figure-clipboard-sheet', 'figure-clipboard-clip-sheet',
  'movieclip-sheet', 'movieclip-loose-sheet', 'channels-figure-sheet', 'channels-movieclip-sheet', 'property-clip-patches-sheet', 'locked-sheet', 'topology-sheet', 'topology-native', 'move-native', 'drag-cancel', 'camera-motion-handles-sheet', 'vfx-stack-sheet']) {
  test(`compact owned adoption matches the editor after each step: ${name}`, async () => {
    const folder = new URL('../engine/tests/edit/', import.meta.url);
    const scenario = JSON.parse(readFileSync(new URL(`${name}.scenario.json`, folder), 'utf8'));
    const document = typeof scenario.document === 'string' ? JSON.parse(readFileSync(new URL(scenario.document, folder), 'utf8')) : scenario.document;
    const options = {demo: false, ...scenario.editor};
    const expected = new Editor(options).load(structuredClone(document));
    const owned = new Editor(options).load(structuredClone(document));
    const oracle = new Editor(options).load(structuredClone(document));
    for (const editor of [expected, owned, oracle]) editor.autoKey = Boolean(scenario.editor?.autoKey);
    const session = sessionFor(oracle);
    const report = installEditShadow(owned, () => ({engine: {handle: {edit_session: () => session}}, mirror: {framesCurrent: () => true, syncFrames: async () => {}}}), {mode: scenario.editor?.layerTimeline ? 'own' : 'shadow'});
    let drawn = 0;
    const source = kind => { drawn++; return kind === 'uuid' ? `d0d0d0d0-0000-4000-8000-${String(drawn).padStart(12, '0')}` : kind === 'seed' ? Math.imul(drawn, 0x9e3779b1) >>> 0 : (Math.imul(drawn, 0x9e3779b1) >>> 0) / 2 ** 32; };
    for (const [index, call] of scenario.steps.entries()) {
      const [method, ...args] = call;
      const run = editor => editor[method.replace(/^@/, '')](...args.map(decode));
      let expectedError, ownedError, draws;
      try { ({draws} = withDraws(() => run(expected), source)); } catch (error) { expectedError = error.messageKey ?? error.message; draws = error.draws ?? []; }
      let at = 0;
      try { withDraws(() => run(owned), () => draws[at++][1]); } catch (error) { ownedError = error.messageKey ?? error.message; }
      const label = `${name} step ${index + 1}: ${method}`;
      assert.equal(ownedError, expectedError, `${label}: error`);
      assert.equal(json(owned.report), json(expected.report), `${label}: document`);
      assert.deepEqual(state(owned), state(expected), `${label}: history and selection`);
      assert.equal(json(owned.evaluatedFrame), json(expected.evaluatedFrame), `${label}: frame`);
      assert.equal(report.mismatches.length, 0, `${label}: shadow comparisons`);
      await Promise.resolve();
    }
    if (scenario.editor?.layerTimeline) assert.ok(report.owned > 0 || name === 'topology-sheet', 'the response was adopted, not run in the JavaScript editor');
    else assert.equal(report.owned, 0, 'native timeline sessions are compared without authored adoption');
  });
}

test('a dense pose and its commit preserve untouched figures and movieclip samples', () => {
  const reference = JSON.parse(readFileSync(new URL('../engine/tests/edit/gestures-sheet.reference.json', import.meta.url), 'utf8'));
  const document = JSON.parse(reference.steps[0].document);
  // Dense numeric pose arrays and extra independent figures expose whole-frame adoption.
  const template = document.figure_types[0], base = template.segments[0];
  template.segments = Array.from({length: 220}, (_, index) => ({...base, segment_id: index + 1, parent_id: 0, length: 20, angle_rad: index * .05}));
  template.segment_count = template.segments.length;
  document.figure_types = Array.from({length: 6}, (_, index) => ({...structuredClone(template), figure_type_id: index + 1}));
  const object = document.keyframes[0].objects[0];
  document.keyframes[0].objects = document.figure_types.map((figure, slot) => ({...structuredClone(object), unique_id: slot + 1,
    object_slot: slot, figure_type_id: figure.figure_type_id, angles: figure.segments.map(segment => segment.angle_rad), join: null, root_x: 100 + slot * 60, root_y: 100}));
  document.keyframes = [document.keyframes[0]];
  document.keyframes[0].object_draw_order = [0, 1, 2, 3, 4, 5];
  document.keyframes[0].object_draw_order_unique_ids = [1, 2, 3, 4, 5, 6];
  // Start from the native data; loading initializes ownership and the layer sheet.
  delete document._styk.layerSheet; delete document._styk.layers; delete document._styk.frameSources;
  const editors = Array.from({length: 3}, () => new Editor({demo: false, propertyChannels: true, layerTimeline: true}).load(structuredClone(document)));
  const [expected, owned, oracle] = editors;
  const session = sessionFor(oracle);
  const report = installEditShadow(owned, () => ({engine: {handle: {edit_session: () => session}}, mirror: {framesCurrent: () => true, syncFrames: async () => {}}}), {mode: 'own'});
  for (const editor of [expected, owned]) { editor.select(1, 5); editor.beginDrag(); }
  const untouched = owned._authoredPoseEdit.frame.objects.slice(1);
  // Runtime clocks are deliberately absent from Rust's records and must survive adoption.
  untouched[2]._movieclipSample = {tick: 7, localTick: 2};
  for (const editor of [expected, owned]) editor.poseJoint(130, 120);
  assert.equal(json(owned._authoredPoseEdit.frame), json(expected._authoredPoseEdit.frame));
  assert.ok(untouched.every((object, index) => owned._authoredPoseEdit.frame.objects[index + 1] === object));
  for (const editor of [expected, owned]) editor.commitDrag();
  assert.equal(json(owned.report), json(expected.report));
  assert.deepEqual(state(owned), state(expected));
  assert.ok(untouched.every((object, index) => owned.evaluatedFrame.objects[index + 1] === object));
  assert.deepEqual(owned.evaluatedFrame.objects[3]._movieclipSample, {tick: 7, localTick: 2});
  assert.ok(report.owned >= 2);
  assert.equal(report.mismatches.length, 0);
});
