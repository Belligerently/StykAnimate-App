#!/usr/bin/env node
// Shadow editing in the real app (docs/engine/editing.md): opens the app with
// `?engineShadow=on`, edits a document through the interface (pointer drags on handles,
// keyboard undo and redo) and through the editor's timeline commands, and reports how
// the engine's answers compared with the JavaScript editor's (src/engine/engine-edit-shadow.mjs).
//
//   node scripts/engine-edit-shadow.mjs [--file <document.json> | --import <file.piv>] [--own [--default]] [--warm]
//        [--origin] [--origin-pose] [--origin-channels] [--camera-follow | --joined-motion | --bounds | --capacity | --origin-range | --origin-rig] [--vfx-remaining] [--selection-adoption] [--movieclip-unique | --layer-folders | --figure-clipboard-pastes] [--focused] [--url http://127.0.0.1:47831] [--out <report.json>]
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
// calls produce). The same interactions run first with the editor alone (shadowed, to time
// it), then with the engine owning; the documents, history labels and selection after
// them, and after undoing everything, must be the same. Prints what each call cost both.
// --own --default: the second run is the app's default (the engine owns editing for the
// document classes it edits by default, and stays out of the others).
// --own --default --figures-gate: only the first five ordinary interactions, proving
// default admission and core adoption; not cost certification. Use the full paired
// workload without --figures-gate to certify editing cost.
// --own --default --warm --exact-math --movieclip-unique: only Make unique at ticks
// 2 and 2.25 through its UI action, with each boundary and undo/redo compared.
// Uses the recorded clip-unique-sheet document unless --file is supplied. Its
// matched command cost is affected-command evidence, not full-class or idle speed.
// --own --default --warm --exact-math --layer-folders: the five ordinary folder
// commands, including the timeline Ungroup menu, with every boundary and full
// undo/redo compared. Uses the document named by layers-folders-sheet.scenario.json.
// Per-method clocks are diagnostic, not an additional class-cost gate.
// --own --default --warm --exact-math --figure-clipboard-pastes: the accepted
// 33-call clipboard tape, including event-time counters, saved clipboard identity,
// results, refusals and full undo. Paste timings are diagnostic, not a class gate.
// --warm: the interactions run once untimed before the timed round (warmed-up code).
// --capture <folder>: the first sessions that meet an unported call, a difference or another
// error are saved there (`?engineEditCapture=on`) as `<run>-<n>.capture.json`, which
// `styk edit-session <file>` replays natively (docs/engine/editing.md).
import {spawn} from 'node:child_process';
import {createHash} from 'node:crypto';
import {mkdir, readFile, writeFile} from 'node:fs/promises';
import path from 'node:path';
import {chromium} from '../tests/support/playwright.mjs';

const root = path.resolve(import.meta.dirname, '..');
const args = process.argv.slice(2);
const option = (name, fallback) => { const at = args.indexOf(name); return at < 0 ? fallback : args[at + 1]; };
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const movieclipUnique = args.includes('--movieclip-unique');
const layerFolders = args.includes('--layer-folders');
const figureClipboardPastes = args.includes('--figure-clipboard-pastes');
const folderMethods = ['createLayerFolder', 'renameLayerFolder', 'moveLayersToFolder', 'arrangeLayers', 'deleteLayerFolder'];
if (layerFolders && ['--own', '--default', '--warm', '--exact-math'].some(flag => !args.includes(flag)))
  throw new Error('--layer-folders requires --own --default --warm --exact-math.');
if (layerFolders && movieclipUnique) throw new Error('--layer-folders and --movieclip-unique are separate scoped proofs.');
if (movieclipUnique && ['--own', '--default', '--warm', '--exact-math'].some(flag => !args.includes(flag)))
  throw new Error('--movieclip-unique requires --own --default --warm --exact-math.');
if (figureClipboardPastes && (['--own', '--default', '--warm', '--exact-math'].some(flag => !args.includes(flag))
    || movieclipUnique || layerFolders || args.includes('--import')))
  throw new Error('--figure-clipboard-pastes requires --own --default --warm --exact-math and its separate recorded document.');
