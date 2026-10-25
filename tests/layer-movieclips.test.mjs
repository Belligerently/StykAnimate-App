import test from 'node:test';
import assert from 'node:assert/strict';
import {Editor as BaseEditor} from '../src/editor.mjs';
class Editor extends BaseEditor { constructor(options={}) { super({...options,layerTimeline:true}); } }
import {engineEditor} from './support/owned-editor.mjs';
/** The editor the engine owns, on a layer sheet like the class above. */
const layerEditor=(options={},document)=>engineEditor({...options,layerTimeline:true},document);
import {movieclipAsset,movieclipTime,playbackSettings,validateMovieclipGraph} from '../src/movieclips.mjs';
import {buildGeometry,evaluatedFrame,timeline} from '../src/pivot/geometry.mjs';
import {materializePivotDocument} from '../src/pivot-export-materialize.mjs';
import {nativeFigureDocument} from '../src/native-figure.mjs';


const sample=(report,tick)=>{const rows=timeline(report);let i=0;while(i+1<rows.length&&rows[i+1].start<=tick)i++;return buildGeometry(report,evaluatedFrame(report,i,rows[i].count?(tick-rows[i].start)/rows[i].count:0,tick));};
const scene=geometry=>geometry.drawables.map(s=>({type:s.type,points:s.points.map(p=>[p.x,p.y]),rgb:s.rgb,alpha:s.alpha}));
function coordinates(geometry,uid){return geometry.drawables.filter(s=>s.obj.unique_id===uid).map(s=>({type:s.type,points:s.points.map(p=>[p.x,p.y]),rgb:s.rgb,alpha:s.alpha}));}
function close(actual,expected){assert.equal(actual.length,expected.length);for(let i=0;i<actual.length;i++){assert.equal(actual[i].type,expected[i].type);assert.deepEqual(actual[i].rgb,expected[i].rgb);assert.equal(actual[i].points.length,expected[i].points.length);for(let j=0;j<actual[i].points.length;j++)for(let k=0;k<2;k++)assert.ok(Math.abs(actual[i].points[j][k]-expected[i].points[j][k])<.0001,`${i}/${j}/${k}: ${actual[i].points[j][k]} != ${expected[i].points[j][k]}`);}}

test('movieclip sampling is deterministic at loop, once, hold, offset and FPS boundaries',()=>{
  const asset={duration:12,document:{fps:24}},base=playbackSettings(null,{start:12});
  assert.equal(movieclipTime(asset,base,11,24),null);assert.equal(movieclipTime(asset,base,12,24),0);
  assert.equal(movieclipTime(asset,base,23,24),11);assert.equal(movieclipTime(asset,base,24,24),0);
  assert.equal(movieclipTime(asset,{...base,mode:'once'},24,24),null);
  assert.equal(movieclipTime(asset,{...base,mode:'hold'},80,24),11);
  assert.equal(movieclipTime(asset,{...base,mode:'single',offset:7},80,24),7);
  assert.equal(movieclipTime(asset,base,14,48),1);
});
test('range conversion preserves all sampled poses and leaves unrelated figures intact',()=>{
  const editor=layerEditor({propertyChannels:true}),source=structuredClone(editor.report);
  editor.select(1);const figure=editor.createMovieclip({start:3,end:24}),uid=editor.selectedUid;
  for(let tick=3;tick<24;tick++){const actual=sample(editor.report,tick),expected=sample(source,tick);close(coordinates(actual,uid),coordinates(expected,1));close(coordinates(actual,3),coordinates(expected,3));}
  assert.equal(movieclipAsset(figure).duration,21);
  editor.undo();assert.equal(editor.report.figure_types.length,3);close(coordinates(sample(editor.report,10),1),coordinates(sample(source,10),1));
  editor.redo();close(coordinates(sample(editor.report,10),uid),coordinates(sample(source,10),1));
});

test('native document round trip retains source baselines and rejects cyclic clips',()=>{
  const editor=layerEditor({propertyChannels:true});editor.select(1);editor.createMovieclip({start:2,end:19});
  const reopened=new Editor();reopened.load(JSON.parse(JSON.stringify(editor.report)));assert.equal(sample(reopened.report,8).drawables.length,12);
  const figure=reopened.report.figure_types.at(-1);movieclipAsset(figure).document.figure_types.push(figure);
  assert.throws(()=>validateMovieclipGraph(reopened.report),/cycle|nested/);
});

