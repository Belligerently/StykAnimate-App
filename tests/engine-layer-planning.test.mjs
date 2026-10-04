import {readFileSync} from 'node:fs';
import test from 'node:test';
import assert from 'node:assert/strict';
import {planEngineLayerMigration} from '../src/engine/engine-layer-planning.mjs';
import {documentChunks} from '../src/document-transport.mjs';
import {Editor} from '../src/editor.mjs';
import {adoptLayerSheet} from '../src/editor-layer-sheet.mjs';
const fixture = name => JSON.parse(readFileSync(new URL(`../engine/tests/layer-migration/${name}.json`, import.meta.url),'utf8'));
const nativeJson = value => JSON.stringify(value, (key,value) => key === 'runtime_segments' ? undefined : value);
function transport(payload, onTake = () => {}) {
  const chunks = documentChunks(payload, {maxBytes: 64*1024,maxRecords:128,timeBudgetMs:2});
  let sequence = 0, awaiting, stopped = false, freed = false, reads = 0;
  const job = {
    take() {
      reads++; onTake(reads);
      if (awaiting !== undefined || stopped) return undefined;
      const next = chunks.next();
      if (next.done) return JSON.stringify({done:true});
      awaiting = sequence++; return JSON.stringify({sequence:awaiting,chunk:next.value,progress:{phase:'sampling',completed:1,total:1}});
    },
    ack(seq) { assert.equal(seq,awaiting); awaiting=undefined; return true; },
    cancel() { stopped=true; }, free() { freed=true; },
  };
  const engine = {threaded:true,workers:1,progressSeen:()=>0,whenProgress:async()=>{},handle:{plan_layers:(revision,request)=>{assert.equal(typeof revision,'number');const ids=JSON.parse(request);assert.deepEqual(Object.keys(ids).sort(),Object.hasOwn(ids,'deletedLayerId')?['deletedLayerId','nextFrameIdentity','targetLayerId','uids']:['nextFrameIdentity','targetLayerId','uids']);return job;}}};
  return {engine, get stopped(){return stopped;}, get freed(){return freed;}};
}
for (const name of ['native-only','independent','joined-scalars','delete-default','dense-single-figure']) test(`worker adapter preserves actual async editor publication and history: ${name}`,async()=>{
  const reference=fixture(name),editor=new Editor({demo:false}).load(JSON.parse(reference.document));
  // Loading already complete IDs preserves the report bytes; revision is host state.
  editor._revision=editor._revisionSerial=reference.revision;
  editor._restoreSelection(reference.initialState.selection);editor._layerRevision=reference.initialState.layerRevision;
  const state=transport(reference.payload);
  // This boundary test uses captured worker packets. Native Rust tests separately
  // compare the actual worker planner and sliced packets to these editor bytes.
  state.engine.handle.plan_layers=(revision,request)=>{
    assert.equal(revision,editor.revision);assert.deepEqual(JSON.parse(request),reference.request);
    const iterator=documentChunks(reference.payload,{maxBytes:64*1024,maxRecords:128,timeBudgetMs:2});let sequence=0,awaiting;
    return {take(){if(awaiting!==undefined)return undefined;const next=iterator.next();if(next.done)return JSON.stringify({done:true});awaiting=sequence++;return JSON.stringify({sequence:awaiting,chunk:next.value});},ack(seq){assert.equal(seq,awaiting);awaiting=undefined;return true;},cancel(){},free(){}};
  };
  const events=[];editor.addEventListener('change',event=>events.push(event.detail));
  const revision=editor.revision,report=editor.report,identity=editor._nextFrameIdentity;
  const planMigration=(request,options)=>planEngineLayerMigration(state.engine,revision,request,{...options,isCurrent:()=>editor.report===report&&editor.revision===revision&&editor._nextFrameIdentity===identity});
  const result=reference.command.name==='deleteLayerAsync'?await editor.deleteLayerAsync(reference.request.deletedLayerId,{targetLayerId:reference.request.targetLayerId,planMigration})
    :await editor.assignFiguresToLayerAsync(reference.request.uids,reference.request.targetLayerId,{planMigration});
  assert.deepEqual(result,reference.command.result);assert.equal(nativeJson(editor.report),reference.command.after);assert.deepEqual(events,reference.command.events);
  assert.equal(editor.undoLabel,reference.command.state.undoLabel);editor.undo();assert.equal(nativeJson(editor.report),reference.command.undone);editor.redo();assert.equal(nativeJson(editor.report),reference.command.redone);
});
test('adapter requires real workers before queueing any planning',async()=>{
  let called=false;await assert.rejects(planEngineLayerMigration({threaded:false,workers:0,handle:{plan_layers(){called=true;}}},1,{}),/background engine workers/);assert.equal(called,false);
});
for(const reason of ['cancel','stale','invalid'])test(`adapter rejects ${reason} packets and disposes its small handle`,async()=>{
  const reference=fixture('independent'),editor=new Editor({demo:false}).load(JSON.parse(reference.document)),before=nativeJson(editor.report),controller=new AbortController();let current=true;
  const state=transport(reference.payload,reads=>{if(reads===2){if(reason==='cancel')controller.abort();else if(reason==='stale')current=false;}});
  if(reason==='invalid')state.engine.handle.plan_layers=()=>({take:()=>JSON.stringify({done:true}),cancel(){},free(){}});
  await assert.rejects(planEngineLayerMigration(state.engine,1,{uids:[1],targetLayerId:'layer-2',nextFrameIdentity:3},{signal:controller.signal,isCurrent:()=>current}),reason==='cancel'?{name:'AbortError'}:reason==='stale'?/document changed/:/invalid layer planning payload/);
  if(reason!=='invalid'){assert.equal(state.stopped,true);assert.equal(state.freed,true);}assert.equal(nativeJson(editor.report),before);assert.equal(editor.canUndo,false);
});
test('actual async editor rolls back if cancelled after a bounded packet',async()=>{
  const reference=fixture('independent'),editor=new Editor({demo:false}).load(JSON.parse(reference.document)),before=nativeJson(editor.report),controller=new AbortController();
  const state=transport(reference.payload,reads=>{if(reads===2)controller.abort();});
  const planMigration=(request,options)=>planEngineLayerMigration(state.engine,editor.revision,request,options);
  await assert.rejects(editor.assignFiguresToLayerAsync([1],'layer-2',{signal:controller.signal,planMigration}),{name:'AbortError'});
  assert.equal(nativeJson(editor.report),before);assert.equal(editor.canUndo,false);assert.equal(state.freed,true);
});
test('actual async editor preserves a concurrent edit when a plan becomes stale',async()=>{
  const reference=fixture('independent'),editor=new Editor({demo:false}).load(JSON.parse(reference.document)),revision=editor.revision;let expected;
  const state=transport(reference.payload,reads=>{if(reads===2){editor.select(1);editor.moveSelection(5,0);expected=nativeJson(editor.report);}});
  const planMigration=(request,options)=>planEngineLayerMigration(state.engine,revision,request,{...options,isCurrent:()=>editor.revision===revision});
  await assert.rejects(editor.assignFiguresToLayerAsync([1],'layer-2',{planMigration}),/document changed/);
  assert.equal(nativeJson(editor.report),expected);assert.equal(editor.layerForUid(1),'layer-1');assert.equal(editor.canUndo,true);assert.equal(state.freed,true);
});

