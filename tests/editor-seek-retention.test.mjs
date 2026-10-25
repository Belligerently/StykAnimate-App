import test from 'node:test';
import assert from 'node:assert/strict';
import {Editor} from '../src/editor.mjs';
import {createDemo} from '../src/demo.mjs';
import {buildGeometry} from '../src/pivot/geometry.mjs';
const fixture=()=>{
  const report=createDemo(),a=report.keyframes[0];a.duration_raw=8;
  const b=structuredClone(a);b.objects[0].root_x+=40;
  b.objects.forEach(o=>{o.record_offset=400;o.record_end=800;});report.keyframes=[a,b];
  return new Editor({demo:false}).load(report);
};
const drawing=value=>JSON.parse(JSON.stringify(value,(key,value)=>key==='record_offset'||key==='record_end'?undefined:value));
const exact=e=>assert.deepEqual(drawing(e.geometry.drawables),drawing(buildGeometry(e.report,e.evaluatedFrame).drawables));
test('joined descendants follow a changing parent while unrelated geometry remains retained',()=>{
  const e=fixture();for(const frame of e.report.keyframes)frame.objects[2].join={target_object_slot:0,target_handle_id:1};e._invalidate();
  const a=e.geometry;e.selectFrame(1);const b=e.geometry;
  assert.equal(a.objects.get(1),b.objects.get(1));assert.notEqual(a.objects.get(2),b.objects.get(2));exact(e);
  e.selectFrame(0);exact(e);
});
