#!/usr/bin/env node
// Prints the rectangles the editor filters each object's effects on at one tick
// (`objectBounds` with and without effect padding, as `rasterObject` and
// `filterArtworkVfx` compute them), to compare with `styk layers <file> <tick>`.
//
//   node engine/tools/vfx-layer-bounds.mjs <project.json|.styk> <tick>
import {readFileSync} from 'node:fs';
import {repo, open} from './fixture-kit.mjs';

const {buildGeometry} = await import(`${repo}/src/pivot/geometry.mjs`);
const {connectedVfxGeometry} = await import(`${repo}/src/vfx-connected.mjs`);
const {objectBounds} = await import(`${repo}/src/renderer-geometry.mjs`);
const {drawableVfxPadding, hasDrawableVfx} = await import(`${repo}/src/vfx-canvas.mjs`);

const [file, tick = '0'] = process.argv.slice(2);
const editor = open(JSON.parse(readFileSync(file, 'utf8')));
editor.seek(Number(tick));
const frame = editor.evaluatedFrame || editor.frame, report = editor.report;
const geometry = connectedVfxGeometry(buildGeometry(report, frame), frame, report);

for (const g of geometry.orderedObjects) {
  const items = g.drawables;
  if (!items.some(hasDrawableVfx)) continue;
  const bounds = objectBounds(items), domain = g.drawables[0]?._movieclipOwner?.domainBounds ?? objectBounds(items, {vfxBounds: false});
  const padding = items.reduce((n, item) => Math.max(n, drawableVfxPadding(item)), 0);
  const rect = r => [r.x, r.y, r.width, r.height].join(', ');
  console.log(`slot ${g.slot} (uid ${g.obj.unique_id}): bounds [${rect(bounds)}] domain [${rect(domain)}] padding ${padding}`);
}
