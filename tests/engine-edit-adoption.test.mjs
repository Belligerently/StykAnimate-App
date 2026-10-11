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
  const events = []; let retainedPublication = null, layerAccepted = null;
  // The Native layer owner certifies the accepted operands of quiet sheet layer edits.
  const layerEdit = oracle._layerEdit;
  oracle._layerEdit = function (label, updates, options = {}) {
    const layers = this.report._styk.layers;
    layerAccepted = {label, structural: Boolean(options.structural), ...(options.activeId === undefined ? {} : {activeId: options.activeId}), updates: updates.filter(([path, value, remove = false]) => {
      let object = layers; for (const part of path.slice(0, -1)) object = object[part];
      const key = path.at(-1);
      return JSON.stringify({exists: Object.hasOwn(object, key), value: object[key]}) !== JSON.stringify({exists: !remove, value});
    }).map(([path, value, remove = false]) => ({path: path.slice(), remove, ...(value === undefined ? {} : {value: structuredClone(value)})}))};
    return layerEdit.call(this, label, updates, options);
  };
  oracle.addEventListener('change', event => {
    events.push(clean(event.detail));
    if (event.detail.layerSheet) retainedPublication = oracle._authoredCommitPreview?.frame ?? null;
  });
  return {
    revision: () => oracle.revision, can_undo: () => oracle.canUndo, can_redo: () => oracle.canRedo, free() {},
    call(name, argsText, viewText) {
      const view = JSON.parse(viewText), beforeSheet = oracle.report._styk.layerSheet, beforeLayers = oracle.report._styk.layers;
      const beforeDefinitions = oracle.report.figure_types.length;
      const pasteBefore = (oracle._selectionClipboard ?? oracle._figureClipboard)?.pastes;
      const beforeFrame = clean(oracle.evaluatedFrame);
      const beforeObjects = new Map(beforeFrame.objects.map(object => [object.unique_id, json(object)]));
      events.length = 0; retainedPublication = null; layerAccepted = null;
      let result, error;
      const draws = view.draws?.slice() ?? [];
      const paste = oracle.pasteSelection, ownPaste = Object.hasOwn(oracle, 'pasteSelection'); let duplicateCounter = null;
      const observeDuplicate = view.figureClipboardPastes === true && name === 'duplicateSelection';
      if (observeDuplicate) oracle.pasteSelection = function (...args) {
        const clipboard = this._selectionClipboard ?? this._figureClipboard, before = clipboard?.pastes;
        const result = paste.apply(this, args);
        duplicateCounter = clipboard ? {before, after: clipboard.pastes} : null;
        return result;
      };
      try { ({result} = withDraws(() => oracle[name](...JSON.parse(argsText).map(decode)), () => draws.shift()[1])); }
      catch (caught) { error = caught.messageKey ?? caught.message; }
      finally { if (observeDuplicate) { if (ownPaste) oracle.pasteSelection = paste; else delete oracle.pasteSelection; } }
      const answer = {status: error ? 'error' : 'ok', ...(error ? {error} : {result: result ?? null}), state: state(oracle), events: events.slice(),
        draft: null, drag: oracle._drag ? {before: (oracle._drag.before ?? []).map(object => object.unique_id), translation: oracle._drag.scope === 'translation', label: oracle._drag.label} : null};
      if (view.figureClipboardPastes === true && (name === 'pasteSelection' || name === 'duplicateSelection') && !error) {
        const clipboard = oracle._selectionClipboard ?? oracle._figureClipboard;
        answer.figureClipboardPastes = name === 'duplicateSelection' ? duplicateCounter : clipboard ? {before: pasteBefore, after: clipboard.pastes} : null;
      }
      if (view.layerPublication === true && !error && result !== false && layerAccepted) answer.layerPublication = layerAccepted;
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
        // Native retained_frame is the committed authored preview, not a cache
        // populated later by the original command's selectedObjects return.
        const retained = retainedPublication;
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
      const wire = clean(answer);
      // The full Native view returns the original result, including clip samples.
      // Drafts, retained records and all other body fields keep their wire cleanup.
      if (!error && !view.changesOnly && (name === 'pasteSelection' || name === 'duplicateSelection')) wire.result = result ?? null;
      return JSON.stringify(wire, (key, item) => key === 'runtime_segments' ? undefined : item);
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


// The separate actual Editor supplies the existing publication fields. Native
// Session tests certify the command-specific marker and shallow COW projection.
function uniquePair(document, {layerTimeline = true, historyLimit} = {}) {
  const options = {demo: false, propertyChannels: true, layerTimeline};
  const expected = new Editor(options).load(structuredClone(document));
  const owned = new Editor(options).load(structuredClone(document));
  let oracle = new Editor(options).load(structuredClone(document)), session, current = true;
  const packets = [], requests = [], releases = [], events = [[], []];
  if (historyLimit !== undefined) for (const editor of [expected, owned, oracle]) editor.historyLimit = historyLimit;
  let alter = answer => answer;
  const start = () => {
    session = sessionFor(oracle);
    const call = session.call.bind(session);
    session.call = (name, args, viewText) => {
      if (name === 'beginBatch') return json({status: 'unported', reason: 'this actual-Editor mock has no batch protocol'});
      const view = JSON.parse(viewText), index = oracle.report.figure_types.length;
      if (view.movieclipUniquePublication) requests.push({name, view});
      const answer = JSON.parse(call(name, args, viewText));
      if (view.movieclipUniquePublication && answer.status === 'ok' && answer.definitions?.length === 1
        && answer.events.length === 5) answer.movieclipUnique = {definitionIndex: index};
      if (name === 'makeMovieclipUnique') { packets.push(structuredClone(answer)); alter(answer); }
      return json(answer);
    };
    session.free = () => { releases.push(json(owned.report)); current = false; };
    return session;
  };
  const mirror = {framesCurrent: () => current, syncFrames: async () => {
    // A genuinely new session starts from the current mirrored report/view.
    // Editor.load alone would advance the revision and invalidate the handshake.
    oracle = new Editor(options).load(structuredClone(owned.report));
    for (const key of ['_revision', '_revisionSerial', '_savedRevision', '_layerRevision', '_definitionRevision', '_nextUid', '_nextFrameIdentity']) oracle[key] = owned[key];
    oracle._generatedFrameSources = new Set(owned._generatedFrameSources);
    oracle.historyLimit = owned.historyLimit; oracle._restoreSelection(owned._selectionState()); oracle._invalidate(); current = true;
  }};
  const report = installEditShadow(owned, () => ({engine: {handle: {edit_session: start}}, mirror}), {mode: 'classes'});
  for (const [index, editor] of [expected, owned].entries()) editor.addEventListener('change', event => events[index].push(clean(event.detail)));
  const same = () => {
    assert.equal(json(owned.report), json(expected.report), 'complete stored document');
    assert.deepEqual(state(owned), state(expected), 'history, source counters and selection');
    assert.deepEqual(topologyCounters(owned), topologyCounters(expected), 'original revision and cleanup owners');
    assert.deepEqual(owned._undo, expected._undo, 'literal history entries and children');
    assert.deepEqual(owned._redo, expected._redo, 'literal redo entries');
    assert.equal(json(owned.evaluatedFrame), json(expected.evaluatedFrame), 'displayed frame');
    assert.deepEqual(events[1], events[0], 'ordered change events');
    // Original cached/fresh clip geometry can differ only by an own undefined
    // runtime cache field. Keep real runtime arrays and every numeric value exact.
    const observable = drawables => drawables.map(drawable => drawable.obj && Object.hasOwn(drawable.obj, 'runtime_segments') && drawable.obj.runtime_segments === undefined
      ? {...drawable, obj: Object.fromEntries(Object.entries(drawable.obj).filter(([key]) => key !== 'runtime_segments'))} : drawable);
    assert.deepEqual(observable(owned.geometry.drawables), observable(buildGeometry(owned.report, owned.evaluatedFrame).drawables), 'fresh geometry');
    assert.equal(report.mismatches.length, 0, JSON.stringify(report.mismatches));
  };
  const run = (name, ...args) => {
    let result, error, actual;
    try { result = expected[name](...args); } catch (caught) { error = caught.messageKey ?? caught.message; }
    if (error) assert.throws(() => owned[name](...args), caught => (caught.messageKey ?? caught.message) === error);
    else actual = owned[name](...args);
    same(); if (!error) assert.equal(json(actual), json(result), 'returned value');
    return actual;
  };
  return {expected, owned, get oracle() { return oracle; }, get session() { return session; }, report, packets, requests, releases, events,
    alter: callback => { alter = callback; }, same, run};
}
const uniqueDocument = name => JSON.parse(readFileSync(new URL(`../engine/tests/${name}.document.json`, import.meta.url), 'utf8'));

for (const family of ['edit/clip-unique-sheet', 'movieclip-timed-sheet-expansion/unique-imported', 'movieclip-timed-sheet-expansion/unique-authored']) {
  test(`default Unique adopts the recorded two-child batch and exact event-time state: ${family}`, () => {
    const scenario = JSON.parse(readFileSync(new URL(`../engine/tests/${family}.scenario.json`, import.meta.url), 'utf8'));
    const pair = uniquePair(uniqueDocument(family)), {expected, owned, report, run, same} = pair;
    const observations = [[], []];
    for (const [index, editor] of [expected, owned].entries()) editor.addEventListener('change', event => {
      observations[index].push({event: clean(event.detail), document: json(editor.report), state: state(editor), counters: topologyCounters(editor),
        frame: json(editor.evaluatedFrame), geometry: clean(editor.geometry.drawables), batch: clean(editor._movieclipBatch ?? null)});
    });
    let originals = 0; const original = owned.makeMovieclipUnique;
    // Count computation through its original sheet planner; adoption uses the
    // existing publication owner without running this topology/conversion math.
    const replace = owned.sheetReplaceMovieclipFrames;
    owned.sheetReplaceMovieclipFrames = function (...args) { originals++; return replace.apply(this, args); };
    assert.equal(report.mode, 'classes'); assert.equal(typeof original, 'function');
    for (const [method, ...args] of scenario.steps) {
      const name = method.replace(/^@/, ''), before = report.owned, calls = originals;
      const source = name === 'makeMovieclipUnique' ? owned.figure : null;
      const priorEvents = observations[1].length, tick = owned.tick;
      const result = run(name, ...args.map(decode));
      if (source) {
        assert.equal(report.owned, before + 1, 'actual default adoption'); assert.equal(originals, calls, 'original Unique planner skipped');
        assert.equal(result, owned.report.figure_types.at(-1), 'return the installed definition');
        for (const key of Object.keys(source)) if (!['figure_type_id', 'name'].includes(key)) assert.equal(result[key], source[key], `shallow definition child ${key}`);
        const command = owned._undo.at(-1);
        assert.equal(command.kind, 'movieclip-batch'); assert.deepEqual(command.commands.map(child => child.kind), ['movieclip-definition', 'layer-sheet']);
        assert.deepEqual(command, expected._undo.at(-1));
        const changes = observations[1].slice(priorEvents);
        assert.equal(changes.length, 5); assert.deepEqual(changes.map(change => change.event.kind), ['edit', 'edit', 'seek', 'selection', 'edit']);
        assert.equal(changes[0].state.tick, tick); assert.equal(changes[1].state.tick, tick);
        const finalUid = owned.selectedUid;
        assert.equal(changes[0].state.nextUid, finalUid + 1, 'identity advances before the definition event');
        assert.equal(changes[1].state.nextUid, finalUid + 1);
        assert.equal(changes[2].state.tick, Math.round(tick));
      }
    }
    same(); assert.deepEqual(observations[1], observations[0], 'each event sees the same report, view, revisions and geometry');
    while (expected.canUndo) run('undo'); assert.equal(owned.canUndo, false); same();
    assert.equal(pair.requests.length, family === 'edit/clip-unique-sheet' ? 2 : 1);
  });
}

test('Unique retains shallow unknown definition identities and original zero-history source cleanup', () => {
  const pair = uniquePair(uniqueDocument('movieclip-timed-sheet-expansion/unique-authored'));
  const {expected, owned, run, same, report} = pair;
  for (const editor of [expected, owned, pair.oracle]) {
    editor.select(1); editor.seek(2.25); const figure = editor.figure;
    figure.uniqueUnknown = {nested: ['keep', null]};
    Object.defineProperty(figure, '__proto__', {value: {keep: 'own data'}, enumerable: true, writable: true, configurable: true});
  }
  const source = owned.figure, before = json(owned.report);
  const result = run('makeMovieclipUnique');
  assert.equal(result.uniqueUnknown, source.uniqueUnknown); assert.equal(result['__proto__'], source['__proto__']);
  assert.equal(result._styk, source._styk); assert.equal(result.segments, source.segments);
  assert.deepEqual(Object.keys(result), Object.keys(expected.report.figure_types.at(-1)));
  run('undo'); assert.equal(json(owned.report), before); run('redo');
  // Original source allocation and pruning remain the batch/record owner's,
  // including an actual history-only source from preceding authored movement.
  run('undo'); run('select', 1, -1); run('seek', 2);
  const initialSources = new Set(Object.keys(owned.report._styk.frameSources ?? {}));
  run('moveSelection', 5, -2);
  const first = Object.keys(owned.report._styk.frameSources).filter(id => !initialSources.has(id));
  assert.ok(first.length); run('moveSelection', -3, 4);
  for (const editor of [expected, owned, pair.oracle]) editor.historyLimit = 0;
  const adopted = report.owned;
  run('makeMovieclipUnique'); assert.equal(report.owned, adopted + 1);
  assert.equal(owned.canUndo, false); assert.equal(owned._undo.length, 0); same();
  assert.ok(first.some(id => !Object.hasOwn(owned.report._styk.frameSources, id)), 'original cleanup retires the old history-only source');
});

test('malformed Unique packets retire Native before an unchanged original fallback', () => {
  for (const alter of [
    answer => { delete answer.movieclipUnique; },
    answer => { answer.movieclipUnique.definitionIndex++; },
    answer => { answer.definitions = []; },
    answer => { answer.definitions[0].figure_type_id = 1; },
    answer => { answer.definitions[0].name = 'foreign'; },
    answer => { delete answer.sheet; },
    answer => { delete answer.sources[Object.keys(answer.sources)[0]]; },
    answer => { answer.sheet.tracks[Object.keys(answer.sheet.tracks)[0]].end = -1; },
    answer => { answer.events.reverse(); },
    answer => { answer.events = null; },
    answer => { answer.state.nextUid++; },
    answer => { answer.events[1].layerRevision = -1; },
    answer => { answer.state.layerRevision = answer.events[1].layerRevision - 1; },
    answer => { answer.state.layerRevision += .5; },
    answer => { answer.state = null; },
  ]) {
    const pair = uniquePair(uniqueDocument('movieclip-timed-sheet-expansion/unique-imported'));
    const {expected, owned, report, run, same} = pair;
    run('select', 1); run('seek', 2.25);
    pair.alter(alter); const before = json(owned.report), adopted = report.owned;
    let originals = 0; const original = owned.sheetReplaceMovieclipFrames;
    owned.sheetReplaceMovieclipFrames = function (...args) {
      originals++; assert.equal(pair.releases.length, 1, 'advanced Native session retired before original publication');
      return original.apply(this, args);
    };
    pair.session.free = () => { pair.releases.push(json(owned.report)); assert.equal(json(owned.report), before, 'no adopted write before malformed refusal'); };
    expected.makeMovieclipUnique(); owned.makeMovieclipUnique(); same();
    assert.equal(originals, 1); assert.equal(report.owned, adopted); assert.equal(report.failedOwned, 1);
    assert.deepEqual(owned._undo.at(-1), expected._undo.at(-1)); originalFallbackHistory(pair);
  }
});

test('Unique application errors keep original batch rollback and discard the advanced session before recovery', async () => {
  const pair = uniquePair(uniqueDocument('movieclip-timed-sheet-expansion/unique-imported'));
  const {expected, owned, report, run, same} = pair;
  run('select', 1); run('seek', 2.25);
  const emits = [expected, owned].map(editor => editor._emit);
  for (const [index, editor] of [expected, owned].entries()) editor._emit = function (kind, detail) {
    if (kind === 'seek') throw new Error('Rejected Unique publication');
    return emits[index].call(this, kind, detail);
  };
  assert.throws(() => expected.makeMovieclipUnique(), /Rejected Unique publication/);
  assert.throws(() => owned.makeMovieclipUnique(), /Rejected Unique publication/);
  same(); assert.equal(owned._movieclipBatch, null); assert.equal(pair.releases.length, 1);
  assert.equal(report.ended['a movieclip unique publication failed'], 1);
  for (const [index, editor] of [expected, owned].entries()) editor._emit = emits[index];
  // Both original recorded children rolled back through movieclipBatch. Keep
  // its original consumed identities/revision serial rather than resetting them.
  assert.equal(owned.selectedUid, expected.selectedUid); assert.equal(owned.tick, 2.25);
  await report.prepare(); const sessions = report.sessions, adopted = report.owned;
  run('makeMovieclipUnique'); assert.equal(report.sessions, sessions + 1); assert.equal(report.owned, adopted + 1);
});

test('native, drafted, captured-source, active and batched Unique routes do not request scoped adoption', () => {
  const document = uniqueDocument('edit/clip-unique-sheet');
  for (const [setup, excluded] of [
    [editor => editor.beginFrameDraft({mode: 'update'}), editor => editor._frameDraft],
    [editor => editor.beginDrag('Existing gesture'), editor => editor._drag],
  ]) {
    const pair = uniquePair(document), {expected, owned, report, same} = pair;
    for (const editor of [expected, owned, pair.oracle]) { editor.seek(3); editor.select(4); setup(editor); assert.ok(excluded(editor)); }
    const adopted = report.owned; let error;
    try { expected.makeMovieclipUnique(); } catch (caught) { error = caught.messageKey ?? caught.message; }
    if (error) assert.throws(() => owned.makeMovieclipUnique(), caught => (caught.messageKey ?? caught.message) === error);
    else owned.makeMovieclipUnique();
    same(); assert.equal(report.owned, adopted); assert.equal(pair.requests.length, 0);
  }
  const native = uniquePair(uniqueDocument('edit/clip-unique-legacy-native'), {layerTimeline: false});
  native.run('select', 1); const adopted = native.report.owned; native.run('makeMovieclipUnique');
  assert.equal(native.requests.length, 0); assert.equal(native.report.owned, adopted);
  const source = document.figure_types.find(figure => figure._styk?.movieclip)._styk.movieclip.document;
  const captured = uniquePair(source);
  assert.ok(captured.owned.report._styk.movieclipSource, 'actual captured-source route');
  const before = captured.report.owned;
  captured.run('select', captured.expected.evaluatedFrame.objects[0].unique_id);
  assert.throws(() => captured.expected.makeMovieclipUnique()); assert.throws(() => captured.owned.makeMovieclipUnique()); captured.same();
  assert.equal(captured.requests.length, 0); assert.equal(captured.report.owned, before);
  const origin = uniquePair(JSON.parse(readFileSync(new URL('../engine/tests/edit/origin-pose-layout-sheet.document.json', import.meta.url), 'utf8')));
  assert.equal(editingClass(origin.owned.report), 'unported');
  assert.ok(origin.owned.report.figure_types.some(figure => figure._styk?.animationOrigin));
  origin.run('select', origin.expected.evaluatedFrame.objects[0].unique_id);
  assert.throws(() => origin.expected.makeMovieclipUnique()); assert.throws(() => origin.owned.makeMovieclipUnique()); origin.same();
  assert.equal(origin.requests.length, 0); assert.equal(origin.report.owned, 0);
  const batch = uniquePair(document); batch.run('seek', 3); batch.run('select', 4);
  const count = batch.report.owned;
  for (const editor of [batch.expected, batch.owned]) editor.movieclipBatch('Outer batch', () => editor.makeMovieclipUnique());
  batch.same(); assert.equal(batch.requests.length, 0); assert.equal(batch.report.owned, count);
});

// Project accepted operands at the separate actual Editor's original layer edit
// boundary. Native tests independently certify the opt-in packet before trim.
const folderCalls = ['createLayerFolder', 'renameLayerFolder', 'moveLayersToFolder', 'arrangeLayers', 'deleteLayerFolder'];
const layerMetadataCalls = ['renameLayer', 'setLayerColor', 'setLayerVisible', 'setLayerLocked', 'reorderLayers', 'moveLayerTo', 'createLayer', 'setLayerParallax'];
function folderPair(document, {layerTimeline = true, historyLimit, mode = 'classes', setup} = {}) {
  const options = {demo: false, propertyChannels: true, layerTimeline};
  const expected = new Editor(options).load(structuredClone(document)), owned = new Editor(options).load(structuredClone(document));
  let oracle = new Editor(options).load(structuredClone(document)), session, current = true, alter = () => {};
  const requests = [], packets = [], releases = [], originalCalls = {}, observations = [[], []];
  if (historyLimit !== undefined) for (const editor of [expected, owned, oracle]) editor.historyLimit = historyLimit;
  if (setup) for (const editor of [expected, owned, oracle]) setup(editor);
  for (const name of [...folderCalls, ...layerMetadataCalls]) {
    const original = owned[name]; originalCalls[name] = 0;
    owned[name] = function (...args) { originalCalls[name]++; return original.apply(this, args); };
  }
  const start = () => {
    let accepted = null;
    const original = oracle._layerEdit;
    oracle._layerEdit = function (label, updates, options = {}) {
      const state = this.report._styk.layers;
      accepted = {label, structural: Boolean(options.structural), ...(options.activeId === undefined ? {} : {activeId: options.activeId}), updates: updates.filter(([path, value, remove = false]) => {
        let object = state; for (const part of path.slice(0, -1)) object = object[part];
        const key = path.at(-1), before = {exists: Object.hasOwn(object, key), value: object[key]}, after = {exists: !remove, value};
        return JSON.stringify(before) !== JSON.stringify(after);
      }).map(([path, value, remove = false]) => ({path: path.slice(), remove, ...(value === undefined ? {} : {value: structuredClone(value)})}))};
      return original.call(this, label, updates, options);
    };
    session = sessionFor(oracle);
    const call = session.call.bind(session);
    session.call = (name, args, viewText) => {
      if (name === 'beginBatch') return json({status: 'unported', reason: 'this actual-Editor mock has no batch protocol'});
      accepted = null; const view = JSON.parse(viewText);
      if (view.layerPublication) requests.push({name, view});
      const answer = JSON.parse(call(name, args, viewText));
      if (view.layerPublication && answer.status === 'ok' && answer.result !== false && accepted) answer.layerPublication = accepted;
      if (folderCalls.includes(name) || layerMetadataCalls.includes(name)) { packets.push(structuredClone(answer)); alter(answer); }
      return json(answer);
    };
    session.free = () => { releases.push(json(owned.report)); current = false; oracle._layerEdit = original; };
    return session;
  };
  const mirror = {framesCurrent: () => current, syncFrames: async () => {
    oracle = new Editor(options).load(structuredClone(owned.report));
    for (const key of ['_revision', '_revisionSerial', '_savedRevision', '_layerRevision', '_definitionRevision', '_nextUid', '_nextFrameIdentity']) oracle[key] = owned[key];
    oracle._generatedFrameSources = new Set(owned._generatedFrameSources); oracle.historyLimit = owned.historyLimit;
    oracle._restoreSelection(owned._selectionState()); oracle._invalidate(); current = true;
  }};
  const report = installEditShadow(owned, () => ({engine: {handle: {edit_session: start}}, mirror}), {mode});
  for (const [index, editor] of [expected, owned].entries()) editor.addEventListener('change', event => observations[index].push({event: clean(event.detail),
    document: json(editor.report), state: state(editor), counters: topologyCounters(editor), history: json(editor._undo), redo: json(editor._redo),
    frame: json(editor.evaluatedFrame), geometry: clean(editor.geometry.drawables)}));
  const same = () => {
    assert.equal(json(owned.report), json(expected.report), 'complete layer document');
    assert.deepEqual(state(owned), state(expected), 'selection and history view');
    assert.deepEqual(topologyCounters(owned), topologyCounters(expected), 'revision, identities, history size and source cleanup');
    assert.deepEqual(owned._undo, expected._undo, 'literal Layers history including value presence');
    assert.deepEqual(owned._redo, expected._redo, 'literal redo entries');
    assert.equal(json(owned.evaluatedFrame), json(expected.evaluatedFrame), 'displayed frame');
    assert.deepEqual(observations[1], observations[0], 'event-time document, history, counters and geometry');
    assert.deepEqual(owned.geometry.drawables, buildGeometry(owned.report, owned.evaluatedFrame).drawables, 'fresh numeric geometry');
    assert.equal(report.mismatches.length, 0, JSON.stringify(report.mismatches));
  };
  const run = (name, ...args) => {
    let result, actual, error;
    try { result = expected[name](...args); } catch (caught) { error = caught.messageKey ?? caught.message; }
    if (error) assert.throws(() => owned[name](...args), caught => (caught.messageKey ?? caught.message) === error);
    else actual = owned[name](...args);
    same(); if (!error) assert.equal(json(actual), json(result), 'original result');
    return {result: actual, error};
  };
  return {expected, owned, get oracle() { return oracle; }, get session() { return session; }, report, requests, packets, releases,
    originalCalls, observations, same, run, alter: callback => { alter = callback; }};
}
const folderDocument = () => JSON.parse(readFileSync(new URL('../engine/tests/edit/sheet-layers.document.json', import.meta.url), 'utf8'));

for (const family of ['layers-folders-sheet', 'layer-list-sheet', 'layers-native']) {
  test(`layer folder adoption preserves each recorded original boundary: ${family}`, () => {
    const folder = new URL('../engine/tests/edit/', import.meta.url), scenario = JSON.parse(readFileSync(new URL(`${family}.scenario.json`, folder), 'utf8'));
    const document = JSON.parse(readFileSync(new URL(scenario.document, folder), 'utf8'));
    // Adjacent layer-list/native tapes contain no folder calls. Keep their
    // original event-time oracle in shadow mode; only the folder tape adopts.
    const mode = family === 'layers-folders-sheet' ? 'classes' : 'shadow';
    const pair = folderPair(document, {layerTimeline: Boolean(scenario.editor.layerTimeline), mode}), {expected, owned, report, run} = pair;
    for (const [method, ...args] of scenario.steps) {
      const name = method.replace(/^@/, ''), adopted = report.owned, original = pair.originalCalls[name];
      const {error} = run(name, ...args.map(decode));
      if (folderCalls.includes(name) && !error) {
        assert.equal(report.owned, adopted + 1, 'the folder result was actually adopted');
        assert.equal(pair.originalCalls[name], original, 'the original folder planner was skipped');
      }
    }
    if (family === 'layers-folders-sheet') {
      for (const name of folderCalls) assert.ok(pair.requests.some(request => request.name === name), `actual ${name} packet requested`);
      assert.ok(pair.packets.some(answer => answer.status === 'error'), 'recorded errors replay the original owner');
      while (expected.canUndo) run('undo'); while (expected.canRedo) run('redo');
    }
    if (!scenario.editor.layerTimeline) { assert.equal(pair.requests.length, 0); assert.equal(report.owned, 0); }
    assert.equal(report.mode, mode); assert.equal(owned.layerTimeline, expected.layerTimeline);
  });
}

test('folder null and omitted removals preserve accepted history operands, no-ops and immutable counters', () => {
  const document = folderDocument(); document.folderUnknown = {keep: ['document', null]};
  document._styk.layers.byId['layer-1'].folderUnknown = {keep: ['layer', null]};
  const pair = folderPair(document), {owned, expected, report, run} = pair;
  const layer = owned.report._styk.layers.byId['layer-1'], unknown = layer.folderUnknown;
  const unchanged = () => ({nextUid: owned._nextUid, nextFrame: owned._nextFrameIdentity, definition: owned.definitionRevision,
    frames: owned.report.keyframes, sources: owned.report._styk.frameSources, counts: owned._layerUidCounts, groups: owned._layerGroups,
    arrays: [...owned._layerUidArrays], tracks: {...owned.report._styk.layerSheet.tracks}});
  for (const editor of [expected, owned, pair.oracle]) editor.getLayers();
  const before = unchanged();
  const created = run('createLayerFolder', 'Group', ['layer-1']).result;
  assert.deepEqual(created, {id: 'folder-1', kind: 'folder', name: 'Group'});
  const folder = owned.report._styk.layers.folders.byId[created.id];
  run('renameLayerFolder', created.id, ' Renamed ');
  assert.equal(owned.report._styk.layers.folders.byId[created.id], folder, 'rename keeps the existing folder record');
  const revision = owned.revision, events = pair.observations[1].length, adopted = report.owned;
  assert.equal(run('renameLayerFolder', created.id, 'Renamed').result, false);
  assert.equal(report.owned, adopted + 1, 'packet-free no-op was adopted'); assert.equal(owned.revision, revision);
  assert.equal(pair.observations[1].length, events, 'no event or history for unchanged name');
  run('arrangeLayers', owned.report._styk.layers.order.slice(), ['layer-1']);
  assert.equal(owned.report._styk.layers.byId['layer-1'].folderId, created.id, 'omitted destination preserves membership');
  run('moveLayersToFolder', ['layer-2'], null);
  const nullRemoval = owned._undo.at(-1).changes.find(change => change.path.join('.') === 'byId.layer-2.folderId');
  assert.deepEqual(nullRemoval.before, {exists: false, value: undefined});
  assert.deepEqual(nullRemoval.after, {exists: false, value: null}, 'null removal on absent leaf deliberately records');
  run('arrangeLayers', owned.report._styk.layers.order.slice(), ['layer-1'], null);
  assert.equal(Object.hasOwn(owned.report._styk.layers.byId['layer-1'], 'folderId'), false);
  run('moveLayersToFolder', ['layer-1'], created.id);
  run('deleteLayerFolder', created.id);
  const deleted = owned._undo.at(-1).changes;
  assert.ok(deleted.every(change => change.after.exists === false && Object.hasOwn(change.after, 'value') && change.after.value === undefined));
  const after = unchanged();
  assert.equal(owned.report._styk.layers.byId['layer-1'], layer); assert.equal(layer.folderUnknown, unknown, 'unknown layer child identity survives');
  assert.deepEqual(owned.report.folderUnknown, document.folderUnknown);
  for (const key of ['nextUid', 'nextFrame', 'frames', 'sources', 'counts', 'groups']) assert.equal(after[key], before[key], `unchanged ${key}`);
  for (const [id, track] of Object.entries(before.tracks)) assert.equal(after.tracks[id], track, `untouched track ${id}`);
  for (const [id, array] of before.arrays) assert.equal(owned._layerUidArrays.get(id), array, `existing UID array ${id}`);
  for (const observation of pair.observations[1]) if (observation.event.kind === 'edit') {
    assert.equal(observation.event.layerSheet, undefined); assert.equal(observation.event.timing, false);
    assert.equal(observation.event.animation, false); assert.equal(observation.event.layerMetadata, true);
  }
  while (expected.canUndo) run('undo'); while (expected.canRedo) run('redo');
});

test('folder history zero uses original source pruning and retained sheet-track identities', () => {
  let first;
  const pair = folderPair(folderDocument(), {setup(editor) {
    editor.select(1, -1); editor.moveSelection(5, -2);
    first ??= new Set(Object.keys(editor.report._styk.frameSources)); editor.moveSelection(-3, 4);
  }}), {expected, owned, run, same} = pair;
  for (const editor of [expected, owned, pair.oracle]) editor.historyLimit = 0;
  const tracks = {...owned.report._styk.layerSheet.tracks}, sources = owned.report._styk.frameSources, adopted = pair.report.owned;
  run('createLayerFolder', 'Zero history', ['layer-1']);
  assert.equal(pair.report.owned, adopted + 1); assert.equal(owned._undo.length, 0); assert.equal(owned.canUndo, false);
  assert.ok([...first].some(id => !Object.hasOwn(sources, id)), 'old history-only source really dropped');
  for (const [id, track] of Object.entries(tracks)) assert.equal(owned.report._styk.layerSheet.tracks[id], track);
  same();
});

test('malformed folder packets retire advanced Native before untouched original fallback', () => {
  for (const alter of [
    answer => { delete answer.layerPublication; }, answer => { answer.layerPublication.label = 'foreign'; },
    answer => { answer.layerPublication.structural = false; }, answer => { answer.layerPublication.updates[0].path = ['activeId']; },
    answer => { answer.layerPublication.updates[0].remove = 'yes'; }, answer => { delete answer.layerPublication.updates[0].value; },
    answer => { answer.layerPublication.updates.push(answer.layerPublication.updates[0]); },
    answer => { answer.layerPublication.updates[0].value.byId['folder-1'].name = ''; },
    answer => { answer.layerPublication.updates.at(-1).value = ['layer-1']; },
    answer => { answer.events[0].layerSheet = true; }, answer => { answer.events = null; }, answer => { answer.state.nextUid++; },
    answer => { answer.result.id = 'folder-99'; },
    answer => { answer.result = false; answer.events = []; delete answer.layerPublication; },
  ]) {
    const pair = folderPair(folderDocument()), {expected, owned, report, same} = pair;
    pair.alter(alter); const before = json(owned.report), original = owned._layerEdit, adopted = report.owned;
    owned._layerEdit = function (...args) {
      assert.equal(pair.releases.length, 1, 'Native was retired before the original layer owner');
      assert.equal(pair.releases[0], before, 'validation happened before the first adopted write');
      return original.apply(this, args);
    };
    expected.createLayerFolder('Group', ['layer-1']); owned.createLayerFolder('Group', ['layer-1']); same();
    assert.equal(pair.originalCalls.createLayerFolder, 1); assert.equal(report.owned, adopted); assert.equal(report.failedOwned, 1);
    originalFallbackHistory(pair);
  }
});

test('folder publication errors preserve original partial effects and discard the session before recovery', async () => {
  const pair = folderPair(folderDocument()), {expected, owned, report, run, same} = pair;
  const emits = [expected, owned].map(editor => editor._emit);
  for (const [index, editor] of [expected, owned].entries()) editor._emit = function (kind, detail) {
    if (kind === 'edit' && detail.layerMetadata) throw new Error('Rejected folder event');
    return emits[index].call(this, kind, detail);
  };
  assert.throws(() => expected.createLayerFolder('First'), /Rejected folder event/);
  assert.throws(() => owned.createLayerFolder('First'), /Rejected folder event/); same();
  assert.equal(pair.releases.length, 1); assert.equal(report.ended['a layer folder publication failed'], 1);
  for (const [index, editor] of [expected, owned].entries()) editor._emit = emits[index];
  await report.prepare(); const sessions = report.sessions, adopted = report.owned;
  run('createLayerFolder', 'Recovered'); assert.equal(report.sessions, sessions + 1); assert.equal(report.owned, adopted + 1);
});

test('native, source, origin, active and batched folder routes retain their original planner', () => {
  for (const [document, options, setup, excluded] of [
    [folderDocument(), {layerTimeline: false}, () => {}, editor => !editor.layerTimeline],
    [folderDocument(), {}, editor => { editor.select(editor.evaluatedFrame.objects[0].unique_id, -1); editor.beginDrag('Active gesture'); }, editor => editor._drag],
    [folderDocument(), {}, editor => editor.beginFrameDraft({mode: 'update'}), editor => editor._frameDraft],
    [JSON.parse(readFileSync(new URL('../engine/tests/edit/origin-pose-layout-sheet.document.json', import.meta.url), 'utf8')), {}, () => {}, editor => editingClass(editor.report) === 'unported'],
    [uniqueDocument('edit/clip-unique-sheet').figure_types.find(figure => figure._styk?.movieclip)._styk.movieclip.document, {}, () => {}, editor => editor.report._styk.movieclipSource],
  ]) {
    const pair = folderPair(document, options);
    for (const editor of [pair.expected, pair.owned, pair.oracle]) { setup(editor); assert.ok(excluded(editor), 'the actual excluded owner is present'); }
    const adopted = pair.report.owned, requests = pair.requests.length;
    pair.run('createLayerFolder', 'Original route');
    assert.equal(pair.report.owned, adopted); assert.equal(pair.requests.length, requests); assert.equal(pair.originalCalls.createLayerFolder, 1);
  }
  const pair = folderPair(folderDocument()), adopted = pair.report.owned;
  for (const editor of [pair.expected, pair.owned]) editor.movieclipBatch('Folder batch', () => editor.createLayerFolder('Inside'));
  pair.same(); assert.equal(pair.requests.length, 0); assert.equal(pair.report.owned, adopted); assert.equal(pair.originalCalls.createLayerFolder, 1);
});

const figureClipboard = editor => editor._selectionClipboard ?? editor._figureClipboard ?? null;
const figureClipboardCount = editor => figureClipboard(editor)?.pastes ?? null;
function clipboardPair({mode = 'classes'} = {}) {
  const reference = JSON.parse(readFileSync(new URL('../engine/tests/edit/figure-clipboard-pastes-sheet.reference.json', import.meta.url), 'utf8'));
  const document = JSON.parse(reference.steps[0].document), options = {demo: false, propertyChannels: true, layerTimeline: true};
  const [expected, owned, initial] = Array.from({length: 3}, () => new Editor(options).load(structuredClone(document)));
  for (const editor of [expected, owned, initial]) editor.autoKey = Boolean(reference.editor.autoKey);
  let oracle = initial, current = true, alter = () => {}, session;
  const requests = [], packets = [], releases = [], observations = [[], []], originalCalls = {pasteSelection: 0, duplicateSelection: 0};
  for (const name of Object.keys(originalCalls)) {
    const original = owned[name]; owned[name] = function (...args) { originalCalls[name]++; return original.apply(this, args); };
  }
  const start = () => {
    session = sessionFor(oracle); const call = session.call.bind(session);
    session.call = (name, args, viewText) => {
      const view = JSON.parse(viewText);
      if (view.figureClipboardPastes === true) { assert.ok(name === 'pasteSelection' || name === 'duplicateSelection'); requests.push({name, view}); }
      const answer = JSON.parse(call(name, args, viewText));
      if (name === 'pasteSelection' || name === 'duplicateSelection') { alter(answer); packets.push(structuredClone(answer)); }
      return JSON.stringify(answer);
    };
    session.free = () => { releases.push({document: json(owned.report), state: state(owned), pastes: figureClipboardCount(owned)}); current = false; };
    return session;
  };
  const mirror = {framesCurrent: () => current, syncFrames: async () => {
    oracle = new Editor(options).load(structuredClone(owned.report));
    oracle.autoKey = owned.autoKey;
    for (const key of ['_revision', '_revisionSerial', '_savedRevision', '_layerRevision', '_definitionRevision', '_nextUid', '_nextFrameIdentity']) oracle[key] = owned[key];
    oracle._generatedFrameSources = new Set(owned._generatedFrameSources); oracle.historyLimit = owned.historyLimit;
    oracle._restoreSelection(owned._selectionState()); oracle._invalidate(); current = true;
  }};
  const report = installEditShadow(owned, () => ({engine: {handle: {edit_session: start}}, mirror}), {mode});
  for (const [index, editor] of [expected, owned].entries()) editor.addEventListener('change', event => observations[index].push({event: clean(event.detail), pastes: figureClipboardCount(editor)}));
  let callIndex = 0, context = 'initial';
  const same = () => {
    assert.equal(json(owned.report), json(expected.report), 'complete document');
    assert.deepEqual(state(owned), state(expected), 'history, selection and identities');
    assert.deepEqual(owned._undo, expected._undo, 'literal undo history'); assert.deepEqual(owned._redo, expected._redo, 'literal redo history');
    const actualFrame = owned.evaluatedFrame, expectedFrame = expected.evaluatedFrame;
    assert.equal(json(actualFrame), json(expectedFrame), 'complete displayed frame');
    const actualGeometry = json(owned.geometry.drawables), expectedGeometry = json(expected.geometry.drawables);
    let difference;
    if (actualGeometry !== expectedGeometry) {
      const first = (actual, expected, path = 'drawables') => {
        if (Object.is(actual, expected)) return null;
        if (!actual || !expected || typeof actual !== 'object' || typeof expected !== 'object') return {path, actual, expected};
        const keys = Object.keys(actual), expectedKeys = Object.keys(expected);
        if (JSON.stringify(keys) !== JSON.stringify(expectedKeys)) return {path, actualKeys: keys, expectedKeys};
        for (const key of keys) { const result = first(actual[key], expected[key], `${path}.${key}`); if (result) return result; }
        return null;
      };
      const samples = frame => frame.objects.map(object => ({uid: object.unique_id, sample: object._movieclipSample}));
      difference = {first: first(JSON.parse(actualGeometry), JSON.parse(expectedGeometry)), actualSamples: samples(actualFrame), expectedSamples: samples(expectedFrame)};
    }
    assert.equal(actualGeometry, expectedGeometry, `exact numeric geometry at ${context}: ${JSON.stringify(difference)}`);
    assert.equal(figureClipboardCount(owned), figureClipboardCount(expected), 'current clipboard count');
    assert.deepEqual(observations[1], observations[0], 'literal event-time counters and events');
    assert.equal(report.mismatches.length, 0, JSON.stringify(report.mismatches));
  };
  const run = (name, ...args) => {
    context = `${++callIndex}: ${name}(${JSON.stringify(args)})`;
    const before = [expected, owned].map(editor => ({clipboard: figureClipboard(editor), pastes: figureClipboardCount(editor)}));
    for (const events of observations) events.length = 0;
    let result, actual, error, actualError;
    try { result = expected[name](...args); } catch (caught) { error = caught; }
    try { actual = owned[name](...args); } catch (caught) { actualError = caught; }
    assert.equal(actualError?.messageKey ?? actualError?.message, error?.messageKey ?? error?.message, 'original error');
    assert.equal(actualError?.message, error?.message, 'original translated message');
    same(); if (!error) assert.equal(json(actual), json(result), 'original return value');
    const counters = [expected, owned].map((editor, index) => ({before: before[index].pastes, after: figureClipboardCount(editor),
      events: observations[index].map(event => event.pastes), sameClipboard: figureClipboard(editor) === before[index].clipboard}));
    assert.deepEqual(counters[1], counters[0], 'saved clipboard identity and all event observations');
    return {result: actual, error: actualError, counters: counters[1]};
  };
  return {expected, owned, get oracle() { return oracle; }, report, reference, requests, packets, releases, observations, originalCalls, same, run,
    alter(callback) { alter = callback; }};
}

test('default paste adoption matches all 33 recorded clipboard counters, events and saved identities', () => {
  const pair = clipboardPair(), {owned, expected, report, reference, run} = pair;
  assert.equal(report.mode, 'classes'); assert.equal(editingClass(owned.report), 'movieclips');
  for (const [index, [method, ...args]] of reference.steps.slice(1).map(step => step.call).entries()) {
    const name = method.replace(/^@/, ''), adopted = report.owned, original = pair.originalCalls[name];
    const {error, counters} = run(name, ...args.map(decode));
    assert.deepEqual(counters, reference.steps[index + 1].figureClipboardPastes, `recorded step ${index + 1}`);
    if (name === 'pasteSelection' || name === 'duplicateSelection') {
      assert.equal(pair.originalCalls[name], original + (error ? 1 : 0), 'success skips the original top-level method; errors replay that owner');
      if (!error) assert.equal(report.owned, adopted + 1, 'actual successful adoption');
      else assert.equal(report.owned, adopted, 'original error is not a successful adoption');
    }
  }
  assert.ok(pair.requests.length > 0); assert.ok(pair.packets.some(answer => answer.figureClipboardPastes === null));
  assert.ok(pair.packets.some(answer => answer.figureClipboardPastes?.after > answer.figureClipboardPastes?.before));
  assert.ok(pair.packets.some(answer => answer.status === 'error' && !Object.hasOwn(answer, 'figureClipboardPastes')));
  while (expected.canUndo) run('undo'); while (expected.canRedo) run('redo');
});

test('zero increments do not write the saved counter, no clipboard returns [] with a selected figure', () => {
  const pair = clipboardPair(), {expected, owned, run, report} = pair;
  run('select', 4, -1); const before = report.owned;
  assert.deepEqual(run('pasteSelection').result, []); assert.equal(owned.selectedUid, 4); assert.equal(report.owned, before + 1);
  run('copySelection'); const saved = [expected, owned].map(figureClipboard), writes = [[], []];
  for (const [index, clipboard] of saved.entries()) {
    let value = clipboard.pastes;
    Object.defineProperty(clipboard, 'pastes', {enumerable: true, configurable: true, get: () => value, set(next) { writes[index].push(next); value = next; }});
  }
  run('pasteSelection', {offset: 0}); run('pasteSelection', {offset: -0});
  run('pasteSelection', {offset: 'x'}); run('pasteSelection', {pasteMode: 'bogus'});
  run('selectMany', []); run('duplicateSelection');
  assert.deepEqual(writes, [[], []], 'zero, error and no-copy routes perform no saved counter assignment');
  run('pasteSelection'); assert.deepEqual(writes, [[1], [1]], 'one authoritative increment uses the original existing property');
  assert.equal(figureClipboard(owned), saved[1]);
});

test('malformed or absent paste certificates retire Native before any original fallback write', () => {
  for (const method of ['pasteSelection', 'duplicateSelection']) for (const change of [
    answer => { delete answer.figureClipboardPastes; }, answer => { answer.figureClipboardPastes = null; },
    answer => { answer.figureClipboardPastes = 1; }, answer => { answer.figureClipboardPastes = []; },
    answer => { delete answer.figureClipboardPastes.before; }, answer => { delete answer.figureClipboardPastes.after; },
    answer => { answer.figureClipboardPastes.before = 1; }, answer => { answer.figureClipboardPastes.before = -1; },
    answer => { answer.figureClipboardPastes.before = .5; }, answer => { answer.figureClipboardPastes.after = null; },
    answer => { answer.figureClipboardPastes.after = 2; }, answer => { answer.figureClipboardPastes.after = -1; },
    answer => { answer.figureClipboardPastes.after = .5; }, answer => { answer.figureClipboardPastes.extra = true; },
  ]) {
    const pair = clipboardPair(), {owned, expected, run, report} = pair;
    run('select', 4, -1); run('copySelection'); pair.alter(change);
    const before = {document: json(owned.report), state: state(owned), pastes: figureClipboardCount(owned)}, adopted = report.owned;
    run(method);
    assert.deepEqual(pair.releases, [before], 'validation retires the advanced session while JS is still untouched');
    assert.equal(report.owned, adopted); assert.equal(report.failedOwned, 1); assert.equal(pair.originalCalls[method], 1);
    assert.deepEqual(owned._undo.at(-1), expected._undo.at(-1));
  }
  for (const value of [{before: 0, after: 0}, 0, [], false]) {
    const pair = clipboardPair(); pair.run('select', 4, -1);
    pair.alter(answer => { answer.figureClipboardPastes = value; });
    assert.deepEqual(pair.run('pasteSelection').result, []); assert.equal(pair.releases.length, 1); assert.equal(pair.report.failedOwned, 1);
  }
});

test('Native before rejects ambiguous counter drift and permits a fresh copied-session recovery', async () => {
  const pair = clipboardPair(), {expected, owned, report, run} = pair;
  run('select', 4, -1); run('copySelection');
  for (const editor of [expected, owned]) figureClipboard(editor).pastes = 1;
  const adopted = report.owned; run('pasteSelection');
  assert.deepEqual(pair.packets.at(-1).figureClipboardPastes, {before: 0, after: 1}, 'after alone would falsely match JS before');
  assert.equal(report.owned, adopted); assert.equal(pair.releases.length, 1); assert.equal(pair.releases[0].pastes, 1);
  assert.equal(figureClipboardCount(owned), 2, 'fallback increment uses the original drifted counter');
  await report.prepare(); const sessions = report.sessions;
  run('select', 4, -1); run('copySelection'); const pasted = report.owned; run('pasteSelection');
  assert.equal(report.sessions, sessions + 1); assert.equal(report.owned, pasted + 1); assert.equal(figureClipboardCount(owned), 1);
});

test('paste event mutation or replacement increments the captured old object after all events', () => {
  for (const replace of [false, true]) {
    const pair = clipboardPair(), {expected, owned, run, report} = pair;
    run('select', 4, -1); run('copySelection'); const saved = [expected, owned].map(figureClipboard);
    for (const [index, editor] of [expected, owned].entries()) {
      let first = true;
      editor.addEventListener('change', event => {
        if (!first || !event.detail.layerSheet) return; first = false;
        assert.equal(saved[index].pastes, 0, 'event sees the pre-call counter');
        if (replace) editor._selectionClipboard = {...saved[index], pastes: 50};
        else saved[index].pastes = 10;
      });
    }
    const adopted = report.owned; run('pasteSelection'); assert.equal(report.owned, adopted + 1);
    assert.deepEqual(saved.map(clipboard => clipboard.pastes), replace ? [1, 1] : [11, 11]);
    assert.equal(figureClipboardCount(owned), replace ? 50 : 11);
    const requests = pair.requests.length, pasted = report.owned; run('pasteSelection');
    assert.equal(report.owned, pasted, 'changed clipboard or Native before drift retains original fallback');
    assert.equal(pair.requests.length, requests + (replace ? 0 : 1)); assert.equal(pair.releases.length, 1);
  }
});

test('duplicate child events complete the retained temporary clipboard then restore both saved owners', () => {
  for (const offset of [undefined, 0]) for (const replace of [false, true]) {
    const pair = clipboardPair(), {expected, owned, run, report} = pair;
    run('select', 4, -1); run('copySelection'); run('pasteSelection'); run('select', 4, -1);
    const saved = [expected, owned].map(editor => [editor._selectionClipboard, editor._figureClipboard]), retained = [], writes = [[], []];
    for (const [index, editor] of [expected, owned].entries()) editor.addEventListener('change', event => {
      if (!event.detail.layerSheet) return;
      const temporary = figureClipboard(editor); retained[index] = temporary;
      assert.equal(temporary.pastes, 0, 'fresh child clipboard is installed at the edit event');
      let value = temporary.pastes;
      Object.defineProperty(temporary, 'pastes', {enumerable: true, configurable: true, get: () => value, set(next) { writes[index].push(next); value = next; }});
      if (replace) editor._selectionClipboard = {...temporary, pastes: 50};
      else temporary.pastes = 10;
    });
    const adopted = report.owned; run('duplicateSelection', offset === undefined ? {} : {offset}); assert.equal(report.owned, adopted + 1);
    const increment = offset === undefined ? 1 : 0;
    assert.deepEqual(retained.map(clipboard => clipboard.pastes), replace ? [increment, increment] : [10 + increment, 10 + increment], 'retained temporary object observes original postevent completion');
    assert.deepEqual(writes[1], writes[0], 'explicit zero causes no postevent assignment');
    assert.deepEqual(writes[1], replace ? increment ? [1] : [] : increment ? [10, 11] : [10]);
    for (const [index, editor] of [expected, owned].entries()) {
      assert.equal(editor._selectionClipboard, saved[index][0]); assert.equal(editor._figureClipboard, saved[index][1]);
      assert.equal(figureClipboardCount(editor), 1, 'temporary event changes never reach the saved counter');
    }
    assert.equal(pair.originalCalls.duplicateSelection, 0); assert.equal(pair.requests.length, 2);
    assert.deepEqual(pair.packets.at(-1).figureClipboardPastes, {before: 0, after: increment}, 'authoritative temporary certificate');
  }
});

test('paste and duplicate publication errors keep original effects, counter and recovery boundary', async () => {
  for (const method of ['pasteSelection', 'duplicateSelection']) {
    const pair = clipboardPair(), {expected, owned, report, run, same} = pair;
    run('select', 4, -1); run('copySelection'); const saved = [expected, owned].map(editor => [editor._selectionClipboard, editor._figureClipboard]);
    const emit = [expected, owned].map(editor => editor._emit), failure = new Error('Rejected figure paste event');
    for (const [index, editor] of [expected, owned].entries()) editor._emit = function (kind, detail) {
      if (kind === 'edit' && detail.layerSheet) throw failure;
      return emit[index].call(this, kind, detail);
    };
    assert.throws(() => expected[method](), error => error === failure); assert.throws(() => owned[method](), error => error === failure); same();
    assert.equal(pair.releases.length, 1); assert.equal(report.ended['a figure paste publication failed'], 1);
    for (const [index, editor] of [expected, owned].entries()) {
      assert.equal(editor._selectionClipboard, saved[index][0]); assert.equal(editor._figureClipboard, saved[index][1]);
      assert.equal(figureClipboardCount(editor), 0, 'failed publication never increments the saved clipboard'); editor._emit = emit[index];
    }
    await report.prepare(); const sessions = report.sessions;
    run('select', 4, -1); run('copySelection'); const pasted = report.owned; run('pasteSelection');
    assert.equal(report.sessions, sessions + 1); assert.equal(report.owned, pasted + 1);
  }
});

test('foreign clipboard and comparison-only calls preserve the original guard and literal request exclusion', () => {
  const pair = clipboardPair(), {expected, owned, report, run} = pair;
  run('select', 4, -1); run('copySelection');
  for (const editor of [expected, owned]) editor._selectionClipboard = {...figureClipboard(editor)};
  const requests = pair.requests.length, adopted = report.owned; run('pasteSelection');
  assert.equal(pair.requests.length, requests); assert.equal(report.owned, adopted); assert.equal(pair.originalCalls.pasteSelection, 1);
  assert.equal(report.ended['a figure clipboard the session did not copy'], 1);
  const shadow = clipboardPair({mode: 'shadow'}); shadow.run('select', 4, -1); shadow.run('copySelection'); shadow.run('pasteSelection');
  assert.equal(shadow.requests.length, 0); assert.equal(shadow.report.owned, 0);
});


for (const family of ['layer-list-sheet', 'layers-folders-sheet', 'layers-native', 'layer-parallax-sheet', 'layer-parallax-camera-sheet']) {
  test(`layer metadata adoption preserves every recorded original boundary: ${family}`, () => {
    const folder = new URL('../engine/tests/edit/', import.meta.url), scenario = JSON.parse(readFileSync(new URL(`${family}.scenario.json`, folder), 'utf8'));
    const reference = JSON.parse(readFileSync(new URL(`${family}.reference.json`, folder), 'utf8'));
    const document = JSON.parse(readFileSync(new URL(scenario.document, folder), 'utf8'));
    const pair = folderPair(document, {layerTimeline: Boolean(scenario.editor.layerTimeline), mode: scenario.editor.layerTimeline ? 'classes' : 'shadow'});
    const {expected, owned, report, run} = pair;
    let stored = reference.steps[0].document;
    for (const [index, [method, ...args]] of scenario.steps.entries()) {
      const name = method.replace(/^@/, ''), before = report.owned, original = pair.originalCalls[name];
      // This tape's adjacent joined move is not a metadata command. Exercise its
      // unchanged original owner in all three editors, retaining exact geometry.
      let error;
      if (name === 'moveSelection') {
        for (const editor of [expected, owned, pair.oracle]) Editor.prototype.moveSelection.apply(editor, args.map(decode));
        pair.same();
      } else ({error} = run(name, ...args.map(decode)));
      if (scenario.editor.layerTimeline && layerMetadataCalls.includes(name)) {
        assert.equal(pair.originalCalls[name], original + (error ? 1 : 0), 'metadata success skips its original planner; errors preserve that owner');
        assert.equal(report.owned, before + (error ? 0 : 1), 'successful changed and false/no-op answers are actually adopted');
      }
      const step = reference.steps[index + 1]; if (step.document !== undefined) stored = step.document;
      assert.equal(json(owned.report), stored, `${family} step${index + 1}: literal recorded document`);
      assert.deepEqual(state(owned), step.state, `${family} step${index + 1}: literal recorded state`);
    }
    if (!scenario.editor.layerTimeline) { assert.equal(report.owned, 0); assert.equal(pair.requests.length, 0); }
    while (expected.canUndo) run('undo'); while (expected.canRedo) run('redo');
    if (family === 'layer-list-sheet') for (const name of layerMetadataCalls.filter(name => name !== 'setLayerParallax')) {
      assert.ok(pair.requests.some(request => request.name === name), `actual opt-in ${name} request`);
    }
    if (family.startsWith('layer-parallax')) assert.ok(pair.requests.some(request => request.name === 'setLayerParallax'));
  });
}

test('metadata operands preserve unknown identities, null-removal history and unchanged track nodes', () => {
  const document = folderDocument(); document._styk.layers.byId['layer-1'].unknownMetadata = {keep: [null, {value: 3}]};
  const pair = folderPair(document), {owned, expected, run, report} = pair;
  const layer = owned.report._styk.layers.byId['layer-1'], unknown = layer.unknownMetadata, tracks = {...owned.report._styk.layerSheet.tracks};
  const before = report.owned;
  run('renameLayer', 'layer-1', '  Renamed  '); run('setLayerColor', ['layer-1', 'layer-2', 'layer-1'], 'violet');
  run('setLayerColor', ['layer-1'], null); run('setLayerColor', ['layer-1'], null);
  assert.equal(owned._undo.at(-1).changes[0].after.exists, false); assert.equal(owned._undo.at(-1).changes[0].after.value, null);
  run('setLayerVisible', 'scene', false); run('setLayerLocked', 'camera', true); run('setLayerLocked', 'camera', false);
  run('reorderLayers', ['layer-2', 'layer-1']); run('moveLayerTo', 'layer-1', 0); run('setLayerParallax', 'layer-1', {enabled: true, x: .5});
  assert.equal(report.owned, before + 10); assert.equal(owned.report._styk.layers.byId['layer-1'], layer); assert.equal(layer.unknownMetadata, unknown);
  for (const [id, track] of Object.entries(tracks)) assert.equal(owned.report._styk.layerSheet.tracks[id], track, 'metadata retains original track children');
  for (const name of ['renameLayer', 'setLayerColor', 'setLayerVisible', 'setLayerLocked', 'reorderLayers', 'moveLayerTo', 'setLayerParallax']) assert.equal(pair.originalCalls[name], 0);
  while (expected.canUndo) run('undo'); while (expected.canRedo) run('redo');
});

test('create layer applies original activation, focus inclusion, folders, return descriptor and event-time counters', () => {
  const pair = folderPair(folderDocument(), {setup(editor) {
    editor.createLayerFolder('Group', ['layer-1']); editor.setLayerFocus(['layer-1']);
  }}), {owned, run, report} = pair;
  const before = report.owned, active = owned.activeLayerId;
  const inactive = run('createLayer', 'Inactive', {activate: false, folderId: 'folder-1'}).result;
  assert.equal(owned.activeLayerId, active); assert.equal(inactive, owned.getLayer(inactive.id)); assert.equal(inactive.folderId, 'folder-1');
  assert.ok(owned.getLayerFocus().ids.has(inactive.id));
  const created = run('createLayer', 'Active').result;
  assert.equal(owned.activeLayerId, created.id); assert.equal(created, owned.getLayer(created.id)); assert.ok(owned.getLayerFocus().ids.has(created.id));
  assert.equal(report.owned, before + 2); assert.equal(pair.originalCalls.createLayer, 0);
  assert.equal(Object.hasOwn(pair.packets.filter(answer => answer.layerPublication?.label === 'Create layer')[0].layerPublication, 'activeId'), false);
  assert.equal(pair.packets.at(-1).layerPublication.activeId, created.id);
});

test('metadata history zero reuses original source pruning and retains false/no-op and error semantics', () => {
  const pair = folderPair(folderDocument(), {setup(editor) {
    editor.select(1, -1); editor.moveSelection(5, 5); editor.moveSelection(0, -7); editor.historyLimit = 0;
  }}), {expected, owned, run, report} = pair;
  const sources = Object.keys(owned.report._styk.frameSources), before = report.owned;
  run('setLayerColor', ['layer-1'], null); run('setLayerColor', ['layer-1'], null);
  run('renameLayer', 'layer-1', 'Layer 1'); run('setLayerVisible', 'layer-2', false); run('setLayerVisible', 'layer-2', false);
  run('createLayer', 'Zero', {activate: false});
  assert.equal(report.owned, before + 6); assert.equal(owned._undo.length, 0); assert.equal(expected._undo.length, 0);
  assert.ok(sources.some(id => !Object.hasOwn(owned.report._styk.frameSources, id)), 'the original metadata record pruned an actual history-only source');
  const count = report.owned; assert.ok(run('setLayerLocked', 'layer-1', 'bad').error); assert.equal(report.owned, count);
  assert.equal(pair.originalCalls.setLayerLocked, 1);
});

test('malformed metadata operands retire the advanced session before the original fallback', () => {
  for (const change of [
    answer => { delete answer.layerPublication; }, answer => { answer.layerPublication.label = 'Other'; },
    answer => { answer.layerPublication.updates[0].path = ['assignments', '1']; },
    answer => { answer.layerPublication.updates[0].path = ['byId', 'layer-2', 'name']; },
    answer => { answer.layerPublication.updates[0].remove = true; }, answer => { answer.layerPublication.updates[0].value = ''; },
    answer => { answer.layerPublication.updates.push(structuredClone(answer.layerPublication.updates[0])); },
    answer => { answer.layerPublication.activeId = 'layer-2'; }, answer => { answer.events[0].live = true; },
    answer => { answer.state.nextUid++; }, answer => { answer.sheet = {}; },
  ]) {
    const pair = folderPair(folderDocument()), before = json(pair.owned.report), adopted = pair.report.owned;
    pair.alter(change); pair.run('renameLayer', 'layer-1', 'New');
    assert.deepEqual(pair.releases, [before]); assert.equal(pair.originalCalls.renameLayer, 1); assert.equal(pair.report.owned, adopted); assert.equal(pair.report.failedOwned, 1);
  }
  for (const change of [
    answer => { delete answer.layerPublication.activeId; }, answer => { answer.layerPublication.activeId = 'layer-1'; },
    answer => { answer.layerPublication.updates.pop(); }, answer => { answer.layerPublication.updates[0].value.drawing = {version: 1}; },
    answer => { answer.result.id = 'layer-99'; },
  ]) {
    const pair = folderPair(folderDocument()), before = json(pair.owned.report); pair.alter(change); pair.run('createLayer', 'New');
    assert.deepEqual(pair.releases, [before]); assert.equal(pair.originalCalls.createLayer, 1); assert.equal(pair.report.failedOwned, 1);
  }
});

test('metadata publication failure preserves original partial effects and fresh-session recovery', async () => {
  const pair = folderPair(folderDocument()), {expected, owned, run, report} = pair;
  const emit = [expected, owned].map(editor => editor._emit), failure = new Error('Rejected metadata event');
  for (const [index, editor] of [expected, owned].entries()) editor._emit = function (kind, detail) {
    if (kind === 'edit' && detail.layerMetadata) throw failure; return emit[index].call(this, kind, detail);
  };
  assert.throws(() => expected.renameLayer('layer-1', 'Rejected'), error => error === failure);
  assert.throws(() => owned.renameLayer('layer-1', 'Rejected'), error => error === failure); pair.same();
  assert.equal(pair.releases.length, 1);
  for (const [index, editor] of [expected, owned].entries()) editor._emit = emit[index];
  await report.prepare(); const before = report.owned; run('renameLayer', 'layer-1', 'Recovered');
  assert.equal(report.owned, before + 1); assert.equal(pair.originalCalls.renameLayer, 0);
});

test('unrecorded drawing and unsupported metadata contexts keep the existing original route', () => {
  for (const [document, options, setup, excluded] of [
    [folderDocument(), {layerTimeline: false}, () => {}, editor => !editor.layerTimeline],
    [folderDocument(), {}, editor => { editor.select(1, -1); editor.beginDrag('Existing'); }, editor => editor._drag],
    [folderDocument(), {}, editor => editor.beginFrameDraft({mode: 'update'}), editor => editor._frameDraft],
    [JSON.parse(readFileSync(new URL('../engine/tests/edit/origin-pose-layout-sheet.document.json', import.meta.url), 'utf8')), {}, () => {}, editor => editingClass(editor.report) === 'unported'],
  ]) {
    const pair = folderPair(document, {...options, setup}); for (const editor of [pair.expected, pair.owned, pair.oracle]) assert.ok(excluded(editor));
    const before = pair.report.owned, requests = pair.requests.length; pair.run('renameLayer', 'layer-1', 'Original');
    assert.equal(pair.report.owned, before); assert.equal(pair.requests.length, requests); assert.equal(pair.originalCalls.renameLayer, 1);
  }
  const pair = folderPair(folderDocument()), before = pair.report.owned, requests = pair.requests.length;
  pair.run('createLayer', 'Drawing', {drawing: true});
  assert.equal(pair.report.owned, before); assert.equal(pair.requests.length, requests); assert.equal(pair.originalCalls.createLayer, 1);
});

for (const [name, args] of [['setLayerColor', [{}, 'violet']], ['createLayer', ['New', null]]]) {
  test(`unsupported ${name} operands preserve the original error before any Native call`, () => {
    for (const mode of ['classes', 'shadow']) {
      const pair = folderPair(folderDocument(), {mode}), {expected, owned, report} = pair;
      const before = json(expected.report), undo = json(expected._undo), redo = json(expected._redo);
      const view = state(expected), counters = topologyCounters(expected);
      let failure;
      try { expected[name](...args); } catch (error) { failure = error; }
      assert.ok(failure instanceof TypeError, 'the actual original public method rejects the operands');
      assert.throws(() => owned[name](...args), error => error.constructor === failure.constructor && error.message === failure.message);
      pair.same();
      assert.equal(json(expected.report), before); assert.equal(json(owned.report), before); assert.equal(json(pair.oracle.report), before);
      assert.equal(json(owned._undo), undo); assert.equal(json(owned._redo), redo);
      assert.deepEqual(state(owned), view); assert.deepEqual(topologyCounters(owned), counters);
      assert.equal(pair.originalCalls[name], 1, 'the original runs exactly once');
      assert.equal(report.calls, 0, 'neither ownership nor shadow sends a Native command');
      assert.equal(report.owned, 0); assert.equal(report.failedOwned, 0); assert.equal(report.skipped, 1);
      assert.equal(pair.requests.length, 0); assert.equal(pair.packets.length, 0);
      assert.deepEqual(pair.releases, [before], 'the unchanged live session retires before the original error');
      assert.equal(pair.observations[0].length, 0); assert.equal(pair.observations[1].length, 0);
    }
  });
}

function settingsPair(document, {mutate, historyLimit, mode = 'classes'} = {}) {
  const options = {demo: false, layerTimeline: true, propertyChannels: true};
  const expected = new Editor(options).load(structuredClone(document));
  const owned = new Editor(options).load(structuredClone(document));
  const oracle = new Editor(options).load(structuredClone(document));
  if (historyLimit !== undefined) for (const editor of [expected, owned, oracle]) editor.historyLimit = historyLimit;
  const methods = ['setFPS', 'setCanvasSize', 'updateProject'], originalCalls = {}, requests = [], events = [[], []];
  for (const method of methods) {
    const original = owned[method]; originalCalls[method] = 0;
    owned[method] = function (...args) { originalCalls[method]++; return original.apply(this, args); };
  }
  for (const [index, editor] of [expected, owned].entries()) editor.addEventListener('change', event => {
    events[index].push({detail: clean(event.detail), frame: json(editor.evaluatedFrame), state: state(editor)});
  });
  const session = sessionFor(oracle), call = session.call.bind(session);
  session.call = (name, text, viewText) => {
    const before = json(owned.report), view = JSON.parse(viewText), answer = JSON.parse(call(name, text, viewText));
    if (view.projectFieldsPublication === true) {
      requests.push(name);
      const event = answer.events?.[0];
      if (answer.status === 'ok' && answer.result === true && event?.projectFields) {
        answer.projectFields = {label: event.label, timing: event.timing,
          fields: event.projectFields.map(key => [key, oracle.report[key]])};
      }
      mutate?.(answer, name);
      assert.equal(json(owned.report), before, 'packet preparation has not mutated the receiving editor');
    }
    return JSON.stringify(answer);
  };
  const report = installEditShadow(owned, () => ({engine: {handle: {edit_session: () => session}},
    mirror: {framesCurrent: () => true, syncFrames: async () => {}}}), {mode});
  const run = (method, ...args) => {
    events[0].length = 0; events[1].length = 0;
    let expectedResult, actualResult, expectedError, actualError;
    try { expectedResult = expected[method](...structuredClone(args)); } catch (error) { expectedError = error.messageKey ?? error.message; }
    try { actualResult = owned[method](...structuredClone(args)); } catch (error) { actualError = error.messageKey ?? error.message; }
    assert.equal(actualError, expectedError, `${method}: original error`);
    assert.deepEqual(actualResult, expectedResult, `${method}: original scalar result`);
    assert.equal(json(owned.report), json(expected.report), `${method}: exact stored document`);
    assert.deepEqual(state(owned), state(expected), `${method}: exact state/counters`);
    assert.deepEqual(events[1], events[0], `${method}: event-time displayed frame and counters`);
    assert.equal(json(owned.evaluatedFrame), json(expected.evaluatedFrame), `${method}: final displayed frame`);
    assert.equal(json(owned._undo), json(expected._undo), `${method}: complete original undo records`);
    assert.equal(json(owned._redo), json(expected._redo), `${method}: complete original redo records`);
    return {result: actualResult, error: actualError};
  };
  return {expected, owned, oracle, report, run, requests, originalCalls};
}

for (const name of ['project-settings-sheet', 'project-settings-format4-sheet']) {
  test(`default settings adoption preserves every recorded result/event/history: ${name}`, () => {
    const folder = new URL('../engine/tests/edit/', import.meta.url);
    const scenario = JSON.parse(readFileSync(new URL(`${name}.scenario.json`, folder), 'utf8'));
    const document = JSON.parse(readFileSync(new URL(scenario.document, folder), 'utf8'));
    const pair = settingsPair(document), initial = json(pair.owned.report);
    for (const [method, ...args] of scenario.steps) {
      const before = pair.report.owned, original = pair.originalCalls[method];
      const {error} = pair.run(method, ...args);
      if (Object.hasOwn(pair.originalCalls, method)) {
        assert.equal(pair.originalCalls[method], original + Number(Boolean(error)), `${method}: only original validation error replay`);
        assert.equal(pair.report.owned, before + Number(!error), `${method}: successful response (including no-op) adopted`);
      }
    }
    while (pair.owned.canUndo) pair.run('undo');
    assert.equal(json(pair.owned.report), initial, 'full undo restores the complete original document');
    assert.equal(pair.report.mode, 'classes');
    assert.deepEqual(pair.report.mismatches, []);
    assert.equal(pair.report.rounded, 0);
  });
}

test('settings packets preserve null scalar writes, input field order, format4 delay and zero history', () => {
  const folder = new URL('../engine/tests/edit/', import.meta.url);
  const document = JSON.parse(readFileSync(new URL('format4.document.json', folder), 'utf8'));
  const pair = settingsPair(document, {historyLimit: 0});
  const metadata = pair.owned.report._styk, figure = pair.owned.report.figure_types[0];
  pair.run('updateProject', {canvas_height: 600, fps: 7, canvas_width: 720});
  assert.equal(pair.owned.report.frame_delay_ms, Math.round(1000 / 7));
  pair.run('updateProject', {canvas_width: null});
  assert.equal(pair.owned.report.canvas_width, null, 'original null-coalesced validation still stores the literal null');
  pair.run('setCanvasSize', 960, 540);
  const before = pair.report.owned;
  pair.run('updateProject', {});
  assert.equal(pair.report.owned, before + 1, 'empty patch is an adopted original no-op');
  assert.equal(pair.owned.report._styk, metadata, 'untouched metadata identity');
  assert.equal(pair.owned.report.figure_types[0], figure, 'untouched definition identity');
  assert.equal(pair.owned.canUndo, false); assert.equal(pair.owned.canRedo, false);
  assert.equal(pair.report.failedOwned, 0);
});

test('malformed or missing settings certificates fall back atomically to the original once', () => {
  const document = JSON.parse(readFileSync(new URL('../engine/tests/edit/startup.document.json', import.meta.url), 'utf8'));
  for (const mutate of [answer => { delete answer.projectFields; }, answer => { answer.projectFields.label = 'Other'; },
    answer => { answer.projectFields.timing = false; }, answer => { answer.projectFields.fields[0][1] = 32; },
    answer => { answer.projectFields.fields.push(['fps', 31]); }, answer => { answer.projectFields.fields.push(['unknown', 1]); },
    answer => { answer.events[0].live = true; }, answer => { answer.state.nextUid++; },
    answer => { answer.state.canRedo = true; }, answer => { answer.state.undoLabel = 'Other'; }]) {
    const pair = settingsPair(document, {mutate});
    pair.run('setFPS', 31);
    assert.equal(pair.report.owned, 0); assert.equal(pair.report.failedOwned, 1);
    assert.equal(pair.originalCalls.setFPS, 1); assert.equal(pair.requests.length, 1);
    assert.equal(pair.owned._undo.length, 1, 'no partial/double history mutation');
    assert.deepEqual(pair.report.mismatches, []);
  }
});


test('settings reject unknown JSON-omitted fields before adopting a no-op', () => {
  const document = JSON.parse(readFileSync(new URL('../engine/tests/edit/startup.document.json', import.meta.url), 'utf8'));
  for (const key of ['unknown', 'frame_delay_ms']) for (const value of [undefined, () => 1, Symbol('unknown setting')]) {
    const pair = settingsPair(document), initial = json(pair.owned.report), before = state(pair.owned);
    const patch = {[key]: value};
    assert.throws(() => pair.expected.updateProject(patch), error => error.messageKey === 'errors.editor.projectSettingIsNotEditableHere');
    assert.throws(() => pair.owned.updateProject(patch), error => error.messageKey === 'errors.editor.projectSettingIsNotEditableHere');
    assert.equal(json(pair.owned.report), initial); assert.deepEqual(state(pair.owned), before);
    assert.equal(pair.owned._undo.length, 0); assert.equal(pair.owned._redo.length, 0);
    assert.equal(pair.report.owned, 0); assert.equal(pair.report.failedOwned, 1);
    assert.equal(pair.originalCalls.updateProject, 1); assert.equal(pair.requests.length, 1);
    assert.deepEqual(pair.report.mismatches, []);
  }
  const pair = settingsPair(document), symbol = Symbol('opaque project value'), value = {unchanged: true};
  const patch = {fps: 31, [symbol]: value};
  assert.equal(pair.expected.updateProject(patch), true); assert.equal(pair.owned.updateProject(patch), true);
  assert.equal(pair.owned.report[symbol], value, 'original spread/assign preserves the own symbol identity');
  assert.equal(pair.expected.report[symbol], value); assert.equal(json(pair.owned.report), json(pair.expected.report));
  assert.deepEqual(state(pair.owned), state(pair.expected));
  assert.equal(json(pair.owned._undo), json(pair.expected._undo));
  assert.equal(pair.requests.length, 0, 'non-JSON input stays on the original route before dispatch');
  assert.equal(pair.report.owned, 0); assert.equal(pair.report.failedOwned, 0); assert.equal(pair.originalCalls.updateProject, 1);
});


// Only this opt-in mock projects the recorded rename's scalar certificate.
// Native Session controls independently prove its shallow COW and default wire.
function renamePair({mode = 'classes', historyLimit} = {}) {
  const reference = JSON.parse(readFileSync(new URL('../engine/tests/edit/movieclip-rename-sheet.reference.json', import.meta.url), 'utf8'));
  const pair = adoptionPair(JSON.parse(reference.steps[0].document), mode), {owned, oracle, session} = pair;
  if (historyLimit !== undefined) for (const editor of [pair.expected, owned, oracle]) editor.historyLimit = historyLimit;
  const requests = [], packets = [], call = session.call.bind(session); let alter = () => {};
  session.call = (name, args, viewText) => {
    const view = JSON.parse(viewText), figure = view.movieclipRenamePublication === true ? oracle.figure : null;
    const index = figure ? oracle.report.figure_types.indexOf(figure) : -1, beforeName = figure?.name;
    if (view.movieclipRenamePublication === true) { assert.equal(name, 'renameMovieclip'); requests.push({name, view}); }
    const answer = JSON.parse(call(name, args, viewText));
    if (figure && answer.status === 'ok') {
      answer.movieclipRename = {index, beforeName, name: oracle.report.figure_types[index].name};
      if (view.changesOnly) answer.result = null;
    }
    if (name === 'renameMovieclip') { alter(answer); packets.push(structuredClone(answer)); }
    return json(answer);
  };
  const run = (name, ...args) => {
    let result, actual, error;
    try { result = pair.expected[name](...args); } catch (caught) { error = caught.messageKey ?? caught.message; }
    if (error) assert.throws(() => owned[name](...args), caught => (caught.messageKey ?? caught.message) === error);
    else actual = owned[name](...args);
    pair.same(); assert.deepEqual(owned._undo, pair.expected._undo); assert.deepEqual(owned._redo, pair.expected._redo);
    assert.deepEqual(topologyCounters(owned), topologyCounters(pair.expected));
    if (!error) assert.equal(json(actual), json(result), 'original return value, including undefined no-op');
    return {result: actual, error};
  };
  return {...pair, reference, requests, packets, run, alter(callback) { alter = callback; }};
}

test('default Rename adopts recorded definition edits and no-op without changing event-time history', () => {
  const pair = renamePair(), {expected, owned, report, reference, run} = pair;
  const observations = [[], []];
  for (const [index, editor] of [expected, owned].entries()) editor.addEventListener('change', event => {
    observations[index].push({event: clean(event.detail), document: json(editor.report), state: state(editor), counters: topologyCounters(editor), frame: json(editor.evaluatedFrame)});
  });
  let tails = 0; const tail = owned._adoptMovieclipRename;
  owned._adoptMovieclipRename = function (...args) { tails++; return tail.apply(this, args); };
  assert.equal(report.mode, 'classes'); assert.equal(editingClass(owned.report), 'movieclips');
  for (const step of reference.steps.slice(1)) {
    const [method, ...args] = step.call, name = method.replace(/^@/, ''), adopted = report.owned, priorTails = tails;
    const figure = name === 'renameMovieclip' ? owned.figure : null, before = json(owned.report), entries = owned._undo.length;
    const {result, error} = run(name, ...args.map(decode));
    if (name === 'renameMovieclip' && !error) {
      assert.equal(report.owned, adopted + 1); assert.equal(tails, priorTails + 1, 'original definition publication tail is used');
      if (step.result === null) {
        assert.equal(result, undefined); assert.equal(owned.figure, figure); assert.equal(json(owned.report), before); assert.equal(owned._undo.length, entries);
      } else {
        assert.equal(result, owned.figure);
        for (const key of Object.keys(figure)) if (key !== 'name') assert.equal(result[key], figure[key], `shallow definition child ${key}`);
        assert.deepEqual(Object.keys(result), Object.keys(figure)); assert.equal(owned._undo.at(-1).kind, 'movieclip-definition');
      }
    }
  }
  assert.equal(tails, 3, 'two changed names and one no-op'); assert.equal(pair.requests.length, 5, 'selected clip requests include both original validation errors');
  assert.deepEqual(observations[1], observations[0], 'every listener sees the original report/view/revisions');
  while (expected.canUndo) run('undo'); assert.equal(owned.canUndo, false);
});

test('Rename keeps held shallow unknown children and zero-history no-op semantics', () => {
  const pair = renamePair({historyLimit: 0}), {owned, report, run} = pair;
  run('seek', 3); run('select', 4, -1);
  for (const editor of [pair.expected, owned, pair.oracle]) {
    editor.figure.renameUnknown = {nested: ['keep', null]};
    Object.defineProperty(editor.figure, '__proto__', {value: {keep: 'own data'}, enumerable: true, configurable: true, writable: true});
  }
  // Compact ownership carries no definition/document; held children are the
  // publication's only source, including unknown and own __proto__ data.
  pair.alter(answer => { if (answer.movieclipRename) assert.equal(answer.result, null); });
  const original = owned.figure, adopted = report.owned;
  const renamed = run('renameMovieclip', '  Zero history  ').result;
  assert.equal(report.owned, adopted + 1); assert.equal(renamed.renameUnknown, original.renameUnknown); assert.equal(renamed['__proto__'], original['__proto__']);
  assert.equal(renamed._styk, original._styk); assert.equal(renamed.segments, original.segments); assert.equal(owned._undo.length, 0);
  const reportNode = owned.report, serial = topologyCounters(owned), events = pair.packets.length;
  assert.equal(run('renameMovieclip', 'Zero history').result, undefined);
  assert.equal(owned.report, reportNode); assert.equal(owned.figure, renamed); assert.deepEqual(topologyCounters(owned), serial);
  assert.equal(pair.packets.length, events + 1); assert.deepEqual(pair.packets.at(-1).events, []); assert.equal(pair.packets.at(-1).result, null);
});

test('malformed Rename packets retire Native before the unchanged original publication', () => {
  for (const alter of [
    answer => { delete answer.movieclipRename; },
    answer => { answer.movieclipRename.index++; },
    answer => { answer.movieclipRename.beforeName = 'foreign held name'; },
    answer => { answer.movieclipRename.extra = true; },
    answer => { answer.events[0].live = true; },
    answer => { answer.events = [null]; },
    answer => { answer.state.nextUid++; },
    answer => { answer.state.canUndo = false; },
    answer => { answer.result = {name: 'large decoded definition'}; },
    answer => { answer.sheet = {}; },
  ]) {
    const pair = renamePair(), {expected, owned, report, run} = pair;
    run('seek', 3); run('select', 4, -1); pair.alter(alter);
    const before = json(owned.report), original = owned.figure, adopted = report.owned; let tails = 0, releases = 0;
    const tail = owned._adoptMovieclipRename;
    owned._adoptMovieclipRename = function (...args) { tails++; return tail.apply(this, args); };
    pair.session.free = () => { releases++; assert.equal(json(owned.report), before, 'no UI writes before Native retirement'); assert.equal(owned.figure, original); };
    run('renameMovieclip', 'Original fallback');
    assert.equal(tails, 0); assert.equal(report.owned, adopted); assert.equal(releases, 1); assert.equal(report.failedOwned, 1);
    assert.deepEqual(owned._undo.at(-1), expected._undo.at(-1));
  }
  for (const key of ['dirty', 'canRedo']) {
    const pair = renamePair(); pair.run('seek', 3); pair.run('select', 4, -1);
    const held = pair.owned.figure, before = json(pair.owned.report), adopted = pair.report.owned;
    pair.alter(answer => { answer.state[key] = !answer.state[key]; });
    assert.equal(pair.run('renameMovieclip', held.name).result, undefined);
    assert.equal(pair.report.owned, adopted); assert.equal(pair.report.failedOwned, 1);
    assert.equal(pair.owned.figure, held); assert.equal(json(pair.owned.report), before); assert.equal(pair.owned._undo.length, 0);
  }
  const shadow = renamePair({mode: 'shadow'}); shadow.run('seek', 3); shadow.run('select', 4, -1); shadow.run('renameMovieclip', 'Compared');
  assert.equal(shadow.requests.length, 0); assert.equal(shadow.report.owned, 0);
});
