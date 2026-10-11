#!/usr/bin/env node
// What the engine owning the document (D19) costs against the editor alone, on one file:
// the same editing workload (posing gestures, one-shot moves and rotations, undo and redo)
// runs on both, in Node with the real engine module, and every call is timed. Reports the
// totals C (owned) and J (editor alone), the calls N and the user's cost rule:
// pass when C <= 1.05 J or (C - J) / N <= 0.1 ms, with identical documents afterwards.
//
//   node scripts/engine-owner-cost.mjs <file.styk|file.json> [--rounds 3] [--app]
//
// `--app`: in the running app (Chromium), the file opened as users open it, with the
// editor alone (`?engineEdit=off`) against the engine owning (`?engineOwner=on`).
import {readFileSync} from 'node:fs';
import path from 'node:path';
import {decodeDocument} from '../src/native-container.mjs';
import {Editor} from '../src/editor.mjs';
import {installEngineOwner} from '../src/engine/engine-owner.mjs';
import {loadEngine} from '../src/engine/engine.mjs';
import {EngineMirror} from '../src/engine/engine-mirror.mjs';

const args = process.argv.slice(2);
const file = args.find(arg => !arg.startsWith('--'));
const rounds = Number(args[args.indexOf('--rounds') + 1]) || 3;
// `--capture <file>`: the owned run's session calls, for `styk edit-session <file>` natively.
const captureFile = args.includes('--capture') ? args[args.indexOf('--capture') + 1] : null;
if (!file) throw new Error('Name a document.');
if (args.includes('--app')) { await appCost(); process.exit(process.exitCode ?? 0); }
const bytes = readFileSync(file);
const decoded = file.endsWith('.json') ? JSON.parse(bytes.toString('utf8')) : await decodeDocument(bytes, path.extname(file).slice(1));
const source = decoded.document ?? decoded;
const options = {demo: false, propertyChannels: true, layerTimeline: true};
const json = value => JSON.stringify(value, (key, item) => key === 'runtime_segments' || key === '_movieclipSample' ? undefined : item);

const engine = await loadEngine({workers: 0});
const mirror = new EngineMirror(engine);

/** The workload: the first figures of the frame on show, posed, moved and rotated. */
function workload(editor, time) {
  const frame = editor.evaluatedFrame, figures = frame.objects.slice(0, 3);
  for (const figure of figures) {
    const joint = Math.min(2, (editor.report.figure_types.find(type => type.figure_type_id === figure.figure_type_id)?.segments.length ?? 1) - 1);
    time('select', () => editor.select(figure.unique_id, joint));
    time('beginDrag', () => editor.beginDrag('Pose figure'));
    for (let step = 0; step < 20; step++) time('rotateJoint', () => editor.rotateJoint(0.01));
    time('commitDrag', () => editor.commitDrag());
    time('select', () => editor.select(figure.unique_id, -1));
    time('moveSelection', () => editor.moveSelection(3, -2));
    time('rotateSelection', () => editor.rotateSelection(0.05));
  }
  for (let step = 0; step < 4; step++) time('undo', () => editor.undo());
  for (let step = 0; step < 4; step++) time('redo', () => editor.redo());
}

