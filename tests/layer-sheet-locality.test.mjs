import test from 'node:test';
import assert from 'node:assert/strict';
import {ownedEditorFor} from './support/owned-editor.mjs';
import {emptyDocument,createFrame,createObject} from '../src/demo.mjs';
import {createLayerState} from '../src/layers.mjs';
import {sampleLayerSheet,validateLayerSheet,validateLayerSheetChanges} from '../src/layer-sheet.mjs';
import {snapshotPoseFrame} from '../src/pose-snapshot.mjs';
import {continuations,setContinuations} from '../src/movieclip-presence.mjs';

function fixture({layers=4,figures=8}={}){
  const report=emptyDocument(),figure=report.figure_types[0],state=createLayerState();
  state.order=Array.from({length:layers},(_,i)=>`layer-${i+1}`);
  state.byId=Object.fromEntries(state.order.map(id=>[id,{name:id,visible:true,locked:false}]));
  state.nextId=layers+1;
  for(let uid=1;uid<=figures;uid++)state.assignments[uid]=state.order[(uid-1)%layers];
  report._styk={...report._styk,layers:state};
  report.keyframes=[0,10,20,30,40].map(tick=>createFrame(Array.from({length:figures},(_,i)=>createObject(figure,i+1,100+tick+i,200)),10));
  return ownedEditorFor({demo:false,layerTimeline:true,propertyChannels:true}, report);
}

test('incremental validation preserves global identities and rejects invalid candidates atomically',()=>{
  const editor=fixture(),report=editor.report,state=report._styk.layerSheet;
  const candidate=change=>{const sheet={...state,tracks:{...state.tracks,'layer-1':{...state.tracks['layer-1'],keys:state.tracks['layer-1'].keys.map(key=>({...key}))}}};change(sheet);return {...report,_styk:{...report._styk,layerSheet:sheet}};};
  const corruptions=[
    sheet=>{sheet.tracks['layer-1'].keys[0].id=state.tracks['layer-2'].keys[0].id;},
    sheet=>{sheet.nextId=1;},
    sheet=>{delete sheet.tracks.camera;},
    sheet=>{sheet.tracks['layer-1'].end=sheet.duration+1;},
    sheet=>{sheet.tracks['layer-1'].keys.reverse();},
    sheet=>{delete sheet.tracks['layer-1'].keys[0].sourceTick;sheet.tracks['layer-1'].keys[0].value={frameId:'missing'};},
  ];
  for(const corrupt of corruptions){
    const invalid=candidate(corrupt);
    assert.throws(()=>validateLayerSheetChanges(invalid,report));assert.throws(()=>validateLayerSheet(invalid));
    assert.equal(validateLayerSheetChanges(report,report),report,'A failed candidate must leave the last valid ID index intact.');
  }
  editor.drawingResources.dispose();
});

test('shared native samples are partitioned once while preserving layer order, joins and continuations',()=>{
  const editor=fixture({layers:20,figures:200}),report=editor.report,native=structuredClone(report.keyframes[0]);
  native.object_draw_order=native.objects.map((_,i)=>i).reverse();
  native.objects[1].join={target_object_slot:0,target_unique_id:1,attachment:1};
  const original=structuredClone(native);
  const steps=report._styk.layers.order.map((id,i)=>({uid:i+1,typeId:1,playback:{clock:'presence'}}));setContinuations(native,steps);
  const owned=native.objects.filter(object=>report._styk.layers.assignments[object.unique_id]==='layer-1').map(object=>object.unique_id),pose=snapshotPoseFrame(native,owned);
  setContinuations(pose,steps.filter(step=>owned.includes(step.uid)));
  pose._styk={...pose._styk,frameId:'authored'};report._styk.frameSources={authored:pose};
  const track=report._styk.layerSheet.tracks['layer-1'];report._styk.layerSheet={...report._styk.layerSheet,tracks:{...report._styk.layerSheet.tracks,'layer-1':{...track,keys:[{...track.keys[0],sourceTick:undefined,value:{frameId:'authored'},interpolation:'constant'}]}}};
  let reads=0;const instrumented={...native,objects:native.objects.map(object=>new Proxy(object,{get(target,key,receiver){if(key==='unique_id')reads++;return Reflect.get(target,key,receiver);}}))};
  const sampled=sampleLayerSheet(report,0,{sampleLegacy:()=>instrumented,sampleNative:source=>source.keyframes[0],resolveGeometry:()=>{throw new Error('Unexpected geometry resolution.');}});
  assert.ok(reads<=200*8+20*3,`Shared-source composition used ${reads} identity reads.`);
  const expected=report._styk.layers.order.flatMap(id=>native.object_draw_order.map(slot=>native.objects[slot].unique_id).filter(uid=>report._styk.layers.assignments[uid]===id));
  assert.deepEqual(sampled.objects.map(object=>object.unique_id),expected);
  const child=sampled.objects.find(object=>object.unique_id===2);assert.equal(sampled.objects[child.join.target_object_slot].unique_id,1);
  assert.deepEqual(continuations(sampled),steps);assert.deepEqual(native.objects,original.objects);
  editor.drawingResources.dispose();
});

