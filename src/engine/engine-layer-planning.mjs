import {applyDocumentChunk} from '../document-transport.mjs';

const cancelled = () => new DOMException('Layer migration was cancelled.', 'AbortError');
const stale = () => new Error('The document changed while layer migration was prepared.');
const pause = () => new Promise(resolve => setTimeout(resolve, 0));

/** Read-only native or sheet plan from the acknowledged engine mirror. The caller waits
 * for its mirror ACK before passing revision; the editor stages/commits the plan.
 * This adapter is intentionally not connected to any document class gate yet. */
export async function planEngineLayerMigration(engine, revision, request, {signal, onProgress, isCurrent = () => true} = {}) {
  if (!engine?.threaded || !engine.workers) throw new Error('Layer migration requires background engine workers.');
  if (typeof engine.handle?.plan_layers !== 'function') throw new Error('The engine layer planning protocol is unavailable.');
  const check = () => { if (signal?.aborted) throw cancelled(); if (!isCurrent()) throw stale(); };
  check();
  // This contains IDs only. The engine already owns the acknowledged document.
  const {uids, targetLayerId, nextFrameIdentity, deletedLayerId} = request;
  if (!Array.isArray(uids) || uids.length > 10_000 || uids.some(uid => !Number.isInteger(uid) || uid < 1 || uid > 0xffffffff) ||
      typeof targetLayerId !== 'string' || targetLayerId.length > 128 ||
      deletedLayerId !== undefined && (typeof deletedLayerId !== 'string' || deletedLayerId.length > 128) ||
      !Number.isSafeInteger(nextFrameIdentity) || nextFrameIdentity < 1)
    throw new Error('Invalid layer planning ID request.');
  const params = {uids, targetLayerId, nextFrameIdentity, ...(deletedLayerId === undefined ? {} : {deletedLayerId})};
  const job = engine.handle.plan_layers(revision, JSON.stringify(params));
  const cancel = () => job.cancel();
  signal?.addEventListener('abort', cancel, {once: true});
  let payload;
  try {
    for (;;) {
      check();
      const seen = engine.progressSeen(), packet = job.take();
      if (packet === undefined || packet === null) {
        await Promise.race([engine.whenProgress(seen), new Promise(resolve => setTimeout(resolve, 32))]);
        continue;
      }
      if (packet.length > 64 * 1024 + 2048) throw new Error('The engine layer planning packet exceeds its budget.');
      const result = JSON.parse(packet);
      if (result.progress) onProgress?.(result.progress);
      check();
      if (result.chunk) {
        if (!Number.isInteger(result.sequence) || result.sequence < 0 || result.sequence > 0xffffffff)
          throw new Error('The engine returned an invalid layer planning packet sequence.');
        payload = applyDocumentChunk(payload, result.chunk);
        // Yield even when a fast worker is ready. Each UI task consumes one
        // bounded packet, including a dense single figure or channel.
        await pause(); check();
        while (!job.ack(result.sequence)) { const before = engine.progressSeen(); await engine.whenProgress(before); check(); }
      }
      if (result.done) {
        if (!payload || !Object.hasOwn(payload, 'plan')) throw new Error('The engine returned an invalid layer planning payload.');
        check(); return payload.plan;
      }
    }
  } finally {
    signal?.removeEventListener('abort', cancel);
    job.cancel(); job.free();
  }
}
