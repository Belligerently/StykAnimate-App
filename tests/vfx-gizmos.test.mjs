import test from 'node:test';
import assert from 'node:assert/strict';
import {VfxGizmos} from '../src/vfx-gizmos.mjs';

test('inactive VFX controls do not request scene evaluation or geometry',()=>{
  const gizmos=Object.create(VfxGizmos.prototype);
  gizmos.getOptions=()=>{throw new Error('Unexpected scene evaluation');};
  for(const inspector of [{visible:false},{visible:true,selectedEffect:()=>null},{visible:true,selectedEffect:()=>({effect:{enabled:false}})}]){
    gizmos.inspector=inspector;assert.equal(gizmos.control(),null);
  }
});

/** A gizmo over three drawables (figure 3 on a hidden layer) and an engine that says which figures figure 1's effects cover. */
function measured(group,{hide=true}={}){
  const item=(uid,x)=>({type:'line',thickness:2,obj:{unique_id:uid,scale:1},points:[{x,y:0},{x:x+10,y:10}]});
  const report={canvas_width:640,canvas_height:480,_styk:{layers:{defaultLayerId:'a',assignments:{1:'a',2:'a',3:'b'},byId:{a:{visible:true},b:{visible:!hide}}}}};
  const options={report,frame:{objects:[{unique_id:1,scale:2}]},geometry:{drawables:[item(1,0),item(2,100),item(3,300)]},view:{x:0,y:0,scale:1},playing:false,vfxEnabled:true,resources:{}};
  const asked=[];
  const editor={dragging:false,revision:1,tick:0,getLayer:()=>({locked:false}),layerForUid:()=>'a',_shown:{vfxGroup:uid=>{asked.push(uid);return group;}}};
  const gizmos=Object.create(VfxGizmos.prototype);
  Object.assign(gizmos,{editor,getOptions:()=>options,inspector:{visible:true,selectedEffect:()=>({target:{kind:'figure',uid:1},effect:{enabled:true,type:'glow',params:{}},includeAttached:true})}});
  return {control:gizmos.control(),asked,gizmos,editor};
}

test('a figure that owns connected effects is measured with the visible figures the engine says they cover',()=>{
  const {control,asked}=measured([1,2,3]);
  // Items 1 and 2 span x 0..110; each is padded by half its stroke and 3.
  assert.deepEqual([control.bounds.x,control.bounds.width],[-4,118]);assert.equal(control.unit,2);
  assert.deepEqual(asked,[1]);
  assert.equal(measured([1,2,3],{hide:false}).control.bounds.width,318,'a figure on a visible layer is covered too');
});

test('a figure with no connected group is measured alone, and the engine is asked once per frame',()=>{
  const {control,gizmos,asked,editor}=measured(null);
  assert.equal(control.bounds.width,18);
  gizmos.control();gizmos.control();assert.deepEqual(asked,[1],'the answer is kept while the frame, the document and the playhead stay');
  editor.revision++;gizmos.control();assert.deepEqual(asked,[1,1]);
});
