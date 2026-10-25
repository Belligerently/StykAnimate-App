import {spatialCamera, spatialShape} from './spatial-projection.mjs';
import {geometryPlans} from './pivot/geometry-pose.mjs';
import {animationRotation} from './animation-origin.mjs';
import {rigConstraintPlan} from './rig-constraints.mjs';
import {spatialBone, spatialMotionValue} from './rig-spatial.mjs';

const plans=e=>geometryPlans(e.geometry.objects.get(e.selectedSlot));
const rigPlan=e=>plans(e)?.plan??rigConstraintPlan(e.figure);
export const editorSpatialMethods={
 /** The 3D rig's perspective and part proportions (the engine session sets them); evaluation stays in the existing rig graph. */
 getSpatialCamera(){return spatialCamera(this.object);},
 getSpatialProportions(){
  const f=this.figure,o=this.object,s=f?.segments[this.selectedJoint];if(!o||!spatialBone(s))return null;
  return {...spatialShape(o,s.segment_id),canResizeLength:s.length>1e-9,length:s.length>1e-9?(o.len_flag?o.lengths[this.selectedJoint]:s.length)/s.length:1};
 },
 getSpatialPose(index=this.selectedJoint){
  const f=this.figure,o=this.object,s=f?.segments[index];if(!o||!spatialBone(s))return null;
  const r=s._stykRig.spatial,delta=animationRotation(f,o),find=id=>{const i=f.segments.findIndex(s=>s.segment_id===id);return i<0?undefined:i;},parent=find(r.parent),turn=find(r.turn),pitch=find(r.pitch);
  const angles=this.geometry.objects.get(this.selectedSlot)?.solvedAngles??o.angles;
  const sign=o.image_text_flags?-1:1,parentAngle=parent===undefined?0:(angles[parent]-delta)*sign-f.segments[parent].angle_rad;
  const bend=(angles[index]-delta)*sign-s.angle_rad-parentAngle;
  // Mechanical details expose their authored residual. Their master's applied
  // rotation remains an evaluation dependency, not another editable channel.
  return r.motion?{bend:r.motion.type==='hinge'?Math.atan2(Math.sin(bend),Math.cos(bend)):0,turn:0,pitch:0}:
   {bend,turn:(angles[turn]-delta)*sign-f.segments[turn].angle_rad,pitch:(angles[pitch]-delta)*sign-f.segments[pitch].angle_rad};
 },
 /** Travel of the selected slide (figure units from rest) or hinge (radians). */
 getSpatialMotion(index=this.selectedJoint){
  const f=this.figure,o=this.object,s=f?.segments[index],r=s?._stykRig?.spatial,m=r?.motion;if(!o||!m||!spatialBone(s))return null;
  const g=this.geometry.objects.get(this.selectedSlot),driver=m.driver?(g?.segments[rigPlan(this).ids.get(m.driver.joint)]?.spatialFrame?.motionValue??0)*m.driver.multiplier:0;
  const applied=spatialMotionValue(r,{length:o.len_flag?o.lengths[index]:s.length,bend:m.type==='hinge'?this.getSpatialPose(index).bend:0,driver});
  const min=m.min-driver,max=m.max-driver,value=applied-driver;
  return{type:m.type,min,max,value,applied,driver,fraction:max>min?(value-min)/(max-min):0};
 },
};