test('global playback settings and frame-local Make unique retain sources with undo',()=>{
  const editor=layerEditor({propertyChannels:true});editor.select(1);editor.createMovieclip({start:3,end:24});const uid=editor.selectedUid;
  editor.setMovieclipPlayback({speed:2,offset:2},{allFrames:true});
  for(const tick of [3,4,10,20]){editor.seek(tick);editor.select(uid);assert.equal(editor.object._stykMovieclip.speed,2);assert.equal(editor.object._movieclipSample,(2+(tick-3)*2)%21);}
  const original=editor.figure.figure_type_id;editor.makeMovieclipUnique();const unique=editor.figure.figure_type_id,uniqueUid=editor.selectedUid;assert.notEqual(unique,original);assert.notEqual(uniqueUid,uid);
  for(const tick of [3,10,20,21]){editor.seek(tick);editor.select(tick===20?uniqueUid:uid);assert.equal(editor.object.figure_type_id,tick===20?unique:original);assert.equal(editor.object._movieclipSample,(2+(tick-3)*2)%21);}
  editor.undo();editor.seek(3);editor.select(uid);assert.equal(editor.figure.figure_type_id,original);editor.undo();assert.equal(editor.object._stykMovieclip.speed,1);
});

test('Bake animation produces ordinary editable poses and one reversible command',async()=>{
  const editor=layerEditor({propertyChannels:true});editor.select(1);editor.createMovieclip({start:2,end:17});const uid=editor.selectedUid,before=structuredClone(editor.report),count=editor._undo.length;
  const children=await editor.bakeMovieclip();assert.ok(children.length);assert.equal(editor._undo.length,count+1);
  for(let tick=2;tick<17;tick++){const actual=sample(editor.report,tick);close(coordinates(actual,children[0]),coordinates(sample(before,tick),uid));assert.equal(coordinates(actual,uid).length,0);}
  editor.undo();close(coordinates(sample(editor.report,8),uid),coordinates(sample(before,8),uid));
  editor.redo();assert.ok(coordinates(sample(editor.report,8),children[0]).length);
});

test('mirrored movieclips and their legacy export agree',()=>{
  const editor=layerEditor({propertyChannels:true});editor.select(1);editor.createMovieclip({start:0,end:8});const uid=editor.selectedUid,old=sample(editor.report,0),root=editor.geometry.objects.get(editor.selectedSlot).root;
  editor.mirrorFigure();const mirrored=coordinates(sample(editor.report,0),uid),expected=coordinates(old,uid).map(s=>({...s,points:s.points.map(([x,y])=>[2*root.x-x,y])}));close(mirrored,expected);
  const flat=materializePivotDocument(editor.report);assert.equal(sample(flat,0).drawables.length,old.drawables.length);
});

test('portable movieclip imports restart and shared editing recovery retains the root project',()=>{
  const editor=layerEditor({propertyChannels:true});editor.select(1);const f=editor.createMovieclip({start:5,end:17});
  const portable=nativeFigureDocument(editor.report,{figureUid:editor.selectedUid,tick:5}),other=layerEditor();other.seek(10);other.importFigure(portable);
  assert.equal(other.object._stykMovieclip.clock,'presence');assert.equal(other.object._movieclipSample,0);assert.equal(other.figure._styk.movieclip.duration,12);
  const parent=editor.report;editor.enterMovieclip(f.figure_type_id);editor.select(1);editor.moveSelection(7,0);
  const recovered=editor.movieclipProjectReport();assert.equal(recovered.keyframes.length,parent.keyframes.length);assert.equal(recovered.figure_types.at(-1)._styk.movieclip.document,editor.report);
  const reopened=new Editor();reopened.load(structuredClone(recovered));assert.equal(reopened.totalTicks,timeline(parent).at(-1).start+timeline(parent).at(-1).count);
});

test('nested conversion preserves the enclosing clip and shared edits update all instances',()=>{
  const editor=layerEditor({propertyChannels:true});editor.select(1);const first=editor.createMovieclip({start:3,end:22}),uid=editor.selectedUid,before=structuredClone(editor.report);
  editor.enterMovieclip(first.figure_type_id);editor.select(1);editor.createMovieclip({start:0,end:19});editor.leaveMovieclip();
  for(let tick=3;tick<22;tick++)close(coordinates(sample(editor.report,tick),uid),coordinates(sample(before,tick),uid));
  editor.seek(3);editor.select(uid);const shared=editor.figure;editor.enterMovieclip();const nested=editor.report.figure_types.find(f=>f._styk?.movieclip);editor.enterMovieclip(nested.figure_type_id);editor.select(1);editor.moveSelection(10,0);editor.leaveMovieclip();editor.leaveMovieclip();
  assert.notEqual(editor.figure,shared);assert.equal(editor._movieclipScopes.length,0);
  const saved=JSON.parse(JSON.stringify(editor.report)),reopened=new Editor();reopened.load(saved);close(coordinates(sample(reopened.report,3),uid),coordinates(sample(editor.report,3),uid));
});

