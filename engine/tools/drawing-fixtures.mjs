#!/usr/bin/env node
// The drawing raster engine's parity fixtures (docs/engine/drawing.md): real strokes, fills,
// selections, transforms, filters, reduced levels, tile and history codecs run through the
// JavaScript drawing engine in Node, recorded as their inputs (pointer samples with
// pressure and tilt, brush settings, tile states) and what the JavaScript produced (dab
// shapes, every batch the stroke worker returns, undo records, tiles, encoded bytes, error
// keys). engine/crates/styk-drawing/tests/parity.rs replays them in Rust and compares.
//
//   node engine/tools/drawing-fixtures.mjs            regenerate every fixture file
//   node engine/tools/drawing-fixtures.mjs --dump N   print the JavaScript side of case N in full
//
// Outputs are SHA-256 prefixes (16 hex digits) of exact bytes: tiles, dab shapes as
// little-endian doubles, PNGs. Inputs are exact (numbers as JSON writes them, tiles as
// deflated base64). Regeneration is byte-stable: every input is generated from fixed seeds.
import './v8-pin.mjs';
import {pathToFileURL} from 'node:url';
import {mkdirSync, writeFileSync} from 'node:fs';
import {createHash} from 'node:crypto';
import path from 'node:path';

const repo = path.resolve(import.meta.dirname, '../..');
const repoUrl = pathToFileURL(repo).href;
const out = path.join(repo, 'engine/crates/styk-drawing/tests/fixtures');
const src = name => import(`${repoUrl}/src/${name}.mjs`);
const {TileStore, TILE, TILE_BYTES} = await src('drawing-tiles');
const {BrushStroke, normalizeBrush, dabShape, pressureMap, jitterAt, StrokePath} = await src('drawing-brush');
const {brushAtlas, ensureTexture, ATLAS_BYTES} = await src('drawing-brush-textures');
const pixels = await src('drawing-pixels');
const {DrawingStroke, floodTiles: floodDense, lassoMask, combineMasks, invertMask, cropMask, maskEdges, fillMaskTiles, filterTiles, rectMask, rectPoints, ellipsePoints, tileChanges} = pixels;
const {StrokeEngine} = await src('drawing-stroke-engine');
const {strokeRegion} = await src('drawing-stroke-region');
const {floodTiles} = await src('drawing-fill');
const {captureSelection, captureMaskSelection} = await src('drawing-lasso');
const {transformTiles, transformTileCoverage, indexedTileCoverage, transformSupportBounds, inverseMatrix} = await src('drawing-transform');
const {transformSelectedPixels, selectionMatrix} = await src('drawing-selection');
const {levelStore} = await src('drawing-raster');
const {deflateZlib} = await src('zlib-sync');
const {encodeRGBAPNG, decodeDrawingPNGBytes} = await src('drawing-png');
const {assembleTileBlobBytes, encodeTile, decodeTileBlob, TileEncodeCache, TILE_BLOB_PREFIX} = await src('drawing-tile-codec');
const {encodeHistoryChunk, decodeHistoryChunk, historyChunks} = await src('drawing-history-codec');
const {materializeDrawingTiles} = await src('drawing-preparation');
const {encodeClipboardTiles, decodePixelClipboard} = await src('drawing-clipboard');
const {bytesToBase64} = await src('base64');

// ------------------------------------------------------------------ helpers

const hash = bytes => createHash('sha256').update(bytes instanceof Uint8Array || bytes instanceof Uint8ClampedArray ? bytes : new Uint8Array(bytes)).digest('hex').slice(0, 16);
const b64 = bytes => Buffer.from(bytes.buffer, bytes.byteOffset, bytes.byteLength).toString('base64');
/** A tile state as fixture input: deflated pixels in base64. */
const packTiles = store => [...store].map(t => [t.tx, t.ty, b64(deflateZlib(t.pixels))]);
const tileHashes = store => [...store].map(t => [t.tx, t.ty, hash(t.pixels)]);
const side = v => v == null ? null : v.byteLength === 4 ? 'u' + Buffer.from(v.buffer, v.byteOffset, 4).toString('hex') : hash(v);
const changesOf = changes => changes.map(c => [c.tx, c.ty, side(c.before), side(c.after)]);
const shapeHash = shapes => {
  const f = new Float64Array(shapes.length * 11);
  shapes.forEach((s, i) => f.set([s.x, s.y, s.radius, s.opacity, s.flow, s.cos, s.sin, s.left, s.top, s.right, s.bottom], i * 11));
  return hash(new Uint8Array(f.buffer));
};
const errorOf = fn => { try { fn(); return null; } catch (error) { if (!error.messageKey) throw error; return error.messageKey; } };
const asyncErrorOf = async fn => { try { await fn(); return null; } catch (error) { if (!error.messageKey) throw error; return error.messageKey; } };
let seed = 2026;
const rnd = () => { seed = (Math.imul(seed, 1103515245) + 12345) >>> 0; return seed / 4294967296; };
const reseed = n => { seed = n; };

/** Deterministic artwork: soft discs, hard lines, translucent washes, uniform tiles,
 * empty tiles between, negative coordinates. */
let bases = {};
/** A base tile state, recorded once per fixture file under its name. */
const packBase = store => { if (!store) return null; const name = store.baseName; if (!name) return packTiles(store); bases[name] ??= packTiles(store); return name; };
function artwork(kind, n = 7) {
  const store = artworkOf(kind, n); store.baseName = `${kind}-${n}`; return store;
}
function artworkOf(kind, n) {
  reseed(kind.length * 977 + n);
  const store = new TileStore(), x0 = -96, y0 = -80, w = 320, h = 260;
  const put = (x, y, r, g, b, a) => {
    const tile = store.writable(Math.floor(x / TILE), Math.floor(y / TILE)), i = ((y - tile.ty * TILE) * TILE + x - tile.tx * TILE) * 4, p = tile.pixels;
    const ba = p[i + 3] / 255, sa = a / 255, out = sa + ba * (1 - sa);
    if (out <= 0) return;
    p[i] = Math.round((r * sa + p[i] * ba * (1 - sa)) / out); p[i + 1] = Math.round((g * sa + p[i + 1] * ba * (1 - sa)) / out); p[i + 2] = Math.round((b * sa + p[i + 2] * ba * (1 - sa)) / out); p[i + 3] = Math.round(out * 255);
  };
  if (kind === 'empty') return store;
  for (let k = 0; k < n; k++) {
    const cx = x0 + rnd() * w, cy = y0 + rnd() * h, r = 6 + rnd() * 40, color = [rnd() * 255 | 0, rnd() * 255 | 0, rnd() * 255 | 0], hard = kind === 'lines' || rnd() < .5;
    for (let y = Math.floor(cy - r); y <= Math.ceil(cy + r); y++) for (let x = Math.floor(cx - r); x <= Math.ceil(cx + r); x++) {
      const d = Math.hypot(x + .5 - cx, y + .5 - cy); if (d > r) continue;
      const a = hard ? Math.min(1, r - d) : (1 - d / r) * (kind === 'wash' ? .45 : .9);
      if (kind === 'lines' && d < r - 3) continue;
      put(x, y, ...color, Math.round(Math.max(0, a) * 255));
    }
  }
  if (kind !== 'lines') { store.set(3, 1, new Uint8ClampedArray([200, 40, 30, 255])); store.set(-2, 2, new Uint8ClampedArray([10, 120, 220, 128])); }
  for (const tile of [...store]) if (pixels.compactTile(tile.pixels)?.byteLength === 4 && tile.pixels[3] === 0) store.delete(tile.tx, tile.ty);
  return store;
}
const loadTiles = list => { if (typeof list === 'string') list = bases[list]; const store = new TileStore(); for (const [tx, ty, data] of list) store.adopt(tx, ty, new Uint8ClampedArray(inflate(Buffer.from(data, 'base64')))); return store; };
const {inflateZlib: inflate} = await src('zlib-sync');
const copyOf = store => loadTiles(packTiles(store));
const emptyStore = () => Object.assign(new TileStore(), {baseName: 'empty'});

