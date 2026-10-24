import {messageError} from './localized-error.mjs';

const cancelled = options => {
  if (options?.signal?.aborted) throw messageError(new DOMException('', 'AbortError'), "errors.editorLayerAsync.layerMigrationWasCancelled", {});
};

/** Moving figures to another layer and deleting a layer keep every figure's poses: the engine plans the change from
 * the poses of the whole animation and publishes it in one step (`assignFiguresToLayer`, `deleteLayer`, commands of the
 * engine session). These are the awaitable forms the interface calls; a signal already aborted cancels them. */
export const editorLayerAsyncMethods = {
  async assignFiguresToLayerAsync(uids, id, options = {}) { cancelled(options); return this.assignFiguresToLayer(uids, id); },
  async deleteLayerAsync(id, options = {}) { cancelled(options); return this.deleteLayer(id, {targetLayerId: options.targetLayerId}); },
};
