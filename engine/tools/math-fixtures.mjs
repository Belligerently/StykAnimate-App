#!/usr/bin/env node
// What V8's Math returns, for styk-math's tests (engine/crates/styk-math/tests/v8.rs).
//   node engine/tools/math-fixtures.mjs
// sincos.bin: [x, Math.sin(x), Math.cos(x)] per input; atan2-hypot.bin: [y, x, Math.atan2(y, x),
// Math.hypot(y, x)]; asin-acos.bin: [x, Math.asin(x), Math.acos(x)]. Little-endian doubles. Inputs come from a fixed sequence, so the files only
// change if V8's results do. Ordinary angles, tiny and large values, every n·π/2 up to 64 and its
// neighbours, the kernel's switch points and special values.
import {writeFileSync} from 'node:fs';
const out = new URL('../crates/styk-math/tests/', import.meta.url).pathname;
let seed = 2026; const rnd = () => { seed = (seed * 1103515245 + 12345) >>> 0; return seed / 4294967296; };
const nudge = (value, ulps) => { const f = new Float64Array([value]), u = new BigInt64Array(f.buffer); u[0] += BigInt(ulps); return f[0]; };
const xs = [0, -0, NaN, Infinity, -Infinity, 5e-324, -5e-324, 2.2250738585072014e-308, 1e-300, 1e300, -1e300, 1.7976931348623157e308, 823550, 823551, 1e6, 1e9, 1e15, 1e22, 1e100];
for (let n = 1; n <= 64; n++) for (const d of [-1, 0, 1]) xs.push(nudge(n * Math.PI / 2, d), -nudge(n * Math.PI / 2, d));
for (let i = 0; i < 1500; i++) xs.push((rnd() - .5) * 4 * Math.PI, 0.29 + rnd() * 0.02, 0.78 + rnd() * 0.01, (rnd() - .5) * 10 ** (rnd() * 30));
const sincos = new Float64Array(xs.length * 3);
xs.forEach((x, i) => sincos.set([x, Math.sin(x), Math.cos(x)], i * 3));
writeFileSync(`${out}sincos.bin`, Buffer.from(sincos.buffer));
const pairs = [[0, 0], [-0, 0], [0, -0], [-0, -0], [1, 0], [0, -1], [NaN, 1], [Infinity, Infinity], [-Infinity, 3], [1e-320, 1e-320], [3, 4], [1e308, 1e308], [5, 1], [-0, 1], [Infinity, -Infinity], [-Infinity, -Infinity], [2, -Infinity], [-2, Infinity], [1, NaN]];
for (let e = 55; e <= 70; e++) for (const [sy, sx] of [[1, 1], [1, -1], [-1, 1], [-1, -1]]) pairs.push([sy * 2 ** e * 1.37, sx * 1.01], [sy * 0.73, sx * 2 ** e * 1.13]);
for (let i = 0; i < 2000; i++) pairs.push([(rnd() - .5) * 800, (rnd() - .5) * 800], [(rnd() - .5) * 10 ** (rnd() * 20 - 10), (rnd() - .5) * 10 ** (rnd() * 20 - 10)]);
const atan = new Float64Array(pairs.length * 4);
pairs.forEach(([y, x], i) => atan.set([y, x, Math.atan2(y, x), Math.hypot(y, x)], i * 4));
writeFileSync(`${out}atan2-hypot.bin`, Buffer.from(atan.buffer));
// asin and acos: the whole domain, fdlibm's switch points (|x| = 0.5, 0.975, 2^-26) and
// their neighbours, values next to ±1, out of range and special values.
const unit = [0, -0, NaN, Infinity, -Infinity, 1, -1, 5e-324, -5e-324, 2 ** -26, -(2 ** -26), 2 ** -27, 1e-300, 1.0000000000000002, -1.0000000000000002, 2, -3];
for (const point of [0.5, 0.975, 1, 2 ** -26]) for (const d of [-3, -2, -1, 0, 1, 2, 3]) unit.push(nudge(point, d), -nudge(point, d));
for (let i = 0; i < 3000; i++) unit.push(rnd() * 2 - 1, 0.45 + rnd() * 0.1, 0.97 + rnd() * 0.03, -(0.97 + rnd() * 0.03), (rnd() - .5) * 10 ** (-rnd() * 20));
const inverse = new Float64Array(unit.length * 3);
unit.forEach((x, i) => inverse.set([x, Math.asin(x), Math.acos(x)], i * 3));
writeFileSync(`${out}asin-acos.bin`, Buffer.from(inverse.buffer));
console.log(`${xs.length} sine and cosine inputs, ${pairs.length} atan2 and hypot pairs, ${unit.length} asin and acos inputs`);
