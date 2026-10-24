#!/usr/bin/env node
// The drawing raster engine against the JavaScript raster engine on the drawing workers'
// work (docs/engine/drawing.md): the same requests through both engines' worker entry
// points in Node (V8, as in the app's workers; the Rust engine as the WebAssembly module
// the workers load), interleaved round by round, after a warm-up round.
//
//   node scripts/benchmark-drawing-engine.mjs [--rounds 5] [--out artifacts/drawing-engine/bench.json]
//   node scripts/benchmark-drawing-engine.mjs --memory     peak resident memory per workload,
//                                                         one fresh process per engine and workload
//
// Workloads: pen strokes through the stroke worker's protocol (every batch timed: the
// worker's share of input-to-pixels latency), dab shapes from the brush model (the
// interface thread's share), fill, magic wand, blur, hue/saturation, selection capture
// and a rotated commit, resampling, tile publication (PNG encoding and blob assembly),
// cel decoding with reduced levels, undo chunks. Medians and ranges of the rounds.
import {spawnSync} from 'node:child_process';
import {mkdirSync, writeFileSync} from 'node:fs';
import path from 'node:path';
import {fileURLToPath, pathToFileURL} from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const args = process.argv.slice(2), option = (name, fallback) => { const i = args.indexOf(name); return i >= 0 ? args[i + 1] : fallback; };
const rootUrl = pathToFileURL(root).href;
const src = name => import(`${rootUrl}/src/${name}.mjs`);
const {TileStore, TILE} = await src('drawing-tiles');
const {BrushStroke, normalizeBrush} = await src('drawing-brush');
const {DrawingStroke, lassoMask} = await src('drawing-pixels');
const {StrokeEngine} = await src('drawing-stroke-engine');
const {drawingWorkerOperation} = await src('drawing-worker-operations');
const {encodeTileBlob} = await src('drawing-tile-codec');
const {encodeHistoryChunk, decodeHistoryChunk} = await src('drawing-history-codec');
const {materializeDrawingTiles} = await src('drawing-preparation');
const engineModule = await import(`${rootUrl}/src/engine/engine-drawing.mjs`);
const {drawingEngineOperation} = await import(`${rootUrl}/src/engine/engine-drawing-worker.mjs`);
const PRESETS = (await src('drawing-view')).BRUSH_PRESETS;
if (!await engineModule.loadDrawingEngine({enabled: true})) throw new Error('Build the drawing engine first: npm run engine:build');

// ---------------------------------------------------------------- inputs

/** A dense painted cel: `strokes` ink, soft and textured strokes over about 1080p at 2x. */
function denseCel(strokes = 24) {
  const store = new TileStore();
  let s = 1;
  const rnd = () => (s = (Math.imul(s, 1103515245) + 12345) >>> 0) / 2 ** 32;
  const kinds = ['ink', 'soft', 'pencil', 'marker', 'charcoal'];
  for (let k = 0; k < strokes; k++) {
    const p = PRESETS.find(preset => preset.id === kinds[k % kinds.length]);
    const stroke = new DrawingStroke(store, {...p, size: p.size * 3, color: [rnd() * 255 | 0, rnd() * 255 | 0, rnd() * 255 | 0], seed: k});
    const x0 = rnd() * 3600, y0 = rnd() * 2000;
    for (let i = 0; i < 90; i++) stroke.sample({x: x0 + Math.cos(i / 9 + k) * 300 + i * 4, y: y0 + Math.sin(i / 7) * 160, pressure: .3 + .7 * Math.abs(Math.sin(i / 20)), time: i * 6});
    stroke.finish();
  }
  // Line art closing a region for the fill.
  const ring = new DrawingStroke(store, {size: 10, color: [20, 20, 20], seed: 99});
  for (let i = 0; i <= 200; i++) { const t = i / 200 * Math.PI * 2; ring.sample({x: 1900 + Math.cos(t) * 700, y: 1080 + Math.sin(t) * 500, time: i}); }
  ring.finish();
  return store;
}
const tilesOf = store => [...store].map(t => ({tx: t.tx, ty: t.ty, pixels: t.pixels.slice().buffer}));
const pen = (n, x, y, w) => Array.from({length: n}, (_, i) => { const t = i / (n - 1); return {x: x + t * w, y: y + Math.sin(t * 8) * 220 + Math.sin(t * 31) * 25, pressure: .15 + .85 * Math.abs(Math.sin(t * 5)), time: i * 4.2, ax: Math.cos(t * 3), ay: Math.sin(t * 3)}; });

