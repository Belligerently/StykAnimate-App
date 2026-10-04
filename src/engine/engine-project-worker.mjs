import {applyDocumentChunk} from '../document-transport.mjs';

const MAX_BYTES = 65536, MAX_UNITS = Math.floor(MAX_BYTES / 3);
const keys = new Set(['fps', 'canvas_width', 'canvas_height']);
const scalar = value => value === null || typeof value === 'number' && Number.isFinite(value) || typeof value === 'boolean';
const same = (a, b) => a.length === b.length && a.every((value, index) => value === b[index]);
const aborted = signal => { if (signal?.aborted) throw signal.reason ?? new DOMException('Editing cancelled.', 'AbortError'); };
const readHeader = worker => { const text = worker.poll();return text === undefined ? null : JSON.parse(text); };

/** An explicit async route for project controls after the host has transferred
 * its sole writer. Existing Editor methods retain their synchronous contract.
 * The host keeps dependent queries/gestures behind `pending`, freezes view/
 * document disposal until readiness or cancellation, and publishes only small
 * scalar state/history in `publish`. It never rebuilds JS command history.
 * This primitive is not itself an app/class gate.
 */
export function projectWorker(side, authority, {report, epochs, pending, publish}) {
  const {engine, mirror} = side, {worker} = authority;
  let stamp = authority.stamp, busy = false, closed = false;
  if (!engine.threaded || typeof epochs !== 'function' || typeof pending !== 'function' || typeof publish !== 'function')
    throw new Error('The project worker requires its canonical threaded host and publication barrier.');
  const current = expected => {
    if (closed || !same(expected, epochs()) || !mirror.framesCurrent(report, stamp[2]))
      throw new Error('The project control belongs to a stale editing view.');
    const identity = engine.handle.try_record_identity();
    if (identity === undefined)
      throw Object.assign(new Error('The canonical project control is busy; retry the request.'), {code: 'engine_busy'});
    if (!same(stamp.slice(0, 3), identity)) throw new Error('The project control belongs to a stale editing view.');
  };
  const retry = async operation => {
    // A contended scalar/mailbox lock may release without a progress signal.
    // Use the existing finite progress wait owner once per attempt. It never
    // fabricates progress or leaves abandoned Promise/timer continuations.
    for (;;) { const seen = engine.progressSeen();if (operation()) return;await engine.whenProgress(seen, 16); }
  };
  const terminal = async request => {
    await retry(() => worker.terminal(request));
    for (;;) {
      const seen = engine.progressSeen(), header = readHeader(worker);
      if (header?.phase === 'idle' || header?.phase === 'closed') return;
      await engine.whenProgress(seen, 16);
    }
  };
  const retire = async () => {
    closed = true;worker.close();
    for (;;) {
      const seen = engine.progressSeen(), header = readHeader(worker);
      if (header?.packetSequence !== undefined) worker.packet();
      if (header?.phase === 'closed') return;
      await engine.whenProgress(seen, 16);
    }
  };
  async function request(method, args, signal) {
    if (busy || closed) throw new Error('The canonical project control is busy or closed.');
    // `args` is created below from <=3 admitted scalar entries. No arbitrary
    // caller object, getter, toJSON, history or report walk reaches stringify.
    const text = JSON.stringify(args);
    if (text.length > MAX_UNITS) throw new Error('The project control exceeds its input budget.');
    aborted(signal);
    const actual = epochs();
    if (!Array.isArray(actual) || actual.length !== 3 || !actual.every(value => Number.isSafeInteger(value) && value >= 0))
      throw new Error('The actual project-control view epochs are invalid.');
    const expected = actual.slice();current(expected);
    busy = true;
    let id, staged = {}, success = false, completed = false, readySent = false;
    try {
      pending(true);
      id = worker.submit_project_fields(new Float64Array([...stamp, ...expected]), method, text, 'null');
      let sequence = 0;
      for (;;) {
        const seen = engine.progressSeen(), header = readHeader(worker);
        if (header && header.request !== id && header.phase !== 'idle') throw new Error('The project response belongs to another request.');
        if (header?.packetSequence !== undefined) {
          if (header.packetSequence !== sequence++) throw new Error('The project packet arrived out of order.');
          const packet = worker.packet();
          if (typeof packet !== 'string' || packet.length > MAX_BYTES) throw new Error('The project packet exceeds its byte budget.');
          staged = applyDocumentChunk(staged, JSON.parse(packet));
          await retry(() => worker.ack_packet(id, header.packetSequence));
        } else if (header?.phase === 'ready' && !readySent) {
          aborted(signal);current(expected);
          const fields = staged.fields;
          if (staged.kind !== 'project-fields' || !Array.isArray(fields) || fields.length > 3 || !staged.answer || !staged.history)
            throw new Error('The project publication is outside its bounded response contract.');
          for (const field of fields) {
            if (!keys.has(field.key) && field.key !== 'frame_delay_ms' || !Object.hasOwn(report, field.key)
              || !Object.hasOwn(field, 'value') || !scalar(field.value))
              throw new Error('The project field requires its recorded absent-key handoff.');
          }
          await retry(() => {
            // Mailbox Busy can yield before this readiness admission. Recheck
            // cancellation/view ownership at every attempt, before ACKing.
            aborted(signal);current(expected);
            return worker.ready(id, new Float64Array(expected));
          });
          readySent = true;
          // The host barrier keeps the old shown head and selection visible
          // until a tagged final success. Cancellation now may race that final
          // commit; a success must be accepted, never rolled back/reexecuted.
        } else if (header?.phase === 'success') {
          if (!same(header.epochs, expected) || header.stamp[1] !== stamp[1] || header.stamp[3] !== id || header.stamp[3] <= stamp[3]
            || header.stamp[2] !== staged.history.revision) throw new Error('The project success belongs to another canonical head.');
          success = true;
          const before = stamp;
          // Final publication already happened exactly once. A busy scalar
          // identity read delays only its invisible host acceptance; the old
          // JS root stays unchanged while the dependent-view barrier is held.
          await retry(() => {
            const status = readHeader(worker);
            if (status?.phase === 'closed' || status?.phase === 'failed')
              throw new Error(status.error ?? 'The committed project worker was closed before host acceptance.');
            if (status && status.request !== id)
              throw new Error('The committed project response belongs to another request.');
            return mirror.accept_session_head(report, before, header.stamp);
          });
          for (const field of staged.fields) report[field.key] = field.value;
          stamp = header.stamp;authority.stamp = stamp;
          publish({report, answer: staged.answer, history: staged.history, stamp, epochs: expected});
          await terminal(id);
          completed = true;
          if (staged.answer.status !== 'ok') throw new Error(staged.answer.error ?? staged.answer.reason ?? 'The project control was refused.');
          return staged.answer.result;
        } else if (header?.phase === 'failed' || header?.phase === 'closed') {
          if (header.phase === 'failed') await terminal(id);
          aborted(signal);
          throw new Error(header.error ?? 'The project worker was closed.');
        }
        if (signal?.aborted) worker.cancel(id);
        await engine.whenProgress(seen, 16);
      }
    } catch (error) {
      if (id !== undefined && !success) {
        worker.cancel(id);
        for (;;) {
          const seen = engine.progressSeen(), header = readHeader(worker);
          if (header?.phase === 'failed') { await terminal(id);break; }
          if (header?.phase === 'idle' || header?.phase === 'closed') break;
          if (header?.phase === 'success') {
            // A transport/host failure after final commit is unrecoverable by
            // foreign JS. Retire this document's canonical owner on worker.
            await retire();break;
          }
          if (header?.packetSequence !== undefined) worker.packet();
          await engine.whenProgress(seen, 16);
        }
      } else if (success && !completed) {
        // A throwing host publication callback cannot restore an older owner.
        // The document must close its worker; no fallback/shadow rerun exists.
        await retire();
      }
      throw error;
    } finally { busy = false;pending(false); }
  }
  return {
    get busy() { return busy; },
    setFPS(value, options = {}) { if (typeof value !== 'number' || !Number.isFinite(value)) throw new Error('FPS requires a finite numeric control.');return request('setFPS', [value], options.signal); },
    setCanvasSize(width, height, options = {}) {
      if (typeof width !== 'number' || typeof height !== 'number' || !Number.isFinite(width) || !Number.isFinite(height)) throw new Error('Canvas size requires finite numeric controls.');
      return request('setCanvasSize', [width, height], options.signal);
    },
    updateProject(entries, options = {}) {
      if (!Array.isArray(entries) || entries.length > 3) throw new Error('Project settings exceed their control budget.');
      const patch = Object.create(null);
      for (const entry of entries) {
        if (!Array.isArray(entry) || entry.length !== 2 || typeof entry[0] !== 'string' || entry[0].length > 256
          || Object.hasOwn(patch, entry[0]) || !scalar(entry[1]))
          throw new Error('Project settings require at most three unique scalar controls.');
        patch[entry[0]] = entry[1];
      }
      return request('updateProject', [patch], options.signal);
    },
    undo(options = {}) { return request('undo', [], options.signal); },
    redo(options = {}) { return request('redo', [], options.signal); },
  };
}
