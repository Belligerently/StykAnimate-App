// Desktop (Electron) check of the drawing raster engine in the app: the stroke, pixel and
// history workers and the brush model run the Rust engine (src/engine/engine-drawing.mjs),
// and the same scripted work (pen strokes with pressure and tilt on the stroke worker, a
// fill, a selection with a filter, a moved selection, an eraser and a smudge, undo and redo)
// leaves every cel identical, pixel for pixel, to a run of the JavaScript raster engine
// (`stykanimate.drawingEngine` = off).
//
//   unset ELECTRON_RUN_AS_NODE; xvfb-run -a node tests/drawing-engine.desktop.mjs
import {_electron as electron} from './support/playwright.mjs';
import assert from 'node:assert/strict';
import {mkdir, writeFile} from 'node:fs/promises';
import path from 'node:path';

const root = path.resolve('.'), out = path.join(root, 'artifacts/drawing-engine'), checks = [];
const check = (name, value, detail) => { assert.ok(value, `${name}${detail ? ` ${JSON.stringify(detail)}` : ''}`); checks.push(name); console.log('PASS', name); };

async function session(engine) {
  const profile = path.join(out, `profile-${engine}-${Date.now()}`);
  await mkdir(profile, {recursive: true});
  await writeFile(path.join(profile, 'telemetry-state.json'), JSON.stringify({decided: true, consent: {analytics: false, diagnostics: false, policyVersion: '2026-09-10', updatedAt: Date.now()}}));
  const env = {...process.env, STYK_USER_DATA_DIR: profile, STYK_LIBRARY_DIR: path.join(profile, 'library')}; delete env.ELECTRON_RUN_AS_NODE;
  const app = await electron.launch({args: [root], cwd: root, env, timeout: 45000});
  const errors = [];
  try {
    let page = await app.firstWindow();
    page.on('pageerror', error => errors.push(error.message));
    await page.waitForFunction(() => window.styk?.ready, undefined, {timeout: 30000});
    await page.evaluate(value => { if (value === 'off') localStorage.setItem('stykanimate.drawingEngine', 'off'); else localStorage.removeItem('stykanimate.drawingEngine'); }, engine);
    await page.reload();
    await page.waitForFunction(() => window.styk?.ready, undefined, {timeout: 30000});
    await page.setViewportSize?.({width: 1440, height: 1000}).catch(() => {});
    await page.evaluate(() => { for (const dialog of document.querySelectorAll('dialog[open]')) dialog.close(); document.querySelector('[data-workspace-mode="draw"]').click(); });
    await page.waitForFunction(() => styk.drawing.active, undefined, {timeout: 10000});
    await page.evaluate(async () => {
      const {emptyDocument} = await import('/src/demo.mjs'), e = styk.editor, d = styk.drawing, report = emptyDocument();
      report.canvas_width = 1280; report.canvas_height = 720;
      e.load(report); d.bind(); e.createDrawingLayer(); styk.state.settings.autosaveSeconds = 0; styk.fit();
      // The stroke worker paints (on WebGPU devices the GPU brush would).
      d.gpuBrushDisabled = true;
    });
    // Which engine answers in each thread.
    const engines = await page.evaluate(async () => {
      const d = styk.drawing, owner = d.owner, module = await import('/src/engine/engine-drawing.mjs');
      await module.loadDrawingEngine();
      const pixels = (await owner.worker.request('engine', {})).engine;
      const strokes = await new Promise(resolve => { const w = owner.strokeWorker, previous = w.onmessage; w.onmessage = data => { if (data.kind === 'engine') { w.onmessage = previous; resolve(data.engine); } else previous?.(data); }; w.post({kind: 'engine'}); });
      return {page: module.drawingEngine() ? 'rust' : 'javascript', pixels, strokes};
    });
    const box = await page.locator('#viewport').boundingBox(), cx = box.x + box.width / 2, cy = box.y + box.height / 2;
    const idle = () => page.waitForFunction(() => !styk.drawing.gesture && !styk.drawing.loading && !styk.drawing.operation, undefined, {timeout: 30000});
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
    await pen('ink', path(60, t => [cx - 300 + t * 600, cy + Math.sin(t * 6) * 80, .2 + .8 * Math.sin(t * Math.PI), 20 * Math.cos(t * 3), 15]), {color: '#203040'});
    await pen('pencil', path(50, t => [cx - 250 + t * 500, cy - 120 + Math.cos(t * 9) * 30, .3 + .6 * t]), {color: '#a02010'});
    await pen('charcoal', path(40, t => [cx - 200 + Math.cos(t * 5) * 150, cy + 100 + Math.sin(t * 5) * 60, .9 - .5 * t, 30, -20]), {color: '#105020'});
    await pen('brushpen', path(45, t => [cx + 50 + t * 250, cy - 50 + t * t * 120, .5 + .5 * Math.sin(t * 7)]), {color: '#5010a0'});
    await pen('marker', path(30, t => [cx - 280 + t * 560, cy + 40, 1]), {color: '#30a0f0', blend: 'multiply'});
    await pen('blender', path(35, t => [cx - 260 + t * 300, cy - 20 + t * 90, .8]));
    await page.locator('[data-tool="drawing-eraser"]').click();
    await pen(null, path(30, t => [cx - 100 + t * 200, cy + Math.cos(t * 4) * 30, .7]), {size: 18});
    await page.locator('[data-tool="drawing-brush"]').click();
    // Fill an enclosed area of the ink.
    await page.keyboard.press('g');
    await page.mouse.click(cx + 5, cy + 150);
    await idle();
    // A rectangle selection, blurred, then moved.
    await page.keyboard.press('l');
    await page.locator('.drawing-quickbar [data-draw-choice="selectShape"][data-value="rect"]').click();
    await page.mouse.move(cx - 220, cy - 90); await page.mouse.down(); await page.mouse.move(cx - 20, cy + 60, {steps: 4}); await page.mouse.up();
    await idle();
    await page.locator('details[data-section="Filters"] summary').click();
    await page.locator('[data-draw-action="filter-blur"]').click();
    await page.waitForFunction(() => styk.editor.undoLabel === 'Blur drawing', undefined, {timeout: 30000});
    await page.mouse.move(cx - 120, cy - 10); await page.mouse.down(); await page.mouse.move(cx - 60, cy + 25, {steps: 5}); await page.mouse.up();
    await page.keyboard.press('Enter');
    await idle();
    await page.evaluate(() => styk.drawing.settle?.());
    await page.keyboard.press('Control+Shift+A');
    // Undo and redo the last two edits through the history.
    await page.keyboard.press('Control+z'); await page.keyboard.press('Control+z'); await page.keyboard.press('Control+Shift+z'); await page.keyboard.press('Control+Shift+z');
    await idle();
    const cels = await page.evaluate(async () => {
      const e = styk.editor, d = styk.drawing, out = []; await d.barrier();
      for (const figure of e.report.figure_types) {
        const meta = figure._styk?.drawing; if (!meta) continue;
        const entry = await d.owner.surface(figure), b = entry.store.bounds;
        let hash = 0; if (b) { const px = entry.store.readRect(b.x, b.y, b.width, b.height); for (let i = 0; i < px.length; i++) hash = (Math.imul(hash, 31) + px[i]) | 0; }
        out.push({bounds: b, tiles: entry.store.size, hash});
      }
      return {cels: out, undo: e.undoLabel, stats: {...d.strokeStats}};
    });
    return {engines, ...cels, errors};
  } finally {
    const proc = app.process();
    await Promise.race([app.close().catch(() => {}), new Promise(resolve => setTimeout(resolve, 10000))]);
    if (proc.exitCode === null) proc.kill();
  }
}

