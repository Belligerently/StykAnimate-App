/**
 * Shadow editing (docs/engine/editing.md): every editor call that the engine has
 * ported also runs in the engine, on the document the engine mirrors, and the two
 * results are compared: the returned value, the playhead and selection, the change
 * events, the gesture's draft and what a command published. The JavaScript editor
 * stays the one the app uses; nothing the engine answers is applied.
 *
 * With `?engineShadow=on` or `localStorage['stykanimate.engine.shadowEdit'] = 'on'`.
 * Results accumulate in `globalThis.__stykEditShadow`.
 *
 * Engine-owned editing: the engine runs the calls in OWNED in the editor's place while a
 * session lives, by default for documents of an editing class in OWNED_CLASSES (D15:
 * classes measured not slower than the editor), for every document with `?engineEdit=on`
 * (or `localStorage['stykanimate.engine.ownEdit'] = 'on'`), never with `off`. The editor
 * adopts the gesture's draft and what the call published (`adoptLayerSheet`), with its own
 * bookkeeping, events and history, and does not compute them. The other ported calls
 * (selection, seeking, starting and cancelling a gesture, undo and redo) run in both and
 * are compared as in shadow mode. A call the engine refuses runs in the editor alone.
 *
 * A session starts from the mirror once it holds the editor's current revision,
 * and lives while every change to the document goes through calls it shadows.
 * Anything else (an unported command, a load, an asynchronous edit) ends it.
 */
import {adoptLayerSheet} from '../editor-layer-sheet.mjs';
import {layerFocusOf} from '../editor-layer-focus.mjs';

/** Calls styk-edit runs (engine/crates/styk-edit/src/lib.rs, `dispatch`). */
const PORTED = ['select', 'selectMany', 'seek', 'selectSheetLayer', 'keepSelectionOnSeek', 'setActiveLayer', 'beginDrag', 'beginTranslationDrag', 'commitDrag', 'cancelDrag', 'moveSelection', 'rotateSelection',
  'scaleSelection', 'scaleSelected', 'mirrorSelection', 'rotateFigure', 'centerFigure', 'resetPose', 'poseJoint', 'rotateJoint', 'bendJoint',
  'sheetInsertKey', 'sheetDuplicateFrameAfter', 'sheetSetEasing', 'sheetClearKeys', 'sheetClearFrames', 'sheetInsertFrames', 'sheetRemoveFrames',
  'sheetSetDuration', 'sheetAppendFrame', 'joinTo', 'unjoinFigure', 'joinFigure', 'deleteSelection', 'deleteFigure', 'duplicateFigure', 'setConnectorPosition', 'setSpatialPose', 'setSpatialMotion', 'setSpatialIKTarget', 'resetSpatialPose', 'setPoseChoice', 'setNodeIK', 'rotateIKTarget', 'setSpatialFeetPinned',
  'renameLayer', 'setLayerColor', 'setLayerVisible', 'setLayerLocked', 'reorderLayers', 'moveLayerTo', 'createLayer', 'createLayerFolder', 'renameLayerFolder',
  'moveLayersToFolder', 'arrangeLayers', 'deleteLayerFolder', 'addFigure', 'sheetMoveFrames', 'updateObject', 'updateSelection', 'updateSegment', 'setCameraMotion', 'deleteCameraMotion',
  'addPaletteColor', 'addPaletteColors', 'removePaletteColor', 'movePaletteColor', 'sheetCopyFrames', 'sheetPasteFrames', 'sheetTransferFrames', 'copySelection', 'cutSelection', 'pasteSelection', 'duplicateSelection', 'importFigure', 'setLayerParallax', 'renameMovieclip', 'setMovieclipPlayback', 'setMovieclipDrawing', 'setFPS', 'setCanvasSize', 'updateProject', 'addPolygon', 'updatePolygon', 'removePolygon',
  'beginVfxClipEdit', 'commitVfxClipEdit', 'cancelVfxClipEdit', 'setVfxClip', 'trimVfxClipStart', 'deleteVfxClip', 'moveVfxClipOrder', 'reorderVfxGroupEffect',
  'setVfxClipParameter', 'setVfxClipParameters', 'toggleVfxClipKey', 'moveVfxClipKey', 'moveVfxClipKeys', 'deleteVfxClipKeys', 'setVfxClipEasing', 'undo', 'redo',
  // Property edits on a layer sheet (styk-edit/src/property_edit.rs).
  'addAnimationKeys', 'addAnimationKey', 'beginAnimationEdit', 'previewAnimationEdit', 'commitAnimationEdit', 'cancelAnimationEdit', 'setSceneColor', 'setSceneBackground',
  'setBackgroundColor', 'updateFigureStyle', 'setVfxParameter'];
