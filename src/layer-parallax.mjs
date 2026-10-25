import {messageError} from './localized-error.mjs';
/** What a layer's parallax and a baked camera's parallax offset may store; the engine resolves them (`styk_eval::parallax`). */
export function validateCameraParallaxOffset(camera){const p=camera?._parallaxMotionOffset;if(p!==undefined&&(!p||![p.x,p.y].every(Number.isFinite)))throw messageError(new Error(''), "errors.layerParallax.invalidBakedCameraParallaxOffset", {});}
export function validateParallax(p){
  if(p===undefined)return;
  if(!p||typeof p.enabled!=='boolean'||typeof p.linked!=='boolean'||![p.x,p.y].every(v=>Number.isFinite(v)&&v>=0&&v<=4)||!p.reference||![p.reference.x,p.reference.y].every(Number.isFinite))throw messageError(new Error(''), "errors.layerParallax.parallaxNeedsA0400ResponseAndA", {});
}