async function run(owned) {
  const editor = new Editor(options).load(structuredClone(source));
  let owner = null;
  if (owned) {
    mirror.reset();
    await mirror.syncFrames(editor.report, editor.revision, {allFigures: true, allFrames: true, allSources: true});
    const stateOf = e => JSON.stringify({tick: e.tick, frameIndex: e.frameIndex,
      selectedUid: e.selectedUid ?? null, selectedUids: [...e.selectedUids], selectedJoint: e.selectedJoint ?? -1, activeLayerId: e.activeLayerId,
      layerRevision: e.layerRevision, nextFrameIdentity: e._nextFrameIdentity, nextUid: e._nextUid, propertyChannels: true, layerTimeline: true});
    owner = installEngineOwner(editor, {start: e => process.env.OWNER_FROM_DOCUMENT ? engine.editSession(json(e.report), stateOf(e), e.revision)
      : mirror.framesCurrent(e.report, e.revision) ? engine.handle.edit_session(stateOf(e)) : null});
    if (!owner.ready()) throw new Error(`The engine did not take the document: ${owner.broken ?? 'not ready'}`);
    if (captureFile) owner.capture = [];
  }
  const calls = {}; let total = 0;
  const time = (name, call) => {
    const started = performance.now();
    call();
    const elapsed = performance.now() - started;
    total += elapsed; (calls[name] ??= {calls: 0, ms: 0}).calls++; calls[name].ms += elapsed;
  };
  const start = captureFile && owned ? {document: owner.session().report_json(), state: JSON.stringify({tick: editor.tick, frameIndex: editor.frameIndex,
    selectedUid: editor.selectedUid ?? null, selectedUids: [...editor.selectedUids], selectedJoint: editor.selectedJoint ?? -1, activeLayerId: editor.activeLayerId,
    layerRevision: editor.layerRevision, nextFrameIdentity: editor._nextFrameIdentity, nextUid: editor._nextUid, propertyChannels: true, layerTimeline: true})} : null;
  workload(editor, time);
  if (start) (await import('node:fs')).writeFileSync(captureFile, JSON.stringify({key: path.basename(file), ...start, calls: owner.capture}));
  const result = {total, calls, document: json(editor.report), native: owner?.native ?? 0, fallback: owner?.fallback ?? 0, refused: owner?.refused ?? {}, ms: owner?.ms};
  owner?.release();
  return result;
}

const results = {alone: [], owned: []};
for (let round = 0; round < rounds; round++) {
  // Alternate, so slow drifts in the machine's load spread over both.
  for (const owned of round % 2 ? [true, false] : [false, true]) results[owned ? 'owned' : 'alone'].push(await run(owned));
}
const median = values => values.slice().sort((a, b) => a - b)[values.length >> 1];
const J = median(results.alone.map(result => result.total)), C = median(results.owned.map(result => result.total));
const N = Object.values(results.alone[0].calls).reduce((sum, entry) => sum + entry.calls, 0);
const same = results.owned.every((result, index) => result.document === results.alone[index].document);
const pass = same && (C <= 1.05 * J || (C - J) / N <= 0.1);
console.log(`${path.basename(file)}: C ${C.toFixed(1)} ms, J ${J.toFixed(1)} ms over N ${N} calls (median of ${rounds}); `
  + `C/J ${(C / J).toFixed(3)}, excess ${((C - J) / N).toFixed(4)} ms/call; documents ${same ? 'identical' : 'DIFFERENT'}; ${pass ? 'pass' : 'FAIL'}`);
console.log(`  native ${results.owned[0].native}, editor ${results.owned[0].fallback}; refused ${JSON.stringify(results.owned[0].refused)}; owner ms ${JSON.stringify(Object.fromEntries(Object.entries(results.owned[0].ms ?? {}).map(([k, v]) => [k, typeof v === 'number' ? +v.toFixed(1) : Object.fromEntries(Object.entries(v).map(([m, x]) => [m, +x.toFixed(1)]))])))}`);
for (const name of Object.keys(results.alone[0].calls)) {
  const alone = median(results.alone.map(result => result.calls[name].ms)), owned = median(results.owned.map(result => result.calls[name]?.ms ?? 0));
  console.log(`  ${name}: ${results.alone[0].calls[name].calls} calls, owned ${owned.toFixed(1)} ms, alone ${alone.toFixed(1)} ms`);
}
process.exitCode = pass ? 0 : 1;

