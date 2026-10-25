// Record the JavaScript selection owners before their Rust counterparts are enabled in the app.
import './v8-pin.mjs';
import {writeFileSync} from 'node:fs';
import {documentOf, limb, createObject, createFrame} from './fixture-kit.mjs';

const out = new URL('../tests/edit/', import.meta.url);
const document = documentOf([limb(), limb(true)]);
const [a, b] = document.figure_types;
document.keyframes = [createFrame([
  createObject(a, 1, 100, 120), createObject(b, 2, 250, 120),
  createObject(a, 3, 400, 120), createObject(b, 4, 550, 120), createObject(a, 5, 700, 120),
], 1)];
writeFileSync(new URL('selection-sets.document.json', out), JSON.stringify(document) + '\n');
for (const [mode, editor] of [
  ['native', {}],
  ['sheet', {layerTimeline: true, propertyChannels: true}],
  ['autokey-sheet', {layerTimeline: true, propertyChannels: true, autoKey: true}],
]) {
  const name = `selection-sets-${mode}`;
  const steps = [
    ['select', null], ['selectSameDefinition'], ['invertSelection', []],
    ['invertSelection'], ['invertSelection'], ['selectSameDefinition', {candidates: null}],
    ['select', 1, 1], ['selectSameDefinition'], ['selectSameDefinition'],
    ['selectSameDefinition', {candidates: [5, 3, 3, 999]}],
    ['select', 2], ['selectSameDefinition', {add: true, candidates: [1, 4]}],
    ['selectMany', [5, 2]], ['selectSameDefinition', {add: true, candidates: [4, 3, 4, 999]}],
    ['selectSameDefinition', {add: true, candidates: []}], ['selectSameDefinition', {candidates: []}],
    ['selectMarquee', [5, 1, 5, '3', 3.5, 4294967299, 3]],
    ['selectMarquee', [1, 3, 5]], ['invertSelection', [5, 1, 5, '3', 3.5, 4294967299]], ['invertSelection', []],
    ['select', 2, 0], ['invertSelection', []],
    ['selectMany', [5, 1]], ['selectMarquee', [1, 3, 3], {mode: 'add'}],
    ['selectMarquee', [1, 3], {mode: 'replace', initial: [5, 1]}],
    ['selectMarquee', [1, 3], {mode: 'add', initial: [5, 1]}],
    ['selectMarquee', [1, 3], {mode: 'toggle', initial: [5, 1]}],
    ['selectMarquee', [1, 3], {mode: 'toggle', initial: [5, 1]}],
    ['selectMarquee', [1, 3], {mode: 'subtract', initial: [5, 1]}],
    ['selectMarquee', [], {mode: 'add', initial: [5, 1, 5, 999]}],
    ['selectMarquee', [1], {mode: 'replace', initial: null}],
    ['selectMarquee', [1], {mode: 'subtract', initial: []}],
    ['selectMarquee', [1], {mode: 'invalid'}], ['selectMarquee', [1], {mode: null}],
    ['selectBranch', 1, 1], ['selectSameDefinition'],
    ['toggleSegment', 1, 0], ['toggleSegment', 1, 1], ['invertSelection', [2]],
    ['selectBranch', 2, 1], ['selectMarquee', [4, 1]],
    ['selectMany', [1, 3]], ['moveSelection', 5, 2], ['invertSelection', [1, 2]],
    ['undo'], ['redo'], ['selectSameDefinition'],
    ['selectMarquee', [5, 2], {mode: 'toggle', initial: [5, 3]}],
    ['selectMarquee', [], {mode: 'replace'}], ['selectMarquee', [], {mode: 'replace'}],
  ];
  writeFileSync(new URL(`${name}.scenario.json`, out), JSON.stringify({name, document: 'selection-sets.document.json', editor, steps}, null, 1) + '\n');
  console.log(`${name}: ${steps.length} steps`);
}
