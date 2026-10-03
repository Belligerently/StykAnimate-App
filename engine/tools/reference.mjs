#!/usr/bin/env node
// Dumps what the current JavaScript editor evaluates for a document, so the
// Rust engine can be checked against it primitive by primitive.
//
//   node tools/reference.mjs <file.styk|decoded.json> <out.json> [--ticks 0,5,9] [--every N] [--max N] [--vfx]
//
// Pivot .piv files go through the repository's Python codec first
// (tools/decode-piv.sh), exactly like the desktop app's import service.
//
// --vfx adds, per tick, the effects the editor resolves (`frames[].vfx`): the
// stacks the renderers hand to the effect executors, the generated effects,
// and the clips the inspector shows as active (docs/engine/vfx.md).
import {readFileSync, writeFileSync} from 'node:fs';
import {fileURLToPath} from 'node:url';
import path from 'node:path';

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, '../..');
const {decodeDocument, normalizeDecodedDocument} = await import(`${repo}/src/native-container.mjs`);
const {Editor} = await import(`${repo}/src/editor.mjs`);

const [input, output, ...rest] = process.argv.slice(2);
if (!input || !output) {
  console.error('usage: node tools/reference.mjs <file.styk|decoded.json> <out.json> [--ticks a,b] [--every N] [--max N] [--vfx]');
  process.exit(2);
}
const option = name => { const i = rest.indexOf(name); return i < 0 ? undefined : rest[i + 1]; };
const effects = rest.includes('--vfx') ? await effectDumper() : null;

let document;
if (/\.(styk|stkz|stykmov)$/i.test(input)) {
  const kind = /\.(styk|stkz|stykmov)$/i.exec(input)[1].toLowerCase();
  const decoded = await decodeDocument(new Uint8Array(readFileSync(input)), kind);
  document = normalizeDecodedDocument(decoded.document, kind);
} else {
  document = JSON.parse(readFileSync(input, 'utf8'));
}

// Same editor configuration as src/app.mjs.
const started = performance.now();
const editor = new Editor({demo: false, propertyChannels: true, layerTimeline: true});
editor.load(document, path.basename(input));
const loadMs = performance.now() - started;
const total = editor.totalTicks;

let ticks;
if (option('--ticks')) ticks = option('--ticks').split(',').map(Number);
else {
  const every = Number(option('--every') ?? 1);
  ticks = [];
  for (let t = 0; t < total; t += every) ticks.push(t);
}
const max = Number(option('--max') ?? Infinity);
ticks = ticks.filter(t => t >= 0 && t < total).slice(0, max);