/** Pointer samples along a path: pen (pressure, tilt) or mouse (neither). */
function samples(n, {pen = true, path = 'wave', x = 0, y = 0, scale = 1, tilt = false} = {}) {
  const out = [];
  for (let i = 0; i < n; i++) {
    const t = i / Math.max(1, n - 1);
    let px, py;
    if (path === 'wave') { px = x + t * 300 * scale; py = y + Math.sin(t * 7) * 40 * scale; }
    else if (path === 'loop') { px = x + Math.cos(t * 6.3) * 70 * scale + t * 40; py = y + Math.sin(t * 6.3) * 50 * scale; }
    else if (path === 'zigzag') { px = x + t * 260 * scale; py = y + ((i >> 3) % 2 ? 1 : -1) * ((i % 8) * 6) * scale; }
    else if (path === 'line') { px = x + t * 200 * scale; py = y + t * 30 * scale; }
    else if (path === 'dot') { px = x + (i % 3) * .05; py = y; }
    else { px = x + Math.sin(t * 11) * 90 * scale; py = y + Math.cos(t * 5) * 70 * scale; }
    const pressure = pen ? .05 + .95 * Math.abs(Math.sin(t * 4 + .3)) : null;
    const angle = t * 3, ax = tilt ? Math.cos(angle) : (pen ? 0 : null), ay = tilt ? Math.sin(angle) : (pen ? 0 : null);
    out.push([px, py, pressure, pen ? i * 4.1666 : null, ax, ay]);
  }
  return out;
}
const sampleOf = s => ({x: s[0], y: s[1], pressure: s[2] ?? undefined, time: s[3] ?? undefined, ax: s[4] ?? undefined, ay: s[5] ?? undefined});
const maskInput = m => m ? (m.points ? {x: m.x, y: m.y, width: m.width, height: m.height, points: m.points.map(p => [p.x, p.y])} : {x: m.x, y: m.y, width: m.width, height: m.height, data: b64(deflateZlib(m.mask)), ...(m.max ? {max: m.max} : {})}) : null;
const maskOut = m => m ? (m.points ? {x: m.x, y: m.y, width: m.width, height: m.height, points: m.points.length} : {x: m.x, y: m.y, width: m.width, height: m.height, hash: hash(m.mask)}) : null;

const PRESETS = (await src('drawing-view')).BRUSH_PRESETS;
const preset = (id, extra = {}) => { const {id: _, name, description, ...settings} = PRESETS.find(p => p.id === id); return {...settings, color: [200, 30, 60], ...extra}; };

// ------------------------------------------------------------------ strokes

/** One stroke through the stroke worker's protocol, as the drawing controller drives
 * it (dabs from the brush model, batches every `batch` samples, seeds from dab bounds,
 * the provisional tail resent when it changes), and through `DrawingStroke.sample()`. */
function strokeCase(name, settings, input, {base = null, mask = null, batch = 7, mirror = false} = {}) {
  const engineSettings = {...settings, mask: mask ?? undefined};
  const brush = normalizeBrush(settings), paths = [new BrushStroke(brush, s => pending.push(s))];
  if (mirror) paths.push(new BrushStroke(brush, s => pending.push(s), {seed: brush.seed + 7919}));
  const engine = new StrokeEngine(), store = base ?? new TileStore(), sent = new Set(), batches = [];
  let pending = [], seq = 0, sentTail = null;
  const allShapes = [];
  engine.begin(1, engineSettings);
  const tail = () => brush.taperOut > 0 ? (paths.length === 1 ? paths[0].tailShapes() : paths.flatMap(p => p.tailShapes())) : undefined;
  let tailVersion = -1;
  const flush = () => {
    const version = paths.reduce((n, p) => n + p.tailVersion, 0), t = tail();
    const sendTail = t && version !== tailVersion ? t : undefined;
    if (sendTail) tailVersion = version;
    const dabs = pending; pending = []; allShapes.push(...dabs);
    const tiles = [];
    for (const d of sendTail ? [...dabs, ...sendTail] : dabs)
      for (let ty = Math.floor(d.top / TILE); ty * TILE < d.bottom; ty++) for (let tx = Math.floor(d.left / TILE); tx * TILE < d.right; tx++) {
        const key = `${tx},${ty}`; if (sent.has(key)) continue; sent.add(key);
        const tile = store.get(tx, ty); tiles.push({tx, ty, pixels: tile ? tile.pixels.slice().buffer : null});
      }
    const seeded = tiles.map(t => [t.tx, t.ty, t.pixels ? 1 : 0]);
    const result = engine.dabs(1, ++seq, dabs, tiles, sendTail);
    batches.push({shapes: dabs.length, hash: shapeHash(dabs), tail: sendTail ? shapeHash(sendTail) : null, tiles: seeded,
      dirty: result.tiles.map(t => [t.tx, t.ty, hash(new Uint8Array(t.pixels))]), rect: result.dirty ? [result.dirty.x, result.dirty.y, result.dirty.width, result.dirty.height] : null});
  };
  const axis = 120;
  input.forEach((s, i) => {
    paths[0].sample(sampleOf(s));
    if (mirror) paths[1].sample(sampleOf([2 * axis - s[0], ...s.slice(1)]));
    if (i % batch === batch - 1) flush();
  });
  for (const p of paths) p.finish();
  flush();
  const changes = changesOf(engine.end(1));
  // The same samples through DrawingStroke.sample(), with its live tail.
  let live = null;
  if (!mirror) {
    const target = base ? copyOf(base) : new TileStore(), stroke = new DrawingStroke(target, settings, {mask: mask ?? undefined});
    for (const s of input) stroke.sample(sampleOf(s));
    live = {changes: changesOf(stroke.finish()), tiles: tileHashes(target)};
  }
  return {name, settings, samples: input, base: packBase(base), mask: maskInput(mask), batch, mirror,
    expect: {shapes: {count: allShapes.length, hash: shapeHash(allShapes)}, batches, changes, live}};
}

