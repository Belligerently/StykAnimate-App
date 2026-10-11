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
import {messageError} from '../localized-error.mjs';
import {invalidateFrameIndex} from '../frame-ids.mjs';
import {invalidateMovieclipTypes} from '../movieclips.mjs';

/** The undo/redo answer when the entry to apply is the editor's own (external.rs). */
const EXTERNAL_ENTRY = 'external history entry ';
/** The tables the read view tracks record by record. */
const TABLES = ['figure_types', 'keyframes', 'backgrounds'];

/** Calls that only change what is shown or selected: the editor runs them, the session
 * receives the result as the view of the next call. */
const VIEW_CALLS = new Set(['select', 'selectMany', 'selectPolygon', 'toggleSegment', 'selectBranch', 'selectConnected', 'selectMarquee',
  'selectSameDefinition', 'invertSelection', 'seek', 'setActiveLayer', 'selectSheetLayer', 'selectFrame', 'keepSelectionOnSeek']);

/** Gestures whose live state the interface reads from the editor itself (motion-path
 * trails, figure-handle and rig sessions, camera drags, property and effect-clip edits,
 * frame drafts): the editor runs them until those reads are engine queries (phase 2). */
const EDITOR_GESTURES = new Set(['beginMotionPathEdit', 'updateMotionPathEdit', 'commitMotionPathEdit', 'cancelMotionPathEdit', 'setMotionPathFalloff',
  'beginFigureHandleDrag', 'transformFigureHandle', 'commitFigureHandleDrag', 'beginRigDrag', 'editRigJoint', 'commitRigDrag', 'cancelRigDrag',
  'beginCameraDrag', 'updateCameraDrag', 'commitCameraDrag', 'cancelCameraDrag', 'beginAnimationEdit', 'previewAnimationEdit', 'commitAnimationEdit',
  'cancelAnimationEdit', 'beginVfxClipEdit', 'commitVfxClipEdit', 'cancelVfxClipEdit', 'beginSegmentValueEdit', 'beginHandleEditDrag', 'editHandlePosition',
  'commitHandleEditDrag', 'beginFrameDraft', 'beginFrameDraftPreview', 'endFrameDraftPreview', 'commitFrameDraft', 'discardFrameDraft', 'updateDraftFrame',
  'addDraftFrame', 'editFrameDraftTimeline', 'enableFrameDrafts', 'disableFrameDrafts', 'beginDuplicateDrag', 'finishDuplicateDrag', 'beginTween', 'updateTween',
  'cancelAuthoredPoseDrag', 'commitAuthoredPoseDrag', 'movieclipBatch', 'movieclipTimelineEdit']);

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

/** A frame range as the app passes it (`rangeFrames()`), which JSON would drop. */
const frameRanges = (key, value) => Array.isArray(value) && Object.hasOwn(value, 'sheetTicks') ? {'@array': [...value], sheetTicks: value.sheetTicks} : value;
const selectionMetadata = state => Object.fromEntries(['polygonSelection', 'segmentIds', 'branch'].filter(key => Object.hasOwn(state, key)).map(key => [key, state[key]]));

/**
 * @param {object} editor the app's Editor, with an empty history (just loaded)
 * @param {{start(editor): object|null, methods: Iterable<string>, verify?: boolean, draws?(): Array}} options
 *   `start` opens a session on the editor's document as it is (EditSession), `methods` names
 *   the calls to route to it, `verify` compares the session's document after every call
 *   (tests), `draws` supplies the random values a call may take (D18).
 */
