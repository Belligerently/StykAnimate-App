#!/usr/bin/env node
// Records what the editor's evaluated frame stores, for every editor-built fixture
// (engine/crates/styk-eval/tests/fixtures/{eval,ik,vfx,spatial}/*.json and the command-replay
// documents), into tests/fixtures/records/<name>/ for styk-eval's tests/records.rs.
//   node engine/tools/records-fixtures.mjs
// Byte-stable: the files change only when the editor's evaluation does.
import {execFileSync} from 'node:child_process';
import {readdirSync, rmSync} from 'node:fs';
import path from 'node:path';
const here = path.dirname(new URL(import.meta.url).pathname);
const fixtures = path.resolve(here, '../crates/styk-eval/tests/fixtures'), out = path.join(fixtures, 'records');
rmSync(out, {recursive: true, force: true});
const sources = [];
for (const group of ['eval', 'ik', 'vfx', 'spatial'])
  for (const file of readdirSync(path.join(fixtures, group)).sort())
    if (file.endsWith('.json') && !file.endsWith('.reference.json') && file !== 'vectors.json') sources.push([`${group}-${file.slice(0, -5)}`, path.join(fixtures, group, file)]);
for (const file of readdirSync(path.resolve(here, '../tests/edit')).sort())
  if (file.endsWith('.document.json')) sources.push([`edit-${file.slice(0, -14)}`, path.resolve(here, '../tests/edit', file)]);
