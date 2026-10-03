#!/usr/bin/env node
// Records what the editor's evaluated frame stores, for every editor-built fixture
// (engine/crates/styk-eval/tests/fixtures/{eval,ik,vfx,spatial}/*.json and the command-replay
// documents), into tests/fixtures/records/<name>/ for styk-eval's tests/records.rs.
//   node engine/tools/records-fixtures.mjs
// Byte-stable: the files change only when the editor's evaluation does.
import {execFileSync} from 'node:child_process';
import {readdirSync, rmSync} from 'node:fs';
import path from 'node:path';
const here = path.dirname(new URL(import.meta.url).pathname);
const fixtures = path.resolve(here, '../crates/styk-eval/tests/fixtures'), out = path.join(fixtures, 'records');
rmSync(out, {recursive: true, force: true});
const sources = [];
for (const group of ['eval', 'ik', 'vfx', 'spatial'])
  for (const file of readdirSync(path.join(fixtures, group)).sort())
    if (file.endsWith('.json') && !file.endsWith('.reference.json') && file !== 'vectors.json') sources.push([`${group}-${file.slice(0, -5)}`, path.join(fixtures, group, file)]);
for (const file of readdirSync(path.resolve(here, '../tests/edit')).sort())
  if (file.endsWith('.document.json')) sources.push([`edit-${file.slice(0, -14)}`, path.resolve(here, '../tests/edit', file)]);
// A Pivot-eased angle that winds past π over several keys: the editor unwraps it along the
// whole run of keys, so the stored tween sits whole turns from the one unwrapped per key.
{
  const {documentOf, limb} = await import('./fixture-kit.mjs'), {mkdtempSync, writeFileSync} = await import('node:fs'), {tmpdir} = await import('node:os');
  const document = documentOf([limb()]), [first] = document.keyframes;
  document.auto_easing = true;
  const wrap = angle => Math.atan2(Math.sin(angle), Math.cos(angle));
  document.keyframes = [0, 1, 2, 3, 4, 5].map(step => ({...structuredClone(first), duration_raw: 4,
    camera: {is_default: false, x: 0, y: 0, scale: 1, angle_rad: wrap(step * 2.2)},
    objects: first.objects.map(object => ({...structuredClone(object), angles: [wrap(0.8 + step * 2.5), wrap(2 - step * 2.9), wrap(step * 1.4)]}))}));
  const file = path.join(mkdtempSync(path.join(tmpdir(), 'styk-records-')), 'legacy-winding.json');
  writeFileSync(file, JSON.stringify(document));
  sources.push(['eval-legacy-winding', file]);
}
for (const [name, file] of sources) execFileSync(process.execPath, [path.join(here, 'records-reference.mjs'), file, path.join(out, name), '--full'], {stdio: ['ignore', 'ignore', 'inherit']});
console.log(`${sources.length} documents recorded`);
