// The drawing raster engine (Rust, src/engine/engine-drawing.mjs) behind the drawing workers'
// interfaces: every pixel-worker request, the stroke worker's protocol, the brush model and
// the undo codec answer exactly as the JavaScript engine does. The crate's own parity suite
// (engine/crates/styk-drawing/tests/parity.rs) checks the kernels; this checks the facades.
import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync, readdirSync} from 'node:fs';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import english from '../src/locales/en.mjs';
import {loadDrawingEngine, StrokeEngine as EngineStrokes, RustBrushStroke, encodeHistoryChunk as engineEncode, decodeHistoryChunk as engineDecode, drawingEngineEnabled, drawingEngineMemory, releaseDrawingEngineMemory, drawingEngineGeneration} from '../src/engine/engine-drawing.mjs';
import {drawingEngineOperation} from '../src/engine/engine-drawing-worker.mjs';
import {drawingWorkerOperation} from '../src/drawing-worker-operations.mjs';
import {materializeDrawingTiles} from '../src/drawing-preparation.mjs';
import {StrokeEngine} from '../src/drawing-stroke-engine.mjs';
import {BrushStroke, normalizeBrush} from '../src/drawing-brush.mjs';
import {TileStore, TILE} from '../src/drawing-tiles.mjs';
import {encodeHistoryChunk, decodeHistoryChunk} from '../src/drawing-history-codec.mjs';
import {encodeTileBlob} from '../src/drawing-tile-codec.mjs';
import {encodeRGBAPNG, drawingPNGDataURL} from '../src/drawing-png.mjs';
import {lassoMask, ellipsePoints} from '../src/drawing-pixels.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const engine = await loadDrawingEngine({enabled: true});

/** Soft and hard discs, a uniform tile, empty space between, negative coordinates. */
function artwork(seed = 3) {
  let s = seed; const rnd = () => (s = (Math.imul(s, 1103515245) + 12345) >>> 0) / 2 ** 32;
  const store = new TileStore();
  for (let k = 0; k < 6; k++) {
    const cx = -80 + rnd() * 300, cy = -60 + rnd() * 220, r = 8 + rnd() * 36, c = [rnd() * 255 | 0, rnd() * 255 | 0, rnd() * 255 | 0], soft = k % 2;
    for (let y = Math.floor(cy - r); y < cy + r; y++) for (let x = Math.floor(cx - r); x < cx + r; x++) {
      const d = Math.hypot(x + .5 - cx, y + .5 - cy); if (d > r) continue;
      const tile = store.writable(Math.floor(x / TILE), Math.floor(y / TILE)), i = ((y - tile.ty * TILE) * TILE + x - tile.tx * TILE) * 4;
      tile.pixels.set([...c, Math.round((soft ? 1 - d / r : Math.min(1, r - d)) * 255)], i);
    }
  }
  store.set(3, 2, new Uint8ClampedArray([12, 200, 40, 255]));
  return store;
}
const tilesOf = store => [...store].map(t => ({tx: t.tx, ty: t.ty, pixels: t.pixels.slice().buffer}));
/** Runs one request through both engines on independent copies of its data. */
async function both(data) {
  const javascript = [], rust = [];
  const previous = globalThis.self;
  globalThis.self = {postMessage: message => javascript.push(message)};
  try { await drawingWorkerOperation(structuredClone(data)); } finally { globalThis.self = previous; }
  await drawingEngineOperation(structuredClone(data), message => rust.push(message));
  return {javascript: javascript[0], rust: rust[0]};
}
const same = async (name, data) => {
  const {javascript, rust} = await both(data);
  assert.deepStrictEqual(rust, javascript, name);
  return rust;
};

test('the drawing engine module loads in Node and the gate reads the page', () => {
  assert.ok(engine, 'src/engine/pkg/styk_drawing.js is built (npm run engine:build)');
  assert.equal(drawingEngineEnabled({search: '?engine=off'}), false);
  assert.equal(drawingEngineEnabled({search: '?engine=off&drawingEngine=on'}), true);
  assert.equal(drawingEngineEnabled({search: '?drawingEngine=off'}), false);
});

