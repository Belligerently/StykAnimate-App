import test from 'node:test';
import assert from 'node:assert/strict';
import {Worker} from 'node:worker_threads';
import {createHash} from 'node:crypto';
import {vfxProperty,validateVfxValue} from '../src/vfx.mjs';
import {VFX_REGISTRY} from '../src/vfx-registry.mjs';
import {evaluateParticle} from '../src/vfx-generators.mjs';
import {PARTICLE_LOOKS} from '../src/vfx-generator-styles.mjs';
import {atlasRow,ATLAS_ROWS,ATLAS_LEVELS,CELL,COLUMNS,MATERIALS} from '../src/vfx-generator-atlas.mjs';

function particles(batch,filter=()=>true){const out=[],p={};for(const draw of batch?.draws??[])if(filter(draw))for(let i=0;i<draw.count;i++)if(evaluateParticle(batch,draw,i,p))out.push({draw,i,...p});return out;}
const digest=strip=>createHash('sha256').update(Buffer.concat(strip.levels.map(l=>Buffer.from(l.data)))).digest('hex');

test('particle materials are reduced levels of cells that stay clear of their neighbours',()=>{
 for(let row=0;row<ATLAS_ROWS;row++){
  const {levels}=atlasRow(row);assert.equal(levels.length,ATLAS_LEVELS);
  levels.forEach((l,i)=>{assert.equal(l.width,(CELL*COLUMNS)>>i);assert.equal(l.height,CELL>>i);assert.equal(l.data.length,l.width*l.height*2);});
  const base=levels[0];
  for(let k=0;k<COLUMNS;k++){
   let covered=0,edge=0;
   for(let y=0;y<CELL;y++)for(let x=0;x<CELL;x++){const a=base.data[(y*base.width+k*CELL+x)*2+1];covered+=a>0;if(x===0||y===0||x===CELL-1||y===CELL-1)edge=Math.max(edge,a);}
   assert.ok(covered>CELL*CELL*.02,`row ${row} cell ${k} has a visible shape`);assert.equal(edge,0,`row ${row} cell ${k} leaves its border clear`);
  }
 }
 for(const m of Object.values(MATERIALS))assert.ok(m.row<ATLAS_ROWS&&m.col+m.count<=COLUMNS);
});

test('workers generate byte-identical materials',async()=>{
 const url=new URL('../src/vfx-generator-atlas.mjs',import.meta.url).href;
 const code=`const {parentPort}=require('node:worker_threads'),{createHash}=require('node:crypto');import(${JSON.stringify(url)}).then(({atlasRow})=>parentPort.postMessage([0,7,16].map(r=>createHash('sha256').update(Buffer.concat(atlasRow(r).levels.map(l=>Buffer.from(l.data)))).digest('hex'))));`;
 const remote=await new Promise((resolve,reject)=>{const w=new Worker(code,{eval:true});w.once('message',m=>{resolve(m);w.terminate();});w.once('error',reject);});
 assert.deepEqual(remote,[0,7,16].map(r=>digest(atlasRow(r))));
});

test('style choices suggest valid values for every option',()=>{
 for(const [type,definition] of Object.entries(VFX_REGISTRY))for(const [param,d] of Object.entries(definition.params)){
  if(!d.applies)continue;
  assert.equal(d.applies.length,d.options.length,`${type}.${param} suggests values for each option`);
  for(const values of d.applies)for(const [key,value] of Object.entries(values))validateVfxValue(vfxProperty(type,'x',key),value);
 }
 assert.deepEqual(VFX_REGISTRY.particles.params.look.options,PARTICLE_LOOKS.map(l=>l.name),'saved look indices match the renderer');
});

