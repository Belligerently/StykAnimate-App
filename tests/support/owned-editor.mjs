// An editor the engine owns, for tests (D19): its session started on the editor's document as it is, or on
// the mirror's views of it, as the app starts one.
import {Editor} from '../../src/editor.mjs';
import {installEngineOwner} from '../../src/engine/engine-owner.mjs';
import {nodeEngine} from './engine-node.mjs';

const json = value => JSON.stringify(value, (key, item) => key === 'runtime_segments' || key === '_movieclipSample' ? undefined : item);

/** An editor the engine owns, its session started on the editor's document as it is. */
/** `recorded`: the identities a call draws come from the test (`setDraws`), else from the editor's own random ones. */
export async function ownedEditor(options, document, {verify = true, mirrored = false, recorded = true} = {}) {
  const {engine, mirror} = await nodeEngine();
  const editor = new Editor(options).load(structuredClone(document));
  let pool = [];
  const stateOf = e => JSON.stringify({tick: e.tick, frameIndex: e.frameIndex, selectedUid: e.selectedUid ?? null,
    selectedUids: [...e.selectedUids], selectedJoint: e.selectedJoint ?? -1, activeLayerId: e.activeLayerId, autoKey: Boolean(e.autoKey),
    layerRevision: e.layerRevision, nextFrameIdentity: e._nextFrameIdentity, nextUid: e._nextUid,
    propertyChannels: Boolean(e.propertyChannels), layerTimeline: Boolean(e.layerTimeline)});
  // As in the app: the session starts on the mirrored document (the stage's), once it is current.
  if (mirrored) { mirror.reset(); await mirror.syncFrames(editor.report, editor.revision, {allFigures: true, allFrames: true, allSources: true}); }
  const owner = installEngineOwner(editor, {
    verify, draws: recorded ? () => pool : null,
    start: e => mirrored ? (mirror.framesCurrent(e.report, e.revision) ? engine.handle.edit_session(stateOf(e)) : null)
      : engine.editSession(json(e.report), stateOf(e), e.revision),
  });
  return {editor, owner, setDraws: draws => { pool = draws; }};
}