test('every pixel-worker request answers as the JavaScript worker does', async () => {
  const art = artwork(), tiles = tilesOf(art), bounds = {x: -160, y: -128, width: 448, height: 384};
  const lasso = lassoMask(ellipsePoints({x: -40, y: -30}, {x: 150, y: 120}, 48));
  await same('fill', {id: 1, kind: 'fill', tiles, bounds, options: {x: 20, y: 30, color: [200, 10, 10, 255], tolerance: 16, expansion: 1, gap: 2, clip: null, maxBytes: 768 * 1024 * 1024}});
  await same('fill clipped', {id: 2, kind: 'fill', tiles, bounds, options: {x: 20, y: 30, color: [0, 0, 255, 128], tolerance: 40, expansion: 0, gap: 0, clip: lasso, maxBytes: 768 * 1024 * 1024}});
  await same('fill refused', {id: 3, kind: 'fill', tiles, bounds: {...bounds, x: .5}, options: {x: 0, y: 0, color: [1, 2, 3, 255]}});
  await same('wand', {id: 4, kind: 'flood-mask', tiles, bounds, options: {x: 20, y: 30, tolerance: 16, gap: 1}});
  await same('wand outside', {id: 5, kind: 'flood-mask', tiles, bounds, options: {x: 9999, y: 30, tolerance: 16, gap: 0}});
  for (const options of [{kind: 'blur', radius: 6}, {kind: 'hsl', hue: 90, saturation: 20, lightness: -10, mask: lasso}, {kind: 'brightness', brightness: 20, contrast: 30}, {kind: 'invert', mask: lasso}, {kind: 'emboss'}])
    await same(`filter ${options.kind}`, {id: 6, kind: 'filter', tiles, bounds, options});
  await same('fill mask', {id: 7, kind: 'fill-mask', tiles, mask: lasso, color: [10, 250, 10, 200]});
  const points = ellipsePoints({x: -50, y: -40}, {x: 130, y: 110}, 64);
  const selected = await same('select', {id: 8, kind: 'select', tiles, points, maxBytes: 384 * 1024 * 1024});
  await same('select by mask', {id: 9, kind: 'select', tiles, mask: lasso, maxBytes: 384 * 1024 * 1024});
  const source = selected.tiles.map(t => ({tx: t.tx, ty: t.ty, pixels: t.full ? art.get(t.tx, t.ty).pixels.slice().buffer : t.pixels, alphaBounds: t.alphaBounds}));
  await same('commit selection', {id: 10, kind: 'selection', tiles, selection: {matrix: [.9, .3, -.3, .9, 14.5, -6], tiles: source, remove: false, filter: 'smooth', maxBytes: 384 * 1024 * 1024}});
  await same('delete selection', {id: 11, kind: 'selection', tiles, selection: {matrix: [1, 0, 0, 1, 0, 0], tiles: source, remove: true, filter: 'nearest'}});
  const copied = await same('copy', {id: 12, kind: 'copy-selection', tiles: source.slice(0, 4).map((t, i) => i % 2 ? {tx: t.tx, ty: t.ty, png: encodeRGBAPNG(new Uint8ClampedArray(t.pixels), 64, 64).buffer} : t), maxBytes: 384 * 1024 * 1024});
  const clip = {version: 2, pixelsPerUnit: 2, world: {x: 0, y: 0}, matrix: [1, 0, 0, 1, 0, 0], bounds: {x: -256, y: -256, width: 768, height: 768}, tiles: copied.tiles};
  await same('paste', {id: 13, kind: 'paste', tiles, clip, matrix: [1.1, 0, .2, 1, 33, 7], filter: 'smooth', maxBytes: 384 * 1024 * 1024});
  for (const ratio of [.5, 2, .25]) await same(`resample ${ratio}`, {id: 14, kind: 'resample', tiles, ratio, filter: 'smooth', maxBytes: 384 * 1024 * 1024});
  await same('encode PNGs', {id: 15, kind: 'encode-pngs', tiles});
  const url = await encodeTileBlob(art);
  await same('decode tiles', {id: 16, kind: 'decode-tiles', url, levels: 3});
  await same('decode a shard', {id: 17, kind: 'decode-tiles', url, levels: 3, shard: [1, 2]});
  const legacy = drawingPNGDataURL(encodeRGBAPNG(new Uint8ClampedArray(130 * 70 * 4).map((_, i) => i % 4 === 3 ? (i % 11 ? 255 : 0) : (i * 7) & 255), 130, 70));
  await same('decode legacy', {id: 18, kind: 'decode-legacy', url: legacy, crop: {x: -17, y: 40}});
  const keys = [...art].map(t => `${t.tx},${t.ty}`);
  for (const request of [{celId: 'a', keep: keys, tiles: tiles.slice(0, 3), partial: true}, {celId: 'a', tiles: tiles.slice(3), partial: true}, {celId: 'a', keep: keys, tiles: []}, {celId: 'b', keep: keys.slice(1), tiles: tiles.slice(1, 2)}, {celId: 'c', keep: ['0,0'], tiles: []}])
    await same(`publish ${request.celId}`, {id: 19, kind: 'encode-tiles', ...request});
  await same('unknown', {id: 20, kind: 'nothing'});
});

