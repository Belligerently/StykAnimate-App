/**
 * The stage: one renderer object for the app, two renderers behind it while
 * the engine port is in progress (docs/RUST-ENGINE-PORT.md, stage S3).
 *
 * `createStageRenderer` wraps the JavaScript renderer. The engine draws into
 * its own canvas, stacked exactly where the JavaScript renderer's is, and the
 * other is hidden; the frames it does not take (below, `refusal`) and everything
 * it does not handle yet are forwarded to the JavaScript renderer, so the app
 * sees one renderer with one interface.
 *
 * What crosses to the engine per edited frame is the pose the editor shows:
 * only the objects whose geometry the editor rebuilt since the last frame.
 */
import {loadEngine} from './engine.mjs';
import {EngineMirror} from './engine-mirror.mjs';
import {ENGINE_KEY, engineTicket} from './engine-playback.mjs';
import {drawingResourcesFor} from '../drawing-resources.mjs';
import {EngineCameraMonitor} from './engine-previews.mjs';

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
  return shared = {engine, stage, surface, mirror: new EngineMirror(engine),
    /** Geometry groups the engine last received, by slot: an unchanged group is not sent again. */
    groups: [], frameReport: null, count: -1, definitions: null, failed: null, dirty: null, lastRefusal: null,
    /** Onion skin poses the engine holds, by the editor's sampled frame. */
    ghosts: new WeakMap(), nextGhost: 1, frameSent: false,
    /** The drawing tiles each figure was last sent with: publication replaces them in place. */
    tiles: new WeakMap(), drawingUnpublished,
    /** Counts the frames drawn: a wait for pictures ends when another frame is drawn. */
    pictureWait: 0,
    stats: {frames: 0, objectsSent: 0, lastSendMs: 0, lastDrawMs: 0, fallbacks: 0, pictureRedraws: 0}};
}

/**
 * Whether a drawing of `report` has strokes not yet published into its figure
 * (drawing-resources.mjs owns them until then): the engine draws a drawing from
 * its published tiles, so until publication the JavaScript renderer draws.
 */
function drawingUnpublished(report) {
  const resources = drawingResourcesFor(report);
  if (!resources) return false;
  for (const entry of resources.entries.values()) if (!entry.transient && (entry.store || entry.deferred?.length) && entry.published?.revision !== entry.revision) return true;
  return false;
}

/** The published tiles of a figure's drawings, its own and its movieclip's. */
function drawingTiles(figure, out = []) {
  if (figure._styk?.drawing) out.push(figure.image_assets?.[0]?.styk_tiles ?? null);
  for (const nested of figure._styk?.movieclip?.document?.figure_types ?? []) drawingTiles(nested, out);
  return out;
}

/**
 * Whether a GPU canvas's picture reaches the page: the engine cleared it to opaque magenta
 * (`probe` in styk-wasm/src/views.rs) in this task. Some setups present nothing from a WebGPU
 * canvas (Xvfb without ANGLE on Vulkan); there the views stay the JavaScript renderer's.
 */
function probed(canvas) {
  const sample = new OffscreenCanvas(1, 1), context = sample.getContext('2d', {willReadFrequently: true});
  context.drawImage(canvas, 0, 0, 1, 1, 0, 0, 1, 1);
  const [r, g, b, a] = context.getImageData(0, 0, 1, 1).data;
  return r > 200 && g < 60 && b > 200 && a > 200;
}

const STRIP = new Set(['runtime_segments']);
/** How long a thumbnail waits for its frame and pictures before the JavaScript renderer draws it. */
const THUMBNAIL_WAIT_MS = 60000;
/** How long a frame shown while its pictures decode waits for the engine's workers to finish one. */
const PICTURE_WAIT_MS = 60000;
/** A pose as JSON, without derived caches. */
const poseJSON = object => JSON.stringify(object, (key, value) => STRIP.has(key) ? undefined : value);