const round = v => typeof v === 'number' ? Math.round(v * 1e6) / 1e6 : v;
const pt = p => p ? [round(p.x), round(p.y)] : null;
const frames = [];
let evalMs = 0;
for (const tick of ticks) {
  const t0 = performance.now();
  editor.seek(tick);
  const frame = editor.evaluatedFrame;
  const geometry = editor.geometry;
  const drawables = geometry.drawables;
  evalMs += performance.now() - t0;
  // Evaluated meshes (docs/engine/spatial.md): each once per tick, in the order
  // their first drawable appears; a drawable names its mesh and its faces.
  const meshes = [], meshIds = new Map();
  const meshId = mesh => {
    if (!meshIds.has(mesh)) {
      meshIds.set(mesh, meshes.length);
      const n = mesh.vertexCount, list = values => Array.from(values, round);
      meshes.push({vertexCount: n, positions: list(mesh.positions.subarray(0, n * 2)), depth: list(mesh.depth.subarray(0, n)), colors: mesh.colors.map(c => [...c]),
        bounds: mesh.bounds ? [mesh.bounds.x0, mesh.bounds.y0, mesh.bounds.x1, mesh.bounds.y1].map(round) : null, zMin: round(mesh.zMin), zMax: round(mesh.zMax)});
    }
    return meshIds.get(mesh);
  };
  const items = drawables.map(d => {
    const item = {
      slot: d.slot, uid: d.obj?.unique_id, type: d.type,
      kind: d.seg?.segment_kind ?? (d.type === 'poly' ? 'poly' : undefined),
      seg: d.seg?.segment_id, poly: d.type === 'poly' ? d.index : undefined,
      z: round(d.z), rgb: d.rgb, alpha: round(d.alpha), opacity: round(d.opacity),
    };
    if (d.type === 'mesh') item.mesh = {id: meshId(d.spatialMesh), index: d.index, count: d.count,
      triangles: Array.from(d.triangles.subarray(0, d.count * 3)), materials: Array.from(d.materials.subarray(0, d.count))};
    else if (d.type === 'poly') {
      item.points = d.points.map(pt);
      // A depth-tested surface: one depth per point.
      if (d.depthValues) item.depth = d.depthValues.map(round);
    } else {
      item.start = pt(d.start); item.end = pt(d.end);
      item.thickness = round(d.thickness); item.length = round(d.length);
      item.angle = round(d.angle); item.bend = round(d.bend);
      if (Math.abs(d.bend ?? 0) > 1e-7) item.points = d.points.map(pt);
    }
    if (d._movieclipOwner || d._movieclipPart !== undefined) item.movieclip = true;
    return item;
  });
  const objects = [];
  for (const [slot, g] of geometry.objects) {
    if (!g) continue;
    objects.push({slot, uid: g.obj.unique_id, figure: g.fig?.figure_type_id, root: pt(g.root),
      joints: [...g.joints].map(([id, p]) => [id, ...pt(p)])});
  }
  frames.push({
    tick,
    camera: frame.camera ?? null,
    background: {id: frame.background_id ?? 0, b: frame.background_id_b ?? 0, mix: frame.background_mix ?? 0},
    objects, drawables: items,
    ...(meshes.length ? {meshes} : {}),
    ...(effects ? {vfx: effects(editor.report, frame, geometry, tick)} : {}),
  });
}

writeFileSync(output, JSON.stringify({
  file: path.basename(input), version: editor.report.version,
  canvas: [editor.report.canvas_width, editor.report.canvas_height], fps: editor.report.fps,
  totalTicks: total, loadMs: round(loadMs), evalMsPerFrame: round(evalMs / Math.max(1, ticks.length)),
  frames,
}));
console.error(`${path.basename(input)}: ${ticks.length} frames of ${total}, load ${loadMs.toFixed(0)} ms, ` +
  `${(evalMs / Math.max(1, ticks.length)).toFixed(2)} ms per evaluated frame`);

/**
 * What the editor resolves for effects at one tick, as the renderers hand it to
 * the effect executors (src/renderer.mjs, src/playback-worker.mjs):
 *
 * - `scene`: `activeVfx(vfxStack(frame))`, the camera's stack;
 * - `objects`: per slot with anything to draw, the stack of the object's own
 *   artwork (`vfxStack(obj)`, or a movieclip container's owner effects, without
 *   what its connected group applies), its connected group from
 *   `connectedVfxGeometry` (`attached`: the group's stack and the slots of its
 *   drawn parts) and its node stacks by handle id (`segmentVfx`);
 * - `roots`: the connected groups, in the order they are composited;
 * - `layerHidden` / `empty`: slots on a hidden layer / without artwork, which
 *   no group draws;
 * - `generators`: generated effects inside their clip's range, with the
 *   parameters, opacity and start `collectLayers` (src/vfx-generators.mjs) reads;
 * - `activeClips`: the clips the inspector lists "At playhead" (`vfxClipActiveAt`).
 *
 * Each stack carries its padding (`vfxPadding`, `vfxSamplingPadding` at fixed
 * scales and the canvas extent) and `vfxWorkingSlots`.
 */
