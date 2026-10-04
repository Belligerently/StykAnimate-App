/**
 * Playback preparation for the app: the engine's for documents it draws, the
 * JavaScript playback cache's for the others (docs/RUST-ENGINE-PORT.md, S3).
 *
 * `createStagePlayback` wraps the JavaScript `PlaybackCache` and keeps its
 * interface, so the app's transport does not change. For a document the engine
 * draws, the document goes to the engine through the mirror instead of being
 * copied into playback workers, ticks are requested from the engine's worker
 * threads, and a "packet" is only a ticket: the tick, the frame's camera and
 * backgrounds for the overlay, and a key the stage renderer recognises.
 */

/** Keys of engine packets: the stage renderer draws these with the engine. */
export const ENGINE_KEY = 'engine:';
export const engineTick = key => typeof key === 'string' && key.startsWith(ENGINE_KEY) ? Number(key.slice(key.lastIndexOf(':') + 1)) : null;
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
  const stats = {};
  const dropPackets = () => { packets.clear(); requested = []; packetGeneration = null; };
  const prunePackets = handle => {
    const generation = handle.generation();
    if (packetGeneration !== generation) { packets.clear(); packetGeneration = generation; }
    for (const tick of packets.keys()) if (!handle.frame_ready(tick)) packets.delete(tick);
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

  const eligible = (candidate, candidateRevision) => {
    const engine = side();
    if (!engine) return false;
    // The stage renderer asks whether its prepared frames come from the engine.
    engine.playbackEngine ??= () => engineMode;
    return !engine.failed && !engine.drawingUnpublished(candidate) && engine.eligibility.check(candidate, candidateRevision, engine.dirty) === null;
  };
  const setHidden = options => {
    if (options?.hiddenKey === undefined || options.hiddenKey === hiddenKey) return;
    hiddenKey = options.hiddenKey;
    side().engine.handle.set_hidden(new Uint32Array(options.hiddenUids ?? []));
    dropPackets();
  };
  const sync = dirty => {
    const engine = side(), target = report, targetRevision = revision;
    syncError = null; dropPackets();
    const run = engine.mirror.syncFrames(target, targetRevision, dirty).catch(error => {
      if (error?.name !== 'AbortError') { syncError = error; engine.failed = `the engine refused the document (${error.message})`; }
    }).finally(() => { if (syncing === run) syncing = null; });
    syncing = run;
    return run;
  };
  const ready = () => engineMode && !syncing && !syncError && side().mirror.framesCurrent(report, revision);
  const packetFor = tick => {
    if (!sample(tick) || !ready()) return null;
    tick = tick === 0 ? 0 : tick;
    const handle = side().engine.handle;
    prunePackets(handle);
    if (!handle.frame_ready(tick)) return null;
    let packet = packets.get(tick);
    if (!packet) {
      const meta = handle.frame_meta(tick);
      if (meta === undefined) return null;
      packet = {tick, key: `${ENGINE_KEY}${handle.generation()}:${tick}`, engine: true, cameraEnabled: true, frame: {...JSON.parse(meta), engine: true},
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
      if (!eligible(next, options.revision)) {
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
      if (!eligible(report, nextRevision)) { engineMode = false; dropPackets(); return legacy.load(report, patch); }
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
      side().engine.handle.request_samples(0, new Float64Array(0));
      dropPackets();
      return playback;
    },
    async prepareRange(ticks, options) {
      if (!engineMode) return legacy.prepareRange(ticks, options);
      if (syncing) await syncing;
      if (syncError) throw syncError;
      if (!ready() || !ticks.length) return;
      const engine = side().engine;
      if (sample(ticks[0])) {
        requested = [];
        for (let index = 0; index < Math.min(ticks.length, ticketLimit); index++) {
          const tick = ticks[index];
          if (sample(tick) && !requested.includes(tick)) requested.push(tick === 0 ? 0 : tick);
        }
      }
      prunePackets(engine.handle);
      engine.handle.request_samples(ticks[0], new Float64Array(ticks));
      engine.pump();
    },
    /**
     * For an export (engine-export.mjs): brings the engine's document to `report` at
     * `revision` through the same synchronisation playback uses, which the app keeps
     * current with every edit afterwards. Resolves `true` once the engine holds it,
     * `false` when the engine does not draw this document.
     */
    async exportDocument(next, nextRevision) {
      if (!eligible(next, nextRevision)) return false;
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
      for (let next = tick; next < last; next++) if (packetFor(next) === null) return false;
      return packetFor(tick) !== null;
    },
    setInteractionActive(active) { return legacy.setInteractionActive(active); },
    setVfxEnabled(enabled) {
      // The engine's prepared frames are kept with or without effects, as the editor's preview shows them.
      const engine = side();
      if (engine) { engine.engine.handle.set_effects(enabled !== false); if (engineMode) dropPackets(); }
      return legacy.setVfxEnabled(enabled);
    },
    stageNativeSnapshot(...args) { return engineMode ? false : legacy.stageNativeSnapshot(...args); },
    discardNativeSnapshot() { return legacy.discardNativeSnapshot(); },
    // Planning jobs still run on the JavaScript mirror: load it only when one is asked for.
    planLayerMigration(request, options) { return planner().planLayerMigration(request, options); },
    planInsertInbetweens(request, options) { return planner().planInsertInbetweens(request, options); },
    dispose() { dropPackets(); return legacy.dispose(); },
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