/**
 * A frame was just drawn by `side.stage`: when it lacks pictures still on their way
 * (`Stage::pictures_pending`: drawing cels and images decode on the engine's workers and each
 * frame uploads a slice of them), `redraw` is called once they can be shown: on the next
 * microtask for pictures already decoded, else when the engine's progress counter (read as
 * `seen` before drawing) advances. One wait at a time per stage side, dropped when another frame
 * is drawn; waiting is `Atomics.waitAsync` (D7), and a wait that times out with nothing finished
 * waits again without drawing, for at most `PICTURE_WAIT_MS`.
 */
export function waitForPictures(side, seen, redraw) {
  const token = ++side.pictureWait, pending = side.stage.pictures_pending();
  if (!pending) return;
  const again = () => { side.stats.pictureRedraws++; redraw(); };
  if (pending === 1) { queueMicrotask(again); return; }
  const started = performance.now();
  const wait = () => side.engine.whenProgress(seen).then(() => {
    if (token !== side.pictureWait) return;
    if (side.engine.progressSeen() !== seen) again();
    else if (performance.now() - started < PICTURE_WAIT_MS) wait();
  });
  wait();
}

/** Cancels only this stage's asynchronous camera prerequisite. */
export function cancelCameraFrames(side) {
  const sync = side.cameraFrames;
  if (!sync) return;
  side.cameraFrames = null;
  sync.controller.abort(new DOMException('The camera preparation was superseded.', 'AbortError'));
}

/** Keep at most one dirty identity per live record while the camera is off. */
export function recordCameraChange(side, detail) {
  const report = side.frameReport;
  if (!report) return;
  const dirty = side.cameraDirty ??= {frameIndices: new Set(), figureIndices: new Set(), sourceIds: new Set(), allFrames: false, allFigures: false, allSources: false};
  // A long edit session stays bounded even when old records are removed without
  // individual dirty names. A full resend uses the mirror's asynchronous owner.
  const include = (items, identities, all, present) => {
    if (dirty[all]) return;
    let visited = 0;
    for (const identity of items ?? []) {
      if (++visited > 256) { identities.clear(); dirty[all] = true; break; }
      if (present(identity)) identities.add(identity); else identities.delete(identity);
      if (identities.size >= 256) { identities.clear(); dirty[all] = true; break; }
    }
  };
  include(detail.frameIndices, dirty.frameIndices, 'allFrames', index => index >= 0 && index < (report.keyframes?.length ?? 0));
  include(detail.figureIndices, dirty.figureIndices, 'allFigures', index => index >= 0 && index < (report.figure_types?.length ?? 0));
  const sources = report._styk?.frameSources ?? {};
  include(detail.sourceIds, dirty.sourceIds, 'allSources', id => Object.hasOwn(sources, id));
  // A removed source does not need an explicit resend; syncFrames removes it.
  dirty.allFigures ||= Boolean(detail.definition && !detail.figureIndices?.length);
  if (detail.structural) {
    dirty.frameIndices.clear(); dirty.sourceIds.clear();
    dirty.allFrames = true; dirty.allSources = true;
  }
}

/**
 * An edit pose can arrive before playback has mirrored its stored target poses.
 * Coalesce one asynchronous frame sync through the existing mirror, then let
 * render_edit request worker frame tickets on that complete Prepared owner.
 */
