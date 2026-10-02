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
 * A session starts from the mirror once it holds the editor's current revision,
 * and lives while every change to the document goes through calls it shadows.
 * Anything else (an unported command, a load, an asynchronous edit) ends it.
 */

/** Calls styk-edit runs (engine/crates/styk-edit/src/lib.rs, `dispatch`). */
const PORTED = ['select', 'selectMany', 'seek', 'beginDrag', 'beginTranslationDrag', 'commitDrag', 'cancelDrag', 'moveSelection', 'rotateSelection',
  'scaleSelection', 'scaleSelected', 'mirrorSelection', 'rotateFigure', 'centerFigure', 'resetPose', 'poseJoint', 'rotateJoint', 'bendJoint',
  'sheetInsertKey', 'sheetDuplicateFrameAfter', 'sheetSetEasing', 'sheetClearKeys', 'sheetClearFrames', 'sheetInsertFrames', 'sheetRemoveFrames',
  'sheetSetDuration', 'sheetAppendFrame', 'joinTo', 'unjoinFigure', 'joinFigure', 'deleteSelection', 'deleteFigure', 'duplicateFigure', 'undo', 'redo'];
/** Calls that return a frame or a figure record: a view, not compared. */
const VIEWS = new Set(['select', 'selectMany', 'seek', 'sheetAppendFrame', 'duplicateFigure']);

export function shadowEditEnabled() {
  try {
    const query = new URLSearchParams(globalThis.location?.search ?? '').get('engineShadow');
    if (query) return query === 'on';
    return globalThis.localStorage?.getItem('stykanimate.engine.shadowEdit') === 'on';
  } catch { return false; }
}

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
export function installEditShadow(editor, engineSide) {
  const report = globalThis.__stykEditShadow = {calls: 0, compared: 0, rounded: 0, skipped: 0, sessions: 0, ended: {}, unported: {}, mismatches: [], jsMs: 0, engineMs: 0};
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

  const compare = (name, args, answer, result, thrown, captured) => {
    let rounded = false;
    const problems = [], same = (label, ours, theirs) => {
      if (ours === theirs) return;
      if (typeof ours === 'string' && typeof theirs === 'string' && close(ours, theirs)) { rounded = true; return; }
      problems.push(`${label}: engine ${String(theirs).slice(0, 300)}, editor ${String(ours).slice(0, 300)}`);
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
      same('layer sheet', JSON.stringify(styk.layerSheet), JSON.stringify(answer.sheet));
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
    if (report.mismatches.length < 50) report.mismatches.push({call: name, args: plain(args), problems});
    console.warn(`[engine shadow] ${name} differs:\n  ${problems.join('\n  ')}`);
    end('a difference');
  };

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
      depth++; events = [];
      const started = performance.now();
      let result, thrown;
      try { result = original.apply(this, args); } catch (error) { thrown = error; } finally { depth--; }
      const captured = events; events = null;
      report.jsMs += performance.now() - started;
      if (live && typeof result?.then === 'function') { end('an asynchronous call'); live = null; }
      if (live) {
        try {
          const called = performance.now(), answer = JSON.parse(live.call(name, payload, before));
          report.engineMs += performance.now() - called; report.calls++;
          if (answer.status === 'unported') { const key = `${name}: ${answer.reason}`; report.unported[key] = (report.unported[key] ?? 0) + 1; end('a call the engine has not ported'); }
          else compare(name, args, answer, result, thrown, captured);
        } catch (error) { report.mismatches.push({call: name, problems: [`the engine failed: ${error?.message ?? error}`]}); session = null; }
      }
      if (thrown) throw thrown;
      return result;
    };
  }
  return report;
}
