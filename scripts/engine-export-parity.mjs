#!/usr/bin/env node
// Export frames drawn by the engine against the JavaScript export renderer and the engine's stage, in the desktop app.
//
//   node scripts/engine-export-parity.mjs [--file <project>] [--ticks 0,30,60] [--size 1280x720 | --size 1280]
//        [--smoothing edges,none,sprites] [--timing <first>-<last>] [--timing-size 1920x1080]
//        [--timing-smoothing edges] [--rounds 2] [--repaint]
//        [--label <name>] [--out <dir>]
//        [--electron-arg <switch>]... [--cancel] [--formats png,png-sequence,gif,apng,mp4,webm]
//        [--format-rounds 1] [--integrity]
//
// One run of the desktop app (under xvfb-run on Linux) with a fresh profile; without --file the document the app
// starts with. For each sampling choice and tick, the same frame three ways, each as the lossless PNG the APNG,
// PNG-sequence and native video encoders receive:
//   engine      the engine's export renderer (src/engine/engine-export.mjs, a worker with its own GPU device);
//   javascript  the JavaScript export renderer (src/export-frame-renderer.mjs, Canvas 2D in a worker), today's exports;
//   stage       the engine's stage drawing the prepared frame through the scene camera at the export's size
//               (Smooth edges only: the stage has no other sampling).
// Differences per pair: mean |Δ| (largest channel, alpha included), the share of pixels beyond 8 and 32 levels, and the
// app-parity rule D13 (solid pixels beyond 32 with all eight neighbours, mean not counting differences of 2 or less).
// --timing renders that range with each renderer in turn (--rounds times, order alternating), sampling the memory of
// every process of the app, the engine's WebAssembly memory and the GPU memory nvidia-smi attributes to the app;
// with --repaint an element of the page repaints meanwhile, as the export dialog's progress does (a browser then
// hands GPU read-backs over later).
// --cancel checks cancellation and disposal: exports aborted mid-frame, workers and engine memory afterwards.
// --integrity runs the app's PNG-sequence export with the engine over an existing file twice, cancelled from the
// progress dialog and failing in the engine's worker (a frame outside the animation), and checks the file is untouched,
// no export worker is left and the engine's memory does not grow.
// --formats png,png-sequence,gif,apng,mp4,webm runs the app's own export commands (today's defaults: the canvas size,
// the whole animation; png: the frame the editor shows) once with each renderer (--format-rounds times, alternating), writing <format>-engine.<ext> and <format>-javascript.<ext> with their times
// and memory peaks; scripts/engine-export-compare.py decodes and compares each pair.
// Output: artifacts/engine-rust/parity/export/<label>/ (pictures, report.json). Exit code 1 when a renderer failed.
import {spawnSync} from 'node:child_process';
import {createHash} from 'node:crypto';
import {mkdir, mkdtemp, readFile, rm, writeFile} from 'node:fs/promises';
import {createRequire} from 'node:module';
import os from 'node:os';
import path from 'node:path';
import zlib from 'node:zlib';
import {_electron} from '../tests/support/playwright.mjs';

const root = path.resolve(import.meta.dirname, '..');
const args = process.argv.slice(2);
const option = (name, fallback) => { const at = args.lastIndexOf(name); return at < 0 ? fallback : args[at + 1]; };
const options = name => args.flatMap((value, index) => args[index - 1] === name ? [value] : []);
const file = option('--file');
const label = option('--label', file ? path.basename(file).replace(/[^\w.-]+/g, '-').toLowerCase() : 'startup');
const out = path.resolve(option('--out', `artifacts/engine-rust/parity/export/${label}`));
const ticks = option('--ticks', '0').split(',').map(Number);
// A size without a height (`--size 1280`) keeps the document's canvas shape.
let [width, height] = option('--size', '1280x720').split('x').map(Number);
const samplings = option('--smoothing', 'edges').split(',');
const timing = option('--timing')?.split('-').map(Number) ?? null;
let [timingWidth, timingHeight] = option('--timing-size', option('--size', '1280x720')).split('x').map(Number);
const timingSamplings = option('--timing-smoothing', 'edges').split(',');
const rounds = Number(option('--rounds', 2));
const SETTINGS = {theme: 'light', locale: 'en', uiScale: 100, showHandles: false, rendererMode: 'auto'};

