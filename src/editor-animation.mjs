import {messageError} from './localized-error.mjs';
/** What the editor reads of the authored animation: its channels, the camera-motion clips and the values the frame on show gives
 * them (the session answers the reads that evaluate, `engine-owner.mjs`). Every edit of them is a command of the engine session. */
import {animationChannels, findAnimationChannel, normalizeAnimationTarget, animationPropertyValue, sampleAnimationChannel} from './animation-channels.mjs';
import {buildGeometry, evaluatedFrame} from './pivot/geometry.mjs';

const MAX_TICK = 10000000;
const checkTick = tick => { if (!Number.isInteger(tick) || tick < 0 || tick > MAX_TICK) throw messageError(new Error(''), "errors.editorAnimation.keyframeTimeMustBeAnIntegerFrom0", {}); return tick; };
const noClips = Object.freeze([]);
function context(editor) { return {resolveGeometry: (report, frame) => frame === editor.evaluatedFrame ? editor.geometry : buildGeometry(report, frame)}; }
function frameAtTick(editor, tick, ignorePreview = false) {
  if (!ignorePreview && tick === editor.tick) return {frame: editor.evaluatedFrame, index: editor.frameIndex};
  const rows = editor.timeline; let low = 0, high = rows.length - 1;
  while (low < high) { const mid = Math.ceil((low + high) / 2); if (rows[mid].start <= tick) low = mid; else high = mid - 1; }
  const row = rows[low];
  return {index: low, frame: evaluatedFrame(editor.report, low, row.count > 0 ? Math.min(1, (tick - row.start) / row.count) : 0, tick)};
}

export const editorAnimationMethods = {
  get animationChannels() { return animationChannels(this.report); },
  segmentAnimationTarget(uid,handleId,property,tick=this.tick){
    const target={kind:'segment',uid,handleId};
    if(!this.animationChannels.some(channel=>channel.target.kind==='segment'&&channel.target.uid===uid&&channel.target.handleId===handleId&&
      channel.target.figureTypeId!==undefined&&(property===undefined||channel.property===property)))return normalizeAnimationTarget(target);
    const {frame}=frameAtTick(this,tick),object=frame.objects.find(object=>object.unique_id===uid);
    return normalizeAnimationTarget(object?{...target,figureTypeId:object.figure_type_id}:target);
  },
  resolveAnimationTarget(target,property,tick=this.tick){
    return target?.kind==='segment'&&target.figureTypeId===undefined?this.segmentAnimationTarget(target.uid,target.handleId,property,tick):normalizeAnimationTarget(target);
  },
  animationChannel(target, property, {exactTarget=false}={}) { return findAnimationChannel(this.report, exactTarget?normalizeAnimationTarget(target):this.resolveAnimationTarget(target,property), property); },
  animationValue(target, property, tick = this.tick, {exactTarget=false}={}) {
    // Inspector reads use the continuous playhead; stored key times stay whole.
    if (!Number.isFinite(tick) || tick < 0 || tick > MAX_TICK) checkTick(tick);
    const {frame} = frameAtTick(this, tick);
    target=exactTarget?normalizeAnimationTarget(target):this.resolveAnimationTarget(target,property,tick);
    const visible=animationPropertyValue(this.report,frame,target,property,context(this));
    return exactTarget&&property!=='pose'&&visible!==undefined?(sampleAnimationChannel(findAnimationChannel(this.report,target,property),tick)??visible):visible;
  },
  getCameraMotions() { return this.report._styk.cameraMotion?.clips ?? noClips; },
  /** The open motion-path gesture's falloff plan for the overlay, or null. */
  motionPathFalloff() {
    const session = this._drag;
    if (!session?.motionPath || !session.falloff) return null;
    return {tick: session.tick, ...session.falloff, writes: session.writes,
      changed: new Set(session.subs.filter(sub => sub.moved || (sub.kind === 'edge' && session.moved)).map(sub => sub.tick))};
  },
  /** The live document for trail sampling while a motion-path gesture is open. */
  motionPathReport() { return this._drag?.motionPath ? this._drag.report : this.report; },
};
