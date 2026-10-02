/**
 * Keeps the engine's document equal to the editor's (docs/engine/document.md).
 *
 * While commands still run in JavaScript, the engine holds a mirror of the
 * document. This module sends it what changed, record by record: the top-level
 * properties as JSON, and each figure, background, keyframe and pose source as
 * a lifted record (number arrays and images as bytes, see liftDocument). Table
 * order travels as "which old record sits where", so inserting or deleting a
 * frame resends no other frame.
 *
 * A record is resent when its object is not the one last sent (the editor
 * replaces records it publishes) or when the caller names it (records the
 * editor edited in place). Nothing is evaluated here.
 *
 * Two kinds of sync, because they have different deadlines:
 * - `syncDefinitions` (properties, figures, backgrounds) finishes before it
 *   returns: the frame about to be drawn needs them.
 * - `syncFrames` (keyframes, pose sources) lifts in slices and commits in the
 *   background: playback is prepared from them.
 */
import {liftDocument} from '../native-container.mjs';
import {playbackProperties} from '../playback-patch.mjs';

const FIGURES = 0, KEYFRAMES = 1, BACKGROUNDS = 2;
/** Lifting runs this long before the interface thread gets a turn. */
const SLICE_MS = 4;
const EMPTY = new Uint8Array(0);

/** A document record as the engine reads it: skeleton JSON and the two byte sections. */
function lift(record) {
  const {skeleton, sections} = liftDocument(record);
  return [JSON.stringify(skeleton), sections[0].bytes, sections[1].bytes];
}

export class EngineMirror {
  /** @param {{handle: object, whenProgress(): Promise<void>, pump(): void}} engine from loadEngine() */
  constructor(engine) {
    this.engine = engine;
    this.stats = {definitionSyncs: 0, frameSyncs: 0, records: 0, lastMs: 0, lastRecords: 0, lastCommitMs: 0, errors: 0};
    this.nextBatch = 1;
    this.frameSync = Promise.resolve();
    this.reset();
  }

  /** Forgets the mirrored document; the next sync sends everything. */
  reset() {
    this.report = null;
    /** The records last sent, by table: what the engine's tables hold. */
    this.sent = {figure_types: [], keyframes: [], backgrounds: []};
    this.sources = new Map();
    /** The interface revision whose frames the engine has confirmed. */
    this.framesRevision = -1;
    this.frameGeneration = (this.frameGeneration ?? 0) + 1;
  }

  /** Whether the engine can evaluate `report` at `revision` (its frames are committed). */
  framesCurrent(report, revision) {
    return this.report === report && this.framesRevision === revision;
  }

  _begin(report) {
    if (this.report === report) return;
    this.reset();
    this.report = report;
    this.engine.handle.stage_reset(0);
  }

  /** Stages the layout and the new or named records of one table. Returns the indices to send. */
  _layout(batch, key, code, next, explicit) {
    const handle = this.engine.handle, previous = this.sent[key];
    const where = new Map(previous.map((record, index) => [record, index]));
    const layout = new Int32Array(next.length), fresh = [];
    next.forEach((record, index) => {
      const from = where.get(record);
      // A record edited in place keeps its object: the caller says so.
      if (from === undefined || explicit.has(index)) { layout[index] = from ?? -1; fresh.push(index); }
      else layout[index] = from;
    });
    if (next.length !== previous.length || layout.some((from, index) => from !== index)) handle.stage_layout(batch, code, layout);
    this.sent[key] = next.slice();
    return fresh;
  }