/** A stroke through the stroke worker's protocol as the controller drives it. */
function strokeJob(preset, scale, base) {
  const p = PRESETS.find(q => q.id === preset), settings = {...p, size: p.size * scale, color: [200, 30, 60], seed: 7, workingBytes: 768 * 1024 * 1024};
  const brush = normalizeBrush(settings), shapes = [], batches = [];
  const stroke = new BrushStroke(brush, shape => shapes.push(shape));
  const samples = pen(600, 300, 1000, 3200), sent = new Set();
  let version = -1;
  const flush = () => {
    const tail = brush.taperOut > 0 && stroke.tailVersion !== version ? stroke.tailShapes() : undefined;
    if (tail) version = stroke.tailVersion;
    const dabs = shapes.splice(0), tiles = [];
    for (const d of tail ? [...dabs, ...tail] : dabs) for (let ty = Math.floor(d.top / TILE); ty * TILE < d.bottom; ty++) for (let tx = Math.floor(d.left / TILE); tx * TILE < d.right; tx++) {
      if (sent.has(`${tx},${ty}`)) continue; sent.add(`${tx},${ty}`); tiles.push({tx, ty, pixels: base.get(tx, ty)?.pixels ?? null});
    }
    batches.push({dabs, tiles, tail});
  };
  samples.forEach((sample, i) => { stroke.sample(sample); if (i % 6 === 5) flush(); });
  stroke.finish(); flush();
  return {settings, batches};
}
/** Runs a recorded stroke on an engine; the time of each batch and of the end. */
function runStroke(Engine, job) {
  const engine = new Engine(), times = [];
  engine.begin(1, job.settings);
  let seq = 0;
  for (const b of job.batches) {
    const tiles = b.tiles.map(t => ({tx: t.tx, ty: t.ty, pixels: t.pixels ? t.pixels.slice().buffer : null}));
    const start = performance.now(); engine.dabs(1, ++seq, b.dabs, tiles, b.tail); times.push(performance.now() - start);
  }
  const start = performance.now(); engine.end(1); const end = performance.now() - start;
  return {total: times.reduce((n, t) => n + t, 0) + end, batches: times, end};
}
async function runOperation(engine, data) {
  let reply;
  const post = message => { reply = message; };
  const copy = structuredClone(data);
  const start = performance.now();
  if (engine === 'javascript' && data.kind === 'prepare-tiles') materializeDrawingTiles(copy.tiles);
  else if (engine === 'javascript') { const previous = globalThis.self; globalThis.self = {postMessage: post}; try { await drawingWorkerOperation(copy); } finally { globalThis.self = previous; } }
  else await drawingEngineOperation(copy, post);
  const elapsed = performance.now() - start;
  if (reply?.error) throw new Error(`${data.kind}: ${reply.error}`);
  return elapsed;
}

// ---------------------------------------------------------------- workloads

