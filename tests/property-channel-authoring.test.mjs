import test from 'node:test';
import assert from 'node:assert/strict';
import {newPlainEditor} from './support/plain-editor.mjs';
import {emptyDocument, createFrame, createObject} from '../src/demo.mjs';
import {validateAnimationDocument} from '../src/animation-channels.mjs';
import {storedFrameReport} from '../src/pivot-frame-draft.mjs';
import {evaluatedFrame} from '../src/pivot/geometry.mjs';
import {timeline} from '../src/pivot/read-view.mjs';
import {beginNativePropertyEdit, previewNativePropertyEdit, commitNativePropertyEdit} from '../src/native-property-edit.mjs';
import {sampleAnimationChannel} from '../src/animation-channels.mjs';
import {buildGeometry} from '../src/pivot/geometry.mjs';
import {buildChannelTimelineTracks} from './helpers/legacy-timeline-projection.mjs';

/** `prepare(report)` changes the document before it opens. */
function fixture(prepare = null) {
  const report = emptyDocument(), figure = report.figure_types[0];
  figure.segment_decode = {global_style:{outline_thickness:2,u30:0,rgb:[0,0,0]}};
  report.keyframes = [0,100,200].map(x => createFrame([createObject(figure,1,x,100),createObject(figure,2,x+300,100)],5));
  prepare?.(report);
  const editor = newPlainEditor({demo:false,propertyChannels:true}).load(report); editor.select(1); return editor;
}
const properties = editor => editor.animationChannels.map(channel => channel.property);
const numeric = (editor,key,value) => {const token=beginNativePropertyEdit(editor,key);previewNativePropertyEdit(token,value);commitNativePropertyEdit(token);};

test('timeline markers never sample frame values and scalar previews reuse their projection', () => {
  const e=fixture(),target={kind:'figure',uid:1};
  const projected=e.nativePropertyChannel(target,'opacity',{markersOnly:true});
  const tracks=buildChannelTimelineTracks(e.layers,e.animationChannels,e.report,new Set(),{
    nativePropertyChannel:(target,property)=>e.nativePropertyChannel(target,property,{markersOnly:true}),
    nativePoseChannel:target=>e.nativePoseChannel(target),figurePoses:true,
  });
  const row=tracks.byId.get('figure:1').channelChildren.find(row=>row.property==='opacity');
  assert.equal(row.keys.length,3);assert.equal(e._propertyChannelCache.frames.size,0);
  assert.equal(e._propertyChannelCache.values.size,0);
  e.beginAnimationEdit(target,'x');e.previewAnimationEdit(70);
  assert.strictEqual(e.nativePropertyChannel(target,'opacity',{markersOnly:true}),projected);
  e.cancelAnimationEdit();
  const owned=e.nativePropertyChannel(target,'opacity');
  assert.ok(owned.keys.every(key=>!Object.getOwnPropertyDescriptor(key,'value').get),'Persisted keys must be ordinary immutable data');
  e.updateObject({field28:128});assert.equal(e.evaluatedFrame.objects[0].field28,128);
});

test('figure appearance and transform edits author only their changed properties, with exact undo', () => {
  const e=fixture(), original=structuredClone(e.report.keyframes);
  e.updateObject({field28:128,color_override_raw:0xff0000ff});
  assert.deepEqual(properties(e).sort(),['color','opacity']);
  assert.deepEqual(e.report.keyframes,original);
  assert.equal(e.evaluatedFrame.objects[0].field28,128);
  assert.equal(e.evaluatedFrame.objects[1].field28,0);
  assert.equal(Object.keys(e.report._styk.frameSources??{}).length,0);
  e.undo();assert.equal(e.animationChannels.length,0);e.redo();
  e.moveSelection(30,0);assert.ok(!properties(e).includes('x'));assert.ok(properties(e).includes('pose'));
  assert.equal(e.evaluatedFrame.objects[0].root_x,30);
  validateAnimationDocument(e.report);
});

test('a skeleton key detaches figure properties and captures no changed opacity into Pose', () => {
  const e=fixture();e.select(1,1);e.beginDrag();e.updateObject({field28:128});e.updateSegment({length:80});e.commitDrag();
  const pose=e.animationChannels.find(c=>c.property==='pose');assert.ok(pose);
  const key=pose.keys.find(k=>k.tick===0), source=e.report._styk.frameSources[key.value.frameId];
  assert.equal(source.objects[0].field28,0);
  assert.equal(e.evaluatedFrame.objects[0].field28,128);
  assert.equal(e.animationChannel({kind:'figure',uid:1},'x'),null);
  const colors=e.animationChannel({kind:'figure',uid:1},'color');
  e.setAnimationKey(pose.id,key.id,{tick:2});
  assert.strictEqual(e.animationChannel({kind:'figure',uid:1},'color'),colors);
});

test('camera gestures and Scene edits have property keys without Pose sources', () => {
  const e=fixture(), frames=structuredClone(e.report.keyframes);
  e.setCameraValues({x:40,scale:2});e.setSceneColor(0xffff0000);
  e.beginCameraDrag();e.updateCameraDrag({y:20});e.commitCameraDrag();
  assert.equal(e.evaluatedFrame.camera.y,20);
  assert.ok(!properties(e).includes('pose'));assert.deepEqual(e.report.keyframes,frames);
  assert.equal(e.evaluatedFrame._stykBackgroundColor,0xffff0000);
  e.undo();assert.equal(e.evaluatedFrame.camera.y,0);
  const clean=fixture();clean.beginCameraDrag();clean.updateCameraDrag({x:20});clean.cancelCameraDrag();
  assert.equal(clean.report._styk.animation,undefined);
});

