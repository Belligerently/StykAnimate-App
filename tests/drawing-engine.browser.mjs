// The drawing raster engine in a browser's drawing workers: the requests that need a
// canvas (cel thumbnails, viewport previews) and the stroke and pixel workers answer the
// same with the Rust engine as with the JavaScript one (`?drawingEngine=off`), worker by
// worker, on a painted cel; the gate reaches every worker.
//
//   npm run serve -- --port 47851 &   STYK_URL=http://127.0.0.1:47851 node tests/drawing-engine.browser.mjs
import {chromium} from './support/playwright.mjs';
import assert from 'node:assert/strict';

const url = process.env.STYK_URL || 'http://127.0.0.1:47839';
const browser = await chromium.launch({channel: 'chrome', headless: true});
const checks = [];
const check = (name, value, detail) => { assert.ok(value, `${name}${detail ? ` ${JSON.stringify(detail)}` : ''}`); checks.push(name); console.log('PASS', name); };

/** Paints a cel through the app, then asks fresh drawing workers for canvas work. */
async function run(engine) {
  const page = await browser.newPage({viewport: {width: 1440, height: 1000}}), errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.goto(engine === 'off' ? `${url}/?drawingEngine=off` : url);
  await page.waitForFunction(() => window.styk?.ready);
  await page.locator('[data-workspace-mode="draw"]').click();
  const result = await page.evaluate(async () => {
    const {emptyDocument} = await import('/src/demo.mjs'), {DrawingWorker} = await import('/src/drawing-worker-client.mjs'), {drawingEngine} = await import('/src/engine/engine-drawing.mjs');
    const e = styk.editor, d = styk.drawing, report = emptyDocument();
    report.canvas_width = 960; report.canvas_height = 540;
    e.load(report); d.bind(); e.createDrawingLayer(); styk.fit(); d.gpuBrushDisabled = true;
    const box = document.querySelector('#viewport').getBoundingClientRect(), cx = box.x + box.width / 2, cy = box.y + box.height / 2;
    const idle = () => new Promise(resolve => { const poll = () => (!d.gesture && !d.loading && !d.operation ? resolve() : setTimeout(poll, 20)); poll(); });
    for (const [preset, color, phase] of [['ink', '#203040', 0], ['soft', '#e05020', 1], ['charcoal', '#105020', 2], ['blender', null, 3]]) {
      document.querySelector(`[data-draw-preset="${preset}"]`).click();
      if (color) d.settings.color = color;
      const points = Array.from({length: 50}, (_, i) => { const t = i / 49; return {pointerId: 9, pointerType: 'pen', button: 0, buttons: 1, clientX: cx - 250 + t * 500, clientY: cy + Math.sin(t * 6 + phase) * 90 - 40 + phase * 25, pressure: .2 + .8 * Math.sin(t * Math.PI), tiltX: 10, tiltY: -20, timeStamp: 100 + i}; });
      d.down({...points[0], released: true, samples: points.slice(1)});
      await idle();
    }
    await d.barrier();
    const figure = e.report.figure_types.find(f => f._styk?.drawing), entry = await d.owner.surface(figure), store = entry.store;
    const tiles = [...store].map(t => ({tx: t.tx, ty: t.ty, pixels: t.pixels.slice().buffer}));
    const hash = bytes => { let h = 0; for (let i = 0; i < bytes.length; i++) h = (Math.imul(h, 31) + bytes[i]) | 0; return h; };
    const worker = new DrawingWorker();
    const engineName = (await worker.request('engine', {})).engine;
    const thumbnail = await worker.request('thumbnail', {tiles: tiles.map(t => ({...t, pixels: t.pixels.slice(0)}))});
    const blob = await worker.request('thumbnail', {url: figure.image_assets[0].styk_tiles});
    const b = store.bounds, previews = [];
    for (const level of [0, 1, 3]) {
      const rect = {x: Math.floor(b.x / 2 ** level / 64) * 64 - 64, y: Math.floor(b.y / 2 ** level / 64) * 64 - 64, width: Math.min(4096, Math.ceil(b.width / 2 ** level / 64) * 64 + 128), height: Math.min(4096, Math.ceil(b.height / 2 ** level / 64) * 64 + 128)};
      const reply = await worker.request('preview', {tiles: level === 0 ? tiles.map(t => ({...t, pixels: t.pixels.slice(0)})) : [], reset: level === 0, keep: tiles.map(t => `${t.tx},${t.ty}`), rect, level});
      const canvas = new OffscreenCanvas(rect.width, rect.height), ctx = canvas.getContext('2d');
      ctx.drawImage(reply.bitmap, 0, 0); reply.bitmap.close();
      previews.push(hash(ctx.getImageData(0, 0, rect.width, rect.height).data));
    }
    worker.dispose();
    // Memory of the page and its workers after the work (cross-origin isolated pages only).
    let memory = null;
    if (performance.measureUserAgentSpecificMemory) {
      const measured = await performance.measureUserAgentSpecificMemory();
      const byScope = {};
      for (const item of measured.breakdown) for (const attribution of item.attribution) { const scope = attribution.scope === 'Window' ? 'page' : (attribution.url?.split('/').pop()?.split('?')[0] ?? attribution.scope); byScope[scope] = (byScope[scope] ?? 0) + item.bytes / (item.attribution.length || 1); }
      memory = {totalMiB: +(measured.bytes / 2 ** 20).toFixed(1), byScope: Object.fromEntries(Object.entries(byScope).map(([k, v]) => [k, +(v / 2 ** 20).toFixed(1)]))};
    }
    let pixels = 0; { const px = store.readRect(b.x, b.y, b.width, b.height); pixels = hash(px); }
    return {page: drawingEngine() ? 'rust' : 'javascript', pixelWorker: engineName, cel: {bounds: b, tiles: store.size, pixels}, published: hash(new TextEncoder().encode(figure.image_assets[0].styk_tiles)),
      thumbnail: {width: thumbnail.width, height: thumbnail.height, pixels: hash(new Uint8Array(thumbnail.pixels))}, blobThumbnail: {width: blob.width, height: blob.height, pixels: hash(new Uint8Array(blob.pixels))}, previews, batches: d.strokeStats.batches, memory};
  });
  await page.close();
  return {...result, errors};
}

