/**
 * Builds the engine's WebAssembly module into src/engine/pkg/ (git-ignored):
 *
 *   node scripts/engine-build.mjs            release build (threads, wasm-opt when available)
 *   node scripts/engine-build.mjs --dev      faster build, no wasm-opt
 *
 * The module uses WebAssembly threads over one shared memory, which needs the
 * standard library rebuilt with atomics: the pinned nightly toolchain below, with
 * `rust-src` and the wasm32 target (see docs/engine/wasm.md for the setup).
 */
import {spawnSync} from 'node:child_process';
import {existsSync, mkdirSync, statSync, writeFileSync, readFileSync, copyFileSync} from 'node:fs';
import {createHash} from 'node:crypto';
import path from 'node:path';

const root = path.resolve(import.meta.dirname, '..');
const engine = path.join(root, 'engine');
const out = path.join(root, 'src/engine/pkg');
export const TOOLCHAIN = 'nightly-2026-09-15';
export const WASM_BINDGEN = '0.2.129';
// 2 GiB of address space: every thread shares it, and a shared memory's maximum is fixed when it links.
const MAX_MEMORY = 2147483648;
const dev = process.argv.includes('--dev');
const profile = dev ? 'release' : 'wasm';

function run(command, args, options = {}) {
  const result = spawnSync(command, args, {stdio: 'inherit', cwd: engine, ...options});
  if (result.error) throw new Error(`${command}: ${result.error.message}`);
  if (result.status !== 0) process.exit(result.status ?? 1);
}
function has(command, args = ['--version']) {
  const result = spawnSync(command, args, {encoding: 'utf8'});
  return result.status === 0 ? result.stdout.trim() : null;
}

const bindgen = has('wasm-bindgen');
if (!bindgen?.endsWith(WASM_BINDGEN))
  throw new Error(`wasm-bindgen ${WASM_BINDGEN} is required (found ${bindgen ?? 'none'}): cargo install wasm-bindgen-cli --version ${WASM_BINDGEN} --locked`);

const rustflags = ['-C', 'target-feature=+atomics,+bulk-memory,+mutable-globals', '-C', 'link-arg=--shared-memory', '-C', 'link-arg=--import-memory',
  '-C', `link-arg=--max-memory=${MAX_MEMORY}`, '-C', 'link-arg=--export=__wasm_init_tls', '-C', 'link-arg=--export=__tls_size',
  '-C', 'link-arg=--export=__tls_align', '-C', 'link-arg=--export=__tls_base'];
run('cargo', [`+${TOOLCHAIN}`, 'build', '--profile', profile, '-p', 'styk-wasm', '--target', 'wasm32-unknown-unknown', '-Z', 'build-std=std,panic_abort'],
  {env: {...process.env, CARGO_ENCODED_RUSTFLAGS: rustflags.join('\x1f'), CARGO_TARGET_DIR: path.join(engine, 'target/wasm-threads')}});

mkdirSync(out, {recursive: true});
const built = path.join(engine, 'target/wasm-threads/wasm32-unknown-unknown', profile, 'styk_wasm.wasm');
run('wasm-bindgen', ['--target', 'web', '--out-dir', out, '--out-name', 'styk_engine', '--no-typescript', ...(dev ? ['--keep-debug'] : ['--remove-name-section', '--remove-producers-section']), built]);
const wasm = path.join(out, 'styk_engine_bg.wasm');
const before = statSync(wasm).size;
if (!dev && has('wasm-opt')) {
  run('wasm-opt', ['-O3', '--enable-threads', '--enable-bulk-memory', '--enable-mutable-globals', '--enable-nontrapping-float-to-int', '--enable-sign-ext', '--enable-simd', '-o', wasm, wasm]);
}
const bytes = readFileSync(wasm);
const info = {profile, toolchain: TOOLCHAIN, wasmBindgen: WASM_BINDGEN, maxMemory: MAX_MEMORY, wasmBytes: bytes.length, wasmBytesBeforeOpt: before,
  jsBytes: statSync(path.join(out, 'styk_engine.js')).size, sha256: createHash('sha256').update(bytes).digest('hex')};
writeFileSync(path.join(out, 'build-info.json'), JSON.stringify(info, null, 2));
console.log(`engine: ${path.relative(root, wasm)} ${(bytes.length / 1024).toFixed(0)} KiB (${info.sha256.slice(0, 12)})${!dev && !has('wasm-opt') ? ' — wasm-opt not found, module not size-optimized' : ''}`);
