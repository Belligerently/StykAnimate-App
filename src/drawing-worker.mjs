import {errorMessageData} from './localized-error.mjs';
import {materializeDrawingTiles} from './drawing-preparation.mjs';
import {drawingEngineEnabled,loadDrawingEngine,drawingEngineMemory,drawingEngineGeneration} from './engine/engine-drawing.mjs';

// The drawing raster engine (Rust, src/engine/engine-drawing.mjs) answers every
// request once this worker has loaded it; without it (the gate is off or the
// module cannot load) the JavaScript engine below does.
const engine=loadDrawingEngine({enabled:drawingEngineEnabled(self.location)});
let engineOperations=null;

// Preparing a commit needs only the tile kernel. Load codecs, brush/fill and
// preview machinery on their first request, in the same bounded worker.
self.onmessage=async({data})=>{
  // Which raster engine answers this worker (diagnostics and tests).
  if(data.kind==='engine'){const rust=await engine;self.postMessage({id:data.id,engine:rust?'rust':'javascript',memory:drawingEngineMemory(),generation:drawingEngineGeneration()});return;}
  if(await engine){
    try{engineOperations??=await import('./engine/engine-drawing-worker.mjs');}
    catch(error){self.postMessage({id:data.id,error:error.message, ...errorMessageData(error),fatal:true});return;}
    await engineOperations.drawingEngineOperation(data);return;
  }
  if(data.kind==='prepare-tiles'){
    try{
      const tiles=materializeDrawingTiles(data.tiles);
      self.postMessage({id:data.id,tiles},tiles.flatMap(t=>t.pixels?[t.pixels]:[]));
    }catch(error){self.postMessage({id:data.id,error:error.message, ...errorMessageData(error)});}
    return;
  }
  if(data.kind==='preview'){
    let preview;
    try{preview=await import('./drawing-worker-preview.mjs');}
    catch(error){self.postMessage({id:data.id,error:error.message, ...errorMessageData(error),fatal:true});return;}
    try{const bitmap=preview.drawingWorkerPreview(data);self.postMessage({id:data.id,bitmap},[bitmap]);}
    catch(error){self.postMessage({id:data.id,error:error.message, ...errorMessageData(error)});}
    return;
  }
  let operations;
  try{operations=await import('./drawing-worker-operations.mjs');}
  catch(error){self.postMessage({id:data.id,error:error.message, ...errorMessageData(error),fatal:true});return;}
  await operations.drawingWorkerOperation(data);
};
