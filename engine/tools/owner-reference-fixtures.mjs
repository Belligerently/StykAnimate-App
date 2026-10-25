// Records the references of the owner tests (tests/support/owner-references.mjs) into tests/fixtures/owner-references/.
//
// Each of those tests compares an engine-owned editor with what an editor the engine does not own gives for the same calls.
// This runs every such test file once in recording mode, which runs the plain editor and keeps what the tests compare the
// owned editor with. The fixtures are then the oracle (as the edit tapes' `.reference.json` files are): regenerate them only
// when the contract a test states changes, never to make a failing comparison pass.
//
//   node engine/tools/owner-reference-fixtures.mjs [test file name ...]
import {spawnSync} from 'node:child_process';
import {copyFileSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync} from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {fileURLToPath} from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const testsFolder = path.join(root, 'tests'), fixtures = path.join(testsFolder, 'fixtures/owner-references');
if (!existsSync(path.join(root, 'src/engine/pkg/styk_engine_bg.wasm'))) {
  console.error('The engine is not built (npm run engine:build): the owned editors the references are compared with need it.');
  process.exit(1);
}
const requested = process.argv.slice(2).map(name => name.replace(/^tests[\\/]/, ''));
const files = readdirSync(testsFolder).filter(name => name.endsWith('.test.mjs') && readFileSync(path.join(testsFolder, name), 'utf8').includes('support/owner-references.mjs'))
  .filter(name => !requested.length || requested.includes(name)).sort();
if (!files.length) { console.error('No test file takes references.'); process.exit(1); }

const scratch = mkdtempSync(path.join(os.tmpdir(), 'styk-owner-references-'));
mkdirSync(fixtures, {recursive: true});
let failed = 0;
try {
  for (const file of files) {
    const started = Date.now();
    const run = spawnSync(process.execPath, ['--import', './tests/support/register-engine.mjs', path.join('tests', file)], {
      cwd: root, encoding: 'utf8', maxBuffer: 256 * 1024 * 1024, env: {...process.env, STYK_RECORD_OWNER_REFERENCES: scratch},
    });
    const output = path.join(scratch, file.replace(/\.test\.mjs$/, '.json'));
    if (run.status !== 0 || !existsSync(output)) {
      failed++;
      console.error(`${file}: not recorded (exit ${run.status})\n${`${run.stdout}\n${run.stderr}`.split('\n').filter(line => /not ok|Error|error:/.test(line)).slice(0, 12).join('\n')}`);
      continue;
    }
    copyFileSync(output, path.join(fixtures, path.basename(output)));
    console.log(`${file}: recorded in ${((Date.now() - started) / 1000).toFixed(1)}s`);
  }
} finally { rmSync(scratch, {recursive: true, force: true}); }
process.exit(failed ? 1 : 0);
