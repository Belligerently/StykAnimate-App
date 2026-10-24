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
import {hasPinnedIK} from '../src/ik.mjs';
import {sampleMotionPath} from '../src/motion-path.mjs';
import {engineBuilt} from './support/engine-node.mjs';
import {isAppMarker, recordedBounds} from '../engine/tools/edit-bounds-callbacks.mjs';

const folder = new URL('../engine/tests/edit/', import.meta.url);
const json = value => JSON.stringify(value, (key, item) => key === 'runtime_segments' || key === '_movieclipSample' ? undefined : item);
const plainJson = json;
const frameText = frame => { const {object_count, ...rest} = frame ?? {}; return json(rest); };
const state = editor => ({tick: editor.tick, frameIndex: editor.frameIndex, selectedUid: editor.selectedUid ?? null,
  selectedUids: [...editor.selectedUids], selectedJoint: editor.selectedJoint ?? -1, canUndo: editor.canUndo, canRedo: editor.canRedo,
  undoLabel: editor.undoLabel, redoLabel: editor.redoLabel, dirty: editor.dirty});
/** What the interface reads of an open motion-path edit (the trails sample `report` over `range`,
 * the arc's overlay and the pinned-origin modifier read the rest), as text; `null` with none open. */
const motionState = (editor, text) => {
  const drag = editor._drag;
  if (!drag?.motionPath) return 'null';
  const {kind, uid, handleId, joint} = drag.target, arc = editor.motionPathFalloff();
  const own = drag.report === editor.report;
  return text({target: {kind, uid, handleId, joint}, tick: drag.tick, falloff: drag.falloff ?? null, moved: Boolean(drag.moved), range: drag.range,
    pinned: (drag.grab?.baseline ?? drag.baseline).some(hasPinnedIK), arc: arc && {...arc, changed: [...arc.changed]},
    report: own ? 'the document' : drag.report, reported: editor.motionPathReport() === drag.report});
};
/** What the interface reads of an open figure-handle, Handle Edit Mode or segment-value gesture (the stage's
 * pivot marker reads the transform, the editor's own code the rest), as text; `null` with none open. */
const handleState = (editor, text) => {
  const drag = editor._drag;
  if (!drag || drag.motionPath) return 'null';
  const {figureHandle: figure, handleEdit: edit, segmentValueOptions: options} = drag;
  return text({figureHandle: figure && {uid: figure.uid, rotate: figure.rotate, scale: figure.scale, transform: figure.transform},
    handleEdit: edit && {uid: edit.uid, index: edit.index, point: edit.point, bend: edit.bend}, segmentValueOptions: options});
};
// The app's bounds callback is the only function the owner is given; other callback markers stay data
// (the editor refuses data where it needs a function, and so does the engine).
const decode = value => Array.isArray(value) ? value.map(decode) : value === null || typeof value !== 'object' ? value
  : isAppMarker(value) ? recordedBounds(value) : Object.hasOwn(value, '@array') ? Object.assign(decode(value['@array']), Object.fromEntries(Object.entries(value).filter(([key]) => key !== '@array')))
  : Object.fromEntries(Object.entries(value).map(([key, item]) => [key, decode(item)]));

