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
import {liftDocument} from '../document-lift.mjs';
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
  /** @param {{handle: object, whenProgress(seen?: number): Promise<void>, progressSeen?(): number, pump(): void}} engine from loadEngine() */
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

  /** Accept a successful <=3 scalar project-field Session publication. Table,
   * source and raw root identities are retained by this same sent owner. No
   * lifting, serialization, reset or evaluator work happens on this path.
   * The host's pending barrier excludes an overlapping legacy mirror sync.
   */
  accept_session_head(report, before, after) {
    if (this.report !== report || this.framesRevision !== before[2]
      || before.length !== 4 || after.length !== 4 || after[1] !== before[1] || after[3] <= before[3])
      throw new Error('The canonical Session head belongs to a stale mirror owner.');
    const actual = this.engine.handle.try_record_identity();
    if (actual === undefined) return false;
    if (actual.length !== 3 || actual.some((value, index) => value !== after[index]))
      throw new Error('The canonical Session publication has not been acknowledged.');
    this.framesRevision = after[2];
    return true;
  }

  _begin(report) {
    if (this.report === report) return;
    this.reset();
    this.report = report;
    this.engine.handle.stage_reset(0);
  }

  /** Stages the layout and the new or named records of one table. Returns the indices to send. */
  _layout(batch, key, code, next, explicit, all = false) {
    const handle = this.engine.handle, previous = this.sent[key];
    const where = new Map(previous.map((record, index) => [record, index]));
    const layout = new Int32Array(next.length), fresh = [];
    next.forEach((record, index) => {
      const from = where.get(record);
      // A record edited in place keeps its object: the caller says so.
      if (all || from === undefined || explicit.has(index)) { layout[index] = from ?? -1; fresh.push(index); }
      else layout[index] = from;
    });
    if (next.length !== previous.length || layout.some((from, index) => from !== index)) handle.stage_layout(batch, code, layout);
    this.sent[key] = next.slice();
    return fresh;
  }

  /**
   * Brings the engine's properties, figure definitions and backgrounds to `report`
   * before returning. `figureIndices` names figures edited in place; `allFigures`
   * resends every one (a definition change that did not say which). `document` is the
   * editor's report when `report` is a draft of it (the cel a first stroke is about to create
   * is in the draft only): the mirror stays that document's, and the next sync of the report
   * itself takes the draft's figure out again.
   */
  syncDefinitions(report, revision, {figureIndices = [], allFigures = false, document = report} = {}) {
    const handle = this.engine.handle, started = performance.now();
    try {
      this._begin(document);
      handle.stage_properties(0, JSON.stringify(playbackProperties(report)));
      const figures = report.figure_types ?? [];
      const named = allFigures ? new Set(figures.keys()) : new Set(figureIndices);
      let records = 0;
      for (const index of this._layout(0, 'figure_types', FIGURES, figures, named)) { handle.stage_record(0, FIGURES, index, ...lift(figures[index])); records++; }
      const backgrounds = report.backgrounds ?? [];
      for (const index of this._layout(0, 'backgrounds', BACKGROUNDS, backgrounds, new Set())) { handle.stage_record(0, BACKGROUNDS, index, ...lift(backgrounds[index])); records++; }
      const result = JSON.parse(handle.commit_now(revision, 0));
      // A background commit is running and this thread may not wait for it (D7): what was
      // staged stays staged and the next commit takes it. The caller tries again later.
      if (result.busy) { this.stats.busy = (this.stats.busy ?? 0) + 1; throw Object.assign(new Error('The engine is committing the document.'), {code: 'engine_busy'}); }
      if (result.error) throw new Error(`The engine refused the document: ${result.error}`);
      Object.assign(this.stats, {definitionSyncs: this.stats.definitionSyncs + 1, records: this.stats.records + records, lastMs: performance.now() - started, lastRecords: records, lastCommitMs: result.ms});
      return result;
    } catch (error) {
      // Busy preparation keeps its staged description. Every actual refusal
      // abandons that batch and its optimistic table/source bookkeeping.
      if (error.code !== 'engine_busy') {
        handle.discard_batch(0);
        this.stats.errors++;
        this.reset();
      }
      throw error;
    }
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

  async _syncFrames(report, revision, {frameIndices = [], sourceIds = [], figureIndices = [], allFrames = false, allSources = false, allFigures = false}, signal) {
    const handle = this.engine.handle, started = performance.now();
    const live = generation => {
      if (signal?.aborted || (generation !== undefined && generation !== this.frameGeneration)) {
        throw signal?.reason ?? new DOMException('The document sync was superseded.', 'AbortError');
      }
    };
    for (;;) {
      live();
      const seen = this.engine.progressSeen?.();
      try { this.syncDefinitions(report, revision, {figureIndices, allFigures}); break; }
      catch (error) { if (error.code !== 'engine_busy') throw error; await this.engine.whenProgress(seen); }
    }
    const batch = this.nextBatch++, generation = this.frameGeneration;
    try {
      let slice = performance.now(), records = 0;
      const breathe = async () => {
        if (performance.now() - slice < SLICE_MS) return;
        await new Promise(resolve => setTimeout(resolve, 0));
        live(generation);
        slice = performance.now();
      };
      const frames = report.keyframes ?? [];
      for (const index of this._layout(batch, 'keyframes', KEYFRAMES, frames, new Set(frameIndices), allFrames)) {
        live(generation);
        handle.stage_record(batch, KEYFRAMES, index, ...lift(frames[index]));
        records++;
        await breathe();
      }
      const current = report._styk?.frameSources ?? {}, named = new Set(sourceIds);
      for (const [id, frame] of Object.entries(current)) {
        live(generation);
        if (!allSources && this.sources.get(id) === frame && !named.has(id)) continue;
        handle.stage_source(batch, id, ...lift(frame));
        this.sources.set(id, frame);
        records++;
        await breathe();
      }
      for (const id of [...this.sources.keys()]) if (!Object.hasOwn(current, id)) { handle.stage_source(batch, id, '', EMPTY, EMPTY); this.sources.delete(id); }
      live(generation);
      handle.commit(revision, batch);
      this.engine.pump();
      const result = await this._committed(revision);
      // A commit already running finishes atomically. Drain its response, but an
      // abandoned sync must not certify the caller's stale revision as current.
      live(generation);
      this.framesRevision = revision;
      Object.assign(this.stats, {frameSyncs: this.stats.frameSyncs + 1, records: this.stats.records + records, lastRecords: records,
        lastMs: performance.now() - started, lastCommitMs: result.ms});
      return result;
    } catch (error) {
      if (generation === this.frameGeneration) {
        // Table identities were staged optimistically. Keep current definitions,
        // but force the next frame sync to resend these records and sources.
        this.sent.keyframes = [];
        this.sources.clear();
        this.framesRevision = -1;
        this.frameGeneration++;
      }
      throw error;
    } finally {
      handle.discard_batch(batch);
    }
  }

  /** Waits for the engine to report the background commit of `revision`. */
  async _committed(revision) {
    for (;;) {
      const seen = this.engine.progressSeen?.(), text = this.engine.handle.take_committed();
      if (text === undefined) { await this.engine.whenProgress(seen); continue; }
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