const cel = denseCel(), tiles = tilesOf(cel), bounds = {x: -128, y: -128, width: 4096, height: 2432};
const lasso = lassoMask(Array.from({length: 96}, (_, i) => ({x: 1900 + Math.cos(i / 96 * Math.PI * 2) * 900, y: 1080 + Math.sin(i / 96 * Math.PI * 2) * 650})));
const selectionPoints = Array.from({length: 96}, (_, i) => ({x: 1500 + Math.cos(i / 96 * Math.PI * 2) * 700, y: 1000 + Math.sin(i / 96 * Math.PI * 2) * 500}));
const blobURL = await encodeTileBlob(copyStore(cel));
function copyStore(store) { const out = new TileStore(); for (const t of store) out.adopt(t.tx, t.ty, t.pixels.slice()); return out; }
const strokes = [['ink', 3], ['pencil', 3], ['soft', 5], ['charcoal', 2], ['airbrush', 2], ['oil', 2], ['brushpen', 3]].map(([preset, scale]) => ({name: `stroke ${preset} ${preset === 'soft' ? 160 : Math.round(PRESETS.find(p => p.id === preset).size * scale)} px`, job: strokeJob(preset, scale, cel)}));
const selected = await (async () => { let reply; await drawingEngineOperation({id: 0, kind: 'select', tiles: tilesOf(cel), points: selectionPoints, maxBytes: 768 * 1024 * 1024}, m => { reply = m; }); return reply.tiles.map(t => ({tx: t.tx, ty: t.ty, pixels: t.full ? cel.get(t.tx, t.ty).pixels.slice().buffer : t.pixels, alphaBounds: t.alphaBounds})); })();
const historyTiles = [...cel].slice(0, 60).map((t, i) => ({tx: t.tx, ty: t.ty, before: i % 3 ? t.pixels.slice() : null, after: i % 4 ? new Uint8ClampedArray([i, 0, 0, 255]) : t.pixels.slice()}));
const keys = [...cel].map(t => `${t.tx},${t.ty}`);
const operations = [
  ['fill enclosed (gap 2, expand 1)', {kind: 'fill', tiles, bounds, options: {x: 1900, y: 1080, color: [255, 200, 0, 255], tolerance: 16, expansion: 1, gap: 2, clip: null, maxBytes: 768 * 1024 * 1024}}],
  ['fill whole stage', {kind: 'fill', tiles, bounds, options: {x: 5, y: 2300, color: [30, 90, 200, 255], tolerance: 16, expansion: 0, gap: 0, clip: null, maxBytes: 768 * 1024 * 1024}}],
  ['magic wand (gap 1)', {kind: 'flood-mask', tiles, bounds: {x: 0, y: 0, width: 3840, height: 2160}, options: {x: 1900, y: 1080, tolerance: 16, gap: 1}}],
  ['blur 2.7 MP, radius 12, masked', {kind: 'filter', tiles, bounds: {x: 1000, y: 430, width: 1800, height: 1300}, options: {kind: 'blur', radius: 12, mask: lasso}}],
  ['hue/saturation, whole cel', {kind: 'filter', tiles, bounds, options: {kind: 'hsl', hue: 40, saturation: 20, lightness: -10}}],
  ['capture a lasso selection', {kind: 'select', tiles, points: selectionPoints, maxBytes: 768 * 1024 * 1024}],
  ['commit a rotated, scaled selection', {kind: 'selection', tiles, selection: {matrix: [1.0392, .6, -.6, 1.0392, 400.5, -700.25], tiles: selected, remove: false, filter: 'smooth', maxBytes: 768 * 1024 * 1024}}],
  ['resample to half', {kind: 'resample', tiles, ratio: .5, filter: 'smooth', maxBytes: 768 * 1024 * 1024}],
  ['resample to double', {kind: 'resample', tiles: tiles.slice(0, 400), ratio: 2, filter: 'smooth', maxBytes: 768 * 1024 * 1024}],
  [`publish ${keys.length} tiles (PNG + blob)`, {kind: 'encode-tiles', celId: 'bench', keep: keys, tiles}],
  [`decode ${keys.length} tiles with levels`, {kind: 'decode-tiles', url: blobURL, levels: 3}],
  ['prepare 64 tiles for a commit', {kind: 'prepare-tiles', tiles: tiles.slice(0, 64)}],
];

// ---------------------------------------------------------------- runs

const median = values => { const s = [...values].sort((a, b) => a - b); return s[Math.floor(s.length / 2)]; };
const percentile = (values, p) => { const s = [...values].sort((a, b) => a - b); return s[Math.min(s.length - 1, Math.floor(s.length * p))]; };
const fmt = v => v >= 100 ? v.toFixed(0) : v >= 10 ? v.toFixed(1) : v.toFixed(2);
const cell = values => `${fmt(median(values))} (${fmt(Math.min(...values))}–${fmt(Math.max(...values))})`;

if (args.includes('--one')) {
  // One workload on one engine in a fresh process: the peak resident memory of the job
  // alone (Linux: the peak is reset after the inputs are built, /proc/self/clear_refs).
  const [name, engine] = [option('--one'), option('--engine')];
  const {readFileSync: read, writeFileSync: write} = await import('node:fs');
  const status = key => Number(read('/proc/self/status', 'utf8').match(new RegExp(`${key}:\\s+(\\d+)`))[1]) * 1024;
  // A fresh instance for the job: the inputs above were prepared with the module too.
  engineModule.releaseDrawingEngineMemory(0);
  globalThis.gc?.(); globalThis.gc?.();
  write('/proc/self/clear_refs', '5');
  const before = status('VmRSS'), wasmBefore = engineModule.drawingEngineMemory();
  const stroke = strokes.find(s => s.name === name), operation = operations.find(o => o[0] === name);
  if (stroke) runStroke(engine === 'rust' ? engineModule.StrokeEngine : StrokeEngine, stroke.job);
  else await runOperation(engine, operation[1]);
  console.log(JSON.stringify({name, engine, rssBefore: before, maxRss: status('VmHWM'), wasmBefore, wasmBytes: engine === 'rust' ? engineModule.drawingEngineMemory() : 0}));
  process.exit(0);
}

