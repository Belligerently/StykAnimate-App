import test from 'node:test';
import assert from 'node:assert/strict';
import {fileURLToPath} from 'node:url';
import {readFileSync} from 'node:fs';
import path from 'node:path';
import {plainEditorFor} from './support/plain-editor.mjs';
import {engineEditor} from './support/owned-editor.mjs';
import {emptyDocument, createObject, createFrame} from '../src/demo.mjs';
import {evaluatedFrame, buildGeometry} from '../src/pivot/geometry.mjs';
import {timeline} from '../src/pivot/read-view.mjs';
import {synchronizeExistingDerived} from '../src/document-normalize.mjs';
import {missingFixture} from './helpers/local-fixtures.mjs';
import {exportDocument, importBytes, nodeCodec} from './support/codec.mjs';
import {pivotDocument, keepsStoredFrames} from './support/engine-export.mjs';

const near = (a, b) => assert.ok(Math.abs(a - b) < .001, `${a} != ${b}`);
function sample(report, tick) {
  const rows = timeline(report); let index = 0;
  while (index + 1 < rows.length && rows[index + 1].start <= tick) index++;
  return evaluatedFrame(report, index, rows[index].count ? (tick - rows[index].start) / rows[index].count : 0, tick);
}
async function nativeRoundtrip(document, sourcePath) {
  synchronizeExistingDerived(document);
  const codec = nodeCodec(), request = {document, format: 'piv'};
  if (sourcePath) request.sourceId = (await importBytes(readFileSync(sourcePath), path.basename(sourcePath), {codec})).sourceId;
  const {bytes, name} = await exportDocument(request, {codec});
  return (await importBytes(bytes, name, {codec})).document;
}
function compareFrames(original, a, exported, b) {
  const before = buildGeometry(original, a), after = buildGeometry(exported, b), byUid = new Map([...after.objects.values()].map(g => [g.obj.unique_id, g]));
  assert.deepEqual(before.orderedObjects.map(g => g.obj.unique_id), after.orderedObjects.map(g => g.obj.unique_id));
  for (const geometry of before.objects.values()) {
    const saved = byUid.get(geometry.obj.unique_id); assert.ok(saved);
    near(saved.root.x, geometry.root.x); near(saved.root.y, geometry.root.y); near(saved.obj.scale, geometry.obj.scale);
    assert.equal(saved.obj.field28, geometry.obj.field28);
    geometry.segments.forEach((segment, index) => { near(saved.segments[index].end.x, segment.end.x); near(saved.segments[index].end.y, segment.end.y); });
  }
  for (const key of ['x', 'y', 'scale', 'angle_rad']) near(a.camera[key], b.camera[key]);
  const argb = frame => frame._stykBackgroundColor ?? original.backgrounds.find(bg => bg.background_id === frame.background_id)?.color;
  if (argb(a) !== undefined) assert.equal(exported.backgrounds.find(bg => bg.background_id === b.background_id)?.color, argb(a));
}

test('untouched native documents retain the exact original export path', () => {
  const native = emptyDocument();
  // Native exposures have no authored per-frame easing metadata.
  native.keyframes = [createFrame([createObject(native.figure_types[0], 1, 40, 200)], 4)];
  const editor = engineEditor({demo: false}, native);
  assert.equal(keepsStoredFrames(editor), true);
});

