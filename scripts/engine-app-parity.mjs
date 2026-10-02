#!/usr/bin/env node
// Compares what the Rust engine and the JavaScript renderer draw in the real app.
//
// The same document and the same steps run twice in Chromium, once with the
// engine and once with `?engine=off`; the stage is captured after every step
// and compared pixel by pixel. A step passes when the two pictures differ only
// along anti-aliased edges (see LIMITS). This is the app-level half of the parity
// harness in docs/RUST-ENGINE-PORT.md; geometry is compared by `styk compare`.
//
//   node scripts/engine-app-parity.mjs [--file <project>] [--ticks 0,10,20] [--onion] [--play]
//        [--target web|electron] [--renderer canvas|gpu] [--label <name>] [--url http://127.0.0.1:47831]
//        [--out <dir>] [--require-engine] [--width 1440 --height 900] [--zoom 1,0.3,4] [--no-effects] [--no-onion]
//
// Without --file it uses the document the app starts with. The web target runs
// headless Chromium, where the JavaScript renderer is its Canvas 2D fallback;
// without --url it starts the development server (scripts/serve.mjs) on a free
// port and stops it afterwards. The electron target runs the desktop app with a
// fresh profile, where the JavaScript renderer is WebGPU unless --renderer canvas. Output: artifacts/engine-rust/parity/app/<label>/ with both
// pictures, a difference picture and report.json per step. Exit code 1 when a
// step fails, 2 when --require-engine is given and the engine did not draw.
import {spawn} from 'node:child_process';
import {mkdir, readFile, writeFile} from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import {createRequire} from 'node:module';
import {mkdtemp, rm} from 'node:fs/promises';
import {chromium, _electron} from '../tests/support/playwright.mjs';

const root = path.resolve(import.meta.dirname, '..');
const args = process.argv.slice(2);
const option = (name, fallback) => { const at = args.indexOf(name); return at < 0 ? fallback : args[at + 1]; };
const flag = name => args.includes(name);
const file = option('--file'), label = option('--label', file ? path.basename(file).replace(/[^\w.-]+/g, '-').toLowerCase() : 'startup');
const out = path.resolve(option('--out', `artifacts/engine-rust/parity/app/${label}`));
const ticks = option('--ticks')?.split(',').map(Number) ?? null;
/** Zoom levels relative to "fit", each a set of steps (the app's own zoom action, 1.2× per step). */
const zooms = option('--zoom')?.split(',').map(Number) ?? [1];
const size = {width: Number(option('--width', 1440)), height: Number(option('--height', 900))};
const target = option('--target', 'web');
const SETTINGS = {theme: 'light', locale: 'en', uiScale: 100, showHandles: false};

/**
 * What counts as "the same picture". Both renderers cover edge pixels by
 * slightly different rules, so differences one or two pixels thick along
 * outlines are expected; and two correct 8-bit blends of the same translucent
 * layers may round differently by a level or two. A moved, missing or
 * recoloured shape differs over an area and by much more. A step fails when any
 * pixel differs by more than `threshold` levels together with all eight of its
 * neighbours (`solid` pixels), or when the mean difference over the stage,
 * not counting differences of `quantum` levels or less, exceeds `mean` levels
 * (a faint change over a large area). The raw mean is reported too.
 */
const LIMITS = {threshold: 32, mean: 0.25, quantum: 2};

