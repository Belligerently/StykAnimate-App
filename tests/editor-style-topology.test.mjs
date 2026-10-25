import test from 'node:test';
import assert from 'node:assert/strict';
import {Editor} from '../src/editor.mjs';
import {emptyDocument,createFrame,createObject} from '../src/demo.mjs';
import {resolveInstanceFigure} from '../src/animation-style.mjs';

// session gap: captureFrameSource and the topology commands with property channels run in the editor, and what they leave does not match the session's document when the owner writes it back.

const clone=value=>structuredClone(value), target=handleId=>({kind:'segment',uid:1,handleId});
function fixture() {
  const report=emptyDocument(), figure=report.figure_types[0];
  figure.segments=figure.segments.slice(0,3).map((segment,i)=>({...segment,segment_id:[42,600,1000][i],parent_id:[0,42,600][i],
    primitive_code:0,segment_kind:'line',segment_type:'line',length:[80,40,30][i],angle_rad:.3*i,
    ...(i===1?{secondary:{rgb:[40,180,220],a:40}}:{})}));
  report.figure_types.push({...clone(figure),figure_type_id:2});
  report.keyframes=[0,1].map(i=>createFrame([createObject(figure,1,100+i*30,200),createObject(report.figure_types[1],2,400,200)],i?1:4));
  for(const frame of report.keyframes) frame.objects[0]._stykStyle={outlineOpacity:.8,segments:{42:{opacity:.25},600:{opacity:.6,gradientOpacity:.45},1000:{opacity:.9}}};
  const editor=new Editor({demo:false}).load(report);editor.captureFrameSource(editor.frame);
  editor.addAnimationKeys([
    {target:target(42),property:'opacity',tick:0,value:.2},{target:target(42),property:'opacity',tick:4,value:.8},
    {target:target(600),property:'gradientOpacity',tick:0,value:.3},{target:target(600),property:'gradientOpacity',tick:4,value:.7},
    {target:{kind:'segment',uid:2,handleId:42},property:'opacity',tick:0,value:.9},
  ]);editor.select(1,0);return editor;
}
const channel=(editor,id,property='opacity')=>editor.animationChannel(target(id),property);
function styledSegments(editor,tick){editor.seek(tick);editor.select(1);return resolveInstanceFigure(editor.figure,editor.object).segments.map(s=>({length:s.length,transparency:s.transparency,secondary:s.secondary?.a}));}

test('Set Origin keeps authored and captured styles on the same physical sparse segments in one undo',()=>{
  const editor=fixture(), before=editor.serialize(), expected=[0,2,4].map(tick=>styledSegments(editor,tick)), original=clone(channel(editor,42)), history=editor._undo.length;
  editor.setOrigin(600);assert.equal(editor._undo.length,history+1);
  // Native Set Origin reverses the second gradient: endpoint roles swap while
  // their world positions retain the same colors/alpha (covered independently).
  assert.deepEqual([0,2,4].map(tick=>styledSegments(editor,tick)),expected.map(frame=>frame.map((segment,index)=>index===1?
    {...segment,transparency:segment.secondary,secondary:segment.transparency}:segment)));
  assert.deepEqual(channel(editor,600).keys,original.keys);assert.equal(channel(editor,42,'gradientOpacity'),null);
  assert.equal(channel(editor,42).keys[0].value,.3);
  for(const frame of editor._allSourceFrames()){
    const style=frame.objects[0]._stykStyle;assert.deepEqual(style.segments[600],{opacity:.25});assert.deepEqual(style.segments[42],{opacity:.45,gradientOpacity:.6});assert.equal(style.outlineOpacity,.8);
  }
  assert.equal(editor.animationChannel({kind:'segment',uid:2,handleId:42},'opacity').keys[0].value,.9);
  const after=editor.serialize();editor.undo();assert.deepEqual(editor.serialize(),before);editor.redo();assert.deepEqual(editor.serialize(),after);
});

