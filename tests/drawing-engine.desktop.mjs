// Desktop (Electron) check of the drawing raster engine in the app: the stroke, pixel and
// history workers and the brush model run the Rust engine (src/engine/engine-drawing.mjs),
// the same scripted work (pen strokes with pressure and tilt on the stroke worker, a fill,
// erasing and smudging, a blurred and moved selection, undo and redo) leaves every cel and
// every published tile blob identical to the JavaScript raster engine's
// (`stykanimate.drawingEngine` = off), and saved files cross between the two: each engine
// opens the other's file pixel for pixel and goes on editing it identically.
//
//   unset ELECTRON_RUN_AS_NODE; xvfb-run -a node tests/drawing-engine.desktop.mjs
//
// Sessions: Rust draws and saves A; JavaScript draws the same and saves B, opens A, edits
// it; Rust opens B and makes the same edit.
import {_electron as electron} from './support/playwright.mjs';
import assert from 'node:assert/strict';
import {mkdir, writeFile} from 'node:fs/promises';
import path from 'node:path';

const root = path.resolve('.'), out = path.join(root, 'artifacts/drawing-engine'), checks = [];
const check = (name, value, detail) => { assert.ok(value, `${name}${detail ? ` ${JSON.stringify(detail)}` : ''}`); checks.push(name); console.log('PASS', name); };
const stamp = Date.now(), fileA = path.join(out, `rust-${stamp}.styk`), fileB = path.join(out, `javascript-${stamp}.styk`);

async function launch(engine) {
  const profile = path.join(out, `profile-${engine}-${Date.now()}`);
  await mkdir(profile, {recursive: true});
  await writeFile(path.join(profile, 'telemetry-state.json'), JSON.stringify({decided: true, consent: {analytics: false, diagnostics: false, policyVersion: '2026-09-10', updatedAt: Date.now()}}));
  const env = {...process.env, STYK_USER_DATA_DIR: profile, STYK_LIBRARY_DIR: path.join(profile, 'library')}; delete env.ELECTRON_RUN_AS_NODE;
  const app = await electron.launch({args: [root], cwd: root, env, timeout: 45000});
  const errors = [], page = await app.firstWindow();
  page.on('pageerror', error => errors.push(error.message));
  await page.waitForFunction(() => window.styk?.ready, undefined, {timeout: 30000});
  await page.evaluate(value => { if (value === 'off') localStorage.setItem('stykanimate.drawingEngine', 'off'); else localStorage.removeItem('stykanimate.drawingEngine'); }, engine);
  await page.reload();
  await page.waitForFunction(() => window.styk?.ready, undefined, {timeout: 30000});
  await page.setViewportSize?.({width: 1440, height: 1000}).catch(() => {});
  await page.evaluate(() => { styk.state.settings.autosaveSeconds = 0; for (const dialog of document.querySelectorAll('dialog[open]')) dialog.close(); document.querySelector('[data-workspace-mode="draw"]').click(); });
  await page.waitForFunction(() => styk.drawing.active, undefined, {timeout: 10000});
  const close = async () => {
    const proc = app.process();
    // An unsaved document would ask before quitting.
    await page.evaluate(async () => { styk.editor.markSaved(); await globalThis.stykDesktop?.setDocumentState({name: styk.editor.name, dirty: false}); }).catch(() => {});
    await Promise.race([app.close().catch(() => {}), new Promise(resolve => setTimeout(resolve, 10000))]);
    if (proc.exitCode === null) proc.kill();
  };
  return {app, page, errors, close};
}
/** A fresh 1280×720 document with one drawing layer; the stroke worker paints. */
const fresh = page => page.evaluate(async () => {
  const {emptyDocument} = await import('/src/demo.mjs'), e = styk.editor, d = styk.drawing, report = emptyDocument();
  report.canvas_width = 1280; report.canvas_height = 720;
  e.load(report); d.bind(); e.createDrawingLayer(); styk.fit();
  d.gpuBrushDisabled = true;
});
async function open(app, page, file) {
  await app.evaluate(({dialog}, file) => { dialog.showOpenDialog = async () => ({canceled: false, filePaths: [file]}); dialog.showMessageBox = async () => ({response: 1}); }, file);
  await page.evaluate(() => styk.actions.open());
  await page.waitForFunction(() => styk.editor.report.figure_types.some(f => f._styk?.drawing), undefined, {timeout: 30000});
  // Opening a file returns to the workspace it was saved from; draw on its drawing layer.
  await page.evaluate(() => { for (const dialog of document.querySelectorAll('dialog[open]')) dialog.close(); document.querySelector('[data-workspace-mode="draw"]').click(); });
  await page.waitForFunction(() => styk.drawing.active, undefined, {timeout: 10000});
  await page.evaluate(() => {
    const e = styk.editor, d = styk.drawing, layer = e.layers.find(l => l.drawing?.version === 1);
    if (layer) e.selectSheetLayer(layer.id);
    d.bind(); styk.fit(); d.gpuBrushDisabled = true;
  });
}
async function save(app, page, file) {
  await app.evaluate(({dialog}, file) => { dialog.showSaveDialog = async () => ({canceled: false, filePath: file}); }, file);
  await page.keyboard.press('Control+s');
  await page.waitForFunction(() => !styk.editor.dirty, undefined, {timeout: 30000});
}
/** Which engine answers in each thread. */
const engines = page => page.evaluate(async () => {
  const owner = styk.drawing.owner, module = await import('/src/engine/engine-drawing.mjs');
  await module.loadDrawingEngine();
  const pixels = (await owner.worker.request('engine', {})).engine;
  const strokes = await new Promise(resolve => { const w = owner.strokeWorker, previous = w.onmessage; w.onmessage = data => { if (data.kind === 'engine') { w.onmessage = previous; resolve(data.engine); } else previous?.(data); }; w.post({kind: 'engine'}); });
  return {page: module.drawingEngine() ? 'rust' : 'javascript', pixels, strokes};
});
/** Every cel's pixels and published tile blob, hashed. */
const snapshot = page => page.evaluate(async () => {
  const e = styk.editor, d = styk.drawing, cels = [];
  await d.barrier();
  for (const figure of e.report.figure_types) {
    const meta = figure._styk?.drawing; if (!meta) continue;
    const entry = await d.owner.surface(figure), b = entry.store.bounds;
    let pixels = 0; if (b) { const px = entry.store.readRect(b.x, b.y, b.width, b.height); for (let i = 0; i < px.length; i++) pixels = (Math.imul(pixels, 31) + px[i]) | 0; }
    const blob = figure.image_assets?.[0]?.styk_tiles ?? '';
    let published = 0; for (let i = 0; i < blob.length; i++) published = (Math.imul(published, 31) + blob.charCodeAt(i)) | 0;
    cels.push({bounds: b, tiles: entry.store.size, pixels, published, blobLength: blob.length});
  }
  return {cels, undo: e.undoLabel, stats: {...d.strokeStats}};
});