async function appCost() {
  const {spawn} = await import('node:child_process');
  const {chromium} = await import('playwright');
  const root = path.resolve(import.meta.dirname, '..');
  const port = 47800 + Math.floor(Math.random() * 90);
  const server = spawn(process.execPath, ['scripts/serve.mjs', '--port', String(port)], {cwd: root, stdio: 'ignore'});
  const origin = `http://127.0.0.1:${port}`;
  for (let i = 0; i < 100; i++) { try { if ((await fetch(origin)).ok) break; } catch {} await new Promise(resolve => setTimeout(resolve, 100)); }
  const browser = await chromium.launch();
  const runs = {alone: [], owned: []};
  try {
    for (let round = 0; round < rounds; round++) for (const owned of round % 2 ? [true, false] : [false, true]) {
      const context = await browser.newContext({viewport: {width: 1440, height: 900}});
      await context.addInitScript(() => localStorage.setItem('stykanimate.editor.settings.v1', JSON.stringify({theme: 'light', locale: 'en', rendererMode: 'canvas'})));
      const page = await context.newPage();
      await page.goto(`${origin}/?${owned ? 'engineOwner=on' : 'engineEdit=off'}`);
      await page.waitForFunction(() => window.styk?.ready === true, null, {timeout: 60000});
      await page.evaluate(() => { document.querySelector('.styk-privacy [data-decline]')?.click(); for (const dialog of document.querySelectorAll('dialog[open]')) dialog.close(); });
      await page.evaluate(() => { if (globalThis.__stykEngineOwner) globalThis.__stykEngineOwner.log = []; });
      await page.setInputFiles('#file-input', path.resolve(file));
      await page.waitForFunction(name => !styk.state.importing && styk.editor.name === name, path.basename(file).replace(/\.(piv|stk|styk)$/i, ''), {timeout: 180000});
      // The session starts once the stage has committed the opened document's frames.
      if (owned) await page.waitForFunction(() => Boolean(globalThis.__stykEngineOwner?.ready() && globalThis.__stykEngineOwner.session() && styk.editor.canUndo === false),
        null, {timeout: 180000, polling: 300});
      else await page.waitForTimeout(1500);
      const result = await page.evaluate(source => {
        const editor = styk.editor, calls = {}; let total = 0;
        globalThis.__stykEngineOwner?.log?.push(`before workload: session ${Boolean(globalThis.__stykEngineOwner.session())} at ${Math.round(performance.now())}`);
        const time = (name, call) => { const started = performance.now(); call(); const elapsed = performance.now() - started; total += elapsed; (calls[name] ??= {calls: 0, ms: 0}).calls++; calls[name].ms += elapsed; };
        (0, eval)(`(${source})`)(editor, time);
        const text = JSON.stringify(editor.report, (key, value) => key === 'runtime_segments' ? undefined : value);
        let hash = 0; for (let i = 0; i < text.length; i++) hash = (hash * 31 + text.charCodeAt(i)) | 0;
        const o = globalThis.__stykEngineOwner;
        const why = o ? {ms: o.ms, log: o.log?.filter(line => !/^(park false|start false|ensure: session false)/.test(line)).slice(-14), broken: o.broken ?? null, session: Boolean(o.session()), calls: o.calls, side: Boolean(styk.state?.renderer?.engineSide), current: styk.state?.renderer?.engineSide?.mirror?.framesCurrent(editor.report, editor.revision)} : null;
        return {why, total, calls, hash, native: globalThis.__stykEngineOwner?.native ?? 0, fallback: globalThis.__stykEngineOwner?.fallback ?? 0, refused: globalThis.__stykEngineOwner?.refused ?? {}};
      }, workload.toString());
      runs[owned ? 'owned' : 'alone'].push(result);
      await context.close();
    }
  } finally { await browser.close(); server.kill(); }
  const median = values => values.slice().sort((a, b) => a - b)[values.length >> 1];
  const J = median(runs.alone.map(run => run.total)), C = median(runs.owned.map(run => run.total));
  const N = Object.values(runs.alone[0].calls).reduce((sum, entry) => sum + entry.calls, 0);
  const same = runs.owned.every((run, index) => run.hash === runs.alone[index].hash);
  const pass = same && (C <= 1.05 * J || (C - J) / N <= 0.1);
  console.log(`${path.basename(file)} in the app: C ${C.toFixed(1)} ms, J ${J.toFixed(1)} ms over N ${N} calls (median of ${rounds}); C/J ${(C / J).toFixed(3)}, excess ${((C - J) / N).toFixed(4)} ms/call; documents ${same ? 'identical' : 'DIFFERENT'}; ${pass ? 'pass' : 'FAIL'}`);
  console.log(`  native ${runs.owned[0].native}, editor ${runs.owned[0].fallback}; refused ${JSON.stringify(runs.owned[0].refused)}; owner ${JSON.stringify(runs.owned[0].why)}`);
  for (const name of Object.keys(runs.alone[0].calls)) console.log(`  ${name}: ${runs.alone[0].calls[name].calls} calls, owned ${median(runs.owned.map(run => run.calls[name]?.ms ?? 0)).toFixed(1)} ms, alone ${median(runs.alone.map(run => run.calls[name].ms)).toFixed(1)} ms`);
  process.exitCode = pass ? 0 : 1;
}