try {
  const rust = await run('on'), javascript = await run('off');
  console.log(JSON.stringify({rust, javascript}));
  console.log(`memory after the work: Rust ${JSON.stringify(rust.memory)}, JavaScript ${JSON.stringify(javascript.memory)}`);
  check('the page and a new pixel worker run the Rust engine by default', rust.page === 'rust' && rust.pixelWorker === 'rust', rust);
  check('?drawingEngine=off reaches the page and the workers', javascript.page === 'javascript' && javascript.pixelWorker === 'javascript', javascript);
  check('the strokes went through the stroke worker', rust.batches > 3 && javascript.batches > 3);
  check('the painted cel and its published tiles are identical', JSON.stringify(rust.cel) === JSON.stringify(javascript.cel) && rust.published === javascript.published, {rust: rust.cel, javascript: javascript.cel});
  check('cel thumbnails from tiles and from the tile blob are identical', JSON.stringify([rust.thumbnail, rust.blobThumbnail]) === JSON.stringify([javascript.thumbnail, javascript.blobThumbnail]), {rust: [rust.thumbnail, rust.blobThumbnail], javascript: [javascript.thumbnail, javascript.blobThumbnail]});
  check('viewport previews at levels 0, 1 and 3 are identical', JSON.stringify(rust.previews) === JSON.stringify(javascript.previews), {rust: rust.previews, javascript: javascript.previews});
  check('no uncaught page errors', !rust.errors.length && !javascript.errors.length, [...rust.errors, ...javascript.errors]);
} finally { await browser.close(); }
