/**
 * Brings the engine's document to the editor's, for work the engine does away from the
 * stage: exports (engine-export.mjs) and thumbnails (engine-thumbnails.mjs).
 */

/**
 * Why the engine cannot work on `report` at `revision`, or null once it holds it: its
 * frames through the playback adapter (`exportDocument`, the synchronisation playback
 * uses, one commit at a time), then what only the stage knows (figures edited in place,
 * drawings just published: `prepareExport`). A background commit the interface thread may
 * not wait for (D7) is waited out on the engine's progress counter. `check()` throws to
 * abandon the wait (an AbortError passes through).
 * @param {{stage: object, playback: object, report: object, revision: number, check?: () => void}} options
 * @returns {Promise<string|null>}
 */
export async function engineDocument({stage, playback, report, revision, check = () => {}}) {
  try {
    if (!await playback.exportDocument(report, revision)) return stage.prepareExport(report, revision) ?? 'the engine does not draw this document';
    const engine = stage.engineSide.engine;
    for (;;) {
      check();
      const seen = engine.progressSeen?.();
      try { return stage.prepareExport(report, revision); }
      catch (error) { if (error?.code !== 'engine_busy') throw error; await engine.whenProgress(seen); }
    }
  } catch (error) {
    if (error?.name === 'AbortError') throw error;
    return String(error?.message ?? error);
  }
}
