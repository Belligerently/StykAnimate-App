/**
 * Shadow editing (docs/engine/editing.md): every editor call that the engine has
 * ported also runs in the engine, on the document the engine mirrors, and the two
 * results are compared: the returned value, the playhead and selection, the change
 * events, the gesture's draft and what a command published. The JavaScript editor
 * stays the one the app uses; nothing the engine answers is applied.
 *
 * Off unless `?engineShadow=on` or `localStorage['stykanimate.engine.shadowEdit'] = 'on'`.
 * Results accumulate in `globalThis.__stykEditShadow`.
 *
 * With `?engineEdit=on` (or `localStorage['stykanimate.engine.ownEdit'] = 'on'`) the
 * engine runs the calls in OWNED in the editor's place while a session lives: the editor
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

/** Calls styk-edit runs (engine/crates/styk-edit/src/lib.rs, `dispatch`). */
const PORTED = ['select', 'selectMany', 'seek', 'beginDrag', 'beginTranslationDrag', 'commitDrag', 'cancelDrag', 'moveSelection', 'rotateSelection',
  'scaleSelection', 'scaleSelected', 'mirrorSelection', 'rotateFigure', 'centerFigure', 'resetPose', 'poseJoint', 'rotateJoint', 'bendJoint',
  'sheetInsertKey', 'sheetDuplicateFrameAfter', 'sheetSetEasing', 'sheetClearKeys', 'sheetClearFrames', 'sheetInsertFrames', 'sheetRemoveFrames',
  'sheetSetDuration', 'sheetAppendFrame', 'joinTo', 'unjoinFigure', 'joinFigure', 'deleteSelection', 'deleteFigure', 'duplicateFigure', 'setConnectorPosition', 'undo', 'redo'];
/** Calls that return a frame or a figure record: a view, not compared. */
const VIEWS = new Set(['select', 'selectMany', 'seek', 'sheetAppendFrame', 'duplicateFigure']);
/** Calls the engine runs in the editor's place when it owns editing. */
const OWNED = new Set(['moveSelection', 'rotateSelection', 'scaleSelection', 'scaleSelected', 'mirrorSelection', 'rotateFigure', 'centerFigure', 'resetPose',
  'poseJoint', 'rotateJoint', 'bendJoint', 'commitDrag', 'sheetInsertKey', 'sheetDuplicateFrameAfter', 'sheetSetEasing', 'sheetClearKeys', 'sheetClearFrames',
  'sheetInsertFrames', 'sheetRemoveFrames', 'sheetSetDuration', 'sheetAppendFrame', 'joinTo', 'unjoinFigure', 'joinFigure', 'deleteSelection', 'deleteFigure', 'duplicateFigure',
  'setConnectorPosition']);

const setting = (query, key) => {
  try {
    const value = new URLSearchParams(globalThis.location?.search ?? '').get(query);
    if (value) return value === 'on';
    return globalThis.localStorage?.getItem(key) === 'on';
  } catch { return false; }
};
export const shadowEditEnabled = () => setting('engineShadow', 'stykanimate.engine.shadowEdit') || engineEditEnabled();
export const engineEditEnabled = () => setting('engineEdit', 'stykanimate.engine.ownEdit');

/** Event details as plain data, as engine/tools/edit-replay.mjs records them. */
const plain = (value, depth = 4) => {
  if (value === null || ['number', 'string', 'boolean'].includes(typeof value)) return value;
  if (value instanceof Set) return plain([...value], depth);
  if (typeof value !== 'object' || depth <= 0) return undefined;
  if (Array.isArray(value)) return value.map(item => plain(item, depth - 1)).filter(item => item !== undefined);
  return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, plain(item, depth - 1)]).filter(([, item]) => item !== undefined));
};
const withoutSample = (key, value) => key === '_movieclipSample' ? undefined : value;

/**
 * Whether two JSON texts say the same, numbers within a few units in the last place.
 * Browsers round `Math.sin`, `cos`, `atan2` and `hypot` differently from each other and
 * from the engine (docs/engine/decisions/0016-v8-exact-math.md), so a pose computed
 * through them may differ in its last digits; keys, order and everything else must be equal.
 */
function close(a, b) {
  if (a === b) return true;
  const near = (x, y) => {
    if (typeof x === 'number' && typeof y === 'number') return Math.abs(x - y) <= 1e-9 * Math.max(1, Math.abs(x), Math.abs(y));
    if (x === null || y === null || typeof x !== 'object' || typeof y !== 'object') return x === y;
    if (Array.isArray(x) !== Array.isArray(y)) return false;
    const keys = Object.keys(x), others = Object.keys(y);
    return keys.length === others.length && keys.every((key, index) => key === others[index] && near(x[key], y[key]));
  };
  try { return near(JSON.parse(a), JSON.parse(b)); } catch { return false; }
}

