#!/usr/bin/env node
// Replays recorded command tapes (engine/tests/edit/<name>.scenario.json) in the running
// app twice: with the engine editing by default and with `?engineEdit=off`. After every
// step and after undoing everything, the document, history, selection and playhead must
// be identical; the report lists how many calls of each method the engine owned.
//
//   node scripts/engine-app-tape.mjs <name> [<name> …] [--require <method,method>] [--url <origin>]
//
// `--require` fails the run unless every named method was owned at least once.
import {spawn} from 'node:child_process';
import {readFile} from 'node:fs/promises';
import path from 'node:path';
import {chromium} from 'playwright';

const root = path.resolve(import.meta.dirname, '..');
const args = process.argv.slice(2);
const option = name => { const at = args.indexOf(name); return at >= 0 ? args[at + 1] : undefined; };
const names = args.filter((arg, index) => !arg.startsWith('--') && !['--require', '--url'].includes(args[index - 1]));
const required = (option('--require') ?? '').split(',').filter(Boolean);
if (!names.length) throw new Error('Name at least one recorded tape.');

async function server() {
  if (option('--url')) return {origin: option('--url'), stop() {}};
  const port = 47900 + Math.floor(Math.random() * 90);
  const child = spawn(process.execPath, ['scripts/serve.mjs', '--port', String(port)], {cwd: root, stdio: 'ignore'});
  for (let i = 0; i < 100; i++) {
    try { if ((await fetch(`http://127.0.0.1:${port}/`)).ok) return {origin: `http://127.0.0.1:${port}`, stop: () => child.kill()}; } catch {}
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  child.kill();
  throw new Error('The development server did not start.');
}

/** Replays one tape in a fresh page; returns a snapshot per step and the shadow report. */
async function replay(browser, origin, scenario, document, mode) {
  const context = await browser.newContext({viewport: {width: 1440, height: 900}});
  await context.addInitScript(settings => localStorage.setItem('stykanimate.editor.settings.v1', JSON.stringify(settings)),
    {theme: 'light', locale: 'en', uiScale: 100, showHandles: true, rendererMode: 'canvas', timelineMode: 'advanced', autoKey: Boolean(scenario.editor?.autoKey)});
  await context.addInitScript(() => { let next = 0; crypto.randomUUID = () => `00000000-0000-4000-8000-${String(++next).padStart(12, '0')}`; });
  const page = await context.newPage();
  const problems = [];
  page.on('pageerror', error => problems.push(error.message.slice(0, 300)));
  await page.goto(`${origin}/?${mode}`);
  await page.waitForFunction(() => window.styk?.ready === true, null, {timeout: 60000});
  await page.evaluate(() => { document.querySelector('.styk-privacy [data-decline]')?.click(); for (const dialog of document.querySelectorAll('dialog[open]')) dialog.close(); });
  // Node's Math, as the recordings were made with (D16).
  await page.evaluate(async () => {
    const m = await import('/src/engine/pkg/styk_engine.js');
    Object.assign(Math, {sin: m.math_sin, cos: m.math_cos, atan2: m.math_atan2, asin: m.math_asin, acos: m.math_acos, log: m.math_log,
      hypot: (...values) => values.length === 2 ? m.math_hypot(+values[0], +values[1]) : m.math_norm(Float64Array.from(values, Number))});
    globalThis.__stykExactMath = true;
  });
  await page.evaluate(({text, autoKey}) => {
    styk.editor.load(JSON.parse(text), 'tape'); styk.editor.markSaved(); styk.editor.autoKey = autoKey;
  }, {text: document, autoKey: Boolean(scenario.editor?.autoKey)});
  const settled = async () => {
    for (let i = 0; i < 100; i++) {
      if (await page.evaluate(async () => {
        await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
        return !globalThis.__stykEditShadow || Boolean(await globalThis.__stykEditShadow.prepare());
      })) return true;
    }
    return false;
  };
  const snapshot = () => page.evaluate(() => {
    const e = styk.editor, runtime = (key, value) => ['_movieclipSample', 'runtime_segments'].includes(key) ? undefined : value;
    return {document: JSON.stringify(e.report, runtime), frame: JSON.stringify(e.evaluatedFrame, runtime),
      selection: JSON.stringify({uid: e.selectedUid ?? null, uids: [...e.selectedUids], joint: e.selectedJoint ?? -1}),
      state: JSON.stringify({tick: e.tick, frameIndex: e.frameIndex, canUndo: e.canUndo, canRedo: e.canRedo, undoLabel: e.undoLabel,
        redoLabel: e.redoLabel, dirty: e.dirty, layerRevision: e.layerRevision, nextUid: e._nextUid, nextFrameIdentity: e._nextFrameIdentity})};
  });
  const steps = [];
  if (!await settled()) throw new Error(`${mode}: the engine was not ready after loading.`);
  for (const call of scenario.steps) {
    const outcome = await page.evaluate(call => {
      const decode = value => Array.isArray(value) ? value.map(decode) : value === null || typeof value !== 'object' ? value
        : Object.hasOwn(value, '@array') ? Object.assign(decode(value['@array']), Object.fromEntries(Object.entries(value).filter(([key]) => key !== '@array')))
        : Object.fromEntries(Object.entries(value).map(([key, item]) => [key, decode(item)]));
      const [method, ...rest] = call, e = styk.editor;
      try {
        if (method === '@batch') { e.movieclipBatch(rest[0], () => { for (const [inner, ...values] of rest[1]) e[inner.replace(/^@/, '')](...values.map(decode)); }); return null; }
        if (method === '@frame') return null;
        e[method.replace(/^@/, '')](...rest.map(decode)); return null;
      } catch (error) { return error.messageKey ?? error.message; }
    }, call);
    await settled();
    steps.push({call: call[0], error: outcome, ...await snapshot()});
  }
  // Undo everything.
  await page.evaluate(() => { let guard = 10000; while (styk.editor.canUndo && guard--) styk.editor.undo(); });
  await settled();
  steps.push({call: 'undo all', error: null, ...await snapshot()});
  const report = await page.evaluate(() => globalThis.__stykEditShadow ? JSON.parse(JSON.stringify({mode: globalThis.__stykEditShadow.mode,
    owned: globalThis.__stykEditShadow.owned, failedOwned: globalThis.__stykEditShadow.failedOwned, ended: globalThis.__stykEditShadow.ended,
    mismatches: globalThis.__stykEditShadow.mismatches.length, mismatchList: globalThis.__stykEditShadow.mismatches.slice(0, 3), unported: globalThis.__stykEditShadow.unported, byCall: globalThis.__stykEditShadow.byCall})) : null);
  await context.close();
  return {steps, report, problems};
}

const {origin, stop} = await server();
const browser = await chromium.launch();
let failed = false;
const ownedTotals = {};
try {
  for (const name of names) {
    const folder = path.join(root, 'engine/tests/edit');
    const scenario = JSON.parse(await readFile(path.join(folder, `${name}.scenario.json`), 'utf8'));
    const document = typeof scenario.document === 'string' ? await readFile(path.join(folder, scenario.document), 'utf8') : JSON.stringify(scenario.document);
    const alone = await replay(browser, origin, scenario, document, 'engineEdit=off');
    const engine = await replay(browser, origin, scenario, document, 'engineEditDefault');
    const differences = [];
    for (const [index, step] of alone.steps.entries()) {
      const other = engine.steps[index];
      for (const key of ['error', 'document', 'frame', 'selection', 'state'])
        if (step[key] !== other?.[key]) differences.push(`step ${index + 1} (${step.call}): ${key} differs`);
    }
    const problems = [...alone.problems, ...engine.problems];
    const owned = Object.fromEntries(Object.entries(engine.report?.byCall ?? {}).filter(([, entry]) => entry.owned > 0).map(([method, entry]) => [method, entry.owned]));
    for (const [method, count] of Object.entries(owned)) ownedTotals[method] = (ownedTotals[method] ?? 0) + count;
    const ok = !differences.length && !problems.length && !engine.report?.mismatches;
    if (!ok) failed = true;
    if (args.includes("--verbose")) console.log(JSON.stringify(engine.report).slice(0, 1500));
    console.log(`${ok ? 'same' : 'DIFFERENT'} ${name}: ${scenario.steps.length} steps, owned ${engine.report?.owned ?? 0}, refused or errors ${engine.report?.failedOwned ?? 0} (${Object.entries(owned).map(([m, n]) => `${m} ${n}`).join(', ') || 'none'})`
      + (Object.keys(engine.report?.ended ?? {}).length ? `; sessions ended: ${JSON.stringify(engine.report.ended)}` : ''));
    for (const line of [...differences.slice(0, 10), ...problems.slice(0, 5)]) console.log(`  ${line}`);
    if (!ok) console.log(`  mismatches ${engine.report?.mismatches} failedOwned ${engine.report?.failedOwned} ${JSON.stringify(engine.report?.mismatchList).slice(0, 1200)} ${JSON.stringify(engine.report?.ended)} ${JSON.stringify(engine.report?.unported)}`);
  }
  const missing = required.filter(method => !ownedTotals[method]);
  if (missing.length) { failed = true; console.log(`Never owned: ${missing.join(', ')}`); }
} finally {
  await browser.close();
  stop();
}
process.exitCode = failed ? 1 : 0;
