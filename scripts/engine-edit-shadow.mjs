#!/usr/bin/env node
// Shadow editing in the real app (docs/engine/editing.md): opens the app with
// `?engineShadow=on`, edits a document through the interface (pointer drags on handles,
// keyboard undo and redo) and through the editor's timeline commands, and reports how
// the engine's answers compared with the JavaScript editor's (src/engine/engine-edit-shadow.mjs).
//
//   node scripts/engine-edit-shadow.mjs [--file <document.json> | --import <file.piv>] [--own] [--url http://127.0.0.1:47831] [--out <report.json>]
//
// Headless Chromium on the development server (started on a free port unless --url).
// Without --file it edits the document the app starts with. Exit code 1 when any call
// differed or nothing was compared.
//
// --exact-math: the browser's Math functions are replaced by the engine's (Node's V8
// results, docs/engine/decisions/0016-v8-exact-math.md) before the document loads, so the
// editor computes what Node computes and numbers must match exactly.
//
// --own: the engine owns editing (`?engineEdit=on`, the editor adopts what the engine's
// calls produce). The same interactions run first with the editor alone, then with the
// engine owning; the documents, history labels and selection after them, and after
// undoing everything, must be the same.
import {spawn} from 'node:child_process';
import {readFile, writeFile} from 'node:fs/promises';
import path from 'node:path';
import {chromium} from '../tests/support/playwright.mjs';

const root = path.resolve(import.meta.dirname, '..');
const args = process.argv.slice(2);
const option = (name, fallback) => { const at = args.indexOf(name); return at < 0 ? fallback : args[at + 1]; };
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));

async function server() {
  const given = option('--url');
  if (given) return {origin: given.replace(/\/$/, ''), stop: async () => {}};
  const port = 47000 + Math.floor(Math.random() * 900);
  const child = spawn(process.execPath, ['scripts/serve.mjs', '--port', String(port)], {cwd: root, stdio: 'ignore',
    env: {...process.env, STYK_PYTHON: process.env.STYK_PYTHON ?? path.join(root, '.venv/bin/python')}});
  const origin = `http://127.0.0.1:${port}`;
  for (let i = 0; i < 100; i++) {
    try { if ((await fetch(origin)).ok) return {origin, stop: async () => { child.kill(); }}; } catch { /* not listening yet */ }
    await sleep(100);
  }
  child.kill();
  throw new Error('The development server did not start.');
}