async function effectDumper() {
  const {activeVfx, vfxStack, resolvedVfx, vfxPadding, vfxSamplingPadding, vfxWorkingSlots} = await import(`${repo}/src/vfx.mjs`);
  const {vfxRenderFrame, vfxClips, vfxClipActiveAt, vfxFadeAt, sampleVfxClip} = await import(`${repo}/src/vfx-clips.mjs`);
  const {connectedVfxGeometry} = await import(`${repo}/src/vfx-connected.mjs`);
  const {isGeneratorType} = await import(`${repo}/src/vfx-registry.mjs`);
  const {generatorClipActiveAt} = await import(`${repo}/src/vfx-generators.mjs`);
  const {layerVisible} = await import(`${repo}/src/layers.mjs`);
  const {movieclipTypes} = await import(`${repo}/src/movieclips.mjs`);
  const effect = e => ({id: e.id, type: e.type, params: e.params});
  return (report, frame, geometry, tick) => {
    const extent = Math.max(report.canvas_width, report.canvas_height);
    const stack = raw => {
      const active = activeVfx(raw), slots = vfxWorkingSlots(active);
      if (slots.gpu.join() !== slots.cpu.join()) throw new Error('vfxWorkingSlots: GPU and CPU slots differ');
      return {effects: active.map(effect), slots: slots.gpu,
        padding: [vfxPadding(active, 1, extent), vfxPadding(active, 2.5, extent), vfxSamplingPadding(active, 1, extent), vfxSamplingPadding(active, .75, extent)]};
    };
    const rendered = vfxRenderFrame(report, frame), grouped = connectedVfxGeometry(geometry, frame, report);
    const assemblies = grouped.connectedVfxAssemblies ?? new Map(), clipTypes = movieclipTypes(report);
    // A group's items keep their member's identity (movieclip content takes its instance's).
    const slotOf = new Map(rendered.objects.map((o, slot) => [o.unique_id, slot]));
    const objects = [], layerHidden = [], empty = [];
    rendered.objects.forEach((o, slot) => {
      const g = geometry.objects.get(slot), container = clipTypes.has(o.figure_type_id);
      if (!layerVisible(report, o.unique_id)) layerHidden.push(slot);
      if (!g?.drawables?.length) empty.push(slot);
      let own = container ? g?.drawables?.[0]?._movieclipOwner?.effects ?? vfxStack(o) : vfxStack(o), attached;
      const items = assemblies.get(slot);
      if (items) {
        // The root's own artwork keeps what the group does not apply (connectedVfxGeometry).
        const mine = items.find(item => item.obj.unique_id === o.unique_id);
        const connected = items[0]?._movieclipOwner.effects ?? o._vfxConnectedEffects ?? vfxStack(o), ids = new Set(connected.map(e => e.id));
        own = mine ? (container ? mine._movieclipInner?.owner?.effects ?? [] : vfxStack(mine.obj)) : own.filter(e => !ids.has(e.id));
        attached = {...stack(connected), members: [...new Set(items.map(item => slotOf.get(item.obj.unique_id)))].sort((a, b) => a - b)};
      }
      const segments = {};
      if (!container) for (const [key, raw] of Object.entries(o._stykVfx ?? {})) {
        const [scope, handle] = key.split('/');
        if (key !== 'figure' && scope === String(o.figure_type_id) && activeVfx(raw).length) segments[handle] = stack(raw);
      }
      const figure = stack(own);
      if (figure.effects.length || attached || Object.keys(segments).length) objects.push({slot, uid: o.unique_id, figure, ...(attached ? {attached} : {}), segments});
    });
    const generators = [];
    for (const clip of vfxClips(report)) {
      const timed = clip.duration !== null, end = timed ? clip.start + clip.duration : Infinity;
      if (!generatorClipActiveAt(clip, tick) || tick >= end) continue;
      const strength = clip.strength * (timed ? vfxFadeAt(clip, Math.max(0, Math.min(clip.duration, tick - clip.start))) : 1);
      if (strength <= 0) continue;
      sampleVfxClip(clip, Math.min(tick, timed ? end - 1e-6 : tick)).forEach((sampled, index) => {
        if (!isGeneratorType(sampled.type) || !sampled.enabled) return;
        const resolved = resolvedVfx(sampled), alpha = strength * resolved.params.effectStrength, start = clip.start + resolved.params.delay;
        if (alpha <= 0 || timed && resolved.params.delay >= clip.duration || tick < start) return;
        generators.push({clip: clip.id, index, alpha, start, ...effect(resolved)});
      });
    }
    return {scene: stack(vfxStack(frame)), objects, roots: [...assemblies.keys()], layerHidden, empty, generators,
      warnings: grouped.warnings?.filter(w => w.startsWith('Connected VFX')).length ?? 0,
      activeClips: vfxClips(report).filter(clip => vfxClipActiveAt(clip, tick)).map(clip => clip.id)};
  };
}
