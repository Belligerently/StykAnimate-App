/**
 * Playback preparation for the app: the engine's, except while the engine has failed or
 * a drawing stroke waits for publication, when the JavaScript playback cache's
 * (docs/RUST-ENGINE-PORT.md, S3).
 *
 * `createStagePlayback` wraps the JavaScript `PlaybackCache` and keeps its
 * interface, so the app's transport does not change. While the engine prepares
 * playback, the document goes to the engine through the mirror instead of being
 * copied into playback workers, ticks are requested from the engine's worker
 * threads, and a "packet" is only a ticket: the tick, the frame's camera and
 * backgrounds for the overlay, and a key the stage renderer recognises.
 */

/** Keys of engine packets: the stage renderer draws these with the engine. */
export const ENGINE_KEY = 'engine:';
/** Generation, settings/resource epoch and the exact sample share one native owner. */
export function engineTicket(key) {
  if (typeof key !== 'string' || key.length > 96 || !key.startsWith(ENGINE_KEY)) return null;
  const parts = key.split(':');
  if (parts.length !== 4) return null;
  const [generation, epoch, tick] = parts.slice(1).map(Number);
  if (!Number.isSafeInteger(generation) || generation <= 0 || !Number.isSafeInteger(epoch) || epoch < 0 || !Number.isFinite(tick) || tick < 0) return null;
  if (parts[1] !== String(generation) || parts[2] !== String(epoch) || parts[3] !== String(tick)) return null;
  return {generation, epoch, tick};
}
export const engineTick = key => engineTicket(key)?.tick ?? null;
// These are metadata tickets, not prepared frames. Native residency remains
// governed by its existing byte budget; a discarded ticket can be recreated.
const MAX_ENGINE_TICKETS = 120;
const sample = tick => Number.isFinite(tick) && tick >= 0;

/**
 * @param {object} legacy the JavaScript PlaybackCache
 * @param {{stage(): object|null}} options `stage()` returns the stage renderer once it exists
 */
