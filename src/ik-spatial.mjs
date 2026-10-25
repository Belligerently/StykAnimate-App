/** Persistent 3D target space for native spatial IK. No render caches or second
 * pose hierarchy: the solver still writes the figure's authored angle channels. */
import {animationRotation} from './animation-origin.mjs';
import {quatMultiply,quatRotate,spatialPoint,spatialQuaternion,betweenVectors} from './rig-spatial.mjs';
export const inverseQuaternion=q=>[-q[0],-q[1],-q[2],q[3]];
const normalize=v=>{const n=Math.hypot(...v);return n>1e-10?v.map(x=>x/n):[0,0,1];};
const dot=(a,b)=>a.reduce((sum,v,i)=>sum+v*b[i],0);
const cross=(a,b)=>[a[1]*b[2]-a[2]*b[1],a[2]*b[0]-a[0]*b[2],a[0]*b[1]-a[1]*b[0]];
export function spatialWorldVector(v,figure,object,inverse=false){
 const a=animationRotation(figure,object),c=Math.cos(a),s=Math.sin(a),m=object.image_text_flags?-1:1;
 return inverse?[v[0]*c+v[1]*s,(-v[0]*s+v[1]*c)*m,v[2]]:[v[0]*c-v[1]*m*s,v[0]*s+v[1]*m*c,v[2]];
}
export function spatialWorldQuaternion(q,figure,object,inverse=false){
 const a=animationRotation(figure,object),reflect=v=>object.image_text_flags?[-v[0],v[1],-v[2],v[3]]:v;
 return inverse?reflect(quatMultiply(spatialQuaternion(-a,0,0),q)):quatMultiply(spatialQuaternion(a,0,0),reflect(q));
}
export function spatialKnee(base,end,pole,swivel,a,b){
 const vector=end.map((v,i)=>v-base[i]),distance=Math.hypot(...vector),axis=normalize(vector),d=Math.max(Math.abs(a-b)+1e-8,Math.min(a+b,distance));
 let direction=pole.map((v,i)=>v-axis[i]*dot(pole,axis));
 if(Math.hypot(...direction)<1e-6)direction=cross(axis,Math.abs(axis[0])<.8?[1,0,0]:[0,1,0]);
 direction=normalize(direction);const side=cross(axis,direction),cs=Math.cos(swivel),sn=Math.sin(swivel);
 const along=(a*a-b*b+d*d)/(2*d),height=Math.sqrt(Math.max(0,a*a-along*along));
 const bend=direction.map((v,i)=>v*cs+side[i]*sn);
 return {knee:base.map((v,i)=>v+axis[i]*along+bend[i]*height),end:base.map((v,i)=>v+axis[i]*d),normal:normalize(cross(axis,bend))};
}
/** Align both the bone axis and its captured bend-plane normal. A shortest
 * swing from the previous pose alone accumulates axial twist on closed drags. */
export function spatialIKOrientation(axis,normal,direction,plane){
 const aim=normalize(direction),swing=betweenVectors(normalize(axis),aim),turned=quatRotate(swing,normal);
 const angle=Math.atan2(dot(aim,cross(turned,plane)),dot(turned,plane))/2;
 const twist=[...aim.map(v=>v*Math.sin(angle)),Math.cos(angle)];
 return quatMultiply(twist,swing);
}
export function interpolateQuaternion(a,b,t){
 let d=dot(a,b),sign=d<0?-1:1;d=Math.min(1,Math.abs(d));
 const angle=Math.acos(d),sin=Math.sin(angle),x=sin>1e-6?Math.sin((1-t)*angle)/sin:1-t,y=(sin>1e-6?Math.sin(t*angle)/sin:t)*sign;
 return normalize(a.map((v,i)=>v*x+b[i]*y));
}
export function interpolatePole(a,b,t){return quatRotate(interpolateQuaternion([0,0,0,1],betweenVectors(a,b),t),a);}
