import test from 'node:test';
import assert from 'node:assert/strict';
import {Editor} from '../src/editor.mjs';
import {emptyDocument,createFrame,createObject} from '../src/demo.mjs';
import {buildGeometry} from '../src/pivot/geometry.mjs';
import {buildChannelTimelineTracks} from './helpers/legacy-timeline-projection.mjs';
import {connectedTimelineGroups} from './helpers/legacy-connected-timeline.mjs';
import {plainEditorFor} from './support/plain-editor.mjs';

function fixture({frames=30,parts=24,plain=false}={}) {
  const report=emptyDocument(),figure=report.figure_types[0];
  report.keyframes=Array.from({length:frames},(_,i)=>createFrame(Array.from({length:parts},(_,slot)=>{
    const object=createObject(figure,slot+1,100+i,200);
    if(slot)object.join={target_object_slot:slot-1,target_unique_id:slot,target_handle_id:1};
    return object;
  }),3));
  const editor=plain?new Editor({demo:false,propertyChannels:true}).load(report):plainEditorFor({demo:false,propertyChannels:true}, report);editor.select(1);return editor;
}
function move(e){e.beginTranslationDrag();e.moveSelection(20,5);e.commitDrag();}

test('cached native topology agrees with a full rebuild as authored joins appear and disappear',()=>{
  const e=fixture({parts:3}),native=connectedTimelineGroups({...e.report,_styk:{}});
  const frame=structuredClone(e.frame);frame.objects.push({...createObject(e.figure,99,0,0),object_slot:3,join:{target_object_slot:2,target_handle_id:1}});
  e.report._styk.frameSources={new:frame};e.report._styk.animation={channels:[{property:'pose',keys:[{value:{frameId:'new'}}]}]};
  assert.deepEqual(connectedTimelineGroups(e.report,{nativeGroups:native}),connectedTimelineGroups(e.report));
  e.report._styk.animation.channels=[];
  assert.deepEqual(connectedTimelineGroups(e.report,{nativeGroups:native}),native);
});

test('focused projection lazily resolves a connected group without constructing unrelated groups',()=>{
  const e=fixture({parts:4}),groups=connectedTimelineGroups(e.report);
  const project=buildChannelTimelineTracks(e.layers,[],e.report,new Set(),{focused:true,connectedGroups:groups,
    figurePoses:true,nativePoseChannel:target=>e.nativePoseChannel(target)});
  assert.equal(project.assemblyByUid.size,0);assert.equal(e._nativeAnimationChannels,null);
  const group=project.assemblyByUid.get(4);
  assert.equal(group.id,'assembly:1');assert.equal(project.byId.get(group.id),group);
  assert.equal(project.byId.get('assembly:1:pose').kind,'assembly-property');
  assert.equal(project.assemblyByUid.get(1),group);
});

