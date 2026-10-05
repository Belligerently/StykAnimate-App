// Owned response adoption against the real editor, including compact drafts and
// retained previews. Rust's session tests independently verify the wire deltas.
import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {Editor} from '../src/editor.mjs';
import {editingClass, installEditShadow} from '../src/engine/engine-edit-shadow.mjs';
import {withDraws} from '../src/editor-draws.mjs';
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
