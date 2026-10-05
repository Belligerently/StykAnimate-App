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
 * classes that meet the accepted editing cost limit), for every document with `?engineEdit=on`
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
import {figureHandleTransform} from '../editor-figure-transform.mjs';
import {withDraws} from '../editor-draws.mjs';
import {isMovieclip} from '../movieclips.mjs';
import {validateLayerSheetChanges} from '../layer-sheet.mjs';
import {validateLayerState} from '../layers.mjs';
import {validateLayerFolders} from '../layer-folders.mjs';
import {captureEditBounds} from './engine-edit-bounds.mjs';

/** Topology edits of the selected figure's definition (styk-edit/src/topology.rs). */
const TOPOLOGY = ['addSegment', 'addSegments', 'deleteSegment', 'deleteSingleSegment', 'unsplitSegment', 'mirrorSegment', 'mirrorSegmentBranch', 'moveBuilderSegment',
  'splitBuilderSegments', 'splitSegment', 'splitSegmentBranch', 'duplicateSegment', 'duplicateSegmentBranch', 'addConnector', 'addWeightedJoint', 'replaceFigureDefinition', 'setOrigin'];
const SPLITS = new Set(['splitBuilderSegments', 'splitSegment', 'splitSegmentBranch']);
const FOLDERS = new Set(['createLayerFolder', 'renameLayerFolder', 'moveLayersToFolder', 'arrangeLayers', 'deleteLayerFolder']);
/** The figure-range commands: an `undefined` argument means what `null` does to them. */
const RANGE_CALLS = ['addFigureInFrames', 'duplicateInFrames', 'duplicateFiguresInFrames', 'deleteFiguresInFrames', 'updateFiguresInFrames', 'orderFiguresInFrames',
  'joinFiguresInFrames', 'detachFiguresInFrames', 'mirrorFiguresInFrames', 'scaleFiguresInFrames', 'rotateFiguresInFrames', 'centerFiguresInFrames', 'resetPoseInFrames', 'alignFiguresInFrames'];
/** Calls styk-edit runs (engine/crates/styk-edit/src/lib.rs, `dispatch`). */
const PORTED = ['select', 'selectMany', 'selectPolygon', 'toggleSegment', 'selectBranch', 'seek', 'selectSheetLayer', 'keepSelectionOnSeek', 'setActiveLayer', 'beginDrag', 'beginTranslationDrag', 'commitDrag', 'cancelDrag', 'moveSelection', 'rotateSelection',
  'beginHandleEditDrag', 'editHandlePosition', 'commitHandleEditDrag', 'cancelHandleEditDrag', 'beginFigureHandleDrag', 'transformFigureHandle', 'commitFigureHandleDrag', 'cancelFigureHandleDrag',
  'nudgeHandle', 'alignHandle', 'locateEditedHandle', 'segmentValues', 'beginSegmentValueEdit', 'setSegmentValues', 'copySegmentValues', 'pasteSegmentValues',
  'copyCamera', 'pasteCamera', 'setCameraValues', 'resetCamera', 'transformCamera', 'centerCamera', 'alignSelection', 'alignCameraToView', 'locateCamera', 'locateHandle', 'beginCameraDrag', 'updateCameraDrag', 'commitCameraDrag', 'cancelCameraDrag', 'applyCameraToFigures',
  // These draw identities or seeds (D18): the editor runs them and hands the engine what it drew.
  'addCameraMotion', 'copyCameraMotion', 'pasteCameraMotion', 'duplicateCameraMotion', 'randomizeCameraMotion', 'addVfx', 'duplicateVfx', 'pasteVfx',
  'applyVfxPreset', 'addVfxGroup', 'addVfxUserPreset', 'addVfxClip', 'copyVfxClip', 'pasteVfxClip', 'duplicateVfxClip', 'addVfxGroupEffect',
  'removeVfx', 'resetVfx', 'moveVfx', 'reorderVfx', 'setVfxScope',
  'scaleSelection', 'scaleSelected', 'mirrorSelection', 'rotateFigure', 'centerFigure', 'resetPose', 'poseJoint', 'rotateJoint', 'bendJoint',
  'sheetInsertKey', 'sheetDuplicateFrameAfter', 'sheetSetEasing', 'sheetClearKeys', 'sheetClearFrames', 'sheetInsertFrames', 'sheetRemoveFrames',
  'sheetSetDuration', 'sheetAppendFrame', 'joinTo', 'unjoinFigure', 'joinFigure', 'deleteSelection', 'deleteFigure', 'duplicateFigure', 'setConnectorPosition', 'setSpatialPose', 'setSpatialMotion', 'setSpatialIKTarget', 'resetSpatialPose', 'setPoseChoice', 'setNodeIK', 'rotateIKTarget', 'setSpatialFeetPinned',
  'renameLayer', 'setLayerColor', 'setLayerVisible', 'setLayerLocked', 'reorderLayers', 'moveLayerTo', 'createLayer', 'createLayerFolder', 'renameLayerFolder',
  'moveLayersToFolder', 'arrangeLayers', 'deleteLayerFolder', 'addFigure', 'sheetMoveFrames', 'updateObject', 'updateSelection', 'updateSegment', 'setCameraMotion', 'deleteCameraMotion',
  'addPaletteColor', 'addPaletteColors', 'removePaletteColor', 'movePaletteColor', 'sheetCopyFrames', 'sheetPasteFrames', 'sheetTransferFrames', 'copySelection', 'cutSelection', 'pasteSelection', 'duplicateSelection', 'importFigure', 'setLayerParallax', 'renameMovieclip', 'makeMovieclipUnique', 'setMovieclipPlayback', 'setMovieclipDrawing', 'restartMovieclip', 'removeMovieclipRestart', 'setFPS', 'setCanvasSize', 'updateProject', 'addPolygon', 'updatePolygon', 'removePolygon',
  'beginVfxClipEdit', 'commitVfxClipEdit', 'cancelVfxClipEdit', 'setVfxClip', 'trimVfxClipStart', 'deleteVfxClip', 'moveVfxClipOrder', 'reorderVfxGroupEffect',
  'setVfxClipParameter', 'setVfxClipParameters', 'toggleVfxClipKey', 'moveVfxClipKey', 'moveVfxClipKeys', 'deleteVfxClipKeys', 'setVfxClipEasing', 'undo', 'redo',
  // Property edits on a layer sheet (styk-edit/src/property_edit.rs).
  'addAnimationKeys', 'addAnimationKey', 'beginAnimationEdit', 'previewAnimationEdit', 'commitAnimationEdit', 'cancelAnimationEdit', 'setSceneColor', 'setSceneBackground',
  'setBackgroundColor', 'updateFigureStyle', 'setVfxParameter',
  // Figures changed across frames, figure order, branch flips (styk-edit/src/range.rs).
  ...RANGE_CALLS, 'bringToFront', 'sendToBack', 'moveSelectionLayer', 'orderSelection', 'moveSelectionInFrames', 'mirrorFigure',
  // Recolouring (styk-edit/src/recolor.rs).
  'editFigureColors', 'setFigureColor', 'replaceFigureColor', 'resetFigureColors',
  // The builder's segment styles (styk-edit/src/builder.rs).
  'updateBuilderSegmentStyle', 'updateSegmentStyle', 'updateSegmentBranchStyle', 'convertSegment', 'setBranchCaps', 'setBranchFill', 'setPolygonOpacity', 'moveBuilderOrder', 'setBuilderOrder',
  // Rig relationships (styk-edit/src/rig_behavior.rs).
  'setRigBehavior', 'enableSmartStretch', 'bindStretchParts', 'setAngleLock', 'setSecondaryAngleReference',
  // Rig edits by dragging (styk-edit/src/rig_drag.rs).
  'beginRigDrag', 'editRigJoint', 'commitRigDrag', 'cancelRigDrag',
  // The figure's topology (styk-edit/src/topology.rs).
  ...TOPOLOGY,
  // Motion-path edits on a layer sheet (styk-edit/src/motion_path.rs).
  'beginMotionPathEdit', 'updateMotionPathEdit', 'commitMotionPathEdit', 'cancelMotionPathEdit', 'setMotionPathFalloff'];
/** Calls that return a frame, a figure record or a layer's view (with layer focus, which the
 * engine does not hold), or clipboard text (numbers in text cannot be compared within the
 * browsers' last-digit differences, D16): a view, not compared. */
