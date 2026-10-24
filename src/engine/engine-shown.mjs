/**
 * What the interface reads of the frame on show, from the engine's session (docs/engine/evaluation.md,
 * "The interface's frame"): the evaluated frame record (`editor.evaluatedFrame`), the timeline rows
 * (`editor.timeline`) and the geometry (`editor.geometry`) in the shape the canvas, the handles and the
 * pickers read: `{objects: Map<slot, group|null>, orderedObjects, drawables, token, changedSlots, warnings}`
 * with groups `{slot, obj, fig, root, joints, segments, polys, bounds, solvedAngles, solvedLengths}`.
 *
 * The session evaluates and builds; this module only reads what it sends. A geometry read names the read the
 * interface holds, and the session sends the groups of the figures that changed since: the others are the
 * groups already held, so the objects the canvas keeps by identity (handle descriptors) stay valid.
 */
import {resolveInstanceFigure} from '../animation-style.mjs';
import {isStatic} from '../pivot/geometry.mjs';
import {isMovieclip} from '../movieclips.mjs';
import {sticknodesDrawables, sticknodesSpriteAsset} from '../sticknodes/geometry.mjs';

const point = ([x, y]) => ({x, y});
/** `[x0, y0, x1, y1, …]` as points. */
const pointList = flat => {
  const points = new Array(flat.length >> 1);
  for (let i = 0; i < points.length; i++) points[i] = {x: flat[2 * i], y: flat[2 * i + 1]};
  return points;
};
/** A number the session wrote as `null` (not finite) is NaN. */
const real = value => value ?? NaN;

const definitionsOf = new WeakMap();
/** The figure definitions of a report by type id. */
function definitions(report) {
  const figures = report.figure_types ?? [];
  let known = definitionsOf.get(figures);
  if (!known) { known = new Map(figures.map(figure => [Number(figure.figure_type_id), figure])); definitionsOf.set(figures, known); }
  return known;
}

/** The image or text asset a segment draws (`segmentAt` in the editor's geometry). */
function assetOf(fig, seg, obj) {
  const assets = seg.segment_type === 'text' ? fig.text_assets : seg.segment_type === 'image' ? fig.image_assets : null;
  if (!assets) return undefined;
  const wanted = Number(seg.extra_u16 ?? seg.extra?.[0] ?? seg.image_asset_index ?? seg.sprite_index ?? 0);
  let asset = assets.find(candidate => Number(candidate.asset_index ?? candidate.table_index ?? candidate.image_index ?? candidate.sprite_index) === wanted);
  if (asset && !asset.anchor_length && asset.pivot && asset.endpoint) {
    asset = {...asset, anchor_start: asset.pivot, anchor_end: asset.endpoint,
      anchor_length: Math.hypot(asset.endpoint.x - asset.pivot.x, asset.endpoint.y - asset.pivot.y),
      anchor_angle_rad: Math.atan2(asset.endpoint.y - asset.pivot.y, asset.endpoint.x - asset.pivot.x)};
  }
  return obj._sticknodesSprite ? sticknodesSpriteAsset(asset, obj) : asset;
}

function spatialFrame(raw) {
  const {motionValue, choiceScale, shape, restLength, ...frame} = raw;
  if (motionValue !== null && motionValue !== undefined) frame.motionValue = motionValue;
  if (choiceScale !== undefined) frame.choiceScale = choiceScale;
  if (shape) { frame.shape = shape; frame.restLength = restLength; }
  return frame;
}

/** The `flags` of a segment row (styk_eval::geometry::interface). */
const START = 1, END = 2, POINTS = 4, CONNECTOR = 8, SPATIAL = 16;

