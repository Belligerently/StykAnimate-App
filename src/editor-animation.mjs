import {messageError} from './localized-error.mjs';
/** What the editor reads of the authored animation: its channels, the camera-motion clips and the values the frame on show gives
 * them (the session answers the reads that evaluate, `engine-owner.mjs`). Every edit of them is a command of the engine session.
 * `nativePropertyChannel` and `nativePoseChannel` stay for the preview of a branch or builder order edit (`editor-branch-style.mjs`,
 * reached from `native-property-edit.mjs`), which still builds its channels here. */
import {animationChannels, animationEndTick, animationChannelId, findAnimationChannel, normalizeAnimationTarget, animationPropertyValue, sampleAnimationChannel} from './animation-channels.mjs';
import {buildGeometry, evaluatedFrame} from './pivot/geometry.mjs';
import {nativeTimeline} from './pivot/read-view.mjs';
import {projectNativePropertyChannel} from './native-property-projection.mjs';
import {figurePoseSequences} from './native-pose-projection.mjs';

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
  /** Detach only the property being authored, retaining its original frame keys.
   * Source poses remain immutable assets; no composed frame is baked per tick. */
  nativePropertyChannel(target, property, options={}) {
    return projectNativePropertyChannel(this,target,property,options,tick=>frameAtTick(this,tick,true).frame);
  },
  get animationChannels() { return animationChannels(this.report); },
  get animationEndTick() { return animationEndTick(this.report); },
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
  /** Native descriptors are lazy, so unopened tracks allocate no per-frame keys. */
  nativePoseChannel(target, {nativeOnly = false} = {}) {
    if(this.report._styk?.layerSheet){
      const id=['camera','scene'].includes(target.kind)?target.kind:target.kind==='layer'?target.id:this.layerForUid(target.uid),track=this.report._styk.layerSheet.tracks[id];
      if(track&&!track.keys.some(k=>k.sourceTick!==undefined)){
        const keys=target.kind==='figure'?figurePoseSequences(this.report,track.keys)(target.uid):track.keys;
        return {id:animationChannelId(target,'pose'),target,property:'pose',source:'authored',keys};
      }
    }
    target = normalizeAnimationTarget(target); const id = animationChannelId(target, 'pose');
    const authored = !nativeOnly && this.animationChannel(target, 'pose'); if (authored) return authored;
    this._nativeAnimationChannels ??= new Map();
    const cacheId = nativeOnly ? `${id}:native-baseline` : id;
    const cached = this._nativeAnimationChannels.get(cacheId), timing = this.timeline;
    if (cached?._timeline === timing && cached?._document === this._documentToken) return cached;
    let markers = this._nativePoseMarkerCache;
    if (markers?.timing !== timing || markers?.document !== this._documentToken) {
      const keys = nativeTimeline(this.report).map((row, index) => ({id:`native:pose:${this.getFrameId(index)}`, tick:row.start,
        value:{frameId:this.getFrameId(index)}, ...(this.report.keyframes[index]._styk?.easing ?? (this.report.version < 5 ? {interpolation:'constant'} : this.report.auto_easing ? {} : {interpolation:'linear'})),
        ...(row.count === 0 ? {hidden:true} : {})}));
      this._nativePoseMarkerCache = markers = {timing, document:this._documentToken, keys};
    }
    const emptyLayer = target.kind === 'layer' && !this.layers?.find(layer => layer.id === target.id)?.figureUids?.length;
    const inherited = !nativeOnly && target.kind === 'figure' ? this.animationChannel({kind:'layer',id:this.layerForUid(target.uid)}, 'pose') : null;
    let markerKeys = inherited ? inherited.keys : emptyLayer ? [] : markers.keys;
    if (target.kind === 'figure') {
      if (markers.figuresRevision !== this.layerRevision) { markers.figures = new WeakMap(); markers.figuresRevision = this.layerRevision; }
      markers.figures ??= new WeakMap();
      let sequences = markers.figures.get(markerKeys);
      if (!sequences) markers.figures.set(markerKeys, sequences = figurePoseSequences(this.report, markerKeys));
      markerKeys = sequences(target.uid);
    }
    // Key identity is scoped by channel ID. Native figures therefore share the
    // same immutable sequence, and connected edits can preserve that sharing.
    const descriptor = {id, target, property: 'pose', source: 'native', keys:markerKeys};
    if (['figure','layer'].includes(target.kind) && (inherited ? inherited.nativeProperties : this.propertyChannels)) descriptor.nativeProperties = true;
    Object.defineProperties(descriptor, {_timeline: {value: timing}, _document: {value: this._documentToken},
      markerKeys:{value:markerKeys}, markerKeyId:{value:key=>key.id}});
    this._nativeAnimationChannels.set(cacheId, descriptor); return descriptor;
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
