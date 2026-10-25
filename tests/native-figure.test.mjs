import test from 'node:test';
import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {createDemo} from '../src/demo.mjs';
import {Editor} from '../src/editor.mjs';
import {plainEditorFor} from './support/plain-editor.mjs';
import {engineEditor} from './support/owned-editor.mjs';
import {validateScene} from '../src/pivot/model.mjs';
import {buildGeometry, evaluatedFrame} from '../src/pivot/geometry.mjs';
import {timeline} from '../src/pivot/read-view.mjs';
import {buildContainerV2, decodeDocument} from './support/native-files.mjs';
import {buildContainerV1} from './support/container-v1.mjs';
import {packPoseSequences, expandPoseSequencesSteps} from '../src/pose-sequence-storage.mjs';
import {figureDocument} from './support/engine-export.mjs';

function documentRecords(document, seen = new Set(), result = []) {
  if (seen.has(document)) return result;
  seen.add(document); result.push(document);
  for (const figure of document.figure_types) if (figure._styk?.movieclip?.document)
    documentRecords(figure._styk.movieclip.document, seen, result);
  if (document._styk?.movieclipSource?.document) documentRecords(document._styk.movieclipSource.document, seen, result);
  return result;
}

/** The document as every reader sees it: the poses its channels share are expanded again. */
function unpacked(document) {
  const copy = structuredClone(document);
  for (const record of documentRecords(copy)) for (const step of expandPoseSequencesSteps(record)) void step;
  return copy;
}

function sample(report, tick) {
  const rows = timeline(report); let index = 0;
  while (index + 1 < rows.length && rows[index + 1].start <= tick) index++;
  const row = rows[index], frame = evaluatedFrame(report, index, row.count ? (tick - row.start) / row.count : 0, tick);
  return buildGeometry(report, frame).drawables.map(value => ({type: value.type,
    points: value.points.map(point => [point.x, point.y]), rgb: value.rgb, alpha: value.alpha}));
}

function movieclipFixture({nested = false} = {}) {
  // These deterministic bytes exercise archive retention, not Pivot parsing.
  const bytes = Buffer.alloc(128 * 1024); let state = 0x51f1c012;
  for (let i = 0; i < bytes.length; i++) {state ^= state << 13; state ^= state >>> 17; state ^= state << 5; bytes[i] = state & 255;}
  const source = {kind: 'piv', version: 5, source_base64: bytes.toString('base64'),
    sha256: createHash('sha256').update(bytes).digest('hex'), document_sha256: '0'.repeat(64), document_digest_version: 3};
  // The source and the asset are written into the document the engine opens.
  const base = new Editor({propertyChannels: true}); base.report._stykSource = source;
  base.report.figure_types[0].image_assets = [{name: 'Retained bitmap', data_base64: 'AQID',
    future: {_stykSource: 'retain this opaque asset field'}}];
  const editor = plainEditorFor({propertyChannels: true}, base.serialize());
  editor.selectMany([1, 2]); editor.beginTranslationDrag(); editor.moveSelection(10, 5); editor.commitDrag();
  const figure = editor.createMovieclip({start: 3, end: 24}), uid = editor.selectedUid;
  if (nested) {
    editor.enterMovieclip(figure.figure_type_id); editor.select(1); editor.createMovieclip({start: 0, end: 10}); editor.leaveMovieclip();
  }
  // Every document of the project carries the source and unknown metadata: the engine opens the project so.
  const saved = editor.serialize();
  for (const document of documentRecords(saved)) {
    document._stykSource = source;
    document.futureMetadata = {_stykSource: {opaque: 'retain this unknown field'}, document: {unknown: true},
      data_url: 'data:image/png;base64,AQID', fractional: .123456789};
  }
  const project = engineEditor({propertyChannels: true}, saved);
  project.seek(3); project.select(uid);
  return {editor: project, uid, source, bytes};
}

