import test from 'node:test';
import assert from 'node:assert/strict';
import {nativeFigureDocument} from '../src/native-figure.mjs';
import {createDemo} from '../src/demo.mjs';

test('missing selection fails rather than exporting a different figure', () => {
  assert.throws(() => nativeFigureDocument(createDemo(), {figureUid: 999}), /Select a figure/);
});

