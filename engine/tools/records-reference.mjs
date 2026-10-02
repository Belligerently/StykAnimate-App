#!/usr/bin/env node
// What the editor's evaluated frame stores when a gesture commits (docs/engine/editing.md,
// "The evaluated frame as records"): for each tick, `snapshotPoseFrame(editor.evaluatedFrame)`
// as JSON, byte for byte, next to the loaded report the engine receives.
//
//   node engine/tools/records-reference.mjs <file.styk|.piv|document.json> <out-folder> [--ticks a,b] [--every N] [--max N] [--full]
//
// Writes <out>/report.json (the editor's report as the mirror sends it: without the
// `runtime_segments` caches) and <out>/records.json: {ticks: [{tick, sha256, bytes, json?}]}
// (`json` with --full, or always for documents under 2 MiB). The editor is configured as in
// src/app.mjs, so every document has a layer sheet. `styk records <out>` compares.
import {createHash} from 'node:crypto';
import {mkdirSync, readFileSync, writeFileSync} from 'node:fs';
import path from 'node:path';
const here = path.dirname(new URL(import.meta.url).pathname), repo = path.resolve(here, '../..');
const {decodeDocument, normalizeDecodedDocument} = await import(`${repo}/src/native-container.mjs`);
const {Editor} = await import(`${repo}/src/editor.mjs`);
const {snapshotPoseFrame} = await import(`${repo}/src/pose-snapshot.mjs`);
const [input, out, ...rest] = process.argv.slice(2);
if (!input || !out) { console.error('usage: node engine/tools/records-reference.mjs <file> <out-folder> [--ticks a,b] [--every N] [--max N] [--full]'); process.exit(2); }
const option = name => { const i = rest.indexOf(name); return i < 0 ? undefined : rest[i + 1]; };
let document;
if (/\.(styk|stkz|stykmov)$/i.test(input)) {
  const kind = /\.(styk|stkz|stykmov)$/i.exec(input)[1].toLowerCase();
  document = normalizeDecodedDocument((await decodeDocument(new Uint8Array(readFileSync(input)), kind)).document, kind);
} else if (/\.(piv|stk)$/i.test(input)) {
  // Through the repository's codec, as tools/reference.mjs does.
  const {execFileSync} = await import('node:child_process'), {tmpdir} = await import('node:os'), {rmSync} = await import('node:fs');
  const decoded = path.join(tmpdir(), `styk-records-${process.pid}.json`);
  execFileSync(`${here}/decode-piv.sh`, [input, decoded], {stdio: ['ignore', 'ignore', 'inherit']});
  document = JSON.parse(readFileSync(decoded, 'utf8')); rmSync(decoded, {force: true});
} else document = JSON.parse(readFileSync(input, 'utf8'));
const editor = new Editor({demo: false, propertyChannels: true, layerTimeline: true}).load(document, path.basename(input));
const strip = (key, value) => key === 'runtime_segments' ? undefined : value;
const report = JSON.stringify(editor.report, strip);
let ticks;
if (option('--ticks')) ticks = option('--ticks').split(',').map(Number);
else { const every = Number(option('--every') ?? 1); ticks = []; for (let t = 0; t < editor.totalTicks; t += every) ticks.push(t); }
ticks = ticks.filter(t => t >= 0 && t < editor.totalTicks).slice(0, Number(option('--max') ?? Infinity));
const full = rest.includes('--full') || report.length < 2 * 1024 * 1024;
const records = ticks.map(tick => {
  editor.seek(tick);
  const json = JSON.stringify(snapshotPoseFrame(editor.evaluatedFrame), strip);
  return {tick, sha256: createHash('sha256').update(json).digest('hex'), bytes: Buffer.byteLength(json), ...(full ? {json} : {})};
});
mkdirSync(out, {recursive: true});
writeFileSync(path.join(out, 'report.json'), report + '\n');
writeFileSync(path.join(out, 'records.json'), JSON.stringify({file: path.basename(input), ticks: records}) + '\n');
console.log(`${path.basename(input)}: report ${(report.length / 1024).toFixed(1)} KiB, ${records.length} ticks`);
