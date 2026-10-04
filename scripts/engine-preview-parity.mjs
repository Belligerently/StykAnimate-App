#!/usr/bin/env node
// Thumbnails, library previews and the camera monitor drawn by the engine, against the JavaScript paths, in the app.
//
//   node scripts/engine-preview-parity.mjs [--target web|electron] [--file <project>]... [--ticks 0,10,20]
//        [--sizes 320x192,512x288,96x54] [--export-check] [--library <file>]... [--elapsed 0,0.4,1.2] [--monitor]
//        [--timing] [--rounds 3] [--cpu-check] [--label <name>] [--out <dir>] [--url http://127.0.0.1:47831] [--electron-arg <switch>]...
//
// Thumbnails (each --file, or the document the app starts with): one run of the app with the engine. For each
// document, tick (by default up to five frame starts spread over the timeline) and size (the filmstrip's 320×192,
// the export dialog's stage 512×288 and its 96×54 strip), the app's own thumbnail renderer
// (src/engine/engine-thumbnails.mjs: the engine where it draws the frame) and a JavaScript one in the same page
// (src/thumbnail-renderer.mjs) draw the same frame; each step records which renderer drew the first, and why not the
// engine; with --export-check an engine thumbnail is also compared, where its canvas area falls on whole pixels, with
// the engine's export frame of that tick at that size (the stage's pixels; the "-against-export" steps). Library
// previews (each --library file): the poster (src/library-files.mjs) and the hover animation's
// frames at --elapsed seconds and its backdrop, from the engine (src/engine/engine-previews.mjs) and from the
// JavaScript worker (src/library-preview-worker.mjs). Camera monitor (--monitor): the app with and without the
// engine (?engine=off), the camera selected so the inspector shows its monitor, the monitor's canvas captured at
// each tick and after playback.
//
// --cpu-check: each engine thumbnail, library poster and hover frame, and an export frame of each tick at the canvas
// size, drawn again with the CPU rasterizer (`stykanimate.engine.cpu` set: the preview worker and the export worker draw
// with styk-raster, as on devices without a GPU device) and compared with the GPU's picture (the "-cpu" steps; counted
// only where the first picture came from the GPU).
//
// Pictures are compared with the app-parity rule D13 (docs/engine/decisions/0013-app-parity-rule.md; the limits of
// scripts/engine-app-parity.mjs, never changed to pass): a step fails when a pixel differs by more than 32 levels
// together with all eight neighbours, or when the mean difference, not counting differences of two levels or less,
// exceeds 0.25. Alpha counts as a channel (library posters are on transparency).
//
// --timing: per document, --rounds interleaved rounds (the order of the two renderers alternating) of the
// filmstrip's 320×192 thumbnails of every frame start (at most 48): each renderer is created fresh, so the JavaScript
// renderer's first thumbnail includes starting its worker and copying the document into it, then the rest; with
// --library, each file's poster, the start of its hover animation and twelve hover frames per round the same way
// (the engine's preview worker is released first, so its first poster includes starting the worker and its GPU
// device). The load average is recorded at the start and end of each round.
//
// Under Xvfb on Linux pass `--electron-arg --use-angle=vulkan`: WebGPU then reaches the GPU and its canvases can be
// copied (the thumbnails' and the monitor's are), as scripts/engine-export-parity.mjs does.
//
// Output: artifacts/engine-rust/parity/previews/<label>/ (pictures, difference pictures, report.json). Exit code 1
// when a step fails.
import {spawn, spawnSync} from 'node:child_process';
import {mkdir, mkdtemp, readFile, rm, writeFile} from 'node:fs/promises';
import {createRequire} from 'node:module';
import os from 'node:os';
import path from 'node:path';
import {chromium, _electron} from '../tests/support/playwright.mjs';

const root = path.resolve(import.meta.dirname, '..');
const args = process.argv.slice(2);
const option = (name, fallback) => { const at = args.lastIndexOf(name); return at < 0 ? fallback : args[at + 1]; };
const options = name => args.flatMap((value, index) => args[index - 1] === name ? [value] : []);
const flag = name => args.includes(name);
const files = options('--file');
const libraries = options('--library');
const target = option('--target', 'web');
const label = option('--label', `${target}-${files.length ? path.basename(files[0]).replace(/[^\w.-]+/g, '-').toLowerCase() : libraries.length ? 'library' : 'startup'}`);
const out = path.resolve(option('--out', `artifacts/engine-rust/parity/previews/${label}`));
const ticks = option('--ticks')?.split(',').map(Number) ?? null;
const sizes = option('--sizes', '320x192,512x288,96x54').split(',').map(size => size.split('x').map(Number));
const elapsed = option('--elapsed', '0,0.4,1.2').split(',').map(Number);
const rounds = Number(option('--rounds', 3));
const SETTINGS = {theme: 'light', locale: 'en', uiScale: 100, showHandles: false};
/** D13 (scripts/engine-app-parity.mjs). */
const LIMITS = {threshold: 32, mean: 0.25, quantum: 2};

