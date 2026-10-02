#!/usr/bin/env node
// The engine's parity suite in one command (docs/RUST-ENGINE-PORT.md, "Parity harness").
//
//   npm run engine:parity                # the repository tier: needs nothing outside the checkout
//   npm run engine:parity -- --corpus    # also the private corpus (~/Downloads), where present
//   npm run engine:parity -- --app       # also the app with and without the engine (Chromium)
//
// Repository tier: the Rust fixture suites (IK, evaluation, effects, draw lists, the Pivot
// codec's recorded answers, command replays), the container reader and writer against src/native-container.mjs
// on every native file in the repository, and fixture regeneration (byte-stable: a fixture
// that changes means the editor's behaviour changed).
// Corpus tier: geometry of every tick of the benchmark files against the editor
// (engine/tools/reference.mjs + styk compare) and the Pivot codec replay when a recording
// exists. App tier: scripts/engine-app-parity.mjs on the start-up document and the IK
// fixtures. Exit code 1 when any check fails.
import {spawnSync} from 'node:child_process';
import {existsSync, mkdtempSync, readdirSync, rmSync, statSync, writeFileSync} from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const root = path.resolve(import.meta.dirname, '..');
const args = new Set(process.argv.slice(2));
const results = [];
const run = (name, command, commandArgs, options = {}) => {
  const started = Date.now();
  const result = spawnSync(command, commandArgs, {cwd: root, stdio: options.quiet ? 'pipe' : 'inherit', encoding: 'utf8', ...options});
  const ok = result.status === 0 && (!options.check || options.check(result));
  results.push({name, ok, seconds: (Date.now() - started) / 1000});
  if (!ok && options.quiet) process.stdout.write(`${result.stdout ?? ''}${result.stderr ?? ''}`.split('\n').slice(-30).join('\n') + '\n');
  return result;
};
const nice = process.platform === 'win32' ? [] : ['nice', '-n', '10'];
const cargo = (...rest) => [nice[0] ?? 'cargo', [...nice.slice(1), ...(nice.length ? ['cargo'] : []), ...rest, '--manifest-path', 'engine/Cargo.toml']];

// Fixture suites (fast: a few seconds once built).
run('Rust fixture suites', ...cargo('test', '--release', '-p', 'styk-doc', '-p', 'styk-formats', '-p', 'styk-draw', '-p', 'styk-eval', '-p', 'styk-engine', '-p', 'styk-render', '-p', 'styk-raster', '-p', 'styk-piv', '-p', 'styk-edit'));

// Fixture generators are byte-stable: regenerating changes nothing.
for (const tool of ['ik-fixtures', 'eval-fixtures', 'vfx-fixtures']) {
  if (!existsSync(path.join(root, `engine/tools/${tool}.mjs`))) continue;
  run(`${tool} regenerate unchanged`, process.execPath, [`engine/tools/${tool}.mjs`], {quiet: true,
    check: () => spawnSync('git', ['diff', '--quiet', '--', 'engine/crates/styk-eval/tests/fixtures'], {cwd: root}).status === 0});
}

// Command recordings are byte-stable too: re-recording them in the JavaScript editor changes
// nothing (styk-edit's test above replays them in the engine).
run('edit recordings regenerate unchanged', process.execPath, ['engine/tools/edit-scenarios.mjs'], {quiet: true,
  check: () => spawnSync(process.execPath, ['engine/tools/edit-replay.mjs', '--all'], {cwd: root}).status === 0
    && spawnSync('git', ['diff', '--quiet', '--', 'engine/tests/edit'], {cwd: root}).status === 0});

// Containers: every native file in the repository, against the JavaScript reader.
run('styk CLI build', ...cargo('build', '--release', '-p', 'styk-cli'), {quiet: true});
const natives = [];
const walk = dir => {
  for (const entry of readdirSync(path.join(root, dir), {withFileTypes: true})) {
    const relative = path.join(dir, entry.name);
    if (entry.isDirectory()) { if (!['node_modules', 'target', '.git', 'tmp', 'build', 'dist'].includes(entry.name) && !entry.name.startsWith('.')) walk(relative); }
    else if (/\.(styk|stkz|stykmov)$/i.test(entry.name) && statSync(path.join(root, relative)).size < 64 * 1024 * 1024) natives.push(path.join(root, relative));
  }
};
for (const dir of ['artifacts', 'AssetReleases', 'tests', 'assets']) if (existsSync(path.join(root, dir))) walk(dir);
const temp = mkdtempSync(path.join(os.tmpdir(), 'styk-parity-'));
try {
  const list = path.join(temp, 'native-files.txt');
  writeFileSync(list, natives.join('\n') + '\n');
  run(`containers (${natives.length} repository files)`, process.execPath, ['engine/tools/container-parity.mjs', list], {quiet: true});
} finally { rmSync(temp, {recursive: true, force: true}); }

if (args.has('--corpus')) {
  const home = os.homedir();
  const files = [`${home}/Downloads/TDD2 URIEL NEMESIS 3.styk`, `${home}/Downloads/Jay on a new app (repaired).styk`].filter(existsSync);
  for (const file of files) {
    const reference = path.join(os.tmpdir(), `styk-reference-${path.basename(file).replace(/\W+/g, '-')}.json`);
    run(`reference ${path.basename(file)}`, process.execPath, ['engine/tools/reference.mjs', file, reference, '--every', '1', '--vfx'], {quiet: true});
    run(`geometry and effects ${path.basename(file)}`, 'engine/target/release/styk', ['compare', file, reference], {quiet: true,
      check: result => /, 0 mismatched primitives, \d+ joints, 0 mismatched joints, 0 frames with differences/.test(result.stdout) && !/[1-9]\d* mismatched effects/.test(result.stdout)});
  }
  if (!files.length) results.push({name: 'corpus (not present on this machine)', ok: true, seconds: 0});
  const recording = path.join(root, 'engine/target/piv-parity');
  if (existsSync(recording)) run('Pivot codec replay', ...cargo('run', '--release', '-p', 'styk-piv', '--bin', 'styk-piv', '--', 'replay', recording), {quiet: true});
}

if (args.has('--app')) {
  run('app: start-up document', process.execPath, ['scripts/engine-app-parity.mjs', '--label', 'parity-startup', '--onion', '--play', '--require-engine'], {quiet: true});
  for (const fixture of ['startup-pinned', 'joined-stretch'])
    run(`app: IK fixture ${fixture}`, process.execPath, ['scripts/engine-app-parity.mjs', '--file', `engine/crates/styk-eval/tests/fixtures/ik/${fixture}.json`, '--label', `parity-${fixture}`, '--require-engine'], {quiet: true});
}

console.log('\nEngine parity');
for (const {name, ok, seconds} of results) console.log(`  ${ok ? 'ok  ' : 'FAIL'} ${name} (${seconds.toFixed(1)} s)`);
process.exit(results.every(result => result.ok) ? 0 : 1);
