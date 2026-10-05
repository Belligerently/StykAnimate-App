// Owned response adoption against the real editor, including compact drafts and
// retained previews. Rust's session tests independently verify the wire deltas.
import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {Editor} from '../src/editor.mjs';
import {editingClass, installEditShadow, OWNED_CLASSES} from '../src/engine/engine-edit-shadow.mjs';
import {withDraws} from '../src/editor-draws.mjs';
import {documentOf} from '../engine/tools/fixture-kit.mjs';
import {createStartupDocument} from '../src/startup-document.mjs';
import {emptyDocument, createFrame, createObject} from '../src/demo.mjs';
import {buildGeometry} from '../src/pivot/geometry.mjs';

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
        draft: null, drag: oracle._drag ? {before: (oracle._drag.before ?? []).map(object => object.unique_id), translation: oracle._drag.scope === 'translation', label: oracle._drag.label} : null};
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
        answer.sources = Object.fromEntries(events.flatMap(event => event.sourceIds ?? []).map(id => [id, oracle.report._styk.frameSources?.[id] ?? null]));
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

for (const mode of ['own', 'classes']) for (const name of mode === 'classes'
  ? ['channels-figure-sheet', 'channels-pose-sheet', 'channels-vfx-sheet', 'channels-movieclip-sheet']
  : ['gestures-sheet', 'layers-sheet', 'figures-layers-sheet', 'figure-clipboard-sheet', 'figure-clipboard-clip-sheet',
  'movieclip-sheet', 'movieclip-loose-sheet', 'channels-figure-sheet', 'channels-movieclip-sheet', 'property-clip-patches-sheet', 'locked-sheet', 'topology-sheet', 'topology-native', 'move-native', 'drag-cancel', 'camera-motion-handles-sheet', 'vfx-stack-sheet']) {
  test(`${mode === 'classes' ? 'default classes compact adoption' : 'compact owned adoption'} matches the editor after each step: ${name}`, async () => {
    const folder = new URL('../engine/tests/edit/', import.meta.url);
    const scenario = JSON.parse(readFileSync(new URL(`${name}.scenario.json`, folder), 'utf8'));
    const document = typeof scenario.document === 'string' ? JSON.parse(readFileSync(new URL(scenario.document, folder), 'utf8')) : scenario.document;
    const options = {demo: false, ...scenario.editor};
    const expected = new Editor(options).load(structuredClone(document));
    const owned = new Editor(options).load(structuredClone(document));
    const oracle = new Editor(options).load(structuredClone(document));
    for (const editor of [expected, owned, oracle]) editor.autoKey = Boolean(scenario.editor?.autoKey);
    if (mode === 'classes') assert.equal(editingClass(owned.report), 'channels', 'saved outer channels precede figure/movieclip classification');
    const originalCalls = {};
    if (mode === 'classes') for (const method of ['moveSelection', 'poseJoint']) {
      const original = owned[method]; originalCalls[method] = 0;
      owned[method] = function (...args) { originalCalls[method]++; return original.apply(this, args); };
    }
    const session = sessionFor(oracle);
    const report = installEditShadow(owned, () => ({engine: {handle: {edit_session: () => session}}, mirror: {framesCurrent: () => true, syncFrames: async () => {}}}), {mode: mode === 'classes' ? 'classes' : scenario.editor?.layerTimeline ? 'own' : 'shadow'});
    if (mode === 'classes') assert.equal(report.mode, 'classes', 'no forced ownership override');
    let drawn = 0;
    const source = kind => { drawn++; return kind === 'uuid' ? `d0d0d0d0-0000-4000-8000-${String(drawn).padStart(12, '0')}` : kind === 'seed' ? Math.imul(drawn, 0x9e3779b1) >>> 0 : (Math.imul(drawn, 0x9e3779b1) >>> 0) / 2 ** 32; };
    for (const [index, call] of scenario.steps.entries()) {
      const [method, ...args] = call;
      const adopted = report.owned, originalBefore = originalCalls[method];
      const run = editor => editor[method.replace(/^@/, '')](...args.map(decode));
      let expectedError, ownedError, draws;
      try { ({draws} = withDraws(() => run(expected), source)); } catch (error) { expectedError = error.messageKey ?? error.message; draws = error.draws ?? []; }
      let at = 0;
      try { withDraws(() => run(owned), () => draws[at++][1]); } catch (error) { ownedError = error.messageKey ?? error.message; }
      const label = `${name} step ${index + 1}: ${method}`;
      assert.equal(ownedError, expectedError, `${label}: error`);
      if (Object.hasOwn(originalCalls, method)) {
        assert.equal(originalCalls[method], originalBefore, `${label}: original editor method was skipped`);
        assert.equal(report.owned, adopted + 1, `${label}: response was adopted`);
      }
      assert.equal(json(owned.report), json(expected.report), `${label}: document`);
      assert.deepEqual(state(owned), state(expected), `${label}: history and selection`);
      assert.equal(json(owned.evaluatedFrame), json(expected.evaluatedFrame), `${label}: frame`);
      assert.equal(report.mismatches.length, 0, `${label}: shadow comparisons`);
      await Promise.resolve();
    }
    if (mode === 'classes') {
      assert.ok(report.byCall.moveSelection?.calls > 0, 'recorded move calls were exercised in class mode');
      if (scenario.steps.some(([method]) => method === 'poseJoint')) assert.ok(report.byCall.poseJoint?.calls > 0, 'recorded pose calls were exercised in class mode');
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

function adoptionPair(document, mode = 'own') {
  const [expected, owned, oracle] = Array.from({length: 3}, () => new Editor({demo: false, propertyChannels: true, layerTimeline: true}).load(structuredClone(document)));
  const report = installEditShadow(owned, () => ({engine: {handle: {edit_session: () => session}}, mirror: {framesCurrent: () => true}}), {mode});
  const session = sessionFor(oracle), events = [[], []];
  for (const [index, editor] of [expected, owned].entries()) editor.addEventListener('change', event => events[index].push(clean(event.detail)));
  const same = () => {
    assert.equal(json(owned.report), json(expected.report));
    assert.deepEqual(state(owned), state(expected));
    assert.equal(json(owned.evaluatedFrame), json(expected.evaluatedFrame));
    assert.deepEqual(events[1], events[0]);
    assert.deepEqual(owned.geometry.drawables, buildGeometry(owned.report, owned.evaluatedFrame).drawables);
    assert.equal(report.mismatches.length, 0, JSON.stringify(report.mismatches));
  };
  const run = (method, ...args) => { for (const editor of [expected, owned]) editor[method](...args); same(); };
  return {expected, owned, oracle, session, report, same, run};
}

test('an adopted ordinary pose commit keeps the exact ready gesture frame and geometry', () => {
  const {expected, owned, oracle, session, same, run} = adoptionPair(createStartupDocument());
  run('select', 1, 3); run('beginDrag', 'Pose figure');
  // Runtime clocks are omitted from native wire records, not discarded by a commit.
  for (const editor of [expected, owned, oracle]) {
    editor.report._styk.vfxClips = {version: 1, clips: []};
    editor.frame._cameraMotionTick = 2;
    editor.frame._vfxTimelineTick = 3;
    editor.frame._vfxTimelineScene = [];
  }
  const tip = expected.geometry.objects.get(0).segments[3].end;
  run('poseJoint', tip.x + 4, tip.y - 3, {snap: false});
  const frames = [expected.evaluatedFrame, owned.evaluatedFrame], geometries = [expected.geometry, owned.geometry];
  let response;
  const call = session.call.bind(session);
  session.call = (...args) => { const text = call(...args); if (args[0] === 'commitDrag') response = JSON.parse(text); return text; };
  expected.commitDrag(); owned.commitDrag();
  assert.deepEqual(response.retained, [], 'an empty compact retained delta is present, not absent');
  assert.equal(response.retainedChanges, true);
  assert.equal(response.retainedOrder, undefined);
  assert.equal(response.retainedFields && Object.keys(response.retainedFields).length, 0);
  for (const [index, editor] of [expected, owned].entries()) {
    assert.ok(editor.evaluatedFrame === frames[index], 'the ordinary commit keeps its exact displayed gesture frame');
    assert.ok(editor._geometry === geometries[index], 'the ready geometry remains the same publication');
    assert.equal(editor.evaluatedFrame._cameraMotionTick, 2);
    assert.equal(editor.evaluatedFrame._vfxTimelineTick, 3);
  }
  same(); run('undo'); run('redo');
});

test('an adopted ordinary pose commit retains pending incremental geometry without preparing it', () => {
  const document = emptyDocument(), figure = document.figure_types[0];
  document.keyframes = [createFrame([createObject(figure, 1, 200, 200)], 4)];
  const {expected, owned, same, run} = adoptionPair(document);
  run('select', 1, 3); run('beginDrag', 'Pending pose');
  const tip = expected.geometry.objects.get(0).segments[3].end;
  for (const editor of [expected, owned]) editor.poseJoint(tip.x + 7, tip.y + 2, {snap: false});
  const frames = [expected.evaluatedFrame, owned.evaluatedFrame], pending = [expected._incremental, owned._incremental];
  assert.ok(pending.every(value => value?.previous));
  for (const editor of [expected, owned]) assert.equal(editor._geometry, null);
  expected.commitDrag(); owned.commitDrag();
  for (const [index, editor] of [expected, owned].entries()) {
    assert.equal(editor._geometry, null, 'publication does not synchronously prepare pending geometry');
    assert.ok(editor._incremental === pending[index], 'the same pending incremental owner survives');
    assert.ok(editor.evaluatedFrame === frames[index]);
  }
  same(); run('undo'); run('redo');
});

test('an adopted joined-root correction keeps the existing reconstruction fallback', () => {
  const document = JSON.parse(readFileSync(new URL('../engine/tests/edit/joined-chain.document.json', import.meta.url), 'utf8'));
  const {expected, owned, session, same, run} = adoptionPair(document);
  run('select', 1, 0); run('beginDrag', 'Joined pose');
  const tip = expected.geometry.objects.get(0).segments[0].end;
  run('poseJoint', tip.x + 17, tip.y - 12, {snap: false});
  const frame = owned.evaluatedFrame, geometry = owned.geometry;
  let response;
  const call = session.call.bind(session);
  session.call = (...args) => { const text = call(...args); if (args[0] === 'commitDrag') response = JSON.parse(text); return text; };
  expected.commitDrag(); owned.commitDrag();
  assert.ok(response.retained.length > 0, 'normalizing attached world roots sends actual replacements');
  assert.notEqual(owned.evaluatedFrame, frame);
  assert.notEqual(owned._geometry, geometry);
  assert.equal(owned._geometry, null);
  assert.equal(owned._incremental, null);
  same(); run('undo'); run('redo');
});

test('an adopted membership transaction never reuses a ready gesture geometry', () => {
  const {expected, owned, same, run} = adoptionPair(createStartupDocument());
  run('select', 1, -1);
  const frame = owned.evaluatedFrame, geometry = owned.geometry;
  expected.duplicateFigure(); owned.duplicateFigure();
  assert.notEqual(owned.evaluatedFrame, frame);
  assert.notEqual(owned._geometry, geometry);
  assert.equal(owned._geometry, null);
  assert.equal(owned._incremental, null);
  same(); run('undo'); run('redo');
});

test('an adopted movieclip clock change does not mistake absent retained data for an empty delta', () => {
  const document = JSON.parse(readFileSync(new URL('../engine/tests/edit/sheet-movieclip.document.json', import.meta.url), 'utf8'));
  const {expected, owned, session, same, run} = adoptionPair(document);
  run('select', 4, -1); run('beginDrag', 'Movieclip clock'); run('updateObject', {_stykMovieclip: {mode: 'once', speed: 2}});
  const frame = owned.evaluatedFrame, geometry = owned.geometry;
  let response;
  const call = session.call.bind(session);
  session.call = (...args) => { const text = call(...args); if (args[0] === 'commitDrag') response = JSON.parse(text); return text; };
  expected.commitDrag(); owned.commitDrag();
  assert.equal(Object.hasOwn(response, 'retained'), false, 'the ordinary clock-change commit samples again');
  assert.notEqual(owned.evaluatedFrame, frame);
  assert.notEqual(owned._geometry, geometry);
  assert.equal(owned._geometry, null);
  assert.equal(owned._incremental, null);
  same(); run('undo'); run('redo');
});

test('an adopted remote motion-path publication keeps ordinary invalidation semantics', () => {
  const document = JSON.parse(readFileSync(new URL('../engine/tests/edit/motion-path.document.json', import.meta.url), 'utf8'));
  const {expected, owned, same, run} = adoptionPair(document);
  run('select', 1, -1);
  run('beginMotionPathEdit', {kind: 'node', uid: 1, handleId: 0, joint: -1}, 6);
  run('updateMotionPathEdit', {x: 272, y: 182});
  const geometry = owned.geometry;
  expected.commitMotionPathEdit(); owned.commitMotionPathEdit();
  assert.equal(expected._geometry, null, 'the ordinary motion-path publisher invalidates geometry');
  assert.equal(owned._geometry, null);
  assert.equal(owned._incremental, null);
  assert.notEqual(owned._geometry, geometry);
  same(); run('undo'); run('redo');
});

test('owned pose/move adoption keeps the existing COW draft identity and exact live history', () => {
  const {expected, owned, run} = adoptionPair(createStartupDocument());
  run('select', 1, 3); run('beginDrag', 'Pose figure');
  const source = owned._authoredPoseEdit.source, sourceText = json(source), draft = owned.object;
  const snapshots = json(owned._drag.before), history = json(owned._undo);
  assert.notEqual(draft, source.objects[owned.selectedSlot]);
  for (let index = 0; index < 8; index++) {
    const tip = expected.geometry.objects.get(expected.selectedSlot).segments[expected.selectedJoint].end;
    run('poseJoint', tip.x + 3, tip.y - 2, {snap: false});
    assert.equal(owned.object, draft, 'an already-owned live draft is updated in place');
    assert.equal(json(source), sourceText, 'the source baseline is not mutated');
    assert.equal(json(owned._drag.before), snapshots, 'cancel snapshots are not mutated');
    assert.equal(json(owned._undo), history, 'live poses do not rewrite history');
  }
  run('cancelDrag');
  run('beginDrag', 'Pose figure');
  const tip = expected.geometry.objects.get(expected.selectedSlot).segments[expected.selectedJoint].end;
  run('poseJoint', tip.x - 5, tip.y + 4, {snap: false});
  run('commitDrag'); run('undo'); run('redo');
});

test('owned pose/move adoption retains translation sources and isolates expanded COW slots', () => {
  const document = emptyDocument(), figure = document.figure_types[0];
  document.keyframes = [createFrame([
    createObject(figure, 1, 200, 200),
    createObject(figure, 2, 260, 200, {join: {target_unique_id: 1, target_object_slot: 0, target_handle_id: 3}}),
    createObject(figure, 3, 500, 200),
  ], 4)];
  const {owned, run} = adoptionPair(document);
  run('select', 2, -1); run('beginDrag', 'Move figures');
  const source = owned._authoredPoseEdit.source, sourceText = json(source), initial = owned.geometry;
  const untouched = owned.frame.objects[2];
  assert.equal(owned.frame.objects[0], source.objects[0], 'the external root starts unowned');
  run('moveSelection', 11, -7);
  const root = owned.frame.objects[0], child = owned.frame.objects[1];
  assert.notEqual(root, source.objects[0], 'expanding the drag first isolates the new root');
  assert.equal(owned.geometry.objects.get(0).renderSource, initial.objects.get(0));
  assert.equal(owned.geometry.objects.get(1).renderSource, initial.objects.get(1));
  run('moveSelection', -3, 5);
  assert.equal(owned.frame.objects[0], root); assert.equal(owned.frame.objects[1], child);
  assert.equal(owned.frame.objects[2], untouched);
  assert.equal(json(source), sourceText);
  assert.equal(owned.geometry.objects.get(0).renderSource, initial.objects.get(0));
  run('cancelDrag');
  run('beginDrag', 'Move figures'); run('moveSelection', 8, 3); run('commitDrag'); run('undo'); run('redo');
});

test('owned pose/move adoption never labels a pending pose or pinned IK as a rigid translation', () => {
  const document = emptyDocument(), figure = document.figure_types[0];
  document.keyframes = [createFrame([createObject(figure, 1, 200, 200)], 4)];
  const {expected, owned, run} = adoptionPair(document);
  run('select', 1, 3); run('beginDrag', 'Pose then move');
  const tip = expected.geometry.objects.get(0).segments[3].end;
  for (const editor of [expected, owned]) editor.poseJoint(tip.x + 8, tip.y + 3, {snap: false});
  for (const editor of [expected, owned]) editor.moveSelection(5, 0);
  assert.equal(owned.geometry.objects.get(0).renderSource, undefined);
  run('cancelDrag');
  const pinnedDocument = createStartupDocument();
  pinnedDocument.keyframes[0].objects[0]._stykIK[8].pinned = true;
  const pinned = adoptionPair(pinnedDocument);
  pinned.run('select', 1, -1); pinned.run('beginDrag', 'Move pinned figure'); pinned.run('moveSelection', 5, 3);
  assert.equal(pinned.owned.geometry.objects.get(0).renderSource, undefined);
  pinned.run('cancelDrag');
});

test('owned pose/move adoption removes obsolete fields and preserves runtime samples and opaque keys', () => {
  const document = emptyDocument(), figure = document.figure_types[0];
  document.keyframes = [createFrame([createObject(figure, 1, 200, 200, {['__proto__']: {opaque: true}})], 4)];
  const {expected, owned, oracle, run} = adoptionPair(document);
  run('select', 1, -1); run('beginDrag', 'Move figure');
  const sourceText = json(owned._authoredPoseEdit.source), sample = {tick: 3, localTick: 1}, draft = owned.object;
  for (const editor of [expected, owned, oracle]) {
    editor.object.runtime_segments = structuredClone(editor.figure.segments);
    editor.object._movieclipSample = editor === owned ? sample : {...sample};
  }
  run('moveSelection', 7, 4);
  assert.equal(owned.object, draft);
  assert.equal(Object.hasOwn(owned.object, 'runtime_segments'), false);
  assert.equal(owned.object._movieclipSample, sample);
  assert.deepEqual(Object.keys(owned.object), Object.keys(expected.object));
  assert.equal(Object.getPrototypeOf(owned.object), Object.prototype);
  assert.deepEqual(owned.object['__proto__'], {opaque: true});
  assert.equal(json(owned._authoredPoseEdit.source), sourceText);
  run('cancelDrag');
});

test('owned pose/move adoption checks ordinary selection without rebuilding its captured state', () => {
  const {expected, owned, run} = adoptionPair(createStartupDocument());
  run('select', 1, 3); run('beginDrag', 'Pose figure');
  const read = owned._selectionState.bind(owned);
  let reads = 0;
  owned._selectionState = () => { reads++; return read(); };
  const tip = expected.geometry.objects.get(0).segments[3].end;
  run('poseJoint', tip.x + 4, tip.y + 2, {snap: false});
  assert.equal(reads, 2, 'the view and before-selection are the only complete selection captures');
  run('cancelDrag');
});

test('owned 3D pose adoption reuses unchanged geometry and preserves cancel and history', () => {
  const document = JSON.parse(readFileSync(new URL('../engine/tests/edit/ik-spatial-free.document.json', import.meta.url), 'utf8'));
  const {expected, owned, report, run} = adoptionPair(document, 'classes');
  assert.equal(editingClass(owned.report), '3d');
  run('select', 1, 9); run('beginDrag', 'Pose spatial leg');
  const source = owned._authoredPoseEdit.source, sourceText = json(source), draft = owned.object;
  const snapshots = json(owned._drag.before), history = json(owned._undo);
  assert.notEqual(draft, source.objects[0]);
  for (const [index, [dx, dy]] of [[5, -3], [7, 4], [-9, 6]].entries()) {
    const previous = owned.geometry.objects.get(0), tip = expected.geometry.objects.get(0).segments[9].end;
    run('poseJoint', tip.x + dx, tip.y + dy, {snap: false});
    assert.equal(owned.object, draft);
    if (index > 0) assert.equal(owned.geometry.objects.get(0).segments[0], previous.segments[0], 'the unchanged control retains its existing pose geometry');
    assert.equal(json(source), sourceText); assert.equal(json(owned._drag.before), snapshots);
    assert.equal(json(owned._undo), history);
  }
  run('cancelDrag');
  run('beginDrag', 'Pose spatial leg');
  const tip = expected.geometry.objects.get(0).segments[9].end;
  run('poseJoint', tip.x + 8, tip.y - 5, {snap: false});
  run('commitDrag'); run('undo'); run('redo');
  assert.equal(report.mode, 'classes'); assert.ok(report.owned >= 5);
});

function timedFacade(t, method, answer, {fixture = 'startup', joint = -1} = {}) {
  const document = JSON.parse(readFileSync(new URL(`../engine/tests/edit/${fixture}.document.json`, import.meta.url), 'utf8'));
  const open = () => {
    const editor = new Editor({demo: false, propertyChannels: true, layerTimeline: true}).load(structuredClone(document));
    editor.select(1, joint); editor.markSaved(); return editor;
  };
  const [owner, expected, oracle] = [open(), open(), open()], observed = {originalCalls: 0, error: null};
  let elapsed = 0;
  t.mock.method(performance, 'now', () => elapsed);
  const original = owner[method];
  owner[method] = function (...args) {
    observed.originalCalls++; elapsed += 5;
    try { return original.apply(this, args); } catch (error) { observed.error = error; throw error; }
  };
  const session = sessionFor(oracle), call = session.call;
  session.call = function (name, ...args) {
    elapsed += 7;
    return answer ? json(typeof answer === 'function' ? answer(owner, name) : answer) : call.call(this, name, ...args);
  };
  session.free = () => { elapsed += 2; };
  const report = installEditShadow(owner, () => ({engine: {handle: {edit_session: () => session}},
    mirror: {framesCurrent: () => true, syncFrames: async () => {}}}), {mode: 'own'});
  return {owner, expected, observed, report};
}

for (const [reason, answer] of [
  ['unported', {status: 'unported', reason: 'recorded refusal'}],
  ['unadoptable', {status: 'ok', events: [{kind: 'edit'}]}],
]) test(`failed ownership cost includes the ${reason} attempt and one original fallback`, t => {
  const {owner, expected, observed, report} = timedFacade(t, 'updateObject', answer);
  const result = owner.updateObject({scale: 1.2}), alone = expected.updateObject({scale: 1.2});
  assert.equal(json(result), json(alone)); assert.equal(json(owner.report), json(expected.report));
  assert.deepEqual(state(owner), state(expected)); assert.equal(observed.originalCalls, 1);
  assert.equal(report.owned, 0); assert.equal(report.ownedMs, 0);
  assert.equal(report.failedOwned, 1); assert.equal(report.failedOwnedMs, 9);
  assert.equal(report.ownableMs, 5); assert.equal(report.byCall.updateObject.calls, 1);
  assert.equal(report.byCall.updateObject.failedOwned, 1); assert.equal(report.byCall.updateObject.failedOwnedMs, 9);
  assert.equal(report.failedOwnedMs + report.ownableMs, 14);
  assert.equal(report.mismatches.length, 0);
});

for (const matches of [true, false]) test(`owned error replay keeps its original thrown error and timing (matching=${matches})`, t => {
  let expectedError;
  const answer = () => ({status: 'error', error: matches ? expectedError.messageKey ?? expectedError.message : 'different error'});
  const {owner, expected, observed, report} = timedFacade(t, 'setConnectorPosition', answer, {fixture: 'rig', joint: 5});
  assert.throws(() => expected.setConnectorPosition(2), error => { expectedError = error; return true; });
  assert.throws(() => owner.setConnectorPosition(2), error => error === observed.error
    && (error.messageKey ?? error.message) === (expectedError.messageKey ?? expectedError.message));
  assert.equal(json(owner.report), json(expected.report)); assert.deepEqual(state(owner), state(expected));
  assert.equal(observed.originalCalls, 1); assert.equal(report.owned, 0);
  assert.equal(report.failedOwned, 1); assert.equal(report.failedOwnedMs, 7);
  assert.equal(report.ownableMs, matches ? 5 : 7);
  assert.equal(report.byCall.setConnectorPosition.calls, 1);
  assert.equal(report.byCall.setConnectorPosition.editorMs, report.ownableMs);
  assert.equal(report.byCall.setConnectorPosition.failedOwnedMs, 7);
  assert.equal(report.ended['a different error'] ?? 0, matches ? 0 : 1);
});

test('an engine error followed by an original success preserves that success and counts it once', t => {
  const {owner, expected, observed, report} = timedFacade(t, 'updateObject', {status: 'error', error: 'unexpected error'});
  const result = owner.updateObject({scale: 1.2}), alone = expected.updateObject({scale: 1.2});
  assert.equal(json(result), json(alone)); assert.equal(json(owner.report), json(expected.report));
  assert.deepEqual(state(owner), state(expected)); assert.equal(observed.originalCalls, 1);
  assert.equal(report.owned, 0); assert.equal(report.failedOwned, 1); assert.equal(report.failedOwnedMs, 7);
  assert.equal(report.ownableMs, 7); assert.equal(report.byCall.updateObject.calls, 1);
  assert.equal(report.ended['an error the editor did not raise'], 1);
});

test('successful compact adoption has no failed ownership cost or original rerun', t => {
  const {owner, expected, observed, report} = timedFacade(t, 'moveSelection');
  owner.moveSelection(8, -3); expected.moveSelection(8, -3);
  assert.equal(json(owner.report), json(expected.report)); assert.deepEqual(state(owner), state(expected));
  assert.equal(observed.originalCalls, 0); assert.equal(report.owned, 1); assert.equal(report.ownedMs, 7);
  assert.equal(report.failedOwned, 0); assert.equal(report.failedOwnedMs, 0);
  assert.equal(report.ownableMs, 0, "cold ownership has no JavaScript fallback");
  const total = report.ownedMs + report.ownableMs + report.failedOwnedMs;
  assert.ok(Number.isFinite(total)); assert.equal(total, 7);
  assert.equal(report.byCall.moveSelection.calls, 1); assert.equal(report.mismatches.length, 0);
});

test('a comparison-only refusal does not become a failed owner attempt', t => {
  const {owner, expected, observed, report} = timedFacade(t, 'updateSegment', {status: 'unported', reason: 'comparison'}, {joint: 1});
  owner.updateSegment({angle_rad: .7}); expected.updateSegment({angle_rad: .7});
  assert.equal(json(owner.report), json(expected.report)); assert.deepEqual(state(owner), state(expected));
  assert.equal(observed.originalCalls, 1); assert.equal(report.calls, 1); assert.equal(report.compared, 0);
  assert.equal(report.failedOwned, 0); assert.equal(report.failedOwnedMs, 0); assert.equal(report.owned, 0);
});

for (const accepted of [true, false]) test(`batch comparison counters remain separate from owner failures (accepted=${accepted})`, t => {
  const events = [];
  const answer = (owner, name) => name === 'beginBatch' ? {status: 'ok'} : accepted
    ? {status: 'ok', state: state(owner), events, draft: null} : {status: 'unported', reason: 'batch close'};
  const {owner, report} = timedFacade(t, 'updateObject', answer);
  owner.addEventListener('change', event => events.push(clean(event.detail)));
  assert.equal(owner.movieclipBatch('No edit', () => 42), 42);
  assert.equal(report.calls, 1); assert.equal(report.compared, accepted ? 1 : 0);
  assert.equal(report.failedOwned, 0); assert.equal(report.failedOwnedMs, 0); assert.equal(report.owned, 0);
  assert.equal(report.mismatches.length, 0);
});

for (const mode of ['shadow', 'own']) {
  test(`segment values left by a null copy stay outside a fresh ${mode} session`, () => {
    const document = JSON.parse(readFileSync(new URL('../engine/tests/edit/joined-chain.document.json', import.meta.url), 'utf8'));
    const open = () => new Editor({demo: false, propertyChannels: true, layerTimeline: true}).load(structuredClone(document));
    const owner = open(), oracle = open();
    owner.select(1, 1);
    const copied = owner.copySegmentValues();
    assert.equal(typeof copied, 'string');
    owner.select(1, -1); oracle.select(1, -1);
    const before = {document: json(owner.report), state: state(owner), frame: json(owner.evaluatedFrame), historySize: owner._historySize};
    const calls = [], session = sessionFor(oracle), call = session.call;
    session.call = function (name, ...args) { const answer = call.call(this, name, ...args); calls.push({name, answer: JSON.parse(answer)}); return answer; };
    const report = installEditShadow(owner, () => ({engine: {handle: {edit_session: () => session}},
      mirror: {framesCurrent: () => true, syncFrames: async () => {}}}), {mode});
    assert.equal(owner.copySegmentValues(), null);
    assert.equal(owner._segmentValuesClipboard, copied);
    assert.equal(oracle._segmentValuesClipboard ?? null, null);
    assert.equal(calls[0].answer.status, 'ok'); assert.equal(calls[0].answer.result, null);
    assert.equal(owner.pasteSegmentValues(), false);
    assert.deepEqual({document: json(owner.report), state: state(owner), frame: json(owner.evaluatedFrame), historySize: owner._historySize}, before);
    assert.equal(report.mismatches.length, 0);
    assert.deepEqual(calls.map(({name}) => name), ['copySegmentValues']);
    assert.equal(report.ended['segment values the session did not copy'], 1);
    assert.equal(report.owned, 0);
  });

  test(`a null copy preserves segment values already copied by the ${mode} session`, () => {
    const document = JSON.parse(readFileSync(new URL('../engine/tests/edit/joined-chain.document.json', import.meta.url), 'utf8'));
    const editors = Array.from({length: 2}, () => new Editor({demo: false, propertyChannels: true, layerTimeline: true}).load(structuredClone(document)));
    const [owner, oracle] = editors;
    for (const editor of editors) editor.select(1, 1);
    const calls = [], session = sessionFor(oracle), call = session.call;
    session.call = function (name, ...args) { calls.push(name); return call.call(this, name, ...args); };
    const report = installEditShadow(owner, () => ({engine: {handle: {edit_session: () => session}},
      mirror: {framesCurrent: () => true, syncFrames: async () => {}}}), {mode});
    const copied = owner.copySegmentValues();
    assert.equal(typeof copied, 'string'); assert.equal(oracle._segmentValuesClipboard, copied);
    owner.select(1, -1);
    assert.equal(owner.copySegmentValues(), null);
    assert.equal(owner._segmentValuesClipboard, copied); assert.equal(oracle._segmentValuesClipboard, copied);
    const before = {document: json(owner.report), state: state(owner), frame: json(owner.evaluatedFrame), historySize: owner._historySize};
    assert.equal(owner.pasteSegmentValues(), false);
    assert.deepEqual({document: json(owner.report), state: state(owner), frame: json(owner.evaluatedFrame), historySize: owner._historySize}, before);
    assert.equal(report.mismatches.length, 0);
    assert.deepEqual(calls, ['copySegmentValues', 'select', 'copySegmentValues', 'pasteSegmentValues']);
    assert.equal(report.ended['segment values the session did not copy'] ?? 0, 0);
    assert.equal(report.owned, mode === 'own' ? 1 : 0);
  });
}


// Measure only this gesture's snapshot reconciliation, without replacing any
// global collection or changing the snapshot records used by cancel/history.
function snapshotTraffic(before) {
  const map = before.map.bind(before), sort = before.sort.bind(before), calls = {map: 0, sort: 0};
  Object.defineProperties(before, {
    map: {configurable: true, value(...args) { calls.map++; return map(...args); }},
    sort: {configurable: true, value(...args) { calls.sort++; return sort(...args); }},
  });
  return calls;
}

test('ordered ordinary live pose snapshots need no reconciliation or sorting', () => {
  const {expected, owned, same, run} = adoptionPair(createStartupDocument());
  run('select', 1, 3); run('beginDrag', 'Ordered pose');
  const before = owned._drag.before, snapshots = before.slice(), text = json(before);
  const source = owned._authoredPoseEdit.source, sourceText = json(source), history = json(owned._undo);
  const calls = snapshotTraffic(before);
  for (let index = 0; index < 4; index++) {
    const tip = expected.geometry.objects.get(0).segments[3].end;
    expected.poseJoint(tip.x + 4, tip.y - 2, {snap: false});
    owned.poseJoint(tip.x + 4, tip.y - 2, {snap: false});
    assert.deepEqual(calls, {map: 0, sort: 0}, 'the matching snapshots are neither recaptured nor sorted');
    assert.equal(owned._drag.before, before);
    assert.ok(before.every((object, slot) => object === snapshots[slot]));
    assert.equal(json(before), text); assert.equal(json(source), sourceText); assert.equal(json(owned._undo), history);
    same();
  }
  run('cancelDrag'); run('beginDrag', 'Ordered pose');
  const tip = expected.geometry.objects.get(0).segments[3].end;
  run('poseJoint', tip.x - 3, tip.y + 5, {snap: false}); run('commitDrag'); run('undo'); run('redo');
});

test('translation expansion reconciles once and preserves the ordered captured roots afterward', () => {
  const document = emptyDocument(), figure = document.figure_types[0];
  document.keyframes = [createFrame([
    createObject(figure, 1, 200, 200),
    createObject(figure, 2, 260, 200, {join: {target_unique_id: 1, target_object_slot: 0, target_handle_id: 3}}),
    createObject(figure, 3, 500, 200),
  ], 4)];
  const {expected, owned, same, run} = adoptionPair(document);
  run('select', 2, -1); run('beginDrag', 'Expand translation');
  const before = owned._drag.before, child = before[0], calls = snapshotTraffic(before);
  const source = owned._authoredPoseEdit.source, sourceText = json(source), history = json(owned._undo);
  assert.deepEqual(before.map(object => object.unique_id), [2]); calls.map = 0;
  expected.moveSelection(7, -3); owned.moveSelection(7, -3);
  assert.deepEqual(calls, {map: 1, sort: 1}, 'newly captured roots still use the original reconciliation');
  assert.equal(owned._drag.before, before); assert.equal(before[1], child);
  assert.deepEqual(Array.from(before, object => object.unique_id), [1, 2]);
  assert.equal(json(before), json(expected._drag.before));
  const root = before[0], snapshots = json(before);
  same();
  for (const [dx, dy] of [[-2, 4], [3, 1]]) {
    expected.moveSelection(dx, dy); owned.moveSelection(dx, dy);
    assert.deepEqual(calls, {map: 1, sort: 1}, 'the already-expanded ordered snapshots are reused');
    assert.equal(before[0], root); assert.equal(before[1], child); assert.equal(json(before), snapshots);
    assert.equal(json(source), sourceText); assert.equal(json(owned._undo), history);
    same();
  }
  run('cancelDrag'); run('beginDrag', 'Expand translation');
  run('moveSelection', 5, 3); run('commitDrag'); run('undo'); run('redo');
});

for (const matchingWireOrder of [false, true]) {
  test(`unordered pose snapshots retain the old sort fallback (${matchingWireOrder ? 'same unordered wire IDs' : 'different ordered wire IDs'})`, () => {
    const document = emptyDocument(), figure = document.figure_types[0];
    document.keyframes = [createFrame([
      createObject(figure, 1, 200, 200),
      createObject(figure, 2, 260, 200, {join: {target_unique_id: 1, target_object_slot: 0, target_handle_id: 3}}),
    ], 4)];
    const {expected, owned, session, same, run} = adoptionPair(document);
    run('select', 1, 3); run('beginDrag', 'Unordered snapshots');
    const before = owned._drag.before, originals = before.slice(), text = json(before);
    assert.equal(before.length, 2);
    before.reverse();
    const calls = snapshotTraffic(before), call = session.call.bind(session);
    if (matchingWireOrder) session.call = (method, ...args) => {
      const answer = JSON.parse(call(method, ...args));
      if (method === 'poseJoint') answer.drag.before.reverse();
      return JSON.stringify(answer);
    };
    const tip = expected.geometry.objects.get(0).segments[3].end;
    expected.poseJoint(tip.x + 4, tip.y + 3, {snap: false});
    owned.poseJoint(tip.x + 4, tip.y + 3, {snap: false});
    assert.deepEqual(calls, {map: 1, sort: 1}, 'wire equality alone cannot bypass frame-order validation');
    assert.equal(owned._drag.before, before);
    assert.ok(before.every((object, slot) => object === originals[slot])); assert.equal(json(before), text);
    assert.equal(json(before), json(expected._drag.before));
    session.call = call; same();
    run('commitDrag'); run('undo'); run('redo');
    run('beginDrag', 'Unordered snapshots'); run('cancelDrag');
  });
}

test('a malformed snapshot slot retains the exact old reconciliation and cancel behavior', () => {
  const {expected, owned, same, run} = adoptionPair(createStartupDocument());
  run('select', 1, 3); run('beginDrag', 'Malformed snapshot');
  const before = owned._drag.before;
  assert.equal(before.length, 1);
  before[0].object_slot = String(before[0].object_slot);
  const text = json(before), calls = snapshotTraffic(before), history = json(owned._undo);
  const source = owned._authoredPoseEdit.source, sourceText = json(source);
  const tip = expected.geometry.objects.get(0).segments[3].end;
  expected.poseJoint(tip.x + 2, tip.y + 4, {snap: false});
  owned.poseJoint(tip.x + 2, tip.y + 4, {snap: false});
  assert.deepEqual(calls, {map: 1, sort: 1}, 'a non-integer slot is left to the old fallback');
  assert.equal(json(before), text); assert.equal(json(source), sourceText); assert.equal(json(owned._undo), history);
  same(); run('cancelDrag');
});

test('other owned live commands retain their original snapshot reconciliation', () => {
  const {expected, owned, same, run} = adoptionPair(createStartupDocument());
  run('select', 1, -1); run('beginDrag', 'Rotate figure');
  const before = owned._drag.before, text = json(before), calls = snapshotTraffic(before);
  expected.rotateSelection(0.1); owned.rotateSelection(0.1);
  assert.deepEqual(calls, {map: 1, sort: 1}); assert.equal(json(before), text);
  same(); run('cancelDrag');
});


// These definition-only releases keep the same original Editor owner and eligible
// fallback clock. The real-Editor compact oracle remains a comparison/adoption harness,
// not evidence about the native runtime or the measured class cost.
function knownFallbackDocument() {
  const document = JSON.parse(readFileSync(new URL('../engine/tests/edit/builder.document.json', import.meta.url), 'utf8'));
  document.guardUnknown = {keep: ['document', 7]};
  document.figure_types[0].guardUnknown = {keep: ['definition', 9]};
  return document;
}
function observeFallbackSession(pair) {
  const calls = [], call = pair.session.call.bind(pair.session);
  let releases = 0;
  pair.session.call = (name, ...args) => { calls.push(name); return call(name, ...args); };
  pair.session.free = () => { releases++; };
  return {calls, get releases() { return releases; }};
}
function originalFallbackHistory(pair) {
  // A definition publication deliberately retires this harness's fixed Session.
  // Exercise the same original history owner without reopening that stale oracle.
  pair.owned.cancelVfxClipEdit = Editor.prototype.cancelVfxClipEdit;
  for (const method of ['undo', 'redo']) {
    assert.equal(Editor.prototype[method].call(pair.owned), Editor.prototype[method].call(pair.expected));
    pair.same();
  }
}

for (const changed of [false, true]) test(`owned rig release uses one original fallback (changed=${changed})`, t => {
  const pair = adoptionPair(knownFallbackDocument()), {expected, owned, report, same, run} = pair;
  run('select', 1, 1); run('beginRigDrag');
  if (changed) {
    const tip = expected.geometry.objects.get(0).segments[1].end;
    run('editRigJoint', tip.x + 8, tip.y - 5);
  }
  assert.ok(owned._drag.rig);
  const traffic = observeFallbackSession(pair), before = {calls: report.calls, owned: report.owned,
    failedOwned: report.failedOwned, skipped: report.skipped, historySize: owned._historySize};
  const commit = owned.commitRigDrag;
  let originals = 0;
  t.mock.method(owned, 'commitRigDrag', function (...args) {
    originals++; assert.equal(traffic.releases, 1, 'the Session is released before the original rig mutation');
    return commit.apply(this, args);
  });
  assert.equal(owned.commitDrag(), expected.commitDrag()); same();
  assert.equal(originals, 1); assert.deepEqual(traffic.calls, []); assert.equal(traffic.releases, 1);
  assert.equal(report.calls, before.calls); assert.equal(report.owned, before.owned);
  assert.equal(report.failedOwned, before.failedOwned); assert.equal(report.skipped, before.skipped + 1);
  assert.equal(report.byCall.commitDrag.calls, 1); assert.ok(Number.isFinite(report.byCall.commitDrag.editorMs));
  assert.equal(owned._drag, null);
  if (!changed) assert.equal(owned._historySize, before.historySize);
  originalFallbackHistory(pair);
});

test('comparison mode still compares changed rig releases and unchanged cancellation', () => {
  const pair = adoptionPair(knownFallbackDocument(), 'shadow'), {expected, report, run} = pair;
  run('select', 1, 1); run('beginRigDrag');
  const tip = expected.geometry.objects.get(0).segments[1].end;
  run('editRigJoint', tip.x + 8, tip.y - 5);
  const traffic = observeFallbackSession(pair), compared = report.compared;
  run('commitDrag'); run('undo'); run('redo'); run('beginRigDrag'); run('commitDrag');
  assert.equal(traffic.calls.filter(name => name === 'commitDrag').length, 2);
  assert.equal(traffic.releases, 0); assert.equal(report.skipped, 0);
  assert.ok(report.compared >= compared + 5); assert.equal(report.owned, 0); assert.equal(report.failedOwned, 0);
});

for (const [initial, patch] of [
  [false, {static: true}], [true, {static: false}], [false, {static: false}],
  [false, {static: true, opacity: .6, thickness: 5, color: '#aa3300'}],
]) test(`an own static patch keeps original definition/history semantics (${initial} -> ${JSON.stringify(patch)})`, t => {
  const seed = new Editor({demo: false}).load(knownFallbackDocument());
  seed.select(1, 1); seed.updateBuilderSegmentStyle({static: initial});
  const pair = adoptionPair(clean(seed.report)), {expected, owned, report, same, run} = pair;
  run('select', 1, 1);
  const traffic = observeFallbackSession(pair), calls = report.calls, skipped = report.skipped, figure = owned.figure;
  const record = owned._record;
  let records = 0;
  t.mock.method(owned, '_record', function (...args) {
    records++; assert.equal(traffic.releases, 1, 'the Session is released before the definition command is recorded');
    return record.apply(this, args);
  });
  const result = owned.updateBuilderSegmentStyle(patch), alone = expected.updateBuilderSegmentStyle(patch);
  assert.equal(result, alone); same();
  assert.deepEqual(traffic.calls, []); assert.equal(traffic.releases, 1);
  assert.equal(report.calls, calls); assert.equal(report.skipped, skipped + 1);
  assert.equal(report.owned, 0); assert.equal(report.failedOwned, 0);
  assert.equal(report.byCall.updateBuilderSegmentStyle.calls, 1);
  assert.equal(records, result ? 1 : 0); assert.equal(owned.figure, figure);
  originalFallbackHistory(pair);
});

for (const [patch, options, messageKey] of [
  [{static: 'yes'}, {}, 'errors.editorBuilderBranch.staticMustBeTrueOrFalse'],
  [{static: 'yes', color: '#oops', opacity: 2}, {}, 'errors.editorBuilderBranch.colorMustBeASixDigitHexColor'],
  [{static: 'yes', opacity: 2}, {}, 'errors.editorBuilderBranch.mustBeAFiniteNumberBetweenAnd'],
  [{static: 'yes'}, {branch: 'yes'}, 'errors.editorBuilderBranch.chooseWhetherToStyleTheWholeBranch'],
]) test(`own static fallback preserves original validation priority (${messageKey})`, () => {
  const pair = adoptionPair(knownFallbackDocument()), {expected, owned, report, same, run} = pair;
  run('select', 1, 1);
  const traffic = observeFallbackSession(pair), before = json(owned.report), history = owned._historySize;
  let alone, result;
  try { expected.updateBuilderSegmentStyle(patch, options); } catch (error) { alone = error; }
  try { owned.updateBuilderSegmentStyle(patch, options); } catch (error) { result = error; }
  assert.equal(alone?.messageKey, messageKey); assert.equal(result?.messageKey, alone.messageKey);
  assert.equal(result.message, alone.message); same();
  assert.equal(json(owned.report), before); assert.equal(owned._historySize, history);
  assert.deepEqual(traffic.calls, []); assert.equal(traffic.releases, 1);
  assert.equal(report.owned, 0); assert.equal(report.failedOwned, 0); assert.equal(report.skipped, 1);
  assert.equal(report.byCall.updateBuilderSegmentStyle.calls, 1);
});

test('known static fallback charges Session cleanup once in the eligible original clock', t => {
  const {owner, expected, observed, report} = timedFacade(t, 'updateBuilderSegmentStyle', null, {fixture: 'builder', joint: 1});
  assert.equal(owner.updateBuilderSegmentStyle({static: true}), expected.updateBuilderSegmentStyle({static: true}));
  assert.equal(json(owner.report), json(expected.report)); assert.deepEqual(state(owner), state(expected));
  assert.equal(observed.originalCalls, 1); assert.equal(report.calls, 0); assert.equal(report.skipped, 1);
  assert.equal(report.owned, 0); assert.equal(report.ownedMs, 0);
  assert.equal(report.failedOwned, 0); assert.equal(report.failedOwnedMs, 0);
  // timedFacade advances the real Session release by 2 ms and original Editor by 5 ms.
  // The v1 pre-admission cleanup would report only 5 ms, omitting eligible work.
  assert.equal(report.ownableMs, 7); assert.equal(report.jsMs, 7);
  assert.equal(report.byCall.updateBuilderSegmentStyle.calls, 1);
  assert.equal(report.byCall.updateBuilderSegmentStyle.editorMs, 7);
  assert.equal(report.ownedMs + report.ownableMs + report.failedOwnedMs, 7, 'cleanup and original fallback are charged once');
});

test('keyed scalar and recolor patches without an own static bit remain adopted', () => {
  const {report, run} = adoptionPair(knownFallbackDocument());
  run('select', 1, 1);
  for (const [patch, options] of [[{opacity: .6}, {}], [{thickness: 5}, {branch: true}], [{color: '#aa3300'}, {}]]) {
    const adopted = report.owned;
    run('updateBuilderSegmentStyle', patch, options);
    assert.equal(report.owned, adopted + 1);
  }
  assert.equal(report.skipped, 0); assert.equal(report.failedOwned, 0);
});

test('comparison mode still records and compares own static definition patches', () => {
  const pair = adoptionPair(knownFallbackDocument(), 'shadow'), {report, run} = pair;
  run('select', 1, 1);
  const traffic = observeFallbackSession(pair), compared = report.compared;
  run('updateBuilderSegmentStyle', {static: true, opacity: .6, color: '#aa3300'});
  assert.deepEqual(traffic.calls, ['updateBuilderSegmentStyle']); assert.equal(traffic.releases, 0);
  assert.equal(report.compared, compared + 1); assert.equal(report.skipped, 0);
  assert.equal(report.owned, 0); assert.equal(report.failedOwned, 0);
});

for (const name of ['startup', 'rig', 'joined-chain']) {
  test(`default classes compactly adopt figures without rerunning the editor: ${name}`, async () => {
    const open = () => {
      const editor = new Editor({demo: false, propertyChannels: true, layerTimeline: true});
      if (name === 'startup') editor.newDocument();
      else editor.load(JSON.parse(readFileSync(new URL(`../engine/tests/edit/${name}.document.json`, import.meta.url), 'utf8')));
      editor.markSaved(); return editor;
    };
    const [expected, owned, oracle] = [open(), open(), open()];
    assert.equal(editingClass(owned.report), 'figures');
    const originals = {}, wire = [], session = sessionFor(oracle);
    for (const method of ['moveSelection', 'poseJoint', 'commitDrag']) {
      const original = owned[method]; originals[method] = 0;
      owned[method] = function (...args) { originals[method]++; return original.apply(this, args); };
    }
    const call = session.call;
    session.call = function (method, args, view) { wire.push({method, view: JSON.parse(view)}); return call.call(this, method, args, view); };
    const report = installEditShadow(owned, () => ({engine: {handle: {edit_session: text => {
      const view = JSON.parse(text); assert.equal(view.propertyChannels, true); assert.equal(view.layerTimeline, true); return session;
    }}}, mirror: {framesCurrent: () => true, syncFrames: async () => {}}}));
    assert.equal(report.mode, 'classes', 'no forced ownership override');
    const history = editor => ({size: editor.historySize,
      undo: editor._undo.map(command => [command.label, command.bytes, command.beforeRevision, command.afterRevision]),
      redo: editor._redo.map(command => [command.label, command.bytes, command.beforeRevision, command.afterRevision])});
    const same = label => {
      assert.equal(json(owned.report), json(expected.report), `${label}: document`);
      assert.deepEqual(state(owned), state(expected), `${label}: history and selection`);
      assert.deepEqual(history(owned), history(expected), `${label}: history entries and budget`);
      assert.equal(json(owned.evaluatedFrame), json(expected.evaluatedFrame), `${label}: frame`);
      assert.equal(report.mismatches.length, 0, `${label}: shadow comparisons ${json(report.mismatches)}`);
    };
    const run = async (method, ...args) => {
      const before = originals[method], adopted = report.owned;
      expected[method](...args); owned[method](...args);
      if (Object.hasOwn(originals, method)) {
        assert.equal(originals[method], before, `${method}: original editor method was skipped`);
        assert.equal(report.owned, adopted + 1, `${method}: answer was adopted`);
        assert.equal(wire.at(-1).view.changesOnly, true, `${method}: compact response requested`);
      }
      same(method); await Promise.resolve();
    };
    const uid = expected.selectionFrame.objects[0].unique_id;
    await run('select', uid, -1); await run('beginTranslationDrag', 'Move figures');
    await run('moveSelection', 8, -3); await run('moveSelection', 2, 1); await run('commitDrag');
    await run('select', uid, 1); await run('beginDrag');
    const joint = expected.geometry.objects.get(expected.selectedSlot).segments[1].end;
    await run('poseJoint', joint.x + 15, joint.y + 9); await run('commitDrag');
    await run('select', uid, -1); await run('beginTranslationDrag', 'Cancelled move');
    await run('moveSelection', -5, 4); await run('cancelDrag');
    await run('undo'); await run('redo');
    while (expected.canUndo) await run('undo');
    assert.equal(owned.canUndo, false); assert.ok(owned.canRedo); same('full undo');
    assert.equal(report.unported && Object.keys(report.unported).length, 0);
  });
}

test('default class admission preserves other classes and invalidates its cached classification', async () => {
  assert.deepEqual([...OWNED_CLASSES].sort(), ['3d', 'channels', 'figures', 'large figures', 'movieclips']);
  const read = name => JSON.parse(readFileSync(new URL(`../${name}`, import.meta.url), 'utf8'));
  const editor = new Editor({demo: false, propertyChannels: true, layerTimeline: true}).newDocument();
  let starts = 0;
  const report = installEditShadow(editor, () => ({engine: {handle: {edit_session() { starts++; return null; }}},
    mirror: {framesCurrent: () => true, syncFrames: async () => {}}}));
  const probe = async (type, admitted) => {
    await report.prepare(); assert.equal(editingClass(editor.report), type);
    const before = starts; editor.select(editor.selectionFrame.objects[0].unique_id, -1);
    assert.equal(starts - before, admitted ? 1 : 0, `${type}: default admission`);
  };
  await probe('figures', true);
  const root = editor.report, definitionRevision = editor.definitionRevision;
  editor.setOrigin(editor.figure.segments[1].segment_id);
  assert.equal(editor.report, root); assert.ok(editor.definitionRevision > definitionRevision);
  await probe('unported', false);
  editor.undo(); assert.equal(editor.report, root); await probe('figures', true);
  for (const [file, type, admitted] of [
    ['engine/crates/styk-eval/tests/fixtures/ik/spatial.json', '3d', true],
    ['engine/crates/styk-eval/tests/fixtures/channels/channels-figure.sheet.json', 'channels', true],
    ['engine/tests/edit/sheet-movieclip.document.json', 'movieclips', true],
    ['engine/crates/styk-eval/tests/fixtures/channels/channels-in-movieclip.sheet.json', 'movieclips', true],
    ['engine/tests/edit/origin-pose-layout-sheet.document.json', 'unported', false],
  ]) { editor.load(read(file)); await probe(type, admitted); }
  assert.equal(editingClass(read('engine/crates/styk-eval/tests/fixtures/channels/channels-figure.json')), 'unported', 'native property channels stay excluded');
  const captured = read('engine/tests/edit/sheet-movieclip.document.json').figure_types.find(figure => figure._styk?.movieclip)._styk.movieclip.document;
  assert.equal(editingClass(captured), 'unported', 'a captured source document stays excluded');
  editor.load(documentOf([Array.from({length: 1000}, (_, index) => [index + 1, 0, 10, .1])]));
  await probe('large figures', true);
  editor.newDocument(); await probe('figures', true);
  assert.equal(report.owned, 0, 'cold session admission preserves the editor fallback');
  assert.equal(report.mismatches.length, 0);
});


test('default classes compactly adopt movieclip instance transforms and full history', async () => {
  const document = JSON.parse(readFileSync(new URL('../engine/tests/edit/sheet-movieclip.document.json', import.meta.url), 'utf8'));
  const open = () => {
    const editor = new Editor({demo: false, propertyChannels: true, layerTimeline: true}).load(structuredClone(document));
    editor.markSaved(); return editor;
  };
  const [expected, owned, oracle] = [open(), open(), open()];
  assert.equal(editingClass(owned.report), 'movieclips');
  const initial = json(expected.report), originals = {}, wire = [], session = sessionFor(oracle);
  for (const method of ['moveSelection', 'rotateSelection', 'scaleSelection', 'commitDrag']) {
    const original = owned[method]; originals[method] = 0;
    owned[method] = function (...args) { originals[method]++; return original.apply(this, args); };
  }
  const call = session.call;
  session.call = function (method, args, view) { wire.push({method, view: JSON.parse(view)}); return call.call(this, method, args, view); };
  const report = installEditShadow(owned, () => ({engine: {handle: {edit_session: () => session}},
    mirror: {framesCurrent: () => true, syncFrames: async () => {}}}));
  assert.equal(report.mode, 'classes', 'no forced ownership override');
  const history = editor => ({size: editor.historySize,
    undo: editor._undo.map(command => [command.label, command.bytes, command.beforeRevision, command.afterRevision]),
    redo: editor._redo.map(command => [command.label, command.bytes, command.beforeRevision, command.afterRevision])});
  const same = label => {
    assert.equal(json(owned.report), json(expected.report), `${label}: document`);
    assert.deepEqual(state(owned), state(expected), `${label}: history and selection`);
    assert.deepEqual(history(owned), history(expected), `${label}: history entries and budget`);
    assert.equal(json(owned.evaluatedFrame), json(expected.evaluatedFrame), `${label}: frame`);
    assert.equal(report.mismatches.length, 0, `${label}: shadow comparisons ${json(report.mismatches)}`);
  };
  const run = async (method, ...args) => {
    const before = originals[method], adopted = report.owned;
    expected[method](...args); owned[method](...args);
    if (Object.hasOwn(originals, method)) {
      assert.equal(originals[method], before, `${method}: original editor method was skipped`);
      assert.equal(report.owned, adopted + 1, `${method}: answer was adopted`);
      assert.equal(wire.at(-1).view.changesOnly, true, `${method}: compact response requested`);
    }
    same(method); await Promise.resolve();
  };
  await run('seek', 3);
  const instance = expected.selectionFrame.objects.find(object => expected.report.figure_types
    .find(figure => figure.figure_type_id === object.figure_type_id)?._styk?.movieclip);
  assert.ok(instance, 'the existing recorded clip instance is on show');
  await run('select', instance.unique_id, -1);
  await run('beginTranslationDrag', 'Move movieclip');
  await run('moveSelection', 14, 6); await run('moveSelection', -2, 1); await run('commitDrag');
  await run('beginDrag', 'Rotate movieclip'); await run('rotateSelection', .3); await run('commitDrag');
  await run('beginDrag', 'Scale movieclip'); await run('scaleSelection', 1.3); await run('commitDrag');
  await run('beginTranslationDrag', 'Cancel movieclip move'); await run('moveSelection', -5, 4); await run('cancelDrag');
  await run('undo'); await run('redo');
  while (expected.canUndo) await run('undo');
  assert.equal(owned.canUndo, false); assert.ok(owned.canRedo); same('full undo');
  assert.equal(json(owned.report), initial, 'full undo restores the original clip document');
  assert.deepEqual(Object.keys(report.unported ?? {}), []);
});


test('a one-shot adopted move exposes the moved COW frame at both real Editor events', () => {
  const document = JSON.parse(readFileSync(new URL('../engine/crates/styk-eval/tests/fixtures/channels/channels-figure.sheet.json', import.meta.url), 'utf8'));
  const {expected, owned, oracle, session, report, same, run} = adoptionPair(document);
  // This is the original full driver's import-and-move route: the new UID is
  // isolated even while the original UID1/UID3 component remains joined.
  run('importFigure', structuredClone(expected.report.figure_types[0]), {position: {x: 200, y: 150}});
  const source = owned.evaluatedFrame;
  const untouched = source.objects.filter(object => !owned.selectedUids.has(object.unique_id));
  assert.ok(untouched.some(object => object.join), 'unrelated joins remain on show');
  for (const editor of [expected, owned, oracle]) {
    // As in the existing ready-commit clock control, the runtime VFX clock
    // belongs to an actual clip state, even when that state has no effects.
    editor.report._styk.vfxClips = {version: 1, clips: []};
    editor.evaluatedFrame._cameraMotionTick = 2.75;
    editor.evaluatedFrame._vfxTimelineTick = 4.25;
    editor.evaluatedFrame._vfxTimelineScene = [];
  }
  const initial = json(expected.report), sourceBytes = json(source);
  // The first native retained head may carry the full frame despite compact
  // sheet publication. Exercise that exact supported response shape.
  const call = session.call.bind(session);
  session.call = (...args) => {
    const answer = JSON.parse(call(...args));
    if (args[0] === 'moveSelection' && answer.retained) {
      // JS's planner clones unchanged layers; Native keeps the same COW node
      // and omits it in compact answers. Prove equality before modeling omission.
      assert.deepEqual(answer.layers, owned.report._styk.layers); delete answer.layers;
      const frame = oracle._evaluated;
      answer.retained = frame.objects.map(clean);
      answer.retainedFields = Object.fromEntries(Object.entries(frame).filter(([key]) => key !== 'objects' && !runtimeClock(key)));
      delete answer.retainedChanges; delete answer.retainedOrder;
    }
    return json(answer);
  };
  const views = [[], []], frames = [[], []], geometries = [[], []];
  for (const [index, editor] of [expected, owned].entries()) editor.addEventListener('change', event => {
    if (event.detail.kind !== 'edit' || !event.detail.live && !event.detail.layerSheet) return;
    const frame = editor.evaluatedFrame, geometry = editor.geometry;
    views[index].push({detail: clean(event.detail), frame: json(frame), drawables: clean(geometry.drawables), state: state(editor), revision: editor.revision});
    frames[index].push(frame); geometries[index].push(geometry);
  });
  const adopted = report.owned;
  expected.moveSelection(10, 0); owned.moveSelection(10, 0);
  assert.equal(report.owned, adopted + 1);
  assert.equal(views[0].length, 2); assert.deepEqual(views[1], views[0], 'live and publication observers see the same moved frame, geometry and history');
  assert.equal(frames[1][1], frames[1][0]); assert.equal(geometries[1][1], geometries[1][0]);
  assert.equal(owned.evaluatedFrame, frames[1][0]); assert.equal(owned._geometry, geometries[1][0]);
  assert.ok(untouched.every(object => owned.evaluatedFrame.objects.includes(object)), 'full packets preserve untouched COW records');
  // Runtime clocks stay on the preview; the report/history sources are isolated.
  assert.equal(owned.evaluatedFrame._cameraMotionTick, 2.75); assert.equal(owned.evaluatedFrame._vfxTimelineTick, 4.25);
  assert.equal(json(source), sourceBytes);
  assert.equal(owned._drag, null); assert.equal(owned._authoredPoseEdit, null);
  same(); run('undo'); assert.equal(json(owned.report), initial); run('redo');
  run('beginTranslationDrag', 'Cancel the next move'); run('moveSelection', -5, 3); run('cancelDrag');
  while (expected.canUndo) run('undo');
});

test('a one-shot adopted move preserves pending scoped geometry through publication', () => {
  const document = JSON.parse(readFileSync(new URL('../engine/crates/styk-eval/tests/fixtures/channels/channels-figure.sheet.json', import.meta.url), 'utf8'));
  const {expected, owned, session, same, run} = adoptionPair(document);
  run('select', 2, -1);
  const call = session.call.bind(session);
  session.call = (...args) => {
    const answer = JSON.parse(call(...args));
    if (args[0] === 'moveSelection' && answer.retained) {
      assert.deepEqual(answer.layers, owned.report._styk.layers); delete answer.layers;
    }
    return json(answer);
  };
  const source = owned.evaluatedFrame, untouched = source.objects.filter(object => object.unique_id !== 2);
  const previous = owned.geometry, slot = owned.selectedSlot, views = [[], []], pending = [[], []], frames = [[], []];
  for (const [index, editor] of [expected, owned].entries()) editor.addEventListener('change', event => {
    if (event.detail.kind !== 'edit' || !event.detail.live && !event.detail.layerSheet) return;
    views[index].push({detail: clean(event.detail), frame: json(editor.evaluatedFrame), state: state(editor)});
    frames[index].push(editor.evaluatedFrame); pending[index].push(editor._incremental);
    assert.equal(editor._geometry, null, 'listeners do not force preparation');
  });
  expected.moveSelection(-7, 4); owned.moveSelection(-7, 4);
  assert.deepEqual(views[1], views[0]); assert.equal(views[1].length, 2);
  assert.ok(pending[1][0]); assert.equal(pending[1][1], pending[1][0]);
  assert.equal(owned._incremental, pending[1][0]); assert.equal(owned._incremental.previous, previous);
  assert.deepEqual([...owned._incremental.dirtySlots], [slot]); assert.ok(owned._incremental.renderSources.has(slot));
  assert.equal(frames[1][1], frames[1][0]);
  assert.ok(untouched.every(object => owned.evaluatedFrame.objects.includes(object)));
  same(); run('undo'); run('redo');
});

test('one-shot joined, rich, zero and failed moves keep the existing adoption fallback', () => {
  const read = name => JSON.parse(readFileSync(new URL(`../${name}`, import.meta.url), 'utf8'));
  for (const [document, uid] of [
    [read('engine/tests/edit/joined-chain.document.json'), 1],
    [read('engine/crates/styk-eval/tests/fixtures/ik/spatial.json'), null],
    [read('engine/tests/edit/origin-pose-layout-sheet.document.json'), null],
  ]) {
    const {expected, owned, run, same} = adoptionPair(document);
    run('select', uid ?? expected.evaluatedFrame.objects[0].unique_id, -1);
    let begins = 0;
    const begin = owned._beginAuthoredPoseDrag;
    owned._beginAuthoredPoseDrag = function (...args) { begins++; return begin.apply(this, args); };
    expected.moveSelection(8, -3); owned.moveSelection(8, -3);
    assert.equal(begins, 0, 'joined/solver/origin moves retain the original reconstruction route');
    assert.equal(owned._drag, null); assert.equal(owned._authoredPoseEdit, null);
    same(); run('undo'); run('redo');
  }
  const {expected, owned, run, same} = adoptionPair(createStartupDocument());
  run('select', 1, -1);
  let begins = 0;
  const begin = owned._beginAuthoredPoseDrag;
  owned._beginAuthoredPoseDrag = function (...args) { begins++; return begin.apply(this, args); };
  const before = json(owned.report);
  expected.moveSelection(0, 0); owned.moveSelection(0, 0);
  assert.equal(begins, 0, 'a move without publication does not invent a draft'); same();
  assert.throws(() => expected.moveSelection(Infinity, 0)); assert.throws(() => owned.moveSelection(Infinity, 0));
  assert.equal(begins, 0, 'native errors replay the original validation before any draft');
  assert.equal(json(owned.report), before); assert.equal(owned._drag, null); assert.equal(owned._authoredPoseEdit, null); same();
});

// Project the agreed packet from the separate actual Editor's topology snapshot.
// Native Session tests independently certify this opt-in wire projection.
function splitPair(document, {mode = 'classes', historyLimit} = {}) {
  const pair = adoptionPair(document, mode), {oracle, owned, session} = pair;
  if (historyLimit !== undefined) for (const editor of [pair.expected, owned, oracle]) editor.historyLimit = historyLimit;
  const call = session.call.bind(session);
  session.call = (name, args, viewText) => {
    const view = JSON.parse(viewText), figure = view.topologyPublication ? oracle.figure : null;
    const index = figure ? oracle.report.figure_types.indexOf(figure) : -1;
    const before = figure ? oracle._topologySnapshot(figure.figure_type_id) : null;
    const fields = [oracle.report._styk, oracle.report._styk.layerSheet], keys = ['animation', 'legacyAnimation'];
    const previous = fields.map((fields, index) => ({exists: Object.hasOwn(fields ?? {}, keys[index]), value: fields?.[keys[index]]}));
    const answer = JSON.parse(call(name, args, viewText));
    if (before && answer.status === 'ok' && answer.result === true && answer.events.length === 1
      && answer.events[0].definition && !answer.events[0].layerSheet) {
      const after = oracle._topologySnapshot(figure.figure_type_id, before);
      const frames = after.frames.map(entry => {
        const old = before.frames.find(frame => frame.index === entry.index && frame.sourceId === entry.sourceId);
        const byUid = new Map(old.objects.map(object => [object.unique_id, json(object)]));
        return {index: entry.index, ...(entry.sourceId ? {sourceId: entry.sourceId} : {}),
          objects: entry.objects.filter(object => byUid.get(object.unique_id) !== json(object))
            .map(object => clean(oracle._sourceFrame(entry).objects.find(value => value.unique_id === object.unique_id)))};
      }).filter(entry => entry.objects.length);
      answer.topology = {index, figure: after.figure, frames};
      for (const [index, key] of keys.entries()) {
        const fields = index ? oracle.report._styk.layerSheet : oracle.report._styk;
        const exists = Object.hasOwn(fields ?? {}, key), value = fields?.[key];
        if (exists !== previous[index].exists || value !== previous[index].value)
          answer.topology[index ? 'sheetAnimation' : 'animation'] = {exists, ...(exists ? {value} : {})};
      }
    }
    return json(answer);
  };
  return pair;
}
const splitDocument = () => JSON.parse(readFileSync(new URL('../engine/tests/edit/builder.document.json', import.meta.url), 'utf8'));
const topologyCounters = editor => ({revision: editor.revision, serial: editor._revisionSerial,
  saved: editor._savedRevision, definition: editor.definitionRevision, historySize: editor._historySize,
  nextUid: editor._nextUid, nextFrame: editor._nextFrameIdentity, generated: [...editor._generatedFrameSources]});

test('default Split direct and aliases adopt the recorded topology with one original history owner', () => {
  const scenario = JSON.parse(readFileSync(new URL('../engine/tests/edit/builder-split-sheet.scenario.json', import.meta.url), 'utf8'));
  const {expected, owned, report, run, same} = splitPair(splitDocument());
  const initial = json(expected.report), edits = [[], []];
  for (const [index, editor] of [expected, owned].entries()) editor.addEventListener('change', event => {
    if (!event.detail.definition || event.detail.kind !== 'edit') return;
    edits[index].push({detail: clean(event.detail), frame: json(editor.evaluatedFrame),
      geometry: clean(editor.geometry.drawables), state: state(editor), counters: topologyCounters(editor)});
  });
  let originals = 0;
  const original = owned._topologyEdit;
  owned._topologyEdit = function (...args) { originals++; return original.apply(this, args); };
  for (const [method, ...args] of scenario.steps) {
    if (method === '@frame') { same(); continue; }
    const name = method.replace(/^@/, ''), adopted = report.owned, beforeOriginal = originals;
    let error;
    try { expected[name](...args); } catch (caught) { error = caught.messageKey ?? caught.message; }
    if (error) assert.throws(() => owned[name](...args), caught => (caught.messageKey ?? caught.message) === error);
    else owned[name](...args);
    same(); assert.deepEqual(topologyCounters(owned), topologyCounters(expected));
    if (['splitBuilderSegments', 'splitSegment', 'splitSegmentBranch'].includes(name) && !error) {
      assert.equal(originals, beforeOriginal, 'the original topology computation was skipped');
      assert.equal(report.owned, adopted + 1, 'the response was actually adopted');
      if (owned.selectedJoint >= 0) assert.equal(owned._undo.at(-1)?.kind, 'topology');
    }
  }
  assert.deepEqual(edits[1], edits[0]); assert.equal(edits[1].length, 4);
  assert.equal(json(owned.report), initial); assert.equal(owned.canUndo, false);
  assert.equal(report.mode, 'classes'); assert.equal(report.failedOwned, 2, 'recorded validation errors replay once');
});

test('adopted Split preserves changed and untouched record identity, key order and source history', () => {
  const document = splitDocument();
  document.topologyUnknown = {keep: null}; document.figure_types[0].topologyUnknown = {keep: ['definition', 3]};
  for (const frame of document.keyframes) for (const object of frame.objects) object.topologyUnknown = {keep: ['pose', object.unique_id]};
  Object.defineProperty(document.figure_types[0], '__proto__', {value: {keep: 'definition'}, enumerable: true, configurable: true, writable: true});
  const {expected, owned, oracle, report, run, same} = splitPair(document);
  const initial = json(owned.report);
  run('select', 1, -1); run('moveSelection', 5, -2); run('select', 1, 1);
  const figure = owned.figure, frames = owned._allSourceFrames();
  const records = frames.map(frame => frame.objects.slice()), layers = owned.report._styk.layers, sheet = owned.report._styk.layerSheet;
  const before = json(owned.report), count = owned._undo.length, adopted = report.owned;
  expected.splitBuilderSegments({branch: false, fraction: .5}); owned.splitBuilderSegments({branch: false, fraction: .5});
  same(); assert.equal(report.owned, adopted + 1); assert.equal(owned.figure, figure);
  frames.forEach((frame, index) => frame.objects.forEach((object, slot) => assert.equal(object, records[index][slot])));
  assert.equal(owned.report._styk.layers, layers); assert.equal(owned.report._styk.layerSheet, sheet);
  assert.equal(owned._undo.length, count + 1); assert.equal(owned._undo.at(-1).kind, 'topology');
  assert.deepEqual(owned._undo.at(-1), expected._undo.at(-1));
  assert.deepEqual(Object.keys(owned.figure), Object.keys(expected.figure));
  assert.ok(owned._undo.at(-1).after.frames.some(entry => entry.sourceId), 'stored source poses join the same history snapshot');
  const history = structuredClone(owned._undo.at(-1)), after = json(owned.report);
  run('undo'); assert.equal(json(owned.report), before); run('redo'); assert.equal(json(owned.report), after);
  assert.deepEqual(owned._undo.at(-1), history, 'replay does not mutate retained snapshots');
  run('undo'); run('select', 1, 1); run('splitSegment', .3);
  assert.equal(owned.canRedo, false, 'a new split retires the same redo branch');
  while (expected.canUndo) run('undo'); assert.equal(json(owned.report), initial);
  // Original pose history snapshots omit an own __proto__ key, but the raw
  // stored pose and Native publication keep it through the forward split.
  for (const editor of [expected, owned, oracle]) for (const frame of editor._allSourceFrames()) for (const object of frame.objects)
    Object.defineProperty(object, '__proto__', {value: {keep: 'pose'}, enumerable: true, configurable: true, writable: true});
  run('select', 1, 1); const forward = report.owned;
  expected.splitSegment(.5); owned.splitSegment(.5); same(); assert.equal(report.owned, forward + 1);
  assert.equal(Object.hasOwn(owned.report.keyframes[0].objects[0], '__proto__'), true);
  assert.deepEqual(owned.report.keyframes[0].objects[0]['__proto__'], {keep: 'pose'});
  assert.deepEqual(Object.keys(owned.report.keyframes[0].objects[0]), Object.keys(expected.report.keyframes[0].objects[0]));
});

test('zero-history Split retains the original source cleanup and revision owner', () => {
  const {expected, owned, oracle, report, run, same} = splitPair(splitDocument());
  run('select', 1, -1);
  const priorSources = new Set(Object.keys(owned.report._styk.frameSources ?? {}));
  run('moveSelection', 13, -4);
  const firstSources = Object.keys(owned.report._styk.frameSources).filter(id => !priorSources.has(id));
  assert.ok(firstSources.length, 'the first actual authored move allocated a retained source');
  run('moveSelection', -7, 9); run('select', 1, 1);
  assert.ok(firstSources.every(id => Object.hasOwn(owned.report._styk.frameSources, id)), 'the first source is still held by move history');
  for (const editor of [expected, owned, oracle]) editor.historyLimit = 0;
  const adopted = report.owned;
  expected.splitBuilderSegments({branch: true, fraction: .5}); owned.splitBuilderSegments({branch: true, fraction: .5});
  same(); assert.deepEqual(topologyCounters(owned), topologyCounters(expected));
  assert.equal(report.owned, adopted + 1); assert.equal(owned.canUndo, false); assert.equal(owned._undo.length, 0);
  assert.deepEqual(owned.report._styk.frameSources, expected.report._styk.frameSources);
  assert.ok(firstSources.some(id => !Object.hasOwn(owned.report._styk.frameSources, id)), 'original zero-history cleanup drops the edited orphan source');
});

test('malformed Split packets fall back atomically before any adopted mutation', () => {
  for (const alter of [
    answer => { delete answer.topology; },
    answer => { answer.topology.index = 999; },
    answer => { answer.topology.frames = []; },
    answer => { answer.topology.frames[0].objects[0].unique_id = 999; },
    answer => { answer.topology.frames[0].objects[0].angles = []; },
    answer => { answer.topology.frames.push(structuredClone(answer.topology.frames[0])); },
    answer => { answer.topology.frames[0].sourceId = 'foreign-source'; answer.topology.frames[0].index = -1; },
    answer => { answer.topology.animation = {exists: true}; },
    answer => { answer.events = null; },
    answer => { answer.state = null; },
    answer => { answer.state.selectedUids.reverse(); answer.state.selectedUid = 999; },
  ]) {
    const pair = splitPair(splitDocument()), {expected, owned, session, report, run, same} = pair;
    run('select', 1, 1);
    const call = session.call.bind(session), before = json(owned.report);
    session.call = (...args) => { const answer = JSON.parse(call(...args)); if (answer.topology) alter(answer); return json(answer); };
    let originals = 0; const original = owned._topologyEdit;
    owned._topologyEdit = function (...args) { originals++; assert.equal(json(this.report), before, 'no definition/pose write preceded refusal'); return original.apply(this, args); };
    expected.splitBuilderSegments({branch: false, fraction: .5}); owned.splitBuilderSegments({branch: false, fraction: .5});
    same(); assert.equal(originals, 1); assert.equal(report.owned, 0); assert.equal(report.failedOwned, 1);
    assert.deepEqual(owned._undo.at(-1), expected._undo.at(-1)); originalFallbackHistory(pair);
  }
});

test('a missing current generated Split source is refused even at zero history', () => {
  const pair = splitPair(splitDocument()), {expected, owned, oracle, session, report, run, same} = pair;
  run('select', 1, -1); run('moveSelection', 13, -4); run('select', 1, 1);
  for (const editor of [expected, owned, oracle]) editor.historyLimit = 0;
  const retained = owned._currentFrameSourceIds();
  assert.ok(retained.size, 'the actual move left a current generated source reference');
  const call = session.call.bind(session), before = json(owned.report), adopted = report.owned;
  session.call = (...args) => {
    const answer = JSON.parse(call(...args));
    if (answer.topology) {
      const count = answer.topology.frames.length;
      answer.topology.frames = answer.topology.frames.filter(entry => !retained.has(entry.sourceId));
      assert.ok(answer.topology.frames.length < count, 'the control removes an actual currently referenced source pose');
    }
    return json(answer);
  };
  let originals = 0; const original = owned._topologyEdit;
  owned._topologyEdit = function (...args) { originals++; assert.equal(json(this.report), before); return original.apply(this, args); };
  expected.splitBuilderSegments({branch: false, fraction: .5}); owned.splitBuilderSegments({branch: false, fraction: .5});
  same(); assert.equal(originals, 1); assert.equal(report.owned, adopted, 'the refused Split did not add an adoption');
  assert.equal(report.failedOwned, 1); assert.equal(owned.canUndo, false);
});

test('Split application failures restore the original topology and retire the advanced Native session', () => {
  const {expected, owned, oracle, session, report, run, same} = splitPair(splitDocument());
  run('select', 1, 1);
  const before = json(owned.report), history = owned._undo.slice(), counters = topologyCounters(owned);
  let releases = 0;
  session.free = () => {
    releases++;
    assert.equal(json(owned.report), before, 'rollback precedes disposal of the advanced session');
    // The next session starts from the mirror's restored report, not the
    // successful Native split that the JavaScript application could not install.
    oracle.load(structuredClone(owned.report));
    // A fresh Native Session takes the mirror's original revision counters and
    // startup layer state; reloading this reused Editor otherwise increments them.
    for (const key of ['_revision', '_revisionSerial', '_savedRevision', '_layerRevision', '_definitionRevision']) oracle[key] = owned[key];
    oracle._restoreSelection(owned._selectionState());
  };
  for (const editor of [expected, owned]) {
    const object = editor.frame.objects.find(object => object.unique_id === 1), angles = object.angles;
    Object.defineProperty(object, 'angles', {enumerable: true, configurable: true,
      get: () => angles, set() { throw new Error('Rejected pose write'); }});
  }
  assert.throws(() => expected.splitBuilderSegments({branch: false, fraction: .5}), /Rejected pose write/);
  assert.throws(() => owned.splitBuilderSegments({branch: false, fraction: .5}), /Rejected pose write/);
  same(); assert.equal(json(owned.report), before); assert.deepEqual(owned._undo, history);
  assert.deepEqual(topologyCounters(owned), counters); assert.equal(report.mismatches.length, 0);
  assert.equal(releases, 1); assert.equal(report.ended['a topology packet application failed'], 1);
  const adopted = report.owned, sessions = report.sessions;
  expected.splitSegment(.5); owned.splitSegment(.5); same();
  assert.equal(report.sessions, sessions + 1); assert.equal(report.owned, adopted + 1, 'a fresh coherent session can adopt the next call');
});

test('native, drafted, captured-source, origin and active Split routes keep their original fallback', () => {
  const read = path => JSON.parse(readFileSync(new URL(`../${path}`, import.meta.url), 'utf8'));
  for (const [document, setup, excluded] of [
    [splitDocument(), editor => editor.beginFrameDraft({mode: 'update'}), editor => editor._frameDraft],
    [read('engine/tests/edit/origin-pose-layout-sheet.document.json'), () => {}, editor => editor.figure?._styk?.animationOrigin],
    [splitDocument(), editor => editor.beginDrag('Existing gesture'), editor => editor._drag],
    [read('engine/tests/edit/sheet-movieclip.document.json').figure_types.find(figure => figure._styk?.movieclip)._styk.movieclip.document,
      () => {}, editor => editor.report._styk.movieclipSource],
  ]) {
    const {expected, owned, oracle, report, session, same} = splitPair(document);
    for (const editor of [expected, owned, oracle]) { editor.select(editor.evaluatedFrame.objects[0].unique_id, 1); setup(editor); assert.ok(excluded(editor), 'the actual Editor has the excluded route'); }
    const adopted = report.owned;
    let requested = 0; const call = session.call.bind(session);
    session.call = (name, args, view) => { if (JSON.parse(view).topologyPublication) requested++; return call(name, args, view); };
    let expectedError, ownedError;
    try { expected.splitBuilderSegments({branch: false, fraction: .5}); } catch (error) { expectedError = error.messageKey ?? error.message; }
    try { owned.splitBuilderSegments({branch: false, fraction: .5}); } catch (error) { ownedError = error.messageKey ?? error.message; }
    assert.equal(ownedError, expectedError); same(); assert.equal(requested, 0); assert.equal(report.owned, adopted, 'the Split itself was not adopted');
  }
  const document = splitDocument(), [expected, owned, oracle] = Array.from({length: 3}, () => new Editor({demo: false}).load(structuredClone(document)));
  for (const editor of [expected, owned, oracle]) {
    assert.equal(editor.layerTimeline, false); assert.equal(editor.report._styk.layerSheet, undefined);
    editor.select(1, 1);
  }
  const session = sessionFor(oracle), call = session.call.bind(session); let requested = 0;
  session.call = (name, args, view) => { if (JSON.parse(view).topologyPublication) requested++; return call(name, args, view); };
  const report = installEditShadow(owned, () => ({engine: {handle: {edit_session: () => session}}, mirror: {framesCurrent: () => true}}), {mode: 'own'});
  expected.splitBuilderSegments({branch: false, fraction: .5}); owned.splitBuilderSegments({branch: false, fraction: .5});
  assert.equal(json(owned.report), json(expected.report)); assert.deepEqual(state(owned), state(expected));
  assert.equal(requested, 0); assert.equal(report.owned, 0);
});
