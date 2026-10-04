/**
 * The engine's preview worker (engine-previews.mjs; docs/engine/rendering.md, Thumbnails and
 * library previews).
 *
 * It joins the engine's shared memory as a thread of its own, as the export worker does, and
 * keeps its event loop. It holds a GPU device of its own (WebGPU in the worker, else WebGL2 on
 * its OffscreenCanvas) and draws library files with the stage's renderer (`Previews`,
 * styk_wasm::preview): a poster (a PNG), the hover animation's frames and the samples its
 * card's backdrop is chosen from. Each picture is taken from the canvas as an ImageBitmap,
 * as the JavaScript library preview worker hands its frames over. Requests run one at a time.
 * `release` ends the worker after freeing its device and what it holds in the engine's memory.
 */
import {initSync, Previews} from './pkg/styk_engine.js';
import {libraryPreviewMatte, LIBRARY_MATTES} from '../library-matte.mjs';

/** Side of a library preview and of the samples its backdrop is chosen from (styk_engine::thumbnails::library). */
const SIZE = 320, SAMPLE = 40;

let wasm = null, previews = null, canvas = null, releasing = false, running = Promise.resolve();
/** The hover animation open in the worker: requests of an older session are answered `stale`. */
let session = null;

async function start(message) {
  wasm ??= initSync({module: message.module, memory: message.memory, thread_stack_size: message.stack});
  canvas ??= new OffscreenCanvas(SIZE, SIZE);
  if (!previews) {
    const created = await Previews.create(canvas);
    // Some setups give no pixels from a GPU canvas (Xvfb without ANGLE on Vulkan): check before any preview.
    created.probe();
    const bitmap = canvas.transferToImageBitmap(), sample = new OffscreenCanvas(1, 1), context = sample.getContext('2d', {willReadFrequently: true});
    context.drawImage(bitmap, 0, 0, 1, 1, 0, 0, 1, 1);
    bitmap.close();
    const [r, g, b, a] = context.getImageData(0, 0, 1, 1).data;
    if (!(r > 200 && g < 60 && b > 200 && a > 200)) { created.dispose(); created.free(); throw new Error('the GPU canvas gives no pixels in this worker'); }
    previews = created;
  }
  return {started: true, backend: previews.backend()};
}

/** The canvas's picture after a draw: its pixels move into the bitmap (nothing is read back). */
const picture = () => canvas.transferToImageBitmap();

async function poster({bytes, extension}) {
  const result = JSON.parse(previews.poster(new Uint8Array(bytes), extension));
  const bitmap = picture(), flat = new OffscreenCanvas(SIZE, SIZE);
  flat.getContext('2d').drawImage(bitmap, 0, 0);
  bitmap.close();
  return {...result, blob: await flat.convertToBlob({type: 'image/png'})};
}

function begin() {
  const count = previews.library_begin(), pixels = new Uint8ClampedArray(count * SAMPLE * SAMPLE * 4);
  const sample = new OffscreenCanvas(SAMPLE, SAMPLE), context = sample.getContext('2d', {willReadFrequently: true});
  for (let i = 0; i < count; i++) {
    previews.library_sample(i);
    const bitmap = picture();
    context.clearRect(0, 0, SAMPLE, SAMPLE);
    context.drawImage(bitmap, 0, 0);
    bitmap.close();
    pixels.set(context.getImageData(0, 0, SAMPLE, SAMPLE).data, i * SAMPLE * SAMPLE * 4);
  }
  // One backdrop for the whole hover, chosen across the samples' colours.
  return {samples: count, matte: LIBRARY_MATTES[libraryPreviewMatte(pixels)]};
}

function frame({elapsed}) {
  const result = JSON.parse(previews.library_frame(elapsed));
  return {...result, bitmap: picture()};
}

async function operate(message) {
  if (message.operation === 'start') return start(message);
  if (!previews) throw new Error('The preview worker has not started.');
  if (['library-begin', 'library-frame', 'library-close'].includes(message.operation) && message.session !== session) return {refused: 'stale', message: 'another library preview is open'};
  switch (message.operation) {
    case 'library-poster': return poster(message);
    case 'library-open': session = message.session; return JSON.parse(previews.library_open(new Uint8Array(message.bytes), message.extension));
    case 'library-begin': return begin();
    case 'library-frame': return frame(message);
    case 'library-close': session = null; previews.library_close(); return {};
    default: throw new Error(`Unknown preview operation ${message.operation}.`);
  }
}

/** Frees the device, what this thread holds in the engine's memory and its stack, then ends the worker. */
async function release() {
  releasing = true;
  await running.catch(() => {});
  try { previews?.dispose(); } catch { /* the device may be gone already */ }
  // Destroying the device settles what was pending on it; let those answers run first.
  for (let turn = 0; turn < 4; turn++) await new Promise(resolve => setTimeout(resolve, 0));
  try { previews?.free(); } catch { /* already freed */ }
  previews = null;
  if (canvas) canvas.width = canvas.height = 1;
  canvas = null;
  // No call into the module may follow on this thread (wasm-bindgen's thread destructor).
  try { wasm?.__wbindgen_thread_destroy(); } catch { /* never started */ }
  self.postMessage({released: true});
  self.close();
}

self.addEventListener('message', ({data: message}) => {
  if (message.operation === 'release') { release(); return; }
  const run = running.then(() => {
    if (releasing) throw new DOMException('The preview worker was released.', 'AbortError');
    return operate(message);
  });
  running = run.catch(() => {});
  run.then(result => {
    const transfer = result?.bitmap ? [result.bitmap] : [];
    self.postMessage({id: message.id, ok: true, ...result}, transfer);
  }, error => {
    self.postMessage({id: message.id, ok: false, error: String(error?.message ?? error), name: error?.name});
  });
});
