import test from 'node:test';
import assert from 'node:assert/strict';
import {ownedEditorSync, ownedEditorFor} from './support/owned-editor.mjs';
import {ownerReferences} from './support/owner-references.mjs';
import {packMotionLanes} from '../src/camera-motion-timeline.mjs';
import {motionRowSize,motionLaneBox,sheetRowLayout} from '../src/timeline-rows.mjs';
import {cameraMatrix} from '../src/renderer-geometry.mjs';
import {validateAnimationDocument} from '../src/animation-channels.mjs';
import {snapshotPoseFrame} from '../src/pose-snapshot.mjs';
import {pivotDocument} from './support/engine-export.mjs';
const refs=ownerReferences(import.meta.url);
const near=(a,b)=>assert.ok(Math.abs(a-b)<.002,`${a} != ${b}`);
function fixture(){const e=ownedEditorFor({demo:false,layerTimeline:true});e.addFigure();return e;}
/** The same document (built by a plain editor, once: its reference) with the engine owning the editor: baking the camera's motion is a command of the engine session. */
function ownedFixture(take){return ownedEditorSync({demo:false},JSON.parse(take('fixture')),{recorded:false}).editor;}
function at(e,tick){e.seek(tick);return e.evaluatedFrame;}
/** The camera the frame at `tick` is shot through (follow travel, then the presets' impulse): the session's `shot`. */
function shotAt(e,tick){const frame=at(e,tick);return e._shown.shot()??frame.camera;}
/** What a Pivot export stores per frame once the camera's motion and the layers' parallax are baked into it (the engine's export). */
const stored=e=>pivotDocument(e).keyframes.map(f=>({camera:{x:f.camera?.x??0,y:f.camera?.y??0,scale:f.camera?.scale??1,angle_rad:f.camera?.angle_rad??0},roots:f.objects.map(o=>[o.root_x,o.root_y])}));
function sameStored(actual,expected){
  assert.equal(actual.length,expected.length);
  actual.forEach((frame,i)=>{for(const k of ['x','y','scale','angle_rad'])near(frame.camera[k],expected[i].camera[k]);frame.roots.forEach((root,j)=>{near(root[0],expected[i].roots[j][0]);near(root[1],expected[i].roots[j][1]);});});
}
test('motions that never play together share a timeline lane; overlapping ones stack',()=>{
  const clip=(id,start,duration)=>({id,start,duration}),lanes=clips=>packMotionLanes(clips).map(lane=>lane.map(c=>c.id));
  assert.deepEqual(lanes([]),[]);
  assert.deepEqual(lanes(Array.from({length:10},(_,i)=>clip(`m${i}`,i*20,12))),[['m0','m1','m2','m3','m4','m5','m6','m7','m8','m9']]);
  // A clip may start on the frame the one before it ends; clips come in start order whatever their saved order.
  assert.deepEqual(lanes([clip('late',12,4),clip('early',0,12),clip('long',6,40),clip('third',10,3)]),[['early','late'],['long'],['third']]);
});
test('the Motion row is a content row for one lane and grows a 14px sub-lane per overlapping motion',()=>{
  assert.equal(motionRowSize(1),32);assert.equal(motionRowSize(1,{row:28}),28);assert.equal(motionRowSize(3),50);assert.equal(motionRowSize(2,{touch:true}),60);
  // Sub-lanes split the row between 4px margins with a 2px gap; one lane keeps today's inset pill.
  assert.deepEqual(motionLaneBox(100,50,0,3),{top:104,height:12});assert.deepEqual(motionLaneBox(100,50,2,3),{top:132,height:12});
  assert.deepEqual(motionLaneBox(0,32,0,1),{top:5,height:22});assert.deepEqual(motionLaneBox(0,48,0,1),{top:8,height:32});
  const layout=sheetRowLayout([{kind:'camera'},{kind:'motion',laneCount:3},{kind:'scene'},{kind:'layer'}],{viewport:400});
  assert.equal(layout.size(1),50);assert.equal(layout.top(2),74);assert.equal(layout.at(30),1);assert.equal(layout.pinned,true);
});
test('motion rejects invalid input atomically and only retains camera metadata in history',()=>{
  const e=fixture(),objects=e.report.keyframes.map(f=>f.objects),before=e.serialize(),history=e._undo.length;
  const id=e.addCameraMotion('impact',{start:5,duration:12});assert.equal(e._undo.length,history+1);assert.ok(e.totalTicks>=17);assert.ok(e._undo.at(-1).bytes<12000);
  assert.deepEqual(e.report.keyframes.map(f=>f.objects),objects);
  const state=e.report._styk.cameraMotion;assert.throws(()=>e.setCameraMotion(id,{params:{zoom:-1}}));assert.equal(e.report._styk.cameraMotion,state);
  e.undo();assert.deepEqual(e.serialize(),before);e.redo();assert.equal(e.getCameraMotions()[0].id,id);
});
test('motion changes output camera but never authored pose; hidden camera bypasses it',()=>{
  const e=fixture();e.addCameraMotion('impact',{start:0,duration:12});const f=at(e,0),base={...f.camera},shot={...f,camera:shotAt(e,0)},matrix=cameraMatrix({report:e.report,frame:shot});
  assert.notDeepEqual(matrix,[1,0,0,1,0,0]);assert.deepEqual(f.camera,base);
  e.setLayerVisible('camera',false);assert.deepEqual(cameraMatrix({report:e.report,frame:shot}),[1,0,0,1,0,0]);
});
test('Pivot materialization bakes motion and parallax once without changing editable project',()=>{
  const e=fixture();e.setLayerParallax(e.activeLayerId,{enabled:true,x:.5});e.setCameraValues({x:60});e.addCameraMotion('impact',{start:0,duration:12});
  const before=e.serialize(),frame=at(e,0),out=pivotDocument(e.report);near(out.keyframes[0].objects[0].root_x,frame.objects[0].root_x+30);
  near(out.keyframes[0].camera.x,shotAt(e,0).x);assert.equal(out._styk.cameraMotion,undefined);assert.deepEqual(e.serialize(),before);
});
refs.test('baking retains camera output and parallax at every output tick and undoes as one action',take=>{
  const e=ownedFixture(take),layer=e.activeLayerId;e.setLayerParallax(layer,{enabled:true,x:.3});e.setCameraValues({x:70});e.addCameraMotion('impact',{duration:12});e.addCameraMotion('wobble',{duration:24});
  const before=e.serialize(),history=e._undo.length,samples=Array.from({length:e.totalTicks},(_,t)=>shotAt(e,t)),exported=stored(e);
  e.bakeCameraMotion();assert.equal(e._undo.length,history+1);assert.ok(e.getCameraMotions().every(c=>!c.enabled));
  samples.forEach((camera,t)=>{const baked=shotAt(e,t);for(const k of ['x','y','scale','angle_rad'])near(baked[k],camera[k]);});
  sameStored(stored(e),exported);
  e.undo();assert.deepEqual(e.serialize(),before);e.redo();validateAnimationDocument(e.report);
});
test('pose snapshots omit the derived motion clock and retain the authored camera',()=>{
  const e=fixture();e.addCameraMotion('impact');const frame=at(e,3),saved=snapshotPoseFrame(frame);assert.equal(saved._cameraMotionTick,undefined);assert.deepEqual(saved.camera,frame.camera);
});
for(const interpolation of ['constant','linear','bezier','auto'])refs.test(`partial bake preserves all output ticks across ${interpolation} camera spans`,take=>{
  const e=ownedFixture(take);e.addCameraMotion('handheld',{duration:60});e.addCameraMotion('impact',{start:5,duration:30});
  e.setLayerParallax(e.activeLayerId,{enabled:true,x:.2});
  for(const [tick,x] of [[0,0],[20,100],[40,240]]){e.sheetInsertKey(['camera'],tick);e.seek(tick);e.setCameraValues({x});}
  e.sheetSetEasing(['camera'],0,40,{interpolation,...(interpolation==='bezier'?{bezier:[.3,0,.8,1]}:{})});
  const sample=()=>Array.from({length:e.totalTicks},(_,tick)=>shotAt(e,tick));
  const before=e.serialize(),expected=sample(),exported=stored(e);e.bakeCameraMotion({start:8,end:14});validateAnimationDocument(e.report);
  const actual=sample();expected.forEach((camera,t)=>{for(const k of ['x','y','scale','angle_rad'])near(camera[k],actual[t][k]);});sameStored(stored(e),exported);
  e.bakeCameraMotion({start:45,end:49});sample().forEach((camera,t)=>{for(const k of ['x','y','scale','angle_rad'])near(camera[k],expected[t][k]);});sameStored(stored(e),exported);
  e.undo();e.undo();assert.deepEqual(e.serialize(),before);
});
refs.test('camera and layer locks reject motion, baking and parallax changes atomically',take=>{
  const e=ownedFixture(take),id=e.addCameraMotion('shake');e.setLayerLocked('camera',true);e.setLayerLocked(e.activeLayerId,true);const before=e.serialize();
  assert.throws(()=>e.setCameraMotion(id,{enabled:false}),/Unlock/);assert.throws(()=>e.addCameraMotion('impact'),/Unlock/);
  assert.throws(()=>e.setLayerParallax(e.activeLayerId,{enabled:true}),/Unlock/);assert.throws(()=>e.bakeCameraMotion(),/Unlock/);assert.deepEqual(e.serialize(),before);
});
