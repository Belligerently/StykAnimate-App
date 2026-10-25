import test from 'node:test';
import assert from 'node:assert/strict';
import {emptyDocument,createFrame,createObject} from '../src/demo.mjs';
import {buildGeometry,evaluatedFrame} from '../src/pivot/geometry.mjs';
import {cameraMatrix} from '../src/renderer-geometry.mjs';
import {ownedScope} from './support/owned-scope.mjs';

const open = ownedScope();

function fixture(){
  const report=emptyDocument(),figure=report.figure_types[0];
  report.keyframes=[0,1,2].map(index=>{
    const object=createObject(figure,1,170+index*35,210-index*15);
    object.angles=object.angles.map((angle,i)=>angle+index*.12*(i%2?1:-1));
    return createFrame([object,createObject(figure,2,430,260)],4);
  });
  const editor=open({demo:false}, report);editor.select(1);editor.setOrigin(4);return editor;
}
function points(editor,index,{evaluated=true,projected=false}={}){
  const raw=editor.report.keyframes[index],frame=evaluated?evaluatedFrame(editor.report,index,0,editor.timeline[index].start):raw;
  const matrix=projected?cameraMatrix({report:editor.report,frame}):[1,0,0,1,0,0];
  return [...buildGeometry(editor.report,frame).objects.values()].flatMap(object=>[...object.joints].map(([id,p])=>
    [object.obj.unique_id,id,matrix[0]*p.x+matrix[2]*p.y+matrix[4],matrix[1]*p.x+matrix[3]*p.y+matrix[5]]));
}
function close(actual,expected){assert.equal(actual.length,expected.length);for(let i=0;i<actual.length;i++){
  assert.deepEqual(actual[i].slice(0,2),expected[i].slice(0,2));
  // Native source roots are float32; virtual-pivot correction retains double
  // precision. Compare within one source-coordinate rounding unit.
  for(const j of [2,3])assert.ok(Math.abs(actual[i][j]-expected[i][j])<1e-4,`${actual[i]} != ${expected[i]}`);
}}
test('unchanged and style-only pivot synchronization preserves shared metadata references',()=>{
  const editor=fixture(),frame=editor.frame,object=frame.objects[0],origin=object._stykAnimationOrigin;
  editor._synchronizeAnimationOrigins(frame,frame.objects);
  assert.equal(object._stykAnimationOrigin,origin);
  const before=structuredClone(object),styled={...object,field28:93};
  editor._synchronizeAnimationOrigins({...frame,objects:[styled]},[before]);
  assert.equal(styled._stykAnimationOrigin,origin);
});
for(const [name,apply]of [
  ['rotation',editor=>editor.rotateFiguresInFrames(.3,[0,2])],
  ['scale',editor=>editor.scaleFiguresInFrames(1.6,[0,2])],
  ['vertical mirror',editor=>editor.mirrorFiguresInFrames('vertical',[0,2])],
  ['center',editor=>editor.centerFiguresInFrames([0,2])],
  ['default pose',editor=>editor.resetPoseInFrames([0,2])],
])test(`rerooted ${name} range keeps edited physical poses through evaluation and undo`,()=>{
  const editor=fixture(),before=editor.serialize(),middle=editor.report.keyframes[1],history=editor._undo.length;
  apply(editor);assert.equal(editor._undo.length,history+1);assert.equal(editor.report.keyframes[1],middle);
  for(const index of [0,2])close(points(editor,index),points(editor,index,{evaluated:false}));
  const after=editor.serialize();editor.undo();assert.deepEqual(editor.serialize(),before);editor.redo();assert.deepEqual(editor.serialize(),after);
});