function strokes() {
  const cases = [];
  const ink = preset('ink');
  for (const id of PRESETS.map(p => p.id)) {
    const settings = preset(id, {seed: 11});
    cases.push(strokeCase(`${id}-pen`, settings, samples(70, {path: 'wave', x: -40, y: 30})));
    cases.push(strokeCase(`${id}-over-art`, {...settings, seed: 12, color: [20, 160, 90]}, samples(45, {path: 'loop', x: 20, y: 40, tilt: true}), {base: artwork('wash')}));
  }
  cases.push(strokeCase('ink-mouse', ink, samples(60, {pen: false, path: 'zigzag', x: -70, y: -20})));
  cases.push(strokeCase('ink-dot', ink, samples(3, {pen: false, path: 'dot', x: 5.3, y: 7.9})));
  cases.push(strokeCase('brushpen-tap', preset('brushpen'), samples(1, {path: 'dot', x: 30, y: 30})));
  cases.push(strokeCase('eraser', {...preset('soft'), erase: true, size: 40}, samples(50, {path: 'curl', x: 60, y: 40}), {base: artwork('blobs')}));
  cases.push(strokeCase('eraser-taper', {...ink, erase: true, size: 22, taperOut: 60}, samples(50, {path: 'wave', x: -60, y: 10}), {base: artwork('blobs')}));
  for (const blend of ['behind', 'multiply', 'screen', 'overlay', 'color'])
    cases.push(strokeCase(`blend-${blend}`, {...preset('marker'), blend, opacity: .8, color: [40, 200, 250]}, samples(40, {path: 'wave', x: -80, y: 0}), {base: artwork('blobs')}));
  cases.push(strokeCase('alpha-lock', {...preset('soft'), alphaLock: true, color: [0, 0, 255]}, samples(40, {path: 'curl', x: 40, y: 30}), {base: artwork('lines')}));
  cases.push(strokeCase('alpha-lock-color', {...preset('marker'), blend: 'color', alphaLock: true, color: [250, 220, 0]}, samples(40, {path: 'wave', x: -60, y: 30}), {base: artwork('blobs')}));
  const lasso = lassoMask(ellipsePoints({x: -20, y: -30}, {x: 150, y: 110}, 64));
  cases.push(strokeCase('selection-mask', {...ink, size: 30}, samples(50, {path: 'wave', x: -60, y: 20}), {mask: lasso}));
  cases.push(strokeCase('selection-points', {...preset('soft'), size: 30}, samples(50, {path: 'wave', x: -60, y: 20}), {mask: {x: -40, y: -40, width: 200, height: 160, points: [{x: -40, y: -40}, {x: 160, y: -10}, {x: 120, y: 120}, {x: -30, y: 90}]}}));
  cases.push(strokeCase('selection-soft-mask', {...ink, size: 26}, samples(40, {path: 'line', x: -30, y: 10}), {mask: {...rectMask({x: -10, y: -10, width: 150, height: 90}), mask: Uint8Array.from({length: 150 * 90}, (_, i) => i % 7), max: 6}}));
  cases.push(strokeCase('smudge-selection', {...preset('blender'), size: 30}, samples(40, {path: 'wave', x: -60, y: 20}), {base: artwork('blobs'), mask: lasso}));
  cases.push(strokeCase('pressure-curve-soft', {...ink, size: 18, pressureCurve: [.1, .5, .4, .95]}, samples(60, {path: 'wave', x: -50, y: 0})));
  cases.push(strokeCase('pressure-curve-firm', {...preset('pencil'), size: 6, pressureCurve: [.6, .05, .9, .5]}, samples(60, {path: 'curl', x: 30, y: 30})));
  cases.push(strokeCase('pressure-size-curve', {...ink, size: 20, sizeCurve: 1.7, opacityCurve: .6, pressureOpacity: true}, samples(60, {path: 'wave', x: -50, y: 0})));
  cases.push(strokeCase('string-stabilizer', {...ink, stabilization: .5, stabilizerMode: 'string', pressureCurve: [.2, .4, .6, .9]}, samples(90, {path: 'curl', x: 20, y: 10})));
  cases.push(strokeCase('average-stabilizer', {...ink, stabilization: .8, pixelsPerUnit: 2}, samples(90, {path: 'zigzag', x: -80, y: 0})));
  cases.push(strokeCase('tilt-nib', {...preset('calligraphy'), angleMode: 'tilt', roundness: .3}, samples(60, {path: 'curl', x: 10, y: 0, tilt: true})));
  cases.push(strokeCase('direction-chalk', {...preset('chalk'), angleMode: 'direction', grainScale: 2.5}, samples(60, {path: 'loop', x: 0, y: 0})));
  cases.push(strokeCase('jitter-scatter', {...preset('sponge'), scatter: 2, sizeJitter: .7, opacityJitter: .6, angleJitter: 1, texture: .4, grain: 'canvas'}, samples(40, {path: 'wave', x: -60, y: 0})));
  cases.push(strokeCase('noise-grain', {...preset('soft'), texture: .7, grain: 'noise', size: 24}, samples(40, {path: 'wave', x: -60, y: 0})));
  cases.push(strokeCase('hairline', {...ink, size: .5, spacingRatio: .5}, samples(50, {path: 'curl', x: 0, y: 0})));
  cases.push(strokeCase('large-soft', {...preset('soft'), size: 180, flow: .3}, samples(30, {path: 'line', x: -100, y: -40}), {batch: 3}));
  cases.push(strokeCase('taper-both', {...ink, size: 16, taperIn: 30, taperOut: 70, pressureSize: false}, samples(80, {path: 'wave', x: -60, y: 0}), {batch: 5}));
  cases.push(strokeCase('taper-mirror', {...preset('brushpen'), seed: 5}, samples(60, {path: 'curl', x: 40, y: 20}), {mirror: true}));
  cases.push(strokeCase('mirror-chalk', {...preset('charcoal'), seed: 6}, samples(50, {path: 'wave', x: 0, y: 20}), {mirror: true, base: artwork('wash')}));
  cases.push(strokeCase('far-coordinates', {...ink, size: 12}, samples(30, {path: 'wave', x: 64 * 100000 - 50, y: -64 * 3000 - 20})));
  cases.push(strokeCase('oil-over-art', {...preset('oil'), size: 40}, samples(50, {path: 'curl', x: 40, y: 30}), {base: artwork('blobs')}));
  return cases;
}

