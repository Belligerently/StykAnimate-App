/**
 * The stage: one renderer object for the app, two renderers behind it while
 * the engine port is in progress (docs/RUST-ENGINE-PORT.md, stage S3).
 *
 * `createStageRenderer` wraps the JavaScript renderer. For a document the
 * engine can draw (engine-eligibility.mjs) the engine draws into its own
 * canvas, stacked exactly where the JavaScript renderer's is, and the other is
 * hidden. Everything the engine does not handle yet is forwarded to the
 * JavaScript renderer, so the app sees one renderer with one interface.
 *
 * What crosses to the engine per edited frame is the pose the editor shows:
 * only the objects whose geometry the editor rebuilt since the last frame.
 */
import {loadEngine} from './engine.mjs';
import {EngineMirror} from './engine-mirror.mjs';
import {EngineEligibility} from './engine-eligibility.mjs';
import {engineTick} from './engine-playback.mjs';

/** The engine side of the stage, shared by every renderer the app creates (it survives a device loss of the other). */
let shared = null;

/** `?engine=off` (or the stored setting) keeps the JavaScript renderer for comparisons. */
export function engineEnabled() {
  try {
    const query = new URLSearchParams(globalThis.location?.search ?? '').get('engine');
    if (query) return query !== 'off';
    return globalThis.localStorage?.getItem('stykanimate.engine') !== 'off';
  } catch { return true; }
}

async function engineSide(canvas) {
  if (shared) return shared;
  const engine = await loadEngine();
  const surface = canvas.cloneNode(false);
  surface.id = 'scene-engine';
  surface.removeAttribute('data-i18n-aria-label');
  surface.setAttribute('aria-hidden', 'true');
  surface.style.visibility = 'hidden';
  canvas.before(surface);
  const stage = await engine.createStage(surface);
  return shared = {engine, stage, surface, mirror: new EngineMirror(engine), eligibility: new EngineEligibility(),
    /** Geometry groups the engine last received, by slot: an unchanged group is not sent again. */
    groups: [], frameReport: null, count: -1, definitions: null, failed: null, dirty: null, lastRefusal: null,
    /** Onion skin poses the engine holds, by the editor's sampled frame. */
    ghosts: new WeakMap(), nextGhost: 1, frameSent: false,
    stats: {frames: 0, objectsSent: 0, lastSendMs: 0, lastDrawMs: 0, fallbacks: 0}};
}

const STRIP = new Set(['runtime_segments']);
/** A pose as JSON, without derived caches. */
const poseJSON = object => JSON.stringify(object, (key, value) => STRIP.has(key) ? undefined : value);

/**
 * Wraps the renderer `create(canvas, options)` builds.
 * @param {HTMLCanvasElement} canvas the scene canvas
 * @param {object} options the renderer options (see docs/renderer.md)
 * @param {(canvas: HTMLCanvasElement, options: object) => Promise<object>} create the JavaScript renderer's factory
 */