test('native figure keeps the evaluated pose, precise style and assets without the project', () => {
  const report = createDemo();
  report.figure_types[0].custom = {opaque: '日本語', fraction: .123456789};
  report.keyframes[0].objects[0]._stykStyle = {outlineOpacity: .123456789};
  report._stykSource = {source_base64: 'unrelated original animation'};
  const before = structuredClone(report), tick = 1;
  const output = figureDocument(report, {figureUid: 1, tick});
  validateScene(output);
  assert.equal(output.figure_types.length, 1);
  assert.equal(output.keyframes.length, 1);
  assert.deepEqual(output.figure_types[0].custom, report.figure_types[0].custom);
  assert.equal(output.keyframes[0].objects[0]._stykStyle.outlineOpacity, .123456789);
  assert.deepEqual(output.keyframes[0].objects[0].angles, evaluatedFrame(report, 0, .5, tick).objects[0].angles);
  assert.equal(output._stykSource, undefined);
  assert.deepEqual(output.backgrounds, []);
  assert.deepEqual(report, before);
  const editor = plainEditorFor({demo: false});
  const name = editor.name, count = editor.frame.objects.length;
  editor.importFigure(output);
  assert.equal(editor.name, name);
  assert.equal(editor.frame.objects.length, count + 1);
  assert.deepEqual(editor.object.angles, output.keyframes[0].objects[0].angles);
  assert.equal(editor.object._stykStyle.outlineOpacity, .123456789);
});

test('missing selection fails rather than exporting a different figure', () => {
  assert.throws(() => figureDocument(createDemo(), {figureUid: 999}), /Select a figure/);
});

test('authored channels are captured at the selected tick without retaining the animation', () => {
  const editor = plainEditorFor(undefined, createDemo());
  editor.addAnimationKeys([
    {target: {kind: 'figure', uid: 1}, property: 'x', tick: 0, value: 110},
    {target: {kind: 'figure', uid: 1}, property: 'x', tick: 12, value: 290},
  ]);
  editor.seek(6);
  const output = figureDocument(editor, {figureUid: 1, tick: editor.tick});
  assert.equal(output.keyframes[0].objects[0].root_x, editor.evaluatedFrame.objects.find(o => o.unique_id === 1).root_x);
  assert.equal(output._styk.animation, undefined);
  validateScene(output);
});

test('portable clips remove only recognized original-source archives, including nested baselines and figure sets', async () => {
  const {editor, uid, source} = movieclipFixture({nested: true}), before = structuredClone(editor.report);
  for (const selection of [{figureUid: uid}, {figureUids: [uid, 3]}]) {
    const output = figureDocument(editor, {...selection, tick: 3});
    validateScene(output); await buildContainerV2(output, 'stkz');   // the writer accepts only a self-contained figure
    const original = {...output, figure_types: output.figure_types.map(figure => editor.report.figure_types.find(f => f.figure_type_id === figure.figure_type_id))};
    const expected = packPoseSequences(structuredClone(original));
    for (const document of documentRecords(expected)) delete document._stykSource;
    assert.deepEqual(output, expected);
    assert.ok(documentRecords(output).length > 3, 'Nested clip and authored baseline documents must be exercised.');
    for (const document of documentRecords(output)) {
      assert.equal(document._stykSource, undefined);
      if (document.futureMetadata) assert.equal(document.futureMetadata._stykSource.opaque, 'retain this unknown field');
    }
    const outputDocument = output.figure_types.find(figure => figure._styk?.movieclip)._styk.movieclip.document;
    const liveDocument = original.figure_types.find(figure => figure._styk?.movieclip)._styk.movieclip.document;
    assert.notEqual(outputDocument, liveDocument);
    assert.deepEqual(outputDocument.keyframes, liveDocument.keyframes);
    assert.deepEqual(outputDocument.futureMetadata, liveDocument.futureMetadata);
    const flat = unpacked(output);
    for (const tick of [0, .5, 2, 3.5, 9, 12.5, 20]) assert.deepEqual(sample(flat, tick), sample(original, tick));
  }
  assert.deepEqual(editor.report, before);
  assert.deepEqual(editor.report._stykSource, source);
});

