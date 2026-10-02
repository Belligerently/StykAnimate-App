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
  const stats = {};
  const dropPackets = () => packets.clear();

  const eligible = (candidate, candidateRevision) => {
    const engine = side();
    if (!engine) return false;
    // The stage renderer asks whether its prepared frames come from the engine.
    engine.playbackEngine ??= () => engineMode;
    return !engine.failed && engine.eligibility.check(candidate, candidateRevision, engine.dirty) === null;
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
    if (!ready() || !side().engine.handle.frame_ready(tick)) return null;
    let packet = packets.get(tick);
    if (!packet) {
      const handle = side().engine.handle, meta = handle.frame_meta(tick);
      if (meta === undefined) return null;
      packet = {tick, key: `${ENGINE_KEY}${handle.generation()}:${tick}`, engine: true, cameraEnabled: true, frame: {...JSON.parse(meta), engine: true},
        byteLength: 0, compileMs: 0, hiddenKey: hiddenKey ?? '', softwareFrames: true, previewQuality: {reduced: false}, compatibility: [], groups: []};
      packets.set(tick, packet);
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
      side().engine.handle.request(0, new Uint32Array(0));
      dropPackets();
      return playback;
    },
    async prepareRange(ticks, options) {
      if (!engineMode) return legacy.prepareRange(ticks, options);
      if (syncing) await syncing;
      if (syncError) throw syncError;
      if (!ready() || !ticks.length) return;
      const engine = side().engine;
      engine.handle.request(ticks[0], new Uint32Array(ticks));
      engine.pump();
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
