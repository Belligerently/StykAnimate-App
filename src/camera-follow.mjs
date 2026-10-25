import {messageError} from './localized-error.mjs';

/**
 * Camera follow: a camera motion clip that keeps figures in frame. The engine samples the followed figures and shapes the
 * camera's path (`styk_eval::follow`; the interface reads it as the shot, `Session::query` `shot` and `cameraSamples`,
 * docs/engine/evaluation.md): this module holds the stored data of a clip that the interface checks and shows.
 */
export const FOLLOW_TARGET_LIMIT = 16;
export const FOLLOW_FRAME_LIMIT = 100000;
export const FOLLOW_DEFAULTS = Object.freeze({strength: 1, followX: 1, followY: 1, offsetX: 0, offsetY: 0, lag: 8, deadZone: 0,
  lookAhead: 0, zoomFit: 0, padding: .2, maxZoom: 3, fadeIn: 6, fadeOut: 6});

export function validateFollowTargets(targets) {
  if (!Array.isArray(targets) || !targets.length || targets.length > FOLLOW_TARGET_LIMIT) throw messageError(new Error(''), "errors.cameraFollow.follow1Figures", {FOLLOW_TARGET_LIMIT: FOLLOW_TARGET_LIMIT});
  const uids = new Set();
  for (const target of targets) {
    if (!target || !Number.isInteger(target.uid) || target.uid < 0 || uids.has(target.uid) || typeof target.weight !== 'number' || !(target.weight >= 0 && target.weight <= 1))
      throw messageError(new Error(''), "errors.cameraFollow.eachFollowedFigureNeedsAUniqueFigureAnd", {});
    uids.add(target.uid);
  }
}
