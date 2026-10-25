import {geometryPlans} from './pivot/geometry-pose.mjs';
import {ikChain, MAX_IK_CHAIN} from './ik.mjs';
import {ikComponents, joinedIKGraph} from './ik-joined.mjs';

function context(editor) {
  const frame = editor.selectionFrame;
  if (!frame.objects.some(o => o.join)) return null;
  const slots = ikComponents(frame).find(slots => slots.includes(editor.selectedSlot));
  return slots?.length > 1 ? joinedIKGraph(editor.report, frame, slots) : null;
}


function controllerAt(editor) {
  const handle = editor.figure?.segments[editor.selectedJoint]?.segment_id;
  return {handle, controller: editor.object?._stykIK?.[handle]};
}

export const editorIKMethods = {
  getIKSettings() {
    const {handle, controller} = controllerAt(this);
    const graph = handle ? context(this) : null;
    const maxLength = handle ? (graph ? graph.chain(this.selectedSlot, handle, MAX_IK_CHAIN) : ikChain(this.figure, handle, MAX_IK_CHAIN,geometryPlans(this._geometry?.objects.get(this.selectedSlot))?.definition)).length : 0;
    return {enabled: false, pinned: false, allowStretch: false, bendDirection: 0, chainLength: Math.min(2, maxLength), ...controller, maxLength};
  },
};
