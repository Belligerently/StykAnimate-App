/**
 * Which documents the engine draws today (docs/RUST-ENGINE-PORT.md, stage S3).
 *
 * The port replaces the JavaScript renderer one feature at a time. Until every
 * feature is ported, a document that uses one the engine lacks keeps the
 * JavaScript renderer, whole. This module is that gate; it shrinks as features
 * land and disappears with the old renderer.
 *
 * The scan reads the document, never evaluates it, and after the first pass
 * only looks at the records an edit names.
 */

/** Every effect of an effects timeline is drawn, generated effects included
 * (docs/engine/vfx.md), and so are 3D meshes, one-sided surfaces, attached movieclip
 * meshes and drawings a spatial bone turns (docs/engine/spatial.md); a movieclip's own
 * document is read. */
function figureReason(figure, depth) {
  const styk = figure._styk;
  if (styk?.movieclip?.document) return depth > 16 ? 'deeply nested movieclips' : documentReason(styk.movieclip.document, depth + 1);
  return null;
}

function metadataReason(report) {
  const styk = report._styk;
  if (!styk) return null;
  if (styk.animation?.channels?.length || styk.layerSheet?.legacyAnimation) return 'property channels';
  if (styk.simpleFrameDraft) return 'a frame draft';
  return null;
}

/** The first feature of `report` the engine cannot draw, or null. Whole-document scan.
 * Poses hold nothing the engine lacks (effect stacks, 3D poses, colour transitions, Stick
 * Nodes sprites and animation origins are drawn), so only metadata and figures are read. */
function documentReason(report, depth = 0) {
  const metadata = metadataReason(report);
  if (metadata) return metadata;
  for (const figure of report.figure_types ?? []) { const reason = figureReason(figure, depth); if (reason) return reason; }
  return null;
}

/** Tracks one document's eligibility across edits without rescanning what did not change. */
export class EngineEligibility {
  constructor() { this.report = null; this.revision = -1; this.reason = null; this.blocked = new Map(); }

  /**
   * @param {object} report the editor's document
   * @param {number} revision the editor's revision
   * @param {{frameIndices?: number[], figureIndices?: number[], sourceIds?: string[], structural?: boolean, definition?: boolean}} [dirty]
   * @returns {string|null} why the engine cannot draw this document, or null when it can
   */
  check(report, revision, dirty) {
    if (this.report === report && this.revision === revision) return this.reason;
    const partial = this.report === report && dirty && !dirty.structural && !(dirty.definition && !dirty.figureIndices?.length);
    this.report = report; this.revision = revision;
    if (!partial) {
      this.blocked.clear();
      const reason = documentReason(report);
      if (reason) this.blocked.set('document', reason);
      return this.reason = reason;
    }
    // Only the named records can have changed what the document uses.
    const set = (key, reason) => { if (reason) this.blocked.set(key, reason); else this.blocked.delete(key); };
    if (this.blocked.has('document')) { this.blocked.clear(); const reason = documentReason(report); if (reason) this.blocked.set('document', reason); return this.reason = reason; }
    set('metadata', metadataReason(report));
    for (const index of dirty.figureIndices ?? []) set(`figure:${index}`, report.figure_types[index] ? figureReason(report.figure_types[index], 0) : null);
    return this.reason = this.blocked.values().next().value ?? null;
  }
}
