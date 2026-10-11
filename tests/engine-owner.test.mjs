// The engine owning the document (D19): every recorded tape gives the same document,
// selection and history with the engine as with the editor alone, after every step and
// after undoing everything. The owner checks the engine's own document against the read
// view after every call.
import test from 'node:test';
import assert from 'node:assert/strict';
import {readdirSync, readFileSync} from 'node:fs';
import {Editor} from '../src/editor.mjs';
import {withDraws} from '../src/editor-draws.mjs';
import {installEngineOwner} from '../src/engine/engine-owner.mjs';
import {engineBuilt, nodeEngine} from './support/engine-node.mjs';

const folder = new URL('../engine/tests/edit/', import.meta.url);
const json = value => JSON.stringify(value, (key, item) => key === 'runtime_segments' || key === '_movieclipSample' ? undefined : item);
const plainJson = json;
const frameText = frame => { const {object_count, ...rest} = frame ?? {}; return json(rest); };
const state = editor => ({tick: editor.tick, frameIndex: editor.frameIndex, selectedUid: editor.selectedUid ?? null,
  selectedUids: [...editor.selectedUids], selectedJoint: editor.selectedJoint ?? -1, canUndo: editor.canUndo, canRedo: editor.canRedo,
  undoLabel: editor.undoLabel, redoLabel: editor.redoLabel, dirty: editor.dirty});
const decode = value => Array.isArray(value) ? value.map(decode) : value === null || typeof value !== 'object' ? value
  : Object.hasOwn(value, '@array') ? Object.assign(decode(value['@array']), Object.fromEntries(Object.entries(value).filter(([key]) => key !== '@array')))
  : Object.fromEntries(Object.entries(value).map(([key, item]) => [key, decode(item)]));

/** An editor the engine owns, its session started on the editor's document as it is. */
export async function ownedEditor(options, document, {verify = true, mirrored = false} = {}) {
  const {engine, mirror} = await nodeEngine();
  const editor = new Editor(options).load(structuredClone(document));
  let pool = [];
  const stateOf = e => JSON.stringify({tick: e.tick, frameIndex: e.frameIndex, selectedUid: e.selectedUid ?? null,
    selectedUids: [...e.selectedUids], selectedJoint: e.selectedJoint ?? -1, activeLayerId: e.activeLayerId, autoKey: Boolean(e.autoKey),
    layerRevision: e.layerRevision, nextFrameIdentity: e._nextFrameIdentity, nextUid: e._nextUid,
    propertyChannels: Boolean(e.propertyChannels), layerTimeline: Boolean(e.layerTimeline)});
  // As in the app: the session starts on the mirrored document (the stage's), once it is current.
  if (mirrored) { mirror.reset(); await mirror.syncFrames(editor.report, editor.revision, {allFigures: true, allFrames: true, allSources: true}); }
  const owner = installEngineOwner(editor, {
    verify, draws: () => pool,
    start: e => mirrored ? (mirror.framesCurrent(e.report, e.revision) ? engine.handle.edit_session(stateOf(e)) : null)
      : engine.editSession(json(e.report), stateOf(e), e.revision),
  });
  return {editor, owner, setDraws: draws => { pool = draws; }};
}

