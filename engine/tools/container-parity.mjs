#!/usr/bin/env node
// Container reader parity: the Rust reader and the JavaScript reader (the oracle,
// src/native-container.mjs) must produce the same document for every file.
//
//   node engine/tools/container-parity.mjs <file-or-list.txt>… [--styk <path to styk>]
//
// A `.txt` argument is a list of files, one per line. For each file the JSON text
// of both documents is compared byte for byte (JSON.stringify on the JS side,
// `styk container json` on the Rust side). Exit code 1 when any file differs.
import {pathToFileURL} from 'node:url';
import {readFileSync, mkdtempSync, rmSync} from 'node:fs';
import {spawnSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import os from 'node:os';
import path from 'node:path';

const repo = path.resolve(import.meta.dirname, '../..');
const repoUrl = pathToFileURL(repo).href;
const {decodeDocument} = await import(`${repoUrl}/src/native-container.mjs`);
const args = process.argv.slice(2);
const option = args.indexOf('--styk');
const styk = option >= 0 ? args.splice(option, 2)[1] : path.join(repo, `engine/target/release/styk${process.platform === 'win32' ? '.exe' : ''}`);
const files = args.flatMap(arg => arg.endsWith('.txt') ? readFileSync(arg, 'utf8').split('\n').filter(Boolean) : [arg]);
if (!files.length) { console.error('usage: node engine/tools/container-parity.mjs <file-or-list.txt>…'); process.exit(2); }
const temp = mkdtempSync(path.join(os.tmpdir(), 'styk-parity-'));
const sha = bytes => createHash('sha256').update(bytes).digest('hex');
let same = 0, different = 0, bothReject = 0;
try {
  for (const file of files) {
    const kind = path.extname(file).slice(1).toLowerCase();
    let expected = null, jsError = null;
    try { expected = Buffer.from(JSON.stringify((await decodeDocument(new Uint8Array(readFileSync(file)), kind)).document)); }
    catch (error) { jsError = error.messageKey || error.message; }
    const out = path.join(temp, 'rust.json');
    const run = spawnSync(styk, ['container', 'json', file, out], {encoding: 'utf8'});
    if (jsError || run.status !== 0) {
      if (jsError && run.status !== 0) { bothReject++; continue; }
      different++; console.log(`DIFFERENT ${file}: JavaScript ${jsError ? `rejects (${jsError})` : 'accepts'}, Rust ${run.status ? `rejects (${run.stderr.trim()})` : 'accepts'}`);
      continue;
    }
    const actual = readFileSync(out);
    if (actual.equals(expected)) same++;
    else {
      different++;
      let at = 0; while (at < actual.length && actual[at] === expected[at]) at++;
      console.log(`DIFFERENT ${file}: first difference at byte ${at}: JS …${expected.subarray(Math.max(0, at - 40), at + 40)}… Rust …${actual.subarray(Math.max(0, at - 40), at + 40)}… (${sha(expected).slice(0, 12)} vs ${sha(actual).slice(0, 12)})`);
    }
  }
} finally { rmSync(temp, {recursive: true, force: true}); }
console.log(JSON.stringify({files: files.length, same, bothReject, different}));
process.exit(different ? 1 : 0);
