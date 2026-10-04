/**
 * Frame thumbnails drawn by the engine (docs/engine/rendering.md, Thumbnails and library
 * previews; stage S4 of docs/RUST-ENGINE-PORT.md).
 *
 * `createThumbnailRenderer` returns an object with the JavaScript `ThumbnailRenderer`'s
 * interface (`renderThumbnail`, `invalidate`, `setPaused`, `destroy`, `stats`), so the
 * filmstrip, the export dialog and the in-between dialog do not change. For a document
 * the engine draws, a thumbnail is the tick of the revision the editor is at, from the
 * document the engine already holds: its frame prepared on the engine's workers, drawn by
 * the stage's GPU renderer on the stage's device (`renderThumbnail` in engine-stage.mjs),
 * so it shows exactly what the stage shows. Nothing of the document is copied into a
 * thumbnail worker, and nothing is evaluated on the interface thread.
 *
 * The JavaScript renderer (thumbnail-renderer.mjs) draws what the engine does not, created
 * only when first needed: documents the gate refuses (engine-eligibility.mjs), in-between
 * previews (a sampled pose that is not in the document), stages on WebGL2 (one device
 * cannot present to a second canvas there), pages without the engine's worker threads. A
 * frame is drawn by one or the other, whole.
 */
import {ThumbnailRenderer} from '../thumbnail-renderer.mjs';
import {layerAppearance, thumbnailVisualChange} from '../thumbnail-validity.mjs';
import {drawingBarrier} from '../drawing-resources.mjs';
import {engineDocument} from './engine-document.mjs';

const abortError = () => new DOMException('The thumbnail was superseded.', 'AbortError');
/** `_route`'s answer while the app is still creating its stage: the caller asks again later. */
const NOT_YET = 'the stage is not ready yet';

/**
 * A thumbnail renderer for `editor`. `stage()` and `playback()` return the app's stage
 * renderer (engine-stage.mjs) and playback adapter (engine-playback.mjs) once they exist;
 * without them every thumbnail is the JavaScript renderer's. `options` are
 * `ThumbnailRenderer`'s.
 */
export function createThumbnailRenderer(editor, options = {}, {stage = () => null, playback = () => null} = {}) {
  return new EngineThumbnails(editor, options, {stage, playback});
}

export class EngineThumbnails {
  /** `javascript(editor, options)` makes the JavaScript renderer (tests give a stand-in). */
  constructor(editor, options = {}, {stage, playback, javascript = (target, settings) => new ThumbnailRenderer(target, settings)}) {
    this.editor = editor;
    this.options = options;
    this.stage = stage;
    this.playback = playback;
    this.makeJavaScript = javascript;
    this.javascript = null;
    this.version = 1;
    this.layerAppearance = layerAppearance(editor.report._styk?.layers);
    this.chain = Promise.resolve();
    this.queued = 0;
    this.paused = false;
    this.disposed = false;
    this.currentAbort = null;
    /** Why the last thumbnail was the JavaScript renderer's, by revision. */
    this.refusal = null;
    this.engineStats = {renders: 0, refused: 0, stale: 0, lastRenderMs: 0};
    this.events = new AbortController();
    editor.addEventListener('change', event => this.invalidate(event.detail || {}), {signal: this.events.signal});
  }

  /** The JavaScript renderer, created when a thumbnail first needs it. */
  _javascript() {
    return this.javascript ??= this.makeJavaScript(this.editor, this.options);
  }

  get stats() {
    const legacy = this.javascript?.stats;
    return {...legacy, backend: this.javascript ? 'Engine and worker thumbnails' : 'Engine thumbnails', engine: {...this.engineStats, refusal: this.refusal}};
  }

  invalidate(detail = {}) {
    const appearance = detail.layers || detail.kind === 'document' ? layerAppearance(this.editor.report._styk?.layers) : this.layerAppearance;
    const visual = thumbnailVisualChange(detail, this.layerAppearance, appearance);
    this.layerAppearance = appearance;
    if (!visual) return;
    this.version++;
    this.currentAbort?.abort();
  }

  setPaused(paused) {
    this.javascript?.setPaused(paused);
    if (this.paused === Boolean(paused)) return;
    this.paused = Boolean(paused);
    if (this.paused) this.currentAbort?.abort();
  }