/** An editor the engine owns: its document opened on the engine, which also holds the session that edits it. */
export async function ownedEditor(options, document, {verify = true} = {}) {
  const editor = new Editor(options).load(structuredClone(document));
  let pool = [];
  const owner = installEngineOwner(editor, {verify, draws: () => pool});
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
// the same tapes with it on every pose, the cache compared too.
const withCache = document => {
  const copy = structuredClone(document);
  for (const frame of [...copy.keyframes ?? [], ...Object.values(copy._styk?.frameSources ?? {})]) for (const object of frame.objects ?? []) object.runtime_segments ??= [];
  return copy;
};
const full = value => JSON.stringify(value, (key, item) => key === '_movieclipSample' ? undefined : item);
// With Pivot's cache on every pose (it changes nothing for the editor where poses lack it).
const variants = [{legacy: false}, {legacy: true}];
for (const {legacy} of variants) test(`the engine owning the document matches the editor alone on every recorded tape${legacy ? ', with legacy pose caches' : ''}`, {skip: !engineBuilt}, async () => {
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
    const {editor: owned, owner, setDraws} = await ownedEditor(options, document);
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
        sameText(motionState(owned, json), motionState(expected, json), `${label(index + 1)} ${method}: motion-path edit`);
        sameText(handleState(owned, json), handleState(expected, json), `${label(index + 1)} ${method}: handle gesture`);
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

// Property and effect-clip edits run in the session; the editor shows them as its own.
const recorded = name => {
  const scenario = JSON.parse(readFileSync(new URL(`${name}.scenario.json`, folder), 'utf8'));
  return {options: {demo: false, ...scenario.editor}, document: JSON.parse(readFileSync(new URL(scenario.document, folder), 'utf8'))};
};
/** An owned editor and an editor alone on the same document, and a call made on both. */
async function pair(name) {
  const {options, document} = recorded(name);
  const {editor: owned, owner} = await ownedEditor(options, document);
  const alone = new Editor(options).load(structuredClone(document));
  const both = (method, ...args) => { const results = [owned, alone].map(editor => editor[method](...args)); return results; };
  const agree = label => {
    sameText(json(owned.report), json(alone.report), `${label}: document`);
    same(state(owned), state(alone), `${label}: state`);
    sameText(frameText(owned.evaluatedFrame), frameText(alone.evaluatedFrame), `${label}: frame on show`);
  };
  return {owned, alone, owner, both, agree};
}

/** The session refuses `name` for the next `times` calls, as it does a state it does not support: the editor runs them. */
const refuse = (owner, name, times = 1) => {
  const session = owner.session(), ask = session.call.bind(session);
  let left = times;
  session.call = (method, ...rest) => method === name && left-- > 0 ? JSON.stringify({status: 'unported', reason: 'a refusal made for the test'}) : ask(method, ...rest);
};

test('a property edit runs in the session and shows on the editor as its own', {skip: !engineBuilt}, async () => {
  const {owned, alone, owner, both, agree} = await pair('property-scrub-sheet');
  both('select', 1, -1);
  both('beginAnimationEdit', {kind: 'figure', uid: 1}, 'scale');
  assert.ok(owned._animationEdit?.engine && owned._animationEdit.sheet, 'the session\'s edit is on the editor');
  assert.equal(owned.dirty, false);
  agree('begin');
  // The session sends the figures the preview changed, not the whole frame.
  const session = owner.session(), ask = session.call.bind(session), answers = [];
  session.call = (...parts) => { const text = ask(...parts); answers.push(JSON.parse(text)); return text; };
  both('previewAnimationEdit', 1.3);
  session.call = ask;
  assert.equal(answers.length, 1);
  assert.equal(answers[0].preview, undefined);
  assert.equal(answers[0].previewChanges.figures.length, 1);
  assert.ok(owned.evaluatedFrame.objects.length > 1);
  assert.equal(owned.dirty, true, 'a previewed change is unsaved');
  assert.equal(owned._animationEdit.changed, alone._animationEdit.changed);
  assert.equal(owned._animationEdit.value, 1.3);
  assert.ok(owned._sheetPropertyPreview, 'the preview is the frame on show');
  agree('preview');
  both('commitAnimationEdit');
  assert.equal(owned._animationEdit, null);
  assert.equal(owned._sheetPropertyPreview, null);
  agree('commit');
  both('beginAnimationEdit', {kind: 'figure', uid: 2}, 'opacity');
  both('previewAnimationEdit', 0.4);
  both('cancelAnimationEdit');
  assert.equal(owned._animationEdit, null);
  agree('cancel');
  assert.equal(owner.fallback, 0, 'no call ran in the editor');
});

test('an effect-clip edit runs in the session and shows on the editor as its own', {skip: !engineBuilt}, async () => {
  const {owned, owner, both, agree} = await pair('vfx-clips-sheet');
  const before = owned.report._styk.vfxClips;
  both('beginVfxClipEdit');
  assert.ok(owned._vfxClipEdit?.engine);
  assert.equal(owned._vfxClipEdit.before, before, 'the clips as they were');
  both('setVfxClipParameter', 'clip-glow', 'fx-glow', 'intensity', 1.5);
  agree('preview');
  assert.equal(owned.canUndo, false, 'a preview is not an entry');
  both('commitVfxClipEdit');
  assert.equal(owned._vfxClipEdit, null);
  agree('commit');
  both('beginVfxClipEdit');
  both('setVfxClip', 'clip-tint', {strength: 0.2});
  both('seek', 4);
  assert.equal(owned._vfxClipEdit, null, 'moving the playhead ends the edit');
  agree('seek');
  both('beginVfxClipEdit');
  both('setVfxClip', 'clip-tint', {strength: 0.4});
  both('undo');
  assert.equal(owned._vfxClipEdit, null);
  agree('undo');
  assert.equal(owner.fallback, 0, 'no call ran in the editor');
});

test('ending an edit that is not open does not ask the session', {skip: !engineBuilt}, async () => {
  const {owned, owner, both} = await pair('property-scrub-sheet');
  owner.capture = [];
  const calls = owner.native;
  assert.deepEqual(both('commitVfxClipEdit'), [false, false]);
  assert.deepEqual(both('cancelVfxClipEdit'), [false, false]);
  assert.deepEqual(both('commitAnimationEdit'), [false, false]);
  assert.deepEqual(both('cancelAnimationEdit'), [false, false]);
  both('seek', 2);
  assert.deepEqual(owner.capture, []);
  assert.equal(owner.native - calls, 5, 'the owner answered them');
  assert.equal(owned.tick, 2);
});

test('a call the editor runs takes over the edit the session holds', {skip: !engineBuilt}, async () => {
  const {owned, alone, owner, both, agree} = await pair('motion-path-sheet');
  both('select', 1, -1);
  both('beginAnimationEdit', {kind: 'figure', uid: 1}, 'scale');
  both('previewAnimationEdit', 1.4);
  assert.ok(owned._animationEdit.engine);
  // A camera drag the session refuses runs in the editor, which ends the property edit before it begins its own.
  refuse(owner, 'beginCameraDrag');
  both('beginCameraDrag');
  assert.equal(owned._animationEdit, null);
  assert.ok(owned._drag?.camera && !owned._drag.engine);
  assert.equal(owned.undoLabel, alone.undoLabel);
  agree('camera drag');
  both('cancelCameraDrag');
  agree('cancel');
  while (alone.canUndo) { both('undo'); agree('undo'); }
});

// Motion-path edits run in the session; the editor shows them as its own.
const head = {kind: 'node', uid: 1, handleId: 22, joint: 1};
/** What the stage draws while the edit is open: the playhead's pose under the trail (`currentFrame`, `display.geometry`), as text. */
const staged = editor => {
  if (!editor._drag?.motionPath) return null;
  const display = editor._drag.display, geometry = display?.geometry ?? editor.geometry;
  return {frame: frameText(display?.frame ?? editor.evaluatedFrame),
    figures: json([...geometry.objects].sort((a, b) => a[0] - b[0]).map(([slot, item]) => [slot, item.root, item.joints]))};
};
/** The owned editor shows what the editor alone does of the open motion-path edit, and samples the same trail. */
const sameLive = (owned, alone, label) => {
  sameText(motionState(owned, json), motionState(alone, json), `${label}: the edit as the interface reads it`);
  assert.deepEqual(staged(owned), staged(alone), `${label}: the pose the stage draws`);
  const target = owned._drag?.target;
  if (target) sameText(json(sampleMotionPath(owned.motionPathReport(), owned, target)), json(sampleMotionPath(alone.motionPathReport(), alone, target)), `${label}: trail`);
};

test('a motion-path edit runs in the session and shows on the editor as its own', {skip: !engineBuilt}, async () => {
  const {owned, alone, owner, both, agree} = await pair('motion-path-sheet');
  both('select', 1, -1);
  both('beginMotionPathEdit', head, 8);
  assert.ok(owned._drag?.engine && owned._drag.motionPath, 'the session\'s edit is on the editor');
  assert.equal(owned.motionPathReport(), owned.report, 'nothing has moved the trail yet');
  assert.equal(owned._drag.display, null, 'the stage draws the pose the document has');
  sameLive(owned, alone, 'begin');
  const start = sampleMotionPath(owned.motionPathReport(), owned, head).points.find(point => point.tick === 8);
  const session = owner.session(), ask = session.call.bind(session), answers = [];
  session.call = (...parts) => { const reply = ask(...parts); answers.push(JSON.parse(reply)); return reply; };
  both('updateMotionPathEdit', {x: 330, y: 300});
  both('updateMotionPathEdit', {x: 340, y: 310}, {stretch: true});
  session.call = ask;
  // The fixed part of the edit is told once; a move says how the edit stands and what its trail changed.
  assert.equal(answers[1].motionPath.target, undefined);
  assert.equal(answers[1].motionPath.moved, true);
  assert.ok(answers[1].motionPath.report.length > 0 && answers[1].motionPath.report.length < 8, 'the changed tracks and sources, not the document');
  assert.notEqual(owned.motionPathReport(), owned.report);
  assert.equal(owned.motionPathReport(), owned.motionPathReport(), 'one report until the trail moves again');
  assert.equal(owned.dirty, alone.dirty);
  sameLive(owned, alone, 'move');
  both('updateMotionPathEdit', {x: start.x, y: start.y});
  sameLive(owned, alone, 'back at the start');
  both('updateMotionPathEdit', {x: 330, y: 300});
  both('updateMotionPathEdit', {x: 340, y: 310}, {stretch: true});
  sameLive(owned, alone, 'moved again');
  both('updateMotionPathEdit', {x: 340, y: 310}, {stretch: true});
  sameLive(owned, alone, 'a move that changes nothing');
  both('commitMotionPathEdit');
  assert.equal(owned._drag, null);
  sameLive(owned, alone, 'commit');
  agree('commit');
  both('beginMotionPathEdit', head, 12);
  both('updateMotionPathEdit', {x: 300, y: 320}, {isolate: true});
  both('cancelMotionPathEdit');
  assert.equal(owned._drag, null);
  agree('cancel');
  while (alone.canUndo) { both('undo'); agree('undo'); }
  assert.equal(owner.fallback, 0, 'no call ran in the editor');
});

test('a motion-path edit on the playhead shows the pose under the trail to the stage', {skip: !engineBuilt}, async () => {
  const {owned, alone, both} = await pair('motion-path-sheet');
  both('select', 1, -1);
  both('seek', 8);
  both('beginMotionPathEdit', head, 8);
  assert.equal(owned._drag.display, null, 'the pose the document has until the trail moves');
  both('updateMotionPathEdit', {x: 330, y: 300});
  const display = owned._drag.display;
  assert.ok(display?.frame, 'the pose under the trail');
  assert.notEqual(frameText(display.frame), frameText(owned.evaluatedFrame), 'which is not the pose the document has');
  assert.equal(owned._drag.display, display, 'built once until the trail moves again');
  sameLive(owned, alone, 'move');
  both('updateMotionPathEdit', {x: 340, y: 310}, {stretch: true});
  assert.notEqual(owned._drag.display, display);
  sameLive(owned, alone, 'moved again');
  both('cancelMotionPathEdit');
  sameLive(owned, alone, 'cancel');
});

test('a motion-path edit that reshapes the arc shows its falloff on the editor', {skip: !engineBuilt}, async () => {
  const {owned, alone, owner, both, agree} = await pair('motion-path-falloff-sheet');
  both('select', 1, -1);
  both('beginMotionPathEdit', {kind: 'node', uid: 1, handleId: 0, joint: -1}, 10, {falloff: {radius: 5, shape: 'smooth'}});
  assert.ok(owned.motionPathFalloff().writes.length > 1);
  sameLive(owned, alone, 'begin');
  both('updateMotionPathEdit', {x: 250, y: 180});
  assert.ok(owned.motionPathFalloff().changed.size > 0);
  sameLive(owned, alone, 'move');
  both('setMotionPathFalloff', {radius: 8, shape: 'linear'});
  sameLive(owned, alone, 'a wider arc');
  both('setMotionPathFalloff', null);
  assert.equal(owned.motionPathFalloff(), null);
  sameLive(owned, alone, 'one frame');
  both('updateMotionPathEdit', {x: 262, y: 190});
  both('commitMotionPathEdit');
  agree('commit');
  while (alone.canUndo) { both('undo'); agree('undo'); }
  assert.equal(owner.fallback, 0, 'no call ran in the editor');
});

test('a motion-path call the session refuses is taken over by the editor with the calls that made the edit', {skip: !engineBuilt}, async () => {
  const {owned, alone, owner, both, agree} = await pair('motion-path-sheet');
  both('select', 1, -1);
  both('beginMotionPathEdit', head, 8);
  both('updateMotionPathEdit', {x: 330, y: 300});
  both('updateMotionPathEdit', {x: 340, y: 310}, {stretch: true});
  const session = owner.session(), ask = session.call.bind(session);
  session.call = (name, ...rest) => name === 'updateMotionPathEdit' ? JSON.stringify({status: 'unported', reason: 'a refusal made for the test'}) : ask(name, ...rest);
  both('updateMotionPathEdit', {x: 345, y: 305}, {stretch: true});
  session.call = ask;
  assert.ok(owned._drag.motionPath && !owned._drag.engine, 'the editor holds the edit now');
  assert.equal(owner.refused['updateMotionPathEdit: a refusal made for the test'], 1);
  sameLive(owned, alone, 'taken over');
  both('updateMotionPathEdit', {x: 350, y: 300}, {stretch: true});
  sameLive(owned, alone, 'move');
  both('commitMotionPathEdit');
  agree('commit');
  while (alone.canUndo) { both('undo'); agree('undo'); }
});

/** Calls of a recorded tape on both editors: what each raised, and the edit as the interface reads it. */
const tapeSteps = (name, from, to, owned, alone) => {
  const scenario = JSON.parse(readFileSync(new URL(`${name}.scenario.json`, folder), 'utf8'));
  const raised = [];
  for (const [index, [method, ...args]] of scenario.steps.entries().filter(([index]) => index >= from && index < to)) {
    const failures = [owned, alone].map(editor => { try { editor[method.replace(/^@/, '')](...args.map(decode)); } catch (error) { return error; } });
    assert.equal(failures[0]?.messageKey, failures[1]?.messageKey, `step ${index + 1} ${method}: error`);
    assert.deepEqual(failures[0]?.messageParams, failures[1]?.messageParams, `step ${index + 1} ${method}: what the error is made of`);
    if (failures[1]) raised.push(index + 1);
    sameLive(owned, alone, `step ${index + 1} ${method}`);
  }
  return raised;
};

test('a pointer move that is rejected leaves the motion-path edit open, in the session', {skip: !engineBuilt}, async () => {
  // The tape has a stretch the editor refuses (a full-circle bend) and pointers that are not finite.
  const {owned, alone, owner, agree} = await pair('motion-path-sheet');
  const raised = tapeSteps('motion-path-sheet', 0, Infinity, owned, alone);
  assert.ok(raised.length >= 4, `errors met: ${raised}`);
  agree('the tape');
  assert.deepEqual(Object.keys(owner.editor ?? {}).filter(name => /MotionPath/.test(name)), [], 'no motion-path call ran in the editor');
});

test('the editor takes over a motion-path edit after a rejected move as the editor alone would have it', {skip: !engineBuilt}, async () => {
  const {owned, alone, owner, both, agree} = await pair('motion-path-sheet');
  // Up to the full-circle bend the editor refuses (step 71): the move is rejected and the edit stays open.
  const raised = tapeSteps('motion-path-sheet', 0, 71, owned, alone);
  assert.deepEqual(raised.slice(-1), [71]);
  assert.ok(owned._drag.engine && owned._drag.motionPath, 'the session still holds the edit');
  // A call the editor runs ends it: the partial effects of the rejected move go with the edit.
  refuse(owner, 'beginCameraDrag');
  both('beginCameraDrag');
  assert.equal(owned._drag?.motionPath, undefined);
  agree('camera drag');
  both('cancelCameraDrag');
  agree('cancel');
  while (alone.canUndo) { both('undo'); agree('undo'); }
});

test('a call the editor runs ends the motion-path edit the session holds as the editor would', {skip: !engineBuilt}, async () => {
  const {owned, alone, owner, both, agree} = await pair('motion-path-sheet');
  both('select', 1, -1);
  both('beginMotionPathEdit', head, 8);
  both('updateMotionPathEdit', {x: 330, y: 300});
  both('updateMotionPathEdit', {x: 340, y: 310}, {stretch: true});
  // A camera drag the session refuses runs in the editor, which commits an open edit before it begins its own.
  refuse(owner, 'beginCameraDrag');
  both('beginCameraDrag');
  assert.equal(owned._drag?.motionPath, undefined);
  assert.equal(owned.undoLabel, alone.undoLabel);
  agree('camera drag');
  both('cancelCameraDrag');
  agree('cancel');
  while (alone.canUndo) { both('undo'); agree('undo'); }
});

test('redo under an open motion-path edit redoes and leaves the edit open, as the editor alone does', {skip: !engineBuilt}, async () => {
  const {owned, alone, both, agree} = await pair('motion-path-sheet');
  both('select', 1, -1);
  both('beginMotionPathEdit', head, 8);
  both('updateMotionPathEdit', {x: 330, y: 300});
  both('commitMotionPathEdit');
  both('undo');
  both('beginMotionPathEdit', head, 12);
  both('updateMotionPathEdit', {x: 300, y: 320});
  // The session refuses to redo under its edit: the edit becomes the editor's own first.
  assert.deepEqual(both('redo'), [true, true]);
  assert.ok(owned._drag?.motionPath && alone._drag?.motionPath, 'the edit is still open');
  agree('redo');
  both('updateMotionPathEdit', {x: 310, y: 330});
  both('commitMotionPathEdit');
  agree('commit after redo');
  while (alone.canUndo) { both('undo'); agree('undo'); }
});

// Figure-handle, Handle Edit Mode and segment-value gestures run in the session; the editor shows them as its own.
const HANDLE_CALLS = ['beginFigureHandleDrag', 'transformFigureHandle', 'commitFigureHandleDrag', 'cancelFigureHandleDrag', 'beginHandleEditDrag', 'editHandlePosition',
  'commitHandleEditDrag', 'cancelHandleEditDrag', 'beginSegmentValueEdit', 'setSegmentValues', 'pasteSegmentValues'];
const ranHere = owner => HANDLE_CALLS.filter(name => owner.editor?.[name]);
/** The gesture as the interface reads it, and what the segment-values dialog and the stage show beside it. */
const sameHandles = (owned, alone, label) => {
  sameText(handleState(owned, json), handleState(alone, json), `${label}: the gesture as the interface reads it`);
  if (alone.figure?.segments[alone.selectedJoint]) for (const options of [{}, {relative: true}]) {
    sameText(json(owned.segmentValues(options)), json(alone.segmentValues(options)), `${label}: segment values ${JSON.stringify(options)}`);
  }
  sameText(frameText(owned.evaluatedFrame), frameText(alone.evaluatedFrame), `${label}: frame on show`);
};
/** The pointer for an angle about the pivot of a figure-handle gesture, at a multiple of the distance it began at. */
const around = (editor, angle, factor = 1) => {
  const {origin, distance} = editor._drag.figureHandle.transform;
  return [origin.x + distance * factor * Math.cos(angle), origin.y + distance * factor * Math.sin(angle)];
};
/** Makes the session refuse the next call of `refused`; the returned function lets it answer again. */
const refusing = (owner, refused) => {
  const session = owner.session(), ask = session.call.bind(session);
  session.call = (name, ...rest) => name === refused ? JSON.stringify({status: 'unported', reason: 'a refusal made for the test'}) : ask(name, ...rest);
  return () => { session.call = ask; };
};

test('a figure-handle gesture runs in the session and shows its turns to the stage', {skip: !engineBuilt}, async () => {
  const {owned, alone, owner, both, agree} = await pair('figure-transform-sheet');
  both('seek', 3);
  both('select', 1, 2);
  both('beginFigureHandleDrag');
  assert.ok(owned._drag?.engine && owned._drag.figureHandle, 'the session\'s gesture is on the editor');
  assert.equal(owned._drag.figureHandle.transform.rotation, 0);
  sameHandles(owned, alone, 'begin');
  const drag = owned._drag, start = owned._drag.figureHandle.transform.lastAngle;
  // Round the pivot past half a turn and back: the turns are summed, not wrapped.
  for (const [turn, factor] of [[0.9, 1.2], [1.9, 0.8], [2.9, 1], [3.9, 1.3], [4.9, 0.7], [5.9, 1], [3.1, 1], [0.2, 1.5]]) {
    both('transformFigureHandle', ...around(owned, start + turn, factor));
    sameHandles(owned, alone, `turn ${turn}`);
  }
  assert.ok(Math.abs(owned._drag.figureHandle.transform.rotation) < 1, 'the turns came back to where they began');
  assert.equal(owned._drag, drag, 'the gesture is one object while it lasts');
  // The locks: Ctrl keeps the scale, Shift the rotation, the toolbar's options come with the call.
  both('transformFigureHandle', ...around(owned, start + 1, 1.4), {ctrlKey: true});
  both('transformFigureHandle', ...around(owned, start + 2, 0.6), {shiftKey: true});
  both('transformFigureHandle', ...around(owned, start + 2.5, 1.1), {rotate: false});
  sameHandles(owned, alone, 'locks');
  both('commitFigureHandleDrag');
  assert.equal(owned._drag, null);
  sameHandles(owned, alone, 'commit');
  agree('commit');
  both('beginFigureHandleDrag');
  both('transformFigureHandle', ...around(owned, start + 0.7, 1.2));
  both('cancelFigureHandleDrag');
  assert.equal(owned._drag, null);
  agree('cancel');
  // One move outside a gesture begins and commits its own.
  both('transformFigureHandle', 410, 260);
  assert.equal(owned._drag, null);
  agree('a single move');
  while (alone.canUndo) { both('undo'); agree('undo'); }
  assert.deepEqual(ranHere(owner), [], 'no figure-handle call ran in the editor');
});

test('a Handle Edit Mode gesture runs in the session and shows the handle on the editor', {skip: !engineBuilt}, async () => {
  const {owned, alone, owner, both, agree} = await pair('handle-edit-sheet');
  both('seek', 3);
  both('select', 1, 1);
  both('beginHandleEditDrag', {bend: true});
  assert.ok(owned._drag?.engine && owned._drag.handleEdit, 'the session\'s gesture is on the editor');
  assert.equal(owned._drag.handleEdit.bend, true);
  sameHandles(owned, alone, 'begin');
  const drag = owned._drag, {x, y} = owned._drag.handleEdit.point;
  for (const point of [[280, 240], [285, 250, {bend: false}], [x, y], [300, 260], [x + 1e-12, y]]) {
    both('editHandlePosition', ...point);
    sameHandles(owned, alone, `to ${point}`);
  }
  assert.equal(owned._drag, drag);
  both('commitHandleEditDrag');
  assert.equal(owned._drag, null);
  agree('commit');
  both('select', 3, 0);
  both('beginHandleEditDrag');
  both('editHandlePosition', 400, 330);
  both('cancelHandleEditDrag');
  assert.equal(owned._drag, null);
  agree('cancel');
  while (alone.canUndo) { both('undo'); agree('undo'); }
  assert.deepEqual(ranHere(owner), [], 'no handle-edit call ran in the editor');
});

test('a segment-value gesture runs in the session and keeps its options and its dialog\'s gesture', {skip: !engineBuilt}, async () => {
  const {owned, alone, owner, both, agree} = await pair('handle-commands-sheet');
  both('seek', 6);
  both('select', 2, 1);
  for (const options of [{editMode: true, relative: true}, {isolate: true}, {}]) {
    both('beginSegmentValueEdit', options);
    const drag = owned._drag;
    assert.ok(drag?.engine && drag.segmentValueOptions, 'the session\'s gesture is on the editor');
    assert.deepEqual(drag.segmentValueOptions, options);
    assert.equal(Boolean(drag.handleEdit), Boolean(options.editMode));
    sameHandles(owned, alone, `begin ${JSON.stringify(options)}`);
    for (const [values, more] of [[{angle: 30, length: 60}, options], [{bend: 15}, options], [{angle: -20}, {}]]) {
      both('setSegmentValues', values, more);
      assert.equal(owned._drag, drag, 'the dialog\'s gesture stays the same object');
      sameHandles(owned, alone, `${JSON.stringify(values)}`);
    }
    both(options.isolate ? 'cancelDrag' : 'commitDrag');
    assert.equal(owned._drag, null);
    agree('end');
  }
  // Pasting begins and commits its own.
  both('pasteSegmentValues', '12,5:3:70');
  both('pasteSegmentValues', '1:2:3');
  agree('paste');
  while (alone.canUndo) { both('undo'); agree('undo'); }
  assert.deepEqual(ranHere(owner), [], 'no segment-value call ran in the editor');
});

test('a seek or an undo ends a figure-handle or handle-edit gesture, and a changed selection cancels it, as the editor alone does', {skip: !engineBuilt}, async () => {
  const {owned, alone, owner, both, agree} = await pair('figure-transform-sheet');
  both('seek', 3);
  both('select', 1, 2);
  both('beginFigureHandleDrag');
  both('transformFigureHandle', ...around(owned, owned._drag.figureHandle.transform.lastAngle + 0.8, 1.3));
  both('seek', 5);
  assert.equal(owned._drag, null);
  agree('seek');
  both('select', 1, 1);
  both('beginHandleEditDrag');
  both('editHandlePosition', 300, 250);
  both('select', 2, 1);
  // The editor raises that the handle changed and cancels the gesture; the session's error is the editor's.
  const raised = editor => { try { editor.editHandlePosition(310, 255); } catch (error) { return error.messageKey; } };
  const [fromOwned, fromAlone] = [raised(owned), raised(alone)];
  assert.equal(fromAlone, 'errors.editorHandleEdit.theSelectedHandleChangedDuringItsGesture');
  assert.equal(fromOwned, fromAlone);
  assert.equal(owned._drag, null);
  agree('selection changed');
  both('select', 1, 1);
  both('beginHandleEditDrag');
  both('editHandlePosition', 305, 255);
  assert.deepEqual(both('undo'), [true, true]);
  assert.equal(owned._drag, null);
  agree('undo');
  while (alone.canUndo) { both('undo'); agree('undo all'); }
  assert.deepEqual(ranHere(owner), []);
});

/** The solved arcs of a Handle Edit Mode gesture, as the editor's code reads them to carry it on. */
const arcs = editor => editor._drag.handleEdit.states.map(state => ({slot: state.slot, vertex: state.vertex, indices: state.indices, moved: state.moved,
  lengths: state.lengths, bends: state.bends, drawn: state.g.segments.map(shape => shape.bend), before: state.before,
  incident: state.incident.map(shape => ({index: shape.index, id: shape.seg.segment_id, line: [0, 6].includes(shape.seg.primitive_code ?? (shape.seg.flags ?? 0) & 255),
    angle: shape.angle, bend: shape.bend, start: shape.start, end: shape.end}))}));

test('a figure-handle call the session refuses is taken over by the editor with the gesture\'s state', {skip: !engineBuilt}, async () => {
  const {owned, alone, owner, both, agree} = await pair('figure-transform-sheet');
  both('seek', 3);
  both('select', 1, 2);
  both('beginFigureHandleDrag');
  const start = owned._drag.figureHandle.transform.lastAngle;
  both('transformFigureHandle', ...around(owned, start + 2.4, 1.2));
  both('transformFigureHandle', ...around(owned, start + 4.1, 0.9));
  const restore = refusing(owner, 'transformFigureHandle');
  both('transformFigureHandle', ...around(owned, start + 5.3, 1.1));
  restore();
  assert.equal(owner.refused['transformFigureHandle: a refusal made for the test'], 1);
  assert.ok(owned._drag.figureHandle && !owned._drag.engine, 'the editor holds the gesture now');
  sameHandles(owned, alone, 'taken over');
  assert.equal(json(owned._drag.figureHandle.targets), json(alone._drag.figureHandle.targets), 'the figures that move');
  assert.ok(Math.abs(alone._drag.figureHandle.transform.rotation) > Math.PI, 'past half a turn, which the editor carried on');
  both('transformFigureHandle', ...around(owned, start + 6, 1));
  both('transformFigureHandle', ...around(owned, start + 6.5, 1.4), {shiftKey: true});
  sameHandles(owned, alone, 'move');
  both('commitFigureHandleDrag');
  agree('commit');
  while (alone.canUndo) { both('undo'); agree('undo'); }
});

test('a handle-edit call the session refuses is taken over by the editor with the gesture\'s state', {skip: !engineBuilt}, async () => {
  const {owned, alone, owner, both, agree} = await pair('handle-edit-sheet');
  both('seek', 3);
  both('select', 1, 1);
  both('beginHandleEditDrag', {bend: true});
  const {x, y} = owned._drag.handleEdit.point;
  both('editHandlePosition', 280, 240);
  both('editHandlePosition', 285, 250, {bend: false});
  const restore = refusing(owner, 'editHandlePosition');
  both('editHandlePosition', 290, 262);
  restore();
  assert.equal(owner.refused['editHandlePosition: a refusal made for the test'], 1);
  assert.ok(owned._drag.handleEdit && !owned._drag.engine, 'the editor holds the gesture now');
  sameHandles(owned, alone, 'taken over');
  sameText(json(arcs(owned)), json(arcs(alone)), 'the arcs the gesture solves');
  both('editHandlePosition', x, y);
  sameText(json(arcs(owned)), json(arcs(alone)), 'back at the start');
  both('editHandlePosition', 300, 270, {bend: true});
  sameHandles(owned, alone, 'move');
  sameText(json(arcs(owned)), json(arcs(alone)), 'moved again');
  both('commitHandleEditDrag');
  agree('commit');
  while (alone.canUndo) { both('undo'); agree('undo'); }
});

test('a segment-value call the session refuses is taken over by the editor with the gesture\'s state', {skip: !engineBuilt}, async () => {
  for (const options of [{editMode: true}, {isolate: true}]) {
    const {owned, alone, owner, both, agree} = await pair('handle-commands-sheet');
    both('seek', 6);
    both('select', 2, 1);
    both('beginSegmentValueEdit', options);
    both('setSegmentValues', {angle: 30, length: 60});
    const restore = refusing(owner, 'setSegmentValues');
    both('setSegmentValues', {bend: 15});
    restore();
    assert.equal(owner.refused['setSegmentValues: a refusal made for the test'], 1);
    assert.ok(owned._drag.segmentValueOptions && !owned._drag.engine, 'the editor holds the gesture now');
    sameHandles(owned, alone, `taken over ${JSON.stringify(options)}`);
    if (options.editMode) sameText(json(arcs(owned)), json(arcs(alone)), 'the arcs the gesture solves');
    both('setSegmentValues', {angle: -10, length: 45});
    sameHandles(owned, alone, 'next value');
    both('commitDrag');
    agree('commit');
    while (alone.canUndo) { both('undo'); agree('undo'); }
  }
});

// Rig edits and camera drags run in the session; the editor shows them as its own, and builds
// its own when it takes one over.
/** The frame a camera drag shows, as text. */
const previewText = editor => editor._sheetPropertyPreview ? frameText(editor._sheetPropertyPreview) : null;
/** The editor holds the gesture the session refused, as it would have begun it. */
const editorHolds = (owned, kind) => assert.ok(owned._drag?.[kind] && !owned._drag.engine, `the editor holds the ${kind} gesture`);

for (const name of ['rig-drag-sheet', 'rig-drag-native']) {
  test(`a rig edit runs in the session and shows on the editor as its own (${name})`, {skip: !engineBuilt}, async () => {
    const {owned, alone, owner, both, agree} = await pair(name);
    both('select', 1, 1);
    both('beginRigDrag', {});
    assert.ok(owned._drag?.engine && owned.dragging, 'the session\'s gesture is on the editor');
    assert.equal(Boolean(owned._authoredPoseEdit), Boolean(alone._authoredPoseEdit), 'a draft on a layer sheet, the keyframe itself on a native timeline');
    assert.equal(owned.dirty, false);
    agree('begin');
    both('editRigJoint', 340, 300);
    both('editRigJoint', 345, 310);
    assert.equal(owned.dirty, true, 'a preview is unsaved');
    agree('moves');
    both('commitRigDrag');
    assert.equal(owned._drag, null);
    assert.equal(owned.undoLabel, alone.undoLabel);
    agree('commit');
    // A rotation and a stretch, ended by the generic commit.
    both('select', 1, 0);
    both('beginRigDrag', {reposition: false});
    both('editRigJoint', 335, 235);
    both('editRigJoint', 318, 250, {stretch: true, isolate: true});
    agree('rotation');
    both('commitDrag');
    assert.equal(owned.undoLabel, alone.undoLabel);
    agree('commit by commitDrag');
    // The bend of a joint (the editor's bend reads the rig it began), and a cancel.
    both('select', 1, 4);
    both('beginRigDrag', {bend: true});
    both('bendJoint', 255, 262);
    both('bendJoint', 250, 265, {stretch: true});
    agree('bend');
    both('cancelRigDrag');
    assert.equal(owned._drag, null);
    agree('cancel');
    // Coordinates that are not finite are rejected before the edit is touched: it stays open, in the session.
    both('select', 1, 2);
    both('beginRigDrag', {});
    both('editRigJoint', 370, 320);
    const messages = [owned, alone].map(editor => { try { editor.editRigJoint('x', 0); } catch (error) { return error.messageKey; } return null; });
    assert.ok(messages[0] && messages[0] === messages[1], `the same error: ${messages}`);
    assert.ok(owned._drag?.engine, 'the session still holds the edit');
    agree('error');
    both('cancelDrag');
    agree('cancel by cancelDrag');
    while (alone.canUndo) { both('undo'); agree('undo'); }
    assert.equal(owner.fallback, 0, 'no call ran in the editor');
  });
}

test('a camera drag runs in the session and shows its frame on the editor', {skip: !engineBuilt}, async () => {
  const {owned, alone, owner, both, agree} = await pair('camera-commands-sheet');
  both('beginCameraDrag');
  assert.ok(owned._drag?.engine && owned.dragging, 'the session\'s gesture is on the editor');
  assert.equal(owned._sheetPropertyPreview, null, 'nothing moved yet');
  agree('begin');
  const session = owner.session(), ask = session.call.bind(session), answers = [];
  session.call = (...parts) => { const text = ask(...parts); answers.push(JSON.parse(text)); return text; };
  both('updateCameraDrag', {x: 10, y: 5});
  both('updateCameraDrag', {x: 20, scale: 1.2});
  session.call = ask;
  // The session tells the camera, not the frame.
  assert.equal(answers.length, 2);
  assert.equal(answers[1].preview, undefined);
  assert.deepEqual(answers[1].cameraPreview, alone._sheetPropertyPreview.camera);
  assert.ok(owned._sheetPropertyPreview, 'the preview is the frame on show');
  assert.equal(previewText(owned), previewText(alone));
  assert.equal(owned.dirty, alone.dirty);
  agree('moves');
  both('updateCameraDrag', {angle_rad: 0.1}, {fromStart: false});
  assert.equal(previewText(owned), previewText(alone));
  agree('a nudge');
  both('commitCameraDrag');
  assert.equal(owned._drag, null);
  assert.equal(owned._sheetPropertyPreview, null);
  agree('commit');
  both('beginCameraDrag', {indices: [0, 1], label: 'Pan camera'});
  both('updateCameraDrag', {x: -15});
  assert.equal(previewText(owned), previewText(alone));
  both('cancelCameraDrag');
  assert.equal(owned._sheetPropertyPreview, null);
  agree('cancel');
  // A drag that begins itself, and ends with the generic commit.
  both('updateCameraDrag', {y: 7});
  assert.ok(owned._drag?.engine);
  assert.equal(previewText(owned), previewText(alone));
  both('commitDrag');
  agree('commit by commitDrag');
  // A move that fails ends the drag.
  both('beginCameraDrag');
  assert.throws(() => owned.updateCameraDrag({bogus: 1}));
  assert.throws(() => alone.updateCameraDrag({bogus: 1}));
  assert.equal(owned._drag, null);
  assert.equal(owned._sheetPropertyPreview, null);
  agree('error');
  while (alone.canUndo) { both('undo'); agree('undo'); }
  assert.equal(owner.fallback, 0, 'no call ran in the editor');
});

test('an open camera drag is ended by the calls that end any gesture', {skip: !engineBuilt}, async () => {
  const {owned, alone, owner, both, agree} = await pair('camera-commands-sheet');
  for (const end of [['seek', 3], ['undo'], ['selectFrame', 1], ['setCameraValues', {x: 2}]]) {
    both('beginCameraDrag');
    both('updateCameraDrag', {x: 12});
    both(...end);
    assert.equal(owned._drag, null);
    assert.equal(alone._drag, null);
    assert.equal(owned._sheetPropertyPreview, null, `after ${end[0]}`);
    agree(end[0]);
    both('seek', 0);
  }
  while (alone.canUndo) { both('undo'); agree('undo'); }
  assert.equal(owner.fallback, 0, 'no call ran in the editor');
});

for (const name of ['rig-drag-sheet', 'rig-drag-native']) {
  test(`a rig edit the session refuses is taken over by the editor (${name})`, {skip: !engineBuilt}, async () => {
    const {owned, alone, owner, both, agree} = await pair(name);
    // Refused in the middle of the gesture: the editor replays what the session had.
    both('select', 1, 1);
    both('beginRigDrag', {});
    both('editRigJoint', 340, 300);
    both('editRigJoint', 345, 310);
    refuse(owner, 'editRigJoint');
    both('editRigJoint', 350, 305);
    editorHolds(owned, 'rig');
    agree('a step');
    both('editRigJoint', 352, 306);
    both('commitRigDrag');
    assert.equal(owned._drag, null);
    assert.equal(owned.undoLabel, alone.undoLabel);
    agree('commit');
    // Refused at the end: the editor commits the rig edit, not a plain pose.
    both('select', 1, 0);
    both('beginRigDrag', {reposition: false});
    both('editRigJoint', 335, 235);
    both('editRigJoint', 332, 238, {isolate: true});
    refuse(owner, 'commitDrag');
    both('commitDrag');
    assert.equal(owned._drag, null);
    assert.equal(owned.undoLabel, alone.undoLabel);
    agree('commit refused');
    // A bend and a cancel.
    both('select', 1, 4);
    both('beginRigDrag', {bend: true});
    both('bendJoint', 255, 262);
    refuse(owner, 'bendJoint');
    both('bendJoint', 250, 265, {stretch: true});
    editorHolds(owned, 'rig');
    agree('bend refused');
    refuse(owner, 'cancelRigDrag');
    both('cancelRigDrag');
    assert.equal(owned._drag, null);
    agree('cancel refused');
    while (alone.canUndo) { both('undo'); agree('undo'); }
    assert.ok(owner.fallback >= 4, 'the refused calls ran in the editor');
  });
}

test('a camera drag the session refuses is taken over by the editor', {skip: !engineBuilt}, async () => {
  const {owned, alone, owner, both, agree} = await pair('camera-commands-sheet');
  // Refused in the middle: the nudge adds to the moves before it, so they are replayed too.
  both('beginCameraDrag', {indices: [0, 1], label: 'Pan camera'});
  both('updateCameraDrag', {x: 10, y: 5});
  both('updateCameraDrag', {x: 20, scale: 1.2});
  both('updateCameraDrag', {angle_rad: 0.1}, {fromStart: false});
  refuse(owner, 'updateCameraDrag');
  both('updateCameraDrag', {y: 3}, {fromStart: false});
  editorHolds(owned, 'camera');
  assert.equal(previewText(owned), previewText(alone));
  agree('a nudge');
  both('commitCameraDrag');
  assert.equal(owned.undoLabel, alone.undoLabel);
  agree('commit');
  // Refused at the end.
  both('beginCameraDrag');
  both('updateCameraDrag', {x: 10});
  refuse(owner, 'commitCameraDrag');
  both('commitCameraDrag');
  assert.equal(owned._drag, null);
  assert.equal(owned.undoLabel, alone.undoLabel);
  agree('commit refused');
  // A call the session does not run during a camera drag belongs to the editor, which ends the drag first.
  both('beginCameraDrag');
  both('updateCameraDrag', {x: 3});
  both('addFigure');
  assert.equal(owned._drag, null);
  assert.equal(owned.undoLabel, alone.undoLabel);
  agree('another call');
  while (alone.canUndo) { both('undo'); agree('undo'); }
});

test('redo under an open camera drag or rig edit redoes and leaves the gesture open, as the editor alone does', {skip: !engineBuilt}, async () => {
  const camera = await pair('camera-commands-sheet');
  camera.both('setCameraValues', {x: 40});
  camera.both('undo');
  camera.both('beginCameraDrag');
  camera.both('updateCameraDrag', {x: 12});
  assert.deepEqual(camera.both('redo'), [true, true]);
  assert.ok(camera.owned._drag?.camera && camera.alone._drag?.camera, 'the gesture is still open');
  camera.agree('redo');
  camera.both('commitCameraDrag');
  camera.agree('commit after redo');
  const rig = await pair('rig-drag-sheet');
  rig.both('select', 1, 1);
  rig.both('beginRigDrag', {});
  rig.both('editRigJoint', 340, 300);
  rig.both('commitRigDrag');
  rig.both('undo');
  rig.both('beginRigDrag', {});
  rig.both('editRigJoint', 345, 310);
  assert.deepEqual(rig.both('redo'), [true, true]);
  assert.ok(rig.owned._drag?.rig && rig.alone._drag?.rig, 'the gesture is still open');
  rig.agree('redo');
  rig.both('commitRigDrag');
  rig.agree('commit after redo');
});