/** Calls that return a frame, a figure record or a layer's view (with layer focus, which the
 * engine does not hold): a view, not compared. */
const VIEWS = new Set(['select', 'selectMany', 'seek', 'sheetAppendFrame', 'duplicateFigure', 'addFigure', 'importFigure', 'createLayer', 'createLayerFolder', 'sheetCopyFrames', 'endBatch', 'beginAnimationEdit']);
/** Calls the engine runs in the editor's place when it owns editing. */
const OWNED = new Set(['moveSelection', 'rotateSelection', 'scaleSelection', 'scaleSelected', 'mirrorSelection', 'rotateFigure', 'centerFigure', 'resetPose',
  'poseJoint', 'rotateJoint', 'bendJoint', 'commitDrag', 'sheetInsertKey', 'sheetDuplicateFrameAfter', 'sheetSetEasing', 'sheetClearKeys', 'sheetClearFrames',
  'sheetInsertFrames', 'sheetRemoveFrames', 'sheetSetDuration', 'sheetAppendFrame', 'joinTo', 'unjoinFigure', 'joinFigure', 'deleteSelection', 'deleteFigure', 'duplicateFigure',
  'setConnectorPosition', 'setSpatialPose', 'setSpatialMotion', 'setSpatialIKTarget', 'resetSpatialPose', 'setPoseChoice', 'setNodeIK', 'rotateIKTarget', 'setSpatialFeetPinned', 'addFigure', 'sheetMoveFrames', 'updateObject', 'updateSelection', 'sheetPasteFrames', 'pasteSelection', 'duplicateSelection', 'importFigure',
  // Property edits publish the sheet; a scrub's previews are compared, not adopted.
  'addAnimationKeys', 'addAnimationKey', 'commitAnimationEdit', 'setSceneColor', 'setSceneBackground', 'setBackgroundColor', 'updateFigureStyle', 'setVfxParameter']);

/** A switch set in the address (`?query=on|off`), else in local storage: 'on', 'off' or null. */
const choice = (query, key) => {
  try {
    const value = new URLSearchParams(globalThis.location?.search ?? '').get(query);
    if (value) return value === 'on' ? 'on' : 'off';
    const stored = globalThis.localStorage?.getItem(key);
    return stored === 'on' || stored === 'off' ? stored : null;
  } catch { return null; }
};

/**
 * How the engine takes part in editing: `own` (it runs every ported call), `shadow` (it
 * runs them too and is compared; the editor stays in charge), `off`, or `classes` (the
 * default: it owns the calls of documents whose editing class is in OWNED_CLASSES and stays
 * out of the others).
 */
export function engineEditMode() {
  const edit = choice('engineEdit', 'stykanimate.engine.ownEdit');
  if (edit === 'on') return 'own';
  if (choice('engineShadow', 'stykanimate.engine.shadowEdit') === 'on') return 'shadow';
  return edit === 'off' ? 'off' : 'classes';
}

/** Segments across a document's figure definitions from which it is a large one. */
const LARGE = 1000;

/**
 * A document's editing class (D15, docs/engine/editing.md): `unported` for documents the
 * engine does not edit at all (property channels on a native timeline, animation origins, a
 * captured movieclip's own document; a layer sheet's `legacyAnimation` is edited), `movieclips`
 * when a figure is a movieclip, `3d` when a figure
 * has 3D bones or a mesh, else `large figures` from LARGE segments across its figure
 * definitions and `figures` below.
 */
export function editingClass(report) {
  const figures = report?.figure_types ?? [], styk = report?._styk;
  if (styk?.animation?.channels?.length || styk?.movieclipSource || figures.some(figure => figure?._styk?.animationOrigin)) return 'unported';
  if (figures.some(figure => figure?._styk?.movieclip)) return 'movieclips';
  if (figures.some(figure => figure?.segments?.some(segment => segment?._stykRig?.spatial))) return '3d';
  return figures.reduce((sum, figure) => sum + (figure?.segments?.length ?? 0), 0) >= LARGE ? 'large figures' : 'figures';
}

