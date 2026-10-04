/** The fresh handoff boundary for the single canonical Rust Session worker.
 * This module never changes an Editor method's synchronous return contract.
 * Activation requires the host's explicit writer-transfer transaction.
 */
const MAX_STATE_UNITS = Math.floor(65536 / 3), MAX_UIDS = 1024;
const counter = value => Number.isSafeInteger(value) && value >= 0;
const boundedId = value => typeof value === 'string' && value.length <= 256;
const sameArray = (a, b) => a.length === b.length && a.every((value, index) => value === b[index]);
const aborted = signal => { if (signal?.aborted) throw signal.reason ?? new DOMException('Editing handoff cancelled.', 'AbortError'); };

/** Actual scalar history evidence is checked before any selected-ID iteration,
 * report walk or serialization. Foreign warm histories retain their owner.
 */
function snapshot(editor) {
  if (editor._undo.length || editor._redo.length || editor.historySize !== 0)
    throw new Error('A warm JavaScript history must keep its existing owner.');
  if (editor._drag || editor._authoredPoseEdit || editor._animationEdit || editor._vfxClipEdit || editor._movieclipBatch)
    throw new Error('A live editor requires the recorded gesture worker handoff.');
  if (editor._authoredCommitPreview || editor._sheetPropertyPreview)
    throw new Error('An existing retained display requires its recorded worker handoff.');
  if (editor._movieclipScopes?.length || editor._pendingSourceBytes || editor._pendingSourceLayerChanges?.length)
    throw new Error('Existing editing scope or staged sources require their recorded worker handoff.');
  for (const key of ['_frameClipboard', '_sheetClipboard', '_figureClipboard', '_selectionClipboard', '_segmentValuesClipboard',
    '_cameraClipboard', '_cameraMotionClipboard', '_vfxClipClipboard']) if (editor[key] != null)
    throw new Error('Existing clipboards require their bounded worker handoff.');
  if (editor._polygonSelection || editor._segmentSelection || editor._branchSelection)
    throw new Error('Selection metadata requires the bounded worker selection handoff.');
  const chosen = editor.selectedUids;
  if (!(chosen instanceof Set) || chosen.size > MAX_UIDS)
    throw new Error('The selection exceeds the editing control budget.');
  const selectedUids = [...chosen];
  // selectedUid's legacy getter normalizes _selectedUid in place. Derive its
  // same observable result without modifying a failed/aborted handoff owner.
  const rawUid = editor._selectedUid;
  const selectedUid = chosen.has(rawUid) ? rawUid : selectedUids.at(-1) ?? null;
  const integers = [editor.revision, editor._revisionSerial, editor._savedRevision, editor.layerRevision,
    editor._nextFrameIdentity, editor._nextUid, editor.frameIndex, editor.historyLimit];
  if (!integers.every(counter) || editor.revision > editor._revisionSerial || editor._savedRevision > editor._revisionSerial
    || !Number.isFinite(editor.historyBytes) || editor.historyBytes < 0 || !Number.isFinite(editor.tick)
    || !selectedUids.every(uid => counter(uid) && uid > 0 && uid <= 0xffffffff)
    || !(rawUid == null || counter(rawUid) && rawUid <= 0xffffffff)
    || !Number.isSafeInteger(editor.selectedJoint) || editor.selectedJoint < -1 || editor.selectedJoint > 0xffffffff)
    throw new Error('The original editing counters or selection are invalid.');
  const activeLayerId = editor.activeLayerId, sheet = editor._sheetSelection;
  if (!boundedId(activeLayerId) || sheet && !boundedId(sheet.id)) throw new Error('The layer identity exceeds the editing control budget.');
  return {report: editor.report, revision: editor.revision, serial: editor._revisionSerial, saved: editor._savedRevision, rawUid,
    limits: [editor.historyLimit, editor.historyBytes],
    state: {tick: editor.tick, frameIndex: editor.frameIndex, selectedUid, selectedUids,
      selectedJoint: editor.selectedJoint, activeLayerId, autoKey: Boolean(editor.autoKey),
      sheetSelection: sheet ? {id: sheet.id, contents: Boolean(sheet.contents)} : null,
      layerRevision: editor.layerRevision, nextFrameIdentity: editor._nextFrameIdentity, nextUid: editor._nextUid,
      propertyChannels: Boolean(editor.propertyChannels), layerTimeline: Boolean(editor.layerTimeline)}};
}
function readEpochs(read) {
  const values = read();
  if (!Array.isArray(values) || values.length !== 3 || !values.every(counter)) throw new Error('The actual shown/view/gesture epochs are invalid.');
  return values.slice();
}
function sameSnapshot(before, after) {
  if (before.report !== after.report || before.revision !== after.revision || before.serial !== after.serial || before.saved !== after.saved || before.rawUid !== after.rawUid
    || !sameArray(before.limits, after.limits) || !sameArray(before.state.selectedUids, after.state.selectedUids)) return false;
  // Only this fixed bounded control shape is compared, never document fields.
  for (const key of ['tick', 'frameIndex', 'selectedUid', 'selectedJoint', 'activeLayerId', 'autoKey', 'layerRevision',
    'nextFrameIdentity', 'nextUid', 'propertyChannels', 'layerTimeline']) if (before.state[key] !== after.state[key]) return false;
  const a = before.state.sheetSelection, b = after.state.sheetSelection;
  return a === b || a && b && a.id === b.id && a.contents === b.contents;
}
const parseHeader = worker => {
  const text = worker.poll();
  return text === undefined ? null : JSON.parse(text);
};
async function closeOnWorker(engine, worker) {
  worker.close();
  for (;;) {
    const seen = engine.progressSeen(), header = parseHeader(worker);
    if (header?.phase === 'closed') break;
    await engine.whenProgress(seen);
  }
  worker.free();
}
const busy = () => Object.assign(new Error('The canonical editing session is busy; retry the handoff.'), {code: 'engine_busy'});
function identityNow(handle) {
  const identity = handle.try_record_identity();
  if (identity === undefined) throw busy();
  return identity;
}