test('commit preparation answers as the JavaScript preparation does', async () => {
  const art = artwork(5), [first] = [...art];
  const input = [{tx: 0, ty: 0, pixels: first.pixels.slice().buffer}, {tx: 1, ty: 0, pixels: new Uint8ClampedArray([1, 2, 3, 4]).buffer}, {tx: 2, ty: 0, pixels: null}, {tx: 3, ty: 0, pixels: new Uint8ClampedArray([1, 2, 3, 0]).buffer}];
  const replies = [];
  await drawingEngineOperation({id: 1, kind: 'prepare-tiles', tiles: structuredClone(input)}, message => replies.push(message));
  assert.deepStrictEqual(replies[0], {id: 1, tiles: materializeDrawingTiles(structuredClone(input))});
});

test('the stroke worker protocol paints the same batches and undo records', () => {
  const base = artwork(7);
  for (const settings of [{size: 14, taperIn: 20, taperOut: 60, color: [10, 20, 30], seed: 4}, {size: 30, hardness: .3, flow: .4, tip: 'chalk', texture: .5, grain: 'paper', color: [200, 0, 100], seed: 9},
    {size: 26, blend: 'smudge', smudgeLength: .7, colorRate: .2, color: [255, 255, 0], seed: 2}, {size: 20, erase: true, seed: 1}, {size: 18, blend: 'multiply', alphaLock: true, color: [0, 128, 255], seed: 3, mask: lassoMask(ellipsePoints({x: 0, y: 0}, {x: 120, y: 90}, 40))}]) {
    const run = engine => {
      engine.begin(1, settings);
      const out = [], sent = new Set(), brush = normalizeBrush(settings);
      let pending = [], seq = 0, version = -1;
      const stroke = new BrushStroke(brush, shape => pending.push(shape));
      const flush = () => {
        const tail = brush.taperOut > 0 && stroke.tailVersion !== version ? stroke.tailShapes() : undefined;
        if (tail) version = stroke.tailVersion;
        const tiles = [];
        for (const d of tail ? [...pending, ...tail] : pending) for (let ty = Math.floor(d.top / TILE); ty * TILE < d.bottom; ty++) for (let tx = Math.floor(d.left / TILE); tx * TILE < d.right; tx++) {
          if (sent.has(`${tx},${ty}`)) continue; sent.add(`${tx},${ty}`); tiles.push({tx, ty, pixels: base.get(tx, ty)?.pixels.slice().buffer ?? null});
        }
        out.push(engine.dabs(1, ++seq, pending, tiles, tail)); pending = [];
      };
      for (let i = 0; i < 80; i++) { const t = i / 79; stroke.sample({x: -30 + t * 260, y: 40 + Math.sin(t * 6) * 50, pressure: .2 + .8 * Math.abs(Math.sin(t * 3)), time: i * 4}); if (i % 9 === 8) flush(); }
      stroke.finish(); flush();
      out.push(engine.dabs(1, 1, [], [], undefined), engine.end(1), engine.end(1));
      return out;
    };
    assert.deepStrictEqual(run(new EngineStrokes()), run(new StrokeEngine()), JSON.stringify(settings, (k, v) => k === 'mask' ? undefined : v));
  }
  const refused = new EngineStrokes();
  refused.begin(2, {size: 10});
  assert.throws(() => refused.dabs(2, 1, [{x: 5, y: 5, radius: 5, opacity: 1, flow: 1, cos: 1, sin: 0, left: 0, top: 0, right: 10, bottom: 10}], [], undefined), error => error.messageKey === 'errors.drawingStrokeEngine.aBrushDabReachedDrawingPixelsThatWere');
});

test('the brush model gives the controller the same dab shapes, tails and pen curve', () => {
  for (const settings of [{size: 16, taperIn: 30, taperOut: 70, seed: 5}, {size: 9, stabilization: .6, stabilizerMode: 'string', pressureCurve: [.1, .5, .4, .95], seed: 6}, {size: 40, scatter: 1.5, sizeJitter: .5, angleJitter: 1, roundness: .4, angleMode: 'tilt', tip: 'bristle', seed: 7}]) {
    const brush = normalizeBrush(settings), rust = [], javascript = [];
    const a = new RustBrushStroke(brush, shape => rust.push(shape), {seed: brush.seed + 7919}), b = new BrushStroke(brush, shape => javascript.push(shape), {seed: brush.seed + 7919});
    const tails = [[], []];
    for (let i = 0; i < 70; i++) {
      const t = i / 69, input = {x: t * 200, y: Math.cos(t * 5) * 40, pressure: .1 + .9 * t, time: i * 3, ax: Math.cos(t * 4), ay: Math.sin(t * 4)};
      a.sample(input); b.sample(input);
      tails[0].push(a.tailShapes()); tails[1].push(b.tailShapes());
      assert.equal(a.tailVersion, b.tailVersion);
    }
    a.finish(); b.finish();
    assert.deepStrictEqual(rust, javascript, JSON.stringify(settings));
    assert.deepStrictEqual(tails[0], tails[1]);
    assert.deepStrictEqual(a.tailShapes(), b.tailShapes());
    assert.equal(a.hasTail, b.hasTail);
    assert.equal(a.pressure?.(.3), b.pressure?.(.3));
  }
});