/** The classes the engine edits by default: their calls cost no more in the engine than in
 * the editor, adoption included (measured 2026-10-03, docs/engine/editing.md). */
export const OWNED_CLASSES = new Set(['large figures']);

/** Event details as plain data, as engine/tools/edit-replay.mjs records them. */
const plain = (value, depth = 4) => {
  if (value === null || ['number', 'string', 'boolean'].includes(typeof value)) return value;
  if (value instanceof Set) return plain([...value], depth);
  if (typeof value !== 'object' || depth <= 0) return undefined;
  if (Array.isArray(value)) return value.map(item => plain(item, depth - 1)).filter(item => item !== undefined);
  return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, plain(item, depth - 1)]).filter(([, item]) => item !== undefined));
};
const withoutSample = (key, value) => key === '_movieclipSample' ? undefined : value;
/** What a property edit's preview shows (`_sheetPropertyPreview`), as the engine composes it:
 * figures and the frame's own fields, without runtime caches and clocks. */
const previewOf = frame => JSON.stringify(frame ? {objects: frame.objects, camera: frame.camera, background_id: frame.background_id, background_id_b: frame.background_id_b,
  background_mix: frame.background_mix, _stykBackgroundColor: frame._stykBackgroundColor, _stykVfx: frame._stykVfx} : null,
  (key, value) => key === '_movieclipSample' || key === 'runtime_segments' ? undefined : value);

/**
 * Whether two JSON texts say the same, numbers within a few units in the last place.
 * Browsers round `Math.sin`, `cos`, `atan2` and `hypot` differently from each other and
 * from the engine (docs/engine/decisions/0016-v8-exact-math.md), so a pose computed
 * through them may differ in its last digits; keys, order and everything else must be equal.
 */
function close(a, b) {
  if (a === b) return true;
  // With the engine's Math in the page (a verification harness), numbers must be equal.
  if (globalThis.__stykExactMath) return false;
  const near = (x, y) => {
    if (typeof x === 'number' && typeof y === 'number') return Math.abs(x - y) <= 1e-9 * Math.max(1, Math.abs(x), Math.abs(y));
    if (x === null || y === null || typeof x !== 'object' || typeof y !== 'object') return x === y;
    if (Array.isArray(x) !== Array.isArray(y)) return false;
    const keys = Object.keys(x), others = Object.keys(y);
    return keys.length === others.length && keys.every((key, index) => key === others[index] && near(x[key], y[key]));
  };
  try { return near(JSON.parse(a), JSON.parse(b)); } catch { return false; }
}

/** When two JSON texts differ in numbers only: how far apart the most distant pair is. */
function numbersApart(a, b) {
  let largest = 0, count = 0;
  const walk = (x, y) => {
    if (typeof x === 'number' && typeof y === 'number') { if (x !== y) { count++; largest = Math.max(largest, Math.abs(x - y)); } return true; }
    if (x === null || y === null || typeof x !== 'object' || typeof y !== 'object') return x === y;
    const keys = Object.keys(x), others = Object.keys(y);
    return keys.length === others.length && keys.every((key, i) => key === others[i] && walk(x[key], y[key]));
  };
  try { return walk(JSON.parse(a), JSON.parse(b)) ? `; ${count} numbers differ, by at most ${largest.toExponential(2)}` : ''; } catch { return ''; }
}

/**
 * @param {object} editor the app's Editor
 * @param {() => object|null} engineSide the stage's engine side (`renderer.engineSide`), once it exists
 */