test('Branch deletion and replacement definitions remove only deleted physical style targets',()=>{
  for(const operation of['delete','replace']){
    const editor=fixture(), before=editor.serialize(), kept=clone(channel(editor,42));editor.select(1,1);
    if(operation==='delete')editor.deleteSegment();
    else {const definition=clone(editor.figure);definition.segments=definition.segments.filter(segment=>segment.segment_id!==600);definition.segments[1].parent_id=42;editor.replaceFigureDefinition(1,definition);}
    assert.deepEqual(channel(editor,42),kept);assert.equal(channel(editor,600,'gradientOpacity'),null);
    for(const frame of editor._allSourceFrames())assert.equal(frame.objects[0]._stykStyle.segments[600],undefined);
    editor.undo();assert.deepEqual(editor.serialize(),before);
  }
});

test('reused identities retain each rig’s continuous physical style curves through remap and later property edits',()=>{
  const editor=fixture();editor.report.keyframes[1].objects[0].figure_type_id=2;
  editor.captureFrameSource(editor.report.keyframes[1]);
  const before=editor.serialize(), history=editor._undo.length,untouched=editor.report.keyframes[1].objects[0],saved=clone(untouched);
  const opacity=clone(channel(editor,42).keys),gradient=clone(channel(editor,600,'gradientOpacity').keys);
  editor.setOrigin(600);assert.equal(editor._undo.length,history+1);assert.equal(editor.report.keyframes[1].objects[0],untouched);assert.deepEqual(untouched,saved);
  const scoped=(handleId,figureTypeId,property='opacity')=>editor.animationChannel({...target(handleId),figureTypeId},property);
  assert.deepEqual(scoped(600,1).keys,opacity);
  assert.deepEqual(scoped(42,2).keys.map(key=>key.value),opacity.map(key=>key.value));
  assert.deepEqual(scoped(42,1).keys,gradient);
  assert.deepEqual(scoped(600,2,'gradientOpacity').keys.map(key=>key.value),gradient.map(key=>key.value));
  for(const frame of editor._allSourceFrames())for(const object of frame.objects)if(object.unique_id===1&&object.figure_type_id===2)assert.deepEqual(object._stykStyle,saved._stykStyle);
  const after=editor.serialize();editor.undo();assert.deepEqual(editor.serialize(),before);editor.redo();assert.deepEqual(editor.serialize(),after);
  editor.seek(4);editor.select(1,0);
  assert.deepEqual(editor.segmentAnimationTarget(1,42,'opacity'),{...target(42),figureTypeId:2});
  const first=clone(scoped(600,1));editor.beginAnimationEdit(target(42),'opacity');editor.previewAnimationEdit(.55);editor.commitAnimationEdit();
  assert.deepEqual(scoped(600,1),first);assert.equal(scoped(42,2).keys.at(-1).value,.55);
  editor.undo();assert.deepEqual(editor.serialize(),after);
  const other=clone(scoped(600,1));editor.setOrigin(600);assert.deepEqual(scoped(600,1),other);
  editor.undo();assert.deepEqual(editor.serialize(),after);
});

test('mixed-rig remapping retains more-specific curves when legacy and scoped properties overlap',()=>{
  for(const reverse of [false,true]){
    const editor=fixture();editor.report.keyframes[1].objects[0].figure_type_id=2;
    editor.addAnimationKey({...target(42),figureTypeId:1},'opacity',0,.65);
    if(reverse)editor.report._styk.animation.channels.reverse();
    const before=editor.serialize();editor.setOrigin(600);
    assert.equal(editor.animationChannel({...target(600),figureTypeId:1},'opacity').keys[0].value,.65);
    assert.equal(editor.animationChannel({...target(42),figureTypeId:2},'opacity').keys[0].value,.2);
    assert.equal(new Set(editor.animationChannels.map(channel=>channel.id)).size,editor.animationChannels.length);
    editor.undo();assert.deepEqual(editor.serialize(),before);
  }
});