export function ensureCameraFrames(side, {report, revision = 0}, redraw) {
  if (side.mirror.framesCurrent(report, revision)) return false;
  const old = side.cameraFrames;
  if (old?.report === report && old.revision === revision) return true;
  cancelCameraFrames(side);
  const sync = {report, revision, controller: new AbortController()};
  side.cameraFrames = sync;
  const dirty = side.cameraDirty;
  sync.promise = Promise.resolve().then(() => side.mirror.syncFrames(report, revision, dirty ?? {}, {signal: sync.controller.signal})).then(() => {
    if (side.cameraFrames !== sync || sync.controller.signal.aborted || !side.mirror.framesCurrent(report, revision)) return;
    if (side.cameraDirty === dirty) side.cameraDirty = null;
    redraw();
  }).catch(error => {
    if (side.cameraFrames !== sync || sync.controller.signal.aborted || error?.name === 'AbortError') return;
    side.failed = `the engine refused the camera document (${error.message})`;
    redraw();
  }).finally(() => { if (side.cameraFrames === sync) side.cameraFrames = null; });
  return true;
}

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
    if (request.drawingPreview) return 'a drawing preview';
    if (drawingUnpublished(request.report)) return 'unpublished drawing strokes';
    return null;
  };

  /** Figures whose drawings were published in place since the engine got them become dirty. */
  const republished = report => {
    const changed = [];
    report.figure_types?.forEach((figure, index) => {
      if (!figure._styk?.drawing && !figure._styk?.movieclip) return;
      const tiles = drawingTiles(figure), known = side.tiles.get(figure);
      side.tiles.set(figure, tiles);
      if (known && (known.length !== tiles.length || known.some((value, i) => value !== tiles[i]))) changed.push(index);
    });
    if (!changed.length) return;
    side.dirty ??= {frameIndices: [], figureIndices: [], sourceIds: [], structural: false, definition: false};
    side.dirty.figureIndices.push(...changed);
    side.definitions = null;
  };

  /** Figure definitions, backgrounds and properties: what an edit frame is drawn with. */
  const syncDefinitions = request => {
    republished(request.report);
    const key = `${request.revision}:${request.artworkRevision}`;
    if (side.mirror.report === request.report && side.definitions === key) return;
    // Figures edited in place keep their objects; the editor's change events said which.
    const dirty = side.dirty;
    side.dirty = null;
    const result = side.mirror.syncDefinitions(request.report, request.revision ?? 0,
      {figureIndices: dirty?.figureIndices ?? [], allFigures: Boolean(dirty?.definition && !dirty.figureIndices?.length)});
    side.definitions = key;
    if (side.frameReport !== request.report) { side.groups = []; side.count = -1; side.fields = null; side.frameSent = false; side.ghosts = new WeakMap(); side.tiles = new WeakMap(); side.frameReport = request.report; }
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

  const awaitPictures = seen => waitForPictures(side, seen, () => options.onInvalidate?.());

  const drawEdit = request => {
    if (side.cameraFrames && (side.cameraFrames.report !== request.report || side.cameraFrames.revision !== (request.revision ?? 0))) cancelCameraFrames(side);
    if (side.frameReport !== request.report || side.mirror.framesCurrent(request.report, request.revision ?? 0)) side.cameraDirty = null;
    const started = performance.now(), seen = side.engine.progressSeen();
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
    side.stage.set_effects(request.vfxEnabled !== false);
    side.stage.set_canvas_drawing(legacy.softwarePlayback === true);
    const drawn = side.stage.render_edit((view.scale ?? 1) * dpr, (view.x ?? 0) * dpr, (view.y ?? 0) * dpr, request.cameraEnabled !== false,
      request.clipToCanvas !== false, new Uint32Array(hidden), new Float64Array(translations),
      Number.isFinite(request.editorBackgroundColor) ? request.editorBackgroundColor >>> 0 : -1, new Float64Array(onions));
    Object.assign(side.stats, {frames: side.stats.frames + 1, lastSendMs: sendMs, lastDrawMs: performance.now() - started - sendMs});
    // Generated effects whose trails are still being sampled refine on the next frame, as
    // the JavaScript renderer's incomplete batches do (6 ms of sampling a frame).
    if (side.stage.refining()) queueMicrotask(() => options.onInvalidate?.());
    const pendingCamera = side.stage.camera_pending?.() === true;
    side.stats.cameraPending = pendingCamera;
    if (pendingCamera) ensureCameraFrames(side, request, () => options.onInvalidate?.());
    else cancelCameraFrames(side);
    if (drawn || pendingCamera) awaitPictures(seen);
    // The previous actual scene stays visible while the shared follow path warms.
    // Keeping ownership also prevents JavaScript preparation on this cold path.
    return drawn || pendingCamera;
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
    /**
     * Returning to the editable canvas in a dense scene: the JavaScript renderer
     * builds its GPU scene ahead (hundreds of milliseconds on large documents). The
     * engine draws an edit frame from the pose itself, so for a document it draws
     * there is nothing to prepare and the edit shows on the next frame.
     */
    prepareSceneUpdate(request) {
      if (refusal(request) !== null) return legacy.prepareSceneUpdate(request);
      const {report, frame, geometry, view} = request;
      return Promise.resolve({report, frame, geometry, view, timings: {elapsedMs: 0, engine: true}, dispose() {}, activate() {}});
    },
    // Prepared playback frames: an engine packet is a ticket for a tick the engine has ready.
    prepareFrame(packet, options) { return packet?.engine ? packet.key : legacy.prepareFrame(packet, options); },
    hasPreparedFrame(key) {
      if (typeof key !== 'string' || !key.startsWith(ENGINE_KEY)) return legacy.hasPreparedFrame(key);
      const ticket = engineTicket(key);
      return ticket !== null && side.engine.handle.frame_ticket_ready(ticket.generation, ticket.epoch, ticket.tick);
    },
    canWarm: (...args) => legacy.canWarm?.(...args),
    renderPrepared(key, options = {}) {
      if (typeof key !== 'string' || !key.startsWith(ENGINE_KEY)) { const result = legacy.renderPrepared(key, options); if (result) show(false); return result; }
      const ticket = engineTicket(key);
      if (!ticket) return false;
      const dpr = legacy.dpr || 1, view = options.view ?? {}, seen = side.engine.progressSeen();
      side.stage.set_canvas_drawing(false);
      const drawn = side.stage.render_ticket(ticket.generation, ticket.epoch, ticket.tick, (view.scale ?? 1) * dpr, (view.x ?? 0) * dpr, (view.y ?? 0) * dpr, options.cameraEnabled !== false, true);
      if (drawn) { show(true); awaitPictures(seen); }
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
      cancelCameraFrames(side);
      recordCameraChange(side, detail);
      const dirty = side.dirty ??= {frameIndices: [], figureIndices: [], sourceIds: [], structural: false, definition: false};
      dirty.frameIndices.push(...detail.frameIndices ?? []);
      dirty.figureIndices.push(...detail.figureIndices ?? []);
      dirty.sourceIds.push(...detail.sourceIds ?? []);
      dirty.structural ||= Boolean(detail.structural);
      dirty.definition ||= Boolean(detail.definition);
    },
    /**
     * For an export (engine-export.mjs): why the engine cannot draw `report`, or null once
     * its properties, figure definitions and backgrounds are current, edits in place and
     * published drawings included (its frames come through the playback adapter first).
     * Throws `engine_busy` while a background commit runs: the caller tries again.
     */
    prepareExport(report, revision) {
      const reason = refusal({report, revision, geometry: true});
      if (reason) return reason;
      try { syncDefinitions({report, revision, artworkRevision: 'export'}); return null; }
      catch (error) { side.definitions = null; if (error?.code === 'engine_busy') throw error; return String(error?.message ?? error); }
    },
    /**
     * The camera monitor (camera-monitor.mjs): the frame the engine drew last, seen through the
     * scene camera, drawn on the stage's device into a canvas of the monitor's own and copied onto
     * `target` (the monitor's 2D context) at its device-pixel size `width × height`, `zoom` device
     * pixels per scene unit, the scene's origin at `panX, panY`. False when the engine did not
     * draw the last frame or cannot present to a second canvas whose pictures reach the page
     * (WebGL2): the caller draws it.
     */
    drawCameraMonitor(target, {width, height, zoom, panX, panY, dpr = 1}, afterSourcePaint) {
      if (!engineShown || side.failed) { side.workerMonitor?.destroy(); side.workerMonitor = null; return false; }
      if (side.engine.threaded) {
        side.workerMonitor ??= new EngineCameraMonitor(side.stage, {invalidate: () => options.onInvalidate?.()});
        return side.workerMonitor.draw(target, {width, height, zoom, panX, panY, dpr}, afterSourcePaint);
      }
      if (side.monitor === undefined) {
        side.monitorCanvas = new OffscreenCanvas(Math.max(1, width), Math.max(1, height));
        try {
          side.monitor = side.stage.attach_monitor(side.monitorCanvas);
          if (side.monitor && !probed(side.monitorCanvas)) { side.stage.detach_monitor(); side.monitor = false; }
        } catch (error) { console.warn('The camera monitor could not use the engine.', error); side.monitor = false; }
      }
      if (!side.monitor) return false;
      const canvas = side.monitorCanvas;
      if (canvas.width !== width || canvas.height !== height) { canvas.width = width; canvas.height = height; }
      if (!side.stage.render_monitor(width, height, zoom, panX, panY)) return false;
      target.save();
      target.setTransform(1, 0, 0, 1, 0, 0);
      target.drawImage(canvas, 0, 0);
      target.restore();
      afterSourcePaint?.(false);
      return true;
    },
    /**
     * A frame thumbnail (engine-thumbnails.mjs): `tick` of the document at `revision`, on a
     * `width × height` card, drawn on the stage's device into a canvas of the thumbnails' own
     * and copied onto `target` (a 2D canvas of that size). The frame is prepared on the engine's
     * workers and every picture reaches the atlas first; this waits for both on the engine's
     * progress counter, never blocking (D7). Resolves `drawn`, `stale` (the engine holds another
     * revision: ask again once it follows) or `refused` (no engine, a device that cannot present
     * to a second canvas, WebGL2, or a canvas whose pictures do not reach the page); rejects with
     * an AbortError when `signal` aborts.
     */
    async renderThumbnail({revision, tick, width, height}, target, {signal} = {}) {
      if (side.failed) return 'refused';
      if (side.thumbnails === undefined) {
        side.thumbnailCanvas = new OffscreenCanvas(Math.max(1, width), Math.max(1, height));
        try {
          side.thumbnails = side.stage.attach_thumbnails(side.thumbnailCanvas);
          if (side.thumbnails && !probed(side.thumbnailCanvas)) { side.stage.detach_thumbnails(); side.thumbnails = false; }
        } catch (error) { console.warn('Thumbnails could not use the engine.', error); side.thumbnails = false; }
      }
      if (!side.thumbnails) return 'refused';
      const engine = side.engine, started = performance.now();
      for (;;) {
        if (signal?.aborted) throw new DOMException('The thumbnail was superseded.', 'AbortError');
        // A frame that never comes (the engine stopped preparing) leaves it to the JavaScript renderer.
        if (performance.now() - started > THUMBNAIL_WAIT_MS) return 'refused';
        const seen = engine.progressSeen();
        const prepared = side.stage.prepare_thumbnail(revision, tick);
        if (prepared === 2) return 'stale';
        if (prepared === 0) {
          const drawn = side.stage.render_thumbnail(revision, tick, width, height);
          if (drawn === 2) return 'stale';
          if (drawn === 0) {
            // In the same task: the canvas holds this picture only until the task ends.
            const context = target.getContext('2d');
            context.setTransform(1, 0, 0, 1, 0, 0);
            context.clearRect(0, 0, target.width, target.height);
            context.drawImage(side.thumbnailCanvas, 0, 0);
            return 'drawn';
          }
        }
        await engine.whenProgress(seen);
      }
    },
    /** The engine side of the stage, for the playback adapter and tests. */
    get engineSide() { return side; },
    destroy() { cancelCameraFrames(side); side.pictureWait++; side.workerMonitor?.destroy(); side.workerMonitor = null; show(false); legacy.destroy(); },
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
