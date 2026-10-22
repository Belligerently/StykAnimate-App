#!/usr/bin/env node
// The valid movieclip graph boundary, built through the editor before the depth gate opens.
import {mkdirSync} from 'node:fs';
import path from 'node:path';
import {here, repo, repoUrl, documentOf, limb, open, whole, nextKey, emptyDocument, createObject, createFrame, writeProject, writeReference} from './fixture-kit.mjs';
const {movieclipFigure, validateMovieclipGraph} = await import(`${repoUrl}/src/movieclips.mjs`);

export function nested(depth) {
  const e = open(documentOf([limb()], () => [180, 160]));
  nextKey(e, 4); whole(e).moveSelection(70, 20); e.rotateSelection(.7);
  let document = e.serialize();
  for (let level = 0; level < depth; level++) {
    const report = emptyDocument(), clip = movieclipFigure(1, `Level ${level + 1}`, document, {duration: 5});
    report.figure_types = [clip]; report.keyframes = [createFrame([createObject(clip, 1, 0, 0)], 4)];
    const outer = open(report); nextKey(outer, 4); document = outer.serialize();
  }
  return document;
}

const args = process.argv.slice(2), outAt = args.indexOf('--out');
const out = path.resolve(outAt < 0 ? `${here}/../crates/styk-eval/tests/fixtures/document` : args[outAt + 1]);
mkdirSync(out, {recursive: true});
const document = nested(8);
validateMovieclipGraph(document);
writeReference(writeProject(out, 'deep-eight', document, [0, 1, 2, 3, 4], {native: true}), 'deep-eight');
// Construct the rejected wrapper directly: the editor must reject it before evaluation.
const invalid = emptyDocument(), clip = movieclipFigure(1, 'Ninth level', document, {duration: 5});
invalid.figure_types = [clip]; invalid.keyframes = [createFrame([createObject(clip, 1, 0, 0)], 4)];
for (const depth of [9, 17]) {
  let report = invalid;
  for (let level = 9; level < depth; level++) {
    const outer = emptyDocument(), next = movieclipFigure(1, `Level ${level + 1}`, report, {duration: 5});
    outer.figure_types = [next]; outer.keyframes = [createFrame([createObject(next, 1, 0, 0)], 4)]; report = outer;
  }
  try { open(report); throw new Error(`Depth ${depth} was accepted`); }
  catch (error) { if (!/eight nested/.test(error.message)) throw error; }
  console.log(`depth ${depth}: rejected by editor's graph validation`);
}
