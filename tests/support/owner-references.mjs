// The frozen references of the owner tests: what an editor the engine does not own produced for the calls an owned editor
// is compared with, recorded once into `tests/fixtures/owner-references/<test file>.json` by
// `node engine/tools/owner-reference-fixtures.mjs` (the way the edit tapes' `.reference.json` files are recorded). A test
// takes each reference by calling `take(label, produce)`: replaying (the tests' normal mode) returns the next recorded
// value and never calls `produce`; recording (`STYK_RECORD_OWNER_REFERENCES=<folder>`, the generator's mode) runs
// `produce` (the plain editor) and keeps what it returns. Either way the value is its JSON form, so the owned side is
// compared through `canonical` too.
import nodeTest from 'node:test';
import assert from 'node:assert/strict';
import {mkdirSync, readFileSync, writeFileSync} from 'node:fs';
import path from 'node:path';
import {fileURLToPath} from 'node:url';

const recordInto = process.env.STYK_RECORD_OWNER_REFERENCES || null;
/** Whether this run records the references (the generator's) instead of replaying them. */
const recording = recordInto !== null;
const fixtures = fileURLToPath(new URL('../fixtures/owner-references/', import.meta.url));
/** Texts at least this long are kept once per file and referred to by their place in `texts`. */
const LONG_TEXT = 160;

/** A value as the fixtures keep it: JSON (`undefined` is `null`), so a reference and what is compared with it are alike. */
export const canonical = value => value === undefined ? null : JSON.parse(JSON.stringify(value));

const encode = (value, texts) => {
  if (typeof value === 'string') {
    if (value.length < LONG_TEXT) return value;
    if (!texts.has(value)) texts.set(value, texts.size);
    return {$text: texts.get(value)};
  }
  if (Array.isArray(value)) return value.map(item => encode(item, texts));
  if (value !== null && typeof value === 'object') {
    assert.ok(!Object.hasOwn(value, '$text'), 'a reference value cannot have a key named $text');
    return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, encode(item, texts)]));
  }
  return value;
};
const decode = (value, texts) => {
  if (Array.isArray(value)) return value.map(item => decode(item, texts));
  if (value !== null && typeof value === 'object') {
    if (Object.hasOwn(value, '$text')) return texts[value.$text];
    return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, decode(item, texts)]));
  }
  return value;
};

/** The fixture's text, one line per test and one per reference (a reviewable diff), the same for the same references. */
function fixtureText(tests) {
  const texts = new Map();
  const lines = Object.keys(tests).sort().map(name => `  ${JSON.stringify(name)}: [\n${tests[name].map(entry => `   ${JSON.stringify(encode(entry, texts))}`).join(',\n')}\n  ]`);
  return `{\n "texts": [\n${[...texts.keys()].map(text => `  ${JSON.stringify(text)}`).join(',\n')}\n ],\n "tests": {\n${lines.join(',\n')}\n }\n}\n`;
}

/**
 * @param {string} testFile `import.meta.url` of a test file `<name>.test.mjs`; its references are `<name>.json`
 * @returns {{test: Function}} `test(name, options, body)`: a `node:test` test whose `body(take, t)` takes the references of
 *   the test called `name`
 */
export function ownerReferences(testFile) {
  const base = path.basename(fileURLToPath(testFile)).replace(/\.test\.mjs$/, '');
  const file = path.join(recording ? recordInto : fixtures, `${base}.json`);
  const recorded = {};
  let loaded = null;
  const stored = () => loaded ??= JSON.parse(readFileSync(path.join(fixtures, `${base}.json`), 'utf8'));
  if (recording) {
    process.on('exit', () => {
      if (!Object.keys(recorded).length) return;
      mkdirSync(path.dirname(file), {recursive: true});
      writeFileSync(file, fixtureText(recorded));
    });
  }

  /** The references of the test called `name`, taken in the order they were recorded. */
  function takes(name) {
    const entries = [];
    let read = 0;
    const wanted = recording ? null : (stored().tests[name] ?? assert.fail(`no references recorded for "${name}" (node engine/tools/owner-reference-fixtures.mjs)`)).map(entry => decode(entry, stored().texts));
    const take = (label, produce) => {
      if (!recording) {
        const entry = wanted[read++];
        assert.ok(entry, `"${name}": the reference "${label}" was not recorded (node engine/tools/owner-reference-fixtures.mjs)`);
        assert.equal(entry[0], label, `"${name}": the references were recorded in another order (node engine/tools/owner-reference-fixtures.mjs)`);
        return entry[1];
      }
      const keep = value => { const kept = canonical(value); entries.push([label, kept]); return kept; };
      const made = produce();
      return typeof made?.then === 'function' ? made.then(keep) : keep(made);
    };
    take.done = () => {
      if (recording) {
        assert.ok(!Object.hasOwn(recorded, name), `the test "${name}" is named twice`);
        recorded[name] = entries;
      } else assert.equal(read, wanted.length, `"${name}": ${wanted.length - read} recorded references were not taken`);
    };
    return take;
  }

  return {
    test(name, options, body) {
      if (typeof options === 'function') [options, body] = [{}, options];
      nodeTest(name, options, async t => {
        const take = takes(name);
        await body(take, t);
        take.done();
      });
    },
  };
}
