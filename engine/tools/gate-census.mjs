// Which native files the engine draws, by the interface's own gate
// (src/engine/engine-eligibility.mjs): each file is loaded as the app loads it (the editor
// configured as in src/app.mjs, so every document has a layer sheet) and the gate names the
// first feature the engine cannot draw, or none.
//
//   node engine/tools/gate-census.mjs <file-or-folder>... [--out census.json]
//
// Pivot animations (.piv) go through the repository's codec (engine/tools/decode-piv.mjs);
// Pivot figures (.stk) are left out: opening one adds it to the animation on screen. Prints
// the count drawn and the files refused per reason.
import {readdirSync, readFileSync, statSync, writeFileSync, rmSync} from 'node:fs';
import {tmpdir} from 'node:os';
import path from 'node:path';
import {fileURLToPath, pathToFileURL} from 'node:url';
import {decodePiv} from './decode-piv.mjs';

const here = path.dirname(fileURLToPath(import.meta.url)), repo = path.resolve(here, '../..');
const repoUrl = pathToFileURL(repo).href;
const {openDocument} = await import(`${repoUrl}/tests/support/native-files.mjs`);
const {Editor} = await import(`${repoUrl}/src/editor.mjs`);
const {EngineEligibility} = await import(`${repoUrl}/src/engine/engine-eligibility.mjs`);

const args = process.argv.slice(2), outAt = args.indexOf('--out');
const out = outAt < 0 ? null : args[outAt + 1];
const inputs = args.filter((arg, i) => !arg.startsWith('--') && (outAt < 0 || i !== outAt + 1));
if (!inputs.length) { console.error('usage: node engine/tools/gate-census.mjs <file-or-folder>... [--out census.json]'); process.exit(2); }

const NATIVE = /\.(styk|stkz|stykmov|piv)$/i;
const files = [];
const walk = entry => {
  if (statSync(entry).isDirectory()) for (const name of readdirSync(entry).sort()) walk(path.join(entry, name));
  else if (NATIVE.test(entry)) files.push(entry);
};
for (const input of inputs) walk(path.resolve(input));

async function load(file) {
  const kind = NATIVE.exec(file)[1].toLowerCase();
  if (kind === 'piv') {
    const decoded = path.join(tmpdir(), `styk-census-${process.pid}.json`);
    try {
      decodePiv(file, decoded);
      return JSON.parse(readFileSync(decoded, 'utf8'));
    } finally { rmSync(decoded, {force: true}); }
  }
  return openDocument(new Uint8Array(readFileSync(file)), kind, {verifySource: false});
}

const results = [];
for (const file of files) {
  let reason, failed = null;
  try {
    const editor = new Editor({demo: false, propertyChannels: true, layerTimeline: true}).load(await load(file), path.basename(file));
    reason = new EngineEligibility().check(editor.report, editor.revision);
  } catch (error) { failed = String(error?.message ?? error).slice(0, 200); }
  results.push({file, reason: reason ?? null, failed});
}
const drawn = results.filter(r => !r.failed && !r.reason).length;
const tally = new Map();
for (const r of results) if (r.reason || r.failed) {
  const key = r.failed ? 'failed to load in Node' : r.reason;
  tally.set(key, (tally.get(key) ?? 0) + 1);
}
console.log(`${results.length} native files: ${drawn} drawn by the engine`);
for (const [reason, count] of [...tally].sort((a, b) => b[1] - a[1])) console.log(`  ${count}  ${reason}`);
if (out) writeFileSync(out, JSON.stringify({files: results.map(r => ({...r, file: path.relative(repo, r.file).startsWith('..') ? path.basename(r.file) : path.relative(repo, r.file)}))}, null, 1) + '\n');