  /**
   * Brings the engine's properties, figure definitions and backgrounds to `report`
   * before returning. `figureIndices` names figures edited in place; `allFigures`
   * resends every one (a definition change that did not say which).
   */
  syncDefinitions(report, revision, {figureIndices = [], allFigures = false} = {}) {
    const handle = this.engine.handle, started = performance.now();
    this._begin(report);
    handle.stage_properties(0, JSON.stringify(playbackProperties(report)));
    const figures = report.figure_types ?? [];
    const named = allFigures ? new Set(figures.keys()) : new Set(figureIndices);
    let records = 0;
    for (const index of this._layout(0, 'figure_types', FIGURES, figures, named)) { handle.stage_record(0, FIGURES, index, ...lift(figures[index])); records++; }
    const backgrounds = report.backgrounds ?? [];
    for (const index of this._layout(0, 'backgrounds', BACKGROUNDS, backgrounds, new Set())) { handle.stage_record(0, BACKGROUNDS, index, ...lift(backgrounds[index])); records++; }
    const result = JSON.parse(handle.commit_now(revision, 0));
    if (result.error) { this.stats.errors++; this.reset(); throw new Error(`The engine refused the document: ${result.error}`); }
    Object.assign(this.stats, {definitionSyncs: this.stats.definitionSyncs + 1, records: this.stats.records + records, lastMs: performance.now() - started, lastRecords: records, lastCommitMs: result.ms});
    return result;
  }

  /**
   * Brings the engine's frames and pose sources to `report` at `revision`, after its
   * definitions. `dirty` names records edited in place: `{frameIndices, sourceIds,
   * figureIndices, allFigures}`. Resolves once the engine has published the document.
   * A sync started later waits for this one; `signal` abandons it between slices.
   */
  syncFrames(report, revision, dirty = {}, {signal} = {}) {
    const run = this.frameSync.then(() => this._syncFrames(report, revision, dirty, signal));
    this.frameSync = run.catch(() => {});
    return run;
  }

  async _syncFrames(report, revision, {frameIndices = [], sourceIds = [], figureIndices = [], allFigures = false}, signal) {
    const handle = this.engine.handle, started = performance.now();
    this.syncDefinitions(report, revision, {figureIndices, allFigures});
    const batch = this.nextBatch++, generation = this.frameGeneration;
    let slice = performance.now(), records = 0;
    const breathe = async () => {
      if (performance.now() - slice < SLICE_MS) return;
      await new Promise(resolve => setTimeout(resolve, 0));
      // The mirror was reset, or the caller gave up: what was staged in this batch is never committed.
      if (signal?.aborted || generation !== this.frameGeneration) { this.reset(); throw signal?.reason ?? new DOMException('The document sync was superseded.', 'AbortError'); }
      slice = performance.now();
    };
    const frames = report.keyframes ?? [];
    for (const index of this._layout(batch, 'keyframes', KEYFRAMES, frames, new Set(frameIndices))) {
      handle.stage_record(batch, KEYFRAMES, index, ...lift(frames[index]));
      records++;
      await breathe();
    }
    const current = report._styk?.frameSources ?? {}, named = new Set(sourceIds);
    for (const [id, frame] of Object.entries(current)) {
      if (this.sources.get(id) === frame && !named.has(id)) continue;
      handle.stage_source(batch, id, ...lift(frame));
      this.sources.set(id, frame);
      records++;
      await breathe();
    }
    for (const id of [...this.sources.keys()]) if (!Object.hasOwn(current, id)) { handle.stage_source(batch, id, '', EMPTY, EMPTY); this.sources.delete(id); }
    if (generation !== this.frameGeneration) throw new DOMException('The document sync was superseded.', 'AbortError');
    handle.commit(revision, batch);
    this.engine.pump();
    const result = await this._committed(revision);
    this.framesRevision = revision;
    Object.assign(this.stats, {frameSyncs: this.stats.frameSyncs + 1, records: this.stats.records + records, lastRecords: records,
      lastMs: performance.now() - started, lastCommitMs: result.ms});
    return result;
  }

  /** Waits for the engine to report the background commit of `revision`. */
  async _committed(revision) {
    for (;;) {
      const text = this.engine.handle.take_committed();
      if (text === undefined) { await this.engine.whenProgress(); continue; }
      const result = JSON.parse(text);
      if (result.error) {
        // The engine kept its previous document; the next sync resends everything.
        this.stats.errors++;
        this.reset();
        throw new Error(`The engine refused the document: ${result.error}`);
      }
      if (result.revision === revision) return result;
    }
  }
}