// ------------------------------------------------------------------ brush model

function brushModel() {
  const normalize = [{}, {size: 99999, opacity: 2, tip: 'nope', grain: 'canvas', blend: 'smudge', taperOut: 9, texture: .5, angle: -30, seed: -1},
    {erase: 1, blend: 'smudge', tip: 'chalk', pressureCurve: [2, -1, .3, .4], stabilizerMode: 'string', angleMode: 'tilt', color: [300, -4, 12.5]},
    {pressureSize: 0, pressureOpacity: 'yes', alphaLock: true, seed: 4294967297.7, pixelsPerUnit: 0}].map(settings => ({settings, brush: normalizeBrush(settings)}));
  const refused = [{size: null}, {color: [1, 2]}, {opacity: 'x'}, {color: [1, 2, NaN]}].map(settings => ({settings: JSON.parse(JSON.stringify(settings)), error: errorOf(() => normalizeBrush(settings))}));
  const curves = [[.1, .5, .4, .95], [.6, .05, .9, .5], [0, 1, 0, 1]].map(curve => { const map = pressureMap(curve); return {curve, values: Array.from({length: 41}, (_, i) => map(i / 40 - .1))}; });
  const jitter = Array.from({length: 40}, (_, i) => [i * 7919 >>> 0, i * 3, i % 5, jitterAt(i * 7919 >>> 0, i * 3, i % 5)]);
  const shapes = [];
  for (const settings of [preset('calligraphy'), preset('sponge'), {...preset('charcoal'), angleMode: 'tilt'}, {...preset('ink'), taperIn: 10}])
    for (const p of [{x: 3.25, y: -7.5, pressure: .4, index: 3, dir: 1.2, ax: .6, ay: -.8, taper: .5}, {x: -100.1, y: 64.5, pressure: 1, index: 0, dir: -2, ax: 0, ay: 0, taper: 1}])
      shapes.push({settings, point: p, shape: dabShape(normalizeBrush(settings), p)});
  for (const name of ['chalk', 'bristle', 'sponge', 'paper', 'canvas']) ensureTexture(name);
  const path = [];
  for (const settings of [{size: 10, stabilization: .4, stabilizerMode: 'string'}, {size: 3, stabilization: .9, pixelsPerUnit: 3}]) {
    const dabs = [], p = new StrokePath(settings, d => dabs.push(d));
    for (const s of samples(50, {path: 'curl', x: 0, y: 0})) p.sample(sampleOf(s));
    p.finish();
    path.push({settings, samples: samples(50, {path: 'curl', x: 0, y: 0}), count: dabs.length, hash: hash(new Uint8Array(new Float64Array(dabs.flatMap(d => [d.x, d.y, d.pressure, d.time, d.ax, d.ay, d.dir, d.dist])).buffer))});
  }
  return {normalize, refused, curves, jitter, shapes, atlas: {bytes: ATLAS_BYTES, hash: hash(brushAtlas())}, path};
}

// ------------------------------------------------------------------ fills and masks

function fills() {
  const cases = [];
  const art = artwork('lines', 9), blobs = artwork('blobs');
  const bounds = {x: -160, y: -140, width: 480, height: 400};
  const fill = (name, base, options, extra = {}) => {
    const store = copyOf(base), {clip, source, ...rest} = options;
    let changes = null;
    const error = errorOf(() => { changes = floodTiles(store, {...rest, bounds: options.bounds ?? bounds, clip: clip ?? null, source}); });
    cases.push({name, base: packBase(base), options: {...rest, bounds: options.bounds ?? bounds}, clip: maskInput(clip), source: source ? b64(deflateZlib(source)) : null, ...extra,
      expect: error ? {error} : {changes: changesOf(changes), tiles: tileHashes(store)}});
  };
  fill('enclosed', art, {x: 10, y: 20, color: [255, 0, 0, 255]});
  fill('outside', art, {x: -150, y: -130, color: [0, 128, 255, 255]});
  fill('tolerance-0', blobs, {x: 30, y: 40, color: [0, 0, 0, 255], tolerance: 0});
  fill('tolerance-80', blobs, {x: 30, y: 40, color: [10, 250, 10, 200], tolerance: 80});
  fill('gap-2', art, {x: 10, y: 20, color: [255, 200, 0, 255], gap: 2});
  fill('gap-8-expand-4', art, {x: 10, y: 20, color: [90, 0, 200, 255], gap: 8, expansion: 4});
  fill('expand-16', blobs, {x: -120, y: -100, color: [0, 0, 0, 255], expansion: 16});
  fill('transparent-color', blobs, {x: 30, y: 40, color: [0, 0, 0, 0], tolerance: 40});
  fill('blank', emptyStore(), {x: 0, y: 0, color: [12, 34, 56, 255], bounds: {x: -70, y: -50, width: 300, height: 200}});
  fill('blank-transparent', emptyStore(), {x: 0, y: 0, color: [0, 0, 0, 0], bounds: {x: -70, y: -50, width: 300, height: 200}});
  fill('clip', art, {x: 10, y: 20, color: [255, 0, 255, 255], clip: lassoMask(rectPoints({x: -50, y: -40}, {x: 90, y: 70}))});
  fill('clip-points', art, {x: 10, y: 20, color: [255, 0, 255, 255], gap: 1, clip: {x: -50, y: -40, width: 160, height: 120, points: [{x: -50, y: -40}, {x: 110, y: -20}, {x: 60, y: 80}]}});
  const source = new Uint8ClampedArray(bounds.width * bounds.height * 4);
  reseed(77); for (let i = 0; i < source.length; i += 4) { const x = (i / 4) % bounds.width, y = Math.floor(i / 4 / bounds.width); if ((x - 200) ** 2 + (y - 180) ** 2 < 90 ** 2 && ((x + y) % 23 > 2)) { source[i] = 40; source[i + 1] = 40; source[i + 2] = 40; source[i + 3] = 255; } }
  fill('sample-all', blobs, {x: 30, y: 40, color: [255, 128, 0, 255], tolerance: 10, gap: 1, source});
  fill('budget', art, {x: 10, y: 20, color: [255, 0, 0, 255], maxBytes: 300000});
  fill('budget-blank', emptyStore(), {x: 0, y: 0, color: [1, 2, 3, 255], maxBytes: 100000, bounds: {x: -700, y: -500, width: 3000, height: 2000}});
  fill('bad-bounds', art, {x: 0, y: 0, color: [1, 2, 3, 255], bounds: {x: 0.5, y: 0, width: 10, height: 10}});
  return cases;
}