async function server() {
  const given = option('--url');
  if (given) return {origin: given.replace(/\/$/, ''), stop: async () => {}};
  const port = 47000 + Math.floor(Math.random() * 900);
  const child = spawn(process.execPath, ['scripts/serve.mjs', '--port', String(port)], {cwd: root, stdio: 'ignore'});
  const origin = `http://127.0.0.1:${port}`;
  for (let i = 0; i < 100; i++) {
    try { if ((await fetch(origin)).ok) return {origin, stop: async () => { child.kill(); }}; } catch { /* not listening yet */ }
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  child.kill();
  throw new Error('The development server did not start.');
}

async function webApp(browser, origin, engine) {
  const context = await browser.newContext({viewport: {width: 1440, height: 900}});
  await context.addInitScript(settings => localStorage.setItem('stykanimate.editor.settings.v1', JSON.stringify(settings)), {...SETTINGS, rendererMode: 'canvas'});
  const page = await context.newPage();
  await page.goto(`${origin}/${engine ? '' : '?engine=off'}`);
  return {page, close: () => context.close()};
}

async function desktopApp(engine) {
  const profile = await mkdtemp(path.join(os.tmpdir(), 'styk-previews-'));
  await writeFile(path.join(profile, 'editor-settings.json'), JSON.stringify({...SETTINGS, rendererMode: 'auto'}));
  const env = {...process.env, STYK_STARTUP_VIEW: 'editor', STYK_USER_DATA_DIR: profile, STYK_LIBRARY_DIR: path.join(profile, 'library')};
  delete env.ELECTRON_RUN_AS_NODE;
  const app = await _electron.launch({executablePath: createRequire(path.join(root, 'package.json'))('electron'), args: [...options('--electron-arg'), root], cwd: root, env, timeout: 60000});
  const page = await app.firstWindow({timeout: 60000});
  await page.setViewportSize({width: 1440, height: 900}).catch(() => {});
  if (!engine) {
    await page.waitForFunction(() => window.styk?.ready === true, null, {timeout: 60000});
    await page.evaluate(() => localStorage.setItem('stykanimate.engine', 'off'));
    await page.reload();
  }
  return {page, electron: app, close: async () => { await app.close().catch(() => {}); await rm(profile, {recursive: true, force: true}); }};
}

async function open(browser, origin, engine) {
  const app = target === 'electron' ? await desktopApp(engine) : await webApp(browser, origin, engine);
  const {page} = app;
  app.problems = [];
  page.on('pageerror', error => app.problems.push(`pageerror ${error.message.slice(0, 300)}`));
  page.on('console', message => { if (message.type() === 'error') app.problems.push(message.text().slice(0, 300)); });
  await page.waitForFunction(() => window.styk?.ready === true, null, {timeout: 60000});
  await page.evaluate(() => { document.querySelector('.styk-privacy [data-decline]')?.click(); for (const dialog of document.querySelectorAll('dialog[open]')) dialog.close(); });
  return app;
}

/** Loads a project into the app as the editor opens one (once per page: a file already open stays). */
async function load(page, file) {
  if (!file || page.__loaded === file) return;
  page.__loaded = file;
  if (file.endsWith('.json')) {
    const text = await readFile(path.resolve(file), 'utf8');
    await page.evaluate(({text, name}) => { styk.editor.load(JSON.parse(text), name); styk.editor.markSaved(); }, {text, name: path.basename(file)});
  } else {
    const before = await page.evaluate(() => { window.__previewReport = styk.editor.report; return document.title; });
    await page.locator('#file-input').setInputFiles(path.resolve(file));
    await page.waitForFunction(() => styk.editor.report !== window.__previewReport && !styk.state.busy, null, {timeout: 600000});
    if (await page.title() === before) throw new Error(`${file} did not open`);
  }
  await page.evaluate(async () => { styk.editor.select(null); styk.fit(); for (let i = 0; i < 4; i++) await new Promise(resolve => requestAnimationFrame(resolve)); });
}

// ---------------------------------------------------------------------------------------------------------------
// In the page.

/** Installs helpers on the page: canvases, PNGs, the D13 comparison. */
async function install(page) {
  await page.evaluate(limits => {
    window.__preview = {
      canvas(width, height) { const canvas = document.createElement('canvas'); canvas.width = width; canvas.height = height; return canvas; },
      png(canvas) { return canvas.toDataURL('image/png').slice('data:image/png;base64,'.length); },
      async bitmapPNG(bitmap, width, height) {
        const canvas = this.canvas(width, height); canvas.getContext('2d').drawImage(bitmap, 0, 0); bitmap.close?.(); return this.png(canvas);
      },
      async blobPNG(blob) { const bitmap = await createImageBitmap(blob); const png = await this.bitmapPNG(bitmap, bitmap.width, bitmap.height); return png; },
      async compare(a, b) {
        const decode = data => createImageBitmap(new Blob([Uint8Array.from(atob(data), c => c.charCodeAt(0))], {type: 'image/png'}));
        const [first, second] = await Promise.all([decode(a), decode(b)]);
        if (first.width !== second.width || first.height !== second.height) return {error: `sizes differ: ${first.width}x${first.height} and ${second.width}x${second.height}`};
        const canvas = new OffscreenCanvas(first.width, first.height), context = canvas.getContext('2d', {willReadFrequently: true});
        const pixels = image => { context.clearRect(0, 0, canvas.width, canvas.height); context.drawImage(image, 0, 0); return context.getImageData(0, 0, canvas.width, canvas.height); };
        const left = pixels(first).data, shown = pixels(second), right = shown.data;
        const width = first.width, height = first.height, strongAt = new Uint8Array(width * height);
        let differing = 0, strong = 0, max = 0, sum = 0, beyond = 0;
        for (let i = 0, pixel = 0; i < left.length; i += 4, pixel++) {
          const d = Math.max(Math.abs(left[i] - right[i]), Math.abs(left[i + 1] - right[i + 1]), Math.abs(left[i + 2] - right[i + 2]), Math.abs(left[i + 3] - right[i + 3]));
          sum += d;
          if (d > limits.quantum) beyond += d;
          if (d) { differing++; if (d > max) max = d; }
          if (d > limits.threshold) { strong++; strongAt[pixel] = 1; }
          const fade = v => 255 - (255 - v) * 0.25;
          if (d > 8) { right[i] = 255; right[i + 1] = right[i + 2] = 255 - Math.min(255, d * 3); }
          else { right[i] = fade(right[i]); right[i + 1] = fade(right[i + 1]); right[i + 2] = fade(right[i + 2]); }
          right[i + 3] = 255;
        }
        let solid = 0;
        for (let y = 1; y < height - 1; y++) for (let x = 1; x < width - 1; x++) {
          const at = y * width + x;
          if (strongAt[at] && strongAt[at - 1] && strongAt[at + 1] && strongAt[at - width] && strongAt[at + width] &&
            strongAt[at - width - 1] && strongAt[at - width + 1] && strongAt[at + width - 1] && strongAt[at + width + 1]) {
            solid++;
            const i = at * 4; right[i] = 160; right[i + 1] = 0; right[i + 2] = 200;
          }
        }
        context.putImageData(shown, 0, 0);
        const blob = await canvas.convertToBlob({type: 'image/png'});
        const bytes = new Uint8Array(await blob.arrayBuffer());
        let text = ''; for (let i = 0; i < bytes.length; i += 0x8000) text += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
        const count = left.length / 4;
        return {width, height, differing, strong, solid, max, rawMean: sum / count, mean: beyond / count, picture: btoa(text)};
      },
    };
  }, LIMITS);
}

/**
 * Both renderers' thumbnails of each tick and size. With `exported`, an engine thumbnail whose canvas lands on whole
 * pixels is also compared with the engine's export frame of that tick at the canvas's size (src/engine/engine-export.mjs,
 * the stage's own pixels): the thumbnail's canvas area, cropped, against the export.
 */
function thumbnails(page, list, exported, cpuCheck) {
  return page.evaluate(async ({list, sizes, exported, cpuCheck}) => {
    const {ThumbnailRenderer} = await import('/src/thumbnail-renderer.mjs');
    const {createExportFrameRenderer} = await import('/src/engine/engine-export.mjs');
    const exportFrame = async (tick, width, height) => {
      const {editor, state, playback} = window.styk;
      const options = {width, height, smoothing: 'edges', baseURL: location.origin, signal: new AbortController().signal, getRevision: () => `${editor.revision}:${editor.frameDraftRevision}`};
      const renderer = createExportFrameRenderer(options, {stage: state.renderer, playback, revision: editor.revision, range: [tick, tick]});
      try {
        await renderer.load(editor.report);
        if (renderer.stats.renderer !== 'engine') return null;
        const output = await renderer.renderNativeTick(tick, {preserveAlpha: true});
        const png = await __preview.blobPNG(output.blob);
        return Object.assign(new String(png), {cpu: /CPU/.test(renderer.stats.engine?.backend ?? '')});
      } finally { renderer.dispose(); }
    };
    /** `run()` with the CPU rasterizer asked for (engineCpuForced). */
    const onCpu = async run => {
      localStorage.setItem('stykanimate.engine.cpu', 'on');
      try { return await run(); } finally { localStorage.removeItem('stykanimate.engine.cpu'); }
    };
    const engine = styk.thumbnails, legacy = new ThumbnailRenderer(styk.editor), rows = styk.editor.timeline, steps = [];
    const draw = async (renderer, index, canvas, tick) => {
      for (let attempt = 0; attempt < 40; attempt++) {
        if (await renderer.renderThumbnail(index, canvas, {tick})) return true;
        await new Promise(resolve => setTimeout(resolve, 100));
      }
      return false;
    };
    try {
      for (const tick of list) for (const [width, height] of sizes) {
        const index = Math.max(0, rows.findLastIndex(row => row.start <= tick));
        const a = __preview.canvas(width, height), b = __preview.canvas(width, height);
        const before = engine.stats.engine?.renders ?? 0, cpuBefore = engine.stats.engine?.cpu ?? 0, t0 = performance.now();
        const drawn = await draw(engine, index, a, tick);
        const engineMs = performance.now() - t0;
        const byEngine = (engine.stats.engine?.renders ?? 0) > before;
        const byGpu = byEngine && (engine.stats.engine?.cpu ?? 0) === cpuBefore;
        const refusal = byEngine ? null : engine.stats.engine?.refusal ?? 'the app has no engine thumbnails';
        const legacyDrawn = await draw(legacy, index, b, tick);
        steps.push({name: `thumb-${tick}-${width}x${height}`, tick, width, height, drawn, legacyDrawn, byEngine, refusal, engine: __preview.png(a), legacy: __preview.png(b)});
        if (cpuCheck && byEngine) {
          const c = __preview.canvas(width, height), cpu = engine.stats.engine?.cpu ?? 0, t1 = performance.now();
          await onCpu(() => draw(engine, index, c, tick));
          const cpuMs = performance.now() - t1;
          const byCpu = (engine.stats.engine?.cpu ?? 0) > cpu;
          // The thumbnail's time as the filmstrip sees it (the request until it is on the card), GPU then CPU.
          steps.push({name: `thumb-${tick}-${width}x${height}-cpu`, tick, width, height, byEngine: byCpu && byGpu, against: 'gpu', gpuMs: engineMs, cpuMs,
            refusal: !byGpu ? 'the first thumbnail was not the GPU\'s' : byCpu ? null : engine.stats.engine?.refusal, engine: __preview.png(c), legacy: __preview.png(a)});
        }
        if (exported && byEngine) {
          // The card's canvas area (styk_engine::thumbnails::card): fitted, centred.
          const {canvas_width: cw, canvas_height: ch} = styk.editor.report, scale = Math.min(width / cw, height / ch);
          const w = cw * scale, h = ch * scale, x = (width - w) / 2, y = (height - h) / 2;
          if ([w, h, x, y].every(Number.isInteger)) {
            const frame = await exportFrame(tick, w, h);
            const crop = __preview.canvas(w, h);
            crop.getContext('2d').drawImage(a, x, y, w, h, 0, 0, w, h);
            if (frame) steps.push({name: `thumb-${tick}-${width}x${height}-against-export`, tick, width, height, byEngine: true, against: 'export', engine: __preview.png(crop), legacy: String(frame)});
          }
        }
      }
      // An export frame of each tick at the canvas's size, from the GPU and from the CPU.
      if (cpuCheck) for (const tick of list) {
        const {canvas_width: cw, canvas_height: ch} = styk.editor.report;
        const t0 = performance.now(), gpu = await exportFrame(tick, cw, ch), t1 = performance.now();
        const cpu = gpu ? await onCpu(() => exportFrame(tick, cw, ch)) : null, t2 = performance.now();
        // Each includes starting its export worker, its device (GPU) and its first frame.
        steps.push({name: `export-${tick}-${cw}x${ch}-cpu`, tick, width: cw, height: ch, byEngine: Boolean(gpu && !gpu.cpu && cpu?.cpu), against: 'gpu', gpuMs: t1 - t0, cpuMs: t2 - t1,
          refusal: !gpu ? 'the engine did not export' : gpu.cpu ? 'the first export was not the GPU\'s' : cpu?.cpu ? null : 'the second export was not the CPU\'s', engine: cpu ? String(cpu) : null, legacy: gpu ? String(gpu) : null});
      }
    } finally { legacy.destroy(); }
    return steps;
  }, {list, sizes, exported, cpuCheck});
}

/** A library file's poster and hover frames from both paths. */
function library(page, name, base64, cpuCheck) {
  return page.evaluate(async ({name, base64, elapsed, cpuCheck}) => {
    const bytes = Uint8Array.from(atob(base64), c => c.charCodeAt(0));
    const file = new File([bytes], name);
    const previews = await import('/src/engine/engine-previews.mjs');
    const worker = () => new Worker('/src/library-preview-worker.mjs', {type: 'module'});
    const ask = (w, message) => new Promise((resolve, reject) => { w.onmessage = ({data}) => data.error ? reject(new Error(data.error)) : resolve(data); w.postMessage(message); });
    const steps = [];
    // With --cpu-check a worker that thumbnails started may draw on the CPU: the GPU's pictures come from a new one.
    if (cpuCheck) (await previews.enginePreviews())?.release();
    const enginePoster = await previews.engineLibraryPoster(file);
    const legacyWorker = worker();
    const legacyPoster = await ask(legacyWorker, {file, baseURL: location.href}).finally(() => legacyWorker.terminate());
    const gpuPoster = enginePoster ? await __preview.blobPNG(enginePoster) : null, gpuFrames = [];
    const firstStats = (await previews.enginePreviews())?.stats, gpuWorker = firstStats?.cpu === false;
    steps.push({name: 'poster', byEngine: Boolean(enginePoster), engine: gpuPoster, legacy: await __preview.blobPNG(legacyPoster.blob)});
    const animation = await previews.engineLibraryAnimation(file);
    const hover = worker();
    try {
      const first = await ask(hover, {operation: 'animation', file, baseURL: location.href});
      const legacyFrames = [first];
      for (const seconds of elapsed.slice(1)) legacyFrames.push(await ask(hover, {operation: 'frame', elapsed: seconds}));
      for (const [i, seconds] of elapsed.entries()) {
        const frame = animation ? await animation.frame(seconds) : null;
        const picture = frame ? await __preview.bitmapPNG(frame.bitmap, 320, 320) : null;
        gpuFrames.push(picture);
        steps.push({name: `hover-${seconds}s`, byEngine: Boolean(frame), engineTick: frame?.tick ?? null, legacyTick: legacyFrames[i].tick,
          engine: picture, legacy: await __preview.bitmapPNG(legacyFrames[i].bitmap, 320, 320)});
      }
      steps.push({name: 'matte', matte: true, engineMatte: animation?.matte ?? null, legacyMatte: first.matte});
    } finally { hover.terminate(); animation?.close(); }
    if (cpuCheck) {
      // The same pictures from a preview worker that draws on the CPU.
      (await previews.enginePreviews())?.release();
      localStorage.setItem('stykanimate.engine.cpu', 'on');
      try {
        const poster = await previews.engineLibraryPoster(file), worker = await previews.enginePreviews(), byCpu = Boolean(worker?.stats.cpu);
        const refusal = !gpuWorker ? `the first worker drew on the CPU too (${firstStats?.gpuRefusal ?? 'no worker'})` : byCpu ? null : 'the second worker did not draw on the CPU';
        steps.push({name: 'poster-cpu', against: 'gpu', byEngine: Boolean(poster && gpuPoster && byCpu && gpuWorker), refusal, engine: poster ? await __preview.blobPNG(poster) : null, legacy: gpuPoster});
        const animation = await previews.engineLibraryAnimation(file);
        try {
          for (const [i, seconds] of elapsed.entries()) {
            const frame = animation ? await animation.frame(seconds) : null;
            steps.push({name: `hover-${seconds}s-cpu`, against: 'gpu', byEngine: Boolean(frame && gpuFrames[i] && byCpu && gpuWorker), refusal, engine: frame ? await __preview.bitmapPNG(frame.bitmap, 320, 320) : null, legacy: gpuFrames[i]});
          }
        } finally { animation?.close(); }
      } finally {
        localStorage.removeItem('stykanimate.engine.cpu');
        (await previews.enginePreviews())?.release();
      }
    }
    return steps;
  }, {name, base64, elapsed, cpuCheck});
}

/** The camera monitor's canvas at each tick, and after playback. */
async function monitor(page, list) {
  await page.evaluate(async () => { await styk.actions['toggle-camera'](); });
  await page.waitForTimeout(400);
  const shots = [];
  const shoot = async name => {
    let previous = null, picture = null;
    for (let i = 0; i < 20; i++) {
      picture = await page.evaluate(async () => {
        for (let i = 0; i < 3; i++) await new Promise(resolve => requestAnimationFrame(resolve));
        const canvas = document.querySelector('[data-camera-monitor]');
        return canvas && canvas.width ? __preview.png(canvas) : null;
      });
      if (picture && picture === previous) break;
      previous = picture;
      await page.waitForTimeout(150);
    }
    // The engine drew the monitor: it drew the stage, and its device gave the monitor a canvas.
    const engine = await page.evaluate(() => Boolean(styk.state.renderer.engineActive && styk.state.renderer.engineSide?.monitor === true));
    shots.push({name, picture, engine});
  };
  for (const tick of list) {
    await page.evaluate(tick => styk.editor.seek(tick), tick);
    await shoot(`monitor-${tick}`);
  }
  return shots;
}

/**
 * One renderer's thumbnails of `list` at 320×192, created fresh (`kind`: `engine` or `javascript`), kept alive in the
 * page afterwards so the caller can sample memory; `dropThumbnails` destroys it.
 */
function thumbnailTiming(page, {list, kind}) {
  return page.evaluate(async ({list, kind}) => {
    const [{ThumbnailRenderer}, {createThumbnailRenderer}] = await Promise.all([import('/src/thumbnail-renderer.mjs'), import('/src/engine/engine-thumbnails.mjs')]);
    const rows = styk.editor.timeline, canvas = __preview.canvas(320, 192);
    const median = values => { const sorted = [...values].sort((a, b) => a - b); return sorted.length ? sorted[Math.floor(sorted.length / 2)] : null; };
    const renderer = window.__timingRenderer = kind === 'engine'
      ? createThumbnailRenderer(styk.editor, {}, {stage: () => styk.state.renderer, playback: () => styk.playback})
      : new ThumbnailRenderer(styk.editor);
    const times = [], started = performance.now();
    let byEngine = 0;
    for (const tick of list) {
      const index = Math.max(0, rows.findLastIndex(row => row.start <= tick)), t0 = performance.now();
      const before = renderer.stats.engine?.renders ?? 0;
      for (let attempt = 0; attempt < 40 && !await renderer.renderThumbnail(index, canvas, {tick}); attempt++) await new Promise(resolve => setTimeout(resolve, 50));
      times.push(performance.now() - t0);
      if ((renderer.stats.engine?.renders ?? 0) > before) byEngine++;
    }
    return {first: times[0], rest: median(times.slice(1)), total: performance.now() - started, count: times.length, byEngine,
      heapMiB: performance.memory ? +(performance.memory.usedJSHeapSize / 1048576).toFixed(1) : null};
  }, {list, kind});
}
const dropThumbnails = page => page.evaluate(() => { window.__timingRenderer?.destroy(); window.__timingRenderer = null; });

/** Interleaved timing of library previews by both paths (`round` decides which goes first). */
function libraryTiming(page, {libraryFiles, round}) {
  return page.evaluate(async ({libraryFiles, round}) => {
    const previews = await import('/src/engine/engine-previews.mjs');
    const worker = await previews.enginePreviews();
    const median = values => { const sorted = [...values].sort((a, b) => a - b); return sorted.length ? sorted[Math.floor(sorted.length / 2)] : null; };
    const result = [];
    for (const {name, base64} of libraryFiles) {
      const file = new File([Uint8Array.from(atob(base64), c => c.charCodeAt(0))], name);
      const ask = (w, message) => new Promise((resolve, reject) => { w.onmessage = ({data}) => data.error ? reject(new Error(data.error)) : resolve(data); w.postMessage(message); });
      const engineSide = async () => {
        worker?.release();
        const t0 = performance.now(), poster = await previews.engineLibraryPoster(file), coldMs = performance.now() - t0;
        const t2 = performance.now(); await previews.engineLibraryPoster(file); const posterMs = performance.now() - t2;
        const t1 = performance.now(), animation = await previews.engineLibraryAnimation(file), beginMs = performance.now() - t1, frames = [];
        for (let i = 0; i < 12 && animation; i++) { const t = performance.now(), frame = await animation.frame(i / 24); frames.push(performance.now() - t); frame?.bitmap.close(); }
        animation?.close();
        return {coldMs, posterMs, beginMs, frameMs: median(frames), drawn: Boolean(poster && animation)};
      };
      const javascriptSide = async () => {
        const w0 = new Worker('/src/library-preview-worker.mjs', {type: 'module'});
        const t0 = performance.now(); await ask(w0, {file, baseURL: location.href}); const posterMs = performance.now() - t0; w0.terminate();
        const w = new Worker('/src/library-preview-worker.mjs', {type: 'module'}), frames = [];
        const t1 = performance.now(), first = await ask(w, {operation: 'animation', file, baseURL: location.href}), beginMs = performance.now() - t1; first.bitmap.close();
        for (let i = 1; i < 12; i++) { const t = performance.now(), frame = await ask(w, {operation: 'frame', elapsed: i / 24}); frames.push(performance.now() - t); frame.bitmap.close(); }
        w.terminate();
        return {posterMs, beginMs, frameMs: median(frames)};
      };
      const entry = {name};
      if (round % 2) { entry.javascript = await javascriptSide(); entry.engine = await engineSide(); }
      else { entry.engine = await engineSide(); entry.javascript = await javascriptSide(); }
      result.push(entry);
    }
    return result;
  }, {libraryFiles, round});
}

/** Memory of the desktop app: every process (Electron's metrics) and the GPU memory nvidia-smi attributes to it. */
async function memory(app) {
  if (!app.electron) return null;
  const metrics = await app.electron.evaluate(({app}) => app.getAppMetrics().map(m => ({pid: m.pid, kib: m.memory.workingSetSize})));
  const result = spawnSync('nvidia-smi', ['-q', '-x'], {encoding: 'utf8'});
  let gpu = null;
  if (result.status === 0) {
    const pids = new Set(metrics.map(m => m.pid));
    for (const [, pid, used] of result.stdout.matchAll(/<pid>(\d+)<\/pid>[\s\S]*?<used_memory>(\d+) MiB<\/used_memory>/g))
      if (pids.has(Number(pid))) gpu = (gpu ?? 0) + Number(used);
  }
  return {totalMiB: Math.round(metrics.reduce((sum, m) => sum + m.kib, 0) / 1024), gpuMiB: gpu};
}

// ---------------------------------------------------------------------------------------------------------------

const host = target === 'electron' ? {origin: null, stop: async () => {}} : await server();
const browser = await chromium.launch({channel: 'chrome', headless: true});
const report = {label, target, limits: LIMITS, documents: [], library: [], monitor: [], timing: [], problems: []};
let failed = 0;
const record = async (group, step, comparer) => {
  const entry = {name: step.name, ...Object.fromEntries(Object.entries(step).filter(([key]) => !['engine', 'legacy', 'picture'].includes(key)))};
  if (step.matte) { entry.pass = step.engineMatte === null || step.engineMatte === step.legacyMatte; if (!entry.pass) failed++; group.push(entry); console.log(`${entry.pass ? 'ok  ' : 'FAIL'} ${step.name}: engine ${step.engineMatte}, JavaScript ${step.legacyMatte}`); return; }
  if (!step.engine || !step.legacy) { entry.pass = null; group.push(entry); console.log(`--   ${step.name}: ${step.engine ? '' : 'no engine picture'} ${step.legacy ? '' : 'no JavaScript picture'}`); return; }
  const result = await comparer(step.legacy, step.engine);
  if (result.picture) await writeFile(path.join(out, `${group.prefix}${step.name}-diff.png`), Buffer.from(result.picture, 'base64'));
  await writeFile(path.join(out, `${group.prefix}${step.name}-engine.png`), Buffer.from(step.engine, 'base64'));
  await writeFile(path.join(out, `${group.prefix}${step.name}-legacy.png`), Buffer.from(step.legacy, 'base64'));
  delete result.picture;
  const pass = !result.error && result.solid === 0 && result.mean <= LIMITS.mean;
  // A step the engine did not draw compares the JavaScript renderer with itself: reported, not counted.
  const counted = step.byEngine !== false;
  if (counted && !pass) failed++;
  Object.assign(entry, result, {pass: counted ? pass : null});
  group.push(entry);
  console.log(`${!counted ? '--  ' : pass ? 'ok  ' : 'FAIL'} ${group.prefix}${step.name}: ${step.byEngine === false ? `JavaScript (${step.refusal ?? 'refused'})` : 'engine'}; ${result.error ?? `${result.differing} px differ, ${result.solid} solid, max ${result.max}, mean beyond ${LIMITS.quantum} ${result.mean.toFixed(4)} (raw ${result.rawMean.toFixed(4)})`}`);
};
try {
  await mkdir(out, {recursive: true});
  const app = await open(browser, host.origin, true);
  await install(app.page);
  // Pictures are compared in a blank page of their own (the app's page may not decode them there).
  const blank = await (await browser.newContext()).newPage();
  await install(blank);
  const comparer = (a, b) => blank.evaluate(({a, b}) => __preview.compare(a, b), {a, b});
  const libraryFiles = await Promise.all(libraries.map(async file => ({name: path.basename(file), base64: (await readFile(path.resolve(file))).toString('base64')})));
  const documents = files.length ? files : (libraries.length ? [] : [null]);
  for (const file of documents) {
    await load(app.page, file);
    const total = await app.page.evaluate(() => styk.editor.totalTicks);
    const starts = await app.page.evaluate(() => styk.editor.timeline.map(row => row.start));
    const list = ticks ?? [...new Set([0, Math.floor(starts.length / 4), Math.floor(starts.length / 2), Math.floor(3 * starts.length / 4), starts.length - 1].map(i => starts[Math.max(0, i)]))].filter(tick => tick >= 0 && tick < total);
    const steps = Object.assign([], {prefix: `${file ? path.basename(file).replace(/[^\w.-]+/g, '-') : 'startup'}-`});
    for (const step of await thumbnails(app.page, list, flag('--export-check'), flag('--cpu-check'))) await record(steps, step, comparer);
    report.documents.push({file, ticks: list, steps: [...steps]});
    if (flag('--timing')) {
      const spread = starts.length <= 48 ? starts : starts.filter((_, i) => i % Math.ceil(starts.length / 48) === 0);
      for (let round = 0; round < rounds; round++) {
        const load0 = os.loadavg()[0], entry = {file, round};
        for (const kind of round % 2 ? ['javascript', 'engine'] : ['engine', 'javascript']) {
          const before = await memory(app);
          entry[kind] = await thumbnailTiming(app.page, {list: spread, kind});
          // The renderer still holds what it took (the JavaScript worker its copy of the document).
          entry[kind].memory = {before, after: await memory(app)};
          await dropThumbnails(app.page);
        }
        entry.load = [load0, os.loadavg()[0]];
        report.timing.push(entry);
        const line = kind => `${kind} first ${entry[kind].first.toFixed(1)} ms, rest ${entry[kind].rest?.toFixed(1)} ms, total ${entry[kind].total.toFixed(0)} ms` + (entry[kind].memory.before ? `, memory ${entry[kind].memory.before.totalMiB}→${entry[kind].memory.after.totalMiB} MiB, GPU ${entry[kind].memory.before.gpuMiB}→${entry[kind].memory.after.gpuMiB} MiB` : '');
        console.log(`timing ${file ?? 'startup'} round ${round}: ${line('engine')} (${entry.engine.byEngine}/${entry.engine.count} by the engine); ${line('javascript')}; load ${load0.toFixed(1)}–${os.loadavg()[0].toFixed(1)}`);
      }
    }
  }
  for (const {name, base64} of libraryFiles) {
    const steps = Object.assign([], {prefix: `${name.replace(/[^\w.-]+/g, '-')}-`});
    for (const step of await library(app.page, name, base64, flag('--cpu-check'))) await record(steps, step, comparer);
    report.library.push({file: name, steps: [...steps]});
  }
  if (flag('--timing') && libraryFiles.length) {
    for (let round = 0; round < rounds; round++) {
      const load0 = os.loadavg()[0];
      const before = await memory(app);
      const result = await libraryTiming(app.page, {libraryFiles, round});
      report.timing.push({library: true, round, entries: result, memory: {before, after: await memory(app)}, load: [load0, os.loadavg()[0]]});
      for (const entry of result) console.log(`timing ${entry.name} round ${round}: engine poster ${entry.engine.coldMs.toFixed(1)} ms with the worker's start, ${entry.engine.posterMs.toFixed(1)} ms after, hover start ${entry.engine.beginMs.toFixed(1)} ms, frame ${entry.engine.frameMs?.toFixed(1)} ms; JavaScript poster ${entry.javascript.posterMs.toFixed(1)} ms, hover start ${entry.javascript.beginMs.toFixed(1)} ms, frame ${entry.javascript.frameMs?.toFixed(1)} ms`);
    }
  }
  report.problems.push(...app.problems.map(problem => `engine: ${problem}`));
  await app.close();
  if (flag('--monitor')) {
    for (const file of documents) {
      const runs = [];
      // A fresh app for each run: the document is opened once in each.
      for (const engine of [true, false]) {
        const run = await open(browser, host.origin, engine);
        await install(run.page);
        await load(run.page, file);
        const starts = await run.page.evaluate(() => styk.editor.timeline.map(row => row.start));
        const list = ticks ?? [...new Set([0, starts[Math.floor(starts.length / 2)], starts.at(-1)])];
        runs.push(await monitor(run.page, list));
        report.problems.push(...run.problems.map(problem => `${engine ? 'engine' : 'JavaScript'} monitor: ${problem}`));
        await run.close();
      }
      const steps = Object.assign([], {prefix: `${file ? path.basename(file).replace(/[^\w.-]+/g, '-') : 'startup'}-`});
      for (const [i, shot] of runs[0].entries()) await record(steps, {name: shot.name, byEngine: shot.engine, refusal: shot.engine ? null : 'the engine did not draw the monitor', engine: shot.picture, legacy: runs[1][i]?.picture}, comparer);
      report.monitor.push({file, steps: [...steps]});
    }
  }
} finally {
  await writeFile(path.join(out, 'report.json'), JSON.stringify(report, null, 2) + '\n').catch(() => {});
  await browser.close();
  await host.stop();
}
for (const problem of report.problems.slice(0, 8)) console.log(`problem: ${problem}`);
console.log(`${failed} failing step(s); pictures and report in ${path.relative(root, out)}`);
process.exit(failed ? 1 : 0);