const {origin, stop} = await server();
const browser = await chromium.launch();
const own = args.includes('--own');
const exactMath = args.includes('--exact-math');
let failed = true;
/** One run of the interactions; `mode` is the query that sets up editing. */
async function run(mode) {
  const context = await browser.newContext({viewport: {width: 1440, height: 900}});
  await context.addInitScript(settings => localStorage.setItem('stykanimate.editor.settings.v1', JSON.stringify(settings)),
    {theme: 'light', locale: 'en', uiScale: 100, showHandles: true, rendererMode: 'canvas'});
  const page = await context.newPage();
  const problems = [];
  page.on('pageerror', error => problems.push(`pageerror ${error.message.slice(0, 300)}`));
  await page.goto(`${origin}/?${mode}`);
  await page.waitForFunction(() => window.styk?.ready === true, null, {timeout: 60000});
  await page.evaluate(() => { document.querySelector('.styk-privacy [data-decline]')?.click(); for (const dialog of document.querySelectorAll('dialog[open]')) dialog.close(); });
  if (exactMath) await page.evaluate(async () => {
    const m = await import('/src/engine/pkg/styk_engine.js');
    Object.assign(Math, {sin: m.math_sin, cos: m.math_cos, atan2: m.math_atan2, asin: m.math_asin, acos: m.math_acos,
      hypot: (...values) => values.length === 2 ? m.math_hypot(+values[0], +values[1]) : m.math_norm(Float64Array.from(values, Number))});
    globalThis.__stykExactMath = true;
  });
  const imported = option('--import');
  if (imported) {
    // A native file through the page's own import (the Pivot codec in WebAssembly).
    await page.setInputFiles('#file-input', path.resolve(imported));
    await page.waitForFunction(() => styk.editor.report.figure_types.length > 0 && !styk.state.importing && styk.editor.name !== undefined, null, {timeout: 180000});
    await page.waitForFunction(name => (styk.editor.name ?? '').length > 0 && !styk.state.importing, path.basename(imported), {timeout: 180000});
  }
  const file = option('--file');
  if (file) {
    const text = await readFile(path.resolve(file), 'utf8');
    await page.evaluate(({text, name}) => { styk.editor.load(JSON.parse(text), name); styk.editor.markSaved(); }, {text, name: path.basename(file)});
  }
  /** Waits until the engine holds the editor's current revision, so a session can start. */
  const settled = async () => {
    for (let i = 0; i < 100; i++) {
      const ready = await page.evaluate(async () => {
        await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
        // The editor alone has nothing to wait for.
        if (!globalThis.__stykEditShadow) return true;
        return Boolean(await globalThis.__stykEditShadow.prepare());
      });
      if (ready) return true;
      await sleep(100);
    }
    return false;
  };
  if (!await settled()) throw new Error(`The engine never held the document: ${await page.evaluate(() => JSON.stringify(styk.state.renderer?.engineStats ?? null))}`);

  /** Client coordinates of the handles of the figure with the most segments. */
  const handles = () => page.evaluate(async () => {
    const {editor, state} = styk;
    const {worldToScreen} = await import('/src/renderer.mjs');
    const {getPivotHandles} = await import('/src/canvas-interactions.mjs');
    const segments = new Map(editor.report.figure_types.map(figure => [figure.figure_type_id, figure.segments.length]));
    const figure = [...editor.evaluatedFrame.objects].sort((a, b) => (segments.get(b.figure_type_id) ?? 0) - (segments.get(a.figure_type_id) ?? 0))[0];
    editor.select(figure.unique_id, -1);
    await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    const rect = (document.querySelector('#overlay') ?? document.querySelector('#viewport')).getBoundingClientRect();
    const options = {report: editor.report, frame: editor.evaluatedFrame, view: state.view, cameraEnabled: false};
    const all = getPivotHandles(editor.geometry, {frame: editor.frame, selectedUid: editor.selectedUid, selectedUids: editor.selectedUids, showUnselected: state.settings.showUnselectedHandles, viewScale: state.view.scale, autoSize: true, maxDiameter: 10});
    const definition = editor.report.figure_types.find(type => type.figure_type_id === figure.figure_type_id)?.segments ?? [];
    const ik = joint => Boolean(figure._stykIK?.[definition[joint]?.segment_id]?.enabled);
    // The knee or elbow of a planted 3D limb: the bone its end hangs from.
    const knees = new Set(definition.flatMap(segment => {
      const c = figure._stykIK?.[segment.segment_id];
      if (!c?.enabled || !c.pinned || !c.spatialTarget || c.bendDirection !== 0) return [];
      const knee = definition.findIndex(other => other.segment_id === segment._stykRig?.spatial?.parent);
      return knee < 0 ? [] : [knee];
    }));
    return all.filter(handle => handle.uid === figure.unique_id && !handle.static).map(handle => { const at = worldToScreen(handle.point, options); return {joint: handle.joint, ik: handle.joint >= 0 && ik(handle.joint),
      knee: knees.has(handle.joint), spatial: Boolean(definition[handle.joint]?._stykRig?.spatial?.tip), x: rect.left + at.x, y: rect.top + at.y}; })
      .filter(handle => handle.x > rect.left + 60 && handle.x < rect.right - 60 && handle.y > rect.top + 60 && handle.y < rect.bottom - 60);
  });
  /** A pointer drag from a handle along a small arc, as a hand would do it. */
  const drag = async (from, dx, dy, button = 'left') => {
    await page.mouse.move(from.x, from.y); await sleep(60);
    await page.mouse.down({button}); await sleep(60);
    for (let i = 1; i <= 12; i++) { await page.mouse.move(from.x + dx * i / 12, from.y + dy * i / 12 + Math.sin(i / 2) * 6); await sleep(16); }
    await page.mouse.up({button}); await sleep(120);
  };
  const steps = [];
  const step = async (name, run) => { await settled(); await run(); steps.push(name); };

  const first = await handles();
  const joints = first.filter(handle => handle.joint >= 0), origin0 = first.find(handle => handle.joint < 0);
  if (joints[0]) await step('drag a joint handle', () => drag(joints[0], 40, -30));
  if (joints[1]) await step('drag another joint handle', async () => { const now = await handles(); const handle = now.find(h => h.joint === joints[1].joint) ?? joints[1]; await drag(handle, -30, 25); });
  if (origin0) await step('drag the figure by its origin', async () => { const now = await handles(); await drag(now.find(h => h.joint < 0) ?? origin0, 50, 20); });
  await step('undo and redo from the keyboard', async () => { await page.keyboard.press('Control+z'); await sleep(150); await page.keyboard.press('Control+z'); await sleep(150); await page.keyboard.press('Control+y'); await sleep(150); });
  await step('seek and transform through the editor', () => page.evaluate(() => { const e = styk.editor, last = Math.max(0, e.totalTicks - 1); e.seek(Math.min(3, last)); e.rotateSelection(0.25); e.scaleSelection(1.1); e.moveSelection(12, -8); e.undo(); e.redo(); }));
  await step('keys and frames of the layer timeline', () => page.evaluate(() => { const e = styk.editor, layer = [e.activeLayerId]; e.sheetInsertKey(layer, Math.min(2, e.totalTicks - 1)); e.sheetDuplicateFrameAfter(); e.sheetSetEasing(layer, 0, 1, {interpolation: 'ease-in'}); e.sheetInsertFrames(layer, 1, 2); e.sheetRemoveFrames(layer, 1, 1); e.undo(); e.undo(); e.redo(); }));
  await step('duplicate, join, unjoin and delete a figure', () => page.evaluate(() => { const e = styk.editor, original = e.selectedUid; const copy = e.duplicateFigure(); if (copy && original !== null) { e.joinTo(original, 0); e.unjoinFigure(); e.moveSelection(20, 20); e.deleteSelection(); e.undo(); e.undo(); } }));
  if (joints[0]) await step('drag a joint again after the commands', async () => { const now = await handles(); const handle = now.find(h => h.joint >= 0); if (handle) await drag(handle, 25, 35); });
  // A hand or foot with an IK controller, when the figure has one: its target is dragged.
  const target = (await handles()).find(handle => handle.ik);
  if (target) await step('drag an IK target', async () => { await settled(); const now = await handles(); await drag(now.find(h => h.joint === target.joint) ?? target, -30, 20); });
  // A figure with 3D bones: a planted leg's knee dragged (its bend plane swings), a bone
  // twisted with the right button (a planted leg's knee swings it instead), the rig reset.
  const knee = (await handles()).find(handle => handle.knee);
  if (knee) await step('drag a planted knee', async () => { const now = await handles(); await drag(now.find(h => h.joint === knee.joint) ?? knee, 25, -15); });
  const bone = (await handles()).find(handle => handle.knee) ?? (await handles()).find(handle => handle.spatial);
  if (bone) {
    await step('twist a 3D bone with the right button', async () => { const now = await handles(); await drag(now.find(h => h.joint === bone.joint) ?? bone, 40, 0, 'right'); });
    await step('reset the 3D rig', () => page.evaluate(() => styk.editor.resetSpatialPose(true)));
  }
  // A rig with pose choices: another choice picked, as the 3D panel's choice menu does.
  const choice = await page.evaluate(async () => {
    const {poseChoiceGroups} = await import('/src/rig-pose-choices.mjs');
    const e = styk.editor, group = poseChoiceGroups(e.figure, e.object)[0], other = group?.choices.find(c => c.id !== group.value);
    return other ? [group.name, other.id] : null;
  });
  if (choice) await step('choose a rig pose', () => page.evaluate(([name, id]) => styk.editor.setPoseChoice(name, id), choice));
  await settled();

  const report = await page.evaluate(() => JSON.parse(JSON.stringify(globalThis.__stykEditShadow ?? null, (key, value) => typeof value === 'function' ? undefined : value)));
  /** The document, history and selection as the editor holds them, then after undoing everything. */
  const outcome = await page.evaluate(() => {
    const e = styk.editor, runtime = (key, value) => ['_movieclipSample', 'runtime_segments'].includes(key) ? undefined : value;
    const capture = () => ({document: JSON.stringify(e.report, runtime), undo: e._undo.map(command => command.label), redo: e._redo.map(command => command.label),
      selection: {uid: e.selectedUid, uids: [...e.selectedUids], joint: e.selectedJoint, tick: e.tick}});
    const after = capture();
    while (e.canUndo) e.undo();
    return {after, undone: capture()};
  });
  await context.close();
  return {report, outcome, steps, problems};
}
try {
  if (own) {
    const alone = await run('engineShadow=off');
    const owned = await run('engineEdit=on');
    const {report, steps, problems} = owned;
    console.log(`steps: ${steps.join('; ')}`);
    console.log(`engine-owned calls: ${report.owned} adopted from the engine (${report.ownedMs.toFixed(1)} ms), ${report.compared} run in both and the same, ${report.mismatches.length} different, ${report.skipped} not shadowed; ${report.sessions} sessions`);
    for (const [why, count] of Object.entries(report.ended)) console.log(`  session ended ${count}×: ${why}`);
    for (const [what, count] of Object.entries(report.unported)) console.log(`  not ported ${count}×: ${what}`);
    for (const mismatch of report.mismatches.slice(0, 8)) console.log(`  ${mismatch.call}(${JSON.stringify(mismatch.args ?? []).slice(0, 80)}):\n    ${mismatch.problems.join('\n    ').slice(0, 1500)}`);
    // Numbers may differ in their last digits: the engine computes with Node's Math, the
    // browser's editor with the browser's (docs/engine/decisions/0016-v8-exact-math.md).
    let rounded = 0;
    const near = (x, y, at) => {
      if (typeof x === 'number' && typeof y === 'number') {
        if (x === y) return null;
        if (Math.abs(x - y) <= 1e-9 * Math.max(1, Math.abs(x), Math.abs(y))) { rounded++; return null; }
        return `${at}: ${x} against ${y}`;
      }
      if (x === null || y === null || typeof x !== 'object' || typeof y !== 'object') return x === y ? null : `${at}: ${JSON.stringify(x)?.slice(0, 200)} against ${JSON.stringify(y)?.slice(0, 200)}`;
      const keys = Object.keys(x), others = Object.keys(y);
      if (keys.length !== others.length || keys.some((key, i) => key !== others[i])) return `${at}: keys ${keys.join(',').slice(0, 200)} against ${others.join(',').slice(0, 200)}`;
      for (const key of keys) { const found = near(x[key], y[key], `${at}/${key}`); if (found) return found; }
      return null;
    };
    const differences = [];
    for (const phase of ['after', 'undone']) for (const key of ['document', 'undo', 'redo', 'selection']) {
      const [a, b] = [alone, owned].map(run => key === 'document' ? JSON.parse(run.outcome[phase][key]) : run.outcome[phase][key]);
      const found = near(a, b, `${phase} ${key}`);
      if (found) differences.push(found);
    }
    console.log(differences.length ? `DIFFERENT from the editor alone:\n  ${differences.join('\n  ')}`
      : `the same as the editor alone: document (${alone.outcome.after.document.length} bytes; ${rounded} numbers differ in their last digits), history (${alone.outcome.after.undo.length} entries), selection, and after undoing everything`);
    for (const problem of [...alone.problems, ...problems].slice(0, 5)) console.log(`  page: ${problem}`);
    const out = option('--out');
    if (out) await writeFile(path.resolve(out), JSON.stringify({steps, report, differences, problems}, null, 1));
    failed = differences.length > 0 || report.mismatches.length > 0 || report.owned === 0 || problems.length > 0;
  } else {
  const {report, steps, problems} = await run('engineShadow=on');
  if (!report) throw new Error('Shadow editing was not installed (is ?engineShadow=on honoured?)');
  const out = option('--out');
  if (out) await writeFile(path.resolve(out), JSON.stringify({steps, ...report, problems}, null, 1));
  console.log(`steps: ${steps.join('; ')}`);
  console.log(`shadowed calls: ${report.calls} run in the engine, ${report.compared} the same (${report.rounded} of them but for last digits of the browser's Math), ${report.mismatches.length} different, ${report.skipped} not shadowed; ${report.sessions} sessions`);
  console.log(`time in those calls: editor ${report.jsMs.toFixed(1)} ms, engine ${report.engineMs.toFixed(1)} ms`);
  for (const [why, count] of Object.entries(report.ended)) console.log(`  session ended ${count}×: ${why}`);
  for (const [what, count] of Object.entries(report.unported)) console.log(`  not ported ${count}×: ${what}`);
  for (const mismatch of report.mismatches.slice(0, 8)) console.log(`  ${mismatch.call}(${JSON.stringify(mismatch.args ?? []).slice(0, 80)}):\n    ${mismatch.problems.join('\n    ').slice(0, 1500)}`);
  for (const problem of problems.slice(0, 5)) console.log(`  page: ${problem}`);
  failed = report.mismatches.length > 0 || report.compared === 0;
  }
} finally {
  await browser.close();
  await stop();
}
process.exit(failed ? 1 : 0);
