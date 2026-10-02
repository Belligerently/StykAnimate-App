import test from 'node:test';
import assert from 'node:assert/strict';
import {existsSync, readFileSync} from 'node:fs';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {declaredMemoryPages} from '../src/engine/engine-memory.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const HEADER = [0, 0x61, 0x73, 0x6d, 1, 0, 0, 0];
const name = text => [text.length, ...Buffer.from(text)];
const section = (id, body) => [id, body.length, ...body];

test('the engine memory starts at the pages the module declares', () => {
  // (type (func)) (import "a" "f" (func)) (import "a" "g" (global i32)) (import "env" "memory" (memory 26 32768 shared))
  const module = Uint8Array.from([...HEADER,
    ...section(1, [1, 0x60, 0, 0]),
    ...section(2, [3, ...name('a'), ...name('f'), 0, 0, ...name('a'), ...name('g'), 3, 0x7f, 0, ...name('env'), ...name('memory'), 2, 3, 26, 0x80, 0x80, 0x02])]);
  assert.equal(WebAssembly.validate(module, {}), true);
  assert.equal(declaredMemoryPages(module), 26);
  // A view into a larger buffer (a Node Buffer from a pool) and a plain ArrayBuffer read the same.
  const padded = new Uint8Array(module.length + 7);
  padded.set(module, 7);
  assert.equal(declaredMemoryPages(padded.subarray(7)), 26);
  assert.equal(declaredMemoryPages(module.buffer), 26);
});

test('a memory of more than 127 pages and a module without one', () => {
  const large = Uint8Array.from([...HEADER, ...section(2, [1, ...name('env'), ...name('memory'), 2, 0, 0xac, 0x02])]);
  assert.equal(declaredMemoryPages(large), 300);
  assert.throws(() => declaredMemoryPages(Uint8Array.from([...HEADER, ...section(1, [1, 0x60, 0, 0])])), /does not import its memory/);
});

test('the built engine module declares what its generated loader would create', {skip: !existsSync(path.join(root, 'src/engine/pkg/styk_engine_bg.wasm'))}, () => {
  const pkg = path.join(root, 'src/engine/pkg');
  const generated = /new WebAssembly\.Memory\(\{initial:(\d+),/.exec(readFileSync(path.join(pkg, 'styk_engine.js'), 'utf8'));
  assert.ok(generated, 'the generated loader names the initial size');
  assert.equal(declaredMemoryPages(readFileSync(path.join(pkg, 'styk_engine_bg.wasm'))), Number(generated[1]));
});
