import test from 'node:test';
import assert from 'node:assert/strict';
import {emptyDocument,createFrame,createObject} from '../src/demo.mjs';
import {connectedTimelineGroups} from './helpers/legacy-connected-timeline.mjs';

function fixture({frames=30,parts=24}={}) {
  const report=emptyDocument(),figure=report.figure_types[0];
  report.keyframes=Array.from({length:frames},(_,i)=>createFrame(Array.from({length:parts},(_,slot)=>{
    const object=createObject(figure,slot+1,100+i,200);
    if(slot)object.join={target_object_slot:slot-1,target_unique_id:slot,target_handle_id:1};
    return object;
  }),3));
  return report;
}

test('cached native topology agrees with a full rebuild as authored joins appear and disappear',()=>{
  const report=fixture({parts:3}),native=connectedTimelineGroups({...report,_styk:{}});
  const frame=structuredClone(report.keyframes[0]);frame.objects.push({...createObject(report.figure_types[0],99,0,0),object_slot:3,join:{target_object_slot:2,target_handle_id:1}});
  report._styk.frameSources={new:frame};report._styk.animation={channels:[{property:'pose',keys:[{value:{frameId:'new'}}]}]};
  assert.deepEqual(connectedTimelineGroups(report,{nativeGroups:native}),connectedTimelineGroups(report));
  report._styk.animation.channels=[];
  assert.deepEqual(connectedTimelineGroups(report,{nativeGroups:native}),native);
});