function masks() {
  const art = artwork('lines', 9), blobs = artwork('blobs');
  const wand = [];
  const bounds = {x: -160, y: -140, width: 480, height: 400};
  for (const [name, base, options] of [['wand', art, {x: 10, y: 20, tolerance: 16}], ['wand-gap', art, {x: 10, y: 20, tolerance: 16, gap: 3}], ['wand-gap-dense', blobs, {x: 30, y: 40, tolerance: 0, gap: 2}],
    ['wand-expand', blobs, {x: -120, y: -100, tolerance: 30, expansion: 3}], ['wand-clip', art, {x: 10, y: 20, clip: lassoMask(rectPoints({x: -50, y: -40}, {x: 90, y: 70}))}], ['wand-outside', art, {x: 9999, y: 0}]]) {
    const store = copyOf(base), {clip, ...rest} = options;
    const mask = floodDense(store, {...rest, bounds, clip: clip ?? null, select: true});
    wand.push({name, base: packBase(base), options: {...rest, bounds}, clip: maskInput(clip), expect: {mask: mask === undefined ? 'none' : maskOut(mask)}});
  }
  const dense = [];
  for (const [name, base, options] of [['dense-fill', art, {x: 10, y: 20, color: [200, 10, 10, 255], gap: 1, expansion: 2}], ['dense-fill-uniform', emptyStore(), {x: 0, y: 0, color: [9, 8, 7, 255]}]]) {
    const store = copyOf(base);
    floodDense(store, {...options, bounds});
    dense.push({name, base: packBase(base), options: {...options, bounds}, expect: {tiles: tileHashes(store)}});
  }
  const lasso = [];
  for (const points of [ellipsePoints({x: -20.5, y: -30}, {x: 150, y: 110.25}, 37), rectPoints({x: 3, y: 4}, {x: 90.5, y: 61}), [{x: 0, y: 0}, {x: 100, y: 0}, {x: 0, y: 100}, {x: 100, y: 100}], [{x: 1, y: 1}, {x: 2, y: 2}]])
    lasso.push({points: points.map(p => [p.x, p.y]), expect: maskOut(lassoMask(points))});
  const a = lassoMask(ellipsePoints({x: 0, y: 0}, {x: 80, y: 60})), b = lassoMask(rectPoints({x: 30, y: 20}, {x: 130, y: 90}));
  const combine = ['replace', 'add', 'subtract', 'intersect'].map(mode => ({mode, expect: maskOut(combineMasks(a, b, mode))}));
  const ops = {a: maskInput(a), b: maskInput(b), combine, invert: maskOut(invertMask(a, {x: -20, y: -10, width: 140, height: 100})), invertNone: maskOut(invertMask(null, {x: 0, y: 0, width: 5, height: 4})),
    crop: maskOut(cropMask({x: 5, y: 6, width: 20, height: 10, mask: Uint8Array.from({length: 200}, (_, i) => +(i % 20 > 6 && i % 20 < 12 && i > 40 && i < 160))})),
    edges: hash(new Uint8Array(new Float64Array(maskEdges(a)).buffer)), edgesCount: maskEdges(a).length};
  const fillMask = [];
  for (const [name, mask, color] of [['fill-lasso', a, [255, 0, 0, 255]], ['fill-translucent', b, [0, 0, 255, 100]], ['fill-points', {x: -40, y: -40, width: 160, height: 120, points: [{x: -40, y: -40}, {x: 110, y: -20}, {x: 60, y: 80}]}, [0, 255, 0, 180]]]) {
    const store = copyOf(blobs); fillMaskTiles(store, {mask, color});
    fillMask.push({name, base: packBase(blobs), mask: maskInput(mask), color, expect: {tiles: tileHashes(store)}});
  }
  return {wand, dense, lasso, ops, fillMask};
}

// ------------------------------------------------------------------ filters

function filters() {
  const cases = [], blobs = artwork('blobs'), wash = artwork('wash');
  const lasso = lassoMask(ellipsePoints({x: -40, y: -30}, {x: 150, y: 120}));
  const run = (name, base, options) => {
    const store = copyOf(base), {mask, ...rest} = options, blur = rest.kind === 'blur', before = new Map();
    if (blur) for (const [tx, ty] of TileStore.tilesInRect(rest.bounds.x, rest.bounds.y, rest.bounds.width, rest.bounds.height)) before.set(`${tx},${ty}`, store.get(tx, ty)?.pixels.slice() ?? null);
    const error = errorOf(() => filterTiles(store, {...rest, mask: mask ?? null, before}));
    if (!error && blur) store.prune();
    cases.push({name, base: packBase(base), options: rest, mask: maskInput(mask), expect: error ? {error} : {changes: changesOf(tileChanges(store, before)), tiles: tileHashes(store)}});
  };
  const bounds = {x: -120, y: -100, width: 380, height: 300};
  run('blur', blobs, {kind: 'blur', bounds, radius: 6});
  run('blur-mask', wash, {kind: 'blur', bounds, radius: 20, mask: lasso});
  run('blur-small', blobs, {kind: 'blur', bounds: {x: 0, y: 0, width: 70, height: 50}, radius: 1});
  run('hsl', blobs, {kind: 'hsl', bounds, hue: 140, saturation: 40, lightness: -20});
  run('hsl-mask', wash, {kind: 'hsl', bounds, hue: -60, saturation: -100, lightness: 30, mask: lasso});
  run('brightness', blobs, {kind: 'brightness', bounds, brightness: 25, contrast: 60});
  run('brightness-mask', blobs, {kind: 'brightness', bounds, brightness: -40, contrast: -30, mask: lasso});
  run('invert', wash, {kind: 'invert', bounds});
  run('invert-mask', wash, {kind: 'invert', bounds: {x: 0, y: 0, width: 100, height: 100}, mask: {x: -40, y: -40, width: 160, height: 120, points: [{x: -40, y: -40}, {x: 110, y: -20}, {x: 60, y: 80}]}});
  run('unknown', blobs, {kind: 'sharpen', bounds});
  run('bad-bounds', blobs, {kind: 'invert', bounds: {x: 0, y: 0, width: 0, height: 10}});
  return cases;
}

// ------------------------------------------------------------------ selections and transforms

const capturedOut = result => ({bounds: result.bounds ? [result.bounds.x, result.bounds.y, result.bounds.width, result.bounds.height] : null,
  tiles: result.tiles.map(t => [t.tx, t.ty, hash(t.pixels), t.alphaBounds ? [t.alphaBounds.x, t.alphaBounds.y, t.alphaBounds.width, t.alphaBounds.height] : null, t.full ? 1 : 0,
    t.remaining ? hash(t.remaining) : null, t.remainingBounds ? [t.remainingBounds.x, t.remainingBounds.y, t.remainingBounds.width, t.remainingBounds.height] : null])});

