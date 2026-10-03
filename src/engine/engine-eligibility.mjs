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

import {isGeneratorType} from '../vfx-registry.mjs';

/** Per-pose fields the engine does not evaluate yet. Effect stacks (`_stykVfx`,
 * `_stykVfxScope`) and 3D poses (`_stykSpatial`: perspective, part proportions)
 * are drawn: a pose never holds a generated effect. */
const POSE_FIELDS = ['_stykAnimationOrigin', '_sticknodesSprite', 'color_transition'];

/** The first effect type of an effects timeline the engine does not draw: generated
 * effects (particles, aura, beams, smoke, explosions) stay with the JavaScript renderer. */
function clipReason(clips) {
  for (const clip of clips ?? []) for (const effect of clip.effects ?? []) if (isGeneratorType(effect.type)) return `generated effects (${effect.type})`;
  return null;
}

/** 3D meshes, one-sided surfaces and attached movieclip meshes are drawn
 * (docs/engine/spatial.md); drawings a spatial bone turns are not. */
function figureReason(figure, depth) {
  const styk = figure._styk;
  if (styk?.animationOrigin) return 'animation origins';
  if (styk?.movieclip?.drawingRotation) return 'rotated drawings';
  if (styk?.movieclip?.document) return depth > 16 ? 'deeply nested movieclips' : documentReason(styk.movieclip.document, depth + 1);
  return null;
}

function frameReason(frame) {
  for (const object of frame?.objects ?? []) for (const field of POSE_FIELDS) if (object[field] !== undefined) return field;
  return null;
}

function metadataReason(report) {
  const styk = report._styk;
  if (!styk) return null;
  const generated = clipReason(styk.vfxClips?.clips);
  if (generated) return generated;
  if (styk.animation?.channels?.length || styk.layerSheet?.legacyAnimation) return 'property channels';
  if (styk.simpleFrameDraft) return 'a frame draft';
  return null;
}

/** The first feature of `report` the engine cannot draw, or null. Whole-document scan. */
function documentReason(report, depth = 0) {
  const metadata = metadataReason(report);
  if (metadata) return metadata;
  for (const figure of report.figure_types ?? []) { const reason = figureReason(figure, depth); if (reason) return reason; }
  for (const frame of report.keyframes ?? []) { const reason = frameReason(frame); if (reason) return reason; }
  for (const frame of Object.values(report._styk?.frameSources ?? {})) { const reason = frameReason(frame); if (reason) return reason; }
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
    for (const index of dirty.frameIndices ?? []) set(`frame:${index}`, frameReason(report.keyframes[index]));
    for (const id of dirty.sourceIds ?? []) set(`source:${id}`, frameReason(report._styk?.frameSources?.[id]));
    return this.reason = this.blocked.values().next().value ?? null;
  }
}
