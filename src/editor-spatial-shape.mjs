import {spatialCamera,spatialShape} from './spatial-projection.mjs';
import {spatialBone} from './rig-spatial.mjs';

/** What the 3D rig's perspective and part proportions are. The engine session sets them (`setSpatialCamera`,
 * `setSpatialProportions`, `resetSpatialProportions`); evaluation stays in the existing rig graph. */
export const editorSpatialShapeMethods={
 getSpatialCamera(){return spatialCamera(this.object);},
 getSpatialProportions(){
  const f=this.figure,o=this.object,s=f?.segments[this.selectedJoint];if(!o||!spatialBone(s))return null;
  return {...spatialShape(o,s.segment_id),canResizeLength:s.length>1e-9,length:s.length>1e-9?(o.len_flag?o.lengths[this.selectedJoint]:s.length)/s.length:1};
 },
};