// A Pivot-eased angle that winds past π over several keys: the editor unwraps it along the
// whole run of keys, so the stored tween sits whole turns from the one unwrapped per key.
{
  const {documentOf, limb} = await import('./fixture-kit.mjs'), {mkdtempSync, writeFileSync} = await import('node:fs'), {tmpdir} = await import('node:os');
  const document = documentOf([limb()]), [first] = document.keyframes;
  document.auto_easing = true;
  const wrap = angle => Math.atan2(Math.sin(angle), Math.cos(angle));
  document.keyframes = [0, 1, 2, 3, 4, 5].map(step => ({...structuredClone(first), duration_raw: 4,
    camera: {is_default: false, x: 0, y: 0, scale: 1, angle_rad: wrap(step * 2.2)},
    objects: first.objects.map(object => ({...structuredClone(object), angles: [wrap(0.8 + step * 2.5), wrap(2 - step * 2.9), wrap(step * 1.4)]}))}));
  const file = path.join(mkdtempSync(path.join(tmpdir(), 'styk-records-')), 'legacy-winding.json');
  writeFileSync(file, JSON.stringify(document));
  sources.push(['eval-legacy-winding', file]);
}
// Property channels (`channel-fixtures.mjs`), as the app holds them (a layer sheet's
// `legacyAnimation`) and on their native timeline (`_styk.animation`). Animation origins are not
// produced as records yet, and a movieclip's own channels are not in its host's records: those
// fixtures are left out.
const channelFixtures = readdirSync(path.join(fixtures, 'channels')).filter(file => file.endsWith('.sheet.json') && !file.includes('-origin') && !file.includes('-in-movieclip')).sort();
for (const file of channelFixtures) sources.push([file.slice(0, -11), path.join(fixtures, 'channels', file)]);
for (const file of channelFixtures) sources.push([`${file.slice(0, -11)}-native`, path.join(fixtures, 'channels', `${file.slice(0, -11)}.json`), '--native']);
// Property edits on a layer sheet (`_sheetPropertyEntries`): at every tick, the frame the
// one-key channels of these entries compose on.
{
  const {mkdtempSync, writeFileSync, readFileSync} = await import('node:fs'), {tmpdir} = await import('node:os');
  const folder = mkdtempSync(path.join(tmpdir(), 'styk-entries-'));
  // Written by hand: an older file's stack keyed with only some parameters, which a keyed
  // parameter fills in (`resolvedVfx`); the editor keeps such stacks as saved.
  {
    const document = JSON.parse(readFileSync(path.join(fixtures, 'channels', 'channels-vfx.sheet.json'), 'utf8'));
    for (const channel of document._styk.layerSheet.legacyAnimation.channels.filter(c => c.property === 'vfxStack'))
      for (const key of channel.keys) for (const effect of key.value) effect.params = Object.fromEntries(Object.entries(effect.params).slice(0, 1));
    const file = path.join(folder, 'channels-vfx-partial.json');
    writeFileSync(file, JSON.stringify(document));
    sources.push(['channels-vfx-partial', file]);
  }
  const edit = name => path.resolve(here, '../tests/edit', `${name}.document.json`), channels = name => path.join(fixtures, 'channels', `${name}.sheet.json`);
  const figure = uid => ({kind: 'figure', uid}), segment = (uid, handleId, figureTypeId) => ({kind: 'segment', uid, handleId, ...(figureTypeId === undefined ? {} : {figureTypeId})});
  const camera = {kind: 'camera'}, scene = {kind: 'scene'};
  const entry = (target, property, value, extra = {}) => ({target, property, value, ...extra});
  const sets = [
    // Every scalar kind on figures, segments, the camera and the scene; a joined figure's
    // position moves its root figure, a joined figure turns.
    ['entries-figures', edit('sheet-layers'), () => [
      entry(figure(1), 'scale', 1.3), entry(figure(2), 'opacity', 0.35), entry(figure(3), 'rotation', 0.7), entry(figure(2), 'x', 330), entry(figure(2), 'y', 210),
      entry(figure(1), 'color', 0xff3080c0), entry(figure(3), 'zIndex', -2), entry(figure(1), 'outlineColor', 0x2040f0), entry(figure(1), 'outlineWidth', 3.5),
      entry(figure(2), 'outlineOpacity', 0.5), entry(segment(1, 22), 'color', 0xe03020), entry(segment(1, 33), 'opacity', 0.6), entry(segment(2, 11), 'thickness', 9),
      entry(segment(1, 44), 'zIndex', 3), entry(camera, 'x', 40), entry(camera, 'scale', 1.25), entry(camera, 'rotation', -0.2),
      entry(scene, 'color', 0xff204060), entry(scene, 'backgroundId', 0),
    ]],
    // On a document whose channels already animate the same properties.
    ['entries-over-channels', channels('channels-figure'), () => [
      entry(figure(1), 'scale', 0.9), entry(figure(3), 'x', 140), entry(figure(3), 'rotation', -0.5), entry(figure(2), 'zIndex', 4), entry(figure(2), 'opacity', 0.8),
    ]],
    ['entries-camera-scene', channels('channels-camera-scene'), () => [entry(camera, 'y', -15), entry(camera, 'x', 25), entry(scene, 'color', null), entry(scene, 'backgroundId', 1)]],
    // Second colours of gradients, fill styles, polygons, a scoped segment.
    ['entries-styles', channels('channels-style'), document => {
      const polygon = document.figure_types.find(f => f.polyfills?.length), polygonId = polygon.polyfills[0].polyfill_id ?? 1;
      const target = {kind: 'polygon', uid: document.keyframes[0].objects.find(o => o.figure_type_id === polygon.figure_type_id).unique_id, polygonId, figureTypeId: polygon.figure_type_id};
      return [entry(segment(1, 22), 'gradientColor', 0x10c040), entry(segment(1, 22), 'gradientOpacity', 0.25), entry(segment(1, 33, 1), 'color', 0x203040, {exactTarget: true}),
        entry(segment(2, 11), 'paint', {type: 'linear', angle: 0.5, stops: [{offset: 0, color: [255, 0, 0], opacity: 1}, {offset: 1, color: [0, 0, 255], opacity: 0.5}]}),
        entry(target, 'color', 0xf0a020), entry(target, 'opacity', 0.4), entry(target, 'zIndex', -1),
        // An unscoped colour after the scoped one on the same segment: the scoped one wins.
        entry(segment(1, 33), 'color', 0x99aa00)];
    }],
    // Effect parameters, switches, colours given as a number and as a paint, the scope.
    ['entries-vfx', channels('channels-vfx'), () => {
      const blur = 'fx-00000000000040008000000000000001', glow = 'fx-00000000000040008000000000000002', grade = 'fx-00000000000040008000000000000003';
      return [entry(figure(1), `vfx1:blur:${blur}:radius`, 7), entry(figure(1), `vfx1:blur:${blur}:enabled`, 0), entry(figure(1), 'vfxScope', 0),
        entry(segment(1, 22, 1), `vfx1:glow:${glow}:color`, {type: 'radial', angle: 0, stops: [{offset: 0, color: [10, 20, 30], opacity: 1}, {offset: 1, color: [200, 100, 50], opacity: 1}]}, {exactTarget: true}),
        entry(scene, `vfx1:grade:${grade}:contrast`, 1.4), entry(scene, `vfx1:grade:${grade}:saturation`, 1.2)];
    }],
    // A parameter given before a whole stack still applies to that stack (stacks go first).
    ['entries-vfx-colour', channels('channels-vfx'), () => [entry(segment(1, 22, 1), 'vfx1:glow:fx-00000000000040008000000000000002:color', 0x30a0f0, {exactTarget: true}), entry(figure(1), 'vfxScope', 1),
      entry(figure(1), 'vfx1:blur:fx-00000000000040008000000000000001:radius', 3),
      entry(figure(1), 'vfxStack', [{version: 2, id: 'fx-00000000000040008000000000000001', type: 'blur', enabled: true, params: {radius: 20}}])]],
    // Poses given by value: a layer's, a joined figure's, the camera's and the scene's.
    ['entries-poses', edit('sheet-layers'), () => [entry({kind: 'layer', id: 'layer-2'}, 'pose', {frameId: 'frame-1'}), entry(figure(2), 'pose', {frameId: 'frame-2'}),
      entry(camera, 'pose', {frameId: 'frame-2'}), entry(scene, 'pose', {frameId: 'frame-1'}), entry(figure(1), 'scale', 1.1)]],
  ];
  for (const [name, file, build] of sets) {
    const list = path.join(folder, `${name}.json`);
    writeFileSync(list, JSON.stringify(build(JSON.parse(readFileSync(file, 'utf8')))));
    sources.push([name, file, '--entries', list]);
  }
}
for (const [name, file, ...options] of sources) execFileSync(process.execPath, [path.join(here, 'records-reference.mjs'), file, path.join(out, name), '--full', ...options], {stdio: ['ignore', 'ignore', 'inherit']});
console.log(`${sources.length} documents recorded`);
