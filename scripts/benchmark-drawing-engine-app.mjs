#!/usr/bin/env node
// The drawing raster engine against the JavaScript raster engine in the app (a browser,
// the development server): on a dense 1080p cel at 2x, large soft strokes through the
// stroke worker (pen-up to commit), their undo and redo once the history has moved them to
// the disk tier (the history worker's chunks, the pixel worker's preparation) back to back
// and with publication settled first, a fill, and publication. Engines alternate run by run (`?drawingEngine=off` for JavaScript).
//
//   npm run serve -- --port 47851 &
//   STYK_URL=http://127.0.0.1:47851 node scripts/benchmark-drawing-engine-app.mjs [--rounds 3]
import {chromium} from '../tests/support/playwright.mjs';
import {mkdir, writeFile} from 'node:fs/promises';
import path from 'node:path';

const url = process.env.STYK_URL || 'http://127.0.0.1:47839';
const args = process.argv.slice(2), rounds = Number(args[args.indexOf('--rounds') + 1] || 3);
const out = path.resolve('artifacts/drawing-engine/app-bench.json');
const browser = await chromium.launch({channel: 'chrome', headless: true});

async function run(engine) {
  const page = await browser.newPage({viewport: {width: 1440, height: 1000}}), errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.goto(engine === 'javascript' ? `${url}/?drawingEngine=off` : url);
  await page.waitForFunction(() => window.styk?.ready);
  await page.locator('[data-workspace-mode="draw"]').click();
  const result = await page.evaluate(async () => {
    const {emptyDocument} = await import('/src/demo.mjs'), e = styk.editor, d = styk.drawing, report = emptyDocument();
    report.canvas_width = 1920; report.canvas_height = 1080;
    e.load(report); d.bind(); e.createDrawingLayer(); styk.state.settings.autosaveSeconds = 0; d.gpuBrushDisabled = true;
    // A textured opaque cel covering the stage: every tile unique (as tests/drawing-latency.browser.mjs builds it).
    const target = e.prepareDrawingCel({pixelsPerUnit: 2, world: {x: 0, y: 0}}); await d.owner.surface(target.figure);
    const tiles = [];
    for (let ty = 0; ty < 34; ty++) for (let tx = 0; tx < 60; tx++) {
      const after = new Uint8ClampedArray(64 * 64 * 4);
      for (let i = 0; i < after.length; i += 4) { const n = (i >> 2) * 2654435761 + tx * 97 + ty * 131 >>> 0; after[i] = 40 + (tx * 5 & 63) + (n & 7); after[i + 1] = 120 + (ty * 3 & 63) + (n >> 8 & 7); after[i + 2] = 60 + (n >> 16 & 7); after[i + 3] = 255; }
      tiles.push({tx, ty, before: null, after});
    }
    e.commitDrawingPixels(target, tiles, 'Fixture');
    await d.owner.flush();
    styk.fit(); d.onRender();
    const idle = () => new Promise(resolve => { const poll = () => (!d.gesture && !d.loading && !d.operation ? resolve() : setTimeout(poll, 5)); poll(); });
    const box = document.querySelector('#viewport').getBoundingClientRect(), cx = box.x + box.width / 2, cy = box.y + box.height / 2;
    document.querySelector('[data-draw-preset="soft"]').click();
    Object.assign(d.settings, {size: 300, color: '#ff00ff', flow: .5});
    const measure = {strokes: [], undo: [], redo: [], fill: null, publish: null};
    for (let k = 0; k < 3; k++) {
      const points = Array.from({length: 120}, (_, i) => { const t = i / 119; return {pointerId: 5, pointerType: 'pen', button: 0, buttons: 1, clientX: cx - 420 + t * 840, clientY: cy - 200 + k * 180 + Math.sin(t * 7) * 60, pressure: .3 + .7 * Math.sin(t * Math.PI), timeStamp: 1000 * k + i}; });
      const start = performance.now();
      d.down({...points[0], released: true, samples: points.slice(1)});
      await idle();
      measure.strokes.push(performance.now() - start);
    }
    // Let the history move the strokes' records to the disk tier.
    const history = d.owner.history;
    // Where undo and redo spend their time: worker requests by kind, timed from the page.
    const spent = {};
    const wrap = (target, name) => { const request = target.request; target.request = function (kind, ...rest) { const start = performance.now(), done = () => { const o = spent[`${name} ${kind}`] ??= {count: 0, ms: 0}; o.count++; o.ms += performance.now() - start; }; const p = request.call(this, kind, ...rest); p.then(done, done); return p; }; };
    wrap(d.owner.worker, 'pixel'); if (history?.disk) wrap(history.disk, 'history');
    for (let i = 0; i < 200 && (history?.stats.pending || !history?.stats.diskBytes); i++) await new Promise(r => setTimeout(r, 50));
    measure.historyBefore = history?.stats;
    measure.workerBefore = await d.owner.worker.request('engine', {});
    for (let k = 0; k < 3; k++) { const start = performance.now(); await e.undo(); await d.settle(); await idle(); measure.undo.push(performance.now() - start); }
    for (let k = 0; k < 3; k++) { const start = performance.now(); await e.redo(); await d.settle(); await idle(); measure.redo.push(performance.now() - start); }
    // The same with publication settled first: undo and redo alone (the pixel worker also
    // publishes; back to back, an edit can wait for the previous edit's publication).
    measure.settledUndo = []; measure.settledRedo = [];
    for (let k = 0; k < 3; k++) { await d.owner.flush(); const start = performance.now(); await e.undo(); await d.settle(); await idle(); measure.settledUndo.push(performance.now() - start); }
    for (let k = 0; k < 3; k++) { await d.owner.flush(); const start = performance.now(); await e.redo(); await d.settle(); await idle(); measure.settledRedo.push(performance.now() - start); }
    measure.historyAfter = history?.stats;
    measure.workerAfter = await d.owner.worker.request('engine', {});
    measure.spent = Object.fromEntries(Object.entries(spent).map(([k, v]) => [k, {count: v.count, ms: Math.round(v.ms)}]));
    d.tool = 'fill'; await d.settle();
    const fillStart = performance.now();
    d.down({pointerId: 6, pointerType: 'mouse', button: 0, buttons: 1, clientX: cx + 10, clientY: cy + 10, pressure: 0, timeStamp: 9000, released: true, samples: []});
    await idle(); measure.fill = performance.now() - fillStart;
    const publishStart = performance.now(); await d.owner.flush(); measure.publish = performance.now() - publishStart;
    measure.undoLabel = e.undoLabel;
    return measure;
  });
  await page.close();
  return {engine, ...result, errors};
}

const results = [];
for (let round = 1; round <= rounds; round++) for (const engine of round % 2 ? ['javascript', 'rust'] : ['rust', 'javascript']) {
  const result = await run(engine);
  results.push(result);
  console.log(JSON.stringify({round, engine, strokes: result.strokes.map(Math.round), undo: result.undo.map(Math.round), redo: result.redo.map(Math.round), settledUndo: result.settledUndo.map(Math.round), settledRedo: result.settledRedo.map(Math.round), fill: Math.round(result.fill), publish: Math.round(result.publish), disk: result.historyBefore, spent: result.spent, worker: [result.workerBefore, result.workerAfter].map(w => w && {memoryMiB: +(w.memory / 2 ** 20).toFixed(1), generation: w.generation}), errors: result.errors.length}));
}
await browser.close();
await mkdir(path.dirname(out), {recursive: true});
await writeFile(out, JSON.stringify(results, null, 2));