test('rig topology inside a captured clip repairs retained source poses and is reversible',()=>{
  const editor=layerEditor({propertyChannels:true});editor.select(1);editor.addAnimationKey({kind:'segment',uid:1,handleId:3},'opacity',4,.4);
  editor.createMovieclip({start:0,end:16});const type=editor.figure.figure_type_id;editor.enterMovieclip(type);editor.select(1,2);
  const count=editor.figure.segments.length;editor.splitSegment(.3);assert.equal(editor.figure.segments.length,count+1);
  new Editor().load(JSON.parse(JSON.stringify(editor.report)));
  editor.undo();assert.equal(editor.figure.segments.length,count);new Editor().load(JSON.parse(JSON.stringify(editor.report)));
  editor.redo();editor.leaveMovieclip();new Editor().load(JSON.parse(JSON.stringify(editor.report)));assert.ok(sample(editor.report,7).drawables.length>0);
});

test('library insertion follows its current exposure and remains one undo action',()=>{
  const editor=layerEditor({propertyChannels:true});editor.select(1);const clip=editor.createMovieclip({start:0,end:12,replace:false});
  editor.seek(6);const count=editor._undo.length;editor.addFigure(clip.figure_type_id);const uid=editor.selectedUid;
  assert.equal(editor._undo.length,count+1);assert.equal(coordinates(sample(editor.report,5),uid).length,0);
  assert.equal(editor.object._stykMovieclip.clock,'presence');assert.equal(editor.object._stykMovieclip.end,undefined);
  for(const tick of [6,7])assert.ok(coordinates(sample(editor.report,tick),uid).length);
  assert.equal(coordinates(sample(editor.report,8),uid).length,0);editor.undo();assert.equal(coordinates(sample(editor.report,6),uid).length,0);editor.redo();assert.ok(coordinates(sample(editor.report,7),uid).length);
});

test('conversion preserves the complete scene before, inside and after the range',()=>{
  for(const nested of [false,true]){
    const e=layerEditor({propertyChannels:true});e.select(1);
    if(nested){e.createMovieclip({start:0,end:40});e.enterMovieclip();e.select(1);}
    const before=structuredClone(e.report);e.createMovieclip({start:3,end:24});
    for(let t=0;t<40;t++)close(scene(sample(e.report,t)),scene(sample(before,t)));
  }
});

test('breaking adjacent frames leaves one clip with gaps and exact ordinary poses',()=>{
  const e=layerEditor({propertyChannels:true});e.select(1);e.createMovieclip({start:0,end:5});const uid=e.selectedUid,before=structuredClone(e.report),undo=e._undo.length;
  const children=[];
  for(const tick of [0,1]){e.seek(tick);e.select(uid);children.push(e.breakApartMovieclip());}
  assert.equal(e._undo.length,undo+2);
  assert.deepEqual(children[0],children[1]);
  assert.equal(e.animationChannels.length,0);
  for(let t=0;t<30;t++){
    const g=sample(e.report,t);close(scene(g),scene(sample(before,t)));
    assert.equal(coordinates(g,uid).length,t>=2&&t<5?10:0);
    assert.equal(coordinates(g,children[0][0]).length,t<2?10:0);
  }
  const restored=new Editor();restored.load(JSON.parse(JSON.stringify(e.report)));
  for(let t=0;t<6;t++)close(scene(sample(restored.report,t)),scene(sample(e.report,t)));
  e.undo();assert.equal(coordinates(sample(e.report,1),uid).length,10);e.redo();assert.equal(coordinates(sample(e.report,1),uid).length,0);
});

test('conversion rejects interleaved stacking later in the range without a partial edit',()=>{
  const stacked=new Editor({propertyChannels:true});
  for(const frame of stacked.report.keyframes){frame.object_draw_order=[0,1,2];frame.object_draw_order_unique_ids=frame.objects.map(o=>o.unique_id);}
  stacked.report.keyframes[1].object_draw_order=[0,2,1];stacked.report.keyframes[1].object_draw_order_unique_ids=[1,3,2];
  const e=layerEditor({propertyChannels:true},stacked.serialize());e._selectIds([1,2]);const count=e._undo.length,definitions=e.report.figure_types.length;
  assert.throws(()=>e.createMovieclip({start:0,end:20}),/stacking order/);
  assert.equal(e._undo.length,count);assert.equal(e.report.figure_types.length,definitions);
});