const VIEWS = new Set(['select', 'selectMany', 'seek', 'sheetAppendFrame', 'duplicateFigure', 'addFigure', 'importFigure', 'createLayer', 'createLayerFolder', 'sheetCopyFrames', 'endBatch', 'beginAnimationEdit', 'copySegmentValues']);
/** Calls the engine runs in the editor's place when it owns editing. */
const OWNED = new Set(['moveSelection', 'rotateSelection', 'scaleSelection', 'scaleSelected', 'mirrorSelection', 'rotateFigure', 'centerFigure', 'resetPose',
  'poseJoint', 'rotateJoint', 'bendJoint', 'commitDrag', 'editHandlePosition', 'commitHandleEditDrag', 'transformFigureHandle', 'commitFigureHandleDrag', 'nudgeHandle', 'alignHandle', 'locateEditedHandle', 'setSegmentValues', 'pasteSegmentValues', 'pasteCamera', 'setCameraValues', 'resetCamera', 'transformCamera', 'alignCameraToView', 'locateCamera', 'locateHandle', 'commitCameraDrag', 'applyCameraToFigures', 'sheetInsertKey', 'sheetDuplicateFrameAfter', 'sheetSetEasing', 'sheetClearKeys', 'sheetClearFrames',
  'sheetInsertFrames', 'sheetRemoveFrames', 'sheetSetDuration', 'sheetAppendFrame', 'joinTo', 'unjoinFigure', 'joinFigure', 'deleteSelection', 'deleteFigure', 'duplicateFigure',
  'setConnectorPosition', 'setSpatialPose', 'setSpatialMotion', 'setSpatialIKTarget', 'resetSpatialPose', 'setPoseChoice', 'setNodeIK', 'rotateIKTarget', 'setSpatialFeetPinned', 'addFigure', 'sheetMoveFrames', 'updateObject', 'updateSelection', 'sheetPasteFrames', 'pasteSelection', 'duplicateSelection', 'importFigure',
  // Property edits publish the sheet; a scrub's previews are compared, not adopted.
  'addAnimationKeys', 'addAnimationKey', 'commitAnimationEdit', 'setSceneColor', 'setSceneBackground', 'setBackgroundColor', 'updateFigureStyle', 'setVfxParameter',
  ...RANGE_CALLS, 'bringToFront', 'sendToBack', 'moveSelectionLayer', 'orderSelection', 'moveSelectionInFrames', 'mirrorFigure',
  'editFigureColors', 'setFigureColor', 'replaceFigureColor', 'resetFigureColors', 'removeVfx', 'resetVfx', 'moveVfx', 'reorderVfx',
  // Style and draw-order edits recoloured or keyed on the figure on show publish the sheet; definition edits run in the editor.
  'updateBuilderSegmentStyle', 'setPolygonOpacity', 'moveBuilderOrder', 'setBuilderOrder',
  ...SPLITS, 'makeMovieclipUnique', ...FOLDERS,
  // A motion-path gesture's pointer moves run in both (the app reads the editor's session for
  // its live trail); what its commit publishes is adopted.
  'commitMotionPathEdit']);
const RANGE = new Set([...RANGE_CALLS, 'bringToFront', 'sendToBack', 'moveSelectionLayer', 'orderSelection', 'moveSelectionInFrames']);
/** Conditional metadata comes from the editor's canonical selection owner. */
const selectionMetadata = state => Object.fromEntries(['polygonSelection', 'segmentIds', 'branch'].filter(key => Object.hasOwn(state, key)).map(key => [key, state[key]]));
/** A frame range as the app passes it on a layer sheet (`rangeFrames()`: an array carrying the
 * timeline's chosen ticks as `sheetTicks`), which JSON would drop: `{"@array", sheetTicks}`. */
