// Command-replay scenarios of the figure builder and rig commands (docs/engine/editing.md):
// segment styles, branch caps and circle fills, the figure's name and outline (one
// `native-properties` or `figure` history entry, or recoloured and keyed on the figure on show
// as the app does it). Run by engine/tools/edit-scenarios-range.mjs.
import {writeFileSync} from 'node:fs';
import {documentOf} from './fixture-kit.mjs';

const out = new URL('../tests/edit', import.meta.url).pathname;
const app = {layerTimeline: true, propertyChannels: true};
const scenario = (name, editor, steps, document) => writeFileSync(`${out}/${name}.scenario.json`, JSON.stringify({name, document, editor, steps}, null, 1) + '\n');

// A figure of every builder shape: a line with native flag bits (11), a line with a native
// gradient (22), a circle (33), a square line with a paint (44), a filled and a clear circle
// (55, 66), a translucent line (77); polygons over the first branch (one without an identity)
// and over the circles.
{
  const document = documentOf([[
    [11, 0, 90, -0.8, {flag_bits: {raw: 0, low_byte: 0, high_byte: 0, set_bits: [], is_static: false}}],
    [22, 11, 80, -1.6, {secondary: {rgb: [200, 40, 40], a: 0}}],
    [33, 22, 30, -1.4, {primitive_code: 1, segment_kind: 'circle', segment_type: 'circle', flags: 1}],
    [44, 0, 70, 2.4, {primitive_code: 6, segment_kind: 'squareLine', flags: 6,
      paint: {type: 'linear', angle: 0, stops: [{offset: 0, color: [43, 43, 43], opacity: 1}, {offset: 1, color: [10, 120, 240], opacity: 0.5}]}}],
    [55, 44, 60, 1.6, {primitive_code: 3, segment_kind: 'circleFill', segment_type: 'circle', flags: 3}],
    [66, 55, 20, 0.4, {primitive_code: 4, segment_kind: 'circleClear', segment_type: 'circle', flags: 4}],
    [77, 22, 30, -0.5, {transparency: 64, color_alpha: 1 - 64 / 255}],
  ]], () => [300, 220]);
  document.figure_types[0].polyfills = [
    {segment_ids: [0, 11, 22], color_rgb: [40, 80, 120], alpha_raw: 0, color_alpha: 1, z_order: 0, z_order_runtime_raw: -1},
    {polyfill_id: 5, segment_ids: [11, 22, 77], color_rgb: [200, 160, 40], alpha_raw: 32, color_alpha: 1 - 32 / 255, z_order: -1, z_order_runtime_raw: -2},
    {polyfill_id: 6, segment_ids: [44, 55, 66], color_rgb: [20, 200, 90], alpha_raw: 0, color_alpha: 1, z_order: -2, z_order_runtime_raw: -3},
  ];
  writeFileSync(`${out}/builder.document.json`, JSON.stringify(document) + '\n');
}
// Segment styles as the app edits them: shapes and the construction flag in the definition,
// colours recoloured and opacity and thickness keyed on the figure on show; branch caps and
// circle fills; the figure's name and outline; refusals; a pose gesture committed first.
scenario('builder-styles-sheet', app, [
  ['@select', 1, 2], ['convertSegment', 'circleFill'], ['convertSegment', 'circleFill'], ['convertSegment', 'bogus'], ['convertSegment', 6],
  ['@select', 1, 0], ['convertSegment', 'squareLine'], ['updateBuilderSegmentStyle', {static: true}], ['updateBuilderSegmentStyle', {static: 'yes'}],
  ['updateBuilderSegmentStyle', {thickness: 3, shape: 'line'}], ['updateBuilderSegmentStyle', {shape: 'circle'}, {branch: true}],
  ['updateBuilderSegmentStyle', {opacity: 0.5}], ['updateBuilderSegmentStyle', {opacity: 0.4}, {branch: true}], ['updateBuilderSegmentStyle', {thickness: 9}],
  ['updateBuilderSegmentStyle', {color: '#ff0000'}],
  ['@select', 1, 1], ['updateBuilderSegmentStyle', {gradient: {color: '#00ff00', opacity: 0.5}}],
  ['updateBuilderSegmentStyle', {paint: {type: 'radial', angle: 0, stops: [{offset: 0, color: [255, 0, 0], opacity: 1}, {offset: 1, color: [0, 0, 255], opacity: 0.5}]}}, {branch: true}],
  ['updateBuilderSegmentStyle', {bogus: 1}], ['updateBuilderSegmentStyle', null], ['updateBuilderSegmentStyle', {}, {branch: 'x'}], ['updateBuilderSegmentStyle', {opacity: 2}],
  ['updateBuilderSegmentStyle', {}], ['@select', 1, 5], ['updateBuilderSegmentStyle', {opacity: 1, thickness: 6}], ['@select', 1, 1], ['updateSegmentStyle', {shape: 'squareLine'}], ['updateSegmentBranchStyle', {static: true}],
  ['@select', 1, 3], ['setBranchCaps'], ['setBranchCaps', true], ['setBranchCaps', 'x'],
  ['@select', 1, 4], ['setBranchFill'], ['setBranchFill', 'circle'], ['setBranchFill', 'line'], ['@select', 1, 0], ['setBranchFill'],
  ['setPolygonOpacity', 1, 0.3], ['setPolygonOpacity', 1, 3],
  ['updateFigureStyle', {name: '  Hero  '}], ['updateFigureStyle', {name: ' '}], ['updateFigureStyle', {name: 'Hero'}],
  ['updateFigureStyle', {name: 'Hero', outlineWidth: 2.5, outlineColor: '#102030', outlineOpacity: 0.5}],
  ['@select', 1, -1], ['convertSegment', 'line'], ['setBranchCaps', true],
  ['@select', 1, 6], ['beginDrag', 'Pose joint'], ['rotateJoint', 0.3], ['convertSegment', 'circle'],
  ['undo'], ['undo'], ['undo'], ['redo'], ['undo'], ['undo'], ['undo'], ['undo'], ['redo'], ['redo'],
], 'builder.document.json');
// On a native timeline without property channels every style edit is the definition's: colours
// with their gradient ends and paints, polygons of a branch, a polygon's opacity, the outline.
scenario('builder-styles-native', {}, [
  ['@select', 1, 1], ['updateBuilderSegmentStyle', {color: '#ff0000'}], ['updateBuilderSegmentStyle', {color: [200, 40, 40], opacity: 1}],
  ['updateBuilderSegmentStyle', {gradient: {color: '#00ff00'}}], ['updateBuilderSegmentStyle', {gradient: {opacity: 0.6}}], ['updateBuilderSegmentStyle', {gradient: null}], ['updateBuilderSegmentStyle', {gradient: {opacity: 0.25}}],
  ['updateBuilderSegmentStyle', {gradient: {bogus: 1}}], ['updateBuilderSegmentStyle', {gradient: 5}], ['updateBuilderSegmentStyle', {gradient: false}],
  ['updateBuilderSegmentStyle', {opacity: 0.5}, {branch: true}],
  ['@select', 1, 0], ['updateBuilderSegmentStyle', {color: '#0000ff', opacity: 0.8}, {branch: true}], ['updateBuilderSegmentStyle', {opacity: 0.8}],
  ['@select', 1, 3], ['updateBuilderSegmentStyle', {color: '#123456'}], ['updateBuilderSegmentStyle', {gradient: {color: '#ffffff', opacity: 0.1}}], ['updateBuilderSegmentStyle', {gradient: null}],
  ['updateBuilderSegmentStyle', {paint: {type: 'linear', angle: 45, stops: [{offset: 1, color: [0, 0, 255], opacity: 0.5}, {offset: 0, color: [255, 0, 0.4], opacity: 1}]}}],
  ['updateBuilderSegmentStyle', {paint: {type: 'spiral'}}], ['updateBuilderSegmentStyle', {paint: {type: 'solid', stops: [{offset: 0, color: [1, 2, 3], opacity: 0.5}]}}],
  ['updateBuilderSegmentStyle', {thickness: 0}], ['updateBuilderSegmentStyle', {thickness: -1}],
  ['@select', 1, 6], ['updateBuilderSegmentStyle', {thickness: 0}], ['updateBuilderSegmentStyle', {thickness: 4, static: false}], ['updateBuilderSegmentStyle', {static: true, shape: 'circle'}], ['updateBuilderSegmentStyle', {static: false}],
  ['setPolygonOpacity', 0, 0.25], ['setPolygonOpacity', 9, 0.5], ['setPolygonOpacity', 0, 0.25], ['setPolygonOpacity', 2, 1], ['setPolygonOpacity', 2, 0.5],
  ['updateFigureStyle', {outlineWidth: 3}], ['updateFigureStyle', {outlineColor: 'red'}], ['updateFigureStyle', {outlineOpacity: 0.3}], ['updateFigureStyle', {color: 1}],
  ['updateFigureStyle', {outlineColor: [9, 8, 7], outlineWidth: 1e7}],
  ['undo'], ['undo'], ['undo'], ['undo'], ['redo'], ['redo'], ['undo'], ['undo'], ['undo'], ['undo'], ['undo'], ['redo'],
], 'builder.document.json');
// A figure whose colour is overridden is recoloured on its frame even without property channels.
scenario('builder-styles-override-native', {}, [
  ['@select', 1, -1], ['updateFiguresInFrames', {color_override_raw: 0x80ff0000}, [0]], ['@select', 1, 1], ['updateBuilderSegmentStyle', {color: '#00ff00'}],
  ['updateBuilderSegmentStyle', {opacity: 0.5}], ['updateBuilderSegmentStyle', {static: true, color: '#0000ff'}], ['undo'], ['undo'], ['redo'],
], 'builder.document.json');
