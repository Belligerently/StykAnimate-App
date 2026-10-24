/**
 * The engine owns the open document (D19, docs/engine/editing.md "Phase 3").
 *
 * One styk-edit session per document holds the document and its history. The editor's
 * methods that change the document run in the session; its answer carries a patch that
 * this module applies copy-on-write to `editor.report`, which the interface keeps reading
 * as a read view, then the session's selection, counters and change events.
 *
 * A call the session refuses runs in the JavaScript editor, on the read view. What it
 * changed is written back into the session as records, with a history entry the session
 * keeps in its place and asks the editor to apply on undo and redo (`external.rs`). The
 * same happens to edits other modules make through `editor._record` or in place.
 *
 * Calls that only move the view (selecting, seeking) stay with the editor; the session
 * receives the view with every call.
 */
import {applyDocumentPatch} from './engine-patch.mjs';
import {editorDraws, withDraws} from '../editor-draws.mjs';
import {messageError} from '../localized-error.mjs';
import {invalidateFrameIndex} from '../frame-ids.mjs';
import {invalidateMovieclipTypes} from '../movieclips.mjs';
import {layerIdForUid} from '../layers.mjs';
import {appBoundsMarker} from './engine-edit-bounds.mjs';

/** The undo/redo answer when the entry to apply is the editor's own (external.rs). */
const EXTERNAL_ENTRY = 'external history entry ';
/** The tables the read view tracks record by record. */
const TABLES = ['figure_types', 'keyframes', 'backgrounds'];

/** Calls that only change what is shown or selected: the editor runs them, the session
 * receives the result as the view of the next call. */
const VIEW_CALLS = new Set(['select', 'selectMany', 'selectPolygon', 'toggleSegment', 'selectBranch', 'selectConnected', 'selectMarquee',
  'selectSameDefinition', 'invertSelection', 'seek', 'setActiveLayer', 'selectSheetLayer', 'selectFrame', 'keepSelectionOnSeek']);

/** Gestures whose live state the interface reads from the editor itself (frame drafts): the
 * editor runs them until those reads are engine queries (phase 2). Pose drags, figure-handle,
 * Handle Edit Mode and segment-value gestures, rig and camera drags, motion-path edits, property
 * edits and effect-clip edits run in the session, which the editor shows as its own (`gesture`,
 * `handles`, `motion`, `edits`). */
const EDITOR_GESTURES = new Set([
  'beginFrameDraft', 'beginFrameDraftPreview', 'endFrameDraftPreview', 'commitFrameDraft', 'discardFrameDraft', 'updateDraftFrame',
  'addDraftFrame', 'editFrameDraftTimeline', 'enableFrameDrafts', 'disableFrameDrafts', 'beginDuplicateDrag', 'finishDuplicateDrag', 'beginTween', 'updateTween',
  'cancelAuthoredPoseDrag', 'commitAuthoredPoseDrag', 'movieclipBatch', 'movieclipTimelineEdit']);

/** The calls that make up a motion-path edit: the editor replays them when it takes the edit over. */
const MOTION_PATH_CALLS = new Set(['beginMotionPathEdit', 'updateMotionPathEdit', 'setMotionPathFalloff']);

/** The calls that end an edit, and the editor's field that holds it: with none open they change
 * nothing and answer false, which the owner knows without asking the session. */
const ENDS_EDIT = {commitVfxClipEdit: '_vfxClipEdit', cancelVfxClipEdit: '_vfxClipEdit', commitAnimationEdit: '_animationEdit', cancelAnimationEdit: '_animationEdit'};

/** Calls that read the document or the editor's state, open or close documents, or
 * configure the editor: never routed. */
const QUERIES = new Set(['addEventListener', 'removeEventListener', 'dispatchEvent', 'animationChannel', 'animationValue', 'canDisconnectSegment',
  'drawingKey', 'drawingTarget', 'figurePalette', 'frameClipboardDocument', 'frameClipboardDocumentView', 'frameIndexForId', 'getCameraMotions',
  'getFrameById', 'getFrameId', 'getIKSettings', 'getLayer', 'getLayerFigureUids', 'getLayerFocus', 'getLayerFolders', 'getLayers', 'getSpatialCamera',
  'getSpatialIKPole', 'getSpatialMotion', 'getSpatialPose', 'getSpatialProportions', 'getVfxClips', 'isFigureFocusLocked', 'isLayerFocusLocked',
  'joinedFigureIds', 'layerForUid', 'layerFrameKeys', 'motionPathReport', 'movieclipProjectReport', 'nativePoseChannel', 'nativePropertyChannel',
  'nearestAnimationKey', 'prepareGeometry', 'resolveAnimationTarget', 'resolveTranslationSelection', 'sampleVfxClip', 'segmentAnimationTarget',
  'serialize', 'vfxEffects', 'segmentValues', 'motionPathFalloff', 'previewInbetween', 'sheetSpan', 'transaction',
  'load', 'loadAsync', 'newDocument', 'detachDocument', 'discardDocument', 'resumeDocument', 'handOverDocument', 'markSaved',
  'setFrameClipboardWorker', 'setInbetweenPlanner', 'setLayerMigrationPlanner', 'enterMovieclip', 'leaveMovieclip',
  'setLayerFocus', 'clearLayerFocus', 'reconcileLayerFocus', 'prepareDrawingCel', 'undo', 'redo', 'constructor']);

/** The editor's public methods the owner routes: everything but queries and view calls. */
export function ownedMethods(editor) {
  const names = new Set();
  for (let prototype = Object.getPrototypeOf(editor); prototype && prototype !== Object.prototype; prototype = Object.getPrototypeOf(prototype)) {
    for (const name of Object.getOwnPropertyNames(prototype)) {
      const descriptor = Object.getOwnPropertyDescriptor(prototype, name);
      if (typeof descriptor.value === 'function' && !name.startsWith('_') && !QUERIES.has(name) && !VIEW_CALLS.has(name)) names.add(name);
    }
  }
  return names;
}

/** A call's arguments as the session takes them: frame ranges keep their `sheetTicks`, and the app's
 * figure-bounds callback is the marker the engine measures for itself (engine-edit-bounds.mjs). */
const sessionArguments = (key, value) => typeof value === 'function' ? appBoundsMarker(value)
  : Array.isArray(value) && Object.hasOwn(value, 'sheetTicks') ? {'@array': [...value], sheetTicks: value.sheetTicks} : value;
/** The reverse of the frame ranges `sessionArguments` writes, as a `JSON.parse` reviver. */
const unframeRanges = (key, value) => value && typeof value === 'object' && Object.hasOwn(value, '@array')
  ? Object.assign(value['@array'], Object.fromEntries(Object.entries(value).filter(([name]) => name !== '@array'))) : value;
const selectionMetadata = state => Object.fromEntries(['polygonSelection', 'segmentIds', 'branch'].filter(key => Object.hasOwn(state, key)).map(key => [key, state[key]]));

/**
 * @param {object} editor the app's Editor, with an empty history (just loaded)
 * @param {{start(editor): object|null, methods: Iterable<string>, verify?: boolean, draws?(): Array}} options
 *   `start` opens a session on the editor's document as it is (EditSession), `methods` names
 *   the calls to route to it, `verify` compares the session's document after every call
 *   (tests), `draws` supplies the random values a call may take (D18).
 */