export async function createStageRenderer(canvas, options, create) {
  const legacy = await create(canvas, options);
  if (!engineEnabled()) return legacy;
  let side;
  try { side = await engineSide(canvas); }
  catch (error) {
    // No engine (an old browser, a failed download): the JavaScript renderer is the whole stage.
    console.warn('The engine could not start; using the JavaScript renderer.', error);
    return legacy;
  }
  if (side.surface.nextElementSibling !== canvas) canvas.before(side.surface);
  let engineShown = false;
  const show = engine => {
    if (engine === engineShown) return;
    engineShown = engine;
    side.surface.style.visibility = engine ? 'visible' : 'hidden';
    canvas.style.visibility = engine ? 'hidden' : '';
  };

  /** Why this frame goes to the JavaScript renderer, or null when the engine draws it. */
  const refusal = request => {
    if (side.failed) return side.failed;
    if (request.view?.rotation || request.view?.flip) return 'a rotated or mirrored view';
    if (!request.geometry) return 'no evaluated geometry';
    const reason = side.eligibility.check(request.report, request.revision ?? -1, side.dirty);
    if (reason && side.dirty) {
      // The JavaScript renderer takes over; which figures were edited in place meanwhile is
      // not tracked, so the engine gets every definition again when it returns.
      side.dirty = null; side.resendFigures = true; side.definitions = null;
    }
    return reason;
  };

  /** Figure definitions, backgrounds and properties: what an edit frame is drawn with. */
  const syncDefinitions = request => {
    const key = `${request.revision}:${request.artworkRevision}`;
    if (side.mirror.report === request.report && side.definitions === key) return;
    // Figures edited in place keep their objects; the editor's change events said which.
    const dirty = side.dirty;
    side.dirty = null;
    const result = side.mirror.syncDefinitions(request.report, request.revision ?? 0,
      {figureIndices: dirty?.figureIndices ?? [], allFigures: side.resendFigures || Boolean(dirty?.definition && !dirty.figureIndices?.length)});
    side.definitions = key; side.resendFigures = false;
    if (side.frameReport !== request.report) { side.groups = []; side.count = -1; side.fields = null; side.frameSent = false; side.ghosts = new WeakMap(); side.frameReport = request.report; }
    const unsupported = JSON.parse(side.engine.handle.unsupported());
    if (unsupported.length) throw new Error(`the engine does not draw ${unsupported.join(', ')} yet`);
    return result;
  };

  /**
   * A pose as the engine reads it: the frame's own fields, and per object either its
   * pose or `null` when the editor has not rebuilt its geometry since `known` last saw it.
   * `null` when nothing changed since `known` last saw the frame (its fields included:
   * a frame without objects still changes the background and camera).
   */
  const frameJSON = (frame, geometry, known) => {
    const objects = frame.objects ?? [], groups = geometry?.objects, same = known.count === objects.length && groups;
    let sent = 0;
    const items = objects.map((object, slot) => {
      const group = groups?.get(slot) ?? object;
      if (same && known.groups[slot] === group) return 'null';
      known.groups[slot] = group; sent++;
      return poseJSON(group.obj ?? object);
    });
    known.groups.length = known.count = objects.length;
    const {objects: _objects, ...header} = frame;
    const fields = JSON.stringify(header);
    if (!sent && known.fields === fields) return null;
    known.fields = fields;
    side.stats.objectsSent += sent;
    return `${fields.slice(0, -1)}${Object.keys(header).length ? ',' : ''}"objects":[${items.join(',')}]}`;
  };

  /** Sends the onion skins that changed and describes all of them for this frame. */
  const onionList = request => {
    const list = [];
    for (const onion of request.onionFrames ?? []) {
      let known = side.ghosts.get(onion.frame);
      if (!known || !side.stage.has_ghost(known.id)) side.ghosts.set(onion.frame, known = {id: known?.id ?? side.nextGhost++, groups: [], count: -1});
      const json = frameJSON(onion.frame, onion.geometry, known);
      if (json !== null) side.stage.edit_ghost(known.id, json);
      // The skin shows the objects its geometry lists (scope, hidden figures and layers are already applied to it).
      const shown = new Set(), ordered = onion.geometry.orderedObjects ?? [...onion.geometry.objects.values()];
      let tint = -1;
      for (const group of ordered) {
        shown.add(group.slot);
        if (tint < 0) { const rgb = group.drawables?.find(item => item.onionTint)?.onionTint; if (rgb) tint = rgb[0] << 16 | rgb[1] << 8 | rgb[2]; }
      }
      const hidden = [];
      for (let slot = 0; slot < (onion.frame.objects?.length ?? 0); slot++) if (!shown.has(slot)) hidden.push(slot);
      list.push(known.id, onion.opacity ?? .14, tint, onion.showOnTop ? 1 : 0, onion.beforeUid ?? -1, hidden.length, ...hidden);
    }
    return list;
  };

  const drawEdit = request => {
    const started = performance.now();
    syncDefinitions(request);
    const {frame, geometry} = request, objects = frame.objects ?? [];
    const json = frameJSON(frame, geometry, side);
    // An unchanged frame is not parsed again; the first frame and every changed one are.
    if (json !== null) { side.stage.edit_frame(json); side.frameSent = true; }
    const onions = onionList(request);
    const sendMs = performance.now() - started;
    const hidden = [], translations = [];
    objects.forEach((object, slot) => {
      const uid = object.unique_id;
      if (request.hiddenUids?.has(uid)) hidden.push(slot);
      const move = request.dragTranslations?.get?.(uid);
      if (move && (move.x || move.y)) translations.push(slot, move.x, move.y);
    });
    const dpr = legacy.dpr || 1, view = request.view ?? {};
    const drawn = side.stage.render_edit((view.scale ?? 1) * dpr, (view.x ?? 0) * dpr, (view.y ?? 0) * dpr, request.cameraEnabled !== false,
      request.clipToCanvas !== false, new Uint32Array(hidden), new Float64Array(translations),
      Number.isFinite(request.editorBackgroundColor) ? request.editorBackgroundColor >>> 0 : -1, new Float64Array(onions));
    Object.assign(side.stats, {frames: side.stats.frames + 1, lastSendMs: sendMs, lastDrawMs: performance.now() - started - sendMs});
    return drawn;
  };

  const renderer = {
    /** Which renderer drew the last frame, for tests and reports. */
    get engineActive() { return engineShown; },
    get engineStats() { return {...side.stats, mirror: {...side.mirror.stats}, refusal: side.lastRefusal ?? null, backend: side.stage.backend()}; },
    get canvas() { return engineShown ? side.surface : canvas; },
    render(request) {
      const reason = refusal(request);
      side.lastRefusal = reason;
      if (!reason) {
        try {
          if (drawEdit(request)) { show(true); renderer.lastGeometry = request.geometry; return true; }
        } catch (error) {
          // The engine could not take this frame: the JavaScript renderer draws it, and the gate says why.
          side.stats.fallbacks++;
          side.lastRefusal = String(error?.message ?? error);
          side.definitions = null;
        }
      }
      const result = legacy.render(request);
      show(false);
      return result;
    },
    // Prepared playback frames: an engine packet is a ticket for a tick the engine has ready.
    prepareFrame(packet, options) { return packet?.engine ? packet.key : legacy.prepareFrame(packet, options); },
    hasPreparedFrame(key) { return engineTick(key) !== null ? true : legacy.hasPreparedFrame(key); },
    canWarm: (...args) => legacy.canWarm?.(...args),
    renderPrepared(key, options = {}) {
      const tick = engineTick(key);
      if (tick === null) { const result = legacy.renderPrepared(key, options); if (result) show(false); return result; }
      const dpr = legacy.dpr || 1, view = options.view ?? {};
      const drawn = side.stage.render(tick, (view.scale ?? 1) * dpr, (view.x ?? 0) * dpr, (view.y ?? 0) * dpr, options.cameraEnabled !== false, true, false);
      if (drawn) show(true);
      return drawn;
    },
    // The engine decodes and uploads its own pictures; a frame it prepared never waits for the other renderer's.
    backgroundsReady(report, frame) { return frame?.engine ? true : legacy.backgroundsReady?.(report, frame) ?? true; },
    prepareBackgrounds(report, frame) { return frame?.engine ? Promise.resolve() : legacy.prepareBackgrounds?.(report, frame) ?? Promise.resolve(); },
    retainPreparedFrames(report, packets) { if (!side.playbackEngine?.()) return legacy.retainPreparedFrames?.(report, packets); },
    clearPreparedFrames() { if (!side.playbackEngine?.()) return legacy.clearPreparedFrames?.(); },
    /** The JavaScript renderer's packet cache; none while the engine prepares playback (nothing is uploaded ahead). */
    get preparedFrames() { return side.playbackEngine?.() ? undefined : legacy.preparedFrames; },
    /** While the engine prepares playback, what is hidden is part of each prepared frame (as for software frames). */
    get softwarePlayback() { return side.playbackEngine?.() ? true : legacy.softwarePlayback; },
    resize(width, height, dpr = 1) {
      legacy.resize(width, height, dpr);
      const w = Math.max(1, Math.round(width * dpr)), h = Math.max(1, Math.round(height * dpr));
      if (side.surface.width !== w || side.surface.height !== h) { side.surface.width = w; side.surface.height = h; side.stage.resize(w, h); }
    },
    /**
     * The editor changed the document: which records it edited in place
     * (`{frameIndices, figureIndices, sourceIds, structural, definition}`), merged until the next frame.
     */
    documentChanged(detail = {}) {
      const dirty = side.dirty ??= {frameIndices: [], figureIndices: [], sourceIds: [], structural: false, definition: false};
      dirty.frameIndices.push(...detail.frameIndices ?? []);
      dirty.figureIndices.push(...detail.figureIndices ?? []);
      dirty.sourceIds.push(...detail.sourceIds ?? []);
      dirty.structural ||= Boolean(detail.structural);
      dirty.definition ||= Boolean(detail.definition);
    },
    /** The engine side of the stage, for the playback adapter and tests. */
    get engineSide() { return side; },
    destroy() { show(false); legacy.destroy(); },
  };
  // Everything else is the JavaScript renderer's, bound to it.
  return new Proxy(renderer, {
    get(target, property) {
      if (Object.hasOwn(target, property) || property === 'engineActive' || property === 'engineStats' || property === 'canvas' || property === 'engineSide' || property === 'softwarePlayback' || property === 'preparedFrames') return Reflect.get(target, property);
      // Handles are drawn on the overlay while the engine draws the scene.
      if (property === 'handleRenderer' && engineShown) return undefined;
      const value = legacy[property];
      return typeof value === 'function' ? value.bind(legacy) : value;
    },
    set(target, property, value) {
      if (Object.hasOwn(target, property)) target[property] = value; else legacy[property] = value;
      return true;
    },
    has: (target, property) => property in target || property in legacy,
  });
}