if (args.includes('--memory')) {
  const rows = [];
  console.log('| Workload | JavaScript: peak resident growth MiB | Rust: peak resident growth MiB | Rust: linear memory growth MiB |\n| --- | ---: | ---: | ---: |');
  for (const name of [...strokes.map(s => s.name), ...operations.map(o => o[0])]) {
    const row = {name};
    for (const engine of ['javascript', 'rust']) {
      const result = spawnSync(process.execPath, ['--expose-gc', fileURLToPath(import.meta.url), '--one', name, '--engine', engine], {encoding: 'utf8', maxBuffer: 1 << 24});
      if (result.status !== 0) throw new Error(result.stderr);
      const line = JSON.parse(result.stdout.trim().split('\n').at(-1));
      row[engine] = {peakMiB: (line.maxRss - line.rssBefore) / 2 ** 20, maxRssMiB: line.maxRss / 2 ** 20, wasmMiB: (line.wasmBytes - line.wasmBefore) / 2 ** 20};
    }
    rows.push(row);
    console.log(`| ${name} | ${row.javascript.peakMiB.toFixed(0)} | ${row.rust.peakMiB.toFixed(0)} | ${row.rust.wasmMiB.toFixed(0)} |`);
  }
  const out = option('--out', path.join(root, 'artifacts/drawing-engine/memory.json'));
  mkdirSync(path.dirname(out), {recursive: true}); writeFileSync(out, JSON.stringify(rows, null, 2));
  process.exit(0);
}

const rounds = Number(option('--rounds', 5));
const results = {};
const record = (name, engine, value) => ((results[name] ??= {javascript: [], rust: []})[engine]).push(value);
const brushSamples = pen(5000, 0, 1000, 3600);
for (let round = 0; round <= rounds; round++) {
  const order = round % 2 ? ['rust', 'javascript'] : ['javascript', 'rust'];
  for (const engine of order) {
    // The interface thread's share: input samples to dab shapes.
    for (const preset of ['ink', 'liner', 'brushpen']) {
      const p = PRESETS.find(q => q.id === preset), brush = normalizeBrush({...p, size: p.size * 3, seed: 3});
      let count = 0;
      const stroke = engine === 'rust' ? new engineModule.RustBrushStroke(brush, () => count++) : new BrushStroke(brush, () => count++);
      const start = performance.now(); for (const s of brushSamples) { stroke.sample(s); stroke.tailShapes?.(); } stroke.finish();
      const elapsed = performance.now() - start;
      if (round) record(`brush model ${preset}: µs per sample (${count} dabs)`, engine, elapsed * 1000 / brushSamples.length);
    }
    for (const {name, job} of strokes) {
      const run = runStroke(engine === 'rust' ? engineModule.StrokeEngine : StrokeEngine, job);
      if (round) { record(`${name}: worker total ms (${job.batches.length} batches)`, engine, run.total); record(`${name}: batch p95 ms`, engine, percentile(run.batches, .95)); record(`${name}: pen-up ms`, engine, run.end); }
    }
    for (const [name, data] of operations) { const elapsed = await runOperation(engine, {id: 1, ...data}); if (round) record(name, engine, elapsed); }
    const start = performance.now();
    for (let i = 0; i < 20; i++) { const bytes = engine === 'rust' ? engineModule.encodeHistoryChunk(historyTiles) : encodeHistoryChunk(historyTiles); engine === 'rust' ? engineModule.decodeHistoryChunk(bytes) : decodeHistoryChunk(bytes); }
    if (round) record('undo chunk encode + decode ×20 (60 tiles)', engine, performance.now() - start);
  }
  process.stderr.write(`round ${round}/${rounds}\n`);
}
const lines = ['| Workload | JavaScript | Rust (WebAssembly) | Rust / JavaScript |', '| --- | ---: | ---: | ---: |'];
for (const [name, {javascript, rust}] of Object.entries(results)) lines.push(`| ${name} | ${cell(javascript)} | ${cell(rust)} | ${(median(rust) / median(javascript)).toFixed(2)} |`);
console.log(lines.join('\n'));
const out = option('--out', path.join(root, 'artifacts/drawing-engine/bench.json'));
mkdirSync(path.dirname(out), {recursive: true});
writeFileSync(out, JSON.stringify({rounds, node: process.version, tiles: keys.length, results}, null, 2));