/** Prepare a genuinely fresh owner from the already acknowledged canonical
 * mirror. No owning Rust Editor is constructed on this interface thread.
 * `epochs()` reads an epoch of the actual Stage shown tuple and view/resource/
 * gesture state. It must be marked stale before any such state changes.
 */
export async function prepareFreshEditWorker(side, editor, {epochs, signal} = {}) {
  if (!side.engine.threaded) throw new Error('Canonical editing requires the existing engine worker threads.');
  if (typeof epochs !== 'function') throw new Error('Canonical editing requires the actual shown/view/gesture epochs.');
  aborted(signal);
  const original = snapshot(editor);
  if (!side.mirror.framesCurrent(original.report, original.revision)) throw new Error('The canonical mirror has not acknowledged this editor revision.');
  const stamp = [...identityNow(side.engine.handle), 0], expectedEpochs = readEpochs(epochs);
  if (stamp.length !== 4 || !stamp.every(counter) || stamp[0] === 0 || stamp[1] === 0 || stamp[2] !== original.revision)
    throw new Error('The canonical mirror identity is stale.');
  // snapshot() admitted this fixed shape and at most 1024 small IDs BEFORE JSON.
  const state = JSON.stringify(original.state);
  if (state.length > MAX_STATE_UNITS) throw new Error('The editing bootstrap state exceeds its control budget.');
  const values = new Float64Array([...stamp, ...expectedEpochs, original.revision, original.serial, original.saved,
    ...original.limits, 0, 0, 0]);
  const {engine} = side, worker = engine.handle.request_edit_worker(state, values);
  if (worker === undefined) throw busy();
  let disposed = false, transferred = false;
  const dispose = async () => { if (!disposed) { disposed = true; await closeOnWorker(engine, worker); } };
  const current = () => {
    aborted(signal);
    if (!sameSnapshot(original, snapshot(editor)) || !side.mirror.framesCurrent(original.report, original.revision)
      || !sameArray(stamp.slice(0, 3), identityNow(engine.handle)) || !sameArray(expectedEpochs, readEpochs(epochs)))
      throw new Error('The original editor or actual shown head changed during handoff.');
  };
  try {
    for (;;) {
      current();
      const seen = engine.progressSeen(), header = parseHeader(worker);
      if (header?.phase === 'attached') {
        if (!sameArray(header.stamp, stamp) || !sameArray(header.epochs, expectedEpochs)) throw new Error('The prepared Session belongs to another editing owner.');
        break;
      }
      if (header?.phase === 'failed' || header?.phase === 'closed') throw new Error(header.error ?? 'The worker refused the fresh editing handoff.');
      await engine.whenProgress(seen);
    }
  } catch (error) { await dispose(); throw error; }
  return {
    stamp, epochs: expectedEpochs,
    /** The host freezes its JS writer, validates the original owner again, then
     * accepts the sole Rust authority. rollback runs only before authority is
     * returned, after worker disposal. Warm Rust ownership never falls back to JS.
     */
    async activate(freeze) {
      if (disposed || transferred) throw new Error('The prepared editing handoff was already consumed.');
      let transfer;
      try {
        current();
        if (typeof freeze !== 'function') throw new Error('Activation requires the host writer-transfer transaction.');
        transfer = freeze();
        if (typeof transfer?.accept !== 'function' || typeof transfer?.rollback !== 'function') throw new Error('The writer-transfer transaction is invalid.');
        current();
        for (;;) {
          const seen = engine.progressSeen();
          if (worker.activate(1, new Float64Array(expectedEpochs))) break;
          await engine.whenProgress(seen);
          current();
        }
        for (;;) {
          const seen = engine.progressSeen(), header = parseHeader(worker);
          if (header?.phase === 'idle') break;
          if (header?.phase === 'failed' || header?.phase === 'closed') throw new Error(header.error ?? 'The worker refused editing activation.');
          await engine.whenProgress(seen);
          current();
        }
        current();
        transfer.accept();
        transferred = true;
        // This weak native control stays attached across class eligibility
        // changes. Only document disposal may close it; JS may not take back a
        // warm history from this worker or rerun its publication callbacks.
        return {worker, stamp, epochs: expectedEpochs};
      } catch (error) {
        await dispose();
        transfer?.rollback?.();
        throw error;
      }
    },
    async cancel() {
      if (transferred) throw new Error('The canonical owner must be retained or disposed with its document.');
      await dispose();
    },
  };
}