const sameText = (actual, wanted, message) => {
  if (actual === wanted) return;
  let at = 0; while (actual[at] === wanted[at]) at++;
  assert.fail(`${message} at ${at}: engine …${actual.slice(Math.max(0, at - 70), at + 50)}… editor …${wanted.slice(Math.max(0, at - 70), at + 50)}…`);
};
const same = (actual, wanted, message) => {
  const differing = Object.keys(wanted).filter(key => JSON.stringify(actual[key]) !== JSON.stringify(wanted[key]));
  if (differing.length) assert.fail(`${message}: ${differing.map(key => `${key} ${JSON.stringify(actual[key])} (editor ${JSON.stringify(wanted[key])})`).join(', ')}`);
};
const scenarios = readdirSync(folder).filter(name => name.endsWith('.scenario.json') && (!process.env.OWNER_TAPE || name.startsWith(`${process.env.OWNER_TAPE}.`))).sort();
// Legacy Pivot poses carry an empty `runtime_segments` cache (styk-piv's legacy decoder):
// the same tapes with it on every pose, started from the mirror, the cache compared too.
const withCache = document => {
  const copy = structuredClone(document);
  for (const frame of [...copy.keyframes ?? [], ...Object.values(copy._styk?.frameSources ?? {})]) for (const object of frame.objects ?? []) object.runtime_segments ??= [];
  return copy;
};
const full = value => JSON.stringify(value, (key, item) => key === '_movieclipSample' ? undefined : item);
// As in the app: started from the mirror; with Pivot's cache on every pose (it changes
// nothing for the editor where poses lack it).
const variants = [{mirrored: false}, {mirrored: true, legacy: true}];
for (const {mirrored, legacy} of variants) test(`the engine owning the document matches the editor alone on every recorded tape${mirrored ? ', started from the mirror' : ''}${legacy ? ', with legacy pose caches' : ''}`, {skip: !engineBuilt}, async () => {
  const json = legacy ? full : plainJson;
  let tapes = 0, steps = 0, native = 0, fallback = 0;
  const editorCalls = {}, refusedCalls = {};
  const failures = [];
  for (const name of scenarios) {
    const scenario = JSON.parse(readFileSync(new URL(name, folder), 'utf8'));
    if (scenario.steps.some(call => !Array.isArray(call) || /^@(?!seek$|select$)/.test(call[0]))) continue;
    const loaded = typeof scenario.document === 'string' ? JSON.parse(readFileSync(new URL(scenario.document, folder), 'utf8')) : scenario.document;
    const document = legacy ? withCache(loaded) : loaded;
    const options = {demo: false, ...scenario.editor};
    let expected;
    try { expected = new Editor(options).load(structuredClone(document)); } catch { continue; }
    const {editor: owned, owner, setDraws} = await ownedEditor(options, document, {mirrored});
    for (const editor of [expected, owned]) editor.autoKey = Boolean(scenario.editor?.autoKey);
    let drawn = 0;
    const source = kind => { drawn++; return kind === 'uuid' ? `d0d0d0d0-0000-4000-8000-${String(drawn).padStart(12, '0')}` : kind === 'seed' ? Math.imul(drawn, 0x9e3779b1) >>> 0 : (Math.imul(drawn, 0x9e3779b1) >>> 0) / 2 ** 32; };
    const label = step => `${scenario.name ?? name} step ${step}`;
    try {
      for (const [index, call] of scenario.steps.entries()) {
        const [method, ...args] = call;
        const run = editor => editor[method.replace(/^@/, '')](...args.map(decode));
        let expectedError, ownedError, draws = [];
        try { ({draws} = withDraws(() => run(expected), source)); } catch (error) { expectedError = error.messageKey ?? error.message; draws = error.draws ?? []; }
        setDraws(draws.map(pair => [...pair]));
        let at = 0;
        try { withDraws(() => run(owned), () => draws[at++]?.[1]); } catch (error) { ownedError = error.messageKey ?? error.message; }
        assert.equal(ownedError, expectedError, `${label(index + 1)} ${method}: error`);
        sameText(json(owned.report), json(expected.report), `${label(index + 1)} ${method}: document`);
        same(state(owned), state(expected), `${label(index + 1)} ${method}: state`);
        // `object_count` is derived (the number of objects); whether the frame on show has it
        // depends on which of the editor's evaluation caches built that frame.
        sameText(frameText(owned.evaluatedFrame), frameText(expected.evaluatedFrame), `${label(index + 1)} ${method}: frame on show`);
        steps++;
      }
      let guard = 1000;
      while (expected.canUndo && guard--) { expected.undo(); owned.undo(); }
      assert.equal(owned.canUndo, false, `${label('undo all')}: the engine's history is as long`);
      sameText(json(owned.report), json(expected.report), `${label('undo all')}: document`);
      same(state(owned), state(expected), `${label('undo all')}: state`);
      tapes++;
    } catch (error) {
      if (process.env.OWNER_TAPE) console.log(error.stack, JSON.stringify(owner.refused));
      failures.push(`${String(error.message).includes(scenario.name ?? name) ? '' : (scenario.name ?? name) + ': '}${String(error.message).split('\n')[0].slice(0, 420)}` + (owner.broken ? ` [session: ${owner.broken.slice(0, 300)}]` : ''));
    } finally {
      native += owner.native; fallback += owner.fallback;
      for (const [key, count] of Object.entries(owner.editor ?? {})) editorCalls[key] = (editorCalls[key] ?? 0) + count;
      for (const [key, count] of Object.entries(owner.refused ?? {})) refusedCalls[key] = (refusedCalls[key] ?? 0) + count;
      owner.release();
    }
  }
  console.log(`# ${tapes} tapes / ${steps} steps; native ${native}, editor ${fallback}; ${failures.length} failing`);
  for (const failure of failures.slice(0, 40)) console.log(`#   ${failure}`);
  if (process.env.OWNER_COUNTS) {
    console.log(`# editor gestures: ${Object.entries(editorCalls).sort((a, b) => b[1] - a[1]).slice(0, 25).map(([k, v]) => `${k} ${v}`).join(', ')}`);
    console.log(`# refused: ${Object.entries(refusedCalls).sort((a, b) => b[1] - a[1]).slice(0, 40).map(([k, v]) => `${k} ${v}`).join(' | ')}`);
  }
  assert.equal(failures.length, 0);
});
