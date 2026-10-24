/**
 * The stage: the editing canvas, drawn by the engine alone (docs/engine/rendering.md).
 *
 * `createStage` gives the page one stage object over the engine's `Stage` (styk-wasm): the engine
 * draws into the page's scene canvas, edit frames and playback frames alike, and nothing else
 * draws a frame. A frame it cannot draw yet (the document is committing, a camera follow awaits
 * its worker, the surface was just resized) leaves the previous picture and is drawn again when
 * it can be; a frame it refuses leaves the previous picture too, with the reason reported once
 * (`engine-failure.mjs`), and the document's next revision is tried afresh.
 *
 * What crosses to the engine per edited frame is the pose the editor shows:
 * only the objects whose geometry the editor rebuilt since the last frame.
 */
import {loadEngine} from './engine.mjs';
import {EngineMirror} from './engine-mirror.mjs';
import {engineTicket} from './engine-playback.mjs';
import {failEngine, engineRefusal} from './engine-failure.mjs';
import {EngineCameraMonitor} from './engine-previews.mjs';
import {viewportBackingSize} from '../preview-quality.mjs';
import {messageError} from '../localized-error.mjs';

/** The engine side of the stage, shared by every stage the app creates (it survives the loss of a canvas or device). */
let shared = null;

/** What belongs to one `Stage` (one canvas and device): a new stage starts without any of it. */
const perStage = () => ({
  /** Geometry groups the engine last received, by slot: an unchanged group is not sent again. */
  groups: [], count: -1, fields: null, frameSent: false,
  /** Onion skin poses the engine holds, by the editor's sampled frame. */
  ghosts: new WeakMap(), nextGhost: 1,
  /** The drawing tiles each figure was last sent with: publication replaces them in place. */
  tiles: new WeakMap(),
  /** Counts the frames drawn: a wait for pictures ends when another frame is drawn. */
  pictureWait: 0,
  cameraFrames: null, cameraDirty: null,
  monitor: undefined, monitorCanvas: null, workerMonitor: null,
  thumbnails: undefined, thumbnailCanvas: null,
});

async function engineSide(load) {
  if (shared) return shared;
  const engine = await load();
  return shared = {engine, mirror: new EngineMirror(engine), stage: null, frameReport: null, definitions: null, failed: null, failure: null, dirty: null, lastRefusal: null,
    stats: {frames: 0, objectsSent: 0, lastSendMs: 0, lastDrawMs: 0, pictureRedraws: 0, retries: 0}, ...perStage()};
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
 * canvas (Xvfb without ANGLE on Vulkan); a second canvas on the device is not used there.
 */
function probed(canvas) {
  const sample = new OffscreenCanvas(1, 1), context = sample.getContext('2d', {willReadFrequently: true});
  context.drawImage(canvas, 0, 0, 1, 1, 0, 0, 1, 1);
  const [r, g, b, a] = context.getImageData(0, 0, 1, 1).data;
  return r > 200 && g < 60 && b > 200 && a > 200;
}

const STRIP = new Set(['runtime_segments']);
/** How long a thumbnail waits for its frame and pictures before it is given up. */
const THUMBNAIL_WAIT_MS = 60000;
/** How long a frame shown while its pictures decode waits for the engine's workers to finish one. */
const PICTURE_WAIT_MS = 60000;
/** How often the device is asked whether it was lost (a page that draws nothing would not notice). */
const DEVICE_POLL_MS = 1000;
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
    failEngine(side, `the engine refused the camera document (${error.message})`, report, revision);
    redraw();
  }).finally(() => { if (side.cameraFrames === sync) side.cameraFrames = null; });
  return true;
}

/**
 * The stage over `canvas`, the page's scene canvas: the engine's surface for as long as the stage lives.
 * Rejects with a localized error when the engine or its graphics device cannot start.
 * @param {HTMLCanvasElement} canvas the scene canvas (a canvas keeps one context type: a stage replaced after a lost device gets a new canvas)
 * @param {object} options `onInvalidate()` asks for another frame; `onStatus(message)` reports the device being lost; `onError(error)` reports a frame the engine refused
 * @param {() => Promise<object>} [load] how the engine is loaded (a test stands in its own)
 */
