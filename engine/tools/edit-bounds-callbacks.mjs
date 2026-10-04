// The app's bounds callback, using its existing renderer owner and fixture resources.
import {objectBounds} from '../../src/renderer-geometry.mjs';
export function appBounds(resources = {}) {
  const cache = new WeakMap();
  return geometry => {
    let bounds = cache.get(geometry);
    if (!bounds) {
      const rect = objectBounds([...geometry.segments.map(segment => ({...segment, type: segment.seg.segment_type || 'line'})),
        ...geometry.polys, ...(geometry.meshDrawables ?? [])], resources);
      bounds = {minX: rect.x, minY: rect.y, maxX: rect.x + rect.width, maxY: rect.y + rect.height};
      cache.set(geometry, bounds);
    }
    return bounds;
  };
}
export function recordedBounds(value) {
  if (!value || typeof value !== 'object' || !Object.hasOwn(value, '@boundsCallback')) return null;
  if (value['@boundsCallback'] === 'app') {
    const given = value.resources ?? {};
    return appBounds({...given, ...(given.imageSize ? {image: () => ({naturalWidth: given.imageSize[0], naturalHeight: given.imageSize[1]})} : {}),
      ...(given.drawingBounds ? {drawing: () => ({store: {bounds: given.drawingBounds}})} : {})});
  }
  if (value['@boundsCallback'] === 'constant') return () => value.value;
  throw new Error('Unknown bounds callback in editing scenario');
}
