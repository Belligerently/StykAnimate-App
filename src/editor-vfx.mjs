import {messageError} from './localized-error.mjs';
import {vfxStack} from './vfx.mjs';
import {vfxClips, sampleVfxClip, sameVfxTarget} from './vfx-clips.mjs';

/** The effects the stack of a scene or a figure holds in the frame on show. */
function owner(editor,target){return target.kind==='scene'?editor.evaluatedFrame:editor.evaluatedFrame.objects.find(o=>o.unique_id===target.uid);}
function find(editor,id){const c=vfxClips(editor.report).find(c=>c.id===id);if(!c)throw messageError(new Error(''), "errors.editorVfxClips.selectAnEffectClip", {});return c;}

/** What the interface reads of the effects in the document; every edit of them is the engine session's (engine-commands.mjs). */
export const editorVfxMethods={
  vfxEffects(target){return vfxStack(owner(this,target),target);},
  getVfxClips(target){return vfxClips(this.report).filter(c=>!target||sameVfxTarget(c.target,target));},
  sampleVfxClip(id,tick=this.tick){return sampleVfxClip(find(this,id),tick);},
};
