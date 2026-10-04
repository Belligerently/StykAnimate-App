import test from 'node:test';
import assert from 'node:assert/strict';
import {cameraMonitorVisible,drawCameraMonitor} from '../src/camera-monitor.mjs';

test('collapsed camera monitors do not evaluate or rasterize a scene',()=>{
  const canvas={clientWidth:320,clientHeight:180,checkVisibility:()=>false,getContext(){throw Error('Hidden monitor rendered');}};
  assert.equal(cameraMonitorVisible(canvas),false);
  assert.equal(drawCameraMonitor(canvas,{report:{},frame:{}}),false);
  assert.equal(cameraMonitorVisible({...canvas,checkVisibility:()=>true}),true);
  assert.equal(cameraMonitorVisible({clientWidth:0,clientHeight:180}),false);
});

test('software camera monitors borrow prepared pixels without a GPU surface or scene evaluation',()=>{
  const report={canvas_width:640,canvas_height:400},calls=[];
  const context={setTransform(){},clearRect(){}};
  const canvas={clientWidth:320,clientHeight:240,getContext:type=>{assert.equal(type,'2d');return context;}};
  const renderer={softwarePlayback:true,renderPreparedInto(key,options){calls.push({key,options});return true;}};
  const options={report,frame:{},prepared:{renderer,key:'1:3',hiddenKey:'hidden-4'},hiddenUids:new Set([4])};
  options.geometry={get drawables(){throw Error('Prepared playback painted geometry.');}};
  assert.equal(drawCameraMonitor(canvas,options),true);
  assert.equal(calls.length,1);assert.equal(calls[0].key,'1:3');
  const forwarded=calls[0].options;
  assert.equal(forwarded.report,report);assert.equal(forwarded.target.context,context);assert.equal(forwarded.target.clear,false);
  assert.equal(forwarded.cameraEnabled,true);assert.equal(forwarded.hiddenKey,'hidden-4');assert.equal(forwarded.hiddenUids,options.hiddenUids);
  assert.deepEqual(forwarded.view,{scale:.5,x:0,y:20});
});


test('completed GPU-source presentation settles only a new actual DOM canvas lifetime',()=>{
  const events=new Map(),context={setTransform(){},clearRect(){}},canvas={clientWidth:278,clientHeight:139,width:278,height:139,
    getContext(){return context;},addEventListener(name,listener){events.set(name,listener);}};
  let invalidations=0,gpu=true;
  const options={report:{canvas_width:960,canvas_height:540},frame:{},onInvalidate(){invalidations++;},
    stage:{drawCameraMonitor(target,view,afterSourcePaint){assert.equal(target,context);afterSourcePaint?.(gpu);return true;}}};
  assert.equal(drawCameraMonitor(canvas,options),true);assert.equal(invalidations,1);
  drawCameraMonitor(canvas,options);assert.equal(invalidations,1,'ordinary subsequent paint does not loop');
  options.report={...options.report};options.stage={...options.stage};drawCameraMonitor(canvas,options);
  assert.equal(invalidations,1,'document/stage switch preserves the actual destination lifetime');
  canvas.clientWidth=320;drawCameraMonitor(canvas,options);assert.equal(invalidations,1,'size changes preserve the actual DOM canvas paint history');
  events.get('contextlost')();assert.equal(drawCameraMonitor(canvas,options),false);assert.equal(invalidations,1);
  events.get('contextrestored')();assert.equal(invalidations,2,'existing restore invalidation remains');
  drawCameraMonitor(canvas,options);assert.equal(invalidations,3,'first source after restore settles once');
  gpu=false;const direct={...canvas,width:278,height:139,clientWidth:278};drawCameraMonitor(direct,options);assert.equal(invalidations,3,'CPU-source presentation never asks for GPU settling');
  gpu=true;drawCameraMonitor(direct,options);assert.equal(invalidations,3,'prior direct/empty paint preserves the later first-GPU undo behavior');
});


test('prepared Canvas presentation preserves destination history through an engine switch',()=>{
  const context={setTransform(){},clearRect(){}},canvas={clientWidth:278,clientHeight:139,width:278,height:139,getContext(){return context;}};
  let invalidations=0;
  const options={report:{canvas_width:960,canvas_height:540},frame:{},onInvalidate(){invalidations++;},
    prepared:{renderer:{softwarePlayback:true,renderPreparedInto(){return true;}},key:'prepared'}};
  assert.equal(drawCameraMonitor(canvas,options),true);
  options.stage={drawCameraMonitor(target,view,afterSourcePaint){afterSourcePaint(true);return true;}};
  assert.equal(drawCameraMonitor(canvas,options),true);assert.equal(invalidations,0,'existing successful source paint is not a fresh destination');
});
