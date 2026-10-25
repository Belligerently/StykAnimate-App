import {messageError} from './localized-error.mjs';
import {drawingMetadata,isDrawingLayer,createDrawingFigure} from './drawing-model.mjs';
import {sheetKeyAt} from './layer-sheet.mjs';
import {createObject} from './demo.mjs';

export function assertDrawingTargets(editor,ids) {
  if(!ids.length)throw messageError(new Error(''), "errors.editorDrawing.chooseADrawingLayer", {});
  for(const id of ids){const layer=editor.getLayer(id);if(!isDrawingLayer(layer))throw messageError(new Error(''), "errors.editorDrawing.chooseADrawingLayerOrCreateANew", {});if(layer.locked||!layer.visible)throw messageError(new Error(''), "errors.editorDrawing.showAndUnlockBeforeDrawing", {name: layer.name});}
}
export function applyDrawingCommand(editor,command,direction,prepared) {
  if(command.kind!=='drawing-pixels')return false;
  const figure=editor.report.figure_types.find(f=>drawingMetadata(f)?.celId===command.celId);
  if(!figure)throw messageError(new Error(''), "errors.editorDrawing.theDrawingHistoryReferencesAMissingCel", {});
  editor.drawingResources.apply(figure,command.tiles,direction,prepared);
  const d=drawingMetadata(figure);
  if(d.version===2)d.bounds=command[direction==='before'?'beforeBounds':'afterBounds']??null;
  editor._revision=command[`${direction}Revision`];editor._invalidate({timing:false});
  editor._emit('history',{label:command.label,definition:true,drawing:true,figureIndices:[editor.report.figure_types.indexOf(figure)]});return true;
}
export const editorDrawingMethods={
  // The structure of a drawing layer (a new layer, a new or copied cel, a longer exposure, an auto key) is the engine
  // session's: `createDrawingLayer`, `addDrawingFrame`, `drawingKey`, `nextDrawing`, `extendDrawing`, `autoKeyDrawing`.
  /** A cel's encoded tiles were attached to its definition in place: a derived publication, not an edit. The engine owner writes the definition into its session. */
  _drawingPublished(figure) {},
  drawingTarget(id=this.activeLayerId,tick=this.tick) {
    assertDrawingTargets(this,[id]);const track=this.report._styk.layerSheet.tracks[id],{key,next}=sheetKeyAt(track,tick);
    if(!key||tick>=track.end)throw messageError(new Error(''), "errors.editorDrawing.insertABlankDrawingOrCopyDrawingHere", {});
    const frame=key.value?.frameId?this.getFrameById(key.value.frameId):null;
    const object=frame?.objects.find(o=>this.layerForUid(o.unique_id)===id),figure=object&&this.report.figure_types.find(f=>f.figure_type_id===object.figure_type_id);
    if(figure&&!drawingMetadata(figure))throw messageError(new Error(''), "errors.editorDrawing.convertThisImageToADrawingBeforePainting", {});
    return {id,key,end:next?.tick??track.end,object,figure};
  },
  prepareDrawingCel({pixelsPerUnit=1,world={x:0,y:0}}={}) {
    const target=this.drawingTarget();if(target.figure)return target;
    const figure=createDrawingFigure({pixelsPerUnit,figureTypeId:this.report.figure_types.reduce((n,f)=>Math.max(n,f.figure_type_id+1),1)});
    const uid=this.getLayerFigureUids(target.id)[0]??this._nextUid;
    const object=createObject(figure,uid,world.x,world.y,{scale:1/pixelsPerUnit});
    return {...target,figure,object,created:true};
  },
  // Publishing a resampled cel and committing a stroke's pixels are the owner's compositions of session calls (`engine-owner-drawing.mjs`).
};
