import {timeline, evaluatedFrame, buildGeometry} from './pivot/geometry.mjs';

export {LIBRARY_MATTES, libraryPreviewMatte} from './library-matte.mjs';

export function libraryPreviewTimeline(report) {
  const rows = timeline(report), fps = Math.max(1, Number(report.fps) || 24);
  let duration = Math.max(1, (rows.at(-1)?.start ?? 0) + (rows.at(-1)?.count ?? 1));
  const types = new Map(report.figure_types.map(figure => [figure.figure_type_id, figure]));
  for (const object of report.keyframes[0].objects) {
    const clip = types.get(object.figure_type_id)?._styk?.movieclip;
    if (clip) duration = Math.max(duration, clip.duration * fps / clip.document.fps);
  }
  return {fps, duration, report, geometry(tick) {
    tick = ((tick % duration) + duration) % duration;
    let low = 0, high = rows.length - 1;
    while (low < high) { const mid = (low + high + 1) >>> 1; if (rows[mid].start <= tick) low = mid; else high = mid - 1; }
    const row = rows[low];
    // The evaluated frame travels with its geometry for generated effects.
    const frame = evaluatedFrame(report, low, (tick - row.start) / Math.max(1, row.count), tick);
    return Object.assign(buildGeometry(report, frame), {sourceFrame: frame});
  }};
}
