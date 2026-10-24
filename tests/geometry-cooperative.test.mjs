import test from 'node:test';
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {buildGeometry} from '../src/pivot/geometry.mjs';
import {geometryFixture,geometrySummary} from './geometry-cooperative-fixture.mjs';

const baseline=JSON.parse(await readFile(new URL('./geometry-cooperative-baseline.json',import.meta.url),'utf8'));
const json=value=>JSON.parse(JSON.stringify(value));
// Segment records now also carry the drawable kind, allowing the ordered
// painter list to reference them directly. Compare every pre-existing field.
const originalFields=geometry=>{
  const summary=json(geometrySummary(geometry));
  for(const [,object] of summary.objects)if(object)for(const segment of object.segments)delete segment.type;
  return summary;
};
test('the builder preserves all pre-change mixed primitive, legacy and warning fields',()=>{
  for(const entry of baseline) {
    const report=geometryFixture(entry.options),frame=report.keyframes[0];
    const before=structuredClone(report),sync=buildGeometry(report,frame);
    assert.deepEqual(originalFields(sync),entry.geometry);
    assert.deepEqual(report,before,'Preparation must not alter the document');
    for(const object of sync.objects.values())if(object) {
      assert.equal(object.obj,frame.objects[object.slot]);
      assert.equal(object.fig,report.figure_types[0]);
    }
  }
});

test('native segment artwork reuses evaluated records without modifying the document',()=>{
  const report=geometryFixture(),before=structuredClone(report),geometry=buildGeometry(report,report.keyframes[0]);
  for(const g of geometry.objects.values())if(g)for(const s of g.drawables)if(s.seg)assert.ok(g.segments.includes(s));
  assert.deepEqual(report,before);
});

test('incremental preparation retains unaffected references and exact joined descendant geometry',()=>{
  const report=geometryFixture(),frame=report.keyframes[0],initial=buildGeometry(report,frame),baseline=json(geometrySummary(initial));
  frame.objects[0].root_x+=13;frame.objects[1].angles[0]-=.4;
  const incremental={previous:initial,dirtySlots:[0,1,2]},prepared=buildGeometry(report,frame,incremental);
  assert.deepEqual(prepared.drawables,buildGeometry(report,frame).drawables);
  assert.equal(prepared.objects.get(3),initial.objects.get(3));assert.notEqual(prepared.objects.get(0),initial.objects.get(0));
  assert.equal(prepared.previousToken,initial.token);assert.notEqual(prepared.token,initial.token);
  assert.deepEqual([...prepared.changedSlots],[0,1,2]);assert.equal(prepared.previous,undefined);
  // The original source object references intentionally observe the caller's
  // edits, but completed geometric coordinates in the previous cache stay fixed.
  assert.deepEqual(initial.objects.get(0).root,baseline.objects.find(([slot])=>slot===0)[1].root);
  assert.deepEqual(initial.objects.get(1).joints.get(2),baseline.objects.find(([slot])=>slot===1)[1].joints.find(([id])=>id===2)[1]);
});