/** The scripted work: part 1 on a new drawing, part 2 on an opened file. */
async function work(page, part) {
  const box = await page.locator('#viewport').boundingBox(), cx = box.x + box.width / 2, cy = box.y + box.height / 2;
  const idle = () => page.waitForFunction(() => !styk.drawing.gesture && !styk.drawing.loading && !styk.drawing.operation, undefined, {timeout: 60000});
  /** A pen stroke as one pointer-down with its samples (exact inputs, independent of input timing). */
  const pen = async (preset, points, extra = {}) => {
    await page.evaluate(({preset, points, extra}) => {
      const d = styk.drawing;
      if (preset) document.querySelector(`[data-draw-preset="${preset}"]`)?.click();
      Object.assign(d.settings, extra);
      const [first, ...rest] = points.map((p, i) => ({pointerId: 77, pointerType: 'pen', button: 0, buttons: 1, clientX: p[0], clientY: p[1], pressure: p[2], tiltX: p[3] ?? 0, tiltY: p[4] ?? 0, timeStamp: 1000 + i}));
      d.down({...first, released: true, samples: rest});
    }, {preset, points, extra});
    await idle();
  };
  const path = (n, f) => Array.from({length: n}, (_, i) => f(i / (n - 1)));
  await page.locator('[data-tool="drawing-brush"]').click();
  if (part === 1) {
    await pen('ink', path(60, t => [cx - 300 + t * 600, cy + Math.sin(t * 6) * 80, .2 + .8 * Math.sin(t * Math.PI), 20 * Math.cos(t * 3), 15]), {color: '#203040'});
    await pen('pencil', path(50, t => [cx - 250 + t * 500, cy - 120 + Math.cos(t * 9) * 30, .3 + .6 * t]), {color: '#a02010'});
    await pen('charcoal', path(40, t => [cx - 200 + Math.cos(t * 5) * 150, cy + 100 + Math.sin(t * 5) * 60, .9 - .5 * t, 30, -20]), {color: '#105020'});
    await pen('brushpen', path(45, t => [cx + 50 + t * 250, cy - 50 + t * t * 120, .5 + .5 * Math.sin(t * 7)]), {color: '#5010a0'});
    await pen('marker', path(30, t => [cx - 280 + t * 560, cy + 40, 1]), {color: '#30a0f0', blend: 'multiply'});
    await pen('blender', path(35, t => [cx - 260 + t * 300, cy - 20 + t * 90, .8]));
    await page.locator('[data-tool="drawing-eraser"]').click();
    await pen(null, path(30, t => [cx - 100 + t * 200, cy + Math.cos(t * 4) * 30, .7]), {size: 18});
    await page.locator('[data-tool="drawing-brush"]').click();
    // Fill around the ink.
    await page.keyboard.press('g');
    await page.mouse.click(cx + 5, cy + 150);
    await idle();
    return;
  }
  await pen('soft', path(40, t => [cx - 200 + t * 420, cy + 60 - t * 160, .4 + .6 * t]), {color: '#e09020', size: 60});
  await pen('oil', path(40, t => [cx + 150 - t * 300, cy - 60 + Math.sin(t * 5) * 40, .9]));
  // A rectangle selection, blurred, then moved.
  await page.keyboard.press('l');
  await page.locator('.drawing-quickbar [data-draw-choice="selectShape"][data-value="rect"]').click();
  await page.mouse.move(cx - 220, cy - 90); await page.mouse.down(); await page.mouse.move(cx - 20, cy + 60, {steps: 4}); await page.mouse.up();
  await idle();
  await page.locator('details[data-section="Filters"] summary').click();
  await page.locator('[data-draw-action="filter-blur"]').click();
  await page.waitForFunction(() => styk.editor.undoLabel === 'Blur drawing', undefined, {timeout: 60000});
  await page.mouse.move(cx - 120, cy - 10); await page.mouse.down(); await page.mouse.move(cx - 60, cy + 25, {steps: 5}); await page.mouse.up();
  await page.keyboard.press('Enter');
  await idle();
  await page.evaluate(() => styk.drawing.settle?.());
  await page.keyboard.press('Control+Shift+A');
  // Undo and redo the last two edits through the history.
  await page.keyboard.press('Control+z'); await page.keyboard.press('Control+z'); await page.keyboard.press('Control+Shift+z'); await page.keyboard.press('Control+Shift+z');
  await idle();
}

