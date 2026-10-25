import test from 'node:test';
import assert from 'node:assert/strict';
import {ownedEditorFor} from './support/owned-editor.mjs';
import {emptyDocument,createFrame,createObject} from '../src/demo.mjs';
import {buildGeometry} from '../src/pivot/geometry.mjs';
import {sampleSheet} from './helpers/sheet-samples.mjs';

function fixture({frames=80,neighbors=1,duration=1}={}){
  const document=emptyDocument(),figure=document.figure_types[0];
  document.keyframes=Array.from({length:frames},(_,i)=>createFrame([
    createObject(figure,1,100+Math.sin(i)*20,200),
    ...Array.from({length:neighbors},(_,n)=>createObject(figure,n+2,300+n*40,200+i,{unrelatedPayload:'outside-capture'.repeat(200)})),
  ],duration));
  return ownedEditorFor({demo:false,layerTimeline:true}, document);
}
const shapes=(report,tick)=>buildGeometry(report,sampleSheet(report,tick)).drawables.map(d=>({type:d.type,rgb:d.rgb,alpha:d.alpha,points:d.points.map(p=>[p.x,p.y])}));
function close(actual,expected){
  assert.equal(actual.length,expected.length);
  actual.forEach((a,i)=>{
    const b=expected[i];assert.equal(a.type,b.type);assert.deepEqual(a.rgb,b.rgb);assert.equal(a.alpha,b.alpha);assert.equal(a.points.length,b.points.length);
    a.points.forEach((p,j)=>p.forEach((n,k)=>assert.ok(Math.abs(n-b.points[j][k])<1e-5,`${i}/${j}/${k}: ${n} != ${b.points[j][k]}`)));
  });
}

test('conversion boundaries preserve fractional eased poses, stacking, reopen and exact undo',()=>{
  const e=fixture({frames:4,neighbors:2,duration:10});
  e.sheetSetEasing(['layer-1'],0,39,{interpolation:'bezier',bezier:[.15,.8,.6,.2]});
  e.seek(10);e.select(2);e.moveSelection(-35,12);
  const before=e.serialize(),samples=Array.from({length:e.totalTicks*4},(_,i)=>shapes(e.report,i/4));
  e.seek(3);e._selectIds([1,2]);e.createMovieclip({start:3,end:24});
  for(let i=0;i<samples.length;i++)close(shapes(e.report,i/4),samples[i]);
  const reopened=ownedEditorFor({demo:false,layerTimeline:true},e.serialize());
  for(let i=0;i<samples.length;i++)close(shapes(reopened.report,i/4),samples[i]);
  e.undo();assert.deepEqual(e.serialize(),before);
  e.redo();for(const tick of [0,2.5,3,12.5,23.75,24,30])close(shapes(e.report,tick),samples[tick*4]);
});

test('a locked replacement layer rejects creation without publishing a partial definition',()=>{
  const e=fixture();e.select(1);e.setLayerLocked('layer-1',true);
  const before=e.serialize(),history=e._undo.length,bytes=e.historySize;
  assert.throws(()=>e.createMovieclip({start:3,end:17}),/Unlock/);
  assert.deepEqual(e.serialize(),before);assert.equal(e._undo.length,history);assert.equal(e.historySize,bytes);
});
