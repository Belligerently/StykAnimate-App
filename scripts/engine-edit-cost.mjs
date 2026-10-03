#!/usr/bin/env node
// What a pose gesture costs the engine and the JavaScript editor, in the real app
// (headless Chromium), on the same document, figure and joint (docs/engine/editing.md).
//
//   node scripts/engine-edit-cost.mjs <document.json | native file>... [--url http://127.0.0.1:47831]
//
// For each document: the figure with the most segments, posed by the first joint from a
// third of the way down its list whose pose drag changes it; four drags of 15 pointer
// moves each, through an edit session (as engine-owned editing calls it, the answer
// parsed) and through the editor (the time its change listeners take subtracted: both pay
// it). The first drag warms up and is not counted. Prints the median pose and commit.
import {spawn} from 'node:child_process';
import {readFileSync} from 'node:fs';
import path from 'node:path';
import {chromium} from '../tests/support/playwright.mjs';

const root = path.resolve(import.meta.dirname, '..');
const args = process.argv.slice(2);
const at = args.indexOf('--url');
const files = args.filter((arg, i) => !arg.startsWith('--') && (at < 0 || i !== at + 1));
if (!files.length) { console.error('usage: node scripts/engine-edit-cost.mjs <document.json | native file>... [--url <origin>]'); process.exit(2); }
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));

let origin = at < 0 ? null : args[at + 1].replace(/\/$/, ''), child = null;
if (!origin) {
  const port = 47000 + Math.floor(Math.random() * 900);
  child = spawn(process.execPath, ['scripts/serve.mjs', '--port', String(port)], {cwd: root, stdio: 'ignore',
    env: {...process.env, STYK_PYTHON: process.env.STYK_PYTHON ?? path.join(root, '.venv/bin/python')}});
  origin = `http://127.0.0.1:${port}`;
  for (let i = 0; i < 100; i++) { try { if ((await fetch(origin)).ok) break; } catch { /* not listening yet */ } await sleep(100); }
}
const browser = await chromium.launch();
try {
  const context = await browser.newContext({viewport: {width: 1440, height: 900}});
  await context.addInitScript(settings => localStorage.setItem('stykanimate.editor.settings.v1', JSON.stringify(settings)),
    {theme: 'light', locale: 'en', uiScale: 100, showHandles: true, rendererMode: 'canvas'});
  const page = await context.newPage();
  // Shadow mode: the editor stays in charge, and the engine holds the document.
  await page.goto(`${origin}/?engineShadow=on`);
  await page.waitForFunction(() => window.styk?.ready === true, null, {timeout: 60000});
  await page.evaluate(() => { document.querySelector('.styk-privacy [data-decline]')?.click(); for (const dialog of document.querySelectorAll('dialog[open]')) dialog.close(); });
  for (const file of files) {
    if (file.endsWith('.json')) {
      const text = readFileSync(path.resolve(file), 'utf8');
      await page.evaluate(text => { styk.editor.load(JSON.parse(text), 'cost'); styk.editor.markSaved(); }, text);
    } else {
      await page.setInputFiles('#file-input', path.resolve(file));
      await page.waitForFunction(() => styk.editor.report.figure_types.length > 0 && !styk.state.importing && (styk.editor.name ?? '').length > 0, null, {timeout: 300000});
    }
    for (let i = 0; i < 300; i++) { if (await page.evaluate(async () => Boolean(await globalThis.__stykEditShadow?.prepare()))) break; await sleep(100); }
    const result = await page.evaluate(() => {
      const e = styk.editor, handle = styk.state.renderer?.engineSide?.engine?.handle;
      if (!handle) return {error: 'the engine is not running'};
      const sizes = new Map(e.report.figure_types.map(f => [f.figure_type_id, f.segments.length]));
      const object = [...e.evaluatedFrame.objects].sort((a, b) => (sizes.get(b.figure_type_id) ?? 0) - (sizes.get(a.figure_type_id) ?? 0))[0];
      const uid = object.unique_id, segments = sizes.get(object.figure_type_id), definition = e.report.figure_types.find(f => f.figure_type_id === object.figure_type_id).segments;
      let joint = -1, p = null;
      for (let k = 0; k < segments && joint < 0; k++) {
        const j = (Math.floor(segments / 3) + k) % segments;
        e.select(uid, j);
        const point = e.geometry.objects.get(e.selectedSlot)?.joints?.get(definition[j].segment_id);
        if (!point) continue;
        e.beginDrag('Pose figure'); const changed = e.poseJoint(point.x + 15, point.y - 12) && e.poseJoint(point.x + 25, point.y - 5); e.cancelDrag();
        if (changed) { joint = j; p = point; }
      }
      if (joint < 0) return {error: 'no joint of the largest figure poses'};
      e.select(uid, joint);
      const view = {tick: e.tick, frameIndex: e.frameIndex, selectedUid: uid, selectedUids: [uid], selectedJoint: joint, activeLayerId: e.activeLayerId, autoKey: false};
      const session = handle.edit_session(JSON.stringify({...view, layerRevision: e.layerRevision, nextFrameIdentity: e._nextFrameIdentity, nextUid: e._nextUid,
        propertyChannels: Boolean(e.propertyChannels), layerTimeline: Boolean(e.layerTimeline)}));
      if (!session) return {error: 'no edit session'};
      const v = JSON.stringify(view), engine = [], engineCommit = [];
      let status = '';
      for (let d = 0; d < 4; d++) {
        session.call('beginDrag', '["Pose figure"]', v);
        for (let i = 0; i < 15; i++) {
          const started = performance.now(), answer = JSON.parse(session.call('poseJoint', JSON.stringify([p.x + 10 + i * 3 + d, p.y - 10 + i]), v));
          if (d) engine.push(performance.now() - started);
          status = answer.status === 'ok' ? 'ok' : `${answer.status}: ${answer.reason ?? answer.error}`;
        }
        const started = performance.now(); JSON.parse(session.call('commitDrag', '[]', v)); if (d) engineCommit.push(performance.now() - started);
      }
      session.free();
      let emitted = 0; const emit = e._emit;
      e._emit = function(kind, detail) { const started = performance.now(); try { return emit.call(this, kind, detail); } finally { emitted += performance.now() - started; } };
      const editor = [], editorCommit = [];
      try {
        e.select(uid, joint);
        for (let d = 0; d < 4; d++) {
          e.beginDrag('Pose figure');
          for (let i = 0; i < 15; i++) { const before = emitted, started = performance.now(); e.poseJoint(p.x + 10 + i * 3 + d, p.y - 10 + i); if (d) editor.push(performance.now() - started - (emitted - before)); }
          const before = emitted, started = performance.now(); e.commitDrag(); if (d) editorCommit.push(performance.now() - started - (emitted - before));
        }
      } finally { e._emit = emit; }
      const median = list => { list.sort((a, b) => a - b); return Number(list[list.length >> 1].toFixed(3)); };
      return {figures: e.evaluatedFrame.objects.length, segments, joint, status, pose: {editor: median(editor), engine: median(engine)}, commit: {editor: median(editorCommit), engine: median(engineCommit)}};
    });
    if (result.error) console.log(`${path.basename(file)}: ${result.error}`);
    else console.log(`${path.basename(file)} (${result.figures} figures, largest ${result.segments} segments, joint ${result.joint}, ${result.status}): pose editor ${result.pose.editor} ms, engine ${result.pose.engine} ms; commit editor ${result.commit.editor} ms, engine ${result.commit.engine} ms`);
  }
} finally {
  await browser.close();
  child?.kill();
}
