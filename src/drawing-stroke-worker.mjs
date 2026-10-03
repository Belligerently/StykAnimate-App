import {errorMessageData} from './localized-error.mjs';
import {StrokeEngine} from './drawing-stroke-engine.mjs';
import {StrokeEngine as EngineStrokes,drawingEngineEnabled,loadDrawingEngine} from './engine/engine-drawing.mjs';

// The drawing raster engine (Rust) paints when this worker has loaded it; the
// JavaScript engine otherwise. Messages wait for that choice in their order.
const ready=loadDrawingEngine({enabled:drawingEngineEnabled(self.location)}).then(rust=>rust?new EngineStrokes():new StrokeEngine());
let queue=Promise.resolve();

self.onmessage=({data})=>{queue=queue.then(()=>ready).then(engine=>handle(engine,data)).catch(error=>self.postMessage({kind:'error',stroke:data.stroke,message:error.message, ...errorMessageData(error)}));};

function handle(engine,data){
  const {kind,stroke:id}=data;
  // Which raster engine paints in this worker (diagnostics and tests).
  if(kind==='engine'){self.postMessage({kind:'engine',engine:engine instanceof StrokeEngine?'javascript':'rust'});return;}
  try{
    if(kind==='begin')engine.begin(id,data.settings);
    else if(kind==='dabs'){
      const result=engine.dabs(id,data.seq,data.dabs,data.tiles,data.tail);
      if(result)self.postMessage({kind:'tiles',stroke:id,seq:data.seq,tiles:result.tiles,dirty:result.dirty},result.tiles.map(t=>t.pixels));
    }
    else if(kind==='end'){
      const changes=engine.end(id);
      self.postMessage({kind:'done',stroke:id,changes:changes??[]},(changes??[]).flatMap(c=>[c.before?.buffer,c.after?.buffer].filter(Boolean)));
    }
    else if(kind==='cancel')engine.cancel(id);
  }catch(error){engine.cancel(id);self.postMessage({kind:'error',stroke:id,message:error.message, ...errorMessageData(error)});}
}
