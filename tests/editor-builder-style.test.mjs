import test from 'node:test';
import assert from 'node:assert/strict';
import {Editor} from '../src/editor.mjs';
import {editorBuilderMethods} from '../src/editor-builder.mjs';
import {builderDrawables,builderStyleTargets,reorderBuilderDrawables} from '../src/editor-builder-style.mjs';

import {defaultFigure,createObject} from '../src/demo.mjs';
import {validateScene} from '../src/pivot/model.mjs';
import {roundTrip} from './support/codec.mjs';
import {plainEditorFor} from './support/plain-editor.mjs';
Object.assign(Editor.prototype,editorBuilderMethods);
/** `change(definition)` writes into the figure's definition before the document opens; `plain` leaves the editor to itself. */
function fixture({change,plain=false}={}){
  const base=plainEditorFor({demo:false}),figure=defaultFigure(201);
  figure.name='Order fixture';
  figure.segments=figure.segments.slice(0,4).map((segment,index)=>({...segment,segment_id:[11,22,33,44][index],parent_id:[0,11,22,0][index],z_order:[1,3,5,6][index]}));
  figure.segment_count=4;
  figure.polyfills=[{polyfill_id:1,segment_ids:[0,11,22],segment_ids_raw:[0,11,22],color_rgb:[20,80,200],color_alpha:1,alpha_raw:0,z_order:2},
    {polyfill_id:2,segment_ids:[0,22,44],segment_ids_raw:[0,22,44],color_rgb:[220,80,20],color_alpha:1,alpha_raw:0,z_order:4}];
  base.importFigure(figure);
  const uid=base.selectedUid,document=base.serialize(),definition=document.figure_types.find(value=>value.figure_type_id===base.figure.figure_type_id);
  definition.image_assets=[{kind:'image',data_url:'data:image/png;base64,'+'A'.repeat(1024*1024)}];
  change?.(definition);
  const editor=plain?new Editor({demo:false}).load(document):plainEditorFor({demo:false},document);
  editor.select(uid,0);
  return editor;
}
const order=editor=>builderDrawables(editor.figure).map(entry=>`${entry.kind==='segment'?'s':'p'}${entry.index}`);
test('native mixed sequence front/back and one-step movement keep both sets stable',()=>{
  const e=fixture(),original=order(e);assert.deepEqual(original,['s0','p0','s1','p1','s2','s3']);
  e.moveBuilderOrder({kind:'polygon',index:0},'up');assert.deepEqual(order(e),['s0','s1','p0','p1','s2','s3']);
  e.undo();assert.deepEqual(order(e),original);e.redo();
  e.moveBuilderOrder({kind:'polygon',index:0},'back');assert.equal(order(e)[0],'p0');
  e.moveBuilderOrder({kind:'segment',index:0},'front',{branch:true});assert.deepEqual(order(e),['p1','s3','p0','s0','s1','s2']);
  const n=e._undo.length;e.moveBuilderOrder({kind:'segment',index:0},'front',{branch:true});assert.equal(e._undo.length,n);
});
test('native branch includes pivot and descendant-only polygons, not crossing polygons',()=>{
  const e=fixture();assert.deepEqual(builderStyleTargets(e.figure,{kind:'segment',index:0},{branch:true}).map(x=>`${x.kind}:${x.index}`),['segment:0','polygon:0','segment:1','segment:2']);
  const entries=builderDrawables(e.figure),targets=builderStyleTargets(e.figure,{kind:'segment',index:0},{branch:true});
  const raised=reorderBuilderDrawables(entries,targets,{delta:2});
  assert.deepEqual(raised.map(x=>`${x.kind[0]}${x.index}`),['p1','s3','s0','p0','s1','s2']);
});
test('tied authored records stay untouched on no-op and reorder deterministically',()=>{
  const e=fixture({change:figure=>{figure.segments.forEach(s=>s.z_order=1);figure.polyfills.forEach(p=>p.z_order=1);}});
  const before=structuredClone(e.figure);e.moveBuilderOrder({kind:'segment',index:3},'front');assert.deepEqual(e.figure,before);
  e.moveBuilderOrder({kind:'polygon',index:0},'front');assert.deepEqual(order(e),['p1','s0','s1','s2','s3','p0']);
  e.undo();assert.deepEqual(e.figure,before);
});
test('document validation rejects malformed order and polygon alpha before rendering',()=>{
  const e=fixture();
  for(const mutate of [f=>f.segments[0].z_order=Infinity,f=>f.segments[0].z_order=1e50,
    f=>f.polyfills[0].z_order=NaN,f=>f.polyfills[0].color_alpha=-.1,f=>f.polyfills[0].color_alpha=1.1]){
    const report=structuredClone(e.report);mutate(report.figure_types.find(f=>f.figure_type_id===e.figure.figure_type_id));
    assert.throws(()=>validateScene(report),/order|opacity|native|finite|float32/i);
  }
  assert.doesNotThrow(()=>validateScene(e.report));
});
test('actual PIV and STK encoders retain mixed order and polygon alpha',async()=>{
  const e=fixture({change:figure=>{figure.image_assets=[];}});e.moveBuilderOrder({kind:'segment',index:1},'back',{branch:true});e.setPolygonOpacity(0,.4);
  const expected=order(e),document=e.serialize(),reopened={};
  for(const kind of ['piv','stk']){
    const figures=(await roundTrip({document,format:kind,figureTypeId:document.figure_types.at(-1).figure_type_id})).figure_types;
    reopened[kind]=figures.find(f=>(f.polyfills??[]).length===2);assert.ok(reopened[kind],`${kind} keeps the two-polygon figure`);
  }
  for(const figure of Object.values(reopened)){assert.deepEqual(builderDrawables(figure).map(entry=>`${entry.kind==='segment'?'s':'p'}${entry.index}`),expected);assert.equal(figure.polyfills[0].alpha_raw,153);}
});