test('undo chunks encode and decode as the JavaScript codec does', () => {
  const art = artwork(9), [a, b] = [...art];
  const tiles = [{tx: 0, ty: 0, before: null, after: a.pixels.slice()}, {tx: -9, ty: 4, before: new Uint8ClampedArray([1, 2, 3, 4]), after: b.pixels.slice()}, {tx: 2 ** 31 - 1, ty: -(2 ** 31), before: b.pixels.slice(), after: null}];
  const bytes = engineEncode(tiles);
  assert.deepStrictEqual(bytes, encodeHistoryChunk(tiles));
  assert.deepStrictEqual(engineDecode(bytes), decodeHistoryChunk(bytes));
  assert.throws(() => engineDecode(bytes.subarray(0, 40)), error => error.messageKey === 'errors.drawingHistoryCodec.drawingUndoDataIsDamaged');
  assert.throws(() => engineEncode([]), error => error.messageKey === 'errors.drawingHistoryCodec.invalidDrawingHistoryChunkSize');
});

test('every error the drawing engine can raise has the editor\'s message', () => {
  const keys = new Set();
  for (const dir of ['engine/crates/styk-drawing/src', 'engine/crates/styk-drawing-wasm/src'])
    for (const file of readdirSync(path.join(root, dir))) for (const [, key] of readFileSync(path.join(root, dir, file), 'utf8').matchAll(/"(errors\.[A-Za-z0-9.]+)"/g)) keys.add(key);
  assert.ok(keys.size > 50);
  assert.deepEqual([...keys].filter(key => !Object.hasOwn(english, key)), []);
});

// Last: it replaces this thread's instance.
test('an instance gives back the memory a large job grew it to, and work goes on', async () => {
  const art = artwork(11), replies = [], generation = drawingEngineGeneration();
  // A 9-megapixel blur needs more than an instance may keep: the pixel worker replaces it afterwards.
  await drawingEngineOperation({id: 1, kind: 'filter', tiles: tilesOf(art), bounds: {x: -1500, y: -1500, width: 3000, height: 3000}, options: {kind: 'blur', radius: 8}}, message => replies.push(message));
  assert.ok(!replies[0].error, replies[0].error);
  assert.equal(drawingEngineGeneration(), generation + 1, 'the grown instance was replaced');
  assert.ok(drawingEngineMemory() < 32 * 2 ** 20, `the replacement starts small (${drawingEngineMemory()} bytes)`);
  // A 2-megapixel blur stays within the allowance; a smaller limit releases it.
  await drawingEngineOperation({id: 2, kind: 'filter', tiles: tilesOf(art), bounds: {x: -700, y: -700, width: 1400, height: 1400}, options: {kind: 'blur', radius: 8}}, message => replies.push(message));
  const grown = drawingEngineMemory();
  assert.ok(grown > 64 * 2 ** 20 && drawingEngineGeneration() === generation + 1, `kept within the allowance (${grown} bytes)`);
  assert.equal(releaseDrawingEngineMemory(64 * 2 ** 20), true);
  assert.ok(drawingEngineMemory() < 32 * 2 ** 20);
  assert.equal(releaseDrawingEngineMemory(64 * 2 ** 20), false, 'a small instance is kept');
  // The pixel worker answers on the fresh instance; a stroke engine renews itself between strokes.
  await same('fill after the replacement', {id: 3, kind: 'fill', tiles: tilesOf(art), bounds: {x: -160, y: -128, width: 448, height: 384}, options: {x: 20, y: 30, color: [9, 9, 9, 255]}});
  const strokes = new EngineStrokes(), reference = new StrokeEngine();
  for (const engine of [strokes, reference]) engine.begin(4, {size: 12, seed: 1});
  const dab = {x: 10.5, y: 12.25, radius: 6, opacity: 1, flow: 1, cos: 1, sin: 0, left: 4, top: 6, right: 17, bottom: 19};
  assert.deepStrictEqual(strokes.dabs(4, 1, [dab], [{tx: 0, ty: 0, pixels: null}]), reference.dabs(4, 1, [dab], [{tx: 0, ty: 0, pixels: null}]));
  assert.deepStrictEqual(strokes.end(4), reference.end(4));
});
