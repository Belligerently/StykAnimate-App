import test from 'node:test';
import assert from 'node:assert/strict';
import {Editor} from '../src/editor.mjs';
import {adoptLayerSheet,drawingSheetPlan,drawingSheetPublish} from '../src/editor-layer-sheet.mjs';
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
  return new Editor({demo:false,layerTimeline:true,propertyChannels:true}).load(report);
}

test('one layer key edit retains untouched keys and immutable history endpoints',()=>{
  const editor=fixture(),before=editor.report._styk.layerSheet,original=structuredClone(before),other=before.tracks['layer-2'];
  for(const track of Object.values(before.tracks))for(const key of track.keys)Object.freeze(key);
  editor.sheetInsertKey(['layer-1'],5);
  const changed=editor.report._styk.layerSheet.tracks['layer-1'];
  assert.equal(editor.report._styk.layerSheet.tracks['layer-2'],other);
  for(const key of before.tracks['layer-1'].keys.filter(key=>key.tick>=20))assert.equal(changed.keys.find(value=>value.id===key.id),key);
  assert.deepEqual(before,original);
  const after=editor.serialize();editor.undo();assert.equal(editor.report._styk.layerSheet,before);editor.redo();assert.deepEqual(editor.serialize(),after);
  editor.drawingResources.dispose();
});

test('retiming, easing and frame replacement copy frozen keys before mutation',()=>{
  const actions=[
    editor=>editor.sheetSetEasing(['layer-1'],10,10,{interpolation:'constant'}),
    editor=>editor.sheetInsertFrames(['layer-1'],10,3),
    editor=>editor.sheetRemoveFrames(['layer-1'],10,12),
    editor=>editor.sheetDuplicateFrameAfter(['layer-1'],10),
    editor=>editor.sheetMoveFrames(['layer-1'],10,20,5),
    editor=>editor.sheetMoveFrames(['layer-1'],10,20,5,{copy:true}),
    editor=>{editor.sheetCopyFrames(['layer-1'],0,10);editor.sheetPasteFrames(['layer-1'],15,{insert:true});},
  ];
  for(const action of actions){
    const editor=fixture(),before=editor.serialize(),state=editor.report._styk.layerSheet,original=structuredClone(state);
    for(const track of Object.values(state.tracks))for(const key of track.keys)Object.freeze(key);
    action(editor);assert.deepEqual(state,original);const after=editor.serialize();
    editor.undo();assert.deepEqual(editor.serialize(),before);editor.redo();assert.deepEqual(editor.serialize(),after);
    editor.drawingResources.dispose();
  }
});

test('materializing a retained key twice reuses its owned copy and source',()=>{
  const editor=fixture(),before=structuredClone(editor.report._styk.layerSheet),plan=drawingSheetPlan(editor,['layer-1']),retained=plan.get('layer-1').keys[0];
  const first=plan.materialize('layer-1',retained),second=plan.materialize('layer-1',retained);
  assert.equal(second,first);assert.equal(plan.sources.length,1);assert.equal(plan.get('layer-1').keys[0],first);
  assert.deepEqual(editor.report._styk.layerSheet,before);editor.drawingResources.dispose();
});

test('local publication skips unchanged key validation while explicit validation still traverses it',()=>{
  const editor=fixture(),key=editor.report._styk.layerSheet.tracks['layer-2'].keys.at(-1),easing=key.interpolation;
  let reads=0;Object.defineProperty(key,'interpolation',{enumerable:true,configurable:true,get(){reads++;return easing;}});
  editor.sheetInsertKey(['layer-1'],5);
  assert.equal(reads,0,'An unrelated far-away key must not be revalidated by a local command.');
  editor.undo();editor.redo();editor.undo();editor.sheetInsertKey(['layer-1'],6);
  assert.equal(reads,0,'Undo/source-table removal must not force validation of unrelated imported keys.');
  validateLayerSheet(editor.report);assert.ok(reads>0,'Explicit document validation must remain complete.');
  editor.drawingResources.dispose();
});

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

test('replacing the source dictionary forces validation of retained track references',()=>{
  const editor=fixture();editor.sheetInsertKey(['layer-1'],5);validateLayerSheet(editor.report);
  editor.report._styk.frameSources={};
  assert.throws(()=>validateLayerSheetChanges(editor.report,editor.report),/missing pose/i);
  editor.drawingResources.dispose();
});