// ---------------------------------------------------------------------------------------------------------------
// PNG in Node: the browser's encoder writes 8-bit RGBA, not interlaced.

function decodePNG(bytes) {
  let at = 8, w = 0, h = 0, channels = 4;
  const data = [];
  while (at < bytes.length) {
    const length = bytes.readUInt32BE(at), type = bytes.toString('latin1', at + 4, at + 8), body = bytes.subarray(at + 8, at + 8 + length);
    if (type === 'IHDR') {
      w = body.readUInt32BE(0); h = body.readUInt32BE(4);
      channels = {2: 3, 6: 4}[body[9]];
      if (body[8] !== 8 || !channels || body[12] !== 0) throw new Error(`unexpected PNG format (depth ${body[8]}, colour ${body[9]}, interlace ${body[12]})`);
    } else if (type === 'IDAT') data.push(body);
    at += 12 + length;
  }
  const raw = zlib.inflateSync(Buffer.concat(data)), stride = w * channels, lines = Buffer.alloc(stride * h);
  for (let y = 0; y < h; y++) {
    const filter = raw[y * (stride + 1)], line = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1));
    for (let x = 0; x < stride; x++) {
      const a = x >= channels ? lines[y * stride + x - channels] : 0, b = y ? lines[(y - 1) * stride + x] : 0, c = x >= channels && y ? lines[(y - 1) * stride + x - channels] : 0;
      const p = a + b - c, pa = Math.abs(p - a), pb = Math.abs(p - b), pc = Math.abs(p - c);
      const predictor = [0, a, b, (a + b) >> 1, pa <= pb && pa <= pc ? a : pb <= pc ? b : c][filter];
      lines[y * stride + x] = (line[x] + predictor) & 255;
    }
  }
  if (channels === 4) return {width: w, height: h, pixels: lines};
  const pixels = Buffer.alloc(w * h * 4, 255);
  for (let i = 0; i < w * h; i++) lines.copy(pixels, i * 4, i * 3, i * 3 + 3);
  return {width: w, height: h, pixels};
}

function encodePNG(w, h, pixels) {
  const crcTable = Array.from({length: 256}, (_, n) => { let c = n; for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1; return c >>> 0; });
  const crc = buffer => { let c = 0xffffffff; for (const byte of buffer) c = crcTable[(c ^ byte) & 255] ^ (c >>> 8); return (c ^ 0xffffffff) >>> 0; };
  const chunk = (type, body) => { const head = Buffer.alloc(8); head.writeUInt32BE(body.length); head.write(type, 4, 'latin1'); const tail = Buffer.alloc(4); tail.writeUInt32BE(crc(Buffer.concat([head.subarray(4), body]))); return Buffer.concat([head, body, tail]); };
  const header = Buffer.alloc(13); header.writeUInt32BE(w); header.writeUInt32BE(h, 4); header[8] = 8; header[9] = 6;
  const raw = Buffer.alloc((w * 4 + 1) * h);
  for (let y = 0; y < h; y++) pixels.copy(raw, y * (w * 4 + 1) + 1, y * w * 4, (y + 1) * w * 4);
  return Buffer.concat([Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]), chunk('IHDR', header), chunk('IDAT', zlib.deflateSync(raw)), chunk('IEND', Buffer.alloc(0))]);
}

/** Differences between two decoded frames, and a picture of them (grey: equal; orange beyond 8; red beyond 32). */
function compare(a, b) {
  const n = a.width * a.height, d = new Uint8Array(n), picture = Buffer.alloc(n * 4);
  let sum = 0, beyond8 = 0, beyond32 = 0, sumBeyond2 = 0, largest = 0;
  for (let i = 0; i < n; i++) {
    let v = 0;
    for (let c = 0; c < 4; c++) v = Math.max(v, Math.abs(a.pixels[i * 4 + c] - b.pixels[i * 4 + c]));
    d[i] = v; sum += v; largest = Math.max(largest, v);
    if (v > 2) sumBeyond2 += v;
    if (v > 8) beyond8++;
    if (v > 32) beyond32++;
    const grey = (a.pixels[i * 4] + a.pixels[i * 4 + 1] + a.pixels[i * 4 + 2]) / 12 * a.pixels[i * 4 + 3] / 255 + 160;
    const colour = v > 32 ? [255, 0, 64] : v > 8 ? [255, 170, 0] : [grey, grey, grey];
    picture.set([...colour, 255], i * 4);
  }
  let solid = 0;
  for (let y = 1; y < a.height - 1; y++) for (let x = 1; x < a.width - 1; x++) {
    let all = true;
    for (let k = 0; k < 9 && all; k++) all = d[(y + Math.floor(k / 3) - 1) * a.width + x + (k % 3) - 1] > 32;
    if (all) solid++;
  }
  const round = (v, digits = 3) => Number(v.toFixed(digits));
  return {mean: round(sum / n), largest, beyond8: round(beyond8 * 100 / n), beyond32: round(beyond32 * 100 / n), solid, d13Mean: round(sumBeyond2 / n, 4),
    d13: solid === 0 && sumBeyond2 / n <= 0.25, picture};
}