await mkdir(out, {recursive: true});
const result = {};
let session = await launch('on');
try {
  await fresh(session.page);
  result.rustEngines = await engines(session.page);
  await work(session.page, 1);
  result.rust1 = await snapshot(session.page);
  await save(session.app, session.page, fileA);
  result.rustErrors = session.errors;
} finally { await session.close(); }
check('the stroke worker, the pixel worker and the brush model run the Rust engine', Object.values(result.rustEngines).every(e => e === 'rust'), result.rustEngines);
check('the scripted strokes went through the stroke worker', result.rust1.stats.batches > 10, result.rust1.stats);
check('the work left artwork', result.rust1.cels.length === 1 && result.rust1.cels[0].tiles > 10, result.rust1.cels);

session = await launch('off');
try {
  await fresh(session.page);
  result.javascriptEngines = await engines(session.page);
  await work(session.page, 1);
  result.javascript1 = await snapshot(session.page);
  await save(session.app, session.page, fileB);
  await open(session.app, session.page, fileA);
  result.javascriptOpensRust = await snapshot(session.page);
  await work(session.page, 2);
  result.javascript2 = await snapshot(session.page);
  result.javascriptErrors = session.errors;
} finally { await session.close(); }
const same = (a, b) => JSON.stringify(a.cels) === JSON.stringify(b.cels);
check('with the gate off every thread runs the JavaScript engine', Object.values(result.javascriptEngines).every(e => e === 'javascript'), result.javascriptEngines);
check('drawing: the cels and their published tiles are identical to the JavaScript raster engine\'s', same(result.rust1, result.javascript1), {rust: result.rust1.cels, javascript: result.javascript1.cels});
check('the JavaScript engine opens the Rust engine\'s file pixel for pixel', JSON.stringify(result.javascriptOpensRust.cels.map(c => [c.bounds, c.tiles, c.pixels])) === JSON.stringify(result.rust1.cels.map(c => [c.bounds, c.tiles, c.pixels])), result.javascriptOpensRust.cels);

session = await launch('on');
try {
  await open(session.app, session.page, fileB);
  result.rustOpensJavaScript = await snapshot(session.page);
  await work(session.page, 2);
  result.rust2 = await snapshot(session.page);
  result.rustErrors2 = session.errors;
} finally { await session.close(); }
check('the Rust engine opens the JavaScript engine\'s file pixel for pixel', JSON.stringify(result.rustOpensJavaScript.cels.map(c => [c.bounds, c.tiles, c.pixels])) === JSON.stringify(result.javascript1.cels.map(c => [c.bounds, c.tiles, c.pixels])), result.rustOpensJavaScript.cels);
check('editing an opened file: the same cels, published tiles and history in both engines', same(result.rust2, result.javascript2) && result.rust2.undo === result.javascript2.undo, {rust: result.rust2, javascript: result.javascript2});
check('no uncaught renderer errors', ![...result.rustErrors, ...result.javascriptErrors, ...result.rustErrors2].length, [...result.rustErrors, ...result.javascriptErrors, ...result.rustErrors2]);
await writeFile(path.join(out, 'drawing-engine-desktop.json'), JSON.stringify({checks, result}, null, 2));
process.exit(0);