  /**
   * Why the engine cannot draw this thumbnail, or null: the request is an in-between
   * preview, the stage has no engine with worker threads, or the gate refuses the document.
   */
  _route(inbetween) {
    if (inbetween) return 'an in-between preview';
    const stage = this.stage(), playback = this.playback();
    if (!stage) return NOT_YET;
    if (!stage?.engineSide?.engine?.threaded || typeof stage.renderThumbnail !== 'function' || typeof playback?.exportDocument !== 'function') return 'no engine with worker threads';
    // A device that cannot present to a second canvas (WebGL2) never will.
    if (stage.engineSide.thumbnails === false) return 'the stage cannot draw thumbnails on this device';
    return null;
  }

  renderThumbnail = (index, canvas, options = {}) => {
    const {signal, inbetween} = options;
    if (this.disposed || this.paused || signal?.aborted) return Promise.resolve(false);
    const route = this._route(inbetween);
    if (route === NOT_YET) return Promise.resolve(false);
    if (route) return this._javascript().renderThumbnail(index, canvas, options);
    // A caller outside the virtual filmstrip still cannot queue an unbounded backlog.
    if (this.queued >= 8) return Promise.resolve(false);
    this.queued++;
    const version = this.version;
    const task = this.chain.catch(() => {}).then(() => this._render(index, canvas, options, version));
    this.chain = task;
    return task.finally(() => { this.queued--; });
  };

  async _render(index, canvas, options, version) {
    const {signal} = options;
    if (this.disposed || this.paused || signal?.aborted || version !== this.version) return false;
    const controller = this.currentAbort = new AbortController();
    const abort = () => controller.abort();
    signal?.addEventListener('abort', abort, {once: true});
    const check = () => { if (controller.signal.aborted || version !== this.version || this.disposed) throw abortError(); };
    try {
      const editor = this.editor, report = editor.report, revision = editor.revision;
      const tick = options.tick ?? editor.timeline?.[index]?.start;
      if (!Number.isInteger(tick) || !report.keyframes?.[index]) return this._fallback(index, canvas, options, 'a frame outside the timeline');
      // Published drawing strokes are what thumbnails show (as the JavaScript worker waits for them).
      await drawingBarrier(report, {signal: controller.signal});
      check();
      const reason = this._route(false) ?? await engineDocument({stage: this.stage(), playback: this.playback(), report, revision, check});
      check();
      // An edit landed meanwhile: the engine follows it, and the caller asks again.
      if (editor.revision !== revision || editor.report !== report) return false;
      if (reason) return this._fallback(index, canvas, options, reason);
      const started = performance.now();
      const result = await this.stage().renderThumbnail({revision, tick, width: canvas.width, height: canvas.height}, canvas, {signal: controller.signal});
      if (result === 'stale') { this.engineStats.stale++; return false; }
      if (result !== 'drawn') {
        this.engineStats.refused++;
        const device = this.stage().engineSide?.thumbnails === false;
        return this._fallback(index, canvas, options, device ? 'the stage cannot draw thumbnails on this device' : 'the stage did not draw the thumbnail');
      }
      if (controller.signal.aborted || version !== this.version || this.paused) return false;
      this.refusal = null;
      Object.assign(this.engineStats, {renders: this.engineStats.renders + 1, lastRenderMs: performance.now() - started});
      return true;
    } catch (error) {
      if (error?.name === 'AbortError') return false;
      // The engine failed on this thumbnail: the JavaScript renderer draws it.
      if (!this.disposed) return this._fallback(index, canvas, options, String(error?.message ?? error));
      return false;
    } finally {
      signal?.removeEventListener('abort', abort);
      if (this.currentAbort === controller) this.currentAbort = null;
    }
  }

  /** The JavaScript renderer draws this thumbnail. */
  _fallback(index, canvas, options, reason) {
    this.refusal = reason;
    if (this.disposed || this.paused || options.signal?.aborted) return false;
    return this._javascript().renderThumbnail(index, canvas, options);
  }

  destroy() {
    this.disposed = true;
    this.events.abort();
    this.currentAbort?.abort();
    this.javascript?.destroy();
    this.javascript = null;
  }
}
