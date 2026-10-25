import test from 'node:test';
import assert from 'node:assert/strict';
import {Editor} from '../src/editor.mjs';
import {createObject,createFrame} from '../src/demo.mjs';
import {beginNativePropertyEdit,previewNativePropertyEdit,commitNativePropertyEdit,cancelNativePropertyEdit} from '../src/native-property-edit.mjs';
const reset=e=>{e._undo=[];e._redo=[];e._historySize=0;e.markSaved();};
function fixture(){
 const e=new Editor({demo:false});e.addFigure();e.select(e.selectedUid,2);const f=e.figure;
 f.segments[2].secondary={rgb:[250,30,40],a:80};f.segments[3].secondary={rgb:[20,240,50],a:120};
 f.polyfills=[{polyfill_id:1,segment_ids:[1,3,4],color_rgb:[10,20,30],alpha_raw:35,color_alpha:1-35/255,z_order:.5}];
 const other=createObject(f,900,30,50,{object_slot:1});e.frame.objects.push(other);e.frame.object_draw_order.push(1);e.frame.object_draw_order_unique_ids.push(900);e.frame.duration_raw=12;e.report.keyframes.push(createFrame(e.frame.objects.map(object=>structuredClone(object))));e._invalidate({timing:true});reset(e);return e;
}
const target=(e,handleId)=>({kind:'segment',uid:e.selectedUid,handleId});
test('captured secondary style cancels exactly and same selected value still updates heterogeneous branch children',()=>{
 const e=fixture();e.frame.objects[0]._stykStyle={segments:{3:{gradientOpacity:.25}}};e._invalidate();reset(e);
 const source=JSON.stringify(e.frame),before=e.report._styk.animation;
 const cancel=beginNativePropertyEdit(e,'segment_gradient_opacity',{branch:true});previewNativePropertyEdit(cancel,60);cancelNativePropertyEdit(cancel);
 assert.equal(e.report._styk.animation,before);assert.equal(JSON.stringify(e.frame),source);assert.equal(e._undo.length,0);
 const token=beginNativePropertyEdit(e,'segment_gradient_opacity',{branch:true});assert.equal(token.initialValue,25);previewNativePropertyEdit(token,25);assert.ok(commitNativePropertyEdit(token));
 assert.equal(e.animationValue(target(e,3),'gradientOpacity'),.25);assert.equal(e.animationValue(target(e,4),'gradientOpacity'),.25);assert.equal(JSON.stringify(e.frame),source);
});
test('branch authored editing resolves current definition scope and retains unrelated channel references and interpolation',()=>{
 const e=fixture(),foreign={...structuredClone(e.figure),figure_type_id:77};e.report.figure_types.push(foreign);
 e.addAnimationKey({...target(e,3),figureTypeId:77},'opacity',0,.9,{exactTarget:true});
 e.addAnimationKey(target(e,3),'opacity',0,.2,{exactTarget:true});
 const global=e.animationChannels.find(c=>c.target.figureTypeId===undefined),other=e.animationChannels.find(c=>c.target.figureTypeId===77);global.keys[0].interpolation='hold';
 e.seek(6);reset(e);const token=beginNativePropertyEdit(e,'segment_opacity',{branch:true});previewNativePropertyEdit(token,60);commitNativePropertyEdit(token);
 assert.equal(e.animationChannels.find(c=>c.target.figureTypeId===77),other);assert.equal(e.animationChannels.find(c=>c.id===global.id),global);
 assert.ok(e.animationChannels.some(c=>c.target.figureTypeId===e.figure.figure_type_id&&c.target.handleId===3&&c.keys.some(k=>k.tick===6&&k.value===.6)));
});