export function installEngineOwner(editor, {start, methods = ownedMethods(editor), verify = false, draws = null} = {}) {
  const report = {calls: 0, native: 0, fallback: 0, external: 0, synced: 0, refused: {}, verified: 0};
  let session = null, document = null, depth = 0, history = null, nextEntry = 1, applying = false, spare = [], handing = false;
  /** The calls (as JSON) of the session's motion-path edit, from the one that began it. */
  let motionCalls = null;
  /** The same for the session's rig or camera drag, which the editor runs by state it builds itself. */
  let heldCalls = null;
  /** JavaScript commands of the session's external entries, by identity. */
  const entries = new Map();
  /** Commands the editor recorded during the call it is running for the owner. */
  let captured = null;
  /** Records the editor changed in place, as its change events name them. */
  let hints = null;
  /** The identity of every record when the session last had the same document. */
  let snapshot = null;
  const originals = new Map();

  const view = () => ({tick: editor.tick, frameIndex: editor.frameIndex, selectedUid: editor.selectedUid ?? null, selectedUids: [...(editor.selectedUids ?? [])],
    selectedJoint: editor.selectedJoint ?? -1, activeLayerId: editor.activeLayerId, autoKey: Boolean(editor.autoKey),
    sheetSelection: editor._sheetSelection ? {id: editor._sheetSelection.id, contents: Boolean(editor._sheetSelection.contents)} : null,
    ...selectionMetadata(editor._selectionState())});

  // --- the session's document as records -------------------------------------------
  const take = () => {
    const root = editor.report, styk = root._styk ?? null;
    snapshot = {
      keys: Object.keys(root), values: new Map(Object.entries(root)),
      tables: Object.fromEntries(TABLES.map(key => [key, Array.isArray(root[key]) ? root[key].slice() : null])),
      stykKeys: styk ? Object.keys(styk) : null, styk: styk ? new Map(Object.entries(styk)) : null,
      // Small metadata the editor changes in place (layers, palette): compared by text.
      stykText: styk ? new Map(Object.entries(styk).filter(([key]) => !['frameSources', 'layerSheet', 'audio', 'animation'].includes(key)).map(([key, value]) => [key, JSON.stringify(value)])) : null,
      sources: new Map(Object.entries(styk?.frameSources ?? {})),
      activeId: styk?.layers?.activeId,
    };
    hints = null;
  };
  /** The operations that bring the session's document to the read view. */
  const changes = () => {
    const root = editor.report, ops = [], all = hints?.all;
    const keys = Object.keys(root), styk = root._styk ?? null;
    const sameOrder = (before, after) => {
      // Deleting the removed keys and assigning the new ones reproduces this order.
      const simulated = {};
      for (const key of before) if (after.includes(key)) simulated[key] = true;
      for (const key of after) if (!before.includes(key)) simulated[key] = true;
      return JSON.stringify(Object.keys(simulated)) === JSON.stringify(after);
    };
    const record = (path, before, after) => {
      if (Array.isArray(after) && Array.isArray(before)) {
        const named = path[0] === 'figure_types' ? hints?.figures : path[0] === 'keyframes' ? hints?.frames : hints?.backgrounds;
        const known = new Set(before);
        if (before.length === after.length && after.every((item, index) => item === before[index] || !known.has(item))) {
          const every = path[0] === 'figure_types' ? hints?.allFigures : path[0] === 'keyframes' ? hints?.allFrames : hints?.allBackgrounds;
          after.forEach((item, index) => { if (all || every || item !== before[index] || named?.has(index)) ops.push({p: [...path, index], v: item}); });
        } else {
          const where = new Map(before.map((item, index) => [item, index]));
          ops.push({p: path, a: after.map((item, index) => where.has(item) && !all && !named?.has(index) ? where.get(item) : {v: item})});
        }
      } else ops.push({p: path, v: after});
    };
    if (!sameOrder(snapshot.keys, keys)) ops.push({p: [], o: keys.map(key => snapshot.values.get(key) === root[key] && key !== '_styk' && !TABLES.includes(key) ? [key] : [key, root[key]])});
    else for (const key of snapshot.keys) if (!Object.hasOwn(root, key)) ops.push({p: [key], x: 1});
    const rootRebuilt = ops.length && ops.at(-1).p.length === 0;
    for (const key of keys) {
      const before = snapshot.values.get(key), after = root[key];
      if (TABLES.includes(key)) { if (Array.isArray(after) && Array.isArray(snapshot.tables[key]) && !rootRebuilt) record([key], snapshot.tables[key], after); else if (!rootRebuilt || before !== after) ops.push({p: [key], v: after}); continue; }
      if (key === '_styk' && styk && snapshot.styk) {
        const stykKeys = Object.keys(styk);
        if (!sameOrder(snapshot.stykKeys, stykKeys)) { ops.push({p: ['_styk'], v: styk}); continue; }
        for (const name of snapshot.stykKeys) if (!Object.hasOwn(styk, name)) ops.push({p: ['_styk', name], x: 1});
        for (const name of stykKeys) {
          const value = styk[name], old = snapshot.styk.get(name);
          if (name === 'frameSources' && value && typeof value === 'object' && old && typeof old === 'object' && !all) {
            const ids = Object.keys(value), oldIds = [...snapshot.sources.keys()];
            if (!sameOrder(oldIds, ids)) { ops.push({p: ['_styk', name], v: value}); continue; }
            for (const id of oldIds) if (!Object.hasOwn(value, id)) ops.push({p: ['_styk', name, id], x: 1});
            for (const id of ids) if (value[id] !== snapshot.sources.get(id) || hints?.sources?.has(id)) ops.push({p: ['_styk', name, id], v: value[id]});
            continue;
          }
          const text = snapshot.stykText.get(name);
          const changed = all || value !== old || (text !== undefined ? JSON.stringify(value) !== text : hints?.styk?.has(name));
          if (changed || !snapshot.styk.has(name)) ops.push({p: ['_styk', name], v: value});
        }
        continue;
      }
      if (rootRebuilt) continue;
      if (before !== after || all || !snapshot.values.has(key)) ops.push({p: [key], v: after});
    }
    return ops;
  };
  const send = (method, args) => {
    const viewText = JSON.stringify({...view(), patch: true});
    report.capture?.push({name: method, args: JSON.stringify(args), view: viewText});
    const text = session.call(method, JSON.stringify(args), viewText);
    report.trace?.push(`${method} ${JSON.stringify(args).slice(0, 120)} -> ${text.slice(0, 160)}`);
    const answer = JSON.parse(text);
    if (answer.status !== 'ok') throw new Error(`The engine refused the editor's records (${method}): ${answer.error ?? answer.reason}`);
    adoptState(answer, {patch: false});
    // What the session changed besides the editor's records (sources it collected).
    if (Array.isArray(answer.result) && answer.result.length) {
      applying = true;
      try {
        const before = posesBefore();
        applyDocumentPatch(editor.report, answer.result);
        for (const op of answer.result) if (op.p[0] === '_styk' && op.p[1] === 'frameSources' && op.x) editor._generatedFrameSources?.delete(op.p[2]);
        if (answer.result.some(op => op.p[0] === '_styk' && op.p[1] === 'frameSources' && op.p.length === 2)) {
          const kept = new Set(Object.keys(editor.report._styk?.frameSources ?? {}));
          for (const id of [...(editor._generatedFrameSources ?? [])]) if (!kept.has(id)) editor._generatedFrameSources.delete(id);
        }
        refresh(answer.result, before);
        editor._invalidate({timing: true});
      } finally { applying = false; }
    }
    return answer;
  };
  const generated = () => [...(editor._generatedFrameSources ?? [])];
  const counters = () => [editor.layerRevision, editor._nextFrameIdentity, editor._nextUid];
  /** The frame the session retained, while the editor still shows it. */
  let retainedShown = null;
  const stillShown = () => Boolean(retainedShown) && editor._evaluated === retainedShown && !editor._authoredPoseEdit;
  /** The counters the session last had: a flush sends the editor's when they moved. */
  let known = null;
  /** Writes what the editor changed since the session last had its document. */
  /** Whether the editor may have changed the document since the session last had it:
   * a call it ran, a command recorded or an edit announced outside the session's calls. */
  let pending = false;
  const flush = ({entry = null} = {}) => {
    if (!session || applying) return;
    // Selecting changes the active layer in place, without an edit event.
    if (!entry && !pending && !hints && JSON.stringify(counters()) === JSON.stringify(known) && editor.report._styk?.layers?.activeId === snapshot?.activeId) return;
    pending = false;
    const ops = changes();
    if (entry) {
      send('@external', [entry.label, entry.id, entry.retained, generated(), entry.bytes, ops, counters(), stillShown()]);
      report.external++;
    } else if (ops.length || hints?.touched || JSON.stringify(counters()) !== JSON.stringify(known)) { send('@sync', [ops, generated(), Boolean(hints?.touched), counters(), stillShown()]); report.synced++; }
    take();
    check();
  };
  const check = () => {
    if (!verify || !session) return;
    const engine = session.report_json(), view = JSON.stringify(editor.report);
    if (engine !== view) {
      let at = 0; while (engine[at] === view[at]) at++;
      throw new Error(`The engine's document differs from the read view at ${at}: engine …${engine.slice(Math.max(0, at - 60), at + 60)}… view …${view.slice(Math.max(0, at - 60), at + 60)}…`);
    }
    report.verified++;
  };

  // --- answers ------------------------------------------------------------------------
  /** Updates what the editor derives from the records a patch replaced. */
  /** The pose records and layer state before a patch, to update the layer index from. */
  const posesBefore = () => ({frames: editor.report.keyframes, sources: editor.report._styk?.frameSources, layers: editor.report._styk?.layers});
  /** Updates what the editor derives from the records a patch replaced: the layer index
   * from the poses that changed (by identity), frame and type indices, caches. */
  const refresh = (patch, before) => {
    const touched = new Set(patch.map(op => op.p[0] ?? '*'));
    const index = editor._layerUidCounts && before && !touched.has('*') ? before : null;
    if (!index) {
      // Counted again from the document, figures seen before kept.
      const seen = editor._layerSeenUids, revision = editor._layerRevision;
      editor._initializeLayers();
      // The session counts layer revisions; rebuilding the index is not one.
      editor._layerRevision = revision;
      if (seen) for (const uid of seen) editor._layerSeenUids.add(uid);
    } else {
      const counts = editor._layerUidCounts, groups = editor._layerGroups, report = editor.report;
      const delta = new Map(), crossed = [];
      const count = (records, sign) => { for (const record of records) for (const object of record?.objects ?? []) delta.set(object.unique_id, (delta.get(object.unique_id) ?? 0) + sign); };
      const compare = (old, next) => {
        if (old === next) return;
        const previous = new Set(old ?? []), current = new Set(next ?? []);
        count([...previous].filter(record => !current.has(record)), -1);
        count([...current].filter(record => !previous.has(record)), 1);
      };
      compare(index.frames, report.keyframes);
      if (index.sources !== report._styk?.frameSources) compare(Object.values(index.sources ?? {}), Object.values(report._styk?.frameSources ?? {}));
      for (const [uid, amount] of delta) {
        if (!amount) continue;
        const previous = counts.get(uid) ?? 0, next = previous + amount;
        if (next > 0) { counts.set(uid, next); editor._layerSeenUids.add(uid); } else counts.delete(uid);
        if ((previous > 0) !== (next > 0)) crossed.push(uid);
      }
      if (index.layers !== report._styk?.layers) {
        // Assignments or layers changed: every figure regrouped.
        const state = report._styk.layers;
        editor._layerGroups = new Map(state.order.map(id => [id, new Set()]));
        for (const uid of counts.keys()) editor._layerGroups.get(layerIdForUid(report, uid))?.add(uid);
      } else for (const uid of crossed) {
        const layer = groups.get(layerIdForUid(report, uid));
        if (counts.has(uid)) layer?.add(uid); else for (const members of groups.values()) members.delete(uid);
      }
    }
    if (verify && index) {
      // The incremental index equals one counted again from the document.
      const counts = new Map(editor._layerUidCounts), groups = new Map([...editor._layerGroups].map(([id, members]) => [id, [...members].sort((a, b) => a - b)]));
      const seen = editor._layerSeenUids, revision = editor._layerRevision;
      editor._initializeLayers();
      editor._layerRevision = revision; for (const uid of seen) editor._layerSeenUids.add(uid);
      const sorted = map => JSON.stringify([...map].sort(([a], [b]) => String(a).localeCompare(String(b))));
      const fresh = new Map([...editor._layerGroups].map(([id, members]) => [id, [...members].sort((a, b) => a - b)]));
      if (sorted(counts) !== sorted(editor._layerUidCounts) || sorted(groups) !== sorted(fresh)) throw new Error('The layer index after a patch differs from one counted from the document.');
    }
    if (touched.has('keyframes') || touched.has('*')) invalidateFrameIndex(editor.report);
    if (touched.has('figure_types') || touched.has('*')) invalidateMovieclipTypes(editor.report);
    editor._layerDescriptors = null; editor._layerUidArrays = new Map(); editor._nativeAnimationChannels = null;
  };
  const adoptState = (answer, {patch = true} = {}) => {
    const {state} = answer;
    if (patch && answer.patch?.length) {
      applying = true;
      try {
        const before = posesBefore();
        applyDocumentPatch(editor.report, answer.patch);
        refresh(answer.patch, before);
        editor._invalidate({timing: true});
      } finally { applying = false; }
    }
    // The playhead first: the selection belongs to the frame it shows. Set as history
    // sets it: the editor's seek would also end a gesture the session keeps going.
    if (state && (state.tick !== editor.tick || state.frameIndex !== editor.frameIndex)) {
      editor.tick = state.tick; editor.frameIndex = state.frameIndex; editor._invalidate({viewOnly: true});
    }
    if (answer.motionPathError) throw new Error(`The engine could not show the motion-path edit: ${answer.motionPathError}`);
    if (Object.hasOwn(answer, 'drag')) gesture(answer);
    if (Object.hasOwn(answer, 'handleGesture')) handles(answer);
    if (!handing) edits(answer);
    if (state) {
      history = {canUndo: state.canUndo, canRedo: state.canRedo, undoLabel: state.undoLabel, redoLabel: state.redoLabel, dirty: state.dirty};
      editor._nextUid = state.nextUid; editor._nextFrameIdentity = state.nextFrameIdentity; editor._layerRevision = state.layerRevision;
      known = [state.layerRevision, state.nextFrameIdentity, state.nextUid];
      const metadata = selectionMetadata(state);
      if (state.selectedUid !== (editor.selectedUid ?? null) || state.selectedJoint !== (editor.selectedJoint ?? -1)
          || JSON.stringify(state.selectedUids) !== JSON.stringify([...editor.selectedUids])
          || JSON.stringify(metadata) !== JSON.stringify(selectionMetadata(editor._selectionState()))) {
        // The session's selection is the editor's, exactly: assigned, not validated again.
        editor.selectedUids = new Set(state.selectedUids); editor._selectedUid = state.selectedUid; editor.selectedJoint = state.selectedJoint;
        editor._branchSelection = metadata.branch ? {...metadata.branch} : null;
        editor._polygonSelection = metadata.polygonSelection ? {...metadata.polygonSelection} : null;
        const active = editor.figure?.segments[editor.selectedJoint]?.segment_id;
        const ids = new Set(metadata.segmentIds ?? []);
        editor._segmentSelection = ids.size > 1 && ids.has(active) ? {uid: editor.selectedUid, active, ids} : null;
      }
    }
    if (Object.hasOwn(answer, 'sheetSelection')) {
      const chosen = answer.sheetSelection;
      editor._sheetSelection = chosen ? {...(editor._sheetSelection?.id === chosen.id ? editor._sheetSelection : {}), id: chosen.id, contents: chosen.contents} : null;
    }
    if (Array.isArray(answer.depths)) { if (writing) lastDepths = answer.depths; else mirror(answer); }
    if (Number.isFinite(answer.revision)) {
      editor._revision = answer.revision;
      editor._revisionSerial = Math.max(editor._revisionSerial ?? 0, answer.revision);
    }
  };
  /** The editor's history stacks follow the session's: the editor's own commands for the
   * entries it applies, placeholders for the session's entries, so the editor's code that
   * reads or amends the entry on top keeps working. */
  let depths = [0, 0], writing = false, lastDepths = null;
  const mirror = answer => {
    const [undo, redo] = answer.depths, stacks = [editor._undo, editor._redo];
    const placeholder = label => ({kind: 'engine', label, bytes: 0, beforeRevision: editor._revision, afterRevision: answer.revision});
    if (undo > depths[0] && redo === 0 && stacks[0].length === depths[0]) {
      // New entries the session recorded itself (a call may commit a gesture first).
      for (let added = depths[0]; added < undo; added++) stacks[0].push(placeholder(added === undo - 1 ? answer.state?.undoLabel ?? '' : ''));
      editor._redo = [];
    } else if (undo === depths[0] - 1 && redo === depths[1] + 1 && stacks[0].length) stacks[1].push(stacks[0].pop());
    else if (undo === depths[0] + 1 && redo === depths[1] - 1 && stacks[1].length) stacks[0].push(stacks[1].pop());
    // Entries the session retired (oldest first), or stacks out of step: as many as the session has.
    while (editor._undo.length > undo) editor._undo.shift();
    while (editor._redo.length > redo) editor._redo.shift();
    while (editor._undo.length < undo) editor._undo.unshift(placeholder(''));
    while (editor._redo.length < redo) editor._redo.unshift(placeholder(''));
    depths = [undo, redo];
  };
  /** The session's gesture in progress, as the editor shows it: its draft on show and
   * `_drag` with the figures it took as they were. */
  const gesture = answer => {
    const drag = answer.drag;
    if (!drag) {
      if (editor._drag?.engine) {
        // A motion-path edit changed no frame on show: the document's patch invalidated what changed.
        const motion = Boolean(editor._drag.motionPath);
        if (editor._drag.cameraBase) editor._sheetPropertyPreview = null;
        editor._drag = null; editor._authoredPoseEdit = null; motionCalls = null; heldCalls = null;
        if (!motion) editor._invalidate();
      }
      return;
    }
    if (answer.motionPath) { motion(answer); return; }
    if (!editor._drag?.engine) {
      const source = editor.evaluatedFrame;
      editor._authoredPoseEdit = answer.draft ? {frame: {...source, objects: source.objects.slice()}, source, owned: new Set(), tick: editor.tick, engine: true} : null;
      const taken = new Set(drag.before);
      editor._drag = {engine: true, label: drag.label, scope: drag.translation ? 'translation' : 'selection', authored: Boolean(answer.draft), index: editor.frameIndex,
        before: source.objects.filter(object => taken.has(object.unique_id)).map(object => structuredClone(object)), beforeSelection: editor._selectionState(), changed: false};
    }
    editor._drag.label = drag.label; editor._drag.scope = drag.translation ? 'translation' : 'selection';
    const edit = editor._authoredPoseEdit;
    if (edit && answer.draftFields) edit.frame = {...edit.frame, ...answer.draftFields};
    if (edit && answer.draft?.length) {
      const objects = edit.frame.objects;
      for (const figure of answer.draft) {
        const slot = objects.findIndex(object => object.unique_id === figure.unique_id);
        // A figure the gesture added (drawing a new figure while posing) joins the draft.
        if (slot < 0) {
          const at = Number.isInteger(figure.object_slot) && figure.object_slot <= objects.length ? figure.object_slot : objects.length;
          objects.splice(at, 0, figure); edit.owned.add(at);
          continue;
        }
        if (objects[slot]._movieclipSample !== undefined) figure._movieclipSample = objects[slot]._movieclipSample;
        objects[slot] = figure; edit.owned.add(slot);
      }
    }
    editor._drag.changed = Boolean(drag.changed);
    editor._invalidate();
  };
  /** The session's figure-handle, Handle Edit Mode and segment-value gesture, as the editor shows its own
   * (`_drag.figureHandle`, `_drag.handleEdit`, `_drag.segmentValueOptions`). The stage's pivot marker reads the
   * transform's origin; every answer says where the turns stand, the rest is told once. The editor's own
   * code reads the rest (the figures that move, the arcs the gesture solves) only when it takes the gesture
   * over, which asks the session for it (`takeHandles`). */
  const handles = answer => {
    const drag = editor._drag?.engine && !editor._drag.motionPath ? editor._drag : null;
    if (!drag) return;
    const {figureHandle, handleEdit, segmentValueOptions} = answer.handleGesture ?? {};
    if (!figureHandle) delete drag.figureHandle;
    else if (drag.figureHandle) Object.assign(drag.figureHandle.transform, {lastAngle: figureHandle.lastAngle, rotation: figureHandle.rotation});
    else {
      const {uid, rotate, scale, origin, distance, lastAngle, rotation} = figureHandle;
      drag.figureHandle = {uid, rotate, scale, transform: {origin: {x: origin[0], y: origin[1]}, distance, lastAngle, rotation}};
    }
    if (!handleEdit) delete drag.handleEdit;
    else if (!drag.handleEdit) {
      const {uid, index, point, bend} = handleEdit;
      drag.handleEdit = {uid, index, point: {x: point[0], y: point[1]}, bend};
    }
    if (!segmentValueOptions) delete drag.segmentValueOptions;
    else drag.segmentValueOptions ??= {...segmentValueOptions};
  };
  /** The session's motion-path edit, as the editor shows its own: `_drag` with what the interface
   * reads of it (the trails sample `report` over `range`; the stage draws the `display` pose; the arc's
   * overlay and the pinned-origin modifier read the falloff, its writes and the pinned figures of the
   * grabbed frame). The answer that opens it carries the fixed part; every answer says how it stands,
   * and `report` (the operations that turn the document into the virtual report the gesture shows)
   * when a pointer move changed it. */
  const motion = answer => {
    const live = answer.motionPath;
    // The gesture this one replaces was committed by the call; its draft is gone.
    if (editor._authoredPoseEdit?.engine) editor._authoredPoseEdit = null;
    let drag = editor._drag?.engine && editor._drag.motionPath ? editor._drag : null;
    if (live.target) drag = editor._drag = motionDrag(answer.drag.label, live);
    if (!drag) throw new Error('The engine holds a motion-path edit the editor does not show.');
    drag.moved = live.moved; drag.range = live.range;
    live.movedSubs?.forEach((moved, index) => { drag.subs[index].moved = moved; });
    if (live.report) drag.ops = live.report;
  };
  /** The editor's session of a motion-path edit, from the call that opened it. `report` and `display`
   * are built when read, from the document as it is then: the document with the operations the session
   * told, and the playhead's pose under it (nothing while the trail is as the document has it). */
  const motionDrag = (label, live) => {
    const drag = {motionPath: true, engine: true, label, scope: 'motion-path', documentToken: editor._documentToken, target: live.target, tick: live.tick, baseline: [], ops: [], moved: false, range: live.range};
    if (live.falloff) Object.assign(drag, {falloff: live.falloff, writes: live.writes, subs: live.subs.map(({tick, kind}) => ({tick, kind, moved: false})), grab: {baseline: live.baseline}});
    else drag.baseline = live.baseline;
    let built = null, shown = null;
    Object.defineProperties(drag, {
      report: {get() {
        if (!this.ops.length) return editor.report;
        if (built?.document !== editor.report || built.ops !== this.ops) {
          const report = {...editor.report};
          applyDocumentPatch(report, this.ops);
          built = {document: editor.report, ops: this.ops, report};
        }
        return built.report;
      }},
      display: {get() {
        if (!this.ops.length) return null;
        const report = this.report;
        if (shown?.report !== report || shown.tick !== editor.tick || shown.range !== this.range) shown = {report, tick: editor.tick, range: this.range, display: editor._motionPathDisplay(this)};
        return shown.display;
      }},
    });
    return drag;
  };
  /** The session's effect-clip and property edits, as the editor shows its own (`_vfxClipEdit`,
   * `_animationEdit`, and the frame a layer sheet's edit previews, `_sheetPropertyPreview`).
   * A mirror begins with the call that opens the edit (`mirrorEdit`); every answer says how the
   * edit stands, and `engine` marks it as the session's until a call the editor runs takes it. */
  const edits = answer => {
    const edit = editor._animationEdit;
    if (edit?.engine && Object.hasOwn(answer, 'animationEdit')) {
      if (answer.animationEdit) {
        edit.changed = answer.animationEdit.changed;
        if (edit.sheet) edit.value = answer.animationEdit.value;
      } else { editor._animationEdit = null; editor._sheetPropertyPreview = null; editor._invalidate(); }
    }
    if (answer.vfxEdit === false && editor._vfxClipEdit?.engine) editor._vfxClipEdit = null;
    if (answer.previewChanges && editor._animationEdit?.engine) {
      editor._sheetPropertyPreview = previewFrame(answer.previewChanges, editor._animationEdit.frame);
      editor._geometry = null;
    }
    // A camera drag shows the frame it began on with the camera it moves.
    if (answer.cameraPreview !== undefined && editor._drag?.engine) {
      const drag = editor._drag;
      drag.cameraBase ??= editor.evaluatedFrame;
      editor._sheetPropertyPreview = {...drag.cameraBase, camera: answer.cameraPreview};
      editor._geometry = null;
    }
  };
  /** A call the editor runs while the session holds a motion-path edit (or the session refuses a call of
   * it): the editor's code ends or amends the gesture as its own, so it replays the calls that made it,
   * which gives its session what the session had (a move the session rejected is not among them: it
   * changed nothing the edit shows or publishes). The session forgets the gesture; the document is
   * unchanged by it until it ends. */
  const takeMotion = () => {
    const calls = motionCalls ?? [];
    motionCalls = null;
    handing = true;
    try { send('@dropGesture', []); } finally { handing = false; }
    editor._drag = null;
    depth++;
    try { for (const [name, payload] of calls) originals.get(name).apply(editor, JSON.parse(payload)); } finally { depth--; }
  };
  /** A call the editor runs while the session holds a figure-handle, Handle Edit Mode or segment-value
   * gesture (the session refused it, or raised an error the editor raises itself): the gesture becomes
   * the editor's own, as it would have begun it. The session forgets it (`@dropGesture`; the document
   * it changed is the read view's) and tells what the editor's code needs to carry it on, which only the
   * session holds: the figures that move and the arcs it solves. */
  const takeHandles = () => {
    const drag = editor._drag, held = call('@gestureState', []).result;
    delete drag.engine;
    if (editor._authoredPoseEdit?.engine) delete editor._authoredPoseEdit.engine;
    handing = true;
    try { send('@dropGesture', []); } finally { handing = false; }
    const snapshots = new Map(drag.before.map(object => [object.unique_id, object]));
    const snapshot = slot => snapshots.get(editor.frame.objects[slot].unique_id);
    if (drag.figureHandle) drag.figureHandle.targets = held.figureHandle.map(slot => ({slot, before: snapshot(slot)}));
    if (drag.handleEdit) {
      drag.handleEdit = {uid: drag.handleEdit.uid, index: drag.handleEdit.index, point: drag.handleEdit.point, states: held.handleEdit.map(state => ({
        slot: state.slot, vertex: state.vertex, g: {segments: state.drawn.map(bend => ({bend}))}, before: snapshot(state.slot), indices: state.indices,
        incident: state.incident.map(shape => ({index: shape.index, seg: {segment_id: shape.id, primitive_code: shape.line ? 0 : 1}, angle: shape.angle, bend: shape.bend,
          start: {x: shape.start[0], y: shape.start[1]}, end: {x: shape.end[0], y: shape.end[1]}})),
        lengths: state.lengths, bends: state.bends, moved: state.moved})), bend: drag.handleEdit.bend};
    }
  };
  /** Keeps the calls of the session's rig or camera drag while it is open. The editor runs these
   * gestures by state the session does not share (the rig's geometry and snapshot, the camera's
   * keys), so taking one over means replaying them. A pointer move sets the state from its own
   * arguments, so it replaces the move before it; a keyboard nudge (`fromStart` false) adds to it. */
  const holdCall = (name, payload, answer) => {
    if (!answer.drag?.rig && !answer.drag?.camera) { heldCalls = null; return; }
    if (!heldCalls || name === 'beginRigDrag' || name === 'beginCameraDrag') { heldCalls = [[name, payload]]; return; }
    const moves = name === 'editRigJoint' || name === 'updateCameraDrag' && JSON.parse(payload)[1]?.fromStart !== false;
    if (moves && heldCalls.at(-1)[0] === name) heldCalls[heldCalls.length - 1] = [name, payload]; else heldCalls.push([name, payload]);
  };
  /** A call the editor runs while the session holds a rig or camera drag (or the session refuses a call
   * of it): the editor's code ends or amends the gesture as its own. The session forgets it; a pose it
   * previewed in the document goes back as it was, and the editor replays the calls that made the gesture. */
  const takeHeld = () => {
    const drag = editor._drag, calls = heldCalls;
    heldCalls = null;
    handing = true;
    try { send('@dropGesture', []); } finally { handing = false; }
    if (!drag.authored && drag.before.length) editor._replaceObjects(drag.index, drag.before);
    editor._invalidate();
    depth++;
    try { for (const [name, payload] of calls) originals.get(name).apply(editor, JSON.parse(payload, unframeRanges)); } finally { depth--; }
  };
  /** The edit a `begin` call opened in the session, as the editor's own would be. */
  const mirrorEdit = (name, answer) => {
    if (name === 'beginVfxClipEdit') { editor._vfxClipEdit = {engine: true, before: editor.report._styk.vfxClips}; return; }
    const opened = answer.animationEdit;
    if (!opened) return;
    // The edit it replaces ended with the call, and its preview with it.
    if (editor._animationEdit?.engine && editor._sheetPropertyPreview) { editor._sheetPropertyPreview = null; editor._invalidate(); }
    const {target, property, exactTarget, tick, initialValue, changed} = opened, edit = {engine: true, target, property, exactTarget, tick, initialValue, selection: editor._selectionState(), changed};
    editor._animationEdit = opened.sheet ? Object.assign(edit, {sheet: true, value: opened.value, frame: editor.evaluatedFrame})
      : Object.assign(edit, {before: editor.report._styk?.animation ?? null, beforeChannel: editor.animationChannel(target, property, {exactTarget})});
  };
  /** A call the editor runs while the session holds an edit belongs to that edit (the editor's
   * code ends or amends it): the edit becomes the editor's own, as it would have begun it. */
  const handOver = ({rigAndCamera = true} = {}) => {
    if (editor._drag?.engine && editor._drag.motionPath) takeMotion();
    else if (editor._drag?.engine && (editor._drag.figureHandle || editor._drag.handleEdit || editor._drag.segmentValueOptions)) takeHandles();
    else if (rigAndCamera && editor._drag?.engine && heldCalls) takeHeld();
    const animation = editor._animationEdit?.engine ? editor._animationEdit : null, vfx = editor._vfxClipEdit?.engine ? editor._vfxClipEdit : null;
    if (!animation && !vfx) return;
    handing = true;
    try { send('@dropEdits', [Boolean(animation), Boolean(vfx)]); } finally { handing = false; }
    if (animation) delete animation.engine;
    if (vfx) delete vfx.engine;
  };
  /** The session's change events, for the interface: they describe what the session already has. */
  const emit = answer => {
    const events = answer.events ?? [];
    if (!events.length) return;
    const was = applying; applying = true;
    try { for (const {kind, frameIndex, tick, ...detail} of events) editor._emit(kind, kind === 'seek' ? {} : detail); }
    finally { applying = was; }
  };
  /** What a call returns when its answer is a view the editor holds. */
  const resultOf = (name, answer) => {
    if (['duplicateFigure', 'addFigure', 'importFigure'].includes(name)) return editor.object;
    if (name === 'pasteSelection' || name === 'duplicateSelection') return editor.selectedObjects;
    if (name === 'sheetAppendFrame') return editor.evaluatedFrame;
    if (name === 'beginAnimationEdit') return editor._animationEdit;
    if (name === 'beginVfxClipEdit') return undefined;
    if (name === 'createLayer' && answer.result?.id) return editor.getLayer(answer.result.id);
    return answer.result;
  };

  // --- calls ----------------------------------------------------------------------------
  /** Sessions of the documents open beside this one (editor sessions, tabs), by document. */
  const parked = new Map();
  const park = () => {
    report.log?.push(`park ${Boolean(session)}`);
    if (!session) return;
    parked.set(document, {session, entries: new Map(entries), depths, history, known, snapshot, nextEntry});
    // Bounded: the oldest parked document gives its session up (it becomes the editor's again).
    while (parked.size > 8) { const [key, value] = parked.entries().next().value; value.session.free(); parked.delete(key); }
    session = null; document = null;
  };
  const call = (method, args) => {
    const answer = JSON.parse(session.call(method, JSON.stringify(args), JSON.stringify(view())));
    if (answer.status !== 'ok') throw new Error(`${method}: ${answer.error ?? answer.reason}`);
    return answer;
  };
  const ensure = () => {
    if (session && document === editor.report) return session;
    report.log?.push(`ensure: session ${Boolean(session)} same ${document === editor.report} name ${editor.name}`);
    park();
    const resumed = parked.get(editor.report);
    if (resumed) {
      parked.delete(editor.report);
      ({session} = resumed); document = editor.report;
      entries.clear(); for (const [id, commands] of resumed.entries) entries.set(id, commands);
      ({depths, history, known, snapshot, nextEntry} = resumed);
      return session;
    }
    // The editor's history from before the session: its own commands, which it applies itself.
    const stacks = [editor._undo, editor._redo];
    if (stacks.some(stack => stack.some(command => command.kind === 'engine'))) return null;
    const started = start(editor);
    report.log?.push(`start ${Boolean(started)}`);
    if (!started) return null;
    session = started; document = editor.report;
    entries.clear(); depths = [stacks[0].length, stacks[1].length];
    take();
    try {
      call('@historyLimits', [editor.historyLimit, editor.historyBytes]);
      call('@revisions', [editor._revision, editor._revisionSerial, editor._savedRevision]);
      // A session started from the mirror: the same records, maybe in another key order
      // and without what the mirror leaves out (audio). Bring it to the read view.
      const [rootKeys, stykKeys] = call('@shape', []).result, root = editor.report, ops = [];
      const rebuild = (path, object, keys) => {
        const held = new Set(keys), names = Object.keys(object ?? {});
        if (JSON.stringify(keys) !== JSON.stringify(names) || names.some(name => !held.has(name))) ops.push({p: path, o: names.map(name => held.has(name) ? [name] : [name, object[name]])});
      };
      rebuild([], root, rootKeys);
      if (root._styk && rootKeys.includes('_styk')) rebuild(['_styk'], root._styk, stykKeys);
      if (ops.length) call('@sync', [ops, [], false, counters()]);
      if (stacks[0].length || stacks[1].length) {
        const described = stack => stack.map(command => {
          const id = nextEntry++; entries.set(id, [command]);
          return [command.label ?? '', id, retainedSources([command]), command.bytes ?? 0];
        });
        call('@externalStacks', [described(stacks[0]), described(stacks[1])]);
      }
      history = {canUndo: stacks[0].length > 0, canRedo: stacks[1].length > 0, undoLabel: stacks[0].at(-1)?.label ?? '', redoLabel: stacks[1].at(-1)?.label ?? '',
        dirty: editor._revision !== editor._savedRevision};
      check();
    } catch (error) {
      session.free(); session = null; document = null; entries.clear();
      report.broken = String(error.message ?? error);
      return null;
    }
    return session;
  };
  /** The values a call may draw: the spare ones, then the caller's (`draws`) or fresh ones
   * up to a few of each kind. */
  const pool = () => {
    if (draws) return [...spare, ...draws()];
    const values = [...spare];
    for (const kind of ['uuid', 'seed', 'random']) for (let count = values.filter(([drawn]) => drawn === kind).length; count < 4; count++) values.push([kind, editorDraws[kind]()]);
    return values;
  };
  /** The editor's own draws take the spare ones first (then draw as usual). */
  const withSpare = run => {
    const outer = {...editorDraws};
    return withDraws(run, kind => {
      const at = spare.findIndex(([drawn]) => drawn === kind);
      return at >= 0 ? spare.splice(at, 1)[0][1] : outer[kind]();
    }).result;
  };
  /** Runs `name` in the JavaScript editor and writes what it changed back. */
  const fallback = (name, original, args, options) => {
    handOver(options);
    const outer = captured === null;
    if (outer) captured = [];
    pending = true;
    depth++;
    let result, unsettled = false;
    const done = () => {
      depth--;
      if (outer) {
        const commands = captured; captured = null;
        writeBack(commands);
      }
    };
    try {
      result = spare.length ? withSpare(() => original.apply(editor, args)) : original.apply(editor, args);
      // An asynchronous call is one unit until it settles: calls made meanwhile run here too.
      if (typeof result?.then === 'function') { unsettled = true; return result.finally(done); }
    } finally { if (!unsettled) done(); }
    return result;
  };
  const writeBack = recorded => {
    if (recorded.length) hintsOf(recorded, hints ??= freshHints(), report);
    // An editor call may record a command and then fold it into the next one: only what is
    // still on the editor's stack, in its order, becomes the session's entries.
    const stack = new Set(editor._undo), commands = recorded.filter(command => stack.has(command)).sort((a, b) => editor._undo.indexOf(a) - editor._undo.indexOf(b));
    // A gesture the editor runs edits the shown keyframe in place until it ends.
    if (editor._drag && !editor._drag.engine && !editor._drag.authored) (hints ??= freshHints()).frames.add(editor._drag.index ?? editor.frameIndex);
    if (!commands.length) { flush(); return; }
    // One session entry per editor entry; the records changed go with the first. The
    // editor's stack already holds them all: it follows the session's once, at the end.
    writing = true;
    try {
      for (const command of commands) {
        const id = nextEntry++;
        entries.set(id, [command]);
        flush({entry: {id, label: command.label ?? '', bytes: command.bytes ?? 0, retained: retainedSources([command])}});
      }
    } finally { writing = false; }
    if (lastDepths) { depths = [editor._undo.length, editor._redo.length]; mirror({depths: lastDepths, state: null, revision: editor._revision}); lastDepths = null; }
  };
  const callSession = (name, original, args) => {
    const live = ensure();
    if (!live) return original.apply(editor, args);
    if (ENDS_EDIT[name] && !editor[ENDS_EDIT[name]]) { report.calls++; report.native++; return false; }
    // A pointer that is not finite is rejected by the editor's own check, before it touches the gesture
    // (the mirror carries the document token that check follows): the session keeps its edit as it was.
    if (name === 'updateMotionPathEdit' && editor._drag?.engine && editor._drag.motionPath && !(Number.isFinite(args[0]?.x) && Number.isFinite(args[0]?.y))) {
      report.errors = (report.errors ?? 0) + 1;
      return original.apply(editor, args);
    }
    const flushed = performance.now();
    flush();
    if (report.ms) report.ms.flush += performance.now() - flushed;
    // A gesture the editor runs (or began) stays with it to its end.
    if (EDITOR_GESTURES.has(name) || (editor._drag && !editor._drag.engine) || (editor._animationEdit && !editor._animationEdit.engine)
        || (editor._vfxClipEdit && !editor._vfxClipEdit.engine) || editor._frameDraftEnabled) {
      report.fallback++; (report.editor ??= {})[name] = (report.editor[name] ?? 0) + 1;
      return fallback(name, original, args);
    }
    let payload;
    try { payload = JSON.stringify(args, sessionArguments); } catch { payload = null; }
    if (payload === null || args.some(value => typeof value === 'function')) { report.refused[`${name}: arguments that are not plain data`] = (report.refused[`${name}: arguments that are not plain data`] ?? 0) + 1; report.fallback++; return fallback(name, original, args); }
    report.calls++;
    const timing = report.ms ??= {flush: 0, session: 0, parse: 0, adopt: 0, emit: 0};
    let mark = performance.now();
    const viewText = JSON.stringify({...view(), patch: true, changesOnly: true, draws: pool(), drawPool: true});
    report.capture?.push({name, args: payload, view: viewText});
    const text = live.call(name, payload, viewText);
    const spent = performance.now() - mark;
    timing.session += spent; (timing.byMethod ??= {})[name] = (timing.byMethod[name] ?? 0) + spent; (timing.bytes ??= {})[name] = (timing.bytes[name] ?? 0) + text.length + viewText.length;
    mark = performance.now();
    const answer = JSON.parse(text);
    timing.parse += performance.now() - mark;
    // Draws the call did not take come first next time, so identities follow one sequence.
    spare = [];
    for (const pair of Array.isArray(answer.drawsLeft) ? answer.drawsLeft : []) if (spare.filter(([kind]) => kind === pair[0]).length < 4) spare.push(pair);
    report.trace?.push(`${name} -> ${answer.status} ${answer.reason ?? answer.error ?? ''} rev ${answer.revision} drag ${JSON.stringify(answer.drag)} draft ${JSON.stringify(answer.draft?.map(f => f.unique_id))} state ${JSON.stringify(answer.state).slice(0, 140)}`);
    // A pointer move the session rejected leaves its edit open, as the editor's does.
    const rejected = answer.status === 'error' && name === 'updateMotionPathEdit' && Boolean(answer.motionPath && editor._drag?.engine && editor._drag.motionPath);
    // An error the session raised without changing anything: the editor raises it itself,
    // with its message and the values it draws on the way, as the editor alone would.
    // (Not pastes: the clipboard a native copy filled is the session's.) A rig edit or camera drag
    // it leaves open stays the session's: the editor's check throws before it touches one.
    if (answer.status === 'error' && !rejected && !answer.patch?.length && !answer.drag === !editor._drag?.engine && !/paste/i.test(name)) {
      report.errors = (report.errors ?? 0) + 1;
      return fallback(name, original, args, {rigAndCamera: false});
    }
    if (answer.status === 'unported') {
      const key = `${name}: ${answer.reason}`;
      report.refused[key] = (report.refused[key] ?? 0) + 1; report.fallback++;
      // The session's gesture, ended by the editor because the session refuses to.
      if (editor._drag?.engine && (name === 'commitDrag' || name === 'cancelDrag')) {
        const ended = fallback(name, original, args);
        send('@dropGesture', []);
        if (editor._drag?.engine) { editor._drag = null; editor._authoredPoseEdit = null; editor._invalidate(); }
        return ended;
      }
      return fallback(name, original, args);
    }
    // A committed gesture's frame stays on show, as the session keeps it (`retained`),
    // until the session lets it go.
    mark = performance.now();
    const shown = answer.retained ? (editor._authoredPoseEdit?.frame ?? editor.evaluatedFrame) : null;
    const kept = answer.retainedKept ? editor._evaluated : null;
    adoptState(answer);
    if (answer.status === 'ok') holdCall(name, payload, answer);
    if (answer.status === 'ok' && (name === 'beginVfxClipEdit' || name === 'beginAnimationEdit')) mirrorEdit(name, answer);
    if (answer.status === 'ok' && MOTION_PATH_CALLS.has(name)) {
      if (name === 'beginMotionPathEdit') motionCalls = answer.motionPath ? [[name, payload]] : null;
      else motionCalls?.push([name, payload]);
    }
    if (shown && !editor._authoredPoseEdit) { editor._evaluated = retainedShown = retainedFrame(answer, shown); editor._geometry = null; editor._incremental = null; }
    else if (kept && !editor._authoredPoseEdit && !editor._evaluated) { editor._evaluated = kept; }
    if (!answer.retained && !answer.retainedKept) retainedShown = null;
    if (answer.patch?.length) take();
    timing.adopt += performance.now() - mark; mark = performance.now();
    emit(answer);
    timing.emit += performance.now() - mark;
    check();
    if (answer.status === 'error') throw failure(answer.error);
    report.native++;
    return resultOf(name, answer);
  };

  /** Undo or redo: the session's entry, or the editor's own when it is on top. */
  const historyCall = (redo, args) => {
    const live = ensure();
    if (!live) return originals.get(redo ? 'redo' : 'undo').apply(editor, args);
    // As the editor's undo and redo begin.
    if (editor._vfxClipEdit) editor.cancelVfxClipEdit();
    if (editor._animationEdit) editor.cancelAnimationEdit();
    if (!redo && editor._drag) editor.cancelDrag();
    // The editor's redo leaves an open motion-path edit, rig edit or camera drag open (the session
    // refuses to redo under some): the gesture becomes the editor's own first, as a call it runs would make it.
    if (redo && editor._drag?.engine) { if (editor._drag.motionPath) takeMotion(); else if (heldCalls) takeHeld(); }
    flush();
    const historyView = JSON.stringify({...view(), patch: true});
    report.capture?.push({name: redo ? 'redo' : 'undo', args: '[]', view: historyView});
    const answer = JSON.parse(live.call(redo ? 'redo' : 'undo', '[]', historyView));
    if (answer.status === 'unported' && answer.reason?.startsWith(EXTERNAL_ENTRY)) {
      const id = Number(answer.reason.slice(EXTERNAL_ENTRY.length)), [command] = entries.get(id) ?? [];
      if (!command) throw new Error(`The editor has no history entry ${id}.`);
      if ((redo ? editor._redo : editor._undo).at(-1) !== command) throw new Error('The editor\'s history is out of step with the session\'s.');
      // The editor's own replay: drawing entries prepare their tiles first (it may wait),
      // and it moves the entry on the editor's stacks.
      depth++;
      let result;
      try { result = editor._drawingHistoryReplay(command, redo ? 'after' : 'before', args[0]?.signal); }
      catch (error) { depth--; throw error; }
      const finish = () => {
        depth--;
        hintsOf([command], hints ??= freshHints(), report);
        depths = [editor._undo.length, editor._redo.length];
        send('@externalDone', [id, redo, changes(), generated(), counters()]);
        take();
        check();
        return true;
      };
      if (typeof result?.then === 'function') return result.then(finish, error => { depth--; throw error; });
      return finish();
    }
    if (answer.status !== 'ok') throw new Error(`The engine could not ${redo ? 'redo' : 'undo'}: ${answer.error ?? answer.reason}`);
    adoptState(answer);
    if (answer.patch?.length) take();
    emit(answer);
    check();
    return answer.result;
  };

  // --- installation ---------------------------------------------------------------------
  for (const name of ['seek', 'undo', 'redo', 'markSaved', '_record', '_pruneFrameSources']) originals.set(name, editor[name]);
  // The owner's methods live on a layer between the editor and its class: document
  // sessions (tabs) move the editor's own properties with each document, never these.
  const dirty = findGetter(editor, 'dirty');
  const getters = Object.fromEntries(['canUndo', 'canRedo', 'undoLabel', 'redoLabel'].map(name => [name, findGetter(editor, name)]));
  const listen = editor._emit;
  const layer = Object.create(Object.getPrototypeOf(editor));
  Object.setPrototypeOf(editor, layer);
  for (const name of new Set(methods)) {
    const original = editor[name];
    if (typeof original !== 'function' || VIEW_CALLS.has(name) || name === 'undo' || name === 'redo') continue;
    originals.set(name, original);
    layer[name] = function owned(...args) {
      // A call made by a call the editor runs belongs to it.
      if (depth > 0 || editor._movieclipScopes?.length) return original.apply(this, args);
      return callSession(name, original, args);
    };
  }
  layer.undo = function owned(...args) { return depth > 0 || editor._movieclipScopes?.length ? originals.get('undo').apply(this, args) : historyCall(false, args); };
  layer.redo = function owned(...args) { return depth > 0 || editor._movieclipScopes?.length ? originals.get('redo').apply(this, args) : historyCall(true, args); };
  layer.markSaved = function owned(...args) {
    const result = originals.get('markSaved').apply(this, args);
    if (session && document === editor.report) { send('@markSaved', []); history.dirty = false; }
    return result;
  };
  // The editor's own commands record into the session's history, through the entry
  // their call writes back; their bookkeeping (revision, weight) still runs here.
  layer._record = function owned(command) {
    if (!session || document !== editor.report || editor._movieclipScopes?.length) return originals.get('_record').call(this, command);
    const length = this._undo.length;
    originals.get('_record').call(this, command);
    // Pushed (not folded into a batch still open): the session gets an entry for it.
    const recorded = this._undo.at(-1) === command && this._undo.length >= Math.min(length + 1, this.historyLimit) ? [command] : [];
    pending = true;
    if (captured) captured.push(...recorded);
    else if (recorded.length) writeBack(recorded);
  };
  // Pose sources only history keeps alive are collected by the session, which holds history.
  layer._pruneFrameSources = function owned(...args) {
    if (!session || document !== editor.report) return originals.get('_pruneFrameSources').apply(this, args);
  };
  for (const [name, read] of Object.entries({
    canUndo: () => history.canUndo, canRedo: () => history.canRedo, undoLabel: () => history.undoLabel, redoLabel: () => history.redoLabel,
  })) {
    const prototype = getters[name];
    Object.defineProperty(layer, name, {configurable: true, get() { return session && document === editor.report && history ? read() : prototype.get.call(this); }});
  }
  Object.defineProperty(layer, 'dirty', {configurable: true,
    get() { return session && document === editor.report && history ? history.dirty || Boolean(this._drag?.changed || this._animationEdit?.changed) : dirty.get.call(this); },
    set(value) { dirty.set.call(this, value); }});
  // Edits other modules make in place, or announce without a command.
  editor.addEventListener('change', event => {
    const detail = event.detail;
    if (detail?.kind === 'document') report.log?.push(`document ${JSON.stringify(detail).slice(0, 120)} name ${editor.name} at ${Math.round(performance.now())}`);
    if (!session || document !== editor.report || depth > 0 || applying || !['edit', 'history', 'document'].includes(detail?.kind)) return;
    if (detail.kind === 'document') return;
    pending = true;
    hints ??= freshHints();
    for (const index of detail.figureIndices ?? []) hints.figures.add(index);
    for (const index of detail.frameIndices ?? []) hints.frames.add(index);
    for (const id of detail.sourceIds ?? []) hints.sources.add(id);
    if (detail.definition && !detail.figureIndices?.length) hints.allFigures = true;
    if (detail.label === 'Project metadata') hints.touched = true;
  });
  // A call the editor runs names what it changed in place in its events.
  layer._emit = function owned(kind, detail = {}) {
    if (session && document === editor.report && depth > 0 && !applying && ['edit', 'history'].includes(kind)) {
      hints ??= freshHints();
      for (const index of detail.figureIndices ?? []) hints.figures.add(index);
      for (const index of detail.frameIndices ?? []) hints.frames.add(index);
      for (const id of detail.sourceIds ?? []) hints.sources.add(id);
      if (detail.definition && !detail.figureIndices?.length) hints.allFigures = true;
      // A live edit changes the shown keyframe's poses in place (native timelines).
      if (detail.live || detail.changedUids) hints.frames.add(editor._drag?.index ?? editor.frameIndex);
    }
    return listen.call(this, kind, detail);
  };
  report.flush = () => flush();
  report.depths = () => depths;
  /** Whether the session holds the open document (starting it if it can). */
  report.ready = () => Boolean(ensure());
  report.session = () => session;
  report.release = () => { session?.free(); session = null; document = null; for (const value of parked.values()) value.session.free(); parked.clear(); };
  return report;
}

