import {messageError} from './localized-error.mjs';
/** What a document stores of the presence clocks of movieclips (the engine derives the clocks from the authored spans, never from
 * playback state; `styk_eval::records`). Frame-only expansion leaves a continuation in the pose so subsequent samples do not move backward. */
export const presencePlayback = (settings = {}) => ({clock:'presence', offset:0, speed:1, mode:'loop', ...settings});
export const continuations = frame => frame?._styk?.movieclipContinuations ?? [];

export function setContinuations(frame, values) {
  frame._styk = {...frame._styk};
  if (values.length) frame._styk.movieclipContinuations = values;
  else delete frame._styk.movieclipContinuations;
  return frame;
}

export function validateContinuations(frame, types) {
  const values=frame?._styk?.movieclipContinuations;
  if(values===undefined)return;
  if(!Array.isArray(values)||values.length>65535)throw messageError(new Error(''), "errors.movieclipPresence.invalidMovieclipContinuationData", {});
  const seen=new Set();
  for(const value of values){
    if(!value||!Number.isInteger(value.uid)||value.uid<0||value.uid>0x7ffffffe||seen.has(value.uid)||!types.has(value.typeId)||value.playback?.clock!=='presence')throw messageError(new Error(''), "errors.movieclipPresence.invalidMovieclipContinuation", {});
    seen.add(value.uid);
  }
}