async function server() {
  const given = option('--url');
  if (given) return {origin: given.replace(/\/$/, ''), stop: async () => {}};
  const port = 47000 + Math.floor(Math.random() * 900);
  const child = spawn(process.execPath, ['scripts/serve.mjs', '--port', String(port)], {cwd: root, stdio: 'ignore',
    env: {...process.env, STYK_PYTHON: process.env.STYK_PYTHON ?? path.join(root, '.venv/bin/python')}});
  const origin = `http://127.0.0.1:${port}`;
  for (let i = 0; i < 100; i++) {
    try { if ((await fetch(origin)).ok) return {origin, stop: async () => { child.kill(); }}; } catch { /* not listening yet */ }
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  child.kill();
  throw new Error('The development server did not start.');
}

/** The app in a browser page, with or without the engine. */
async function webApp(browser, origin, engine) {
  const context = await browser.newContext({viewport: size});
  await context.addInitScript(settings => localStorage.setItem('stykanimate.editor.settings.v1', JSON.stringify(settings)),
    {...SETTINGS, rendererMode: option('--renderer', 'canvas') === 'gpu' ? 'auto' : 'canvas'});
  const page = await context.newPage();
  await page.goto(`${origin}/${engine ? '' : '?engine=off'}`);
  return {page, close: () => context.close()};
}

/** The desktop app with a fresh profile, with or without the engine. */
async function desktopApp(engine) {
  const profile = await mkdtemp(path.join(os.tmpdir(), 'styk-parity-'));
  await writeFile(path.join(profile, 'editor-settings.json'), JSON.stringify({...SETTINGS, rendererMode: option('--renderer', 'gpu') === 'canvas' ? 'canvas' : 'auto'}));
  const env = {...process.env, STYK_STARTUP_VIEW: 'editor', STYK_USER_DATA_DIR: profile, STYK_LIBRARY_DIR: path.join(profile, 'library'),
    STYK_PYTHON: process.env.STYK_PYTHON ?? path.join(root, '.venv/bin/python')};
  delete env.ELECTRON_RUN_AS_NODE;
  const app = await _electron.launch({executablePath: createRequire(path.join(root, 'package.json'))('electron'), args: [root], cwd: root, env, timeout: 60000});
  const page = await app.firstWindow({timeout: 60000});
  await page.setViewportSize(size).catch(() => {});
  if (!engine) {
    await page.waitForFunction(() => window.styk?.ready === true, null, {timeout: 60000});
    await page.evaluate(() => localStorage.setItem('stykanimate.engine', 'off'));
    await page.reload();
  }
  return {page, close: async () => { await app.close().catch(() => {}); await rm(profile, {recursive: true, force: true}); }};
}

/** Opens the app, loads the document and returns the picture of the stage after every step. */
async function capture(browser, origin, engine) {
  const {page, close} = target === 'electron' ? await desktopApp(engine) : await webApp(browser, origin, engine);
  const problems = [];
  page.on('pageerror', error => problems.push(`pageerror ${error.message.slice(0, 300)}`));
  page.on('console', message => { if (message.type() === 'error') problems.push(message.text().slice(0, 300)); });
  await page.waitForFunction(() => window.styk?.ready === true, null, {timeout: 60000});
  // First-run prompts of the desktop app (data sharing, what's new) cover the stage.
  const dismiss = () => page.evaluate(() => { document.querySelector('.styk-privacy [data-decline]')?.click(); for (const dialog of document.querySelectorAll('dialog[open]')) dialog.close(); });
  await dismiss();
  if (file?.endsWith('.json')) {
    // A plain JSON document (the engine's test fixtures): loaded as the editor loads an opened project.
    const text = await readFile(path.resolve(file), 'utf8');
    await page.evaluate(({text, name}) => { styk.editor.load(JSON.parse(text), name); styk.editor.markSaved(); }, {text, name: path.basename(file)});
  } else if (file) {
    const before = await page.evaluate(() => { window.__parityReport = styk.editor.report; return document.title; });
    await page.locator('#file-input').setInputFiles(path.resolve(file));
    await page.waitForFunction(() => styk.editor.report !== window.__parityReport && !styk.state.busy, null, {timeout: 600000});
    if (await page.title() === before) throw new Error(`${file} did not open`);
  }
  await dismiss();
  const total = await page.evaluate(async () => {
    styk.editor.select(null); styk.fit();
    for (let i = 0; i < 4; i++) await new Promise(resolve => requestAnimationFrame(resolve));
    return styk.editor.totalTicks;
  });
  const steps = [];
  const stage = page.locator('#viewport');
  const settle = () => page.evaluate(async () => {
    // Images decode and frames prepare in the background: wait until two captures in a row agree.
    for (let i = 0; i < 6; i++) await new Promise(resolve => requestAnimationFrame(resolve));
    const renderer = styk.state.renderer;
    return {engine: Boolean(renderer.engineActive), refusal: renderer.engineStats?.refusal ?? null, tick: styk.editor.tick};
  });
  const shoot = async name => {
    let state = await settle(), picture = await stage.screenshot();
    for (let i = 0; i < 20; i++) {
      await page.waitForTimeout(150);
      state = await settle();
      const next = await stage.screenshot();
      if (next.equals(picture)) break;
      picture = next;
    }
    steps.push({name, picture, ...state});
  };
  const list = ticks ?? [...new Set([0, Math.floor(total / 4), Math.floor(total / 2), Math.floor(3 * total / 4), total - 1])].filter(tick => tick >= 0 && tick < total);
  // --no-effects: the effects preview off in both runs (what differs without effects is not theirs).
  if (flag('--no-effects')) await page.evaluate(() => { if (styk.state.vfxEnabled) document.getElementById('vfx-preview').click(); });
  // --no-onion: onion skins (on when the app starts) off in both runs.
  if (flag('--no-onion')) await page.evaluate(async () => { if (styk.state.onion) await styk.actions['toggle-onion'](); });
  for (const zoom of zooms) {
    // The zoom action multiplies the scale by 1.2 (zoom-out divides): the nearest whole number of steps.
    await page.evaluate(async zoom => {
      styk.fit();
      const steps = Math.round(Math.log(zoom) / Math.log(1.2));
      for (let i = 0; i < Math.abs(steps); i++) await styk.actions[steps > 0 ? 'zoom-in' : 'zoom-out']();
    }, zoom);
    for (const tick of list) {
      await page.evaluate(tick => styk.editor.seek(tick), tick);
      await shoot(zoom === 1 ? `tick-${tick}` : `zoom-${zoom}-tick-${tick}`);
    }
  }
  await page.evaluate(() => styk.fit());
  if (flag('--onion')) {
    await page.evaluate(async () => { if (!styk.state.onion) await styk.actions['toggle-onion'](); });
    for (const tick of list) {
      await page.evaluate(tick => styk.editor.seek(tick), tick);
      await shoot(`onion-${tick}`);
    }
    await page.evaluate(async () => { if (styk.state.onion) await styk.actions['toggle-onion'](); });
  }
  if (flag('--play')) {
    // Playback presents prepared frames: stop on a tick and capture what playback left on the stage.
    const stopAt = list[Math.min(1, list.length - 1)];
    const played = await page.evaluate(async stopAt => {
      const {state, editor, actions} = styk, frame = () => new Promise(resolve => requestAnimationFrame(resolve));
      state.loop = false; editor.seek(0); await frame();
      actions.play();
      const started = performance.now();
      while (performance.now() - started < 20000 && (!state.playing || editor.tick < stopAt)) await frame();
      const engine = Boolean(state.renderer.engineActive);
      if (state.playing) actions.play();
      await frame();
      editor.seek(stopAt);
      return {engine};
    }, stopAt);
    await shoot(`after-play-${stopAt}`);
    steps.at(-1).playedWithEngine = played.engine;
  }
  await close();
  return {steps, problems};
}

/** Runs in a page: per-pixel difference of two PNGs and a picture of where they differ. */
async function compare(page, a, b, limits) {
  return page.evaluate(async ({a, b, limits}) => {
    const decode = async data => createImageBitmap(await (await fetch(`data:image/png;base64,${data}`)).blob());
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
      // Faded picture with differences in red, stronger for larger ones.
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
  }, {a: a.toString('base64'), b: b.toString('base64'), limits});
}

const host = target === 'electron' ? {origin: null, stop: async () => {}} : await server();
const browser = await chromium.launch({channel: 'chrome', headless: true});
let failed = 0, engineDrew = true;
try {
  const withEngine = await capture(browser, host.origin, true);
  const legacy = await capture(browser, host.origin, false);
  await mkdir(out, {recursive: true});
  const page = await (await browser.newContext()).newPage();
  const report = {label, file: file ?? null, target, renderer: option('--renderer', target === 'electron' ? 'gpu' : 'canvas'), limits: LIMITS, steps: [], problems: {engine: withEngine.problems, legacy: legacy.problems}};
  if (legacy.steps.some(step => step.engine)) throw new Error('The comparison run without the engine was drawn by the engine: ?engine=off or the stored setting was ignored.');
  for (const [index, step] of withEngine.steps.entries()) {
    const other = legacy.steps[index];
    await writeFile(path.join(out, `${step.name}-engine.png`), step.picture);
    await writeFile(path.join(out, `${step.name}-legacy.png`), other.picture);
    const result = await compare(page, other.picture, step.picture, LIMITS);
    if (result.picture) await writeFile(path.join(out, `${step.name}-diff.png`), Buffer.from(result.picture, 'base64'));
    delete result.picture;
    const pass = !result.error && result.solid === 0 && result.mean <= LIMITS.mean;
    if (!pass) failed++;
    if (!step.engine) engineDrew = false;
    report.steps.push({name: step.name, engine: step.engine, refusal: step.refusal, playedWithEngine: step.playedWithEngine, pass, ...result});
    console.log(`${pass ? 'ok  ' : 'FAIL'} ${step.name}: engine ${step.engine ? 'drew' : `did not draw (${step.refusal})`}; ${result.error ?? `${result.differing} px differ, ${result.strong} by more than ${LIMITS.threshold}, ${result.solid} solid, max ${result.max}, mean beyond ${LIMITS.quantum} ${result.mean.toFixed(4)} (raw ${result.rawMean.toFixed(4)})`}`);
  }
  await writeFile(path.join(out, 'report.json'), JSON.stringify(report, null, 2) + '\n');
  for (const [name, problems] of Object.entries(report.problems)) if (problems.length) console.log(`${name} errors:\n  ${problems.slice(0, 5).join('\n  ')}`);
  console.log(`${withEngine.steps.length - failed} of ${withEngine.steps.length} steps match; pictures in ${path.relative(root, out)}`);
} finally {
  await browser.close();
  await host.stop();
}
process.exit(failed ? 1 : flag('--require-engine') && !engineDrew ? 2 : 0);