// ---------------------------------------------------------------------------------------------------------------
// Memory of the app: every process (Electron's metrics) and the GPU memory nvidia-smi attributes to them.

function gpuMiB(pids) {
  const result = spawnSync('nvidia-smi', ['-q', '-x'], {encoding: 'utf8'});
  if (result.status !== 0) return null;
  let total = 0, found = false;
  for (const [, pid, used] of result.stdout.matchAll(/<pid>(\d+)<\/pid>[\s\S]*?<used_memory>(\d+) MiB<\/used_memory>/g))
    if (pids.has(Number(pid))) { total += Number(used); found = true; }
  return found ? total : null;
}

class Sampler {
  constructor(app) { this.app = app; this.timer = null; }
  async read() {
    const metrics = await this.app.evaluate(({app}) => app.getAppMetrics().map(m => ({pid: m.pid, type: m.type, kib: m.memory.workingSetSize})));
    const total = metrics.reduce((s, m) => s + m.kib, 0), byType = {};
    for (const m of metrics) byType[m.type] = (byType[m.type] ?? 0) + m.kib;
    return {totalMiB: total / 1024, gpuProcessMiB: (byType.GPU ?? 0) / 1024, rendererMiB: (byType.Tab ?? 0) / 1024, pids: new Set(metrics.map(m => m.pid))};
  }
  start() {
    this.peak = {totalMiB: 0, gpuProcessMiB: 0, rendererMiB: 0, gpuMiB: 0};
    this.samples = 0;
    let tick = 0;
    const sample = async () => {
      try {
        const now = await this.read();
        for (const key of ['totalMiB', 'gpuProcessMiB', 'rendererMiB']) this.peak[key] = Math.max(this.peak[key], now[key]);
        if (tick++ % 3 === 0) { const gpu = gpuMiB(now.pids); if (gpu !== null) this.peak.gpuMiB = Math.max(this.peak.gpuMiB, gpu); }
        this.samples++;
      } catch { /* the app is busy; next sample */ }
      if (this.timer !== null) this.timer = setTimeout(sample, 150);
    };
    this.timer = setTimeout(sample, 0);
  }
  async stop() {
    clearTimeout(this.timer); this.timer = null;
    const round = v => Math.round(v);
    return Object.fromEntries(Object.entries(this.peak).map(([k, v]) => [k, round(v)]));
  }
}

// ---------------------------------------------------------------------------------------------------------------
// The app.

async function launch() {
  const profile = await mkdtemp(path.join(os.tmpdir(), 'styk-export-parity-'));
  await writeFile(path.join(profile, 'editor-settings.json'), JSON.stringify(SETTINGS));
  await writeFile(path.join(profile, 'telemetry-state.json'), JSON.stringify({decided: true, consent: {analytics: false, diagnostics: false, policyVersion: '2026-09-10', updatedAt: Date.now()}}));
  const env = {...process.env, STYK_STARTUP_VIEW: 'editor', STYK_USER_DATA_DIR: profile, STYK_LIBRARY_DIR: path.join(profile, 'library')};
  delete env.ELECTRON_RUN_AS_NODE;
  const app = await _electron.launch({executablePath: createRequire(path.join(root, 'package.json'))('electron'), args: [...options('--electron-arg'), root], cwd: root, env, timeout: 60000});
  const page = await app.firstWindow({timeout: 60000});
  await page.setViewportSize({width: 1440, height: 900}).catch(() => {});
  return {app, page, close: async () => { await app.close().catch(() => {}); await rm(profile, {recursive: true, force: true}); }};
}