export function installEditShadow(editor, engineSide, {mode = engineEditMode()} = {}) {
  const own = mode !== 'shadow';
  // `classes`: the document's class, read again when the document or its figures change.
  let classed = {report: null, revision: -1, owned: false};
  const ownedClass = () => {
    if (mode !== 'classes') return true;
    if (classed.report !== editor.report || classed.revision !== editor.definitionRevision) {
      classed = {report: editor.report, revision: editor.definitionRevision, owned: OWNED_CLASSES.has(editingClass(editor.report))};
    }
    return classed.owned;
  };
  const report = globalThis.__stykEditShadow = {own, mode, calls: 0, compared: 0, rounded: 0, skipped: 0, sessions: 0, ended: {}, unported: {}, mismatches: [], jsMs: 0, engineMs: 0, owned: 0, ownedMs: 0};
  let session = null, depth = 0, events = null;
  // A batch in progress (`movieclipBatch`): the session records it as one entry too, and a
  // session that ends inside it does not restart until it closes. `closing` collects the
  // batch's own change event, which the editor sends after the batch's last call.
  let batching = false, closing = null;
  // The timeline and figure clipboards the session copied (the editor's objects): a session
  // starts with none, and pastes only when the editor's clipboard is that one.
  let copied = null, copiedFigures = null;
  const end = why => {
    if (!session) return;
    session.free(); session = null; copied = null; copiedFigures = null;
    report.ended[why] = (report.ended[why] ?? 0) + 1;
  };
  const view = () => ({tick: editor.tick, frameIndex: editor.frameIndex, selectedUid: editor.selectedUid ?? null, selectedUids: [...(editor.selectedUids ?? [])],
    selectedJoint: editor.selectedJoint ?? -1, activeLayerId: editor.activeLayerId, autoKey: Boolean(editor.autoKey),
    sheetSelection: editor._sheetSelection ? {id: editor._sheetSelection.id, contents: Boolean(editor._sheetSelection.contents)} : null});
  const counters = () => ({layerRevision: editor.layerRevision, nextFrameIdentity: editor._nextFrameIdentity, nextUid: editor._nextUid});

  editor.addEventListener('change', event => {
    const detail = event.detail;
    if (['edit', 'history', 'document'].includes(detail?.kind)) changedInPlace(detail);
    if (depth > 0) { events?.push(plain(detail ?? null)); return; }
    if (closing) { closing.push(plain(detail ?? null)); return; }
    // The document changed through something the session did not run.
    if (['edit', 'history', 'document'].includes(detail?.kind)) end(`a ${detail.kind} outside the shadowed calls`);
    // Runtime state the engine does not hold (editing.md): layer focus, which locks layers
    // and counts as a layer revision, and a selected branch, which commands act on instead
    // of whole figures.
    else if (detail?.kind === 'layer-focus') end('layer focus');
    else if (detail?.kind === 'selection' && editor.branchSelected) end('a branch selection');
  });
  /** Whether the editor holds runtime state that changes what calls do and the engine lacks. */
  const runtimeState = () => Boolean(layerFocusOf(editor.report)?.lock || editor.branchSelected);

  /** Brings the engine's frames to the editor's revision (playback does it only when it prepares). */
  let syncing = null;
  // Records the editor changed in place, which the mirror cannot see by identity, as its change
  // events name them: a definition edit may change a figure in place (every figure is resent),
  // and one that changes its structure outside a sheet publication rewrites every pose of it
  // too (everything is). The next sync resends them, and no session starts before it has.
  let stale = null;
  const changedInPlace = detail => {
    const topology = detail.definition && detail.structural && !detail.layerSheet;
    // A sheet publication names the pose sources it adds; they are new records, not changed ones.
    const sources = detail.layerSheet ? [] : detail.sourceIds ?? [];
    if (!detail.definition && !detail.figureIndices?.length && !detail.frameIndices?.length && !sources.length) return;
    stale ??= {all: false, figures: false, figureIndices: [], frameIndices: [], sourceIds: []};
    if (detail.definition && !detail.figureIndices?.length) stale.figures = true;
    if (topology || detail.kind === 'document') stale.all = true;
    stale.figureIndices.push(...detail.figureIndices ?? []); stale.frameIndices.push(...detail.frameIndices ?? []); stale.sourceIds.push(...sources);
  };
  const prepare = () => {
    const side = engineSide();
    if (!side || side.failed) return Promise.resolve(false);
    if (!stale && side.mirror.framesCurrent(editor.report, editor.revision)) return Promise.resolve(true);
    if (syncing) return syncing;
    const target = editor.report, revision = editor.revision, dirty = stale;
    stale = null;
    const resend = !dirty ? {} : dirty.all
      ? {allFigures: true, frameIndices: target.keyframes.map((_, index) => index), sourceIds: Object.keys(target._styk?.frameSources ?? {})}
      : {allFigures: dirty.figures, figureIndices: dirty.figureIndices, frameIndices: dirty.frameIndices, sourceIds: dirty.sourceIds};
    syncing = side.mirror.syncFrames(target, revision, resend).then(() => side.mirror.framesCurrent(editor.report, editor.revision),
      () => { if (dirty) stale = {...dirty, ...(stale ? {all: dirty.all || stale.all, figures: dirty.figures || stale.figures,
        figureIndices: [...dirty.figureIndices, ...stale.figureIndices], frameIndices: [...dirty.frameIndices, ...stale.frameIndices], sourceIds: [...dirty.sourceIds, ...stale.sourceIds]} : {})}; return false; })
      .finally(() => { syncing = null; });
    return syncing;
  };
  report.prepare = prepare;

  // After the engine fails a call (it threw), editing stays with the editor for the rest of
  // the page: the call ran in the editor, and the engine is not asked again.
  let broken = false;
  const begin = () => {
    if (session) return session;
    if (broken || batching || !ownedClass() || runtimeState()) return null;
    const side = engineSide();
    if (!side || side.failed || editor._drag || editor._authoredPoseEdit || editor._animationEdit || editor._vfxClipEdit) return null;
    // Not there yet: this call is not shadowed, the next one may be.
    if (stale || syncing || !side.mirror.framesCurrent(editor.report, editor.revision)) { prepare(); return null; }
    const started = side.engine.handle.edit_session(JSON.stringify({...view(), ...counters(), propertyChannels: Boolean(editor.propertyChannels), layerTimeline: Boolean(editor.layerTimeline)}));
    if (!started) return null;
    if (started.revision() !== editor.revision) { started.free(); return null; }
    report.sessions++;
    return session = started;
  };

  const compare = (name, args, answer, result, thrown, captured, sheetBefore) => {
    let rounded = false;
    const problems = [], same = (label, ours, theirs) => {
      if (ours === theirs) return;
      if (typeof ours === 'string' && typeof theirs === 'string' && close(ours, theirs)) { rounded = true; return; }
      const a = String(theirs), b = String(ours);
      let at = 0; while (at < a.length && a[at] === b[at]) at++;
      const from = Math.max(0, at - 160);
      problems.push(`${label} (from character ${at}${numbersApart(a, b)}): engine …${a.slice(from, at + 160)}, editor …${b.slice(from, at + 160)}`);
    };
    if (answer.status === 'error' || thrown) same('error', thrown ? (thrown.messageKey ?? thrown.message) : 'none', answer.status === 'error' ? answer.error : 'none');
    else if (!VIEWS.has(name)) same('result', JSON.stringify(result ?? null), JSON.stringify(answer.result ?? null));
    const state = {...view(), ...counters()};
    for (const key of ['tick', 'frameIndex', 'selectedUid', 'selectedJoint', 'layerRevision', 'nextFrameIdentity', 'nextUid']) same(key, state[key], answer.state[key]);
    same('selectedUids', JSON.stringify(state.selectedUids), JSON.stringify(answer.state.selectedUids));
    same('events', JSON.stringify(captured), JSON.stringify(answer.events));
    const edit = editor._authoredPoseEdit, drag = editor._drag;
    const taken = edit && drag ? new Set(drag.before.map(object => object.unique_id)) : null;
    same('draft', JSON.stringify(taken ? edit.frame.objects.filter(object => taken.has(object.unique_id)) : null, withoutSample), JSON.stringify(answer.draft));
    if ('preview' in answer) same('preview', previewOf(editor._sheetPropertyPreview), previewOf(answer.preview));
    const published = captured.some(detail => detail?.layerSheet), styk = editor.report._styk;
    if (published !== ('sheet' in answer)) problems.push(`published: engine ${'sheet' in answer}, editor ${published}`);
    else if (published) {
      // The answer carries the tracks the call replaced: the editor must have left the others as they were.
      const kept = (answer.trackOrder ?? []).filter(id => !Object.hasOwn(answer.sheet.tracks, id));
      for (const id of kept) if (styk.layerSheet?.tracks?.[id] !== sheetBefore?.tracks?.[id]) problems.push(`track ${id}: the editor replaced it, the engine did not`);
      const sheet = {...answer.sheet, tracks: Object.fromEntries((answer.trackOrder ?? []).map(id => [id, Object.hasOwn(answer.sheet.tracks, id) ? answer.sheet.tracks[id] : styk.layerSheet?.tracks?.[id]]))};
      same('layer sheet', JSON.stringify(styk.layerSheet), JSON.stringify(sheet));
      same('layers', JSON.stringify(styk.layers), JSON.stringify(answer.layers));
      for (const [id, source] of Object.entries(answer.sources ?? {})) same(`pose source ${id}`, JSON.stringify(styk.frameSources?.[id] ?? null), JSON.stringify(source));
    }
    if (!problems.length) {
      report.compared++;
      (report.comparedByCall ??= {})[name] = (report.comparedByCall[name] ?? 0) + 1;
      // The same but for last digits: the session goes on with its own, and later calls
      // are compared within the same tolerance.
      if (rounded) report.rounded++;
      return;
    }
    if (report.mismatches.length < 50) report.mismatches.push({call: name, args: plain(args), problems,
      context: {tick: editor.tick, frameIndex: editor.frameIndex, draftObjects: edit?.frame.objects.length, keyframe: edit?.frame.keyframe_index, frameDraft: Boolean(editor.report._styk?.simpleFrameDraft)}});
    console.warn(`[engine shadow] ${name} differs:\n  ${problems.join('\n  ')}`);
    end('a difference');
  };

  const originals = new Map();
  /** What the editor does with an owned call's answer instead of computing it. */
  const adopt = (name, answer, selection) => {
    const {state} = answer, edit = editor._authoredPoseEdit, drag = editor._drag;
    // The frame a gesture the call began and ended itself drafted: the one on show.
    const shown = answer.retained && !editor._authoredPoseEdit ? editor.evaluatedFrame : null;
    // The gesture's draft: the engine's figures replace the editor's, and the editor's
    // snapshots take in what the engine's gesture took in.
    if (answer.drag && answer.draft) {
      const objects = edit.frame.objects, taken = new Set(drag.before.map(object => object.unique_id));
      for (const uid of answer.drag.before) if (!taken.has(uid)) {
        const object = objects.find(item => item.unique_id === uid);
        if (object) drag.before.push(structuredClone(object));
      }
      drag.before.sort((a, b) => a.object_slot - b.object_slot);
      if (answer.drag.translation) drag.scope = 'translation';
      for (const figure of answer.draft) {
        const slot = objects.findIndex(object => object.unique_id === figure.unique_id); if (slot < 0) continue;
        if (objects[slot]._movieclipSample !== undefined) figure._movieclipSample = objects[slot]._movieclipSample;
        objects[slot] = figure; edit.owned.add(slot);
      }
    }
    const engineSelection = state.selectedUid !== editor.selectedUid || state.selectedJoint !== editor.selectedJoint
      || JSON.stringify(state.selectedUids) !== JSON.stringify([...editor.selectedUids]);
    if (engineSelection) {
      editor._polygonSelection = null; editor._segmentSelection = null; editor._branchSelection = null;
      editor.selectedUids = new Set(state.selectedUids); editor._selectedUid = state.selectedUid; editor.selectedJoint = state.selectedJoint;
    }
    editor._nextUid = state.nextUid;
    // A property edit ends as `commitAnimationEdit` ends it, whatever it publishes.
    if (name === 'commitAnimationEdit') { editor._animationEdit = null; editor._sheetPropertyPreview = null; editor._invalidate(); }
    let published = false;
    for (const {kind, frameIndex, tick, ...detail} of answer.events) {
      if (kind === 'edit' && detail.live) {
        if (drag) { drag.changed = true; editor._invalidateObjects(detail.changedUids, null, {pose: true}); }
        else editor._invalidate();
        editor._emit('edit', detail);
      } else if (kind === 'edit' && detail.layerSheet && !published) {
        published = true;
        // As `_commitSheetPose`: the committed draft stays on show through publication.
        let preview = null;
        const drafted = edit && drag ? edit.frame : shown;
        if (answer.retained && drafted) {
          const samples = new Map(drafted.objects.map(object => [object.unique_id, object._movieclipSample]));
          for (const object of answer.retained) if (samples.get(object.unique_id) !== undefined) object._movieclipSample = samples.get(object.unique_id);
          // Geometry rebuilds: the commit may have moved joined figures' roots.
          preview = {frame: {...drafted, objects: answer.retained}, geometry: null, incremental: null};
        }
        if (!answer.drag) { editor._drag = null; editor._authoredPoseEdit = null; }
        editor._authoredCommitPreview = preview;
        try {
          adoptLayerSheet(editor, {sheet: answer.sheet, trackOrder: answer.trackOrder, layers: answer.layers, sources: answer.sources, nextFrame: state.nextFrameIdentity, definitions: answer.definitions},
            detail.label, {selection: drag?.beforeSelection ?? selection, retainGeometry: Boolean(preview)});
        } finally { editor._authoredCommitPreview = null; }
      } else if (kind === 'seek') originals.get('seek').call(editor, tick);
    }
    // A gesture that ended without publishing (nothing changed).
    if (!answer.drag && editor._drag && !published) { editor._drag = null; editor._authoredPoseEdit = null; editor._invalidate(); }
    if (name === 'duplicateFigure' || name === 'addFigure' || name === 'importFigure') return editor.object;
    if (name === 'pasteSelection' || name === 'duplicateSelection') return editor.selectedObjects;
    if (name === 'sheetAppendFrame') return editor.evaluatedFrame;
    return answer.result;
  };
  /** Why the editor cannot adopt every effect of this answer (then it runs the call itself), or null. */
  const unadoptable = answer => {
    const other = answer.events.find(({kind, live, layerSheet}) => !(kind === 'seek' || kind === 'edit' && (live || layerSheet)));
    if (other) return `a ${other.kind} event`;
    if (answer.events.filter(event => event.layerSheet).length > 1) return 'two publications';
    if (answer.draft && !(editor._authoredPoseEdit && editor._drag && answer.drag)) return `a gesture the editor did not begin (drag ${Boolean(editor._drag)}, draft ${Boolean(editor._authoredPoseEdit)}, engine ${JSON.stringify(answer.drag)})`;
    return null;
  };

  for (const name of PORTED) {
    const original = editor[name];
    if (typeof original === 'function') originals.set(name, original);
  }
  for (const name of PORTED) {
    const original = editor[name];
    if (typeof original !== 'function') continue;
    editor[name] = function shadowed(...args) {
      // A ported call made by another call belongs to it.
      if (depth > 0) return original.apply(this, args);
      let live = begin(), before = null, payload = null;
      if (live && (name === 'undo' && !live.can_undo() || name === 'redo' && !live.can_redo())) { end('history the session did not record'); live = null; }
      if (live && name === 'sheetPasteFrames' && (editor._sheetClipboard ?? null) !== copied) { end('a timeline clipboard the session did not copy'); live = null; }
      if (live && name === 'pasteSelection' && (editor._selectionClipboard ?? editor._figureClipboard ?? null) !== copiedFigures) { end('a figure clipboard the session did not copy'); live = null; }
      if (live) {
        // Trailing omitted arguments stay omitted; anything JSON cannot carry is not compared.
        const given = args.slice(); while (given.length && given.at(-1) === undefined) given.pop();
        if (given.some(value => value === undefined || typeof value === 'function' || value instanceof Set || name === 'undo' || name === 'redo')) payload = name === 'undo' || name === 'redo' ? '[]' : null;
        else try { payload = JSON.stringify(given); } catch { payload = null; }
        if (payload === null) { end('arguments that are not plain data'); live = null; }
        else before = JSON.stringify(view());
      }
      if (!live) report.skipped++;
      // The engine runs the call in the editor's place.
      if (live && own && OWNED.has(name)) {
        const selection = editor._selectionState(), called = performance.now();
        let answer, text = '', parsing = called;
        try { text = live.call(name, payload, before); parsing = performance.now(); answer = JSON.parse(text); }
        catch (error) { if (report.mismatches.length < 50) report.mismatches.push({call: name, problems: [`the engine failed: ${error?.message ?? error}`]}); session = null; answer = null; broken = true; report.broken = String(error?.message ?? error).slice(0, 200); }
        report.calls++;
        const why = answer?.status === 'ok' ? unadoptable(answer) : null;
        if (answer?.status === 'ok' && !why) {
          depth++;
          const adopting = performance.now();
          try { return adopt(name, answer, selection); }
          finally {
            depth--; report.owned++; report.ownedMs += performance.now() - called;
            const entry = (report.byCall ??= {})[name] ??= {calls: 0};
            entry.calls++; entry.engineMs = (entry.engineMs ?? 0) + adopting - called; entry.adoptMs = (entry.adoptMs ?? 0) + performance.now() - adopting;
            entry.callMs = (entry.callMs ?? 0) + parsing - called; entry.answerBytes = (entry.answerBytes ?? 0) + text.length;
          }
        }
        if (answer?.status === 'unported') { const key = `${name}: ${answer.reason}`; report.unported[key] = (report.unported[key] ?? 0) + 1; end('a call the engine has not ported'); }
        else if (answer?.status === 'error') {
          // The editor throws the error itself, with its message; both are unchanged.
          depth++;
          try { const result = original.apply(this, args); end('an error the editor did not raise'); return result; }
          catch (error) { if ((error.messageKey ?? error.message) !== answer.error) end('a different error'); throw error; }
          finally { depth--; }
        } else if (answer) end(`an answer the editor cannot adopt (${name}: ${why})`);
        live = null;
      }
      const sheetBefore = editor.report._styk?.layerSheet;
      depth++; events = [];
      const started = performance.now();
      let result, thrown;
      try { result = original.apply(this, args); } catch (error) { thrown = error; } finally { depth--; }
      const captured = events; events = null;
      report.jsMs += performance.now() - started;
      // What the calls the engine would own cost the editor (compared with `ownedMs`).
      if (OWNED.has(name)) {
        report.ownableMs = (report.ownableMs ?? 0) + performance.now() - started;
        const entry = (report.byCall ??= {})[name] ??= {calls: 0};
        entry.calls++; entry.editorMs = (entry.editorMs ?? 0) + performance.now() - started;
      }
      if (live && typeof result?.then === 'function') { end('an asynchronous call'); live = null; }
      if (live) {
        try {
          const called = performance.now(), answer = JSON.parse(live.call(name, payload, before));
          report.engineMs += performance.now() - called; report.calls++;
          if (answer.status === 'unported') { const key = `${name}: ${answer.reason}`; report.unported[key] = (report.unported[key] ?? 0) + 1; end('a call the engine has not ported'); }
          else compare(name, args, answer, result, thrown, captured, sheetBefore);
          if (name === 'sheetCopyFrames' && session && !thrown) copied = editor._sheetClipboard;
          if ((name === 'copySelection' || name === 'cutSelection') && session && !thrown) copiedFigures = editor._selectionClipboard ?? null;
        } catch (error) { if (report.mismatches.length < 50) report.mismatches.push({call: name, problems: [`the engine failed: ${error?.message ?? error}`]}); session = null; broken = true; report.broken = String(error?.message ?? error).slice(0, 200); }
      }
      if (thrown) throw thrown;
      return result;
    };
  }
  // Batches: the calls inside run as they would alone; the session opens and closes the same
  // batch, and its closing event and state are compared with the editor's.
  const batch = editor.movieclipBatch;
  if (typeof batch === 'function') editor.movieclipBatch = function batched(label, operation, options) {
    // Inside another batch or call, a batch only runs its operation.
    if (depth > 0 || editor._movieclipBatch || batching) return batch.call(this, label, operation, options);
    const live = begin();
    if (!live) return batch.call(this, label, operation, options);
    if (typeof label !== 'string' || options?.beforeSelection) { end('a batch the engine cannot open'); return batch.call(this, label, operation, options); }
    let answer;
    try { answer = JSON.parse(live.call('beginBatch', JSON.stringify([label]), JSON.stringify(view()))); }
    catch (error) { if (report.mismatches.length < 50) report.mismatches.push({call: 'beginBatch', problems: [`the engine failed: ${error?.message ?? error}`]}); session = null; broken = true; return batch.call(this, label, operation, options); }
    if (answer.status !== 'ok') { end('a batch the engine cannot open'); return batch.call(this, label, operation, options); }
    batching = true;
    const sheetBefore = editor.report._styk?.layerSheet;
    let result, closed = null;
    try {
      result = batch.call(this, label, () => { const value = operation(); closing = []; return value; }, options);
      closed = closing;
    } catch (error) { closing = null; batching = false; end('a batch that failed'); throw error; }
    finally { closing = null; batching = false; }
    if (session && closed) {
      try {
        const called = performance.now(), ended = JSON.parse(session.call('endBatch', '[]', JSON.stringify(view())));
        report.engineMs += performance.now() - called; report.calls++;
        if (ended.status !== 'ok') end('a batch the engine cannot close');
        else compare('endBatch', [], ended, result, null, closed, sheetBefore);
      } catch (error) { if (report.mismatches.length < 50) report.mismatches.push({call: 'endBatch', problems: [`the engine failed: ${error?.message ?? error}`]}); session = null; broken = true; }
    }
    return result;
  };
  return report;
}