/** The frame the session keeps on show after a commit: its changed figures (and fields)
 * over the frame that was on show. */
function retainedFrame(answer, drafted) {
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
    frame = {...Object.fromEntries(answer.retainedFieldOrder.map(key => [key, frame[key]])), ...Object.fromEntries(Object.entries(frame).filter(([key]) => !fields.has(key)))};
  }
  return frame;
}

/** The frame a layer sheet's property edit previews: the changes over the frame it began on
 * (the figures that changed, the order, the other fields), as a retained frame is laid. */
function previewFrame({figures, order, fields, removed, fieldOrder}, base) {
  return retainedFrame({retained: figures, retainedChanges: true, retainedOrder: order, retainedFields: fields, retainedRemoved: removed, retainedFieldOrder: fieldOrder}, base);
}

const freshHints = () => ({figures: new Set(), frames: new Set(), backgrounds: new Set(), sources: new Set(), styk: new Set()});

/** The records the editor's history commands changed in place, by kind. */
function hintsOf(commands, into, report) {
  const frames = entries => { for (const entry of entries ?? []) { if (entry?.sourceId) into.sources.add(entry.sourceId); else if (Number.isInteger(entry?.index)) into.frames.add(entry.index); } };
  for (const command of commands) {
    for (const change of command.frameSourceChanges ?? []) into.sources.add(change.id);
    switch (command.kind) {
      case 'movieclip-batch': hintsOf(command.commands ?? [], into, report); break;
      case 'objects': case 'frame': case 'project': case 'figure-splice': into.frames.add(command.index); break;
      case 'import': into.frames.add(command.index); into.allFigures = true; break;
      case 'figure': case 'native-properties': case 'movieclip-definition': into.figures.add(command.index); break;
      case 'topology': case 'movieclip-poses':
        if (Number.isInteger(command.index)) into.figures.add(command.index);
        for (const side of ['before', 'after']) frames(command[side]?.frames);
        break;
      case 'camera': for (const side of ['before', 'after']) frames(command[side]); break;
      case 'frame-timing': case 'figure-range': for (const change of command.changes ?? []) if (Number.isInteger(change.index)) into.frames.add(change.index); break;
      case 'scene': into.allBackgrounds = true; into.allFrames = true; break;
      case 'disconnect': into.allFrames = true; into.allFigures = true; break;
      // These replace their records (or table) by identity, or change compared metadata.
      case 'ranges': case 'splice': case 'layers': case 'layer-sheet': case 'animation': case 'camera-motion': case 'vfx-clips': case 'palette': case 'audio': case 'fields': break;
      default:
        into.all = true;
        report.unknownKinds = [...new Set([...(report.unknownKinds ?? []), command.kind])];
    }
  }
}