export function installEngineOwner(editor, {start, methods = ownedMethods(editor), verify = false, draws = defaultDraws} = {}) {
  const report = {calls: 0, native: 0, fallback: 0, external: 0, synced: 0, refused: {}, verified: 0};
  let session = null, document = null, depth = 0, history = null, nextEntry = 1, applying = false;
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
    const text = session.call(method, JSON.stringify(args), JSON.stringify({...view(), patch: true}));
    report.trace?.push(`${method} ${JSON.stringify(args).slice(0, 120)} -> ${text.slice(0, 160)}`);
    const answer = JSON.parse(text);
    if (answer.status !== 'ok') throw new Error(`The engine refused the editor's records (${method}): ${answer.error ?? answer.reason}`);
    adoptState(answer, {patch: false});
    // What the session changed besides the editor's records (sources it collected).
    if (Array.isArray(answer.result) && answer.result.length) {
      applying = true;
      try {
        applyDocumentPatch(editor.report, answer.result);
        for (const op of answer.result) if (op.p[0] === '_styk' && op.p[1] === 'frameSources' && op.x) editor._generatedFrameSources?.delete(op.p[2]);
        if (answer.result.some(op => op.p[0] === '_styk' && op.p[1] === 'frameSources' && op.p.length === 2)) {
          const kept = new Set(Object.keys(editor.report._styk?.frameSources ?? {}));
          for (const id of [...(editor._generatedFrameSources ?? [])]) if (!kept.has(id)) editor._generatedFrameSources.delete(id);
        }
        refresh(answer.result);
        editor._invalidate({timing: true});
      } finally { applying = false; }
    }
    return answer;
  };
  const generated = () => [...(editor._generatedFrameSources ?? [])];
  const counters = () => [editor.layerRevision, editor._nextFrameIdentity, editor._nextUid];
  /** The counters the session last had: a flush sends the editor's when they moved. */
  let known = null;
  /** Writes what the editor changed since the session last had its document. */
  const flush = ({entry = null} = {}) => {
    if (!session || applying) return;
    const ops = changes();
    if (entry) {
      send('@external', [entry.label, entry.id, entry.retained, generated(), entry.bytes, ops, counters()]);
      report.external++;
    } else if (ops.length || hints?.touched || JSON.stringify(counters()) !== JSON.stringify(known)) { send('@sync', [ops, generated(), Boolean(hints?.touched), counters()]); report.synced++; }
    take();
    check();
  };
  const check = () => {
    if (!verify || !session) return;
    const engine = session.report_json(), view = JSON.stringify(editor.report, (key, value) => key === 'runtime_segments' ? undefined : value);
    if (engine !== view) {
      let at = 0; while (engine[at] === view[at]) at++;
      throw new Error(`The engine's document differs from the read view at ${at}: engine …${engine.slice(Math.max(0, at - 60), at + 60)}… view …${view.slice(Math.max(0, at - 60), at + 60)}…`);
    }
    report.verified++;
  };

  // --- answers ------------------------------------------------------------------------
  /** Updates what the editor derives from the records a patch replaced. */
  const refresh = (patch, before) => {
    const touched = new Set(patch.map(op => op.p[0] ?? '*'));
    const frames = touched.has('keyframes') || touched.has('*') || patch.some(op => op.p[0] === '_styk' && (op.p[1] === 'frameSources' || op.p.length === 1));
    if (frames || touched.has('_styk')) {
      // Layer membership: counted again from the document, figures seen before kept.
      const seen = editor._layerSeenUids;
      editor._initializeLayers();
      if (seen) for (const uid of seen) editor._layerSeenUids.add(uid);
    }
    if (touched.has('keyframes') || touched.has('*')) invalidateFrameIndex(editor.report);
    if (touched.has('figure_types') || touched.has('*')) invalidateMovieclipTypes(editor.report);
    editor._layerDescriptors = null; editor._layerUidArrays = new Map(); editor._nativeAnimationChannels = null;
    void before;
  };
  const adoptState = (answer, {patch = true} = {}) => {
    const {state} = answer;
    if (patch && answer.patch?.length) {
      applying = true;
      try {
        applyDocumentPatch(editor.report, answer.patch);
        refresh(answer.patch);
        editor._invalidate({timing: true});
      } finally { applying = false; }
    }
    // The playhead first: the selection belongs to the frame it shows. Set as history
    // sets it: the editor's seek would also end a gesture the session keeps going.
    if (state && (state.tick !== editor.tick || state.frameIndex !== editor.frameIndex)) {
      editor.tick = state.tick; editor.frameIndex = state.frameIndex; editor._invalidate({viewOnly: true});
    }
    if (Object.hasOwn(answer, 'drag')) gesture(answer);
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
    if (Array.isArray(answer.depths)) mirror(answer);
    if (Number.isFinite(answer.revision)) {
      editor._revision = answer.revision;
      editor._revisionSerial = Math.max(editor._revisionSerial ?? 0, answer.revision);
    }
  };
  /** The editor's history stacks follow the session's: the editor's own commands for the
   * entries it applies, placeholders for the session's entries, so the editor's code that
   * reads or amends the entry on top keeps working. */
  let depths = [0, 0];
  const mirror = answer => {
    const [undo, redo] = answer.depths, stacks = [editor._undo, editor._redo];
    const placeholder = label => ({kind: 'engine', label, bytes: 0, beforeRevision: editor._revision, afterRevision: answer.revision});
    if (undo === depths[0] + 1 && redo === 0 && stacks[0].length === depths[0]) {
      // A new entry the session recorded itself.
      stacks[0].push(placeholder(answer.state?.undoLabel ?? '')); editor._redo = [];
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
      if (editor._drag?.engine) { editor._drag = null; editor._authoredPoseEdit = null; editor._invalidate(); }
      return;
    }
    if (!editor._drag?.engine) {
      const source = editor.evaluatedFrame;
      editor._authoredPoseEdit = answer.draft ? {frame: {...source, objects: source.objects.slice()}, source, owned: new Set(), tick: editor.tick, engine: true} : null;
      const taken = new Set(drag.before);
      editor._drag = {engine: true, label: drag.label, scope: drag.translation ? 'translation' : 'selection', authored: Boolean(answer.draft), index: editor.frameIndex,
        before: source.objects.filter(object => taken.has(object.unique_id)).map(object => structuredClone(object)), beforeSelection: editor._selectionState(), changed: false};
    }
    editor._drag.label = drag.label; editor._drag.scope = drag.translation ? 'translation' : 'selection';
    // The document changed under the gesture (a figure drawn meanwhile): the draft is the
    // new frame on show with the gesture's own figures as the draft has them.
    if (answer.patch?.length && editor._authoredPoseEdit?.engine) {
      const previous = editor._authoredPoseEdit, taken = new Set(drag.before);
      editor._authoredPoseEdit = null; editor._invalidate();
      const source = editor.evaluatedFrame, kept = new Map(previous.frame.objects.filter(object => taken.has(object.unique_id)).map(object => [object.unique_id, object]));
      editor._authoredPoseEdit = {...previous, source, frame: {...source, objects: source.objects.map(object => kept.get(object.unique_id) ?? object)},
        owned: new Set(source.objects.flatMap((object, slot) => kept.has(object.unique_id) ? [slot] : []))};
    }
    const edit = editor._authoredPoseEdit;
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
    if (name === 'createLayer' && answer.result?.id) return editor.getLayer(answer.result.id);
    return answer.result;
  };

  // --- calls ----------------------------------------------------------------------------
  const ensure = () => {
    if (session && document === editor.report) return session;
    session?.free(); session = null;
    if (editor._undo.length || editor._redo.length) return null;
    const started = start(editor);
    if (!started) return null;
    session = started; document = editor.report;
    entries.clear(); depths = [0, 0];
    take();
    try {
      for (const [method, args] of [['@historyLimits', [editor.historyLimit, editor.historyBytes]], ['@revisions', [editor._revision, editor._revisionSerial, editor._savedRevision]]]) {
        const answer = JSON.parse(session.call(method, JSON.stringify(args), JSON.stringify(view())));
        if (answer.status !== 'ok') throw new Error(`${method}: ${answer.error ?? answer.reason}`);
      }
      history = {canUndo: false, canRedo: false, undoLabel: '', redoLabel: '', dirty: editor._revision !== editor._savedRevision};
      check();
    } catch (error) {
      session.free(); session = null; document = null;
      report.broken = String(error.message ?? error);
      return null;
    }
    return session;
  };
  /** Runs `name` in the JavaScript editor and writes what it changed back. */
  const fallback = (name, original, args) => {
    const outer = captured === null;
    if (outer) captured = [];
    depth++;
    let result, pending = false;
    const done = () => {
      depth--;
      if (outer) {
        const commands = captured; captured = null;
        writeBack(commands);
      }
    };
    try {
      result = original.apply(editor, args);
      // An asynchronous call is one unit until it settles: calls made meanwhile run here too.
      if (typeof result?.then === 'function') { pending = true; return result.finally(done); }
    } finally { if (!pending) done(); }
    return result;
  };
  const writeBack = commands => {
    if (commands.length) hintsOf(commands, hints ??= freshHints(), report);
    // A gesture the editor runs edits the shown keyframe in place until it ends.
    if (editor._drag && !editor._drag.engine && !editor._drag.authored) (hints ??= freshHints()).frames.add(editor._drag.index ?? editor.frameIndex);
    if (!commands.length) { flush(); return; }
    // One session entry per editor entry; the records changed go with the first.
    for (const command of commands) {
      const id = nextEntry++;
      entries.set(id, [command]);
      flush({entry: {id, label: command.label ?? '', bytes: command.bytes ?? 0, retained: retainedSources([command])}});
    }
  };
  const callSession = (name, original, args) => {
    const live = ensure();
    if (!live) return original.apply(editor, args);
    flush();
    // A gesture the editor runs (or began) stays with it to its end.
    if (EDITOR_GESTURES.has(name) || (editor._drag && !editor._drag.engine) || editor._animationEdit || editor._vfxClipEdit || editor._frameDraftEnabled) {
      report.fallback++; (report.editor ??= {})[name] = (report.editor[name] ?? 0) + 1;
      return fallback(name, original, args);
    }
    let payload;
    try { payload = JSON.stringify(args, frameRanges); } catch { payload = null; }
    if (payload === null || args.some(value => typeof value === 'function')) { report.refused[`${name}: arguments that are not plain data`] = (report.refused[`${name}: arguments that are not plain data`] ?? 0) + 1; report.fallback++; return fallback(name, original, args); }
    report.calls++;
    const answer = JSON.parse(live.call(name, payload, JSON.stringify({...view(), patch: true, changesOnly: true, draws: draws(), drawPool: true})));
    report.trace?.push(`${name} -> ${answer.status} ${answer.reason ?? answer.error ?? ''} rev ${answer.revision} drag ${JSON.stringify(answer.drag)} draft ${JSON.stringify(answer.draft?.map(f => f.unique_id))} state ${JSON.stringify(answer.state).slice(0, 140)}`);
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
    adoptState(answer);
    if (answer.patch?.length) take();
    emit(answer);
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
    flush();
    const answer = JSON.parse(live.call(redo ? 'redo' : 'undo', '[]', JSON.stringify({...view(), patch: true})));
    if (answer.status === 'unported' && answer.reason?.startsWith(EXTERNAL_ENTRY)) {
      const id = Number(answer.reason.slice(EXTERNAL_ENTRY.length)), commands = entries.get(id);
      if (!commands) throw new Error(`The editor has no history entry ${id}.`);
      depth++;
      try { for (const command of redo ? commands : [...commands].reverse()) editor._applyCommand(command, redo ? 'after' : 'before'); }
      finally { depth--; }
      hintsOf(commands, hints ??= freshHints(), report);
      send('@externalDone', [id, redo, changes(), generated(), counters()]);
      take();
      check();
      return true;
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
  for (const name of new Set(methods)) {
    const original = editor[name];
    if (typeof original !== 'function' || VIEW_CALLS.has(name) || name === 'undo' || name === 'redo') continue;
    originals.set(name, original);
    editor[name] = function owned(...args) {
      // A call made by a call the editor runs belongs to it.
      if (depth > 0 || editor._movieclipScopes?.length) return original.apply(this, args);
      return callSession(name, original, args);
    };
  }
  editor.undo = function owned(...args) { return depth > 0 || editor._movieclipScopes?.length ? originals.get('undo').apply(this, args) : historyCall(false, args); };
  editor.redo = function owned(...args) { return depth > 0 || editor._movieclipScopes?.length ? originals.get('redo').apply(this, args) : historyCall(true, args); };
  editor.markSaved = function owned(...args) {
    const result = originals.get('markSaved').apply(this, args);
    if (session && document === editor.report) { send('@markSaved', []); history.dirty = false; }
    return result;
  };
  // The editor's own commands record into the session's history, through the entry
  // their call writes back; their bookkeeping (revision, weight) still runs here.
  editor._record = function owned(command) {
    if (!session || document !== editor.report || editor._movieclipScopes?.length) return originals.get('_record').call(this, command);
    const length = this._undo.length;
    originals.get('_record').call(this, command);
    // Pushed (not folded into a batch still open): the session gets an entry for it.
    const recorded = this._undo.at(-1) === command && this._undo.length >= Math.min(length + 1, this.historyLimit) ? [command] : [];
    if (captured) captured.push(...recorded);
    else if (recorded.length) writeBack(recorded);
  };
  // Pose sources only history keeps alive are collected by the session, which holds history.
  editor._pruneFrameSources = function owned(...args) {
    if (!session || document !== editor.report) return originals.get('_pruneFrameSources').apply(this, args);
  };
  for (const [name, read] of Object.entries({
    canUndo: () => history.canUndo, canRedo: () => history.canRedo, undoLabel: () => history.undoLabel, redoLabel: () => history.redoLabel,
  })) {
    const prototype = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(editor), name) ?? findGetter(editor, name);
    Object.defineProperty(editor, name, {configurable: true, get() { return session && document === editor.report && history ? read() : prototype.get.call(this); }});
  }
  const dirty = findGetter(editor, 'dirty');
  Object.defineProperty(editor, 'dirty', {configurable: true,
    get() { return session && document === editor.report && history ? history.dirty || Boolean(this._drag?.changed || this._animationEdit?.changed) : dirty.get.call(this); },
    set(value) { dirty.set.call(this, value); }});
  // Edits other modules make in place, or announce without a command.
  editor.addEventListener('change', event => {
    const detail = event.detail;
    if (!session || document !== editor.report || depth > 0 || applying || !['edit', 'history', 'document'].includes(detail?.kind)) return;
    if (detail.kind === 'document') return;
    hints ??= freshHints();
    for (const index of detail.figureIndices ?? []) hints.figures.add(index);
    for (const index of detail.frameIndices ?? []) hints.frames.add(index);
    for (const id of detail.sourceIds ?? []) hints.sources.add(id);
    if (detail.definition && !detail.figureIndices?.length) hints.allFigures = true;
    if (detail.label === 'Project metadata') hints.touched = true;
  });
  // A call the editor runs names what it changed in place in its events.
  const listen = editor._emit;
  editor._emit = function owned(kind, detail = {}) {
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
  report.session = () => session;
  report.release = () => { session?.free(); session = null; document = null; };
  return report;
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

/** Random values a call may take (D18): a few of each kind, drawn as the editor draws them. */
function defaultDraws() {
  const pool = [];
  for (let index = 0; index < 4; index++) {
    pool.push(['uuid', crypto.randomUUID()]);
    pool.push(['seed', crypto.getRandomValues(new Uint32Array(1))[0]]);
    pool.push(['random', Math.random()]);
  }
  return pool;
}
