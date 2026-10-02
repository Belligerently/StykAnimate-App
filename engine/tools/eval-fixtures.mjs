#!/usr/bin/env node
// Builds the evaluation fixtures of the Rust engine with the real JavaScript
// editor: each is a small project (plain JSON, opened like a .styk) and what
// the editor evaluates for it (tools/reference.mjs).
//
//   node engine/tools/eval-fixtures.mjs [name ...] [--out <folder>] [--list]
//
// Without names it regenerates every fixture into
// engine/crates/styk-eval/tests/fixtures/eval/. The Rust test
// engine/crates/styk-eval/tests/eval.rs compares the engine with each of them.
// Inverse kinematics has its own set (ik-fixtures.mjs).
import {mkdirSync} from 'node:fs';
import path from 'node:path';
import {here, documentOf, limb, open, whole, nextKey, nextSheetKey, writeProject, writeReference} from './fixture-kit.mjs';

/** Two limbs on one layer, the first moving between two keys of a layer sheet. */
function pair() {
  const e = open(documentOf([limb(), limb()], i => [260 + i * 120, 150]), {layerTimeline: true});
  nextSheetKey(e, 6, 1); whole(e, 1).moveSelection(40, -20);
  e.seek(0);
  return e;
}
const instanceOf = (e, clip) => e.evaluatedFrame.objects.find(object => object.figure_type_id === clip.figure_type_id).unique_id;

const fixtures = {
  /** A figure captured as a movieclip: the layer's keys replace it with the
   * clip's instance, and consecutive replacements tween like a pose track. */
  'movieclip-replacement': () => {
    const e = pair(); e.select(1);
    e.createMovieclip({start: 0, end: 7});
    return {document: e.serialize(), ticks: [0, 2, 3, 5, 6]};
  },
  /** One frame of that movieclip broken apart: patch-only keys bound the
   * frame, and its replacement is itself replaced by the clip's parts. */
  'movieclip-break-apart': () => {
    const e = pair(); e.select(1);
    const clip = e.createMovieclip({start: 0, end: 7});
    e.seek(3); e.select(instanceOf(e, clip)); e.breakApartMovieclip();
    return {document: e.serialize(), ticks: [0, 2, 3, 4, 5, 6]};
  },
  /** Two joined figures captured together and one frame broken apart: the
   * parts keep their join inside the replacement, and a replacement can be empty. */
  'movieclip-joined': () => {
    const e = pair(); e.select(2); e.joinTo(1, 22);
    e.seek(0); e.select(1);
    const clip = e.createMovieclip({start: 0, end: 7});
    e.seek(4); e.select(instanceOf(e, clip)); e.breakApartMovieclip();
    return {document: e.serialize(), ticks: [0, 1, 3, 4, 5, 6]};
  },
  /** Written by hand: the figure another is joined to is replaced and nothing
   * takes its place in the join, which the editor's commands do not produce
   * but its evaluation handles. The joined figure
   * stays where the join held it; with `replacementUid` it follows the stand-in. */
  'movieclip-loose-child': () => {
    const e = pair(); e.select(2); e.joinTo(1, 22);
    const document = e.serialize(), sheet = document._styk.layerSheet, track = Object.values(sheet.tracks).find(track => track.keys.some(key => key.value?.frameId));
    const standIn = (id, uid, dx) => {
      const frame = structuredClone(document._styk.frameSources[track.keys[0].value.frameId]);
      frame.objects = frame.objects.filter(object => object.unique_id === 1).map(object => ({...object, unique_id: uid, object_slot: 0, root_x: object.root_x + dx}));
      frame.object_count = 1; frame.object_draw_order = [0]; frame.object_draw_order_unique_ids = [uid];
      frame._styk = {...frame._styk, frameId: id};
      document._styk.frameSources[id] = frame;
    };
    standIn('frame-loose', 8, 35); standIn('frame-held', 9, -30);
    track.keys[0].movieclipPatches = [{uid: 1, frameId: 'frame-loose'}];
    track.keys[1].movieclipPatches = [{uid: 1, frameId: 'frame-held', replacementUid: 9}];
    return {document: open(document, {layerTimeline: true}).serialize(), ticks: [0, 3, 6]};
  },
  /** An imported animation (four native keys, two joined limbs) with its second and
   * fourth layer keys edited in the layer timeline: the layer then mixes imported and
   * authored keys, which tween through a window of the keys around the playhead. */
  'sheet-mixed-keys': () => {
    const native = open(documentOf([limb(true), limb()], i => [240 + i * 150, 160]));
    native.select(2); native.joinTo(1, 33);
    for (const [dx, turn] of [[30, 0.5], [-10, -0.9], [45, 0.4]]) { nextKey(native, 4); native.select(1); native.moveSelection(dx, 12); native.rotateSelection(turn); }
    const e = open(native.serialize(), {layerTimeline: true});
    e.seek(4); e.select(1); e.moveSelection(25, -30);
    e.seek(12); e.select(2, 1); e.rotateJoint(0.7);
    e.seek(0);
    return {document: e.serialize(), ticks: [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]};
  },
};

const args = process.argv.slice(2);
if (args.includes('--list')) { console.log(Object.keys(fixtures).join('\n')); process.exit(0); }
const outAt = args.indexOf('--out');
const out = path.resolve(outAt < 0 ? `${here}/../crates/styk-eval/tests/fixtures/eval` : args[outAt + 1]);
const names = args.filter((a, i) => !a.startsWith('--') && (outAt < 0 || i !== outAt + 1));
for (const name of names) if (!fixtures[name]) { console.error(`unknown fixture ${name}`); process.exit(2); }
mkdirSync(out, {recursive: true});
for (const name of names.length ? names : Object.keys(fixtures)) {
  const built = fixtures[name]();
  writeReference(writeProject(out, name, built.document, built.ticks), name);
}