/** A group the session sent, as the interface reads it. */
function hydrate(raw, slot, frame, report, defs) {
  const obj = frame.objects[slot];
  const fig = obj && resolveInstanceFigure(defs.get(Number(obj.figure_type_id)), obj);
  if (!obj || !fig) return null;
  const root = point(raw.root);
  const joints = new Map([[0, root]]);
  const {ends} = raw;
  for (let i = 0; i < fig.segments.length && 2 * i < ends.length; i++) if (ends[2 * i] !== null) joints.set(Number(fig.segments[i].segment_id), {x: ends[2 * i], y: ends[2 * i + 1]});
  if (isMovieclip(fig)) {
    // A movieclip instance is its handle: forty units from the root along its angle (what it draws is the engine's).
    const [row] = raw.segs, end = joints.get(Number(fig.segments[0].segment_id));
    const handle = {seg: fig.segments[0], obj, fig, slot, index: 0, start: root, end, points: [root, end], angle: real(row[1]), length: real(row[3]),
      bend: 0, thickness: 0, opacity: 1, alpha: 1, rgb: [0, 0, 0]};
    const group = {slot, obj, fig, root, joints, segments: [handle], polys: [], drawables: []};
    if (raw.bounds) group.bounds = {minX: raw.bounds[0], minY: raw.bounds[1], maxX: raw.bounds[2], maxY: raw.bounds[3]};
    if (raw.drawn) group.drawn = true;
    return group;
  }
  const segments = raw.segs.map(row => {
    const index = row[0], flags = row[11], seg = fig.segments[index];
    let at = 12;
    // A segment starts where its parent ends, unless it says otherwise.
    const start = flags & START ? {x: row[at++], y: row[at++]} : joints.get(Number(seg.parent_id)) ?? root;
    const joint = joints.get(Number(seg.segment_id));
    const end = flags & END ? {x: row[at++], y: row[at++]} : joint;
    const points = flags & POINTS ? pointList(row[at++]) : [start, end];
    if (flags & POINTS) points[points.length - 1] = end;
    const result = {
      type: seg.segment_type === 'circle' || seg.segment_type === 'image' || seg.segment_type === 'text' ? seg.segment_type : 'line',
      seg, obj, fig, asset: assetOf(fig, seg, obj), version: report.version, slot, index, start, end, points,
      angle: real(row[1]), bend: row[2] ?? 0, length: real(row[3]), z: Number(row[5] ?? seg.z_order ?? index + 1),
      thickness: row[4] ?? 0, rgb: [row[6], row[7], row[8]], alpha: row[9] ?? 1, opacity: row[10] ?? 1, static: isStatic(seg),
    };
    if (flags & CONNECTOR) { result.handle = joint; result.connector = true; }
    if (flags & SPATIAL) result.spatialFrame = spatialFrame(row[at++]);
    return result;
  });
  const polys = raw.polys.map(shape => {
    const poly = {type: 'poly', poly: fig.polyfills[shape.index], obj, fig, slot, index: shape.index, points: pointList(shape.points),
      z: shape.z, rgb: shape.rgb, alpha: shape.alpha, opacity: shape.opacity};
    if (shape.culled) poly.culled = true;
    if (shape.depthValues) poly.depthValues = shape.depthValues;
    return poly;
  });
  const group = {slot, obj, fig, root, joints, segments, polys};
  if (raw.solved) { group.solvedAngles = raw.solved.angles; if (raw.solved.lengths) group.solvedLengths = raw.solved.lengths; }
  if (raw.bounds) group.bounds = {minX: raw.bounds[0], minY: raw.bounds[1], maxX: raw.bounds[2], maxY: raw.bounds[3]};
  if (raw.drawn) group.drawn = true;
  let drawables;
  // What paints, in painter order: the faces, then the visible segments (Stick Nodes shapes expanded).
  Object.defineProperty(group, 'drawables', {enumerable: true, get() {
    if (!drawables) {
      drawables = [];
      for (const poly of polys) if (!poly.culled) drawables.push(poly);
      for (const shape of segments) {
        if (shape.seg.visible === false) continue;
        const expanded = sticknodesDrawables(shape);
        if (expanded) drawables.push(...expanded); else drawables.push(shape);
      }
      drawables.sort((a, b) => a.z - b.z);
    }
    return drawables;
  }});
  return group;
}

/**
 * @param {{ask(kind: string, args: object): (string|null), source(motion: boolean): (object|null), report(): object}} owner
 *   `ask` reads the session (flushing the editor's pending edits first) and answers JSON text, `null` where the
 *   engine does not hold the document (a movieclip being edited, a call the editor is running); `source` gives
 *   the stage what it draws from.
 */
export function createShown(owner) {
  /** The geometry the interface holds, per kind of frame (the frame on show, the motion-path display). */
  const held = [null, null];
  const read = (kind, args = {}) => {
    const text = owner.ask(kind, args);
    return text === null ? null : JSON.parse(text);
  };
  return {
    /** Whether the engine answers (the session holds the open document). */
    get active() { return owner.ask('active', {}) !== null; },
    frame: () => read('frame'),
    documentFrame: () => read('document'),
    /** The motion-path edit's display frame, or `null` where it shows the document. */
    motionFrame: () => read('motion'),
    /** `editor.timeline`: one `{index, start, count}` per row. */
    timeline() {
      const rows = read('timeline');
      return rows && rows.map(([start, count], index) => ({index, start, count}));
    },
    /**
     * The geometry of `frame` (what `frame` is on show, or the motion-path display with `motion`): the groups the
     * session sent over the ones held. `null` where the engine does not answer (or shows nothing, for the motion display).
     */
    geometry(frame, {motion = false} = {}) {
      const previous = held[+motion];
      const answer = read('geometry', {since: previous?.read ?? null, motion});
      if (!answer) return null;
      const report = owner.report(), defs = definitions(report);
      const groups = answer.full || !previous ? new Map() : new Map(previous.geometry.objects);
      const changed = new Set();
      for (const [slot, raw] of answer.slots) { groups.set(slot, raw && hydrate(raw, slot, frame, report, defs)); changed.add(slot); }
      for (const slot of [...groups.keys()]) if (slot >= answer.count) groups.delete(slot);
      const order = frame.object_draw_order ?? Array.from({length: answer.count}, (_, slot) => slot);
      const orderedObjects = [];
      for (const slot of order) { const group = groups.get(Number(slot)); if (group?.drawn) orderedObjects.push(group); }
      if (previous && !answer.full && !changed.size && previous.geometry.orderedObjects.length === orderedObjects.length
          && orderedObjects.every((group, index) => group === previous.geometry.orderedObjects[index])) {
        held[+motion] = {read: answer.token, geometry: previous.geometry};
        return previous.geometry;
      }
      let flat;
      const geometry = {objects: groups, orderedObjects, token: {}, previousToken: previous?.geometry.token, changedSlots: answer.full || !previous ? null : changed,
        get drawables() { return flat ??= orderedObjects.flatMap(group => group.drawables); }, warnings: answer.warnings};
      held[+motion] = {read: answer.token, geometry};
      return geometry;
    },
    /** What the stage draws from: the session and the view it is at, or `null`. */
    source: motion => owner.source(motion),
    /** Forgets the geometry held (another document is open). */
    release() { held[0] = held[1] = null; },
  };
}
