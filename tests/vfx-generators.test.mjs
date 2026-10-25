import test from 'node:test';
import assert from 'node:assert/strict';
import {newOwnedEditor} from './support/owned-editor.mjs';
import {emptyDocument,createObject,createFrame} from '../src/demo.mjs';
import {VFX_PRESETS,presetAvailable} from '../src/vfx-presets.mjs';
import {effectAvailable,isGeneratorType} from '../src/vfx-registry.mjs';

/** Figure 1 walks right 40px per 20-frame key; figure 2 stands still. */
function fixture({dark=false}={}){
 const report=emptyDocument(),fig=report.figure_types[0];
 report.keyframes=[0,1,2,3,4,5,6,7].map(i=>{const f=createFrame([createObject(fig,1,120+i*40,300),createObject(fig,2,520,300)],20);if(dark)f._stykBackgroundColor=0xff101018;return f;});
 const e=newOwnedEditor({demo:false,layerTimeline:true}).load(report);return e;
}

test('new generated effects start at the playhead with their own seed and undo as one step',()=>{
 const e=fixture();e.seek(17);const n=e._undo.length;
 const a=e.addVfxClip({kind:'figure',uid:1},'explosion'),b=e.addVfxClip({kind:'figure',uid:1},'explosion'),[ca,cb]=e.getVfxClips();
 assert.deepEqual([ca.start,ca.duration],[17,36]);assert.notEqual(ca.effects[0].params.seed,cb.effects[0].params.seed);
 const smoke=e.addVfxClip({kind:'scene'},'smoke');assert.deepEqual([e.getVfxClips().find(c=>c.id===smoke).start,e.getVfxClips().find(c=>c.id===smoke).duration],[17,72]);
 assert.ok(Object.values(VFX_PRESETS).every(p=>p.effects.every(([type])=>!isGeneratorType(type))),'presets hold filters only');assert.equal(presetAvailable(VFX_PRESETS['Golden hour'],true,effectAvailable),true);
 assert.equal(effectAvailable('aura',true),false);assert.equal(effectAvailable('explosion',true),true);
 assert.equal(e._undo.length,n+3);e.undo();e.undo();e.undo();assert.equal(e.getVfxClips().length,0);assert.ok(a&&b);
});