export function createStagePlayback(legacy, {stage}) {
  const side = () => stage()?.engineSide ?? null;
  /** Whether the engine prepares playback for the loaded document. */
  let engineMode = false;
  let report = null, revision = -1, hiddenKey = null, syncing = null, syncError = null;
  const packets = new Map();
  const configured = Number(legacy.maxFrames);
  const ticketLimit = Number.isFinite(configured) ? Math.max(1, Math.min(MAX_ENGINE_TICKETS, Math.floor(configured))) : MAX_ENGINE_TICKETS;
  let requested = [], packetGeneration = null;
  // Latest admission requests only. Busy never certifies the old native view.
  let pendingHidden = null, pendingEffects = null, pendingRequest = null;
  let controlOrder = 0, controlLifetime = 0, retryScheduled = false;
  const controlsPending = () => pendingHidden !== null || pendingEffects !== null || pendingRequest !== null;
  const retryControls = (engine, seen) => {
    if (retryScheduled || !controlsPending()) return;
    if (!engine) return;
    retryScheduled = true;
    const lifetime = controlLifetime;
    // The same bounded progress owner retries even when an idle lock released
    // without publishing progress. At most one waiter is outstanding.
    const waiting = engine.whenProgress?.(seen, 16) ?? Promise.resolve();
    waiting.then(() => setTimeout(() => {
      if (lifetime !== controlLifetime) return;
      retryScheduled = false;
      flushControls();
    }, 0));
  };
  const flushControls = () => {
    if (!controlsPending()) return true;
    const engine = side()?.engine, handle = engine?.handle;
    if (!handle) return false;
    // Read before testing admission, matching the existing progress contract.
    const seen = engine.progressSeen?.();
    for (const kind of ['hidden', 'effects']) {
      const pending = kind === 'hidden' ? pendingHidden : pendingEffects;
      if (!pending) continue;
      const admission = kind === 'hidden' ? handle.try_set_hidden(pending.uids) : handle.try_set_effects(pending.enabled);
      if (admission === 0) { retryControls(engine, seen); return false; }
      // An actually changed setting resets older demand; a no-op does not.
      if (admission === 2 && pendingRequest && pendingRequest.order < pending.order) pendingRequest = null;
      if (kind === 'hidden') { hiddenKey = pending.key; pendingHidden = null; } else pendingEffects = null;
    }
    if (pendingRequest) {
      const admission = handle.try_request_samples(pendingRequest.playhead, pendingRequest.samples);
      if (admission === 0) { retryControls(engine, seen); return false; }
      pendingRequest = null;
      side()?.engine.pump();
    }
    return true;
  };
  const stats = {};
  const dropPackets = () => { packets.clear(); requested = []; packetGeneration = null; };
  const prunePackets = handle => {
    const generation = handle.generation();
    if (packetGeneration !== generation) { packets.clear(); packetGeneration = generation; }
    for (const [tick, packet] of packets) {
      const ticket = engineTicket(packet.key);
      if (!ticket || !handle.frame_ticket_ready(ticket.generation, ticket.epoch, ticket.tick)) packets.delete(tick);
    }
  };
  const rank = tick => { const at = requested.indexOf(tick); return at < 0 ? ticketLimit : at; };
  const retainPacket = (tick, packet) => {
    if (packets.size >= ticketLimit) {
      let victim, farthest = -1;
      for (const candidate of packets.keys()) {
        // Preserve the current request's first presentation while checking ahead.
        if (candidate === requested[0]) continue;
        const priority = rank(candidate);
        if (priority > farthest) { victim = candidate; farthest = priority; }
      }
      if (victim === undefined || farthest < rank(tick)) return;
      packets.delete(victim);
    }
    packets.set(tick, packet);
  };

  const eligible = candidate => {
    const engine = side();
    if (!engine) return false;
    // The stage renderer asks whether its prepared frames come from the engine.
    engine.playbackEngine ??= () => engineMode;
    return !engine.failed && !engine.drawingUnpublished(candidate);
  };
  const setHidden = options => {
    if (options?.hiddenKey === undefined || (!pendingHidden && options.hiddenKey === hiddenKey)) return;
    pendingHidden = {key: options.hiddenKey, uids: new Uint32Array(options.hiddenUids ?? []), order: ++controlOrder};
    dropPackets();
    flushControls();
  };
  const sync = dirty => {
    const engine = side(), target = report, targetRevision = revision;
    // Publication clears native demand; an unaccepted request for its previous
    // document must not be retried after that publication either.
    pendingRequest = null;
    syncError = null; dropPackets();
    const run = engine.mirror.syncFrames(target, targetRevision, dirty).catch(error => {
      if (error?.name !== 'AbortError') { syncError = error; engine.failed = `the engine refused the document (${error.message})`; }
    }).finally(() => { if (syncing === run) syncing = null; });
    syncing = run;
    return run;
  };
  const mirrored = () => engineMode && !syncing && !syncError && side().mirror.framesCurrent(report, revision);
  const ready = () => !controlsPending() && mirrored();
  const packetFor = tick => {
    flushControls();
    if (!sample(tick) || !ready()) return null;
    tick = tick === 0 ? 0 : tick;
    const handle = side().engine.handle;
    prunePackets(handle);
    let packet = packets.get(tick);
    if (!packet) {
      const meta = handle.frame_meta(tick);
      if (meta === undefined) return null;
      const frame = JSON.parse(meta), key = `${ENGINE_KEY}${frame.generation}:${frame.epoch}:${tick}`;
      const ticket = engineTicket(key);
      if (!ticket || frame.tick !== tick || !handle.frame_ticket_ready(ticket.generation, ticket.epoch, ticket.tick)) return null;
      packet = {tick, key, engine: true, cameraEnabled: true, frame: {...frame, engine: true},
        byteLength: 0, compileMs: 0, hiddenKey: hiddenKey ?? '', softwareFrames: true, previewQuality: {reduced: false}, compatibility: [], groups: []};
      retainPacket(tick, packet);
    } else {
      // Existing Map order supplies recency after the bounded request priority.
      packets.delete(tick); packets.set(tick, packet);
    }
    return packet;
  };

  const playback = {
    get engineMode() { return engineMode; },
    get report() { return engineMode ? report : legacy.report; },
    get documentRevision() { return engineMode ? revision : legacy.documentRevision; },
    set documentRevision(value) { if (engineMode) revision = value; else legacy.documentRevision = value; },
    get cache() { return engineMode ? packets : legacy.cache; },
    get stats() {
      if (!engineMode) return legacy.stats;
      const frames = JSON.parse(side().engine.handle.stats());
      return Object.assign(stats, {preparedFrames: frames.prepared, compileMs: frames.totalMs, lastCompileMs: frames.prepared ? frames.totalMs / frames.prepared : 0,
        pendingFrames: 0, bytes: frames.cachedBytes, readyFrames: frames.cached, hits: 0, misses: 0, engine: frames, mirror: {...side().mirror.stats}});
    },
    load(next, options = {}) {
      next = next?.document ?? next?.report ?? next;
      if (!eligible(next)) {
        engineMode = false; report = null; dropPackets();
        return legacy.load(next, options);
      }
      // The document goes to the engine once; no worker gets a copy of it.
      if (!engineMode && legacy.report) legacy.invalidate();
      engineMode = true; report = next; revision = options.revision;
      setHidden(options);
      sync({});
      return playback;
    },
    updateDocument(patch = {}) {
      const nextRevision = patch.revision ?? revision;
      if (!engineMode) return legacy.updateDocument(patch);
      if (!eligible(report)) { engineMode = false; dropPackets(); return legacy.load(report, patch); }
      revision = nextRevision;
      setHidden(patch);
      sync({frameIndices: patch.frameIndices, sourceIds: patch.sourceIds, figureIndices: patch.figureIndices,
        allFigures: Boolean(patch.definition && !patch.figureIndices?.length)});
      return playback;
    },
    updateView(options = {}) {
      if (!engineMode) return legacy.updateView(options);
      // The engine's frames do not depend on the view or the camera mode; only on what is hidden.
      setHidden(options);
      return playback;
    },
    invalidate(options) {
      if (!engineMode) return legacy.invalidate(options);
      pendingRequest = {playhead: 0, samples: new Float64Array(0), order: ++controlOrder};
      flushControls();
      dropPackets();
      return playback;
    },
    async prepareRange(ticks, options) {
      if (!engineMode) return legacy.prepareRange(ticks, options);
      if (syncing) await syncing;
      if (syncError) throw syncError;
      // A busy earlier admission stays cold, but cannot suppress newer demand.
      if (!mirrored() || !ticks.length || !sample(ticks[0])) return;
      const engine = side().engine;
      if (sample(ticks[0])) {
        requested = [];
        for (let index = 0; index < Math.min(ticks.length, ticketLimit); index++) {
          const tick = ticks[index];
          if (sample(tick) && !requested.includes(tick)) requested.push(tick === 0 ? 0 : tick);
        }
      }
      prunePackets(engine.handle);
      pendingRequest = {playhead: ticks[0], samples: new Float64Array(ticks), order: ++controlOrder};
      flushControls();
    },
    /**
     * For an export (engine-export.mjs): brings the engine's document to `report` at
     * `revision` through the same synchronisation playback uses, which the app keeps
     * current with every edit afterwards. Resolves `true` once the engine holds it,
     * `false` when the engine cannot take it (it has failed, or a stroke awaits publication).
     */
    async exportDocument(next, nextRevision) {
      if (!eligible(next)) return false;
      if (!engineMode || report !== next) playback.load(next, {revision: nextRevision});
      else if (revision !== nextRevision || (!syncing && !side().mirror.framesCurrent(report, revision))) playback.updateDocument({revision: nextRevision});
      if (!engineMode) return false;
      while (syncing) await syncing;
      if (syncError) throw syncError;
      return side().mirror.framesCurrent(next, nextRevision);
    },
    get(tick) { return engineMode ? packetFor(tick) : legacy.get(tick); },
    has(tick) { return engineMode ? packetFor(tick) !== null : legacy.has(tick); },
    bufferedAhead(tick, count, end) {
      if (!engineMode) return legacy.bufferedAhead(tick, count, end);
      const total = side().engine.handle.total_ticks(), last = Math.min(tick + count, end ?? total, total);
      if (total === 0) return false; // No document or a busy scalar header is cold.
      for (let next = tick; next < last; next++) if (packetFor(next) === null) return false;
      return packetFor(tick) !== null;
    },
    setInteractionActive(active) { return legacy.setInteractionActive(active); },
    setVfxEnabled(enabled) {
      // The engine's prepared frames are kept with or without effects, as the editor's preview shows them.
      const engine = side();
      if (engine) {
        pendingEffects = {enabled: enabled !== false, order: ++controlOrder};
        if (engineMode) dropPackets();
        flushControls();
      }
      return legacy.setVfxEnabled(enabled);
    },
    stageNativeSnapshot(...args) { return engineMode ? false : legacy.stageNativeSnapshot(...args); },
    discardNativeSnapshot() { return legacy.discardNativeSnapshot(); },
    // Planning jobs still run on the JavaScript mirror: load it only when one is asked for.
    planLayerMigration(request, options) { return planner().planLayerMigration(request, options); },
    planInsertInbetweens(request, options) { return planner().planInsertInbetweens(request, options); },
    dispose() {
      pendingHidden = pendingEffects = pendingRequest = null;
      controlLifetime++; retryScheduled = false;
      dropPackets(); return legacy.dispose();
    },
  };
  const planner = () => {
    if (engineMode && (legacy.report !== report || legacy.documentRevision !== revision)) legacy.load(report, {revision});
    return legacy;
  };
  return new Proxy(playback, {
    get(target, property) {
      if (property in target) return Reflect.get(target, property);
      const value = legacy[property];
      return typeof value === 'function' ? value.bind(legacy) : value;
    },
    set(target, property, value) {
      if (property in target) target[property] = value; else legacy[property] = value;
      return true;
    },
    has: (target, property) => property in target || property in legacy,
  });
}
