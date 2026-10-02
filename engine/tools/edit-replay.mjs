#!/usr/bin/env node
// Command-replay harness, JavaScript side (docs/engine/editing.md): runs a scenario's
// editor calls in the real JavaScript Editor and records, after every step, what a
// port must reproduce: the document's bytes, the selection, the playhead, history and
// the change events the interface listens to.
//
//   node engine/tools/edit-replay.mjs <scenario.json> <reference.json>
//   node engine/tools/edit-replay.mjs --all [--out <folder>]   # every scenario in engine/tests/edit/
//
// A scenario: {"name", "document": <path relative to the scenario | inline document>,
// "editor": {"propertyChannels", "layerTimeline"}, "steps": [["method", ...arguments], …]}.
// A step may also be ["@seek", tick] or ["@select", uid, joint] (the same as the method
// calls, spelled out for readability). Documents are compared as `JSON.stringify` of
// `editor.serialize()` (what a save writes), which is byte-stable.
import {createHash} from 'node:crypto';
import {readdirSync, readFileSync, writeFileSync, mkdirSync, existsSync} from 'node:fs';
import path from 'node:path';

const here = path.dirname(new URL(import.meta.url).pathname);
const repo = path.resolve(here, '../..');
const {Editor} = await import(`${repo}/src/editor.mjs`);

const sha = text => createHash('sha256').update(text).digest('hex');
/** Event details as plain data: numbers, strings, booleans, arrays and objects of them. */
const plain = (value, depth = 4) => {
  if (value === null || ['number', 'string', 'boolean'].includes(typeof value)) return value;
  if (value instanceof Set) return plain([...value], depth);
  if (typeof value !== 'object' || depth <= 0) return undefined;
  if (Array.isArray(value)) return value.map(item => plain(item, depth - 1)).filter(item => item !== undefined);
  return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, plain(item, depth - 1)]).filter(([, item]) => item !== undefined));
};

export function replay(scenario, base = repo) {
  const document = typeof scenario.document === 'string' ? JSON.parse(readFileSync(path.resolve(base, scenario.document), 'utf8')) : structuredClone(scenario.document);
  const editor = new Editor({demo: false, ...(scenario.editor ?? {})}).load(document, scenario.name ?? 'Scenario');
  const events = [];
  // The editor dispatches every change as one 'change' event whose detail names its kind.
  editor.addEventListener('change', event => events.push(plain(event.detail ?? null)));
  const state = () => ({
    tick: editor.tick, frameIndex: editor.frameIndex,
    selectedUid: editor.selectedUid ?? null, selectedUids: [...(editor.selectedUids ?? [])], selectedJoint: editor.selectedJoint ?? -1,
    canUndo: editor.canUndo, canRedo: editor.canRedo, undoLabel: editor.undoLabel, redoLabel: editor.redoLabel, dirty: editor.dirty,
  });
  let previous = JSON.stringify(editor.serialize());
  const steps = [{step: 0, call: null, state: state(), documentSha: sha(previous), document: previous, events: []}];
  scenario.steps.forEach((call, index) => {
    events.length = 0;
    const [name, ...args] = call;
    const method = name === '@seek' ? 'seek' : name === '@select' ? 'select' : name;
    let result, error = null;
    try { result = editor[method](...args); }
    catch (caught) { error = {message: caught.message, key: caught.messageKey ?? null}; }
    const text = JSON.stringify(editor.serialize());
    steps.push({step: index + 1, call, result: plain(result ?? null), error, state: state(), documentSha: sha(text), ...(text !== previous ? {document: text} : {}), events: events.slice()});
    previous = text;
  });
  return {scenario: scenario.name, steps};
}

const args = process.argv.slice(2);
if (args[0] === '--all') {
  const folder = path.join(repo, 'engine/tests/edit');
  const outAt = args.indexOf('--out'), out = outAt >= 0 ? path.resolve(args[outAt + 1]) : folder;
  mkdirSync(out, {recursive: true});
  for (const name of existsSync(folder) ? readdirSync(folder).filter(file => file.endsWith('.scenario.json')).sort() : []) {
    const scenario = JSON.parse(readFileSync(path.join(folder, name), 'utf8'));
    const reference = replay(scenario, folder);
    writeFileSync(path.join(out, name.replace('.scenario.json', '.reference.json')), JSON.stringify(reference) + '\n');
    const changed = reference.steps.filter(step => step.document && step.step > 0).length, failed = reference.steps.filter(step => step.error).length;
    console.log(`${scenario.name}: ${reference.steps.length - 1} steps, ${changed} changed the document, ${failed} refused`);
  }
} else if (args.length === 2) {
  const scenario = JSON.parse(readFileSync(args[0], 'utf8'));
  writeFileSync(args[1], JSON.stringify(replay(scenario, path.dirname(path.resolve(args[0])))) + '\n');
} else {
  console.error('usage: node engine/tools/edit-replay.mjs <scenario.json> <reference.json> | --all [--out <folder>]');
  process.exit(2);
}