const clipboardBinding = figureClipboardPastes ? {
  scenario: 'engine/tests/edit/figure-clipboard-pastes-sheet.scenario.json',
  scenarioSha256: '91be2dfb3dc80accb4a2253dda7e59124a5e564624966566e0c156a0260d11b0',
  reference: 'engine/tests/edit/figure-clipboard-pastes-sheet.reference.json',
  referenceSha256: '00d926809974abbf8419bd8d6337c71bf88b9296fe3504d76f6238df2ca692f0',
  document: 'engine/tests/edit/sheet-movieclip.document.json',
  documentSha256: '4c9a0d124be313f0b5bf263282d07db1748db57a97f36b7dbea8e02454bc397d',
} : null;
let clipboardScenario, clipboardReference, clipboardDocument;
if (figureClipboardPastes) {
  for (const key of ['scenario', 'reference', 'document']) {
    const text = await readFile(path.join(root, clipboardBinding[key]), 'utf8');
    if (createHash('sha256').update(text).digest('hex') !== clipboardBinding[`${key}Sha256`])
      throw new Error(`The accepted clipboard ${key} changed.`);
    if (key === 'scenario') clipboardScenario = JSON.parse(text);
    if (key === 'reference') clipboardReference = JSON.parse(text);
    if (key === 'document') clipboardDocument = text;
  }
  if (clipboardScenario.steps.length !== 33 || clipboardReference.steps.length !== 34
      || clipboardReference.steps.some((step, index) => step.step !== index || !step.figureClipboardPastes
        || JSON.stringify(step.call) !== JSON.stringify(index ? clipboardScenario.steps[index - 1] : null)))
    throw new Error('The clipboard tape is missing its exact calls or counter observations.');
}

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
const captureFolder = option('--capture');
const requiredCalls = [
  ...(layerFolders ? folderMethods : []),
  ...(movieclipUnique ? ['makeMovieclipUnique'] : []),
  ...(figureClipboardPastes ? ['pasteSelection', 'duplicateSelection'] : []),
  ...(args.includes('--figures-gate') ? ['moveSelection', 'poseJoint', 'commitDrag'] : []),
  ...(args.includes('--selection-adoption') ? ['duplicateFigure', 'pasteSelection', 'duplicateSelection'] : []),
  ...(args.includes('--vfx-remaining') ? ['setVfxScope', 'applyVfxPreset', 'addVfxGroup', 'addVfxUserPreset'] : []),
  ...(args.includes('--origin-range') ? ['rotateFiguresInFrames', 'mirrorFiguresInFrames', 'scaleFiguresInFrames', 'resetPoseInFrames', 'centerFiguresInFrames', 'joinFiguresInFrames', 'detachFiguresInFrames'] : []),
  ...(args.includes('--origin-rig') ? ['beginRigDrag', 'editRigJoint', 'commitRigDrag', 'cancelRigDrag'] : []),
  ...(args.includes('--camera-follow') ? ['nudgeHandle', 'setCameraMotion', 'applyCameraToFigures'] : []),
  ...(args.includes('--joined-motion') ? ['beginMotionPathEdit', 'updateMotionPathEdit', 'commitMotionPathEdit', 'cancelMotionPathEdit'] : []),
  ...(args.includes('--bounds') ? ['centerCamera', 'alignFiguresInFrames'] : []),
  ...(args.includes('--capacity') ? ['addSegment'] : []),
  ...(args.includes('--origin') ? ['setOrigin'] : []),
  ...(args.includes('--origin-pose') ? ['poseJoint', 'rotateJoint', 'bendJoint', 'editHandlePosition', 'transformFigureHandle'] : []),
  ...(args.includes('--origin-channels') ? ['beginAnimationEdit', 'previewAnimationEdit', 'commitAnimationEdit', 'cancelAnimationEdit', 'addAnimationKey', 'addAnimationKeys'] : []),
];
const missingCalls = report => requiredCalls.filter(name => !(report?.comparedByCall?.[name] > 0));
/** Writes the sessions a run captured (`report.captures`) into `--capture`'s folder. */
async function saveCaptures(report, run) {
  if (!captureFolder || !report?.captures?.length) return;
  await mkdir(path.resolve(captureFolder), {recursive: true});
  for (const [at, capture] of report.captures.entries()) await writeFile(path.resolve(captureFolder, `${run}-${at}.capture.json`), JSON.stringify(capture));
  console.log(`  captured ${report.captures.length} sessions (${run}) in ${captureFolder}: ${report.captures.map(capture => capture.key).join('; ')}`);
  delete report.captures;
}
let failed = true;
/** One run of the interactions; `mode` is the query that sets up editing. */
async function run(mode) {
  const context = await browser.newContext({viewport: {width: 1440, height: 900}});
  await context.addInitScript(settings => localStorage.setItem('stykanimate.editor.settings.v1', JSON.stringify(settings)),
    {theme: 'light', locale: 'en', uiScale: 100, showHandles: true, rendererMode: 'canvas'});
  // Identities the editor draws (effect clips, camera clips) come out the same in every run,
  // so the runs with and without the engine owning editing can be compared.
  await context.addInitScript(() => { let next = 0; crypto.randomUUID = () => `00000000-0000-4000-8000-${String(++next).padStart(12, '0')}`; });
  const page = await context.newPage();
  const problems = [];
  page.on('pageerror', error => {
    problems.push(`pageerror ${error.message.slice(0, 300)}`);
    if (args.includes('--verbose')) console.error(String(error.stack ?? error.message).split('\n').slice(0, 8).join('\n'));
  });
  await page.goto(`${origin}/?${mode}${captureFolder ? '&engineEditCapture=on' : ''}`);
  await page.waitForFunction(() => window.styk?.ready === true, null, {timeout: 60000});
  await page.evaluate(() => { document.querySelector('.styk-privacy [data-decline]')?.click(); for (const dialog of document.querySelectorAll('dialog[open]')) dialog.close(); });
  if (exactMath) await page.evaluate(async () => {
    const m = await import('/src/engine/pkg/styk_engine.js');
    Object.assign(Math, {sin: m.math_sin, cos: m.math_cos, atan2: m.math_atan2, asin: m.math_asin, acos: m.math_acos, log: m.math_log,
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
  const folderScenario = layerFolders ? JSON.parse(await readFile(path.join(root, 'engine/tests/edit/layers-folders-sheet.scenario.json'), 'utf8')) : null;
  const file = option('--file', layerFolders ? path.join(root, 'engine/tests/edit', folderScenario.document)
    : movieclipUnique ? path.join(root, 'engine/tests/edit/clip-unique-sheet.document.json')
    : figureClipboardPastes ? path.join(root, clipboardBinding.document) : undefined);
  if (file) {
    const text = await readFile(path.resolve(file), 'utf8');
    if (figureClipboardPastes && text !== clipboardDocument) throw new Error('The clipboard proof must load its accepted document bytes.');
    await page.evaluate(({text, name}) => { styk.editor.load(JSON.parse(text), name); styk.editor.markSaved(); }, {text, name: path.basename(file)});
  }
  if (layerFolders) await page.evaluate(() => styk.settingsUI.commit({timelineMode: 'advanced'}));
  if (figureClipboardPastes) await page.evaluate(({text, name, options}) => {
    styk.settingsUI.commit({timelineMode: 'advanced', autoKey: options.autoKey ?? false});
    const e = styk.editor;
    if (!e.propertyChannels || !e.layerTimeline || e._selectionClipboard || e._figureClipboard)
      throw new Error('The clipboard proof needs the actual app options and an initially empty figure clipboard.');
    const saved = Object.fromEntries(['_selectionClipboard', '_figureClipboard'].map(key => [key, {present: Object.hasOwn(e, key), value: e[key]}]));
    // The recorder's existing depth-four projection, used only for results/events.
    const plain = (value, depth = 4) => {
      if (value === null || ['number', 'string', 'boolean'].includes(typeof value)) return value;
      if (value instanceof Set) return plain([...value], depth);
      if (typeof value !== 'object' || depth <= 0) return undefined;
      if (Array.isArray(value)) return value.map(item => plain(item, depth - 1)).filter(item => item !== undefined);
      return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, plain(item, depth - 1)]).filter(([, item]) => item !== undefined));
    };
    const clipboard = () => e._selectionClipboard ?? e._figureClipboard;
    const counter = () => clipboard()?.pastes ?? null;
    const snapshot = () => ({document: JSON.stringify(e.report, (key, value) => key === 'runtime_segments' ? undefined : value),
      frame: JSON.stringify(e.evaluatedFrame, (key, value) => key === 'runtime_segments' ? undefined : value),
      undo: JSON.stringify(e._undo), redo: JSON.stringify(e._redo), selection: e._selectionState(),
      state: {tick: e.tick, frameIndex: e.frameIndex, selectedUid: e.selectedUid ?? null, selectedUids: [...e.selectedUids], selectedJoint: e.selectedJoint ?? -1,
        canUndo: e.canUndo, canRedo: e.canRedo, undoLabel: e.undoLabel, redoLabel: e.redoLabel, dirty: e.dirty,
        layerRevision: e.layerRevision, nextFrameIdentity: e._nextFrameIdentity, nextUid: e._nextUid,
        ...Object.fromEntries(Object.entries(e._selectionState()).filter(([key]) => ['polygonSelection', 'segmentIds', 'branch'].includes(key)))},
      clipboardPastes: counter()});
    globalThis.__stykClipboardProof = {snapshot,
      reset() {
        // Loading retires the old Native session. Restore the actual initial
        // empty clipboards, which load deliberately preserves across documents.
        e.load(JSON.parse(text), name); e.autoKey = options.autoKey ?? false; e.markSaved();
        for (const [key, entry] of Object.entries(saved)) {
          if (entry.present) e[key] = entry.value; else delete e[key];
        }
        if (clipboard() || e.canUndo || e.canRedo) throw new Error('The warm clipboard baseline was not restored.');
      },
      invoke(call, step) {
        const method = call[0] === '@select' ? 'select' : call[0] === '@seek' ? 'seek' : call[0];
        const r = globalThis.__stykEditShadow;
        const beforeEntry = {...r.byCall?.[method]}, beforeOwned = r.owned, beforeFailed = r.failedOwned;
        const beforeCalls = r.calls, beforeClipboard = clipboard(), before = counter(), events = [], counters = [], eventClipboards = [];
        const observe = event => { events.push(plain(event.detail ?? null)); counters.push(counter()); eventClipboards.push(clipboard()); };
        let result = null, error = null;
        e.addEventListener('change', observe);
        try { result = plain(e[method](...call.slice(1)) ?? null); }
        catch (caught) { error = {message: caught.message, key: caught.messageKey ?? null}; }
        finally { e.removeEventListener('change', observe); }
        const entry = r.byCall?.[method] ?? {}, keys = ['calls', 'callMs', 'engineMs', 'adoptMs', 'editorMs', 'failedOwned', 'failedOwnedMs', 'answerBytes'];
        return {step, call, result, error, events,
          figureClipboardPastes: {before, after: counter(), events: counters, sameClipboard: clipboard() === beforeClipboard},
          retainedEventClipboardPastes: eventClipboards.map(value => value?.pastes ?? null),
          clock: {method, ...Object.fromEntries(keys.map(key => [key, (entry[key] ?? 0) - (beforeEntry[key] ?? 0)])),
            owned: r.owned - beforeOwned, globalFailedOwned: r.failedOwned - beforeFailed, globalCalls: r.calls - beforeCalls}, snapshot: snapshot()};
      }};
  }, {text: clipboardDocument, name: clipboardScenario.name, options: clipboardScenario.editor});
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
  const uniqueSnapshots = [];
  const uniqueCalls = [];
  const folderSnapshots = [];
  const folderCalls = [];
  const clipboardRows = [];
  // A step the editor refuses on this document (its message shown to users, compared in the
  // shadow like any other answer) is noted and the run goes on.
  const refused = [];
  const step = async (name, run) => {
    if (args.includes('--verbose')) console.log(`${mode}: ${name}`);
    const ready = await settled();
    if ((movieclipUnique || layerFolders || figureClipboardPastes) && !ready) throw new Error(`The engine was not ready before ${name}.`);
    try { await run(); steps.push(name); }
    catch (error) {
      if (movieclipUnique || layerFolders || figureClipboardPastes) throw error;
      const message = String(error?.message ?? error).replace(/^page\.evaluate: (Error: )?/, '').split('\n')[0]; refused.push(`${name}: ${message}`);
    }
  };

  /** The interactions, once. */
  const interactions = async () => {
    if (figureClipboardPastes) {
      await page.evaluate(() => globalThis.__stykClipboardProof.reset());
      if (!await settled()) throw new Error('The reloaded clipboard document was not ready.');
      clipboardRows.push(await page.evaluate(() => ({step: 0, call: null, result: null, error: null, events: [],
        figureClipboardPastes: {before: null, after: null, events: [], sameClipboard: true}, snapshot: globalThis.__stykClipboardProof.snapshot()})));
      for (const [index, call] of clipboardScenario.steps.entries()) {
        await step(`clipboard ${index + 1}: ${call[0]}`, async () => {
          clipboardRows.push(await page.evaluate(({call, step}) => globalThis.__stykClipboardProof.invoke(call, step), {call, step: index + 1}));
          if (!await settled()) throw new Error(`The clipboard call ${index + 1} did not become ready.`);
        });
      }
      return;
    }
    if (layerFolders) {
      const boundary = async (name, operation) => {
        await step(name, operation);
        if (!await settled()) throw new Error(`The engine was not ready after ${name}.`);
        folderSnapshots.push(await page.evaluate(name => {
          const e = styk.editor, runtime = (key, value) => ['_movieclipSample', 'runtime_segments'].includes(key) ? undefined : value;
          return {name, document: JSON.stringify(e.report, runtime), frame: JSON.stringify(e.evaluatedFrame, runtime),
            undo: JSON.stringify(e._undo, runtime), redo: JSON.stringify(e._redo, runtime),
            selection: e._selectionState(), tick: e.tick, frameIndex: e.frameIndex, activeLayerId: e.activeLayerId,
            counters: {layerRevision: e.layerRevision, definitionRevision: e.definitionRevision,
              nextFrameIdentity: e._nextFrameIdentity, nextUid: e._nextUid, revision: e.revision, revisionSerial: e._revisionSerial,
              historySize: e.historySize, dirty: e.dirty, canUndo: e.canUndo, canRedo: e.canRedo}};
        }, name));
      };
      await boundary('prepare the ordinary layer-folder sheet', () => page.evaluate(() => {
        const e = styk.editor;
        if (!e.layerTimeline || !e.report._styk?.layerSheet || e.frameDraft || e._drag || e._animationEdit
            || e.report._styk.movieclipSource || e.getLayerFolders().length || e.canUndo || e.report._styk.layers.order.length < 2)
          throw new Error('The folder proof needs the quiet ordinary recorded sheet with two layers and no folders.');
      }));
      const initial = folderSnapshots.at(-1);
      const ids = await page.evaluate(() => styk.editor.report._styk.layers.order.slice(0, 2));
      const invoke = async (method, operation, ui = false) => {
        const before = await page.evaluate(method => {
          const r = globalThis.__stykEditShadow, e = styk.editor;
          return {entry: {...r.byCall?.[method]}, owned: r.owned, failedOwned: r.failedOwned, calls: r.calls,
            quiet: !e._drag && !e._animationEdit,
            preludes: Object.fromEntries(['commitDrag', 'commitAnimationEdit'].map(name => [name, {...r.byCall?.[name]}]))};
        }, method);
        await boundary(`${ui ? 'timeline UI ' : ''}${method}`, operation);
        folderCalls.push(await page.evaluate(({method, before, ui}) => {
          const r = globalThis.__stykEditShadow, e = styk.editor, entry = r.byCall?.[method] ?? {};
          const keys = ['calls', 'callMs', 'engineMs', 'adoptMs', 'editorMs', 'failedOwned', 'failedOwnedMs'];
          const delta = (after, prior) => Object.fromEntries(keys.map(key => [key, (after[key] ?? 0) - (prior[key] ?? 0)]));
          return {method, ui, ...delta(entry, before.entry), owned: r.owned - before.owned,
            globalCalls: r.calls - before.calls, globalFailedOwned: r.failedOwned - before.failedOwned,
            quiet: {before: before.quiet, after: !e._drag && !e._animationEdit},
            preludes: Object.fromEntries(['commitDrag', 'commitAnimationEdit'].map(name => [name, delta(r.byCall?.[name] ?? {}, before.preludes[name])]))};
        }, {method, before, ui}));
      };
      await invoke('createLayerFolder', () => page.evaluate(ids => {
        const e = styk.editor, folder = e.createLayerFolder('Folder proof', [ids[0]]);
        if (!folder || folder.kind !== 'folder' || e.getLayer(ids[0]).folderId !== folder.id)
          throw new Error('Folder creation did not group the requested layer.');
      }, ids));
      const folderId = await page.evaluate(() => styk.editor.getLayerFolders()[0]?.id);
      if (!folderId) throw new Error('The created folder is missing.');
      await invoke('renameLayerFolder', () => page.evaluate(folderId => {
        const e = styk.editor;
        if (!e.renameLayerFolder(folderId, 'Renamed folder proof') || e.getLayerFolders().find(folder => folder.id === folderId)?.name !== 'Renamed folder proof')
          throw new Error('Folder rename did not publish its new name.');
      }, folderId));
      await invoke('moveLayersToFolder', () => page.evaluate(({ids, folderId}) => {
        const e = styk.editor;
        if (!e.moveLayersToFolder([ids[1]], folderId) || ids.some(id => e.getLayer(id).folderId !== folderId))
          throw new Error('Moving a layer into the folder did not preserve both memberships.');
      }, {ids, folderId}));
      await invoke('arrangeLayers', () => page.evaluate(ids => {
        const e = styk.editor, before = [...e.report._styk.layers.order];
        if (!e.arrangeLayers(before.toReversed()) || e.report._styk.layers.order.indexOf(ids[1]) >= e.report._styk.layers.order.indexOf(ids[0]))
          throw new Error('Arranging layers did not reverse the grouped layer order.');
      }, ids));
      // This is the existing Advanced timeline folder menu, as exercised by
      // tests/layer-folders.browser.mjs. Its original _run/quiet guards remain active.
      await invoke('deleteLayerFolder', async () => {
        await page.locator(`.sheet-layer[data-layer-id="${folderId}"]:visible`).click({button: 'right'});
        await page.getByRole('menuitem', {name: 'Ungroup folder', exact: true}).click();
        await page.waitForFunction(({folderId, ids}) => !styk.editor.getLayerFolders().some(folder => folder.id === folderId)
          && ids.every(id => styk.editor.getLayer(id).folderId === undefined), {folderId, ids});
      }, true);
      // Every undo and redo is captured separately, including complete stored
      // history entries and runtime counters, before returning to the warm baseline.
      for (const action of ['undo', 'redo', 'undo']) for (let index = 0; index < 5; index++) {
        await boundary(`${action} folder command ${index + 1}`, () => page.evaluate(action => {
          const e = styk.editor;
          if (!(action === 'undo' ? e.canUndo : e.canRedo)) throw new Error(`Missing ${action} for a folder command.`);
          e[action]();
        }, action));
      }
      await page.evaluate(ids => {
        const e = styk.editor;
        if (e.canUndo || e.getLayerFolders().length || ids.some(id => e.getLayer(id).folderId !== undefined)
            || e.report._styk.layers.order[0] !== ids[0] || e.report._styk.layers.order[1] !== ids[1])
          throw new Error('Folder full undo did not restore the original layer state.');
      }, ids);
      const restored = folderSnapshots.at(-1);
      if (restored.document !== initial.document || JSON.stringify(restored.selection) !== JSON.stringify(initial.selection))
        throw new Error('Folder full undo did not restore the initial document and selection.');
      return;
    }
    if (movieclipUnique) {
      const boundary = async (name, operation) => {
        await step(name, operation);
        if (!await settled()) throw new Error(`The engine was not ready after ${name}.`);
        uniqueSnapshots.push(await page.evaluate(name => {
          const e = styk.editor, runtime = (key, value) => ['_movieclipSample', 'runtime_segments'].includes(key) ? undefined : value;
          return {name, document: JSON.stringify(e.report, runtime), undo: e._undo.map(command => command.label), redo: e._redo.map(command => command.label),
            selection: {uid: e.selectedUid, uids: [...e.selectedUids], joint: e.selectedJoint, tick: e.tick},
            phases: e.evaluatedFrame.objects.filter(object => e.report.figure_types.find(figure => figure.figure_type_id === object.figure_type_id)?._styk?.movieclip)
              .map(object => ({uid: object.unique_id, type: object.figure_type_id, phase: object._movieclipSample ?? null}))};
        }, name));
      };
      for (const tick of [2, 2.25]) {
        await boundary(`seek to ${tick} for Make unique`, () => page.evaluate(tick => {
          const e = styk.editor;
          if (!e.layerTimeline || !e.report._styk?.layerSheet || e.frameDraft || e._drag || e.report._styk.movieclipSource)
            throw new Error('The Unique check needs an ordinary recorded sheet movieclip.');
          e.seek(tick);
          if (e.tick !== tick) throw new Error('The Unique check did not reach its requested tick.');
        }, tick));
        await boundary(`select the movieclip at ${tick}`, () => page.evaluate(() => {
          const e = styk.editor, clip = e.evaluatedFrame.objects.find(object => e.report.figure_types.find(figure => figure.figure_type_id === object.figure_type_id)?._styk?.movieclip);
          if (!clip) throw new Error('The recorded movieclip is not visible at the requested tick.');
          e.select(clip.unique_id, -1);
        }));
        const before = await page.evaluate(() => {
          const r = globalThis.__stykEditShadow, e = styk.editor;
          return {uid: e.selectedUid, type: e.figure.figure_type_id, phase: e.object._movieclipSample,
            preludeState: {drag: Boolean(e._drag), animationEdit: Boolean(e._animationEdit)},
            preludes: Object.fromEntries(['commitDrag', 'commitAnimationEdit'].map(name => [name, {...r.byCall?.[name]}])),
            counter: {...r.byCall?.makeMovieclipUnique, owned: r.owned, failedOwned: r.failedOwned, globalCalls: r.calls}};
        });
        await boundary(`Make unique at ${tick}`, async () => {
          await page.locator('[data-action=movieclip-unique]').click();
          await page.evaluate(({before, tick}) => {
            const e = styk.editor;
            if (e.selectedUid === before.uid || e.figure?.figure_type_id === before.type || !e.figure?._styk?.movieclip
                || e.tick !== Math.round(tick) || !Number.isFinite(before.phase) || e.object?._movieclipSample !== before.phase)
              throw new Error('Make unique lost the independent identity or displayed phase.');
          }, {before, tick});
        });
        uniqueCalls.push(await page.evaluate(({before, tick}) => {
          const r = globalThis.__stykEditShadow, e = styk.editor, entry = r.byCall?.makeMovieclipUnique ?? {}, prior = before.counter;
          const preludes = Object.fromEntries(['commitDrag', 'commitAnimationEdit'].map(name => {
            const prior = before.preludes[name], after = {...r.byCall?.[name]};
            const delta = Object.fromEntries(['calls', 'callMs', 'engineMs', 'adoptMs', 'editorMs', 'failedOwned', 'failedOwnedMs']
              .map(key => [key, (after[key] ?? 0) - (prior[key] ?? 0)]));
            return [name, {before: prior, after, delta}];
          }));
          return {tick, owned: r.owned - prior.owned, failedOwned: r.failedOwned - prior.failedOwned, globalCalls: r.calls - prior.globalCalls,
            calls: (entry.calls ?? 0) - (prior.calls ?? 0), callMs: (entry.callMs ?? 0) - (prior.callMs ?? 0),
            engineMs: (entry.engineMs ?? 0) - (prior.engineMs ?? 0), adoptMs: (entry.adoptMs ?? 0) - (prior.adoptMs ?? 0),
            editorMs: (entry.editorMs ?? 0) - (prior.editorMs ?? 0), preludes,
            preludeState: {before: before.preludeState, after: {drag: Boolean(e._drag), animationEdit: Boolean(e._animationEdit)}}};
        }, {before, tick}));
        const after = await page.evaluate(() => ({uid: styk.editor.selectedUid, type: styk.editor.figure.figure_type_id}));
        await boundary(`undo Make unique at ${tick}`, () => page.evaluate(before => {
          const e = styk.editor; e.undo();
          if (e.selectedUid !== before.uid || e.figure?.figure_type_id !== before.type) throw new Error('Undo did not restore the original movieclip identity.');
        }, before));
        await boundary(`redo Make unique at ${tick}`, () => page.evaluate(after => {
          const e = styk.editor; e.redo();
          if (e.selectedUid !== after.uid || e.figure?.figure_type_id !== after.type) throw new Error('Redo did not restore the independent movieclip identity.');
        }, after));
        await boundary(`restore the original movieclip at ${tick}`, () => page.evaluate(before => {
          const e = styk.editor; e.undo();
          if (e.selectedUid !== before.uid || e.figure?.figure_type_id !== before.type) throw new Error('The Unique cycle did not restore its original identity.');
        }, before));
      }
      return;
    }
    if (args.includes('--selection-adoption')) {
      await step('duplicate a figure and restore its new selection', () => page.evaluate(() => {
        const e = styk.editor, object = e.evaluatedFrame.objects[0];
        if (!object) throw new Error('The selection check needs a figure.');
        e.select(object.unique_id, -1);
        const oldUid = e.selectedUid, copy = e.duplicateFigure();
        if (!copy || copy.unique_id === oldUid || e.selectedUid !== copy.unique_id
          || !e.selectionFrame.objects.some(object => object.unique_id === copy.unique_id)) {
          throw new Error('Duplicating a figure lost its new selection.');
        }
        e.undo(); e.redo();
      }));
      await step('paste and duplicate the current selection', () => page.evaluate(() => {
        const e = styk.editor, selected = () => e.selectedUid !== null
          && e.selectionFrame.objects.some(object => object.unique_id === e.selectedUid);
        const previous = e.selectedUid;
        e.copySelection(); e.pasteSelection();
        if (!selected() || e.selectedUid === previous) throw new Error('Pasting lost its new selection.');
        const pasted = e.selectedUid;
        e.duplicateSelection();
        if (!selected() || e.selectedUid === pasted) throw new Error('Duplicating lost its new selection.');
        e.undo(); e.redo();
      }));
    }
    if (args.includes('--origin-range')) {
      await step('transform animation-origin figures across recorded sheet ticks', () => page.evaluate(() => {
        const e = styk.editor, range = Object.assign([0, 1], {sheetTicks: [0, 3, 8]});
        e.select(1, -1); e.seek(3.5);
        e.rotateFiguresInFrames(.27, range, {center: {x: 265.125, y: 180.75}});
        e.mirrorFiguresInFrames('horizontal', range); e.scaleFiguresInFrames(1.2, range, {relative: true});
        e.resetPoseInFrames(range); e.centerFiguresInFrames(range); e.undo(); e.redo();
      }));
      await step('join and detach animation-origin figures across frames', () => page.evaluate(() => {
        const e = styk.editor, range = Object.assign([0, 1], {sheetTicks: [0, 3, 8]});
        e.select(3, -1); e.joinFiguresInFrames(1, 22, range, {uids: [3]});
        e.detachFiguresInFrames(range, {uids: [3]}); e.undo(); e.redo();
      }));
    }
    if (args.includes('--origin-rig')) await step('commit and cancel an animation-origin rig gesture', () => page.evaluate(() => {
      const e = styk.editor, figure = e.report.figure_types[0];
      const joint = figure.segments.findIndex(segment => segment._stykRig?.stretch);
      if (joint < 0) throw new Error('The origin rig check needs its recorded stretch target.');
      e.select(1, joint); e.seek(3.5);
      const tip = () => e.geometry.objects.get(e.selectedSlot).segments[joint].end;
      let p = tip(); if (!e.beginRigDrag()) throw new Error('The origin rig gesture was refused.');
      e.editRigJoint(p.x + 5.25, p.y - 2.125); e.commitRigDrag(); e.undo(); e.redo();
      p = tip(); if (!e.beginRigDrag()) throw new Error('The origin rig cancel gesture was refused.');
      e.editRigJoint(p.x - 7.125, p.y + 4.75); e.cancelRigDrag();
    }));
    if (args.includes('--camera-follow')) {
      await step('nudge a handle under camera follow', () => page.evaluate(() => {
        const e = styk.editor;
        if (!e.report._styk?.cameraMotion?.clips?.some(clip => clip.type === 'follow')) throw new Error('The camera check needs a follow clip.');
        e.seek(2); e.select(1, 1); e.nudgeHandle(3, -2);
      }));
      await step('change camera follow parameters', () => page.evaluate(() => {
        const e = styk.editor, follow = e.report._styk.cameraMotion.clips.find(clip => clip.type === 'follow');
        e.setCameraMotion(follow.id, {params: {lag: 8, lookAhead: 0}});
      }));
      await step('bake camera follow into figures', () => page.evaluate(() => styk.editor.applyCameraToFigures([0], {ticks: [1, 3, 4, 6]})));
      await step('undo and redo camera follow edits', () => page.evaluate(() => { const e = styk.editor; e.undo(); e.undo(); e.redo(); }));
    }
    if (args.includes('--joined-motion')) await step('turn a joined motion path with pinned followers', () => page.evaluate(async () => {
      const e = styk.editor, {buildGeometry} = await import('/src/pivot/geometry.mjs');
      const target = {kind: 'node', uid: 3, handleId: 33, joint: 0};
      e.select(3, 0); e.seek(5);
      if (!e.beginMotionPathEdit(target, 2)) throw new Error('The joined motion check needs its recorded target.');
      const shape = buildGeometry(e.report, e._drag.working).objects.get(2).segments[0];
      const turn = radians => ({x: shape.start.x + (shape.end.x - shape.start.x) * Math.cos(radians) - (shape.end.y - shape.start.y) * Math.sin(radians),
        y: shape.start.y + (shape.end.x - shape.start.x) * Math.sin(radians) + (shape.end.y - shape.start.y) * Math.cos(radians)});
      e.updateMotionPathEdit(turn(.25)); e.updateMotionPathEdit(shape.end); e.cancelMotionPathEdit();
      if (!e.beginMotionPathEdit(target, 2)) throw new Error('The joined motion target stopped being editable.');
      e.updateMotionPathEdit(turn(.25)); e.updateMotionPathEdit(turn(.35), {isolate: true}); e.commitMotionPathEdit(); e.undo(); e.redo();
    }));
    if (args.includes('--bounds')) await step('center and align using measured app bounds', () => page.evaluate(async () => {
      const e = styk.editor, {objectBounds} = await import('/src/renderer-geometry.mjs');
      const boundsOf = geometry => {
        const rect = objectBounds([...geometry.segments.map(segment => ({...segment, type: segment.seg.segment_type || 'line'})),
          ...geometry.polys, ...(geometry.meshDrawables ?? [])], styk.state.renderer?.resources ?? {});
        return {minX: rect.x, minY: rect.y, maxX: rect.x + rect.width, maxY: rect.y + rect.height};
      };
      e.selectMany([2, 3]); e.centerCamera('both', [0, 1], {boundsOf}); e.undo(); e.redo();
      e.selectMany([1, 3]); e.alignFiguresInFrames('x', 'end', [0, 1], {boundsOf}); e.undo(); e.redo();
    }));
    if (args.includes('--capacity')) await step('refuse a new figure at identity capacity atomically', () => page.evaluate(() => {
      const e = styk.editor; e.select(null);
      const snapshot = () => JSON.stringify({document: e.report, nextUid: e._nextUid, undo: e._undo.length, redo: e._redo.length,
        revision: e.revision, layerRevision: e.layerRevision, dragging: e.dragging, dirty: e.dirty}, (key, value) => key === 'runtime_segments' ? undefined : value);
      const before = snapshot(); let rejected = false;
      try { e.addSegment({length: 5}); }
      catch (error) { rejected = error.messageKey === 'errors.editorLayerSheet.objectIdentityCapacityReached'; if (!rejected) throw error; }
      if (!rejected || snapshot() !== before) throw new Error('The capacity refusal changed the project or its history.');
    }));
    if (args.includes('--vfx-remaining')) await step('compare editable effect presets, paints and attached scopes', () => page.evaluate(async () => {
      const e = styk.editor, {createVfx} = await import('/src/vfx.mjs');
      e.select(1, -1); const target = {kind: 'figure', uid: 1};
      e.setVfxScope(target, true); e.applyVfxPreset(target, 'Neon');
      e.addVfxGroup(target, 'Ice', {includeAttached: true, start: 0, duration: 12});
      const paint = {type: 'linear', angle: 45, stops: [{offset: 0, color: [40, 120, 255], opacity: 1}, {offset: 1, color: [240, 60, 80], opacity: .6}]};
      const tint = {...createVfx('tint', 'saved-tint'), params: {color: paint, mix: .7}};
      e.addVfxUserPreset(target, {id: 'saved-preset', name: 'Gradient pulse', created: 0, effects: [tint], keys: {}, strength: .8, fadeIn: 2, fadeOut: 3, duration: 12, offset: 2}, {includeAttached: true});
      e.pasteVfx(target, [tint], {includeAttached: true}); e.setVfxParameter(target, tint.id, 'color', {...paint, type: 'radial'});
      for (let i = 0; i < 6; i++) e.undo(); for (let i = 0; i < 6; i++) e.redo();
    }));
    if (args.includes('--focused')) return;
    if (args.includes('--origin')) await step('set a figure origin and replay its history', () => page.evaluate(() => {
      const e = styk.editor, object = e.evaluatedFrame.objects.find(object => e.report.figure_types.find(figure => figure.figure_type_id === object.figure_type_id)?._styk?.animationOrigin);
      if (!object) throw new Error('The origin check needs a figure with an animation origin.');
      e.select(object.unique_id, -1);
      const handle = e.figure.segments.find(segment => segment.segment_id !== e.figure._styk.animationOrigin.handleId)?.segment_id;
      if (handle === undefined) throw new Error('The origin check needs another native handle.');
      e.setOrigin(handle); e.undo(); e.redo(); e.undo();
    }));
    if (args.includes('--origin-pose')) await step('pose origin joints and both handle gestures', () => page.evaluate(() => {
      const e = styk.editor, object = e.evaluatedFrame.objects.find(object => e.report.figure_types.find(figure => figure.figure_type_id === object.figure_type_id)?._styk?.animationOrigin);
      if (!object) throw new Error('The pose check needs a figure with an animation origin.');
      const select = handle => {
        const figure = e.report.figure_types.find(figure => figure.figure_type_id === object.figure_type_id);
        const joint = figure.segments.findIndex(segment => segment.segment_id === handle);
        if (joint < 0) throw new Error(`The recorded origin pose check needs handle ${handle}.`);
        e.select(object.unique_id, joint);
      };
      const tip = () => e.geometry.objects.get(e.selectedSlot).segments[e.selectedJoint].end;
      select(44);
      let p = tip();
      e.beginDrag('Origin joint check'); e.poseJoint(p.x + 13.25, p.y - 8.125, {snap: false}); e.rotateJoint(.19);
      p = tip(); e.bendJoint(p.x - 9, p.y + 15, {snap: false}); e.commitDrag(); e.undo(); e.redo();
      select(22); p = tip();
      e.beginHandleEditDrag({bend: false}); e.editHandlePosition(p.x + 7, p.y - 9); e.commitHandleEditDrag(); e.undo(); e.redo();
      p = tip(); e.beginHandleEditDrag({bend: true}); e.editHandlePosition(p.x + 12, p.y + 15); e.cancelHandleEditDrag();
      select(33); p = tip();
      e.beginFigureHandleDrag(); e.transformFigureHandle(p.x + 20, p.y - 30, {ctrlKey: true}); e.commitFigureHandleDrag(); e.undo(); e.redo();
      p = tip(); e.beginFigureHandleDrag(p.x, p.y, {rotate: true, scale: true});
      e.transformFigureHandle(p.x + 11, p.y + 17, {shiftKey: true}); e.cancelFigureHandleDrag();
    }));
    if (args.includes('--origin-channels')) await step('scrub and key the saved origin channels', () => page.evaluate(() => {
      const e = styk.editor, object = e.evaluatedFrame.objects.find(object => e.report.figure_types.find(figure => figure.figure_type_id === object.figure_type_id)?._styk?.animationOrigin);
      if (!object || !e.report._styk?.layerSheet) throw new Error('The channel check needs an origin figure on a layer sheet.');
      e.select(object.unique_id, -1);
      const target = {kind: 'figure', uid: object.unique_id};
      e.beginAnimationEdit(target, 'rotation'); e.previewAnimationEdit(-.4); e.previewAnimationEdit(.73); e.commitAnimationEdit(); e.undo(); e.redo();
      e.beginAnimationEdit(target, 'scale'); e.previewAnimationEdit(1.4); e.cancelAnimationEdit();
      const tick = Math.min(8, Math.max(0, e.totalTicks - 1));
      e.addAnimationKey(target, 'scale', tick, .91); e.undo(); e.redo();
      e.addAnimationKeys([{target, property: 'x', tick, value: 260.25}, {target, property: 'rotation', tick, value: .52}]); e.undo(); e.redo();
    }));
    const first = await handles();
    const joints = first.filter(handle => handle.joint >= 0), origin0 = first.find(handle => handle.joint < 0);
    if (joints[0]) await step('drag a joint handle', () => drag(joints[0], 40, -30));
    if (joints[1]) await step('drag another joint handle', async () => { const now = await handles(); const handle = now.find(h => h.joint === joints[1].joint) ?? joints[1]; await drag(handle, -30, 25); });
    if (origin0) await step('drag the figure by its origin', async () => { const now = await handles(); await drag(now.find(h => h.joint < 0) ?? origin0, 50, 20); });
    await step('undo and redo from the keyboard', async () => { await page.keyboard.press('Control+z'); await sleep(150); await page.keyboard.press('Control+z'); await sleep(150); await page.keyboard.press('Control+y'); await sleep(150); });
    await step('seek and transform through the editor', () => page.evaluate(() => { const e = styk.editor, last = Math.max(0, e.totalTicks - 1); e.seek(Math.min(3, last)); e.rotateSelection(0.25); e.scaleSelection(1.1); e.moveSelection(12, -8); e.undo(); e.redo(); }));
    if (args.includes('--figures-gate')) return;
    await step('keys and frames of the layer timeline', () => page.evaluate(() => { const e = styk.editor, layer = [e.activeLayerId]; e.sheetInsertKey(layer, Math.min(2, e.totalTicks - 1)); e.sheetDuplicateFrameAfter(); e.sheetSetEasing(layer, 0, 1, {interpolation: 'ease-in'}); e.sheetInsertFrames(layer, 1, 2); e.sheetRemoveFrames(layer, 1, 1); e.undo(); e.undo(); e.redo(); }));
    await step('rename, colour, hide, lock, create, move and group layers', () => page.evaluate(() => {
      const e = styk.editor, layers = () => e.report._styk?.layers;
      const first = layers()?.order?.[0];
      if (!first) return;
      e.renameLayer(first, 'Renamed'); e.setLayerColor([first], 'teal'); e.setLayerVisible(first, false); e.setLayerVisible(first, true); e.setLayerLocked('scene', true);
      const created = e.createLayer('Extra', {activate: false});
      e.moveLayerTo(created.id, 0); const folder = e.createLayerFolder('Group', [created.id]); e.renameLayerFolder(folder.id, 'Kept');
      e.arrangeLayers([...layers().order].reverse()); e.deleteLayerFolder(folder.id); e.setLayerLocked('scene', false); e.undo(); e.undo(); e.redo();
    }));
    await step('focus a layer with its lock, and follow a timeline layer', () => page.evaluate(() => {
      const e = styk.editor, kept = e.activeLayerId, other = e.report._styk?.layers?.order?.find(id => id !== kept);
      if (!other) return;
      e.setLayerFocus([kept], {lock: true});
      try { e.sheetInsertKey([other], 1); } catch {}
      e.sheetInsertKey([kept], 1); e.undo(); e.clearLayerFocus();
      const uid = e.selectedUid;
      e.selectSheetLayer(kept); e.seek(Math.min(2, e.totalTicks - 1)); e.seek(0); e.moveSelection(4, 2); e.undo(); e.keepSelectionOnSeek(); e.seek(1); e.seek(0);
      if (uid !== null) e.select(uid, -1);
    }));
    await step('select a branch and delete it', () => page.evaluate(() => {
      const e = styk.editor, uid = e.selectedUid, figure = e.figure; if (uid === null || !figure || figure.segments.length < 3) return;
      e.selectBranch(uid, 1); e.deleteSelection(); e.select(uid, -1);
    }));
    await step('extend a key in one batch, as the timeline does', () => page.evaluate(() => { const e = styk.editor, layer = e.activeLayerId; e.movieclipBatch('Extend keyframe', () => { const {start, end} = e.sheetSpan(layer, e.tick); e.sheetSetDuration(end - start + 1, layer, start); }); e.undo(); e.redo(); }));
    await step('copy and paste timeline frames', () => page.evaluate(() => {
      const e = styk.editor, layer = e.activeLayerId, other = e.report._styk?.layers?.order?.find(id => id !== layer);
      e.sheetCopyFrames([layer], 0, 1); e.sheetPasteFrames([layer], 3); e.sheetPasteFrames([layer], 6, {insert: true});
      if (other) e.sheetPasteFrames([other], 2);
      e.undo();
    }));
    await step('drag frames to another layer', () => page.evaluate(() => { const e = styk.editor, order = e.report._styk?.layers?.order ?? []; if (order.length < 2) return; e.sheetTransferFrames([order[0]], [order[1]], 0, 1, 3, {copy: true}); e.undo(); }));
    await step('move and copy keys along the timeline', () => page.evaluate(() => { const e = styk.editor, layer = [e.activeLayerId]; e.sheetMoveFrames(layer, 0, 0, 1); e.sheetMoveFrames(layer, 1, 1, 1, {copy: true}); e.undo(); e.undo(); e.redo(); }));
    await step('edit a figure from the inspector', () => page.evaluate(() => { const e = styk.editor; e.updateObject({scale: 1.2}); e.updateObject({field28: 200, name: 'Inspected'}); e.undo(); }));
    // Property edits on the layer sheet: scrubs with their previews, keyed values, the scene's
    // colour, the figure's outline and a polygon's opacity, then an effect's parameter.
    await step('scrub and key properties from the inspector', () => page.evaluate(() => {
      const e = styk.editor, uid = e.selectedUid; if (uid === null || !e.report._styk?.layerSheet) return;
      const figure = {kind: 'figure', uid}, later = Math.max(0, Math.min(2, e.totalTicks - 1));
      // Earlier steps may leave a layer locked: a refused edit is compared like any other.
      const attempt = run => { try { run(); } catch { /* refused: compared in the shadow */ } };
      attempt(() => { e.beginAnimationEdit(figure, 'scale'); e.previewAnimationEdit(1.2); e.previewAnimationEdit(1.3); e.commitAnimationEdit(); });
      attempt(() => { e.beginAnimationEdit(figure, 'opacity'); e.previewAnimationEdit(0.6); e.cancelAnimationEdit(); });
      attempt(() => { e.beginAnimationEdit({kind: 'camera'}, 'x'); e.previewAnimationEdit(25); e.commitAnimationEdit(); });
      attempt(() => e.addAnimationKeys([{target: figure, property: 'color', value: 0xff3080c0}, {target: figure, property: 'zIndex', tick: later, value: 1}]));
      // The layer step leaves the scene locked (its unlock undone): unlocked here so that the scene's colour is edited.
      if (e.report._styk?.layers?.fixed?.scene?.locked) e.setLayerLocked('scene', false);
      attempt(() => e.setSceneColor(0xff204060)); attempt(() => e.setBackgroundColor(0xffe0e0ff)); attempt(() => e.setSceneBackground(0));
      attempt(() => e.updateFigureStyle({outlineWidth: 2, outlineOpacity: 0.5}));
      if (e.figure?.polyfills?.length) attempt(() => e.updatePolygon(0, {opacity: 0.4}));
      e.cancelAnimationEdit(); e.undo(); e.undo(); e.redo();
    }));
    await step('add an effect to a figure', () => page.evaluate(() => { const e = styk.editor; if (e.selectedUid !== null && e.report._styk?.layerSheet) e.addVfx({kind: 'figure', uid: e.selectedUid}, 'blur'); }));
    await step('duplicate, move, reorder, reset and remove effects', () => page.evaluate(() => {
      const e = styk.editor, uid = e.selectedUid; if (uid === null) return;
      const figure = {kind: 'figure', uid}, effect = e.vfxEffects(figure).find(item => item.type === 'blur'); if (!effect) return;
      try { const copy = e.duplicateVfx(figure, effect.id); e.moveVfx(figure, copy, -1); e.reorderVfx(figure, copy, 0); e.resetVfx(figure, effect.id); e.removeVfx(figure, copy); e.pasteVfx(figure, [effect]); e.undo(); } catch {}
    }));
    await step('key and scrub an effect parameter', () => page.evaluate(() => {
      const e = styk.editor, uid = e.selectedUid; if (uid === null) return;
      const figure = {kind: 'figure', uid}, effect = e.vfxEffects(figure).find(item => item.type === 'blur'); if (!effect) return;
      try {
        e.setVfxParameter(figure, effect.id, 'radius', 5);
        e.beginAnimationEdit(figure, `vfx1:blur:${effect.id}:radius`, {exactTarget: true}); e.previewAnimationEdit(8); e.previewAnimationEdit(9); e.commitAnimationEdit();
      } catch { /* a locked layer: compared in the shadow */ }
      e.cancelAnimationEdit(); e.undo(); e.undo();
    }));
    await step('create, reshape and delete a polygon', () => page.evaluate(() => {
      const e = styk.editor, figure = e.figure; if (!figure) return;
      const ids = [0, ...figure.segments.slice(0, 3).map(segment => segment.segment_id)]; if (ids.length < 4) return;
      e.addPolygon(ids.slice(0, 3)); const index = e.figure.polyfills.length - 1;
      e.updatePolygon(index, {zOrder: -3}); e.updatePolygon(index, {handleIds: [ids[0], ids[1], ids[3]]}); e.removePolygon(index); e.undo(); e.undo();
    }));
    await step('copy, paste, duplicate and cut figures', () => page.evaluate(() => {
      const e = styk.editor; if (e.selectedUid === null) return;
      e.copySelection(); e.pasteSelection(); e.pasteSelection({pasteMode: 'instance'}); e.duplicateSelection(); e.cutSelection(); e.pasteSelection(); e.undo(); e.undo(); e.undo();
    }));
    await step('import a figure', () => page.evaluate(() => { const e = styk.editor, figure = e.report.figure_types[0]; if (!figure || figure._styk?.movieclip) return; e.importFigure(structuredClone(figure), {position: {x: 200, y: 150}}); e.moveSelection(10, 0); e.undo(); e.undo(); }));
    await step('add an effect clip', () => page.evaluate(() => { const e = styk.editor; if (e.selectedUid !== null && !e.getVfxClips().length) e.addVfxClip({kind: 'figure', uid: e.selectedUid}, 'glow', {start: 0, duration: 8}); }));
    await step('edit an effect clip, its parameters and keys', () => page.evaluate(() => {
      const e = styk.editor, clip = e.getVfxClips()[0]; if (!clip) return; const effect = clip.effects[0].id;
      e.setVfxClip(clip.id, {name: 'Shadowed', strength: 0.7}); e.setVfxClipParameter(clip.id, effect, 'radius', 20); e.setVfxClipParameters(clip.id, effect, {intensity: 1.2}, {key: true});
      e.beginVfxClipEdit(); e.setVfxClipParameter(clip.id, effect, 'radius', 24); e.setVfxClipParameter(clip.id, effect, 'radius', 26); e.commitVfxClipEdit();
      e.trimVfxClipStart(clip.id, 2); e.undo(); e.deleteVfxClip(clip.id); e.undo();
    }));
    await step('copy, paste and duplicate an effect clip, add generated effects', () => page.evaluate(() => {
      const e = styk.editor, clip = e.getVfxClips()[0]; if (!clip || e.selectedUid === null) return; const figure = {kind: 'figure', uid: e.selectedUid};
      e.copyVfxClip(clip.id); e.pasteVfxClip(figure); e.pasteVfxClip(figure, {animation: false}); e.duplicateVfxClip(clip.id);
      e.addVfxGroupEffect(clip.id, 'particles'); e.addVfxClip({kind: 'scene'}, 'smoke'); for (let i = 0; i < 5; i++) e.undo();
    }));
    await step('edit a segment from the inspector', () => page.evaluate(() => { const e = styk.editor; if (e.selectedUid === null) return; e.select(e.selectedUid, 1); e.updateSegment({color_rgb: [10, 20, 30]}); e.updateSegment({angle_rad: 1}); e.undo(); e.select(e.selectedUid, -1); }));
    // The builder's segment styles: shapes and the construction flag in the definition, opacity
    // and thickness keyed and colours recoloured on the figure on show, caps, the figure's name.
    await step('style segments as the builder does', () => page.evaluate(() => {
      const e = styk.editor, uid = e.selectedUid; if (uid === null || !e.figure || e.figure.segments.length < 2) return;
      const attempt = run => { try { run(); } catch { /* refused: compared in the shadow */ } };
      e.select(uid, 1);
      attempt(() => e.convertSegment('circle')); attempt(() => e.convertSegment('line'));
      attempt(() => e.updateBuilderSegmentStyle({opacity: 0.6})); attempt(() => e.updateBuilderSegmentStyle({thickness: 5}, {branch: true}));
      attempt(() => e.updateBuilderSegmentStyle({color: '#aa3300'})); attempt(() => e.updateBuilderSegmentStyle({static: true}));
      attempt(() => e.setBranchCaps(true)); attempt(() => e.setBranchFill()); attempt(() => e.updateFigureStyle({name: 'Built'}));
      for (let i = 0; i < 4; i++) e.undo();
      e.select(uid, -1);
    }));
    // The figure's own drawing order: a segment raised, lowered, placed; a polygon to the front.
    await step('order the figure\'s segments as the builder does', () => page.evaluate(() => {
      const e = styk.editor, uid = e.selectedUid; if (uid === null || !e.figure || e.figure.segments.length < 2) return;
      const attempt = run => { try { run(); } catch { /* refused: compared in the shadow */ } };
      e.select(uid, 1);
      attempt(() => e.moveBuilderOrder({kind: 'segment'}, 'front')); attempt(() => e.moveBuilderOrder({kind: 'segment'}, 'down', {branch: true}));
      attempt(() => e.setBuilderOrder({kind: 'segment', index: 1}, 2)); attempt(() => e.orderSelection(1));
      if (e.figure?.polyfills?.length) attempt(() => e.moveBuilderOrder({kind: 'polygon', index: 0}, 'front'));
      for (let i = 0; i < 5; i++) e.undo();
      e.select(uid, -1);
    }));
    // Rig relationships: Smart Stretch, a behaviour merged into a joint, angle locks measured on
    // the figure on show, parts bound to a driver.
    await step('relate parts of the rig', () => page.evaluate(() => {
      const e = styk.editor, uid = e.selectedUid; if (uid === null || !e.figure || e.figure.segments.length < 3) return;
      const attempt = run => { try { run(); } catch { /* refused: compared in the shadow */ } };
      e.select(uid, 0); attempt(() => e.enableSmartStretch(true)); attempt(() => e.setRigBehavior({stretch: {min: 0.5}}));
      e.select(uid, 1); attempt(() => e.setAngleLock('world')); attempt(() => e.setRigBehavior({response: {mode: 'rigid'}}));
      e.select(uid, 2); attempt(() => e.bindStretchParts(e.figure.segments[0].segment_id)); attempt(() => e.setAngleLock('parent'));
      attempt(() => e.setSecondaryAngleReference(e.figure.segments[0].segment_id));
      for (let i = 0; i < 6; i++) e.undo();
      e.select(uid, -1);
    }));
    // The figure's topology as the builder edits it: segments added and drawn, split, flipped,
    // duplicated, moved in the hierarchy, unsplit and deleted, rig joints appended.
    await step('change the figure\'s topology as the builder does', () => page.evaluate(() => {
      const e = styk.editor, uid = e.selectedUid; if (uid === null || !e.figure || e.figure.segments.length < 3) return;
      const attempt = run => { try { run(); } catch { /* refused: compared in the shadow */ } };
      const ids = () => e.figure.segments.map(segment => segment.segment_id);
      e.select(uid, 1); attempt(() => e.addSegment({length: 30, angle: 0.4})); attempt(() => e.addSegments([{length: 20, angle: 1}, {length: 15, angle: 1.4}]));
      e.select(uid, 1); attempt(() => e.splitSegment(0.5)); attempt(() => e.mirrorSegment('horizontal')); attempt(() => e.duplicateSegmentBranch({angleOffset: 0.3}));
      e.select(uid, 2); attempt(() => e.moveBuilderSegment(ids()[2], {parentId: ids()[0]})); attempt(() => e.unsplitSegment());
      e.select(uid, e.figure.segments.length - 1); attempt(() => e.deleteSingleSegment()); attempt(() => e.deleteSegment());
      e.select(uid, 0); attempt(() => e.addConnector(ids()[0], ids()[1])); attempt(() => e.addWeightedJoint(ids()[0], [{joint: ids()[1], weight: 0.5}]));
      attempt(() => e.replaceFigureDefinition(e.figure.figure_type_id, {...structuredClone(e.figure), name: 'Swapped', segments: e.figure.segments.slice(0, -1)}, {label: 'Edit text'}));
      for (let i = 0; i < 13; i++) e.undo();
      e.select(uid, -1);
    }));
    // The Rig tool's drags: a joint repositioned, a branch turned, a joint bent with its branch,
    // a cancelled drag; each commit changes the definition and every pose of it.
    await step('drag joints with the Rig tool', () => page.evaluate(() => {
      const e = styk.editor, uid = e.selectedUid; if (uid === null || !e.figure || e.figure.segments.length < 3) return;
      const attempt = run => { try { run(); } catch { /* refused: compared in the shadow */ } };
      const end = joint => e.geometry.objects.get(e.selectedSlot)?.segments[joint]?.end;
      e.select(uid, 1); let p = end(1);
      if (p) attempt(() => { if (e.beginRigDrag({})) { e.editRigJoint(p.x + 12, p.y - 6); e.editRigJoint(p.x + 16, p.y - 4); e.commitDrag(); } });
      e.select(uid, 2); p = end(2);
      if (p) attempt(() => e.editRigJoint(p.x - 10, p.y + 8, {reposition: false}));
      p = end(2);
      if (p) attempt(() => { if (e.beginRigDrag({bend: true})) { e.bendJoint(p.x + 6, p.y + 6); e.commitDrag(); } });
      p = end(2);
      if (p) attempt(() => { if (e.beginRigDrag({})) { e.editRigJoint(p.x + 5, p.y + 5); e.cancelRigDrag(); } });
      for (let i = 0; i < 3; i++) e.undo();
      e.select(uid, -1);
    }));
    await step('move a handle in Handle Edit Mode', () => page.evaluate(() => {
      const e = styk.editor; if (e.selectedUid === null) return; e.select(e.selectedUid, 1);
      const p = e.geometry.objects.get(e.selectedSlot)?.joints.get(e.figure?.segments[1]?.segment_id); if (!p) return;
      try { if (!e.beginHandleEditDrag()) return; e.editHandlePosition(p.x + 20, p.y - 10); e.editHandlePosition(p.x + 25, p.y - 5, {bend: true}); e.commitHandleEditDrag(); e.editHandlePosition(p.x - 10, p.y + 10); e.undo(); } catch {}
    }));
    await step('turn and scale figures by a handle', () => page.evaluate(() => {
      const e = styk.editor; if (e.selectedUid === null) return; e.select(e.selectedUid, 1);
      const p = e.geometry.objects.get(e.selectedSlot)?.segments[1]?.end; if (!p) return;
      try { if (!e.beginFigureHandleDrag(p.x, p.y, {rotate: true, scale: true, joint: 1})) return; e.transformFigureHandle(p.x - 30, p.y + 20); e.transformFigureHandle(p.x - 40, p.y + 60, {shiftKey: true}); e.commitFigureHandleDrag(); e.transformFigureHandle(p.x + 10, p.y - 10, {ctrlKey: true}); e.undo(); } catch {}
    }));
    await step('nudge and align handles, type and paste segment values', () => page.evaluate(() => {
      const e = styk.editor; if (e.selectedUid === null) return; e.select(e.selectedUid, 1);
      try {
        e.nudgeHandle(4, -2); e.nudgeHandle(2, 2, {bend: true}); e.alignHandle(1, 0); e.nudgeHandle(3, 0, {editMode: true});
        if (e.beginSegmentValueEdit({})) { e.setSegmentValues({angle: 30}); e.setSegmentValues({length: 70}); e.commitDrag(); }
        e.setSegmentValues({bend: 10}); e.copySegmentValues(); e.select(e.selectedUid, 2); e.pasteSegmentValues(); e.locateEditedHandle(1); e.undo();
      } catch {}
    }));
    await step('move the camera and type its values', () => page.evaluate(() => {
      const e = styk.editor;
      try {
        if (e.beginCameraDrag()) { e.updateCameraDrag({x: 12, y: -6}); e.updateCameraDrag({x: 20, scale: 1.1}); e.commitCameraDrag(); }
        e.updateCameraDrag({angle_rad: 0.05}); e.cancelCameraDrag();
        e.setCameraValues({x: 15}); e.transformCamera({y: 4, scale: 1.05}); e.copyCamera(); e.resetCamera(); e.pasteCamera(); e.locateCamera(1); e.applyCameraToFigures(); e.undo(); e.undo();
      } catch {}
    }));
    await step('change the playback of a movieclip on every frame', () => page.evaluate(() => {
      const e = styk.editor, clip = e.selectionFrame.objects.find(o => e.report.figure_types.find(f => f.figure_type_id === o.figure_type_id)?._styk?.movieclip);
      if (!clip) return; e.select(clip.unique_id, -1); e.setMovieclipPlayback({speed: 1.5}, {allFrames: true}); e.undo();
    }));
    await step('add, copy, paste, duplicate and reseed camera motion clips', () => page.evaluate(() => {
      const e = styk.editor; try { const id = e.addCameraMotion('shake'); e.copyCameraMotion(id); e.pasteCameraMotion(); e.duplicateCameraMotion(id); e.randomizeCameraMotion(id); e.undo(); } catch {}
    }));
    await step('edit and delete a camera motion clip', () => page.evaluate(() => { const e = styk.editor, clip = e.getCameraMotions()[0]; if (!clip) return; e.setCameraMotion(clip.id, {start: clip.start + 1}); e.setCameraMotion(clip.id, {enabled: !clip.enabled}); e.deleteCameraMotion(clip.id); e.undo(); }));
    await step('set a layer parallax', () => page.evaluate(() => { const e = styk.editor, layer = e.report._styk?.layers?.order?.[0]; if (!layer) return; e.setLayerParallax(layer, {enabled: true, x: 0.5}); e.setLayerParallax(layer, {linked: false, y: 2}); e.undo(); e.undo(); }));
    await step('change the project settings', () => page.evaluate(() => { const e = styk.editor, {fps, canvas_width: width, canvas_height: height} = e.report; e.setFPS(fps + 1); e.updateProject({canvas_width: width + 10}); e.setCanvasSize(width, height); e.undo(); e.undo(); e.undo(); }));
    await step('add, reorder and remove project colours', () => page.evaluate(() => { const e = styk.editor; e.addPaletteColor('#3366cc'); e.addPaletteColors(['#ff8800', [10, 200, 30]]); e.movePaletteColor('#ff8800', 0); e.removePaletteColor('#3366cc'); e.undo(); }));
    await step('add a figure from the library', () => page.evaluate(() => { const e = styk.editor; e.addFigure(e.report.figure_types[0]?.figure_type_id); e.moveSelection(-30, 15); e.undo(); }));
    await step('duplicate, join, unjoin and delete a figure', () => page.evaluate(() => { const e = styk.editor, original = e.selectedUid; const copy = e.duplicateFigure(); if (copy && original !== null) { e.joinTo(original, 0); e.unjoinFigure(); e.moveSelection(20, 20); e.deleteSelection(); e.undo(); e.undo(); } }));
    // Figures changed across frames as the app does it (a timeline range carries its ticks),
    // ordered, joined and detached at the playhead, a branch and a figure flipped.
    await step('change figures across frames, order, join and flip them', () => page.evaluate(() => {
      const e = styk.editor, uid = e.selectedUid; if (uid === null || !e.report._styk?.layerSheet) return;
      const last = Math.max(0, e.totalTicks - 1), range = Object.assign([e.frameIndex], {sheetTicks: [...new Set([0, Math.min(2, last), e.tick])]});
      const attempt = run => { try { run(); } catch { /* refused: compared in the shadow */ } };
      attempt(() => e.updateFiguresInFrames({field28: 40}, range)); attempt(() => e.mirrorFiguresInFrames('horizontal', range));
      attempt(() => e.rotateFiguresInFrames(Math.PI / 12, range)); attempt(() => e.centerFiguresInFrames(range));
      attempt(() => e.moveSelectionInFrames(6, -4, range)); attempt(() => e.resetPoseInFrames(range)); attempt(() => e.duplicateInFrames(range));
      const copy = e.selectedUid;
      attempt(() => e.orderSelection('back', {frameIndices: undefined})); attempt(() => e.bringToFront()); attempt(() => e.moveSelectionLayer(-1));
      attempt(() => e.orderFiguresInFrames('front', range));
      attempt(() => e.joinFiguresInFrames(uid, 0, undefined, {uids: [copy], atPlayhead: true})); attempt(() => e.detachFiguresInFrames(undefined, {uids: [copy], atPlayhead: true}));
      attempt(() => e.deleteFiguresInFrames(range));
      e.select(uid, -1); attempt(() => e.updateFiguresInFrames({field28: 10}, [e.frameIndex], {ticks: [e.tick]})); attempt(() => e.addFigureInFrames(undefined, range));
      e.select(uid, 1); attempt(() => e.mirrorFigure('horizontal')); attempt(() => e.mirrorSelection('vertical'));
      e.select(uid, -1); attempt(() => e.mirrorFigure('vertical'));
      for (let i = 0; i < 6; i++) e.undo();
      e.select(uid, -1);
    }));
    // Recolouring as the inspector does it: a palette colour replaced, the figure's colour set
    // (across a timeline range too), a polygon's colour, a reset.
    await step('recolour figures from the palette and the inspector', () => page.evaluate(() => {
      const e = styk.editor; if (e.selectedUid === null) return;
      const attempt = run => { try { run(); } catch { /* refused: compared in the shadow */ } };
      const entry = e.figurePalette()[0], last = Math.max(0, e.totalTicks - 1);
      if (entry) attempt(() => e.replaceFigureColor(entry.color, '#204080', {targets: entry, indices: undefined}));
      attempt(() => e.setFigureColor('#c04020', {indices: undefined}));
      attempt(() => e.setFigureColor('#40c020', {indices: Object.assign([e.frameIndex], {sheetTicks: [...new Set([0, Math.min(2, last), e.tick])]}), opacity: 0.8}));
      if (e.figure?.polyfills?.length) attempt(() => e.updatePolygon(0, {color: '#336699'}));
      attempt(() => e.resetFigureColors({indices: undefined}));
      e.undo(); e.undo();
    }));
    if (joints[0]) await step('drag a joint again after the commands', async () => { const now = await handles(); const handle = now.find(h => h.joint >= 0); if (handle) await drag(handle, 25, 35); });
    // Motion paths: trail points dragged at another tick than the playhead (the figure's origin,
    // a joint, the end of a limb with IK), a falloff reshaping the keys nearby and its influence
    // changed mid-gesture, a cancelled gesture, history.
    await step('drag motion-path points and reshape an arc', () => page.evaluate(() => {
      const e = styk.editor, uid = e.selectedUid, figure = e.figure;
      if (uid === null || !figure || !e.report._styk?.layerSheet || e.totalTicks < 2) return;
      const attempt = run => { try { run(); } catch { /* refused: compared in the shadow */ } };
      const last = Math.max(0, Math.ceil(e.totalTicks) - 1), other = e.tick >= 1 ? 0 : Math.min(2, last);
      const root = e.geometry.objects.get(e.selectedSlot)?.root ?? {x: 400, y: 300};
      const node = (handleId, joint) => ({kind: 'node', uid, handleId, joint});
      const trail = (target, tick, points, options) => attempt(() => {
        if (!e.beginMotionPathEdit(target, tick, options)) return;
        for (const [x, y, more] of points) e.updateMotionPathEdit({x, y}, more);
        e.commitMotionPathEdit();
      });
      trail(node(0, -1), other, [[root.x + 14, root.y - 6], [root.x + 20, root.y - 2]]);
      const joint = figure.segments.findIndex((s, i) => i > 0 && !s.is_static && !(s.aux_code & 1) && !e.object?._stykIK?.[s.segment_id]?.enabled);
      if (joint > 0) {
        const id = figure.segments[joint].segment_id;
        trail(node(id, joint), other, [[root.x + 60, root.y + 40]]);
        trail(node(id, joint), last, [[root.x + 50, root.y + 50], [root.x + 55, root.y + 45, {stretch: true}]], {falloff: {radius: 3, shape: 'smooth'}});
      }
      const ik = figure.segments.findIndex(s => e.object?._stykIK?.[s.segment_id]?.enabled);
      if (ik >= 0) trail(node(figure.segments[ik].segment_id, ik), other, [[root.x - 40, root.y + 70]], {falloff: {radius: 2, shape: 'linear'}});
      attempt(() => {
        if (!e.beginMotionPathEdit(node(0, -1), last)) return;
        e.updateMotionPathEdit({x: root.x + 8, y: root.y + 8}); e.setMotionPathFalloff({radius: 4, shape: 'sharp'}); e.setMotionPathFalloff({radius: 6, shape: 'flat'});
        e.updateMotionPathEdit({x: root.x + 10, y: root.y + 4}); e.commitMotionPathEdit();
      });
      attempt(() => { if (e.beginMotionPathEdit(node(0, -1), other, {falloff: {radius: 2}})) { e.updateMotionPathEdit({x: root.x - 30, y: root.y}); e.cancelMotionPathEdit(); } });
      // A gesture that ends as it began changes nothing.
      attempt(() => { if (e.beginMotionPathEdit(node(0, -1), other)) e.commitMotionPathEdit(); });
      e.undo(); e.undo(); e.redo();
    }));
    // The same with the paths shown, as people do it: a trail point dragged with the pointer,
    // then reshaped with the right button (the saved falloff).
    await step('drag a motion-path point with the pointer, and reshape with the right button', async () => {
      const point = await page.evaluate(async () => {
        const {editor: e, state, actions} = styk, uid = e.selectedUid;
        if (uid === null || !e.report._styk?.layerSheet || e.totalTicks < 2) return null;
        e.select(uid, -1);
        if (!state.motionPaths) actions['toggle-motion-paths']();
        for (let i = 0; i < 8; i++) await new Promise(resolve => requestAnimationFrame(resolve));
        const {samplePathPoint} = await import('/src/motion-path.mjs');
        const {worldToScreen} = await import('/src/renderer.mjs');
        const tick = e.tick >= 1 ? 0 : Math.min(2, Math.ceil(e.totalTicks) - 1);
        const world = samplePathPoint(e.report, {kind: 'node', uid, handleId: 0, joint: -1}, tick);
        if (!world || !Number.isFinite(world.x)) return null;
        const rect = (document.querySelector('#overlay') ?? document.querySelector('#viewport')).getBoundingClientRect();
        const at = worldToScreen(world, {report: e.report, frame: e.evaluatedFrame, view: state.view, cameraEnabled: false});
        return {x: rect.left + at.x, y: rect.top + at.y};
      });
      if (!point) return;
      await drag(point, 30, 20);
      await drag(point, -20, 25, 'right');
      await page.evaluate(() => { const {state, actions, editor} = styk; if (state.motionPaths) actions['toggle-motion-paths'](); editor.undo(); });
    });
    // A hand or foot with an IK controller, when the figure has one: its target is dragged.
    const target = (await handles()).find(handle => handle.ik);
    if (target) await step('drag an IK target', async () => { await settled(); const now = await handles(); await drag(now.find(h => h.joint === target.joint) ?? target, -30, 20); });
    // The inspector's IK settings on that limb: pinned and released, its target turned, the
    // bend side changed.
    if (target) await step('change the IK settings of a limb', () => page.evaluate(joint => {
      const e = styk.editor; e.select(e.selectedUid, joint);
      const pinned = Boolean(e.getIKSettings().pinned);
      e.setNodeIK({pinned: !pinned}); e.rotateIKTarget(0.2); e.setNodeIK({bendDirection: -1}); e.setNodeIK({pinned});
    }, target.joint));
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
  };
  // --warm: once untimed, so that both editors run warmed-up code (the browser optimizes
  // hot WebAssembly and JavaScript after their first calls), then again, timed.
  if (args.includes('--warm')) {
    await interactions();
    await settled();
    await page.evaluate(() => { const r = globalThis.__stykEditShadow; if (r) Object.assign(r, {calls: 0, compared: 0, rounded: 0, skipped: 0, jsMs: 0, engineMs: 0, owned: 0, ownedMs: 0, ownableMs: 0, failedOwned: 0, failedOwnedMs: 0, byCall: {}, comparedByCall: {}}); });
    steps.length = 0;
    if (movieclipUnique) uniqueSnapshots.length = 0;
    if (movieclipUnique) uniqueCalls.length = 0;
    if (layerFolders) folderSnapshots.length = 0;
    if (layerFolders) folderCalls.length = 0;
    if (figureClipboardPastes) clipboardRows.length = 0;
  }
  await interactions();
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
  const clipboardUndone = figureClipboardPastes ? await page.evaluate(() => {
    const r = globalThis.__stykEditShadow;
    return {snapshot: globalThis.__stykClipboardProof.snapshot(),
      shadow: {mode: r.mode, rounded: r.rounded, mismatches: r.mismatches, failedOwned: r.failedOwned, broken: r.broken ?? null}};
  }) : null;
  await context.close();
  return {report, outcome, steps, refused, problems, ...(movieclipUnique ? {uniqueSnapshots, uniqueCalls} : {}), ...(layerFolders ? {folderSnapshots, folderCalls} : {}),
    ...(figureClipboardPastes ? {clipboardRows, clipboardUndone} : {})};
}
try {
  if (own) {
    // The editor alone, shadowed so that its time in each call the engine would own is
    // measured (shadowing never changes what the editor does).
    const alone = await run('engineShadow=on');
    // With --default, the app as users get it: the engine owns the document's calls when its
    // editing class is owned by default (src/engine/engine-edit-shadow.mjs, OWNED_CLASSES).
    const owned = await run(args.includes('--default') ? '' : 'engineEdit=on');
    const {report, steps, refused, problems} = owned;
    await saveCaptures(alone.report, 'alone');
    await saveCaptures(report, 'owned');
    console.log(`steps: ${steps.join('; ')}`);
    for (const line of refused) console.log(`  refused by the editor: ${line}`);
    console.log(`engine-owned calls: ${report.owned} adopted from the engine (${report.ownedMs.toFixed(1)} ms), ${report.compared} run in both and the same, ${report.mismatches.length} different, ${report.skipped} not shadowed; ${report.sessions} sessions; ${report.failedOwned} failed owner attempts (${report.failedOwnedMs.toFixed(1)} ms)`);
    for (const [why, count] of Object.entries(report.ended)) console.log(`  session ended ${count}×: ${why}`);
    for (const [what, count] of Object.entries(report.unported)) console.log(`  not ported ${count}×: ${what}`);
    for (const mismatch of report.mismatches.slice(0, 8)) console.log(`  ${mismatch.call}(${JSON.stringify(mismatch.args ?? []).slice(0, 80)}):\n    ${mismatch.problems.join('\n    ').slice(0, 1500)}`);
    // Numbers may differ in their last digits: the engine computes with Node's Math, the
    // browser's editor with the browser's (docs/engine/decisions/0016-v8-exact-math.md).
    let rounded = 0;
    const near = (x, y, at) => {
      if (typeof x === 'number' && typeof y === 'number') {
        if (x === y) return null;
        if (!exactMath && Math.abs(x - y) <= 1e-9 * Math.max(1, Math.abs(x), Math.abs(y))) { rounded++; return null; }
        return `${at}: ${x} against ${y}`;
      }
      if (x === null || y === null || typeof x !== 'object' || typeof y !== 'object') return x === y ? null : `${at}: ${JSON.stringify(x)?.slice(0, 200)} against ${JSON.stringify(y)?.slice(0, 200)}`;
      const keys = Object.keys(x), others = Object.keys(y);
      if (keys.length !== others.length || keys.some((key, i) => key !== others[i])) return `${at}: keys ${keys.join(',').slice(0, 200)} against ${others.join(',').slice(0, 200)}`;
      for (const key of keys) { const found = near(x[key], y[key], `${at}/${key}`); if (found) return found; }
      return null;
    };
    const differences = [];
    // Identities the editor draws (effect clips, their effects, which write them as `fx-` and
    // the hex digits without dashes) are named by where they first appear: other parts of the
    // app draw identities too, at moments that differ between runs.
    const drawn = text => { const names = new Map(); return text.replace(/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}|(?<=fx-)[0-9a-f]{32}\b/g, id => { if (!names.has(id)) names.set(id, `id-${names.size + 1}`); return names.get(id); }); };
    for (const phase of ['after', 'undone']) for (const key of ['document', 'undo', 'redo', 'selection']) {
      const [a, b] = [alone, owned].map(run => key === 'document' ? JSON.parse(drawn(run.outcome[phase][key])) : run.outcome[phase][key]);
      const found = near(a, b, `${phase} ${key}`);
      if (found) differences.push(found);
    }
    let figureClipboardProof = null;
    if (figureClipboardPastes) {
      const check = (condition, message) => { if (!condition) differences.push(`Figure clipboard: ${message}`); };
      const literal = (a, b, message) => check(JSON.stringify(a) === JSON.stringify(b), message);
      for (const [label, run] of [['editor alone', alone], ['default owned', owned]]) {
        check(run.clipboardRows?.length === 34 && run.steps.length === 33, `${label} is missing a recorded boundary`);
        if (run.clipboardRows?.length !== 34) continue;
        const offset = run.clipboardRows[0].snapshot.state.layerRevision - clipboardReference.steps[0].state.layerRevision;
        check(Number.isSafeInteger(offset), `${label} has an invalid initial layer revision`);
        let document = clipboardReference.steps[0].document;
        for (const [index, row] of run.clipboardRows.entries()) {
          const recorded = clipboardReference.steps[index];
          if (recorded.document !== undefined) document = recorded.document;
          literal(row.call, recorded.call, `${label} changed call ${index}`);
          check(row.step === index, `${label} changed step order ${index}`);
          literal(row.figureClipboardPastes, recorded.figureClipboardPastes, `${label} counter/event/identity at ${index}`);
          if (index) {
            literal(row.result, recorded.result, `${label} result at ${index}`);
            literal(row.error, recorded.error, `${label} error at ${index}`);
          }
          // load has its existing page-local revision. Compare the recorded
          // absolute revision plus that one explicit initial offset; no tolerance.
          literal(row.snapshot.state, {...recorded.state, layerRevision: recorded.state.layerRevision + offset}, `${label} state at ${index}`);
          literal(row.events, recorded.events.map(event => Object.hasOwn(event, 'layerRevision')
            ? {...event, layerRevision: event.layerRevision + offset} : event), `${label} event ordering/details at ${index}`);
          check(row.snapshot.document === document, `${label} stored document at ${index}`);
        }
        check(run.clipboardUndone?.snapshot.document === run.clipboardRows[0].snapshot.document, `${label} full undo did not restore its document`);
        check(run.clipboardUndone?.snapshot.undo === '[]' && !run.clipboardUndone?.snapshot.state.canUndo, `${label} full undo left history`);
        check(run.clipboardUndone?.snapshot.clipboardPastes === run.clipboardRows.at(-1).figureClipboardPastes.after, `${label} undo changed the clipboard counter`);
        check(!run.clipboardUndone?.shadow.broken && run.clipboardUndone?.shadow.mismatches.length === 0
          && run.clipboardUndone?.shadow.rounded === 0, `${label} full undo has a shadow failure`);
      }
      if (alone.clipboardRows?.length === 34 && owned.clipboardRows?.length === 34) {
        for (let index = 0; index < 34; index++) literal(owned.clipboardRows[index].snapshot, alone.clipboardRows[index].snapshot,
          `paired literal frame/document/history/selection at ${index}`);
        for (let index = 1; index < 34; index++) literal(owned.clipboardRows[index].retainedEventClipboardPastes,
          alone.clipboardRows[index].retainedEventClipboardPastes, `paired retained event-clipboard counters after return at ${index}`);
        literal(owned.clipboardUndone?.snapshot, alone.clipboardUndone?.snapshot, 'paired literal full undo snapshot');
      }
      const success = [], validationErrors = [];
      const finite = value => Number.isFinite(value) && value >= 0;
      for (const row of owned.clipboardRows ?? []) {
        if (!['pasteSelection', 'duplicateSelection'].includes(row.clock?.method)) continue;
        const clock = row.clock, recorded = clipboardReference.steps[row.step];
        check(clock.calls === 1 && clock.globalCalls === 1, `unmatched root invocation ${row.step}`);
        if (recorded.error) {
          validationErrors.push({step: row.step, ...clock});
          check(clock.owned === 0 && clock.failedOwned === 1 && clock.globalFailedOwned === 1
            && ['editorMs', 'failedOwnedMs'].every(key => finite(clock[key]))
            && clock.callMs === 0 && clock.engineMs === 0 && clock.adoptMs === 0, `validation error accounting ${row.step}`);
        } else {
          const emptyDuplicateNoOp = row.step === 23 && clock.method === 'duplicateSelection'
            && Array.isArray(recorded.result) && recorded.result.length === 0 && recorded.events.length === 0
            && recorded.document === undefined && recorded.figureClipboardPastes.sameClipboard
            && recorded.figureClipboardPastes.before === recorded.figureClipboardPastes.after;
          success.push({step: row.step, ...clock, emptyDuplicateNoOp, zeroAdoptionClock: emptyDuplicateNoOp && clock.adoptMs === 0});
          check(clock.owned === 1 && clock.failedOwned === 0 && clock.globalFailedOwned === 0 && clock.editorMs === 0
            && ['callMs', 'engineMs'].every(key => Number.isFinite(clock[key]) && clock[key] > 0)
            && Number.isFinite(clock.adoptMs) && (emptyDuplicateNoOp ? clock.adoptMs >= 0 : clock.adoptMs > 0)
            && clock.engineMs >= clock.callMs, `successful default Native adoption ${row.step}`);
        }
      }
      const expectedSuccess = clipboardReference.steps.filter(row => row.call && ['pasteSelection', 'duplicateSelection'].includes(row.call[0]) && !row.error).length;
      const expectedErrors = clipboardReference.steps.filter(row => row.call?.[0] === 'pasteSelection' && row.error).length;
      check(success.length === expectedSuccess && validationErrors.length === expectedErrors && expectedErrors === 3,
        'missing successful calls or the three recorded validation errors');
      check(report.mode === 'classes' && report.failedOwned === expectedErrors && alone.report.failedOwned === 0
        && owned.clipboardUndone?.shadow.failedOwned === expectedErrors && !report.broken && !alone.report.broken,
      'default mode or aggregate failed-attempt accounting');
      const paste = report.byCall?.pasteSelection, editorPaste = alone.report.byCall?.pasteSelection;
      const expectedPasteCalls = clipboardScenario.steps.filter(call => call[0] === 'pasteSelection').length;
      check(paste?.calls === expectedPasteCalls && editorPaste?.calls === expectedPasteCalls && paste?.failedOwned === expectedErrors,
        'aggregate paste call counts');
      const candidateMs = paste ? (paste.engineMs ?? 0) + (paste.adoptMs ?? 0) + (paste.editorMs ?? 0) + (paste.failedOwnedMs ?? 0) : null;
      const editorMs = editorPaste?.editorMs ?? null;
      check(finite(candidateMs) && finite(editorMs), 'missing finite paste timings');
      figureClipboardProof = {binding: clipboardBinding, adopted: differences.length === 0,
        successfulInvocations: success, validationErrorInvocations: validationErrors,
        nativeOwnedEmptyDuplicateSteps: success.filter(row => row.emptyDuplicateNoOp).map(row => row.step),
        pasteSelection: {calls: paste?.calls ?? 0, nativeCallMs: paste?.callMs ?? null,
          engineParseValidationMs: paste?.engineMs ?? null, adoptMs: paste?.adoptMs ?? null,
          editorFallbackMs: paste?.editorMs ?? 0, failedOwnedMs: paste?.failedOwnedMs ?? 0, answerUtf16Units: paste?.answerBytes ?? null,
          matchedCommandDiagnostic: {candidateMs, editorMs, gate: false}},
        timingGate: false, fullClassRowAdvance: false, idleSpeedClaim: false,
        boundaries: {editorAlone: alone.clipboardRows, engineOwned: owned.clipboardRows},
        fullUndo: {editorAlone: alone.clipboardUndone, engineOwned: owned.clipboardUndone}};
      console.log(`Figure clipboard: ${success.length} successful default Native calls, ${validationErrors.length} recorded error replays; paste clocks diagnostic only`);
    }
    let uniqueProof = null;
    if (movieclipUnique) {
      if (alone.uniqueSnapshots.length !== 12 || owned.uniqueSnapshots.length !== 12 || alone.steps.length !== 12 || steps.length !== 12)
        differences.push('Make unique is missing one or more required boundaries.');
      for (let index = 0; index < 12; index++) {
        const [a, b] = [alone, owned].map(run => {
          const snapshot = run.uniqueSnapshots[index];
          return snapshot ? {...snapshot, document: JSON.parse(drawn(snapshot.document))} : null;
        });
        const found = near(a, b, `Make unique boundary ${index}`);
        if (found) differences.push(found);
      }
      const entry = report.byCall?.makeMovieclipUnique, editor = alone.report?.byCall?.makeMovieclipUnique;
      const positive = value => Number.isFinite(value) && value > 0;
      // The actual UI action calls finishCurrentEdit first. Its two quiet
      // commits are separately owned no-ops, not additional Unique calls.
      const preludesMatch = (call, owned) => call.globalCalls === 3
        && Object.values(call.preludeState.before).every(value => value === false)
        && Object.values(call.preludeState.after).every(value => value === false)
        && ['commitDrag', 'commitAnimationEdit'].every(name => {
          const delta = call.preludes[name].delta;
          return delta.calls === 1 && delta.failedOwned === 0 && delta.failedOwnedMs === 0
            && ['callMs', 'engineMs', 'adoptMs', 'editorMs'].every(key => Number.isFinite(delta[key]) && delta[key] >= 0)
            && (owned ? delta.editorMs === 0 && delta.engineMs >= delta.callMs
              : delta.callMs === 0 && delta.engineMs === 0 && delta.adoptMs === 0);
        });
      const adopted = report.mode === 'classes' && report.owned >= 6 && entry?.calls === 2 && editor?.calls === 2
        && alone.report.comparedByCall?.makeMovieclipUnique === 2 && positive(entry.callMs) && positive(entry.engineMs) && positive(entry.adoptMs)
        && positive(editor.editorMs) && !entry.editorMs && !report.failedOwned && !report.broken && !alone.report.broken
        && owned.uniqueCalls.length === 2 && alone.uniqueCalls.length === 2
        && owned.uniqueCalls.every((call, index) => call.tick === [2, 2.25][index] && call.calls === 1 && call.owned === 3 && call.failedOwned === 0 && preludesMatch(call, true)
          && positive(call.callMs) && positive(call.engineMs) && positive(call.adoptMs) && call.editorMs === 0)
        && alone.uniqueCalls.every((call, index) => call.tick === [2, 2.25][index] && call.calls === 1 && call.owned === 0 && call.failedOwned === 0 && preludesMatch(call, false) && positive(call.editorMs))
        && !refused.length && !alone.refused.length;
      if (!adopted) differences.push('Make unique did not complete two positive default Native adoptions without fallback.');
      const candidateMs = entry ? entry.engineMs + entry.adoptMs : null, editorMs = editor?.editorMs ?? null;
      const costPass = adopted && (candidateMs <= editorMs * 1.05 || (candidateMs - editorMs) / 2 <= .1);
      uniqueProof = {ticks: [2, 2.25], calls: entry?.calls ?? 0, adopted, nativeCallMs: entry?.callMs ?? null,
        engineParseValidationMs: entry?.engineMs ?? null, adoptMs: entry?.adoptMs ?? null,
        matchedCommandDiagnostic: {candidateMs, editorMs, costPass, extraAverageMs: adopted ? (candidateMs - editorMs) / 2 : null, gate: false},
        fullClassRowAdvance: false, idleSpeedClaim: false,
        invocationClocks: {editorAlone: alone.uniqueCalls, engineOwned: owned.uniqueCalls},
        outcomes: {editorAlone: alone.outcome, engineOwned: owned.outcome},
        boundaries: {editorAlone: alone.uniqueSnapshots, engineOwned: owned.uniqueSnapshots}};
      console.log(`Make unique: ${adopted ? 'two default Native adoptions' : 'adoption proof failed'}; matched command cost ${candidateMs} / editor ${editorMs} ms (diagnostic only); not a full-class or idle-speed result`);
    }
    let layerFoldersProof = null;
    if (layerFolders) {
      const boundaryCount = 21;
      if (alone.folderSnapshots.length !== boundaryCount || owned.folderSnapshots.length !== boundaryCount
          || alone.steps.length !== boundaryCount || steps.length !== boundaryCount)
        differences.push('Layer folders is missing one or more required boundaries.');
      for (let index = 0; index < boundaryCount; index++) {
        const [a, b] = [alone, owned].map(run => {
          const snapshot = run.folderSnapshots[index];
          return snapshot ? {...snapshot, ...Object.fromEntries(['document', 'frame', 'undo', 'redo']
            .map(key => [key, JSON.parse(drawn(snapshot[key]))]))} : null;
        });
        const found = near(a, b, `Layer-folder boundary ${index}`);
        if (found) differences.push(found);
      }
      const positive = value => Number.isFinite(value) && value > 0;
      const invocation = (call, native) => {
        if (!call || !call.quiet.before || !call.quiet.after || call.calls !== 1 || call.globalFailedOwned
            || call.failedOwned || call.failedOwnedMs) return false;
        let preludeCalls = 0;
        for (const name of ['commitDrag', 'commitAnimationEdit']) {
          const delta = call.preludes[name];
          if (!delta || ![0, ...(call.ui ? [1] : [])].includes(delta.calls) || delta.failedOwned || delta.failedOwnedMs
              || !['callMs', 'engineMs', 'adoptMs', 'editorMs'].every(key => Number.isFinite(delta[key]) && delta[key] >= 0)
              || (native ? delta.editorMs !== 0 || delta.engineMs < delta.callMs
                : delta.callMs !== 0 || delta.engineMs !== 0 || delta.adoptMs !== 0)) return false;
          preludeCalls += delta.calls;
        }
        return call.globalCalls === 1 + preludeCalls && call.owned === (native ? call.globalCalls : 0)
          && (native ? positive(call.callMs) && positive(call.engineMs) && positive(call.adoptMs) && call.editorMs === 0
            : positive(call.editorMs) && call.callMs === 0 && call.engineMs === 0 && call.adoptMs === 0);
      };
      const matchedCommandDiagnostics = folderMethods.map(method => {
        const entry = report.byCall?.[method], editor = alone.report?.byCall?.[method];
        const candidate = owned.folderCalls.find(call => call.method === method), oracle = alone.folderCalls.find(call => call.method === method);
        const adopted = entry?.calls === 1 && editor?.calls === 1 && alone.report.comparedByCall?.[method] === 1
          && invocation(candidate, true) && invocation(oracle, false)
          && ['commitDrag', 'commitAnimationEdit'].every(name => candidate.preludes[name].calls === oracle.preludes[name].calls);
        const candidateMs = entry ? entry.engineMs + entry.adoptMs : null, editorMs = editor?.editorMs ?? null;
        return {method, calls: entry?.calls ?? 0, adopted, nativeCallMs: entry?.callMs ?? null,
          engineParseValidationMs: entry?.engineMs ?? null, adoptMs: entry?.adoptMs ?? null, answerUtf16Units: entry?.answerBytes ?? null,
          matchedCommandDiagnostic: {candidateMs, editorMs, excessMs: candidateMs === null || editorMs === null ? null : candidateMs - editorMs, gate: false}};
      });
      const adopted = report.mode === 'classes' && owned.folderCalls.length === 5 && alone.folderCalls.length === 5
        && owned.folderCalls.map(call => call.method).join() === folderMethods.join()
        && alone.folderCalls.map(call => call.method).join() === folderMethods.join()
        && matchedCommandDiagnostics.every(command => command.adopted) && !report.failedOwned && !report.broken && !alone.report.broken
        && !refused.length && !alone.refused.length;
      if (!adopted) differences.push('Layer folders did not complete all five positive default Native adoptions without fallback.');
      layerFoldersProof = {adopted, methods: matchedCommandDiagnostics, uiAction: 'Advanced timeline Ungroup folder',
        timingGate: false, fullClassRowAdvance: false, idleSpeedClaim: false,
        invocationClocks: {editorAlone: alone.folderCalls, engineOwned: owned.folderCalls},
        outcomes: {editorAlone: alone.outcome, engineOwned: owned.outcome},
        boundaries: {editorAlone: alone.folderSnapshots, engineOwned: owned.folderSnapshots}};
      console.log(`Layer folders: ${adopted ? 'five default Native adoptions' : 'adoption proof failed'}; per-method clocks diagnostic only; not a full-class or idle-speed result`);
    }
    console.log(differences.length ? `DIFFERENT from the editor alone:\n  ${differences.join('\n  ')}`
      : `the same as the editor alone: document (${alone.outcome.after.document.length} bytes; ${rounded} numbers differ in their last digits), history (${alone.outcome.after.undo.length} entries), selection, and after undoing everything`);
    // What each call cost the editor alone and the engine owning it (its call and the
    // editor's adoption of the answer).
    const ms = value => (value ?? 0).toFixed(1);
    for (const [name, entry] of Object.entries(report.byCall ?? {})) {
      const editor = alone.report?.byCall?.[name];
      console.log(`  ${name} ×${entry.calls}: editor ${editor ? `${ms(editor.editorMs)} ms (×${editor.calls})` : 'not measured'}, engine ${ms(entry.engineMs)} ms + adoption ${ms(entry.adoptMs)} ms, ${Math.round((entry.answerBytes ?? 0) / Math.max(1, entry.calls))} bytes an answer; ${entry.failedOwned ?? 0} failed owner attempts (${ms(entry.failedOwnedMs)} ms)`);
    }
    if (alone.report) console.log(`  all owned calls: editor alone ${ms(alone.report.ownableMs)} ms, engine owning ${ms(report.ownedMs + report.ownableMs + report.failedOwnedMs)} ms (${ms(report.ownedMs)} ms adopted + ${ms(report.ownableMs)} ms JavaScript fallback + ${ms(report.failedOwnedMs)} ms failed attempts)`);
    for (const problem of [...alone.problems, ...problems].slice(0, 5)) console.log(`  page: ${problem}`);
    if (missingCalls(alone.report).length) console.log(`  required calls not compared: ${missingCalls(alone.report).join(', ')}`);
    const out = option('--out');
    if (out) await writeFile(path.resolve(out), JSON.stringify({steps, report, editorAlone: alone.report, differences, problems, editorAloneProblems: alone.problems,
      ...(movieclipUnique ? {uniqueProof} : {}), ...(layerFolders ? {layerFoldersProof} : {}), ...(figureClipboardPastes ? {figureClipboardProof} : {})}, null, 1));
    console.log(`  mode ${report.mode}`);
    failed = differences.length > 0 || report.mismatches.length > 0 || alone.report.mismatches.length > 0
      || (report.owned === 0 && !args.includes('--default')) || problems.length > 0 || alone.problems.length > 0
      || missingCalls(alone.report).length > 0 || (exactMath && (report.rounded > 0 || alone.report.rounded > 0));
  } else {
  const {report, steps, refused, problems} = await run('engineShadow=on');
  if (!report) throw new Error('Shadow editing was not installed (is ?engineShadow=on honoured?)');
  await saveCaptures(report, 'shadow');
  const out = option('--out');
  if (out) await writeFile(path.resolve(out), JSON.stringify({steps, refused, ...report, problems}, null, 1));
  console.log(`steps: ${steps.join('; ')}`);
  for (const line of refused) console.log(`  refused by the editor: ${line}`);
  console.log(`shadowed calls: ${report.calls} run in the engine, ${report.compared} the same (${report.rounded} of them but for last digits of the browser's Math), ${report.mismatches.length} different, ${report.skipped} not shadowed; ${report.sessions} sessions`);
  console.log(`time in those calls: editor ${report.jsMs.toFixed(1)} ms, engine ${report.engineMs.toFixed(1)} ms`);
  console.log(`  the same, by call: ${Object.entries(report.comparedByCall ?? {}).map(([name, count]) => `${name} ×${count}`).join(', ')}`);
  for (const [why, count] of Object.entries(report.ended)) console.log(`  session ended ${count}×: ${why}`);
  for (const [what, count] of Object.entries(report.unported)) console.log(`  not ported ${count}×: ${what}`);
  for (const mismatch of report.mismatches.slice(0, 8)) console.log(`  ${mismatch.call}(${JSON.stringify(mismatch.args ?? []).slice(0, 80)}):\n    ${mismatch.problems.join('\n    ').slice(0, 1500)}`);
  for (const problem of problems.slice(0, 5)) console.log(`  page: ${problem}`);
  failed = report.mismatches.length > 0 || report.compared === 0 || problems.length > 0
    || missingCalls(report).length > 0 || (exactMath && report.rounded > 0);
  if (missingCalls(report).length) console.log(`  required calls not compared: ${missingCalls(report).join(', ')}`);
  }
} finally {
  await browser.close();
  await stop();
}
process.exit(failed ? 1 : 0);