/** The editor's error for an engine refusal: a message key, or the editor's own text. */
function failure(error) {
  try { return messageError(new Error(''), error, {}); } catch { return new Error(error); }
}

function findGetter(object, name) {
  for (let prototype = Object.getPrototypeOf(object); prototype; prototype = Object.getPrototypeOf(prototype)) {
    const descriptor = Object.getOwnPropertyDescriptor(prototype, name);
    if (descriptor) return descriptor;
  }
  throw new Error(`The editor has no ${name}.`);
}

/** The pose sources the editor's commands keep alive (`_pruneFrameSources`). */
function retainedSources(commands) {
  const ids = new Set();
  const visit = command => {
    if (command.kind === 'movieclip-batch') { for (const child of command.commands ?? []) visit(child); return; }
    for (const id of command.retainedFrameSourceIds ?? []) ids.add(id);
    for (const change of command.frameSourceChanges ?? []) ids.add(change.id);
    for (const side of ['before', 'after']) {
      if (command.kind === 'topology' || command.kind === 'movieclip-poses') for (const entry of command[side]?.frames ?? []) if (entry.sourceId) ids.add(entry.sourceId);
      const sheet = command.kind === 'layer-sheet' ? command[side] : null;
      for (const track of Object.values(sheet?.tracks ?? {})) for (const key of track.keys ?? []) {
        if (key.value?.frameId) ids.add(key.value.frameId);
        for (const patch of key.movieclipPatches ?? []) if (patch.frameId) ids.add(patch.frameId);
      }
    }
  };
  for (const command of commands) visit(command);
  return [...ids];
}

/**
 * The app's session start: on the stage's mirrored document, once its frames are current
 * (the session reuses the stage's prepared views). Until then the editor runs the calls,
 * and the session takes its history when it starts.
 * @param {() => object|null} engineSide the stage's engine side
 */
export function appSessionStart(engineSide) {
  let syncing = null;
  return editor => {
    const side = engineSide();
    if (!side || side.failed) return null;
    if (!side.mirror.framesCurrent(editor.report, editor.revision)) {
      syncing ??= side.mirror.syncFrames(editor.report, editor.revision).catch(() => {}).finally(() => { syncing = null; });
      return null;
    }
    return side.engine.handle.edit_session(JSON.stringify({tick: editor.tick, frameIndex: editor.frameIndex, selectedUid: editor.selectedUid ?? null,
      selectedUids: [...editor.selectedUids], selectedJoint: editor.selectedJoint ?? -1, activeLayerId: editor.activeLayerId, autoKey: Boolean(editor.autoKey),
      layerRevision: editor.layerRevision, nextFrameIdentity: editor._nextFrameIdentity, nextUid: editor._nextUid,
      propertyChannels: Boolean(editor.propertyChannels), layerTimeline: Boolean(editor.layerTimeline)})) ?? null;
  };
}