async function openDocument(page) {
  await page.waitForFunction(() => window.styk?.ready === true, null, {timeout: 60000});
  const dismiss = () => page.evaluate(() => { document.querySelector('.styk-privacy [data-decline]')?.click(); for (const dialog of document.querySelectorAll('dialog[open]')) dialog.close(); });
  await dismiss();
  if (file?.endsWith('.json')) {
    const text = await readFile(path.resolve(file), 'utf8');
    await page.evaluate(({text, name}) => { styk.editor.load(JSON.parse(text), name); styk.editor.markSaved(); }, {text, name: path.basename(file)});
  } else if (file) {
    await page.evaluate(() => { window.__exportParityReport = styk.editor.report; });
    await page.locator('#file-input').setInputFiles(path.resolve(file));
    await page.waitForFunction(() => styk.editor.report !== window.__exportParityReport && !styk.state.busy, null, {timeout: 600000});
  }
  await dismiss();
  return page.evaluate(async () => {
    styk.editor.select(null); styk.fit();
    for (let i = 0; i < 8; i++) await new Promise(resolve => requestAnimationFrame(resolve));
    const renderer = styk.state.renderer, report = styk.editor.report;
    const adapter = await navigator.gpu?.requestAdapter({powerPreference: 'high-performance'}).catch(() => null);
    const info = adapter?.info ?? {};
    return {gpu: adapter ? {vendor: info.vendor, architecture: info.architecture, description: info.description, fallback: Boolean(adapter.isFallbackAdapter ?? info.isFallbackAdapter)} : null,name: styk.editor.name, ticks: styk.editor.totalTicks, canvas: [report.canvas_width, report.canvas_height], engine: Boolean(renderer.engineSide),
      engineActive: Boolean(renderer.engineActive), refusal: renderer.engineStats?.refusal ?? null, backend: renderer.engineStats?.backend ?? null,
      threaded: Boolean(renderer.engineSide?.engine?.threaded)};
  });
}

/** Renders `ticks` with one renderer: PNG bytes (base64) when `keep`, times, the renderer's statistics. */
function renderFrames(page, {kind, width, height, smoothing, ticks, keep, abortAfter}) {
  return page.evaluate(async ({kind, width, height, smoothing, ticks, keep, abortAfter}) => {
    const {createExportFrameRenderer} = await import('/src/engine/engine-export.mjs');
    const {ExportFrameRenderer} = await import('/src/export-frame-renderer.mjs');
    const {editor, state, playback} = window.styk;
    const controller = new AbortController();
    const options = {width, height, smoothing, baseURL: location.origin, signal: controller.signal,
      getRevision: () => `${editor.revision}:${editor.frameDraftRevision}`};
    const started = performance.now();
    const renderer = kind === 'javascript' ? new ExportFrameRenderer(options)
      : createExportFrameRenderer(options, {stage: state.renderer, playback, revision: editor.revision, range: [ticks[0], ticks.at(-1)]});
    const frames = [];
    let error = null;
    try {
      await renderer.load(editor.report);
      const loadMs = performance.now() - started;
      for (const [index, tick] of ticks.entries()) {
        if (abortAfter !== undefined && index === abortAfter) setTimeout(() => controller.abort(), 0);
        const at = performance.now();
        const output = await renderer.renderNativeTick(tick, {preserveAlpha: true});
        const ms = performance.now() - at;
        let png = null;
        if (keep) {
          const bytes = new Uint8Array(await output.blob.arrayBuffer());
          let text = '';
          for (let i = 0; i < bytes.length; i += 32768) text += String.fromCharCode(...bytes.subarray(i, i + 32768));
          png = btoa(text);
        }
        frames.push({tick, ms, bytes: output.blob.size, png});
      }
      return {kind: renderer.stats.renderer ?? kind, loadMs, totalMs: performance.now() - started, frames, stats: JSON.parse(JSON.stringify(renderer.stats)), error};
    } catch (failure) {
      error = {name: failure.name, message: String(failure.message ?? failure)};
      return {kind: renderer.stats.renderer ?? kind, frames, stats: JSON.parse(JSON.stringify(renderer.stats ?? {})), error};
    } finally { renderer.dispose(); }
  }, {kind, width, height, smoothing, ticks, keep, abortAfter});
}