export async function createStage(canvas, options = {}, load = loadEngine) {
  let side;
  try { side = await engineSide(load); }
  catch (error) { throw messageError(error, 'errors.engineStage.theDrawingEngineCouldNotStart', {reason: error?.message ?? error}); }
  let stage;
  try { stage = await side.engine.createStage(canvas); }
  catch (error) { throw messageError(error, 'errors.engineStage.theDrawingEngineCouldNotStart', {reason: error?.message ?? error}); }
  // The engine keeps the document; a stage on a new device starts from empty poses and tiles.
  Object.assign(side, perStage(), {stage, failed: null, failure: null, dirty: null});
  /** The stage's pixels per CSS pixel (the last `resize`), and a frame the stage has drawn. */
  let deviceRatio = 1, engineShown = false, destroyed = false, lost = '', retrying = false, redrawQueued = false, preparedError = null;
  const warnings = [];
  /** The engine's counters, read from the stage once per frame drawn. */
  let statsAt = -1, gpuStats = null;
  const backend = (() => {
    const [kind, adapter = ''] = stage.backend().split(' · ');
    return {name: kind === 'BrowserWebGpu' ? 'WebGPU' : 'WebGL2', adapter};
  })();

  const invalidate = () => options.onInvalidate?.();
  /** Another attempt at a frame the engine could not draw yet, on the next animation frame. */
  const redrawSoon = () => {
    if (redrawQueued) return;
    redrawQueued = true;
    requestAnimationFrame(() => { redrawQueued = false; if (!destroyed) invalidate(); });
  };
  /** The device was lost, or reported errors nothing captured. */
  const pollDevice = () => {
    if (destroyed) return;
    const errors = stage.take_device_errors();
    if (errors) { for (const line of errors.split('\n')) { warnings.push(line); console.warn('The graphics device reported:', line); } warnings.splice(0, warnings.length - 8); }
    if (lost) return;
    const reason = stage.device_lost();
    if (!reason) return;
    lost = reason;
    // Not inside the frame that noticed it: the page stops what it is doing after the frame.
    queueMicrotask(() => options.onStatus?.({backend: 'WebGPU device lost', reason, message: `Graphics device disconnected: ${reason}. Reinitialize the renderer to continue.`,
      messageKey: 'rendererStatus.deviceDisconnected', messageParams: {detail: reason}}));
  };
  const poller = setInterval(pollDevice, DEVICE_POLL_MS);
  poller.unref?.();
  /** The poses the engine received are in doubt after an error: the next frame sends them all again. */
  const resend = () => {
    side.definitions = null;
    side.groups = []; side.count = -1; side.fields = null; side.frameSent = false; side.ghosts = new WeakMap();
  };
  /** The engine refused this document; reported once, retried with the next revision. */
  const refuse = (error, report, revision) => {
    failEngine(side, String(error?.message ?? error), report, revision);
    resend();
    options.onError?.(error);
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
      if (!known || !stage.has_ghost(known.id)) side.ghosts.set(onion.frame, known = {id: known?.id ?? side.nextGhost++, groups: [], count: -1});
      const json = frameJSON(onion.frame, onion.geometry, known);
      if (json !== null) stage.edit_ghost(known.id, json);
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

  const awaitPictures = seen => waitForPictures(side, seen, invalidate);

  /** The workspace view's turn and mirror (the Draw view), applied to the frame about to be drawn. */
  const orient = view => stage.set_view_orientation(Number.isFinite(view.rotation) ? view.rotation : 0, Boolean(view.flip));

  const drawEdit = request => {
    if (side.cameraFrames && (side.cameraFrames.report !== request.report || side.cameraFrames.revision !== (request.revision ?? 0))) cancelCameraFrames(side);
    if (side.frameReport !== request.report || side.mirror.framesCurrent(request.report, request.revision ?? 0)) side.cameraDirty = null;
    const started = performance.now(), seen = side.engine.progressSeen();
    syncDefinitions(request);
    const {frame, geometry} = request, objects = frame.objects ?? [];
    const json = frameJSON(frame, geometry, side);
    // An unchanged frame is not parsed again; the first frame and every changed one are.
    if (json !== null) { stage.edit_frame(json); side.frameSent = true; }
    const onions = onionList(request);
    const sendMs = performance.now() - started;
    const hidden = [], translations = [];
    objects.forEach((object, slot) => {
      const uid = object.unique_id;
      if (request.hiddenUids?.has(uid)) hidden.push(slot);
      const move = request.dragTranslations?.get?.(uid);
      if (move && (move.x || move.y)) translations.push(slot, move.x, move.y);
    });
    const view = request.view ?? {};
    stage.set_effects(request.vfxEnabled !== false);
    // The editor's frame is drawn on the destination pixel grid, as a canvas paints it.
    stage.set_canvas_drawing(true);
    orient(view);
    const drawn = stage.render_edit((view.scale ?? 1) * deviceRatio, (view.x ?? 0) * deviceRatio, (view.y ?? 0) * deviceRatio, request.cameraEnabled !== false,
      request.clipToCanvas !== false, new Uint32Array(hidden), new Float64Array(translations),
      Number.isFinite(request.editorBackgroundColor) ? request.editorBackgroundColor >>> 0 : -1, new Float64Array(onions));
    Object.assign(side.stats, {frames: side.stats.frames + (drawn ? 1 : 0), lastSendMs: sendMs, lastDrawMs: performance.now() - started - sendMs});
    // Generated effects whose trails are still being sampled refine on the next frame
    // (6 ms of sampling a frame).
    if (stage.refining()) queueMicrotask(invalidate);
    const pendingCamera = stage.camera_pending?.() === true;
    side.stats.cameraPending = pendingCamera;
    if (pendingCamera) ensureCameraFrames(side, request, invalidate);
    else cancelCameraFrames(side);
    if (drawn || pendingCamera) awaitPictures(seen);
    // A surface that was being resized takes the frame on the next animation frame.
    else if (stage.surface_pending()) redrawSoon();
    // The previous picture stays visible while the shared follow path warms.
    return drawn;
  };

  /** A background commit is running (`engine_busy`): the frame is drawn when the engine has published something. */
  const retryWhenIdle = () => {
    if (retrying) return;
    retrying = true;
    side.stats.retries++;
    side.engine.whenProgress(side.engine.progressSeen(), 16).then(() => { retrying = false; if (!destroyed) invalidate(); });
  };

  const renderer = {
    get canvas() { return canvas; },
    get dpr() { return deviceRatio; },
    /** Counters of the last frames, as the status label, telemetry and the performance report read them. */
    get stats() {
      if (destroyed) return {backend: backend.name, adapter: backend.adapter, warnings: []};
      if (statsAt !== side.stats.frames) { statsAt = side.stats.frames; gpuStats = JSON.parse(stage.stats()); }
      const gpu = gpuStats;
      return {backend: lost ? 'WebGPU device lost' : backend.name, adapter: backend.adapter, reason: lost || undefined,
        primitives: gpu.prims, objects: gpu.groups, drawCalls: gpu.passes, frameMs: side.stats.lastDrawMs + side.stats.lastSendMs,
        gpuBytes: gpu.effectBytes + gpu.layerBytes + gpu.depthBytes + gpu.materialBytes + gpu.glyphBytes, uploadBytes: gpu.uploadBytes,
        textureObjects: gpu.pictures, multisampled: gpu.multisampled, warnings: [...warnings], engine: gpu};
    },
    /** Set by the frame loop when this frame's stages are measured; `telemetryStages` then holds them. */
    measureFrame: false,
    telemetryStages: null,
    get engineStats() { return {...side.stats, mirror: {...side.mirror.stats}, refusal: side.failed ?? null, backend: stage.backend()}; },
    /** Draws an edited frame. False when it is not drawn (the previous picture stays and it is tried again when it can be). */
    render(request) {
      if (destroyed) return false;
      pollDevice();
      if (lost) return false;
      // The new-cel preview of a drawing is a document the engine does not hold.
      if (request.drawingPreview) return false;
      const revision = request.revision ?? 0;
      if (engineRefusal(side, request.report, revision)) return false;
      try {
        const drawn = drawEdit(request);
        if (drawn) engineShown = true;
        if (renderer.measureFrame) renderer.telemetryStages = {geometry: side.stats.lastSendMs, submit: side.stats.lastDrawMs};
        return drawn;
      } catch (error) {
        if (error?.code === 'engine_busy') { resend(); retryWhenIdle(); return false; }
        refuse(error, request.report, revision);
        return false;
      }
    },
    // Prepared playback frames: a packet is a ticket for a tick the engine has ready.
    hasPreparedFrame(key) {
      const ticket = engineTicket(key);
      return ticket !== null && side.engine.handle.frame_ticket_ready(ticket.generation, ticket.epoch, ticket.tick);
    },
    renderPrepared(key, frame = {}) {
      if (destroyed || lost) return false;
      const ticket = engineTicket(key);
      if (!ticket) return false;
      const view = frame.view ?? {}, seen = side.engine.progressSeen();
      try {
        stage.set_canvas_drawing(false);
        orient(view);
        const drawn = stage.render_ticket(ticket.generation, ticket.epoch, ticket.tick, (view.scale ?? 1) * deviceRatio, (view.x ?? 0) * deviceRatio, (view.y ?? 0) * deviceRatio, frame.cameraEnabled !== false, true);
        if (drawn) { engineShown = true; side.stats.frames++; awaitPictures(seen); }
        else if (stage.surface_pending()) redrawSoon();
        return drawn;
      } catch (error) {
        // The same refusal on every frame of a playback is reported once.
        if (preparedError !== error?.message) { preparedError = error?.message; options.onError?.(error); }
        return false;
      }
    },
    /** Destroys the graphics device as a driver reset would, for the tests of the recovery. */
    loseDevice() { stage.lose_device(); },
    /** The quality ladder's edge sampling: four samples, or one (`previewMsaaLevels`). */
    setMsaa(count) { stage.set_multisampled(!(Number.isFinite(count) && count < 4)); },
    resize(width, height, dpr = 1) {
      if (destroyed || lost) return;
      const backing = viewportBackingSize(width, height, dpr, stage.max_texture());
      deviceRatio = backing.dpr;
      if (canvas.width !== backing.pw || canvas.height !== backing.ph) { canvas.width = backing.pw; canvas.height = backing.ph; stage.resize(backing.pw, backing.ph); }
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
      const refused = engineRefusal(side, report, revision);
      if (refused) return refused;
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
     * revision: ask again once it follows) or `refused` (a device that cannot present
     * to a second canvas, WebGL2, or a canvas whose pictures do not reach the page); rejects with
     * an AbortError when `signal` aborts.
     */
    async renderThumbnail({revision, tick, width, height}, target, {signal} = {}) {
      if (destroyed || lost || side.failed) return 'refused';
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
        // A frame that never comes (the engine stopped preparing) is given up.
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
    /** The engine side of the stage, for the playback adapter and the other engine modules. */
    get engineSide() { return side; },
    destroy() {
      if (destroyed) return;
      destroyed = true; engineShown = false;
      clearInterval(poller);
      cancelCameraFrames(side); side.pictureWait++;
      side.workerMonitor?.destroy(); side.workerMonitor = null;
      if (side.stage === stage) { side.stage = null; stage.free(); }
    },
  };
  return renderer;
}