/**
 * @param {object} editor the app's Editor
 * @param {() => object|null} engineSide the stage's engine side (`renderer.engineSide`), once it exists
 */
export function installEditShadow(editor, engineSide, {own = engineEditEnabled()} = {}) {
  const report = globalThis.__stykEditShadow = {own, calls: 0, compared: 0, rounded: 0, skipped: 0, sessions: 0, ended: {}, unported: {}, mismatches: [], jsMs: 0, engineMs: 0, owned: 0, ownedMs: 0};
  let session = null, depth = 0, events = null;
  const end = why => {
    if (!session) return;
    session.free(); session = null;
    report.ended[why] = (report.ended[why] ?? 0) + 1;
  };
  const view = () => ({tick: editor.tick, frameIndex: editor.frameIndex, selectedUid: editor.selectedUid ?? null, selectedUids: [...(editor.selectedUids ?? [])],
    selectedJoint: editor.selectedJoint ?? -1, activeLayerId: editor.activeLayerId, autoKey: Boolean(editor.autoKey)});
  const counters = () => ({layerRevision: editor.layerRevision, nextFrameIdentity: editor._nextFrameIdentity, nextUid: editor._nextUid});

  editor.addEventListener('change', event => {
    const detail = event.detail;
    if (depth > 0) { events?.push(plain(detail ?? null)); return; }
    // The document changed through something the session did not run.
    if (['edit', 'history', 'document'].includes(detail?.kind)) end(`a ${detail.kind} outside the shadowed calls`);
  });

  /** Brings the engine's frames to the editor's revision (playback does it only when it prepares). */
  let syncing = null;
  const prepare = () => {
    const side = engineSide();
    if (!side || side.failed) return Promise.resolve(false);
    if (side.mirror.framesCurrent(editor.report, editor.revision)) return Promise.resolve(true);
    const target = editor.report, revision = editor.revision;
    syncing ??= side.mirror.syncFrames(target, revision).then(() => side.mirror.framesCurrent(editor.report, editor.revision), () => false).finally(() => { syncing = null; });
    return syncing;
  };
  report.prepare = prepare;

  const begin = () => {
    if (session) return session;
    const side = engineSide();
    if (!side || side.failed || editor._drag || editor._authoredPoseEdit || editor._animationEdit) return null;
    // Not there yet: this call is not shadowed, the next one may be.
    if (!side.mirror.framesCurrent(editor.report, editor.revision)) { prepare(); return null; }
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
      problems.push(`${label} (from character ${at}): engine …${a.slice(from, at + 160)}, editor …${b.slice(from, at + 160)}`);
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
          adoptLayerSheet(editor, {sheet: answer.sheet, trackOrder: answer.trackOrder, layers: answer.layers, sources: answer.sources, nextFrame: state.nextFrameIdentity},
            detail.label, {selection: drag?.beforeSelection ?? selection, retainGeometry: Boolean(preview)});
        } finally { editor._authoredCommitPreview = null; }
      } else if (kind === 'seek') originals.get('seek').call(editor, tick);
    }
    // A gesture that ended without publishing (nothing changed).
    if (!answer.drag && editor._drag && !published) { editor._drag = null; editor._authoredPoseEdit = null; editor._invalidate(); }
    if (name === 'duplicateFigure') return editor.object;
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
        let answer;
        try { answer = JSON.parse(live.call(name, payload, before)); }
        catch (error) { report.mismatches.push({call: name, problems: [`the engine failed: ${error?.message ?? error}`]}); session = null; answer = null; }
        report.calls++;
        const why = answer?.status === 'ok' ? unadoptable(answer) : null;
        if (answer?.status === 'ok' && !why) {
          depth++;
          const adopting = performance.now();
          try { return adopt(name, answer, selection); }
          finally {
            depth--; report.owned++; report.ownedMs += performance.now() - called;
            const entry = (report.byCall ??= {})[name] ??= {calls: 0, engineMs: 0, adoptMs: 0};
            entry.calls++; entry.engineMs += adopting - called; entry.adoptMs += performance.now() - adopting;
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
        const entry = (report.byCall ??= {})[name] ??= {calls: 0, editorMs: 0};
        entry.calls++; entry.editorMs += performance.now() - started;
      }
      if (live && typeof result?.then === 'function') { end('an asynchronous call'); live = null; }
      if (live) {
        try {
          const called = performance.now(), answer = JSON.parse(live.call(name, payload, before));
          report.engineMs += performance.now() - called; report.calls++;
          if (answer.status === 'unported') { const key = `${name}: ${answer.reason}`; report.unported[key] = (report.unported[key] ?? 0) + 1; end('a call the engine has not ported'); }
          else compare(name, args, answer, result, thrown, captured, sheetBefore);
        } catch (error) { report.mismatches.push({call: name, problems: [`the engine failed: ${error?.message ?? error}`]}); session = null; }
      }
      if (thrown) throw thrown;
      return result;
    };
  }
  return report;
}