const frameRanges = (key, value) => Array.isArray(value) && Object.hasOwn(value, 'sheetTicks') ? {'@array': [...value], sheetTicks: value.sheetTicks} : value;

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
 * engine does not fully edit (property channels on a native timeline, animation origins, a
 * captured movieclip's own document), `channels` for documents whose layer sheet keeps saved
 * property channels (`legacyAnimation`: edited, their cost not yet measured on large
 * documents), `movieclips` when a figure is a movieclip, `3d` when a figure
 * has 3D bones or a mesh, else `large figures` from LARGE segments across its figure
 * definitions and `figures` below.
 */
export function editingClass(report) {
  const figures = report?.figure_types ?? [], styk = report?._styk;
  if (styk?.animation?.channels?.length || styk?.movieclipSource || figures.some(figure => figure?._styk?.animationOrigin)) return 'unported';
  if (styk?.layerSheet?.legacyAnimation?.channels?.length) return 'channels';
  if (figures.some(figure => figure?._styk?.movieclip)) return 'movieclips';
  if (figures.some(figure => figure?.segments?.some(segment => segment?._stykRig?.spatial))) return '3d';
  return figures.reduce((sum, figure) => sum + (figure?.segments?.length ?? 0), 0) >= LARGE ? 'large figures' : 'figures';
}

/** The classes the engine edits by default: their measured cost meets the class limit,
 * adoption included (measured 2026-10-03, for `3d` 2026-10-04, and for
 * `figures` 2026-10-05: docs/engine/editing.md). */
export const OWNED_CLASSES = new Set(['large figures', '3d', 'figures', 'movieclips', 'channels']);

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
  const report = globalThis.__stykEditShadow = {own, mode, calls: 0, compared: 0, rounded: 0, skipped: 0, sessions: 0, ended: {}, unported: {}, mismatches: [], jsMs: 0, engineMs: 0, ownableMs: 0, owned: 0, ownedMs: 0, failedOwned: 0, failedOwnedMs: 0};
  let session = null, depth = 0, events = null;
  // A batch in progress (`movieclipBatch`): the session records it as one entry too, and a
  // session that ends inside it does not restart until it closes. `closing` collects the
  // batch's own change event, which the editor sends after the batch's last call.
  let batching = false, closing = null;
  // The timeline, figure and segment-value clipboards the session copied (the editor's own): a
  // session starts with none, and pastes only when the editor's clipboard is that one.
  let copied = null, copiedFigures = null, copiedValues = null, copiedCamera = null, copiedMotion = null, copiedVfxClip = null;
  // `?engineEditCapture=on`: the session's starting document and state and every call it is
  // sent are kept; the first sessions to meet an unported call, a difference or another error
  // are saved in `report.captures`, for `styk edit-session` to replay natively.
  const capturing = choice('engineEditCapture', 'stykanimate.engine.editCapture') === 'on';
  let log = null;
  const keep = key => {
    const captures = report.captures ??= [];
    if (log && captures.length < 8 && !captures.some(capture => capture.key === key)) captures.push({key, document: log.document, state: log.state, calls: log.calls.slice()});
  };
  const call = (live, name, args, viewText) => { log?.calls.push({name, args, view: viewText}); return live.call(name, args, viewText); };
  const end = why => {
    if (!session) return;
    log = null;
    session.free(); session = null; copied = null; copiedFigures = null; copiedValues = null; copiedCamera = null; copiedMotion = null; copiedVfxClip = null;
    report.ended[why] = (report.ended[why] ?? 0) + 1;
  };
  const view = () => ({tick: editor.tick, frameIndex: editor.frameIndex, selectedUid: editor.selectedUid ?? null, selectedUids: [...(editor.selectedUids ?? [])],
    selectedJoint: editor.selectedJoint ?? -1, activeLayerId: editor.activeLayerId, autoKey: Boolean(editor.autoKey),
    sheetSelection: editor._sheetSelection ? {id: editor._sheetSelection.id, contents: Boolean(editor._sheetSelection.contents)} : null,
    ...selectionMetadata(editor._selectionState())});
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
    else if (detail?.kind === 'selection' && editor.selectedSegmentIds?.size > 1) end('several selected segments');
    else if (detail?.kind === 'selection' && editor.selectedPolygonIndex >= 0) end('a polygon selection');
  });
  /** Whether the editor holds runtime state that changes what calls do and the engine lacks:
   * layer focus, a selected branch, several selected segments of a figure (a branch flip turns
   * them all over). */
  const runtimeState = () => Boolean(layerFocusOf(editor.report)?.lock || editor.branchSelected || editor.selectedSegmentIds?.size > 1 || editor.selectedPolygonIndex >= 0);
  // Legacy Pivot poses carry `runtime_segments`, which the engine's copy leaves out; the range
  // operations read them (lengths and bends materialized before a transform, and geometry), so
  // documents that have them keep those calls in the editor. Checked once per keyframe list.
  let legacy = {keyframes: null, found: false};
  const legacyRuntime = () => {
    const keyframes = editor.report?.keyframes;
    if (legacy.keyframes !== keyframes) legacy = {keyframes, found: Boolean(keyframes?.some(frame => frame?.objects?.some(object => object?.runtime_segments)))};
    return legacy.found;
  };

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
    const state = JSON.stringify({...view(), ...counters(), propertyChannels: Boolean(editor.propertyChannels), layerTimeline: Boolean(editor.layerTimeline)});
    const started = side.engine.handle.edit_session(state);
    if (!started) return null;
    if (started.revision() !== editor.revision) { started.free(); return null; }
    if (capturing) log = {document: JSON.stringify(editor.report), state, calls: []};
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
    same('selection metadata', JSON.stringify(selectionMetadata(state)), JSON.stringify(selectionMetadata(answer.state)));
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
    keep(`${name}: a difference`);
    end('a difference');
  };

  const originals = new Map();
  // Use the same retained-record reconstruction for commits and self-contained moves.
  const retainedFrame = (answer, drafted) => {
    const byUid = new Map(drafted.objects.map(object => [object.unique_id, object]));
    for (const object of answer.retained) if (byUid.get(object.unique_id)?._movieclipSample !== undefined) object._movieclipSample = byUid.get(object.unique_id)._movieclipSample;
    let objects = answer.retained;
    if (answer.retainedChanges) {
      for (const object of answer.retained) byUid.set(object.unique_id, object);
      objects = (answer.retainedOrder ?? drafted.objects.map(object => object.unique_id)).map(uid => byUid.get(uid));
    }
    let frame = {...drafted, ...answer.retainedFields, objects};
    for (const key of answer.retainedRemoved ?? []) delete frame[key];
    if (answer.retainedFieldOrder) {
      const fields = new Set(answer.retainedFieldOrder);
      // Engine records omit the JavaScript frame's runtime effect/camera clocks.
      frame = {...Object.fromEntries(answer.retainedFieldOrder.map(key => [key, frame[key]])),
        ...Object.fromEntries(Object.entries(frame).filter(([key]) => !fields.has(key)))};
    }
    return frame;
  };
  const replacePoseObject = (object, figure) => {
    const keys = Object.keys(figure), previous = Object.keys(object);
    if (keys.length !== previous.length || keys.some((key, index) => key !== previous[index])) {
      for (const key of previous) delete object[key];
    }
    for (const key of keys) {
      if (key === '__proto__') Object.defineProperty(object, key, {value: figure[key], enumerable: true, configurable: true, writable: true});
      else object[key] = figure[key];
    }
  };
  // Split publishes an existing definition and its stored poses, not a sheet.
  // Keep native timelines, source documents, origins and live drafts on their
  // original route; the native topology owner retains its other refusals.
  const splitFigure = () => {
    const styk = editor.report._styk;
    if (!editor.layerTimeline || !styk?.layerSheet || styk.simpleFrameDraft || styk.movieclipSource
      || editor._drag || editor._authoredPoseEdit || editor._frameDraft || editor._animationEdit
      || editor._sheetPropertyPreview || editor._vfxClipEdit || editingClass(editor.report) === 'unported') return null;
    let figure;
    // Eligibility must not replace the original command's validation error
    // with a failure while reading its currently shown selection.
    try { figure = editor.figure; } catch { return null; }
    return figure && !isMovieclip(figure) && !figure._styk?.animationOrigin ? figure : null;
  };
  const uniqueFigure = () => {
    const styk = editor.report._styk;
    if (!editor.layerTimeline || !styk?.layerSheet || styk.simpleFrameDraft || styk.movieclipSource
      || editor._drag || editor._authoredPoseEdit || editor._frameDraft || editor._animationEdit
      || editor._sheetPropertyPreview || editor._vfxClipEdit || editor._movieclipBatch || batching
      || editingClass(editor.report) === 'unported') return null;
    try { const figure = editor.figure; return isMovieclip(figure) ? figure : null; } catch { return null; }
  };
  const folderReady = () => {
    const styk = editor.report._styk;
    return editor.layerTimeline && styk?.layerSheet && !styk.simpleFrameDraft && !styk.movieclipSource
      && !editor._drag && !editor._authoredPoseEdit && !editor._frameDraft && !editor._animationEdit
      && !editor._sheetPropertyPreview && !editor._vfxClipEdit && !editor._movieclipBatch && !batching
      && editingClass(editor.report) !== 'unported';
  };
  // The native layer owner certifies the accepted operands, including null-valued
  // removals. Apply them through the original history/publication owner, never a
  // sheet publication or a reconstruction from the surviving undo entry.
  const prepareFolder = (name, answer, selection) => {
    const reject = why => ({why: `a layer folder packet ${why}`});
    const record = value => value && typeof value === 'object' && !Array.isArray(value);
    const labels = {createLayerFolder: 'Create layer folder', renameLayerFolder: 'Rename layer folder',
      moveLayersToFolder: 'Move layers to folder', arrangeLayers: 'Move layers', deleteLayerFolder: 'Ungroup layer folder'};
    const label = labels[name], structural = name !== 'renameLayerFolder', {state, events, layerPublication: packet} = answer;
    if (!record(state) || !Array.isArray(events) || events.some(event => !record(event))
      || answer.drag || answer.draft || answer.preview || answer.retained || answer.definitions?.length
      || ['sheet', 'layers', 'sources', 'topology', 'movieclipUnique'].some(key => Object.hasOwn(answer, key))) return reject('with an unrelated publication');
    if (state.tick !== editor.tick || state.frameIndex !== editor.frameIndex
      || state.selectedUid !== (editor.selectedUid ?? null) || state.selectedJoint !== (editor.selectedJoint ?? -1)
      || JSON.stringify(state.selectedUids) !== JSON.stringify(selection.uids)
      || JSON.stringify(selectionMetadata(state)) !== JSON.stringify(selectionMetadata(selection))
      || state.nextUid !== editor._nextUid || state.nextFrameIdentity !== editor._nextFrameIdentity) return reject('with a different view');
    if (answer.result === false) {
      if (name === 'createLayerFolder' || packet !== undefined || events.length || state.layerRevision !== editor.layerRevision
        || ['canUndo', 'canRedo', 'undoLabel', 'redoLabel', 'dirty'].some(key => state[key] !== editor[key])) return reject('with a changed no-op');
      return {label, structural, updates: []};
    }
    if (!record(packet) || Object.keys(packet).length !== 3 || packet.label !== label || packet.structural !== structural
      || !Array.isArray(packet.updates) || !packet.updates.length || events.length !== 1) return reject('with incomplete operands');
    const event = events[0];
    if (event.kind !== 'edit' || event.label !== label || event.layers !== true || event.layerMetadata !== true
      || event.structural !== structural || event.animation !== false || event.timing !== false
      || event.live || event.layerSheet || event.definition || event.tick !== editor.tick || event.frameIndex !== editor.frameIndex
      || !Number.isSafeInteger(event.layerRevision) || event.layerRevision <= editor.layerRevision
      || state.layerRevision !== event.layerRevision) return reject('outside the original layer event');
    const layers = editor.report._styk.layers, candidate = {...layers, byId: {...layers.byId}}, updates = [], paths = new Set();
    if (layers.folders) candidate.folders = {...layers.folders, byId: {...layers.folders.byId}};
    for (const update of packet.updates) {
      if (!record(update) || !Array.isArray(update.path) || !update.path.length || typeof update.remove !== 'boolean'
        || Object.keys(update).some(key => !['path', 'remove', 'value'].includes(key))
        || !update.remove && !Object.hasOwn(update, 'value')) return reject('with invalid operands');
      const path = update.path, key = JSON.stringify(path);
      if (paths.has(key)) return reject('with repeated operands');
      paths.add(key);
      const order = path.length === 1 && path[0] === 'order' && name !== 'renameLayerFolder' && name !== 'deleteLayerFolder';
      const member = path.length === 3 && path[0] === 'byId' && Object.hasOwn(layers.byId, path[1]) && path[2] === 'folderId'
        && name !== 'renameLayerFolder';
      const folders = name === 'createLayerFolder' && (path.length === 1 && path[0] === 'folders' && !layers.folders
        || path.length === 3 && path[0] === 'folders' && path[1] === 'byId' && /^folder-[1-9]\d*$/.test(path[2]) && !Object.hasOwn(layers.folders?.byId ?? {}, path[2])
        || path.length === 2 && path[0] === 'folders' && path[1] === 'nextId' && layers.folders);
      const rename = name === 'renameLayerFolder' && path.length === 4 && path[0] === 'folders' && path[1] === 'byId'
        && Object.hasOwn(layers.folders?.byId ?? {}, path[2]) && path[3] === 'name';
      const remove = name === 'deleteLayerFolder' && path.length === 3 && path[0] === 'folders' && path[1] === 'byId'
        && Object.hasOwn(layers.folders?.byId ?? {}, path[2]);
      if (![order, member, folders, rename, remove].some(Boolean) || update.remove && !member && !remove
        || remove && !update.remove || member && !update.remove && typeof update.value !== 'string') return reject('outside folder metadata');
      if (name === 'createLayerFolder' && update.remove
        || name === 'deleteLayerFolder' && (!update.remove || Object.hasOwn(update, 'value'))
        || member && ['moveLayersToFolder', 'arrangeLayers'].includes(name) && update.remove && (!Object.hasOwn(update, 'value') || update.value !== null)
        || rename && (update.remove || typeof update.value !== 'string' || update.value !== update.value.trim())) return reject('with different removal or name operands');
      if (order && (update.remove || !Array.isArray(update.value) || update.value.length !== layers.order.length
        || new Set(update.value).size !== update.value.length || update.value.some(id => !Object.hasOwn(layers.byId, id)))) return reject('with an invalid layer order');
      // Copy only metadata on the received leaf path. The existing validator
      // checks folder membership/grouping; it never reads retained pose tracks.
      let object = candidate;
      if (member) object = candidate.byId[path[1]] = {...candidate.byId[path[1]]};
      else if (rename) object = candidate.folders.byId[path[2]] = {...candidate.folders.byId[path[2]]};
      else if (path.length === 3) object = candidate.folders?.byId;
      else if (path.length === 2) object = candidate.folders;
      if (!record(object)) return reject('with a missing parent');
      if (update.remove) delete object[path.at(-1)]; else object[path.at(-1)] = update.value;
      updates.push([path, update.value, update.remove]);
    }
    try { validateLayerFolders(candidate); } catch { return reject('with invalid folders'); }
    if (name === 'createLayerFolder') {
      const result = answer.result;
      if (!record(result) || Object.keys(result).join(',') !== 'id,kind,name' || result.kind !== 'folder'
        || result.id !== `folder-${layers.folders?.nextId ?? 1}` || result.name !== candidate.folders?.byId[result.id]?.name
        || candidate.folders.nextId !== (layers.folders?.nextId ?? 1) + 1) return reject('with a different created folder');
    } else if (answer.result !== true) return reject('with a different result');
    return {label, structural, updates};
  };
  const prepareUnique = (answer, original, selection) => {
    const reject = why => ({why: `a movieclip unique packet ${why}`});
    const record = value => value && typeof value === 'object' && !Array.isArray(value);
    const {movieclipUnique: marker, state, events, definitions} = answer;
    if (!record(marker) || Object.keys(marker).length !== 1 || marker.definitionIndex !== editor.report.figure_types.length
      || !Array.isArray(definitions) || definitions.length !== 1 || !record(definitions[0])
      || !record(state) || !Array.isArray(events) || events.length !== 5 || events.some(event => !record(event))) return reject('with an incomplete response');
    const added = definitions[0], [definition, publication, seek, selected, closing] = events;
    if (!Number.isInteger(added.figure_type_id) || added.figure_type_id < 0 || added.figure_type_id > 0xffffffff
      || editor.report.figure_types.some(figure => figure.figure_type_id === added.figure_type_id)
      || added.name !== `${original.name} copy` || answer.drag || answer.draft
      || events.some(event => event.live)
      || definition.kind !== 'edit' || definition.definition !== true || definition.layerSheet
      || definition.structural !== true || definition.timing !== true || definition.label !== 'Create movieclip'
      || publication.kind !== 'edit' || publication.layerSheet !== true || publication.definition
      || publication.structural !== true || publication.timing !== true || publication.label !== 'Make movieclip unique'
      || seek.kind !== 'seek' || selected.kind !== 'selection'
      || closing.kind !== 'edit' || closing.layerSheet || closing.definition
      || closing.structural !== true || closing.timing !== true || closing.label !== 'Make movieclip unique'
      || definition.tick !== editor.tick || publication.tick !== editor.tick
      || definition.frameIndex !== editor.frameIndex || publication.frameIndex !== editor.frameIndex
      || !Number.isInteger(seek.tick) || seek.tick !== Math.round(editor.tick) || seek.tick !== state.tick || selected.tick !== seek.tick || closing.tick !== seek.tick
      || seek.frameIndex !== state.frameIndex || selected.frameIndex !== state.frameIndex || closing.frameIndex !== state.frameIndex)
      return reject('outside the original publication sequence');
    if (state.selectedUid !== editor._nextUid || state.nextUid !== editor._nextUid + 1 || state.selectedJoint !== -1
      || !Array.isArray(state.selectedUids) || state.selectedUids.length !== 1 || state.selectedUids[0] !== state.selectedUid
      || !Number.isSafeInteger(state.nextFrameIdentity) || state.nextFrameIdentity < editor._nextFrameIdentity
      || !Number.isSafeInteger(publication.layerRevision) || publication.layerRevision < editor.layerRevision
      || !Number.isSafeInteger(state.layerRevision) || state.layerRevision < publication.layerRevision
      || Object.keys(selectionMetadata(state)).length)
      return reject('with a different final view');
    if (!record(answer.sheet) || !record(answer.sheet.tracks) || !record(answer.sources)
      || !Array.isArray(publication.sourceIds) || publication.sourceIds.length !== Object.keys(answer.sources).length
      || publication.sourceIds.some((id, index) => typeof id !== 'string' || id !== Object.keys(answer.sources)[index]
        || Object.hasOwn(editor.report._styk.frameSources ?? {}, id) || !record(answer.sources[id]) || !Array.isArray(answer.sources[id].objects)))
      return reject('missing a captured source');
    const before = editor.report._styk.layerSheet;
    let sheet = answer.changesOnly ? {...before, ...answer.sheet} : {...answer.sheet};
    if (answer.changesOnly) {
      if (answer.sheetRemoved !== undefined && (!Array.isArray(answer.sheetRemoved) || answer.sheetRemoved.some(key => typeof key !== 'string'))) return reject('with invalid sheet removals');
      for (const key of answer.sheetRemoved ?? []) delete sheet[key];
      if (answer.sheetOrder !== undefined) {
        if (!Array.isArray(answer.sheetOrder) || answer.sheetOrder.length !== Object.keys(sheet).length
          || new Set(answer.sheetOrder).size !== answer.sheetOrder.length || answer.sheetOrder.some(key => !Object.hasOwn(sheet, key))) return reject('with an incomplete sheet order');
        sheet = Object.fromEntries(answer.sheetOrder.map(key => [key, sheet[key]]));
      }
    }
    const trackOrder = answer.trackOrder ?? Object.keys(before.tracks);
    if (!Array.isArray(trackOrder) || new Set(trackOrder).size !== trackOrder.length
      || trackOrder.some(id => typeof id !== 'string' || !Object.hasOwn(answer.sheet.tracks, id) && !Object.hasOwn(before.tracks, id))
      || Object.keys(answer.sheet.tracks).some(id => !trackOrder.includes(id))) return reject('with an incomplete track order');
    const layers = Object.hasOwn(answer, 'layers') ? answer.layers : editor.report._styk.layers;
    // Native certifies the definition's unchanged COW children and key order.
    // Reuse them rather than decoding a second nested movieclip document.
    const figure = {...original, figure_type_id: added.figure_type_id, name: added.name};
    const complete = {...sheet, tracks: Object.fromEntries(trackOrder.map(id => [id,
      Object.hasOwn(answer.sheet.tracks, id) ? answer.sheet.tracks[id] : before.tracks[id]]))};
    const candidate = {...editor.report, figure_types: [...editor.report.figure_types, figure], _styk: {...editor.report._styk,
      layerSheet: complete, layers, frameSources: {...editor.report._styk.frameSources, ...answer.sources}}};
    // Validate with the existing owners before the first definition write.
    try { validateLayerState(layers); validateLayerSheetChanges(candidate, editor.report); } catch { return reject('with an invalid sheet publication'); }
    return {figure, publication: {sheet, trackOrder, layers, sources: answer.sources, nextFrame: state.nextFrameIdentity},
      tick: seek.tick, uid: state.selectedUid, selection};
  };
  const prepareSplit = (answer, figure, selection) => {
    const packet = answer.topology, state = answer.state;
    const reject = why => ({why: `a topology packet ${why}`});
    const record = value => value && typeof value === 'object' && !Array.isArray(value);
    if (!Array.isArray(answer.events) || !record(state)) return reject('with an incomplete response');
    const event = answer.events[0];
    if (answer.result !== true || !record(packet) || answer.events.length !== 1 || event?.kind !== 'edit'
      || event.structural !== true || event.definition !== true || event.live || event.layerSheet
      || typeof event.label !== 'string' || event.tick !== editor.tick || event.frameIndex !== editor.frameIndex
      || answer.drag || answer.draft || answer.sheet || answer.definitions || answer.retained) return reject('outside a single split');
    if (!Number.isInteger(packet.index) || editor.report.figure_types[packet.index] !== figure
      || !record(packet.figure) || packet.figure.figure_type_id !== figure.figure_type_id
      || !Array.isArray(packet.figure.segments) || !packet.figure.segments.length || !Array.isArray(packet.frames)) return reject('with a foreign definition');
    if (state.tick !== editor.tick || state.frameIndex !== editor.frameIndex || state.selectedUid !== selection.uid
      || !Array.isArray(state.selectedUids) || state.selectedUids.length !== selection.uids.length
      || state.selectedUids.some((uid, index) => uid !== selection.uids[index])
      || !Number.isInteger(state.selectedJoint) || state.selectedJoint < 0 || state.selectedJoint >= packet.figure.segments.length
      || state.nextUid !== editor._nextUid || state.nextFrameIdentity !== editor._nextFrameIdentity
      || state.layerRevision !== editor.layerRevision || Object.keys(selectionMetadata(state)).length) return reject('with a different view');
    for (const key of ['animation', 'sheetAnimation']) if (Object.hasOwn(packet, key)) {
      const field = packet[key];
      if (!record(field) || typeof field.exists !== 'boolean' || field.exists !== Object.hasOwn(field, 'value')) return reject('with an incomplete animation field');
    }
    const before = editor._topologySnapshot(figure.figure_type_id), changes = [];
    const placeKey = entry => Object.hasOwn(entry, 'sourceId') ? `source:${entry.sourceId}` : `keyframe:${entry.index}`;
    const stored = new Map(before.frames.map((entry, index) => [placeKey(entry), {index,
      captured: new Map(entry.objects.map((object, index) => [object.unique_id, index])),
      targets: new Map(editor._sourceFrame(entry).objects.map(object => [object.unique_id, object]))}]));
    let lastFrame = -1;
    for (const entry of packet.frames) {
      if (!record(entry) || !Number.isInteger(entry.index) || !Array.isArray(entry.objects) || !entry.objects.length
        || Object.keys(entry).some(key => !['index', 'sourceId', 'objects'].includes(key))
        || Object.hasOwn(entry, 'sourceId') && (entry.index !== -1 || typeof entry.sourceId !== 'string')) return reject('with an invalid stored frame');
      const frame = stored.get(placeKey(entry));
      if (!frame || frame.index <= lastFrame) return reject('with a foreign or repeated stored frame');
      lastFrame = frame.index;
      let lastObject = -1;
      for (const object of entry.objects) {
        const at = record(object) && Number.isInteger(object.unique_id) ? frame.captured.get(object.unique_id) : undefined;
        if (at === undefined || at <= lastObject) return reject('with a foreign or repeated pose');
        lastObject = at;
        const target = frame.targets.get(object.unique_id);
        if (!target || object.figure_type_id !== target.figure_type_id || object.object_slot !== target.object_slot) return reject('with a different pose identity');
        if (object.figure_type_id === figure.figure_type_id && (!Array.isArray(object.angles) || object.angles.length !== packet.figure.segments.length
          || object.len_flag && (!Array.isArray(object.lengths) || object.lengths.length !== packet.figure.segments.length)
          || object.bend_flag && (!Array.isArray(object.bends) || object.bends.length !== packet.figure.segments.length))) return reject('with an incomplete split pose');
        changes.push({target, object});
      }
    }
    // Keyframes and current sources cannot disappear in a split. Only an
    // unreferenced generated source can be absent after zero-history pruning;
    // use the same current-reference owner as _record's source collector.
    const changed = new Set(changes.map(change => change.target));
    const currentSources = editor.historyLimit === 0 ? editor._currentFrameSourceIds() : null;
    for (const entry of before.frames) {
      const prunable = entry.sourceId && currentSources && editor._generatedFrameSources.has(entry.sourceId) && !currentSources.has(entry.sourceId);
      if (prunable) continue;
      for (const object of entry.objects)
        if (object.figure_type_id === figure.figure_type_id && !changed.has(stored.get(placeKey(entry)).targets.get(object.unique_id)))
          return reject('missing a split stored pose');
    }
    return {packet, figure, before, changes, selection, label: event.label};
  };
  const adoptSplit = ({packet, figure, before, changes, selection, label}, state) => {
    const beforeAnimation = editor.report._styk.animation, beforeSheet = editor.report._styk.layerSheet;
    try {
      replacePoseObject(figure, packet.figure);
      for (const {target, object} of changes) replacePoseObject(target, object);
      if (packet.animation) {
        if (packet.animation.exists) editor.report._styk.animation = packet.animation.value;
        else delete editor.report._styk.animation;
      }
      if (packet.sheetAnimation) {
        const sheet = {...beforeSheet};
        if (packet.sheetAnimation.exists) sheet.legacyAnimation = packet.sheetAnimation.value;
        else delete sheet.legacyAnimation;
        editor.report._styk.layerSheet = sheet;
      }
      editor.selectedJoint = state.selectedJoint;
    } catch (error) {
      // Match _topologyEdit's rollback before its successful history tail.
      try {
        editor.report.figure_types[packet.index] = before.figure;
        if (beforeAnimation === undefined) delete editor.report._styk.animation;
        else editor.report._styk.animation = beforeAnimation;
        editor.report._styk.layerSheet = beforeSheet;
        for (const entry of before.frames) editor._replaceObjects(entry.index, entry.objects, entry.sourceId, entry.draft, entry.sourceDepth);
        editor._restoreSelection(selection); editor._invalidate();
      } finally { end('a topology packet application failed'); }
      throw error;
    }
    try {
      return editor._finishTopologyEdit(label, {figure, index: packet.index, typeId: figure.figure_type_id,
        before, beforeSelection: selection, beforeAnimation});
    } catch (error) {
      // The original tail may already have recorded history. Keep those same
      // partial effects, but never continue with the advanced Native session.
      end('a topology publication failed'); throw error;
    }
  };
  /** What the editor does with an owned call's answer instead of computing it. */
  const adopt = (name, answer, selection, args, topology, unique, folder, pasteClipboard, pasteIncrement) => {
    if (folder) {
      try {
        const result = editor._layerEdit(folder.label, folder.updates, {structural: folder.structural});
        return name === 'createLayerFolder' ? answer.result : result;
      } catch (error) { end('a layer folder publication failed'); throw error; }
    }
    if (unique) {
      try { return editor._adoptMovieclipUnique(unique); }
      catch (error) { end('a movieclip unique publication failed'); throw error; }
    }
    if (topology) return adoptSplit(topology, answer.state);
    // A camera drag the engine committed: the editor's ends, and its preview with it.
    if ((name === 'commitCameraDrag' || name === 'commitDrag') && editor._drag?.camera) { editor._drag = null; editor._sheetPropertyPreview = null; editor._invalidate(); }
    const {state} = answer;
    let edit = editor._authoredPoseEdit, drag = editor._drag;
    // The frame a gesture the call began and ended itself drafted: the one on show.
    const shown = answer.retained && !editor._authoredPoseEdit ? editor.evaluatedFrame : null;
    // An isolated one-shot move has the same frame at its live event and commit:
    // no captured join is normalized, and no origin or solver changes it afterwards.
    // Begin the existing COW owner only after a successful, adoptable answer.
    let movedDraft = false;
    const [liveEvent, publication] = answer.events;
    if (name === 'moveSelection' && shown && !edit && !drag && !answer.drag && !answer.draft
      && editor.report._styk?.layerSheet && !editor.report._styk.simpleFrameDraft && !editor.report._styk.movieclipSource
      && !editor._sheetPropertyPreview && !editor._animationEdit && !editor._vfxClipEdit
      && Number.isInteger(editor.tick) && state.tick === editor.tick && state.frameIndex === editor.frameIndex
      && state.selectedUid === editor.selectedUid && state.selectedJoint === editor.selectedJoint
      && state.selectedUids.length === editor.selectedUids.size && state.selectedUids.every((uid, index) => uid === selection.uids[index])
      && !editor._polygonSelection && !editor._segmentSelection && !editor._branchSelection
      && !Object.keys(selectionMetadata(state)).length && !editor.animationChannels.length
      && answer.events.length === 2 && liveEvent.kind === 'edit' && liveEvent.live && !liveEvent.layerSheet
      && liveEvent.tick === editor.tick && liveEvent.frameIndex === editor.frameIndex && !liveEvent.structural && !liveEvent.definition
      && publication.kind === 'edit' && publication.layerSheet && !publication.live && !publication.definition
      && publication.tick === editor.tick && publication.frameIndex === editor.frameIndex
      && !answer.definitions?.length && !Object.hasOwn(answer, 'layers')
      && !editor.report.figure_types.some(figure => figure._styk?.animationOrigin || figure.segments?.some(segment => segment._stykRig))
      && !shown.objects.some(object => object._stykIK || object._stykSpatial || object.runtime_segments)) {
      const slots = editor.resolveTranslationSelection(editor.selectedUids, {frame: shown}).affectedSlots;
      if (slots.length && slots.every(slot => !shown.objects[slot].join
          && !isMovieclip(editor.report.figure_types.find(figure => figure.figure_type_id === shown.objects[slot].figure_type_id)))
        && liveEvent.changedUids?.length === slots.length && slots.every((slot, index) => liveEvent.changedUids[index] === shown.objects[slot].unique_id)) {
        const frame = retainedFrame(answer, shown);
        if (frame.objects.length === shown.objects.length && frame.objects.every((object, slot) => object?.unique_id === shown.objects[slot].unique_id)
          && editor._beginAuthoredPoseDrag(publication.label, 'translation')) {
          edit = editor._authoredPoseEdit; drag = editor._drag;
          // A first compact response may carry the whole retained frame. Only the
          // captured records changed; leave untouched COW nodes and samples alone.
          try {
            for (const slot of slots) replacePoseObject(edit.frame.objects[slot], frame.objects[slot]);
            frame.objects = edit.frame.objects; edit.frame = frame; editor._evaluated = frame;
            movedDraft = true;
          } catch (error) { editor.cancelAuthoredPoseDrag(); throw error; }
        }
      }
    }
    // The gesture's draft: the engine's figures replace the editor's, and the editor's
    // snapshots take in what the engine's gesture took in.
    if (answer.drag && answer.draft) {
      const objects = edit.frame.objects;
      // Ordinary live poses keep their snapshots in frame order. When the
      // engine names those exact snapshots, there is nothing to add or sort.
      let ordered = (name === 'poseJoint' || name === 'moveSelection') && drag.authored && !edit.transaction
        && Array.isArray(drag.before) && Array.isArray(answer.drag.before) && drag.before.length === answer.drag.before.length;
      for (let index = 0; ordered && index < drag.before.length; index++) {
        const object = drag.before[index];
        if (!object || Array.isArray(object) || !Number.isInteger(object.unique_id) || object.unique_id !== answer.drag.before[index]
          || !Number.isInteger(object.object_slot) || object.object_slot < 0 || objects[object.object_slot]?.unique_id !== object.unique_id
          || index > 0 && drag.before[index - 1].object_slot >= object.object_slot) ordered = false;
      }
      if (!ordered) {
        const taken = new Set(drag.before.map(object => object.unique_id));
        for (const uid of answer.drag.before) if (!taken.has(uid)) {
          const object = objects.find(item => item.unique_id === uid);
          if (object) drag.before.push(structuredClone(object));
        }
        drag.before.sort((a, b) => a.object_slot - b.object_slot);
      }
      if (answer.drag.translation) drag.scope = 'translation';
      // Back at its start, a handle gesture restores what it moved (restoring the unmoved changes nothing).
      for (const state of drag.handleEdit?.states ?? []) state.moved = true;
      for (const figure of answer.draft) {
        const slot = objects[figure.object_slot]?.unique_id === figure.unique_id ? figure.object_slot : objects.findIndex(object => object.unique_id === figure.unique_id); if (slot < 0) continue;
        if (objects[slot]._movieclipSample !== undefined) figure._movieclipSample = objects[slot]._movieclipSample;
        // Pose and move already own these COW records. Keep their identity, as
        // the editor's own mutation does, so incremental geometry can retain
        // its pose plan. Newly included slots still isolate the source first.
        if ((name === 'poseJoint' || name === 'moveSelection') && edit.owned.has(slot)) {
          replacePoseObject(objects[slot], figure);
        } else objects[slot] = figure;
        edit.owned.add(slot);
      }
    }
    // A handle transform accumulates the turns it crossed: the editor's own state keeps up.
    if (name === 'transformFigureHandle' && answer.drag && drag?.figureHandle) figureHandleTransform(drag.figureHandle.transform, {x: args[0], y: args[1]}, {...drag.figureHandle, ...args[2]});
    let engineSelection;
    if ((name === 'poseJoint' || name === 'moveSelection')
      && !editor._polygonSelection && !editor._segmentSelection && !editor._branchSelection
      && !Object.hasOwn(state, 'polygonSelection') && !Object.hasOwn(state, 'segmentIds') && !Object.hasOwn(state, 'branch')) {
      // Ordinary live poses already supplied their selection in the view. Check
      // the answer directly instead of building it again and serializing both
      // selections; the Set's order is part of the editor's selection contract.
      engineSelection = state.selectedUid !== editor.selectedUid || state.selectedJoint !== editor.selectedJoint
        || state.selectedUids.length !== editor.selectedUids.size;
      if (!engineSelection) {
        let index = 0;
        for (const uid of editor.selectedUids) if (uid !== state.selectedUids[index++]) { engineSelection = true; break; }
      }
    } else {
      engineSelection = state.selectedUid !== editor.selectedUid || state.selectedJoint !== editor.selectedJoint
        || JSON.stringify(state.selectedUids) !== JSON.stringify([...editor.selectedUids])
        || JSON.stringify(selectionMetadata(state)) !== JSON.stringify(selectionMetadata(editor._selectionState()));
    }
    if (engineSelection) {
      const restored = editor._selectionState(), metadata = selectionMetadata(state);
      if (Object.keys(metadata).length || Object.keys(selectionMetadata(restored)).length) {
        for (const key of ['polygonSelection', 'segmentIds', 'branch']) delete restored[key];
        editor._restoreSelection({...restored, uid: state.selectedUid, uids: state.selectedUids, joint: state.selectedJoint, ...metadata});
      } else {
        // Publication may introduce these IDs. Preserve the existing bounded
        // selection exchange until its sources arrive; restoring against the
        // old frame would discard a newly duplicated or inserted figure.
        editor._polygonSelection = null; editor._segmentSelection = null; editor._branchSelection = null;
        editor.selectedUids = new Set(state.selectedUids); editor._selectedUid = state.selectedUid; editor.selectedJoint = state.selectedJoint;
      }
    }
    editor._nextUid = state.nextUid;
    // A property edit ends as `commitAnimationEdit` ends it, whatever it publishes.
    if (name === 'commitAnimationEdit') { editor._animationEdit = null; editor._sheetPropertyPreview = null; editor._invalidate(); }
    let published = false;
    for (const {kind, frameIndex, tick, ...detail} of answer.events) {
      // A motion-path gesture that ended without changing anything: the editor's ends with it.
      if (kind === 'edit' && detail.motionPath && !detail.live && !detail.layerSheet) {
        if (editor._drag?.motionPath) editor._drag = null;
        editor._emit('edit', detail);
      } else if (kind === 'edit' && detail.live) {
        try {
          if (drag) {
            drag.changed = true;
            if (name === 'poseJoint' || name === 'moveSelection') editor._invalidatePoseObjects(detail.changedUids, {translation: name === 'moveSelection'});
            else editor._invalidateObjects(detail.changedUids, null, {pose: true});
          }
          else editor._invalidate();
          editor._emit('edit', detail);
        } catch (error) { if (movedDraft) editor.cancelAuthoredPoseDrag(); throw error; }
      } else if (kind === 'edit' && detail.layerSheet && !published) {
        published = true;
        // As `_commitSheetPose`: the committed draft stays on show through publication.
        let preview = null;
        const drafted = edit && drag ? edit.frame : shown;
        if (answer.retained && drafted) {
          const order = answer.retainedFieldOrder;
          const fields = order && Object.keys(drafted).filter(key => !['_cameraMotionTick', '_vfxTimelineTick', '_vfxTimelineScene'].includes(key));
          // An ended ordinary gesture already has its final displayed pose. An
          // empty compact delta preserves it exactly, as `_commitSheetPose`
          // does, including ready or still-pending incremental geometry.
          // Actual replacements (notably corrected joined roots) keep the
          // reconstruction below; absent retained data is not an empty delta.
          const keepDraft = (name === 'commitDrag' || name === 'commitHandleEditDrag' || name === 'commitFigureHandleDrag')
            && edit && drag?.authored && !drag.motionPath && !edit.transaction && edit.tick === editor.tick && !answer.drag
            && answer.retainedChanges === true && Array.isArray(answer.retained) && answer.retained.length === 0
            && !answer.retainedOrder && !Object.keys(answer.retainedFields ?? {}).length && !answer.retainedRemoved?.length
            && !detail.definition && !answer.definitions?.length
            && (!order || order.length === fields.length && order.every((key, index) => key === fields[index]));
          if (keepDraft || movedDraft) preview = {frame: drafted, geometry: editor._geometry, incremental: editor._incremental};
          else {
            // Actual replacements may normalize joined roots; rebuild their geometry.
            preview = {frame: retainedFrame(answer, drafted), geometry: null, incremental: null};
          }
        }
        if (!answer.drag) { editor._drag = null; editor._authoredPoseEdit = null; }
        editor._authoredCommitPreview = preview;
        try {
          const beforeSheet = editor.report._styk.layerSheet;
          let sheet = answer.sheet;
          if (answer.changesOnly) {
            sheet = {...beforeSheet, ...sheet};
            for (const key of answer.sheetRemoved ?? []) delete sheet[key];
            if (answer.sheetOrder) sheet = Object.fromEntries(answer.sheetOrder.map(key => [key, sheet[key]]));
          }
          adoptLayerSheet(editor, {sheet, trackOrder: answer.trackOrder ?? Object.keys(beforeSheet.tracks),
            layers: Object.hasOwn(answer, 'layers') ? answer.layers : editor.report._styk.layers, sources: answer.sources, nextFrame: state.nextFrameIdentity, definitions: answer.definitions},
            detail.label, {selection: drag?.beforeSelection ?? selection, retainGeometry: Boolean(preview) || name === 'pasteSelection' || name === 'duplicateSelection'});
        } finally { editor._authoredCommitPreview = null; }
      } else if (kind === 'seek') originals.get('seek').call(editor, tick);
    }
    // A gesture that ended without publishing (nothing changed).
    if (!answer.drag && editor._drag && !published) { editor._drag = null; editor._authoredPoseEdit = null; editor._invalidate(); }
    if (name === 'duplicateFigure' || name === 'addFigure' || name === 'importFigure') return editor.object;
    if (name === 'pasteSelection' || name === 'duplicateSelection') {
      // The original owner increments its captured clipboard after publication.
      // Listeners may replace it or change its count; keep that same local target.
      if (!pasteClipboard) return [];
      if (pasteIncrement === 1) pasteClipboard.pastes++;
      return editor.selectedObjects;
    }
    if (name === 'sheetAppendFrame') return editor.evaluatedFrame;
    return answer.result;
  };
  /** Why the editor cannot adopt every effect of this answer (then it runs the call itself), or null. */
  const unadoptable = answer => {
    const other = answer.events.find(({kind, live, layerSheet, motionPath}) => !(kind === 'seek' || kind === 'edit' && (live || layerSheet || motionPath)));
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
      let live = begin(), before = null, payload = null, measured = null, fallbackStarted = null;
      const topologyFigure = live && own && SPLITS.has(name) ? splitFigure() : null;
      const movieclipFigure = live && own && name === 'makeMovieclipUnique' ? uniqueFigure() : null;
      const folder = live && own && FOLDERS.has(name) && folderReady();
      const ownCall = own && OWNED.has(name) && (!SPLITS.has(name) || Boolean(topologyFigure))
        && (name !== 'makeMovieclipUnique' || Boolean(movieclipFigure)) && (!FOLDERS.has(name) || folder);
      // Selection changes inside shadowed calls (history restoring a branch) are not seen by the listener.
      if (live && runtimeState()) { end('runtime state the engine does not hold'); live = null; }
      if (live && (RANGE.has(name) || TOPOLOGY.includes(name) || (name === 'mirrorSelection' || name === 'mirrorFigure') && editor.selectedJoint >= 0) && legacyRuntime()) { end('poses with runtime segments'); live = null; }
      if (live && (name === 'undo' && !live.can_undo() || name === 'redo' && !live.can_redo())) { end('history the session did not record'); live = null; }
      if (live && name === 'sheetPasteFrames' && (editor._sheetClipboard ?? null) !== copied) { end('a timeline clipboard the session did not copy'); live = null; }
      if (live && name === 'pasteSelection' && (editor._selectionClipboard ?? editor._figureClipboard ?? null) !== copiedFigures) { end('a figure clipboard the session did not copy'); live = null; }
      if (live && name === 'pasteSegmentValues' && args[0] == null && (editor._segmentValuesClipboard ?? null) !== copiedValues) { end('segment values the session did not copy'); live = null; }
      if (live && name === 'pasteCamera' && (editor._cameraClipboard ?? null) !== copiedCamera) { end('a camera the session did not copy'); live = null; }
      if (live && name === 'pasteCameraMotion' && (editor._cameraMotionClipboard ?? null) !== copiedMotion) { end('a camera motion clip the session did not copy'); live = null; }
      if (live && name === 'pasteVfxClip' && (editor._vfxClipClipboard ?? null) !== copiedVfxClip) { end('an effect clip the session did not copy'); live = null; }
      if (live) {
        // Trailing omitted arguments stay omitted; anything JSON cannot carry is not compared.
        let given = args.slice(); while (given.length && given.at(-1) === undefined) given.pop();
        if (RANGE_CALLS.includes(name)) given = given.map(value => value === undefined ? null : value);
        measured = captureEditBounds(name, given, this);
        if (given.some(value => value === undefined || typeof value === 'function' || value instanceof Set || name === 'undo' || name === 'redo')) payload = name === 'undo' || name === 'redo' ? '[]' : null;
        else try { payload = measured ? measured.payload() : JSON.stringify(given, frameRanges); } catch { payload = null; }
        if (payload === null) { end('arguments that are not plain data'); live = null; }
        else before = JSON.stringify({...view(), ...(ownCall ? {changesOnly: true} : {}), ...(topologyFigure ? {topologyPublication: true} : {}), ...(movieclipFigure ? {movieclipUniquePublication: true} : {}), ...(folder ? {layerPublication: true} : {}), ...(ownCall && (name === 'pasteSelection' || name === 'duplicateSelection') && !measured ? {figureClipboardPastes: true} : {})});
      }
      // Rig releases emit plain/definition edits; own static patches bypass keyed styles.
      // Keep these routes in the original editor with their eligible fallback timed;
      // comparison mode still sends the recorded commands to the session.
      if (live && own && OWNED.has(name) && !measured &&
          (name === 'commitDrag' && editor._drag?.rig || name === 'updateBuilderSegmentStyle' &&
            args[0] && typeof args[0] === 'object' && !Array.isArray(args[0]) && Object.hasOwn(args[0], 'static'))) {
        fallbackStarted = performance.now();
        end('a rig or static-definition call outside owned adoption'); live = null;
      }
      if (!live) report.skipped++;
      // The engine runs the call in the editor's place.
      // Callback bounds have no owning query yet: observe only the measurements the editor
      // itself requested, never evaluate the range a second time to prepare an owned call.
      if (live && ownCall && !measured) {
        const pasteClipboard = name === 'pasteSelection' ? editor._selectionClipboard ?? editor._figureClipboard ?? null : null;
        const pasteBefore = pasteClipboard?.pastes;
        const selection = editor._selectionState(), called = performance.now();
        let answer, text = '', parsing = called;
        try { text = call(live, name, payload, before); parsing = performance.now(); answer = JSON.parse(text); }
        catch (error) { if (report.mismatches.length < 50) report.mismatches.push({call: name, problems: [`the engine failed: ${error?.message ?? error}`]}); session = null; answer = null; broken = true; report.broken = String(error?.message ?? error).slice(0, 200); }
        report.calls++;
        const topology = answer?.status === 'ok' && topologyFigure && answer.result !== false ? prepareSplit(answer, topologyFigure, selection) : null;
        const unique = answer?.status === 'ok' && movieclipFigure ? prepareUnique(answer, movieclipFigure, selection) : null;
        const folderPlan = answer?.status === 'ok' && folder ? prepareFolder(name, answer, selection) : null;
        let pasteIncrement = 0, pasteWhy = null;
        if ((name === 'pasteSelection' || name === 'duplicateSelection') && answer?.status === 'ok') {
          const counter = answer.figureClipboardPastes, duplicate = name === 'duplicateSelection';
          const pair = duplicate ? counter !== null : Boolean(pasteClipboard);
          if (!Object.hasOwn(answer, 'figureClipboardPastes') || (pair
            ? !counter || typeof counter !== 'object' || Array.isArray(counter) || Object.keys(counter).length !== 2
              || !Object.hasOwn(counter, 'before') || !Object.hasOwn(counter, 'after')
              || !Number.isInteger(counter.before) || counter.before < 0 || !Number.isInteger(counter.after) || counter.after < 0
              || counter.before !== (duplicate ? 0 : pasteBefore) || counter.after !== counter.before && counter.after !== counter.before + 1
            : counter !== null || duplicate && (!Array.isArray(answer.events) || answer.events.length !== 0))) pasteWhy = 'a different figure clipboard counter';
          else if (counter) pasteIncrement = counter.after - counter.before;
        }
        const why = answer?.status === 'ok' ? pasteWhy ?? (folderPlan ? folderPlan.why : unique ? unique.why : topology ? topology.why : unadoptable(answer)) : null;
        if (answer?.status === 'ok' && !why) {
          depth++;
          const adopting = performance.now();
          const groupClipboard = name === 'duplicateSelection' ? editor._selectionClipboard : null;
          const singleClipboard = name === 'duplicateSelection' ? editor._figureClipboard : null;
          try {
            // Duplicate's original child events see its freshly copied clipboard.
            // Reuse that owner, then restore both saved clipboards in its finally.
            let counterClipboard = pasteClipboard;
            if (name === 'duplicateSelection') {
              if (answer.figureClipboardPastes === null) return [];
              if (!originals.get('copySelection').call(editor, {wholeFigures: args[0]?.wholeFigures})) { end('a different duplicate clipboard'); return []; }
              counterClipboard = editor._selectionClipboard ?? editor._figureClipboard;
            }
            return adopt(name, answer, selection, args, topology, unique, folderPlan, counterClipboard, pasteIncrement);
          } catch (error) { if (name === 'pasteSelection' || name === 'duplicateSelection') end('a figure paste publication failed'); throw error; }
          finally {
            if (name === 'duplicateSelection') { editor._selectionClipboard = groupClipboard; editor._figureClipboard = singleClipboard; }
            depth--; report.owned++; report.ownedMs += performance.now() - called;
            const entry = (report.byCall ??= {})[name] ??= {calls: 0};
            entry.calls++; entry.engineMs = (entry.engineMs ?? 0) + adopting - called; entry.adoptMs = (entry.adoptMs ?? 0) + performance.now() - adopting;
            entry.callMs = (entry.callMs ?? 0) + parsing - called; entry.answerBytes = (entry.answerBytes ?? 0) + text.length;
          }
        }
        let replayStarted = null;
        try {
          if (answer?.status === 'unported') { const key = `${name}: ${answer.reason}`; report.unported[key] = (report.unported[key] ?? 0) + 1; keep(key); end('a call the engine has not ported'); }
          else if (answer?.status === 'error') {
            // The editor throws the error itself, with its message; both are unchanged.
            depth++;
            replayStarted = performance.now();
            try { const result = original.apply(this, args); keep(`${name}: an error the editor did not raise`); end('an error the editor did not raise'); return result; }
            catch (error) { if ((error.messageKey ?? error.message) !== answer.error) { keep(`${name}: a different error`); end('a different error'); } throw error; }
            finally {
              depth--;
              const elapsed = performance.now() - replayStarted;
              report.jsMs += elapsed; report.ownableMs = (report.ownableMs ?? 0) + elapsed;
              const entry = (report.byCall ??= {})[name] ??= {calls: 0};
              entry.calls++; entry.editorMs = (entry.editorMs ?? 0) + elapsed;
            }
          } else if (answer) end(`an answer the editor cannot adopt (${name}: ${why})`);
          live = null;
        } finally {
          // Count a rejected owner attempt separately, including call/parse and refusal
          // cleanup, but never its original JS replay or a successful adoption twice.
          const elapsed = (replayStarted ?? performance.now()) - called;
          report.failedOwned++; report.failedOwnedMs += elapsed;
          const entry = (report.byCall ??= {})[name] ??= {calls: 0};
          entry.failedOwned = (entry.failedOwned ?? 0) + 1;
          entry.failedOwnedMs = (entry.failedOwnedMs ?? 0) + elapsed;
        }
      }
      const sheetBefore = editor.report._styk?.layerSheet;
      depth++; events = [];
      const started = fallbackStarted ?? performance.now();
      let result, thrown, draws = [];
      // What the editor draws at random during the call goes to the engine with it (D18).
      try { ({result, draws} = withDraws(() => original.apply(this, measured?.args ?? args))); } catch (error) { thrown = error; draws = error?.draws ?? []; } finally { depth--; }
      const captured = events; events = null;
      report.jsMs += performance.now() - started;
      // What the calls the engine would own cost the editor (compared with `ownedMs`).
      if (OWNED.has(name)) {
        report.ownableMs = (report.ownableMs ?? 0) + performance.now() - started;
        const entry = (report.byCall ??= {})[name] ??= {calls: 0};
        entry.calls++; entry.editorMs = (entry.editorMs ?? 0) + performance.now() - started;
      }
      if (live && typeof result?.then === 'function') { end('an asynchronous call'); live = null; }
      if (live && measured) {
        payload = measured.payload();
        if (payload === null) { end('bounds callback measurements unavailable'); live = null; }
      }
      if (live) {
        try {
          const called = performance.now(), answer = JSON.parse(call(live, name, payload, draws.length ? JSON.stringify({...JSON.parse(before), draws}) : before));
          report.engineMs += performance.now() - called; report.calls++;
          if (answer.status === 'unported') { const key = `${name}: ${answer.reason}`; report.unported[key] = (report.unported[key] ?? 0) + 1; keep(key); end('a call the engine has not ported'); }
          else compare(name, args, answer, result, thrown, captured, sheetBefore);
          if (name === 'sheetCopyFrames' && session && !thrown) copied = editor._sheetClipboard;
          if ((name === 'copySelection' || name === 'cutSelection') && session && !thrown) copiedFigures = editor._selectionClipboard ?? null;
          if (name === 'copySegmentValues' && session && !thrown && typeof result === 'string') copiedValues = editor._segmentValuesClipboard ?? null;
          if (name === 'copyCamera' && session && !thrown) copiedCamera = editor._cameraClipboard ?? null;
          if (name === 'copyCameraMotion' && session && !thrown) copiedMotion = editor._cameraMotionClipboard ?? null;
          if ((name === 'copyVfxClip' || name === 'duplicateVfxClip') && session && !thrown) copiedVfxClip = editor._vfxClipClipboard ?? null;
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
    try { answer = JSON.parse(call(live, 'beginBatch', JSON.stringify([label]), JSON.stringify(view()))); }
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
        const called = performance.now(), ended = JSON.parse(call(session, 'endBatch', '[]', JSON.stringify(view())));
        report.engineMs += performance.now() - called; report.calls++;
        if (ended.status !== 'ok') end('a batch the engine cannot close');
        else compare('endBatch', [], ended, result, null, closed, sheetBefore);
      } catch (error) { if (report.mismatches.length < 50) report.mismatches.push({call: 'endBatch', problems: [`the engine failed: ${error?.message ?? error}`]}); session = null; broken = true; }
    }
    return result;
  };
  return report;
}