await mkdir(out, {recursive: true});
const rust = await session('on');
console.log(JSON.stringify({engines: rust.engines, cels: rust.cels, stats: rust.stats}));
check('the stroke worker, the pixel worker and the brush model run the Rust engine', rust.engines.page === 'rust' && rust.engines.pixels === 'rust' && rust.engines.strokes === 'rust', rust.engines);
check('the scripted strokes went through the stroke worker', rust.stats.batches > 10, rust.stats);
check('the work left artwork', rust.cels.length === 1 && rust.cels[0].bounds && rust.cels[0].tiles > 10, rust.cels);
const javascript = await session('off');
console.log(JSON.stringify({engines: javascript.engines, cels: javascript.cels}));
check('with the gate off every thread runs the JavaScript engine', javascript.engines.page === 'javascript' && javascript.engines.pixels === 'javascript' && javascript.engines.strokes === 'javascript', javascript.engines);
check('the cels are identical, pixel for pixel, to the JavaScript raster engine\'s', JSON.stringify(rust.cels) === JSON.stringify(javascript.cels), {rust: rust.cels, javascript: javascript.cels});
check('the same history', rust.undo === javascript.undo, {rust: rust.undo, javascript: javascript.undo});
check('no uncaught renderer errors', !rust.errors.length && !javascript.errors.length, [...rust.errors, ...javascript.errors]);
await writeFile(path.join(out, 'drawing-engine-desktop.json'), JSON.stringify({checks, rust, javascript}, null, 2));