test('portable save/reopen retains authored poses, assets and independent edit/undo/redo while projects keep provenance', async () => {
  const {editor, uid, source, bytes} = movieclipFixture();
  const portable = figureDocument(editor, {figureUid: uid, tick: 3}), before = structuredClone(editor.report);
  // The export stores the poses its channels share once.
  const retained = {...portable, figure_types: [editor.figure]}, packed = portable;
  const baseline = packed.figure_types[0]._styk.movieclip.document._styk.movieclipSource.document;
  assert.equal(baseline._styk.animation.version, 2);
  assert.equal(baseline._styk.animation.poseSequences.length, 1);
  assert.deepEqual(baseline._styk.animation.channels.map(channel => channel.keys), [{sequence: 0}, {sequence: 0}]);
  const retainedBytes = await buildContainerV2(retained, 'stykmov'), portableBytes = await buildContainerV2(packed, 'stykmov');
  assert.ok(retainedBytes.byteLength - portableBytes.byteLength > bytes.length * .9);
  const decoded = (await decodeDocument(portableBytes, 'stykmov')).document, expanded = unpacked(decoded);
  const reopened = new Editor({propertyChannels: true}).load(decoded);
  for (const document of documentRecords(reopened.report)) assert.equal(document._stykSource, undefined);
  for (let tick = 0; tick < 21; tick += .5) assert.deepEqual(sample(reopened.report, tick), sample(expanded, tick));
  const reopenedAssets = reopened.figure._styk.movieclip.document.figure_types.flatMap(figure => figure.image_assets);
  assert.deepEqual(reopenedAssets, [{name: 'Retained bitmap', data_base64: 'AQID',
    future: {_stykSource: 'retain this opaque asset field'}}]);
  const originalAnimation = structuredClone(editor.figure._styk.movieclip.document);
  reopened.enterMovieclip(reopened.figure.figure_type_id); reopened.seek(4); reopened.select(1);
  const clipBefore = sample(reopened.report, 4), untouched = structuredClone(reopened.evaluatedFrame.objects.find(object => object.unique_id === 2));
  reopened.moveSelection(23, -7);
  assert.notDeepEqual(sample(reopened.report, 4), clipBefore);
  assert.deepEqual(reopened.evaluatedFrame.objects.find(object => object.unique_id === 2), untouched);
  const edited = sample(reopened.report, 4);
  reopened.undo(); assert.deepEqual(sample(reopened.report, 4), clipBefore);
  reopened.redo(); assert.deepEqual(sample(reopened.report, 4), edited);
  reopened.leaveMovieclip();
  const resaved = await buildContainerV2(packPoseSequences(reopened.report), 'stykmov');
  const editedReload = new Editor().load((await decodeDocument(resaved, 'stykmov')).document);
  for (let tick = 0; tick < 21; tick += .5) assert.deepEqual(sample(editedReload.report, tick), sample(reopened.report, tick));
  assert.deepEqual(editor.figure._styk.movieclip.document, originalAnimation);
  assert.deepEqual(editor.report, before);
  const project = (await decodeDocument(await buildContainerV2(packPoseSequences(editor.report), 'styk'), 'styk')).document;
  assert.equal(project._stykSource.source_base64, source.source_base64);
  for (const document of documentRecords(project)) assert.equal(document._stykSource.source_base64, source.source_base64);
});

test('legacy portable clips with retained import archives stay editable when re-exported compactly', async () => {
  const {editor, uid} = movieclipFixture();
  const portable = figureDocument(editor, {figureUid: uid, tick: 3});
  const legacy = {...portable, figure_types: [editor.figure]};
  for (const bytes of [await buildContainerV1(JSON.stringify(legacy), 'stykmov'), await buildContainerV2(legacy, 'stykmov')]) {
    const old = new Editor().load((await decodeDocument(bytes, 'stykmov')).document);
    const output = figureDocument(old, {figureUid: old.selectedUid});
    for (const document of documentRecords(output)) assert.equal(document._stykSource, undefined);
    const flat = unpacked(output), expected = unpacked(portable);
    for (const tick of [0, 3.5, 12, 20]) assert.deepEqual(sample(flat, tick), sample(expected, tick));
    old.enterMovieclip(); old.seek(4); old.select(1); old.moveSelection(4, 1); old.undo(); old.redo(); old.leaveMovieclip();
    assert.ok(sample(old.report, 4).length);
  }
});
