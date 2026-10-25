import test from 'node:test';
import assert from 'node:assert/strict';
import {reverseGradientEndpoints} from '../src/editor-style-topology.mjs';

test('gradient reversal handles primary alpha-only aliases and does not alter solid segments',()=>{
  const original={color_rgb:[10,20,30],color_alpha:.2,secondary:{rgb:[40,50,60],a:102,unknown:'retained'}};
  const result=reverseGradientEndpoints(original);assert.deepEqual(result.color_rgb,[40,50,60]);assert.equal(result.transparency,102);assert.equal(result.color_alpha,.6);
  assert.deepEqual(result.secondary,{rgb:[10,20,30],a:204,unknown:'retained'});assert.equal(original.color_alpha,.2);
  const solid={color_rgb:[1,2,3],transparency:80};assert.equal(reverseGradientEndpoints(solid),solid);
});
