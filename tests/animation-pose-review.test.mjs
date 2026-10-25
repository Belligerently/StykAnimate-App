import test from 'node:test';
import assert from 'node:assert/strict';
import {emptyDocument, createFrame, createObject} from '../src/demo.mjs';
import {createLayerState} from '../src/layers.mjs';
import {animationChannelId} from '../src/animation-channels.mjs';
import {composePoseChannels} from '../src/animation-pose.mjs';

const channel = (target, property, keys) => ({id: animationChannelId(target, property), target, property,
  keys: keys.map(([tick, value, interpolation = 'linear'], index) => ({id: `key-${index}`, tick, value: property === 'pose' ? {frameId: value} : value, interpolation}))});

test('blanking one parent layer resolves many detached children with linear source lookups', () => {
  const report = emptyDocument(), definition = report.figure_types[0], count = 1200;
  definition.segments = [{...definition.segments[0], length: 10, angle_rad: 0}]; definition.segment_count = 1;
  const objects = Array.from({length: count}, (_, index) => createObject(definition, index + 1, 100, 100,
    index ? {join: {target_object_slot: 0, target_unique_id: 1, target_handle_id: 1}} : {}));
  const frame = createFrame(objects, 1); frame._styk = {frameId: 'frame-1'}; report.keyframes = [frame];
  report._styk.layers = {...createLayerState(), nextId: 3, order: ['layer-1', 'layer-2'],
    byId: {'layer-1': {name: 'Parent', visible: true, locked: false}, 'layer-2': {name: 'Children', visible: true, locked: false}},
    assignments: Object.fromEntries(objects.slice(1).map(object => [object.unique_id, 'layer-2']))};
  report._styk.animation = {version: 1, nextKeyId: 2, channels: [channel({kind: 'layer', id: 'layer-1'}, 'pose', [[0, null]])]};
  const roots = new Map(objects.map((object, slot) => [slot, {root: {x: slot ? 110 : 100, y: 100}}]));
  let reads = 0, geometryCalls = 0;
  frame.objects = new Proxy(objects, {get(target, key, receiver) {
    if (typeof key === 'string' && /^\d+$/.test(key)) reads++;
    return Reflect.get(target, key, receiver);
  }});
  const output = composePoseChannels(report, frame, 0, {
    sampleNative: (virtual, index) => virtual.keyframes[index],
    resolveGeometry: () => { geometryCalls++; return {objects: roots}; },
  });
  assert.equal(geometryCalls, 1); assert.equal(output.objects.length, count - 1);
  assert.ok(output.objects.every(object => object.join === null && object.root_x === 110 && object.root_y === 100));
  assert.ok(reads < count * 20, `${reads} source-object reads for ${count} figures`);
});

test('many joined figures share indexed handle lookups on a large reusable definition', () => {
  const report = emptyDocument(), childDefinition = report.figure_types[0], segmentCount = 800, childCount = 1200;
  childDefinition.segments = [{...childDefinition.segments[0], length: 10, angle_rad: 0}]; childDefinition.segment_count = 1;
  const parentDefinition = {...childDefinition, figure_type_id: 2,
    segments: Array.from({length: segmentCount}, (_, index) => ({...childDefinition.segments[0], segment_id: index + 1, parent_id: 0}))};
  parentDefinition.segment_count = segmentCount; report.figure_types.push(parentDefinition);
  const objects = [createObject(parentDefinition, 1, 100, 100), ...Array.from({length: childCount}, (_, index) => createObject(childDefinition, index + 2, 0, 0,
    {join: {target_object_slot: 0, target_unique_id: 1, target_handle_id: segmentCount}}))];
  const frame = createFrame(objects, 1); frame._styk = {frameId: 'frame-1'}; report.keyframes = [frame];
  report._styk.layers = {...createLayerState(), nextId: 3, order: ['layer-1', 'layer-2'],
    byId: {'layer-1': {name: 'Parent', visible: true, locked: false}, 'layer-2': {name: 'Children', visible: true, locked: false}},
    assignments: Object.fromEntries(objects.slice(1).map(object => [object.unique_id, 'layer-2']))};
  report._styk.animation = {version: 1, nextKeyId: 2, channels: [channel({kind: 'layer', id: 'layer-1'}, 'pose', [[0, 'frame-1']])]};
  let reads = 0;
  parentDefinition.segments = new Proxy(parentDefinition.segments, {get(target, key, receiver) {
    if (typeof key === 'string' && /^\d+$/.test(key)) reads++;
    return Reflect.get(target, key, receiver);
  }});
  const output = composePoseChannels(report, frame, 0, {sampleNative: (virtual, index) => virtual.keyframes[index]});
  assert.equal(output.objects.length, childCount + 1);
  assert.ok(output.objects.filter(object => object.unique_id !== 1).every(object => output.objects[object.join.target_object_slot].unique_id === 1));
  assert.ok(reads < (childCount + segmentCount) * 10, `${reads} definition segment reads for ${childCount} joins and ${segmentCount} segments`);
});