for(const name of ['shared','vacant','vacant-tail','combined','absence','delete-unused','delete-native','delete-joined','dense','noop','movieclip','delete-movieclip','view-changed'])test(`bounded sheet adapter payload reaches actual publication/history owner: ${name}`,async()=>{
  const reference=JSON.parse(readFileSync(new URL(`../engine/tests/layer-sheet-migration/${name}.json`,import.meta.url),'utf8'));
  const editor=new Editor({demo:false,layerTimeline:true,propertyChannels:false}).load(JSON.parse(reference.document));
  editor._restoreSelection(reference.initialState.selection);editor._layerRevision=reference.initialState.layerRevision;
  const before=editor.report,revision=editor.revision,identity=editor._nextFrameIdentity,state=transport(reference.payload);
  let advanced=false;const onProgress=name==='view-changed'?()=>{if(!advanced){advanced=true;editor.seek(3);editor.select(2);}}:undefined;
  const events=[];editor.addEventListener('change',event=>events.push(event.detail));
  const plan=await planEngineLayerMigration(state.engine,revision,reference.request,{onProgress,isCurrent:()=>editor.report===before&&editor.revision===revision&&editor._nextFrameIdentity===identity});
  assert.equal(nativeJson(editor.report),name==='view-changed'?reference.command.undone:reference.document,'streaming a plan never changes document content; permitted live view survives');
  // The existing JS owner remains the publication oracle in this boundary test.
  // Application adoption awaits the Rust worker publication/history protocol;
  // this test makes no claim that the existing synchronous oracle is D7-ready.
  const result=plan?adoptLayerSheet(editor,plan,name.startsWith('delete-')?'Delete layer':'Move figures to layer'):false;
  assert.equal(result,reference.command.result);assert.equal(nativeJson(editor.report),reference.command.after);
  assert.deepEqual(events,reference.command.events);assert.equal(editor.canUndo,reference.command.state.canUndo);
  editor.undo();assert.equal(nativeJson(editor.report),reference.command.undone);editor.redo();assert.equal(nativeJson(editor.report),reference.command.redone);
  assert.equal(state.stopped,true);assert.equal(state.freed,true);
});