/**
 * The engine's stage drawing the prepared frame of `tick` through the scene camera at the export's size, as the
 * window shows it: the stage's canvas is given the export's size in device pixels, drawn, and photographed by the
 * browser (a WebGPU canvas cannot be read back by the page). Restored afterwards.
 */
async function stageFrame(page, {tick, width, height}) {
  const ready = await page.evaluate(async ({tick, width, height}) => {
    const {editor, state, playback} = window.styk;
    const side = state.renderer.engineSide;
    if (!await playback.exportDocument(editor.report, editor.revision)) return {error: 'the engine does not draw this document'};
    const surface = side.surface, dpr = window.devicePixelRatio || 1;
    window.__stageBefore = {width: surface.width, height: surface.height, style: surface.getAttribute('style'), parent: surface.parentNode, next: surface.nextSibling};
    surface.width = width; surface.height = height; side.stage.resize(width, height);
    // Above everything, one canvas pixel to a device pixel.
    document.body.append(surface);
    surface.setAttribute('style', `position: fixed; left: 0; top: 0; width: ${width / dpr}px; height: ${height / dpr}px; z-index: 2147483647; visibility: visible; background: transparent`);
    const draw = () => side.stage.render(tick, width / editor.report.canvas_width, 0, 0, true, true, true);
    // Pictures upload a few megabytes a frame: draw until every one is in.
    for (let i = 0; i < 60 && JSON.parse(side.engine.handle.stats()).imagesPending > 0; i++) { draw(); await new Promise(resolve => setTimeout(resolve, 30)); }
    draw();
    await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    return {ok: true};
  }, {tick, width, height});
  if (ready.error) return ready;
  let png;
  try {
    const dpr = await page.evaluate(() => window.devicePixelRatio || 1);
    png = await page.screenshot({clip: {x: 0, y: 0, width: width / dpr, height: height / dpr}, scale: 'device', animations: 'disabled'});
  }
  finally {
    await page.evaluate(() => {
      const side = styk.state.renderer.engineSide, surface = side.surface, before = window.__stageBefore;
      surface.width = before.width; surface.height = before.height; side.stage.resize(before.width, before.height);
      if (before.style === null) surface.removeAttribute('style'); else surface.setAttribute('style', before.style);
      before.parent.insertBefore(surface, before.next);
    });
  }
  return {png: png.toString('base64')};
}