test('replacing an existing source rechecks retained patch replacement identities',()=>{
  const editor=fixture();editor.sheetInsertKey(['layer-1'],5);
  const key=editor.report._styk.layerSheet.tracks['layer-1'].keys.find(key=>key.tick===5),id=key.value.frameId;
  key.movieclipPatches=[{uid:1,frameId:id,replacementUid:1}];validateLayerSheet(editor.report);
  const source=editor.report._styk.frameSources[id];editor.report._styk.frameSources[id]={...source,objects:source.objects.filter(object=>object.unique_id!==1)};
  assert.throws(()=>validateLayerSheetChanges(editor.report,editor.report),/replacement/i);
  assert.throws(()=>validateLayerSheet(editor.report),/replacement/i);
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

test('adopting a prepared sheet skips endpoint filtering and preserves publication and history',()=>{
  const [expected,adopted]=[fixture(),fixture()];
  for(const editor of [expected,adopted]){
    editor.report._styk.custom={note:'retained metadata',values:[1,2,3]};
    editor.report.keyframes[0].objects[0].custom={note:'retained pose'};
    editor.report._styk.layerSheet.tracks['layer-1'].keys[0].custom={note:'retained key'};
  }
  const before=adopted.serialize(),state=adopted.report._styk.layerSheet,unchanged=state.tracks['layer-2'];
  const events=[[],[]];
  for(const [index,editor] of [expected,adopted].entries())editor.addEventListener('change',event=>events[index].push(structuredClone(event.detail)));
  expected.sheetInsertKey(['layer-1'],5);
  const command=expected._undo.at(-1),sheet=command.after;
  const tracks=Object.fromEntries(Object.entries(sheet.tracks).filter(([id,track])=>track!==command.before.tracks[id]));
  const sources=Object.fromEntries(command.frameSourceChanges.map(change=>[change.id,change.after.value]));
  let filters=0;
  const keys=[state.tracks['layer-1'].keys,tracks['layer-1'].keys];
  for(const array of keys)Object.defineProperty(array,'filter',{configurable:true,value:function(...args){filters++;return Array.prototype.filter.apply(this,args);}});
  try{
    adoptLayerSheet(adopted,{sheet:{...sheet,tracks},trackOrder:Object.keys(sheet.tracks),layers:command.afterLayers,
      sources,nextFrame:expected._nextFrameIdentity},command.label);
  }finally{for(const array of keys)delete array.filter;}
  assert.equal(filters,0,'A prepared adoption has no imported endpoints left to materialize.');
  assert.equal(adopted.report._styk.layerSheet.tracks['layer-2'],unchanged);
  assert.deepEqual(adopted.serialize(),expected.serialize());
  assert.deepEqual(adopted._undo,expected._undo);
  assert.equal(adopted.historySize,expected.historySize);
  assert.deepEqual(adopted._selectionState(),expected._selectionState());
  assert.deepEqual(events[1],events[0]);
  for(const tick of [0,4,5,9,10,20]){
    expected.seek(tick);adopted.seek(tick);
    assert.deepEqual(adopted.evaluatedFrame,expected.evaluatedFrame);
    assert.deepEqual(adopted.geometry.drawables,expected.geometry.drawables);
  }
  expected.seek(0);adopted.seek(0);
  const after=adopted.serialize();
  for(const editor of [expected,adopted]){editor.select(1);editor.beginTranslationDrag('Cancelled move');editor.moveSelection(4,2);editor.cancelDrag();}
  assert.deepEqual(adopted.serialize(),after);assert.deepEqual(adopted._undo,expected._undo);
  adopted.undo();assert.deepEqual(adopted.serialize(),before);adopted.redo();assert.deepEqual(adopted.serialize(),after);
  const valid=adopted.serialize(),history=adopted.historySize,nextFrame=adopted._nextFrameIdentity;
  const invalid=structuredClone(adopted.report._styk.layerSheet);
  invalid.tracks['layer-1'].keys[0].id=invalid.tracks.camera.keys[0].id;
  assert.throws(()=>adoptLayerSheet(adopted,{sheet:invalid,trackOrder:Object.keys(invalid.tracks),layers:adopted.report._styk.layers,
    nextFrame},'Invalid prepared sheet'),/unique identities/i);
  assert.deepEqual(adopted.serialize(),valid);assert.equal(adopted.historySize,history);assert.equal(adopted._nextFrameIdentity,nextFrame);
  for(const editor of [expected,adopted])editor.drawingResources.dispose();
});

test('ordinary plans still materialize changed imported endpoints before publication',()=>{
  const editor=fixture(),before=editor.serialize(),plan=drawingSheetPlan(editor,['layer-1']);
  const key=plan.ownKey('layer-1',plan.get('layer-1').keys[1]);key.interpolation='constant';
  const materialize=plan.materialize;let calls=0;
  plan.materialize=(...args)=>{calls++;return materialize(...args);};
  drawingSheetPublish(editor,plan,'Change imported easing');
  assert.equal(calls,2);assert.equal(plan.sources.length,2);
  const changed=editor.report._styk.layerSheet.tracks['layer-1'].keys;
  assert.equal(changed[0].sourceTick,0);
  for(const endpoint of changed.slice(1,3)){
    assert.equal(Object.hasOwn(endpoint,'sourceTick'),false);assert.ok(endpoint.value.frameId);
  }
  const after=editor.serialize();editor.undo();assert.deepEqual(editor.serialize(),before);editor.redo();assert.deepEqual(editor.serialize(),after);
  editor.drawingResources.dispose();
});