test('Scene background, color release, undo and redo retain scalar channel ownership', () => {
  const e=fixture();e.setSceneColor(0xffff0000);
  const asset={background_id:1,kind:2,color:0xff00ff00,name:'Green'};e.report.backgrounds=[...(e.report.backgrounds??[]),asset];e.setSceneBackground(asset.background_id);
  assert.equal(e.evaluatedFrame.background_id,asset.background_id);
  assert.equal(e.evaluatedFrame._stykBackgroundColor,undefined);
  assert.ok(!properties(e).includes('pose'));
  e.undo();assert.equal(e.evaluatedFrame._stykBackgroundColor,0xffff0000);
  e.redo();assert.equal(e.evaluatedFrame.background_id,asset.background_id);validateAnimationDocument(e.report);
});

test('numeric appearance edits use property history and keep the source definition unchanged', () => {
  const e=fixture(), original=structuredClone(e.figure);
  numeric(e,'outline_opacity',30);numeric(e,'outline_width',9);e.select(1,0);numeric(e,'segment_opacity',45);numeric(e,'thickness',14);
  assert.deepEqual(e.figure,original);assert.ok(!properties(e).includes('pose'));
  assert.equal(e.animationValue({kind:'figure',uid:1},'outlineWidth'),9);
  assert.equal(e.animationValue({kind:'segment',uid:1,handleId:1},'thickness'),14);
});

test('Simple Discard and Update carry the same independent property edits across mode changes', () => {
  const e=fixture();e.selectFrame(1);e.select(1);e.enableFrameDrafts({mode:'add'});numeric(e,'figure_opacity',50);
  assert.ok(e.frameDraft.dirty);assert.ok(!properties(e).includes('pose'));
  const stored=storedFrameReport(e.report);assert.equal(evaluatedFrame(stored,1,0,timeline(stored)[1].start).objects[0].field28,0);
  e.discardFrameDraft();assert.equal(e.evaluatedFrame.objects[0].field28,0);
  numeric(e,'figure_opacity',25);e.updateDraftFrame();e.disableFrameDrafts();e.selectFrame(1);
  assert.equal(e.evaluatedFrame.objects[0].field28,191);
  const reopened=newPlainEditor({demo:false,propertyChannels:true}).load(structuredClone(e.report));reopened.selectFrame(1);
  assert.equal(reopened.evaluatedFrame.objects[0].field28,191);
});

test('new Simple frames retain explicit appearance keys instead of returning those values to Pose', () => {
  const e=fixture();e.selectFrame(2);e.select(1);e.enableFrameDrafts({mode:'add'});numeric(e,'figure_opacity',25);e.addDraftFrame();
  const channel=e.animationChannel({kind:'figure',uid:1},'opacity'),key=channel.keys.find(k=>k.tick===e.tick);
  assert.ok(key&&!key.passthrough);assert.equal(e.evaluatedFrame.objects[0].field28,191);
  e.undo();assert.equal(e.report.keyframes.length,3);e.redo();assert.equal(e.report.keyframes.length,4);
});

test('range color edits are property-only and preserve native frames', () => {
  const e=fixture(),before=structuredClone(e.report.keyframes);
  e.updateFiguresInFrames({color_override_raw:0xff0000ff},[0,2]);
  assert.deepEqual(properties(e),['color']);assert.deepEqual(e.report.keyframes,before);e.seek(10);assert.equal(e.evaluatedFrame.objects[0].color_override_raw,0xff0000ff);
  e.undo();assert.equal(e.animationChannels.length,0);
});

test('detached imported curves retain native automatic easing, opacity bytes and shortest rotation', () => {
  const e=fixture(report=>{report.auto_easing=true;
    report.keyframes.forEach((frame,i)=>{frame.objects[0].field28=i*71;frame.objects[0].angles=frame.objects[0].angles.map(v=>v+i*2.8);});});
  for(const property of ['x','rotation','opacity','color']) {
    const channel=e.nativePropertyChannel({kind:'figure',uid:1},property);
    for(let tick=0;tick<10;tick++) {
      e.seek(tick);const value=e.animationValue({kind:'figure',uid:1},property);
      const sampled=sampleAnimationChannel(channel,tick);
      if(property==='rotation')assert.ok(Math.abs(Math.sin(value)-Math.sin(sampled))<1e-8);
      else assert.ok(Math.abs(value-sampled)<1e-8,`${property} at ${tick}: ${value} vs ${sampled}`);
    }
  }
});

test('parent rotation keeps joined children attached without manufacturing absolute child tracks', () => {
  const e=fixture();e.select(2);e.joinTo(1,1);e.select(1,1);e.updateSegment({length:90});
  assert.equal(e.animationChannel({kind:'figure',uid:2},'rotation'),null);
  const before=buildGeometry(e.report,e.evaluatedFrame).objects.get(e.evaluatedFrame.objects.findIndex(o=>o.unique_id===2)).segments[0].angle;
  e.select(1);const angle=e.animationValue({kind:'figure',uid:1},'rotation');e.addAnimationKey({kind:'figure',uid:1},'rotation',e.tick,angle+.4);
  const after=buildGeometry(e.report,e.evaluatedFrame).objects.get(e.evaluatedFrame.objects.findIndex(o=>o.unique_id===2)).segments[0].angle;
  assert.ok(Math.abs(after-before-.4)<1e-8);
});

