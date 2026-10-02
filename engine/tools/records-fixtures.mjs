#!/usr/bin/env node
// Records what the editor's evaluated frame stores, for every editor-built fixture
// (engine/crates/styk-eval/tests/fixtures/{eval,ik,vfx}/*.json and the command-replay
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
for (const group of ['eval', 'ik', 'vfx'])
  for (const file of readdirSync(path.join(fixtures, group)).sort())
    if (file.endsWith('.json') && !file.endsWith('.reference.json') && file !== 'vectors.json') sources.push([`${group}-${file.slice(0, -5)}`, path.join(fixtures, group, file)]);
for (const file of readdirSync(path.resolve(here, '../tests/edit')).sort())
  if (file.endsWith('.document.json')) sources.push([`edit-${file.slice(0, -14)}`, path.resolve(here, '../tests/edit', file)]);
for (const [name, file] of sources) execFileSync(process.execPath, [path.join(here, 'records-reference.mjs'), file, path.join(out, name), '--full'], {stdio: ['ignore', 'ignore', 'inherit']});
console.log(`${sources.length} documents recorded`);