function selections() {
  const blobs = artwork('blobs'), cases = [];
  for (const [name, points, maxBytes] of [['ellipse', ellipsePoints({x: -60, y: -50}, {x: 140, y: 120}, 96), 384 * 1024 * 1024], ['lasso', [{x: -30, y: -70}, {x: 220, y: 10}, {x: 40, y: 30}, {x: 180, y: 170}, {x: -90, y: 100}], 384 * 1024 * 1024],
    ['whole', rectPoints({x: -200, y: -200}, {x: 400, y: 400}), 384 * 1024 * 1024], ['budget', ellipsePoints({x: -60, y: -50}, {x: 140, y: 120}, 96), 200000]]) {
    let result = null; const error = errorOf(() => { result = captureSelection(copyOf(blobs), points, {maxBytes}); });
    cases.push({name, base: packBase(blobs), points: points.map(p => [p.x, p.y]), maxBytes, expect: error ? {error} : capturedOut(result)});
  }
  const mask = lassoMask(ellipsePoints({x: -60, y: -50}, {x: 140, y: 120}, 50));
  const result = captureMaskSelection(copyOf(blobs), mask, {});
  cases.push({name: 'mask', base: packBase(blobs), mask: maskInput(mask), maxBytes: 384 * 1024 * 1024, expect: capturedOut(result)});
  return cases;
}

function transforms() {
  const blobs = artwork('blobs'), wash = artwork('wash'), cases = [];
  const selection = (name, s, {remove = false, filter = 'smooth', points = ellipsePoints({x: -50, y: -40}, {x: 120, y: 100}, 64), maxBytes = 384 * 1024 * 1024} = {}) => {
    const store = copyOf(blobs), captured = captureSelection(store, points, {});
    const source = new TileStore();
    for (const t of captured.tiles) source.tiles.set(`${t.tx},${t.ty}`, {tx: t.tx, ty: t.ty, pixels: t.full ? store.get(t.tx, t.ty).pixels.slice() : t.pixels, alphaBounds: t.alphaBounds, frozen: false});
    const matrix = selectionMatrix(s);
    let changes = null; const error = errorOf(() => { changes = transformSelectedPixels(store, {source, matrix, remove, filter, maxBytes}); });
    cases.push({name, kind: 'selection', base: packBase(blobs), source: packTiles(source), matrix, remove, filter, maxBytes, pose: s,
      expect: error ? {error} : {changes: changesOf(changes), tiles: tileHashes(store)}});
  };
  const pose = (angle, scale, dx, dy, fx = 1, fy = 1) => ({angle, scale, dx, dy, cx: 35, cy: 30, fx, fy});
  selection('move-integer', pose(0, 1, 64, -128));
  selection('move-fraction', pose(0, 1, 13.25, -7.5));
  selection('rotate-scale', pose(33, 1.4, 10, 5));
  selection('rotate-nearest', pose(-71, .8, 0, 0), {filter: 'nearest'});
  selection('right-angle', pose(90, 1, 3, 0));
  selection('mirror', pose(0, 1, 0, 0, -1, 1));
  selection('shrink', pose(15, .2, -30, 40));
  selection('tiny', pose(5, .004, 0, 0));
  selection('remove', pose(0, 1, 0, 0), {remove: true});
  selection('budget', pose(10, 1, 0, 0), {maxBytes: 600000});
  const resample = (name, base, ratio, filter = 'smooth', maxBytes = 384 * 1024 * 1024) => {
    let out = null; const error = errorOf(() => { out = transformTiles(copyOf(base), [ratio, 0, 0, ratio, 0, 0], {filter, maxBytes}); });
    cases.push({name, kind: 'resample', base: packBase(base), ratio, filter, maxBytes,
      expect: error ? {error} : {tiles: tileHashes(out), bounds: out.bounds ? [out.bounds.x, out.bounds.y, out.bounds.width, out.bounds.height] : null,
        alpha: [...out].map(t => t.alphaBounds ? [t.alphaBounds.x, t.alphaBounds.y, t.alphaBounds.width, t.alphaBounds.height] : t.alphaBounds === null ? 'empty' : 'unknown')}});
  };
  resample('half', blobs, .5); resample('double', wash, 2); resample('quarter', blobs, .25); resample('double-nearest', blobs, 2, 'nearest'); resample('tenth', wash, .1); resample('budget', blobs, 4, 'smooth', 2000000);
  const target = artwork('lines', 5);
  const general = (name, matrix, filter = 'smooth') => {
    const t = copyOf(target), before = new Map();
    let error = errorOf(() => transformTiles(copyOf(blobs), matrix, {target: t, before, filter}));
    cases.push({name, kind: 'general', base: packBase(blobs), target: packBase(target), matrix, filter, expect: error ? {error} : {changes: changesOf(tileChanges(t, before)), tiles: tileHashes(t)}});
  };
  general('affine', [1.2, .3, -.2, .9, 17.5, -33.25]);
  general('skew-nearest', [1, .5, 0, 1, -4, 9], 'nearest');
  general('singular', [1, 2, 2, 4, 0, 0]);
  general('far', [1, 0, 0, 1, 64 * 2 ** 31, 0]);
  const coverage = [];
  for (const [matrix, filter] of [[[1.2, .3, -.2, .9, 17.5, -33.25], 'smooth'], [[.1, 0, 0, .1, 3, 3], 'smooth'], [[2, 0, 0, 2, 0, 0], 'nearest']]) {
    coverage.push({matrix, filter, tiles: transformTileCoverage(copyOf(blobs), matrix, filter).size, keys: [...transformTileCoverage(copyOf(blobs), matrix, filter).keys()].join(';'),
      indexed: [...indexedTileCoverage([{tx: 0, ty: 0}, {tx: -3, ty: 2}], matrix, filter).keys()].join(';'),
      support: transformSupportBounds({x: -10.5, y: 3, width: 70, height: 33}, matrix, filter)});
  }
  return {cases, base: packBase(blobs), coverage};
}

// ------------------------------------------------------------------ levels and codecs

function levels() {
  const out = [];
  for (const [name, base] of [['blobs', artwork('blobs')], ['wash', artwork('wash')], ['lines', artwork('lines', 12)]]) {
    const entry = {store: copyOf(base)}, chain = [];
    for (let k = 1; k <= 5; k++) chain.push(tileHashes(levelStore(entry, k)));
    out.push({name, base: packBase(base), levels: chain});
  }
  return out;
}