const report = {file: file ?? '(the start document)', size: [width, height], date: new Date().toISOString(), load: os.loadavg(), frames: [], timing: [], cancel: null, problems: []};
await mkdir(out, {recursive: true});
const session = await launch();
const errors = [];
session.page.on('pageerror', error => errors.push(`pageerror ${error.message.slice(0, 300)}`));
session.page.on('console', message => { if (message.type() === 'error') errors.push(message.text().slice(0, 300)); });
try {
  report.document = await openDocument(session.page);
  const [cw, ch] = report.document.canvas;
  if (!height) height = Math.round(width * ch / cw);
  if (!timingHeight) timingHeight = Math.round(timingWidth * ch / cw);
  report.size = [width, height];
  console.error(`[export] ${report.document.name}: ${report.document.ticks} ticks, engine ${report.document.engineActive ? `drawing (${report.document.backend})` : `not drawing (${report.document.refusal})`}, GPU ${JSON.stringify(report.document.gpu)}`);
  for (const smoothing of samplings) {
    const runs = {};
    for (const kind of ['engine', 'javascript']) {
      runs[kind] = await renderFrames(session.page, {kind, width, height, smoothing, ticks, keep: true});
      if (runs[kind].error) report.problems.push(`${kind} ${smoothing}: ${runs[kind].error.message}`);
    }
    if (runs.engine.kind !== 'engine') report.problems.push(`${smoothing}: the engine did not draw (${runs.engine.stats.engineRefusal ?? runs.engine.kind})`);
    for (const [index, tick] of ticks.entries()) {
      const pictures = {};
      for (const kind of ['engine', 'javascript']) {
        const frame = runs[kind].frames[index];
        if (!frame?.png) continue;
        const bytes = Buffer.from(frame.png, 'base64');
        await writeFile(path.join(out, `${smoothing}-${tick}-${kind}.png`), bytes);
        pictures[kind] = decodePNG(bytes);
      }
      if (smoothing === 'edges' && report.document.engine) {
        const stage = await stageFrame(session.page, {tick, width, height});
        if (stage.png) {
          const bytes = Buffer.from(stage.png, 'base64');
          await writeFile(path.join(out, `${smoothing}-${tick}-stage.png`), bytes);
          pictures.stage = decodePNG(bytes);
        } else report.problems.push(`stage ${tick}: ${stage.error}`);
      }
      const row = {smoothing, tick, ms: {engine: runs.engine.frames[index]?.ms ?? null, javascript: runs.javascript.frames[index]?.ms ?? null}};
      for (const [a, b] of [['engine', 'javascript'], ['engine', 'stage'], ['javascript', 'stage']]) {
        if (!pictures[a] || !pictures[b]) continue;
        const {picture, ...numbers} = compare(pictures[a], pictures[b]);
        row[`${a}-${b}`] = numbers;
        await writeFile(path.join(out, `${smoothing}-${tick}-${a}-${b}-diff.png`), encodePNG(width, height, picture));
      }
      report.frames.push(row);
      console.log(JSON.stringify(row));
    }
    report[`stats-${smoothing}`] = {engine: runs.engine.stats, javascript: runs.javascript.stats};
  }
  if (timing) {
    const [first, last] = timing, range = Array.from({length: last - first + 1}, (_, i) => first + i);
    const sampler = new Sampler(session.app);
    const idle = await sampler.read();
    if (args.includes('--repaint')) await session.page.evaluate(() => {
      const style = document.createElement('style');
      style.textContent = '@keyframes export-parity-repaint { from { width: 10px } to { width: 400px } } #export-parity-repaint { position: fixed; top: 4px; left: 4px; height: 8px; background: #888; animation: export-parity-repaint 0.5s linear infinite alternate; z-index: 2147483647 }';
      const bar = Object.assign(document.createElement('div'), {id: 'export-parity-repaint'});
      document.head.append(style); document.body.append(bar);
    });
    report.idleMemory = {totalMiB: Math.round(idle.totalMiB), gpuProcessMiB: Math.round(idle.gpuProcessMiB), rendererMiB: Math.round(idle.rendererMiB), gpuMiB: gpuMiB(idle.pids)};
    for (let round = 0; round < rounds; round++) {
      for (const kind of round % 2 ? ['javascript', 'engine'] : ['engine', 'javascript']) {
        for (const smoothing of timingSamplings) {
          sampler.start();
          const load = os.loadavg()[0];
          const run = await renderFrames(session.page, {kind, width: timingWidth, height: timingHeight, smoothing, ticks: range, keep: false});
          const peak = await sampler.stop();
          const times = run.frames.map(f => f.ms).sort((a, b) => a - b);
          const engineMemory = await session.page.evaluate(() => styk.state.renderer.engineSide?.engine.memoryBytes ?? null);
          const row = {round, kind: run.kind, smoothing, size: `${timingWidth}x${timingHeight}`, frames: run.frames.length, loadMs: Math.round(run.loadMs ?? 0), totalMs: Math.round(run.totalMs ?? 0),
            p50: Number((times[Math.floor(times.length / 2)] ?? 0).toFixed(1)), p95: Number((times[Math.floor(times.length * 0.95)] ?? 0).toFixed(1)), max: Number((times.at(-1) ?? 0).toFixed(1)),
            peak, engineMemoryMiB: engineMemory && Math.round(engineMemory / 1048576), load: Number(load.toFixed(1)), error: run.error, backend: run.stats.engine?.backend ?? null, tiles: run.stats.engine?.tiles ?? null, start: run.stats.engine && {modules: run.stats.engine.modulesMs, message: run.stats.engine.startAt, join: run.stats.engine.joinMs, create: run.stats.engine.createMs, phases: run.stats.engine.startMs}, engineLoad: run.stats.engineLoad ?? null, firstFrameMs: Number((run.frames[0]?.ms ?? 0).toFixed(1)), lastFrame: {evaluateMs: run.stats.evaluateMs, readMs: run.stats.readMs, resolveMs: run.stats.resolveMs, pngMs: run.stats.pngMs, ahead: run.stats.ahead}};
          report.timing.push(row);
          console.log(JSON.stringify(row));
        }
      }
    }
  }
  const formats = option('--formats')?.split(',') ?? [];
  const EXTENSION = {png: 'png', 'png-sequence': 'zip', gif: 'gif', apng: 'png', mp4: 'mp4', webm: 'webm'};
  const ACTION = {png: 'export-png', 'png-sequence': 'export-png-sequence', gif: 'export-gif', apng: 'export-apng', mp4: 'export-mp4', webm: 'export-video'};
  report.formats = [];
  if (formats.length) await session.page.evaluate(async () => {
    // Keeps the statistics of the last export frame renderer the app used (it disposes of it).
    const {ExportFrameRenderer} = await import('/src/export-frame-renderer.mjs');
    const dispose = ExportFrameRenderer.prototype.dispose;
    ExportFrameRenderer.prototype.dispose = function () { if (!this.disposed) window.__lastExportStats = JSON.parse(JSON.stringify(this.stats)); return dispose.call(this); };
  });
  const formatRounds = Number(option('--format-rounds', 1));
  for (let round = 0; round < formatRounds; round++) for (const format of formats) {
    for (const kind of round % 2 ? ['javascript', 'engine'] : ['engine', 'javascript']) {
      const target = path.join(out, `${format}-${kind}.${EXTENSION[format]}`);
      await rm(target, {force: true});
      await session.app.evaluate(({dialog}, file) => { dialog.showSaveDialog = async () => ({canceled: false, filePath: file}); }, target);
      await session.page.evaluate(on => localStorage.setItem('stykanimate.engine.export', on ? 'on' : 'off'), kind === 'engine');
      const sampler = new Sampler(session.app);
      sampler.start();
      const started = Date.now(), load = os.loadavg()[0];
      let problem = null;
      try { await session.page.evaluate(id => window.styk.actions[id](), ACTION[format]); } catch (error) { problem = String(error.message ?? error); }
      const finished = Date.now();
      // The file is complete once its size stops changing (the save renames it into place).
      let size = -1;
      for (let stable = 0, deadline = Date.now() + 600000; stable < 3 && Date.now() < deadline;) {
        const now = await readFile(target).then(b => b.length, () => -1);
        stable = now === size && now > 0 ? stable + 1 : 0; size = now;
        await new Promise(resolve => setTimeout(resolve, 150));
      }
      const ms = finished - started;
      const peak = await sampler.stop();
      const stats = await session.page.evaluate(() => window.__lastExportStats ?? null);
      const row = {round, format, kind, file: path.relative(root, target), bytes: size, ms, peak, load: Number(load.toFixed(1)), problem,
        renderer: stats?.renderer ?? null, engineLoad: stats?.engineLoad ?? null, frames: stats?.renderedFrames ?? null, lastWorkerMs: stats?.workerMs ?? null,
        lastFrame: stats && {evaluateMs: stats.evaluateMs, readMs: stats.readMs, resolveMs: stats.resolveMs, pngMs: stats.pngMs, ahead: stats.ahead}};
      report.formats.push(row);
      console.log(JSON.stringify({format: row}));
    }
  }
  await session.page.evaluate(() => localStorage.removeItem('stykanimate.engine.export'));
  if (args.includes('--integrity')) {
    // Workers the page creates from now on, and whether the engine's export worker freed what it held.
    await session.page.evaluate(() => {
      if (window.__exportWorkers) return;
      const live = window.__exportWorkers = new Set();
      window.__exportReleased = 0;
      const Original = window.Worker;
      window.Worker = class extends Original {
        constructor(url, init) {
          super(url, init); this.__name = init?.name; live.add(this);
          this.addEventListener('message', ({data}) => { if (data?.released) window.__exportReleased++; });
        }
        terminate() { live.delete(this); return super.terminate(); }
        postMessage(message, transfer) {
          // A frame outside the animation, asked of the engine's worker when the page says so.
          if (window.__failRender && message?.operation === 'render' && this.__name?.includes('engine') && --window.__failRender === 0) message = {...message, tick: -1};
          return super.postMessage(message, transfer);
        }
      };
    });
    const target = path.join(out, 'integrity.zip'), previous = Buffer.from('the export written before');
    const hash = bytes => createHash('sha256').update(bytes).digest('hex');
    report.integrity = [];
    for (const how of ['cancel', 'failure']) {
      await writeFile(target, previous);
      await session.app.evaluate(({dialog}, file) => { dialog.showSaveDialog = async () => ({canceled: false, filePath: file}); }, target);
      const memoryBefore = await session.page.evaluate(() => styk.state.renderer.engineSide?.engine.memoryBytes ?? null);
      await session.page.evaluate(how => { localStorage.setItem('stykanimate.engine.export', 'on'); window.__failRender = how === 'failure' ? 3 : 0; }, how);
      const exporting = session.page.evaluate(() => window.styk.actions['export-png-sequence']().then(() => 'finished', error => `failed: ${error.message}`));
      if (how === 'cancel') {
        await session.page.waitForFunction(() => Number(document.querySelector('#export-rendered')?.textContent.split('/')[0].replace(/\D/g, '') || 0) >= 2, null, {timeout: 120000});
        await session.page.evaluate(() => document.querySelector('#cancel-export')?.click());
      }
      const outcome = await exporting;
      await new Promise(resolve => setTimeout(resolve, 6000));
      const after = await session.page.evaluate(() => ({workers: [...window.__exportWorkers].filter(w => /export/i.test(w.__name ?? '')).map(w => w.__name), released: window.__exportReleased,
        memory: styk.state.renderer.engineSide?.engine.memoryBytes ?? null, status: document.querySelector('#status')?.textContent?.trim() ?? null}));
      const bytes = await readFile(target);
      const row = {how, outcome, untouched: hash(bytes) === hash(previous), exportWorkersLeft: after.workers, released: after.released,
        engineMemoryBeforeMiB: memoryBefore && Math.round(memoryBefore / 1048576), engineMemoryAfterMiB: after.memory && Math.round(after.memory / 1048576), status: after.status};
      report.integrity.push(row);
      console.log(JSON.stringify({integrity: row}));
      // A failed export leaves its message open over the editor.
      await session.page.evaluate(() => { for (const dialog of document.querySelectorAll('dialog[open]')) dialog.close(); });
      if (!row.untouched || after.workers.length) report.problems.push(`integrity ${how}: ${JSON.stringify(row)}`);
    }
    await session.page.evaluate(() => { localStorage.removeItem('stykanimate.engine.export'); window.__failRender = 0; });
  }
  if (args.includes('--cancel')) {
    // Exports aborted after their first frame, ten times: workers end, the engine's memory does not grow each time.
    const watch = await session.page.evaluate(() => {
      const live = window.__exportWorkers = new Set();
      window.__exportReleased = 0;
      const Original = window.Worker;
      window.Worker = class extends Original {
        constructor(url, init) {
          super(url, init); this.__name = init?.name; live.add(this);
          // The engine's export worker says when it freed its memory and GPU device.
          this.addEventListener('message', ({data}) => { if (data?.released) window.__exportReleased++; });
        }
        terminate() { live.delete(this); return super.terminate(); }
      };
      return styk.state.renderer.engineSide?.engine.memoryBytes ?? null;
    });
    const results = [];
    const range = Array.from({length: 6}, (_, i) => Math.min(i, Math.max(0, report.document.ticks - 1)));
    for (let i = 0; i < 10; i++) results.push(await renderFrames(session.page, {kind: 'engine', width, height, smoothing: 'edges', ticks: range, keep: false, abortAfter: 1}));
    await new Promise(resolve => setTimeout(resolve, 6000));
    const after = await session.page.evaluate(() => ({workers: [...window.__exportWorkers].map(w => w.__name), released: window.__exportReleased, memory: styk.state.renderer.engineSide?.engine.memoryBytes ?? null}));
    report.cancel = {aborted: results.filter(r => r.error?.name === 'AbortError').length, framesBeforeAbort: results.map(r => r.frames.length),
      engineMemoryBeforeMiB: watch && Math.round(watch / 1048576), engineMemoryAfterMiB: after.memory && Math.round(after.memory / 1048576), workersLeft: after.workers, workersReleased: after.released};
    console.log(JSON.stringify({cancel: report.cancel}));
  }
} catch (error) {
  report.problems.push(String(error?.stack ?? error));
} finally {
  report.errors = errors;
  await session.close();
  await writeFile(path.join(out, 'report.json'), JSON.stringify(report, null, 2));
}
if (report.problems.length) { console.error(report.problems.join('\n')); process.exitCode = 1; }
console.error(`[export] report: ${path.relative(root, path.join(out, 'report.json'))}`);