async function codecs() {
  const blobs = artwork('blobs'), wash = artwork('wash');
  const tiles = [...blobs, ...wash].slice(0, 30);
  const pngs = tiles.map(t => ({tx: t.tx, ty: t.ty, data: b64(deflateZlib(t.pixels)), png: hash(encodeRGBAPNG(t.pixels, TILE, TILE)), tile: hash(encodeTile(t.pixels))}));
  // Deflate on inputs other encoders never see: runs, noise, short, empty.
  reseed(5);
  const streams = [new Uint8Array(0), new Uint8Array([7]), Uint8Array.from({length: 70000}, () => rnd() * 4 | 0), Uint8Array.from({length: 40000}, (_, i) => (i >> 9) & 255), Uint8Array.from({length: 3000}, () => rnd() * 256 | 0)]
    .map(bytes => ({data: b64(bytes), deflate: hash(deflateZlib(bytes)), length: deflateZlib(bytes).length}));
  // PNGs other encoders write: every filter type, RGB.
  const filtered = [];
  for (const [filter, channels] of [[0, 4], [1, 4], [2, 3], [3, 4], [4, 4], [4, 3]]) {
    const w = 37, h = 23, stride = w * channels, raw = new Uint8Array((stride + 1) * h), image = new Uint8Array(stride * h);
    reseed(filter * 10 + channels);
    for (let i = 0; i < image.length; i++) image[i] = (i * 7 + (rnd() * 30 | 0)) & 255;
    for (let y = 0; y < h; y++) {
      raw[y * (stride + 1)] = filter;
      for (let x = 0; x < stride; x++) {
        const v = image[y * stride + x], a = x >= channels ? image[y * stride + x - channels] : 0, b = y ? image[(y - 1) * stride + x] : 0, c = y && x >= channels ? image[(y - 1) * stride + x - channels] : 0;
        const p = a + b - c, pa = Math.abs(p - a), pb = Math.abs(p - b), pc = Math.abs(p - c), paeth = pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
        raw[y * (stride + 1) + 1 + x] = (v - [0, a, b, Math.floor((a + b) / 2), paeth][filter]) & 255;
      }
    }
    const zlib = (await import('node:zlib')).deflateSync(raw);
    const {crc32} = await src('sticknodes/decoder/archive');
    const chunk = (type, data) => { const o = new Uint8Array(data.length + 12), v = new DataView(o.buffer); v.setUint32(0, data.length); for (let i = 0; i < 4; i++) o[4 + i] = type.charCodeAt(i); o.set(data, 8); v.setUint32(o.length - 4, crc32(o.subarray(4, -4))); return o; };
    const header = new Uint8Array(13), hv = new DataView(header.buffer); hv.setUint32(0, w); hv.setUint32(4, h); header[8] = 8; header[9] = channels === 4 ? 6 : 2;
    const half = Math.floor(zlib.length / 2);
    const png = Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk('IHDR', header), chunk('tEXt', new TextEncoder().encode('note\0x')), chunk('IDAT', zlib.subarray(0, half)), chunk('IDAT', zlib.subarray(half)), chunk('IEND', new Uint8Array())]);
    const decoded = await decodeDrawingPNGBytes(new Uint8Array(png));
    filtered.push({filter, channels, png: b64(png), expect: {width: decoded.width, height: decoded.height, pixels: hash(decoded.pixels)}});
  }
  const broken = [];
  const good = encodeRGBAPNG(blobs.get(0, 0)?.pixels ?? [...blobs][0].pixels, TILE, TILE);
  for (const [name, mutate, expected] of [['signature', b => { b[0] = 0; }], ['crc', b => { b[20] ^= 1; }], ['truncated', b => b.subarray(0, b.length - 20)], ['dimensions', b => b, {width: 32, height: 64}],
    ['corrupt', b => { const i = 8 + 25 + 8 + 4; b[i] ^= 0x55; const v = new DataView(b.buffer, b.byteOffset); const len = v.getUint32(33); return b; }]]) {
    const bytes = new Uint8Array(good); const mutated = mutate(bytes) ?? bytes;
    broken.push({name, png: b64(mutated), expected: expected ?? null, error: await asyncErrorOf(() => decodeDrawingPNGBytes(mutated, expected ?? {}))});
  }
  // Tile blobs: assembly, decoding with shards, refusals.
  const encoded = [...blobs].map(t => ({tx: t.tx, ty: t.ty, png: encodeTile(t.pixels)}));
  const blob = assembleTileBlobBytes(encoded);
  const url = TILE_BLOB_PREFIX + bytesToBase64(blob);
  const decoded = await decodeTileBlob(url), shard = await decodeTileBlob(url, {shard: [1, 3]});
  const blobCase = {tiles: encoded.map(t => [t.tx, t.ty, b64(t.png)]), blob: hash(blob), blobLength: blob.length,
    decoded: decoded.map(t => [t.tx, t.ty, hash(t.pixels), hash(t.png)]), shard: shard.map(t => [t.tx, t.ty]),
    duplicate: errorOf(() => assembleTileBlobBytes([...encoded, encoded[0]])),
    trailing: await asyncErrorOf(() => decodeTileBlob(TILE_BLOB_PREFIX + bytesToBase64(new Uint8Array([...blob, 0])))),
    budget: await asyncErrorOf(() => decodeTileBlob(url, {maxBytes: 1000}))};
  // The pixel worker's encoded-tile cache over a sequence of publications.
  const cache = new TileEncodeCache({maxBytes: 60000}), steps = [];
  const pub = async (celId, keep, list, partial = false, pngs = null) => {
    if (pngs) cache.adopt(celId, pngs);
    const result = await cache.encode(celId, keep, list.map(t => t.png ? {tx: t.tx, ty: t.ty, png: t.png} : {tx: t.tx, ty: t.ty, pixels: t.pixels}), {partial});
    steps.push({celId, keep, partial, adopt: pngs ? pngs.map(p => [p.tx, p.ty, b64(p.png)]) : null, tiles: list.map(t => t.png ? [t.tx, t.ty, null, b64(t.png)] : [t.tx, t.ty, b64(deflateZlib(t.pixels)), null]),
      expect: {encoded: result.encoded, tiles: result.tiles.map(t => [t.tx, t.ty, hash(t.png)]), missing: result.missing ?? null, blob: result.bytes ? hash(result.bytes) : null, reused: result.reused ?? null, bytes: cache.bytes}});
  };
  const list = [...blobs];
  await pub('a', list.map(t => `${t.tx},${t.ty}`), list.slice(0, 4), true);
  await pub('a', null, list.slice(4), true);
  await pub('a', list.map(t => `${t.tx},${t.ty}`), []);
  await pub('b', [...wash].map(t => `${t.tx},${t.ty}`), [...wash]);
  await pub('a', list.slice(1).map(t => `${t.tx},${t.ty}`), [{...list[1], pixels: list[2].pixels}]);
  await pub('c', ['0,0', '1,0'], [], false, [{tx: 0, ty: 0, png: encodeTile(list[0].pixels)}]);
  await pub('a', list.map(t => `${t.tx},${t.ty}`), [{tx: list[0].tx, ty: list[0].ty, png: encodeTile(list[0].pixels)}]);
  // History chunks.
  const changes = [{tx: 0, ty: 0, before: null, after: list[0].pixels.slice()}, {tx: -5, ty: 7, before: new Uint8ClampedArray([1, 2, 3, 4]), after: null}, {tx: 2 ** 31 - 1, ty: -(2 ** 31), before: list[1].pixels.slice(), after: new Uint8ClampedArray([0, 0, 0, 255])}];
  const chunk = encodeHistoryChunk(changes), round = decodeHistoryChunk(chunk);
  const many = Array.from({length: 140}, (_, i) => ({tx: i, ty: 0, before: list[i % list.length].pixels, after: i % 3 ? new Uint8ClampedArray([i, 0, 0, 255]) : null}));
  const history = {changes: changes.map(c => [c.tx, c.ty, c.before ? b64(deflateZlib(c.before)) : null, c.after ? b64(deflateZlib(c.after)) : null]), chunk: hash(chunk), length: chunk.length,
    round: changesOf(round), chunks: [...historyChunks(many)].map(c => c.length), manyBefore: many.slice(0, 1).length,
    damaged: errorOf(() => decodeHistoryChunk(chunk.subarray(0, chunk.length - 3))), trailing: errorOf(() => { const b = new Uint8Array(chunk.length + 1); b.set(chunk); new DataView(b.buffer).setUint32(8, b.length, true); decodeHistoryChunk(b); }),
    empty: errorOf(() => encodeHistoryChunk([]))};
  // Commit preparation.
  const input = [{tx: 0, ty: 0, pixels: list[0].pixels.slice().buffer}, {tx: 1, ty: 1, pixels: new Uint8ClampedArray([5, 6, 7, 8]).buffer}, {tx: 2, ty: 2, pixels: null}, {tx: 3, ty: 3, pixels: new Uint8ClampedArray([5, 6, 7, 0]).buffer}, {tx: 4, ty: 4, pixels: new Uint8ClampedArray(TILE_BYTES).buffer}];
  const packed = input.map(t => [t.tx, t.ty, t.pixels ? b64(deflateZlib(new Uint8Array(t.pixels))) : null]);
  const prepared = materializeDrawingTiles(input).map(t => [t.tx, t.ty, t.pixels ? hash(new Uint8Array(t.pixels)) : null, t.alphaBounds ? [t.alphaBounds.x, t.alphaBounds.y, t.alphaBounds.width, t.alphaBounds.height] : null]);
  // The pixel clipboard: copy (encode) and paste (decode).
  const clipTiles = [{tx: list[0].tx, ty: list[0].ty, pixels: list[0].pixels.slice().buffer}, {tx: list[1].tx, ty: list[1].ty, png: encodeTile(list[1].pixels).buffer}];
  const copied = await encodeClipboardTiles(clipTiles, {});
  const packet = {version: 2, pixelsPerUnit: 2, world: {x: 1, y: 2}, matrix: [1, 0, 0, 1, 0, 0], bounds: {x: Math.min(list[0].tx, list[1].tx) * 64, y: Math.min(list[0].ty, list[1].ty) * 64, width: (Math.abs(list[0].tx - list[1].tx) + 1) * 64, height: (Math.abs(list[0].ty - list[1].ty) + 1) * 64}, tiles: copied};
  const pasted = await decodePixelClipboard(packet, {});
  const v1 = {pixelsPerUnit: 1, world: {x: 0, y: 0}, width: 50, height: 40, dataUrl: 'data:image/png;base64,' + bytesToBase64(encodeRGBAPNG(new Uint8ClampedArray(50 * 40 * 4).map((_, i) => i % 4 === 3 ? (i % 7 ? 255 : 0) : (i * 3) & 255), 50, 40))};
  const pastedV1 = await decodePixelClipboard(v1, {});
  const clipboard = {copy: clipTiles.map(t => t.png ? [t.tx, t.ty, null, b64(new Uint8Array(t.png))] : [t.tx, t.ty, b64(deflateZlib(new Uint8Array(t.pixels))), null]), copied: copied.map(t => [t.tx, t.ty, hash(new TextEncoder().encode(t.png))]),
    packet, pasted: tileHashes(pasted.store), encodedBytes: pasted.encodedBytes, v1, pastedV1: tileHashes(pastedV1.store),
    refused: await asyncErrorOf(() => decodePixelClipboard({...packet, tiles: [...copied, copied[0]]}, {})), budget: await asyncErrorOf(() => decodePixelClipboard(packet, {maxBytes: 1000}))};
  // Paste: decode then transform into an existing cel, as the worker's paste does.
  const pasteTarget = copyOf(wash), pasteBefore = new Map();
  transformTiles(pasted.store, [.9, .2, -.2, .9, 30.5, -12], {target: pasteTarget, before: pasteBefore, filter: 'smooth', maxBytes: 384 * 1024 * 1024 - pasted.encodedBytes});
  const paste = {target: packBase(wash), matrix: [.9, .2, -.2, .9, 30.5, -12], changes: changesOf(tileChanges(pasteTarget, pasteBefore)), tiles: tileHashes(pasteTarget)};
  // Stroke region.
  const region = [...strokeRegion([{x: 0, y: 0}, {x: 130, y: 40}, {x: 70, y: 200}, {x: -90, y: 120}], 12)];
  return {pngs, streams, filtered, broken, blob: blobCase, cache: steps, history, prepare: {input: packed, prepared}, clipboard, paste, region};
}

// ------------------------------------------------------------------ main

const files = {
  strokes: () => ({cases: strokes()}),
  brush: () => brushModel(),
  fills: () => ({cases: fills()}),
  masks: () => masks(),
  filters: () => ({cases: filters()}),
  selections: () => ({cases: selections()}),
  transforms: () => transforms(),
  levels: () => ({cases: levels()}),
  codecs: () => codecs(),
};
const dump = process.argv.indexOf('--dump');
mkdirSync(out, {recursive: true});
for (const [name, build] of Object.entries(files)) {
  bases = {empty: []};
  const data = await build();
  data.bases = bases;
  if (dump >= 0) { const wanted = process.argv[dump + 1]; for (const c of data.cases ?? []) if (c.name === wanted) console.log(JSON.stringify(c, null, 1)); continue; }
  writeFileSync(path.join(out, `${name}.json`), JSON.stringify(data) + '\n');
}
if (dump < 0) console.log(`drawing fixtures: ${Object.keys(files).length} files in ${path.relative(repo, out)}`);
