import {messageError} from './localized-error.mjs';
/** StykAnimate native container codec (worker side).
 *
 * Container version 1: 56-byte header + zlib-compressed UTF-8 JSON document.
 * Container version 2: same 56-byte header (version=2; the size/digest fields
 * cover the JSON skeleton) + a u32 section count + 64-byte section records +
 * section payloads. Inside the skeleton, large values are replaced by string
 * markers resolved against the binary sections:
 *   "S2B:<t>:<s>:<o>:<n>"  numeric array, n elements of type t in section s
 *   "S2S:<s>:<o>:<n>"      string = base64 of the raw byte range
 *   "S2D:<s>:<o>:<n> <p>"  string = literal prefix p + base64 of the range
 *   "S2E:<rest>"           escape for a literal string that starts like a marker
 * Section record (64 bytes): <kind u8, codec u8, reserved u16, pad u32,
 * rawLen u64, storedLen u64, sha256(raw) 32, pad 8>. Kind 1 = numeric arrays,
 * kind 2 = decoded payload blobs.
 * Codec 0 = stored, 1 = zlib-wrapped deflate, 2 = zstd. The skeleton's codec
 * shares byte 5 of the header. A marker resolves to a fresh plain Array (or
 * string); markers with identical text resolve to the *same* array instance so
 * shared pose/key sequences keep their identity through a save/load cycle.
 */
import {base64ToBytes,bytesToBase64} from './base64.mjs';
export {base64ToBytes,bytesToBase64};
const HEADER = 56, SECTION = 64, MAX_BYTES = 256 * 1024 * 1024;
// A sectioned scene's JSON metadata can exceed its compressed-file/section
// limit. Keep it below the engine string ceiling and retain the 1 GiB combined
// materialization budget; source uploads and individual sections stay bounded.
const MAX_SKELETON_BYTES = 384 * 1024 * 1024;
const MAGIC = {styk: 'STYK', stkz: 'STKZ', stykmov: 'STMV'};
const LIFT_MIN_ARRAY = 32, LIFT_MIN_STRING = 64 * 1024;
// Binary64's shortest decimal has at most 17 significant digits. JSON uses
// fixed notation only from 1e-6 to below 1e21, or a signed exponent with at
// most three digits. Including sign and punctuation, 32 bytes bounds either.
const MAX_JSON_NUMBER_BYTES = 32;
const B64_FIELDS = new Set(['data_base64', 'source_base64']);
const TYPES = {u8: Uint8Array, u16: Uint16Array, u32: Uint32Array, i32: Int32Array, f32: Float32Array, f64: Float64Array};
const TYPE_BYTES = {u8: 1, u16: 2, u32: 4, i32: 4, f32: 4, f64: 8};
const MARKER_ARRAY = /^S2B:(u8|u16|u32|i32|f32|f64):(\d+):(\d+):(\d+)$/;
const MARKER_B64 = /^S2S:(\d+):(\d+):(\d+)$/;
const MARKER_DATA = /^S2D:(\d+):(\d+):(\d+):(.*)$/s;
const ESCAPABLE = /^S2[BSED]:/;

const encoder = new TextEncoder(), decoder = new TextDecoder();
const bytesToHex = bytes => [...bytes].map(b => b.toString(16).padStart(2, '0')).join('');

export async function sha256(bytes) {
  const view = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  return new Uint8Array(await crypto.subtle.digest('SHA-256', view));
}

export async function inflate(bytes, limit = MAX_BYTES) {
  if (!Number.isSafeInteger(limit) || limit < 0 || limit > MAX_SKELETON_BYTES)
    throw messageError(new Error(''), "errors.nativeContainer.theStykanimateDocumentExceedsTheUncompressedSizeLimit", {});
  const stream = new Blob([bytes]).stream().pipeThrough(new DecompressionStream('deflate'));
  const reader = stream.getReader(), chunks = [];
  let length = 0;
  try {
    for (;;) {
      const {value, done} = await reader.read();
      if (done) break;
      if (value.byteLength > limit - length) {
        await reader.cancel().catch(() => {});
        throw messageError(new Error(''), "errors.nativeContainer.theStykanimateDocumentExceedsTheUncompressedSizeLimit", {});
      }
      chunks.push(value); length += value.byteLength;
    }
  } finally { reader.releaseLock(); }
  const out = new Uint8Array(length);
  let offset = 0;
  for (const chunk of chunks) { out.set(chunk, offset); offset += chunk.byteLength; }
  return out;
}

export async function deflate(bytes) {
  const stream = new Blob([bytes]).stream().pipeThrough(new CompressionStream('deflate'));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

let zstdModule = null, zstdFailed = false;
export async function zstd() {
  if (zstdModule || zstdFailed) return zstdModule;
  try { zstdModule = await import('../vendor/zstd/index.mjs'); await zstdModule.init(); }
  catch { zstdModule = null; zstdFailed = true; }
  return zstdModule;
}

/** Bounded cache of immutable compressed blocks, owned by an export worker.
 * It retains no editable document data and checks content on every use. */
export function createContainerCompressionCache({maxBytes = 64 * 1024 * 1024, maxEntries = 8} = {}) {
  if (!Number.isSafeInteger(maxBytes) || maxBytes < 0 || !Number.isSafeInteger(maxEntries) || maxEntries < 0)
    throw new RangeError('Invalid compression cache capacity.');
  const entries = new Map();
  let byteLength = 0;
  const insert = (key, blob) => {
    if (blob.size > maxBytes || !maxEntries) return;
    const previous = entries.get(key);
    if (previous) { byteLength -= previous.size; entries.delete(key); }
    while (entries.size >= maxEntries || byteLength + blob.size > maxBytes) {
      const oldest = entries.keys().next().value;
      byteLength -= entries.get(oldest).size; entries.delete(oldest);
    }
    entries.set(key, blob); byteLength += blob.size;
  };
  return {
    get byteLength() { return byteLength; },
    get size() { return entries.size; },
    get(key) {
      const value = entries.get(key);
      if (value) { entries.delete(key); entries.set(key, value); }
      return value;
    },
    put(key, bytes) {
      if (bytes.byteLength > maxBytes || !maxEntries) return;
      insert(key, new Blob([bytes]));
    },
    // Worker retirement transfers only immutable compressed blocks. Snapshot
    // metadata is copied; Blob values remain safe to structured-clone.
    snapshot() { return [...entries].map(([key, blob]) => ({key, blob})); },
    restore(snapshot) {
      entries.clear(); byteLength = 0;
      if (!Array.isArray(snapshot) || !maxEntries) return;
      const keyPattern = /^(?:2:3|1:default):(0|[1-9]\d{0,8}):[a-f0-9]{64}$/;
      for (const entry of snapshot.slice(-maxEntries)) {
        if (!entry || typeof entry.key !== 'string' || entry.key.length > 128 || !(entry.blob instanceof Blob)) continue;
        const match = keyPattern.exec(entry.key);
        if (!match || Number(match[1]) > MAX_SKELETON_BYTES) continue;
        insert(entry.key, entry.blob);
      }
    },
    clear() { entries.clear(); byteLength = 0; },
  };
}

async function packCached(bytes, digest, {cache, stats}) {
  const z = await zstd(), codec = z ? 2 : 1;
  // Include codec and its fixed compression settings; lengths plus SHA-256
  // prevent reused capacity/identity from concealing an edited resource.
  const key = `${codec}:${codec === 2 ? 3 : 'default'}:${bytes.byteLength}:${bytesToHex(digest)}`;
  const cached = cache?.get(key);
  if (cached) {
    if (stats) {
      stats.hits++; stats.rawBytesReused += bytes.byteLength; stats.compressedBytesReused += cached.size;
    }
    return {codec, bytes: cached, length: cached.size};
  }
  if (stats) stats.misses++;
  const stored = z ? await z.compress(bytes, 3) : await deflate(bytes);
  cache?.put(key, stored);
  return {codec, bytes: stored, length: stored.byteLength};
}

async function unpack(codec, bytes, rawLength) {
  if (codec === 0) {
    if (bytes.byteLength !== rawLength) throw messageError(new Error(''), "errors.nativeContainer.theStykanimateFileIsDamagedInvalidPayloadLength", {});
    return bytes;
  }
  if (codec === 1) {
    const out = await inflate(bytes, rawLength);
    if (out.byteLength !== rawLength) throw messageError(new Error(''), "errors.nativeContainer.theStykanimateFileIsDamagedInvalidPayloadLength", {});
    return out;
  }
  if (codec === 2) {
    const z = await zstd();
    if (!z) throw Object.assign(messageError(new Error(''), "errors.nativeContainer.thisProjectUsesZstdCompressionThisBuildCannot", {}), {code: 'codec_unavailable'});
    const out = await z.decompress(bytes, rawLength);
    if (out.byteLength !== rawLength) throw messageError(new Error(''), "errors.nativeContainer.theStykanimateFileIsDamagedInvalidPayloadLength", {});
    return out;
  }
  throw messageError(new Error(''), "errors.nativeContainer.unsupportedStykanimateFileVersionUpdateTheAppTo", {});
}

function parseHeader(bytes, kind) {
  if (bytes.byteLength < HEADER) throw messageError(new Error(''), "errors.nativeContainer.theStykanimateFileIsTruncatedOrHasAn", {});
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  if (decoder.decode(bytes.subarray(0, 4)) !== MAGIC[kind])
    throw messageError(new Error(''), "errors.nativeContainer.theStykanimateFileTypeDoesNotMatchIts", {});
  const version = view.getUint8(4), codec = view.getUint8(5), flags = view.getUint16(6, true);
  if (![1, 2].includes(version) || ![0, 1, 2].includes(codec) || flags !== 0 || (version === 1 && codec !== 1))
    throw messageError(new Error(''), "errors.nativeContainer.unsupportedStykanimateFileVersionUpdateTheAppTo", {});
  const raw = view.getBigUint64(8, true), stored = view.getBigUint64(16, true);
  if (raw <= 0n || raw > BigInt(version === 2 ? MAX_SKELETON_BYTES : MAX_BYTES) || stored > BigInt(MAX_BYTES))
    throw messageError(new Error(''), "errors.nativeContainer.theNativeDocumentExceedsTheUncompressedSizeLimit", {});
  return {view, version, codec, raw: Number(raw), stored: Number(stored), digest: bytes.subarray(24, 56)};
}

const equalBytes = (a, b) => a.length === b.length && a.every((v, i) => v === b[i]);

/** Read any supported container/legacy form. Returns
 *  {document, version, skeletonBytes} — for v2, `document` already has binary
 *  markers materialized; `skeletonBytes` is the raw JSON section payload. */
export async function decodeDocument(data, kind) {
  const bytes = data instanceof Uint8Array ? data : new Uint8Array(data);
  if (bytes.byteLength > MAX_BYTES) throw messageError(new Error(''), "errors.nativeContainer.theNativeFileExceedsTheCompressedSizeLimit", {});
  if (kind === 'styk') {
    let probe = bytes;
    // BOM/whitespace-tolerant plain JSON read compatibility.
    while (probe.length && (probe[0] === 0xef || probe[0] === 0xbb || probe[0] === 0xbf || probe[0] === 32 || probe[0] === 9 || probe[0] === 13 || probe[0] === 10)) probe = probe.subarray(1);
    if (probe[0] === 0x7b) return {document: JSON.parse(decoder.decode(bytes)), version: 0, skeletonBytes: bytes, materializedBytes: bytes.byteLength};
  }
  const header = parseHeader(bytes, kind);
  if (header.version === 1) {
    if (HEADER + header.stored !== bytes.byteLength)
      throw messageError(new Error(''), "errors.nativeContainer.theStykanimateFileIsTruncatedOrHasTrailing", {});
    const payload = await unpack(header.codec, bytes.subarray(HEADER), header.raw);
    if (!(await sha256(payload)).every((b, i) => b === header.digest[i]))
      throw messageError(new Error(''), "errors.nativeContainer.theStykanimateFileIsDamagedChecksumMismatch", {});
    return {document: JSON.parse(decoder.decode(payload)), version: 1, skeletonBytes: payload, materializedBytes: payload.byteLength};
  }
  if (bytes.byteLength < HEADER + 4)
    throw messageError(new Error(''), "errors.nativeContainer.theStykanimateFileIsTruncatedOrHasAn", {});
  // Layout: header(56) | count u32 | compressed skeleton | N x 64B records | payloads.
  const count = header.view.getUint32(HEADER, true);
  const skeletonEnd = HEADER + 4 + header.stored;
  if (count > 64 || skeletonEnd + count * SECTION > bytes.byteLength)
    throw messageError(new Error(''), "errors.nativeContainer.theStykanimateFileIsTruncatedOrHasAn", {});
  const records = [];
  let pos = skeletonEnd + count * SECTION, expected = header.raw;
  for (let i = 0; i < count; i++) {
    const off = skeletonEnd + i * SECTION, view = header.view;
    const section = {kind: view.getUint8(off), codec: view.getUint8(off + 1),
      raw: Number(view.getBigUint64(off + 8, true)), stored: Number(view.getBigUint64(off + 16, true)),
      digest: bytes.subarray(off + 24, off + 56)};
    if (![1, 2].includes(section.kind))
      throw messageError(new Error(''), "errors.nativeContainer.theStykanimateFileHasAnUnknownSectionKind", {});
    if (![0, 1, 2].includes(section.codec) || view.getUint16(off + 2, true) || view.getUint32(off + 4, true) || view.getBigUint64(off + 56, true))
      throw messageError(new Error(''), "errors.nativeContainer.unsupportedStykanimateFileVersionUpdateTheAppTo", {});
    if (!Number.isSafeInteger(section.raw) || !Number.isSafeInteger(section.stored) ||
        section.raw > MAX_BYTES || section.stored > bytes.byteLength - pos ||
        (section.codec === 0 && section.raw !== section.stored))
      throw messageError(new Error(''), "errors.nativeContainer.theStykanimateFileIsTruncatedOrHasAn2", {});
    expected += section.raw * 3; // materialized JSON upper bound (base64 growth)
    records.push({...section, offset: pos}); pos += section.stored;
  }
  if (pos !== bytes.byteLength) throw messageError(new Error(''), "errors.nativeContainer.theStykanimateFileIsTruncatedOrHasTrailing", {});
  if (expected > MAX_BYTES * 4)
    throw messageError(new Error(''), "errors.nativeContainer.theNativeDocumentExceedsTheUncompressedSizeLimit", {});
  const skeleton = await unpack(header.codec, bytes.subarray(HEADER + 4, skeletonEnd), header.raw);
  if (!(await sha256(skeleton)).every((b, i) => b === header.digest[i]))
    throw messageError(new Error(''), "errors.nativeContainer.theStykanimateFileIsDamagedChecksumMismatch", {});
  const sections = [];
  for (const section of records) {
    const raw = await unpack(section.codec, bytes.subarray(section.offset, section.offset + section.stored), section.raw);
    if (!(await sha256(raw)).every((b, i) => b === section.digest[i]))
      throw messageError(new Error(''), "errors.nativeContainer.theStykanimateFileIsDamagedChecksumMismatch", {});
    sections.push({kind: section.kind, bytes: raw});
  }
  const document = JSON.parse(decoder.decode(skeleton));
  const materialized = {};
  return {document: materializeMarkers(document, sections, {metrics: materialized}), version: 2,
    skeletonBytes: skeleton, materializedBytes: Math.max(expected, materialized.byteLength)};
}

/** Replace v2 marker strings with plain arrays/strings. Identical markers
 * resolve to one shared array instance (pose/key sequences rely on identity). */
export function materializeMarkers(node, sections, {maxMaterializedBytes = MAX_BYTES * 4, metrics, validateOnly = false} = {}) {
  const shared = new Map();
  // Charge each reference's reconstructed JSON, rather than only the unique
  // stored bytes. In particular, repeated payload markers allocate new strings.
  // Complete this admission pass before allocating any restored array/string.
  const arrayViews = new Map();
  let materializedBytes = 0, allocationBytes = sections.reduce((sum, section) => sum + section.bytes.byteLength, 0);
  const admit = length => {
    if (!Number.isSafeInteger(length) || length > maxMaterializedBytes - materializedBytes)
      throw messageError(new Error(''), "errors.nativeContainer.theNativeDocumentExceedsTheUncompressedSizeLimit", {});
    materializedBytes += length;
  };
  const admitAllocation = length => {
    if (length > maxMaterializedBytes - allocationBytes)
      throw messageError(new Error(''), "errors.nativeContainer.theNativeDocumentExceedsTheUncompressedSizeLimit", {});
    allocationBytes += length;
  };
  const arrayView = (value, match) => {
    const cached = arrayViews.get(value);
    if (cached) return cached;
    const s = +match[2], offset = +match[3], count = +match[4], type = match[1], width = TYPE_BYTES[type];
    const section = sections[s];
    if (![s, offset, count].every(Number.isSafeInteger) || !section || section.kind !== 1 || offset % width ||
        offset > section.bytes.byteLength || count > Math.floor((section.bytes.byteLength - offset) / width))
      throw messageError(new Error(''), "errors.nativeContainer.theStykanimateFileIsDamagedInvalidArraySection", {});
    // Covers Python numeric objects and list capacity as well as JS arrays.
    admitAllocation(count * 48);
    const bytes = section.bytes, absolute = bytes.byteOffset + offset;
    // Stored sections can begin at an unaligned file offset. Read their little-
    // endian values without assuming the ArrayBuffer's offset is aligned.
    const get = {u8: 'getUint8', u16: 'getUint16', u32: 'getUint32', i32: 'getInt32', f32: 'getFloat32', f64: 'getFloat64'}[type];
    const view = new DataView(bytes.buffer, absolute, count * width);
    const read = i => view[get](i * width, true);
    let jsonBytes = 2 + Math.max(0, count - 1);
    // At least one digit per element; reject implausibly large references
    // before scanning them, even when their section is valid.
    if (jsonBytes + count > maxMaterializedBytes)
      throw messageError(new Error(''), "errors.nativeContainer.theNativeDocumentExceedsTheUncompressedSizeLimit", {});
    for (let i = 0; i < count; i++) {
      const item = read(i);
      if (!Number.isFinite(item)) throw messageError(new Error(''), "errors.nativeContainer.theStykanimateDocumentContainsNonFiniteNumbers", {});
      jsonBytes += String(item).length;
      if (jsonBytes > maxMaterializedBytes)
        throw messageError(new Error(''), "errors.nativeContainer.theNativeDocumentExceedsTheUncompressedSizeLimit", {});
    }
    const result = {count, read, jsonBytes};
    arrayViews.set(value, result);
    return result;
  };
  const payloadRange = match => {
    const s = +match[1], offset = +match[2], count = +match[3], section = sections[s];
    if (![s, offset, count].every(Number.isSafeInteger) || !section || section.kind !== 2 ||
        offset > section.bytes.byteLength || count > section.bytes.byteLength - offset)
      throw messageError(new Error(''), "errors.nativeContainer.theStykanimateFileIsDamagedInvalidPayloadSection", {});
    return {section, offset, count};
  };
  const preflight = value => {
    if (typeof value === 'string') {
      let match = MARKER_ARRAY.exec(value);
      if (match) { admit(arrayView(value, match).jsonBytes); return; }
      if ((match = MARKER_B64.exec(value)) || (match = MARKER_DATA.exec(value))) {
        const {count} = payloadRange(match);
        admitAllocation(4 * Math.ceil(count / 3));
        admit(jsonStringBytes(match[4] || '') + 4 * Math.ceil(count / 3)); return;
      }
      if (value.startsWith('S2E:')) { admit(jsonStringBytes(value.slice(4))); return; }
      if (value.startsWith('S2B:') || value.startsWith('S2S:') || value.startsWith('S2D:'))
        throw messageError(new Error(''), "errors.nativeContainer.theStykanimateFileIsDamagedMalformedSectionMarker", {});
      admit(jsonStringBytes(value)); return;
    }
    if (Array.isArray(value)) {
      admit(2 + Math.max(0, value.length - 1));
      for (const item of value) preflight(item);
    } else if (value && typeof value === 'object') {
      const keys = Object.keys(value); admit(2 + Math.max(0, keys.length - 1));
      for (const key of keys) { admit(jsonStringBytes(key) + 1); preflight(value[key]); }
    } else admit(value == null || typeof value === 'number' && !Number.isFinite(value) ? 4 : String(value).length);
  };
  preflight(node);
  if (metrics) metrics.byteLength = materializedBytes;
  if (validateOnly) return node;
  const resolve = value => {
    let match = MARKER_ARRAY.exec(value);
    if (match) {
      const cached = shared.get(value);
      if (cached) return cached;
      const {count, read} = arrayViews.get(value);
      const array = Array.from({length: count}, (_, i) => read(i));
      shared.set(value, array);
      return array;
    }
    if ((match = MARKER_B64.exec(value)) || (match = MARKER_DATA.exec(value))) {
      const {section, offset, count} = payloadRange(match);
      const encoded = bytesToBase64(section.bytes.subarray(offset, offset + count));
      return match[0][2] === 'S' ? encoded : match[4] + encoded;
    }
    if (value.startsWith('S2E:')) return value.slice(4);
    if (value.startsWith('S2B:') || value.startsWith('S2S:') || value.startsWith('S2D:'))
      throw messageError(new Error(''), "errors.nativeContainer.theStykanimateFileIsDamagedMalformedSectionMarker", {});
    return value;
  };
  const walk = value => {
    if (typeof value === 'string') return value.length > 3 && value.charCodeAt(0) === 83 ? resolve(value) : value;
    if (Array.isArray(value)) { for (let i = 0; i < value.length; i++) value[i] = walk(value[i]); return value; }
    if (value && typeof value === 'object')
      for (const key of Object.keys(value)) {
        const walked = walk(value[key]);
        if (walked === value[key]) continue;
        if (key === '__proto__') Object.defineProperty(value, key, {value: walked, writable: true, configurable: true, enumerable: true});
        else value[key] = walked;
      }
    return value;
  };
  return walk(node);
}

/** Drain the same lifted owner for synchronous container/worker callers. */
function drainLift(steps) {
  let step;
  do { step = steps.next(); } while (!step.done);
  return step.value;
}

/** UTF-8 length of a JSON string, with a checkpoint within long strings. */
function* jsonStringBytesSteps(value) {
  let bytes = 2, checkpoint = 2047;
  for (let i = 0; i < value.length; i++) {
    const code = value.charCodeAt(i);
    if (code === 34 || code === 92 || code === 8 || code === 9 || code === 10 || code === 12 || code === 13) bytes += 2;
    else if (code < 32) bytes += 6;
    else if (code < 128) bytes++;
    else if (code < 2048) bytes += 2;
    else if (code >= 0xd800 && code <= 0xdbff && value.charCodeAt(i + 1) >= 0xdc00 && value.charCodeAt(i + 1) <= 0xdfff) { bytes += 4; i++; }
    else bytes += code >= 0xd800 && code <= 0xdfff ? 6 : 3;
    if (i >= checkpoint) { yield null; checkpoint = i + 2048; }
  }
  return bytes;
}
function jsonStringBytes(value) { return drainLift(jsonStringBytesSteps(value)); }

/** The one exact numeric-array type selection, shared by both callers. */
function* arrayTypeSteps(values) {
  let ints = true, min = Infinity, max = -Infinity, f32 = true;
  for (let index = 0; index < values.length; index++) {
    const v = values[index];
    if (typeof v !== 'number' || !Number.isFinite(v)) return null;
    if (Number.isInteger(v)) {
      if (Math.abs(v) >= 2 ** 53) return null;
      if (v < min) min = v; if (v > max) max = v;
    } else ints = false;
    if (f32 && Math.fround(v) !== v) f32 = false;
    if ((index & 2047) === 2047) yield null;
  }
  if (ints) {
    if (min >= 0) {
      if (max <= 0xff) return 'u8';
      if (max <= 0xffff) return 'u16';
      if (max <= 0xffffffff) return 'u32';
    } else if (min >= -0x80000000 && max <= 0x7fffffff) return 'i32';
    return 'f64';
  }
  return f32 ? 'f32' : 'f64';
}

/** Canonical base64 validation never decodes or hashes a whole payload at once. */
function* canonicalBase64LengthSteps(value) {
  if (value.length % 4) return null;
  const end = value.length - (value.endsWith('==') ? 2 : value.endsWith('=') ? 1 : 0);
  let last = 0;
  for (let i = 0; i < end; i++) {
    const code = value.charCodeAt(i);
    if (code >= 65 && code <= 90) last = code - 65;
    else if (code >= 97 && code <= 122) last = code - 71;
    else if (code >= 48 && code <= 57) last = code + 4;
    else if (code === 43) last = 62;
    else if (code === 47) last = 63;
    else return null;
    if ((i & 2047) === 2047) yield null;
  }
  const padding = value.length - end;
  if ((padding === 2 && (last & 15)) || (padding === 1 && (last & 3))) return null;
  return value.length / 4 * 3 - padding;
}

/** Bounded exact comparison: string-key hashing/equality must not flatten big blobs. */
function* blobFingerprint(value) {
  let hash = 2166136261;
  for (let i = 0; i < value.length; i++) {
    hash = Math.imul(hash ^ value.charCodeAt(i), 16777619);
    if ((i & 2047) === 2047) yield null;
  }
  return `${value.length}:${hash >>> 0}`;
}
function* sameBlob(a, b) {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    if (a.charCodeAt(i) !== b.charCodeAt(i)) return false;
    if ((i & 2047) === 2047) yield null;
  }
  return true;
}

/** Existing numeric encoding, in fragments small enough to copy through a binding. */
function* numberFragments(type, value, offset) {
  for (let at = 0; at < value.length; at += 2048) {
    const typed = new TYPES[type](value.slice(at, at + 2048));
    yield {kind: 'section', section: 0, offset: offset + at * TYPE_BYTES[type], bytes: new Uint8Array(typed.buffer)};
  }
}
function* byteFragments(bytes, section, offset) {
  for (let at = 0; at < bytes.byteLength; at += 16384)
    yield {kind: 'section', section, offset: offset + at, bytes: bytes.subarray(at, at + 16384)};
}

/** JSON.stringify's scalar escaping, subdivided before a large escaped allocation. */
function* scalarFragments(value, array = false, prefix = '') {
  if (typeof value !== 'string') {
    const text = JSON.stringify(value);
    yield text ?? (array ? 'null' : '');
    return;
  }
  yield '"';
  if (prefix) yield JSON.stringify(prefix).slice(1, -1);
  for (let at = 0; at < value.length;) {
    let end = Math.min(value.length, at + 512);
    // Preserve a paired surrogate across the escape chunks exactly as JSON.stringify.
    if (end < value.length && value.charCodeAt(end - 1) >= 0xd800 && value.charCodeAt(end - 1) <= 0xdbff && value.charCodeAt(end) >= 0xdc00 && value.charCodeAt(end) <= 0xdfff) end--;
    yield JSON.stringify(value.slice(at, end)).slice(1, -1);
    at = end;
  }
  yield '"';
}

/**
 * The one lifted document walk. Null yields are work checkpoints; stream mode yields
 * UTF-8-compatible skeleton text and offset binary fragments, then section lengths.
 * A stream caller supplies a worker/canonical key manifest for every object: this
 * owner never discovers cold own keys on the interface thread. Incomplete streams
 * are discarded atomically by their transport, and parsed/validated only on a worker.
 */
function* liftSteps(document, {preBlobs = [], maxBytes = MAX_BYTES, stream = false, keyManifest} = {}) {
  const arrays = [], blobs = [], seenArrays = new Map(), seenBlobs = new Map(), preBlobRanges = new Map();
  const stringSizes = new Map();
  const stringSize = function*(value) {
    if (value.length > 128) return yield* jsonStringBytesSteps(value);
    const cached = stringSizes.get(value);
    if (cached !== undefined) return cached;
    const length = yield* jsonStringBytesSteps(value);
    if (stringSizes.size < 512) stringSizes.set(value, length);
    return length;
  };
  let arraysLength = 0, blobsLength = 0, jsonUpperBytes = 0, knownJSONBytes = 0, allocationBytes = 0;
  const maxMaterializedBytes = maxBytes * 4;
  const admit = (length, estimated = false) => {
    if (!Number.isSafeInteger(length)) throw documentTooLarge();
    if (!estimated) {
      if (length > maxMaterializedBytes - knownJSONBytes) throw documentTooLarge();
      knownJSONBytes += length;
    }
    jsonUpperBytes += length;
  };
  const checkAllocation = () => {
    if (allocationBytes + arraysLength + blobsLength > maxMaterializedBytes) throw documentTooLarge();
  };
  const admitPayload = function*(prefix, encodedLength) {
    admit((yield* stringSize(prefix)) + encodedLength);
    allocationBytes += encodedLength; checkAllocation();
  };
  const blobOffsets = [];
  for (const bytes of preBlobs) {
    if (bytes.byteLength > maxBytes - blobsLength) throw documentTooLarge();
    if (!preBlobRanges.has(bytes)) preBlobRanges.set(bytes, {s: 1, o: blobsLength, n: bytes.byteLength});
    blobOffsets.push(blobsLength);
    if (stream) yield* byteFragments(bytes, 1, blobsLength); else blobs.push(bytes);
    blobsLength += bytes.byteLength;
    yield null;
  }
  const align = (length, type) => length + ((TYPE_BYTES[type] - (length % TYPE_BYTES[type])) % TYPE_BYTES[type]);
  // Reuse one small frame per nesting depth. Dense records must not allocate a
  // generator/closure/temporary frame for every scalar on the interface.
  const frames = [], frameAt = depth => frames[depth] ??= {kind: 0, source: null, out: null, keys: null, index: 0, key: '', slot: '', present: true, array: false, emitted: false};
  const root = frameAt(0); root.source = document;
  let depth = 0, skeleton;
  const finish = lifted => {
    const child = frames[depth--];
    if (stream) return;
    if (depth < 0) { skeleton = lifted; return; }
    const parent = frames[depth];
    if (child.present) {
      if (child.slot === '__proto__') Object.defineProperty(parent.out, child.slot, {value: lifted, writable: true, configurable: true, enumerable: true});
      else parent.out[child.slot] = lifted;
    }
  };
  while (depth >= 0) {
    const frame = frames[depth], value = frame.source;
    if (frame.kind !== 0) {
      const count = frame.kind === 1 ? value.length : frame.keys.length;
      if (frame.index >= count) {
        if (stream) yield frame.kind === 1 ? ']' : '}';
        finish(frame.out); continue;
      }
      const key = frame.kind === 1 ? frame.index++ : frame.keys[frame.index++];
      if (frame.kind === 2) admit((stringSizes.get(key) ?? (yield* stringSize(key))) + 1);
      const next = value[key];
      if (stream && frame.kind === 2 && (next === undefined || typeof next === 'function' || typeof next === 'symbol')) continue;
      if (stream) {
        if (frame.emitted) yield ',';
        if (frame.kind === 2) { if (key.length <= 512) yield JSON.stringify(key); else yield* scalarFragments(key); yield ':'; }
        frame.emitted = true;
      }
      const child = frameAt(++depth);
      child.kind = 0; child.source = next; child.key = frame.kind === 1 ? '' : key; child.slot = key;
      child.present = frame.kind !== 1 || key in value; child.array = frame.kind === 1;
      if (stream) yield null;
      continue;
    }
    const preBlob = B64_FIELDS.has(frame.key) && preBlobRanges.get(value);
    if (preBlob) {
      yield* admitPayload('', 4 * Math.ceil(preBlob.n / 3));
      const marker = `S2S:${preBlob.s}:${preBlob.o}:${preBlob.n}`;
      if (stream) yield JSON.stringify(marker);
      finish(marker); continue;
    }
    if (typeof value === 'string') {
      let payload = false;
      if (value.length >= LIFT_MIN_STRING) {
        let base64 = null, prefix = '';
        if (B64_FIELDS.has(frame.key)) base64 = value;
        else if ((frame.key === 'data_url' || frame.key === 'styk_tiles') && value.startsWith('data:')) {
          let comma = -1;
          for (let at = 0; at < value.length; at += 2048) {
            const found = value.slice(at, at + 2048).indexOf(',');
            if (found >= 0) { comma = at + found; break; }
            yield null;
          }
          if (comma > 5 && value.slice(0, comma).endsWith(';base64')) { base64 = value.slice(comma + 1); prefix = value.slice(0, comma + 1); }
        }
        if (base64 !== null) {
          const decodedLength = yield* canonicalBase64LengthSteps(base64);
          if (decodedLength !== null) {
            const fingerprint = yield* blobFingerprint(base64), candidates = seenBlobs.get(fingerprint) ?? [];
            let hit;
            for (const candidate of candidates) if (yield* sameBlob(candidate.value, base64)) { hit = candidate; break; }
            if (hit) {
              yield* admitPayload(prefix, base64.length);
              const lead = prefix ? `S2D:${hit.s}:${hit.o}:${hit.n}:` : `S2S:${hit.s}:${hit.o}:${hit.n}`;
              if (stream) yield* scalarFragments(prefix, false, lead);
              finish(stream ? null : lead + prefix); payload = true;
            } else {
              if (decodedLength > maxBytes - blobsLength) throw documentTooLarge();
              yield* admitPayload(prefix, base64.length);
              const s = 1, o = blobsLength, n = decodedLength;
              const owned = stream ? null : new Uint8Array(n);
              let offset = 0;
              for (let at = 0; at < base64.length; at += 16384) {
                const bytes = base64ToBytes(base64.slice(at, at + 16384));
                if (stream) yield {kind: 'section', section: 1, offset: o + offset, bytes};
                else { owned.set(bytes, offset); yield null; }
                offset += bytes.byteLength;
              }
              if (!stream) blobs.push(owned);
              blobsLength += n; candidates.push({value: base64, s, o, n}); seenBlobs.set(fingerprint, candidates);
              checkAllocation();
              const lead = prefix ? `S2D:${s}:${o}:${n}:` : `S2S:${s}:${o}:${n}`;
              if (stream) yield* scalarFragments(prefix, false, lead);
              finish(stream ? null : lead + prefix); payload = true;
            }
          }
        }
      }
      if (payload) continue;
      admit((value.length <= 128 ? stringSizes.get(value) : undefined) ?? (yield* stringSize(value)));
      const escaped = ESCAPABLE.test(value);
      if (stream) {
        if (value.length <= 512) yield JSON.stringify(escaped ? 'S2E:' + value : value);
        else yield* scalarFragments(value, frame.array, escaped ? 'S2E:' : '');
      }
      finish(stream ? null : escaped ? 'S2E:' + value : value); continue;
    }
    if (Array.isArray(value)) {
      const hit = seenArrays.get(value);
      if (hit) { admit(hit.jsonUpperBytes, true); if (stream) yield JSON.stringify(hit.marker); finish(hit.marker); continue; }
      const type = value.length >= LIFT_MIN_ARRAY ? yield* arrayTypeSteps(value) : null;
      if (type) {
        const jsonUpperBytes = 2 + Math.max(0, value.length - 1) + value.length * MAX_JSON_NUMBER_BYTES;
        admit(jsonUpperBytes, true);
        const offset = align(arraysLength, type);
        if (value.length > Math.floor((maxBytes - offset) / TYPE_BYTES[type])) throw documentTooLarge();
        if (stream && offset > arraysLength) yield {kind: 'section', section: 0, offset: arraysLength, bytes: new Uint8Array(offset - arraysLength)};
        arraysLength = offset + value.length * TYPE_BYTES[type];
        allocationBytes += value.length * 48; checkAllocation();
        const marker = `S2B:${type}:0:${offset}:${value.length}`;
        if (stream) yield* numberFragments(type, value, offset); else arrays.push({type, value, offset});
        seenArrays.set(value, {marker, jsonUpperBytes});
        if (stream) yield JSON.stringify(marker);
        finish(marker); continue;
      }
      admit(2 + Math.max(0, value.length - 1));
      frame.kind = 1; frame.out = stream ? null : new Array(value.length); frame.keys = null; frame.index = 0; frame.emitted = false;
      if (stream) yield '[';
      continue;
    }
    if (value && typeof value === 'object') {
      const keys = stream ? keyManifest?.get(value) : Object.keys(value);
      if (!keys) throw Object.assign(new Error('The canonical record keys are not ready.'), {code: 'engine_keys_not_ready'});
      admit(2 + Math.max(0, keys.length - 1));
      frame.kind = 2; frame.out = stream ? null : {}; frame.keys = keys; frame.index = 0; frame.emitted = false;
      if (stream) yield '{';
      continue;
    }
    if (typeof value === 'number' && Number.isFinite(value)) admit(MAX_JSON_NUMBER_BYTES, true);
    else admit(value == null || typeof value === 'number' ? 4 : String(value).length);
    if (stream) yield JSON.stringify(value) ?? (frame.array ? 'null' : '');
    finish(value);
  }
  if (stream) return {sectionLengths: [arraysLength, blobsLength], blobOffsets, materializedUpperBytes: jsonUpperBytes, materializationLimit: maxMaterializedBytes};
  const arraysBytes = new Uint8Array(arraysLength);
  for (const entry of arrays) for (const fragment of numberFragments(entry.type, entry.value, entry.offset)) { arraysBytes.set(fragment.bytes, fragment.offset); yield null; }
  const blobsBytes = new Uint8Array(blobsLength);
  let offset = 0;
  for (const bytes of blobs) {
    for (const fragment of byteFragments(bytes, 1, offset)) { blobsBytes.set(fragment.bytes, fragment.offset); yield null; }
    offset += bytes.byteLength;
  }
  const sections = [{kind: 1, bytes: arraysBytes}, {kind: 2, bytes: blobsBytes}];
  if (jsonUpperBytes > maxMaterializedBytes) {
    try { materializeMarkers(skeleton, sections, {maxMaterializedBytes, validateOnly: true}); }
    catch (error) { if (!error.code) error.code = 'document_too_large'; throw error; }
  }
  return {skeleton, sections, blobOffsets};
}

/**
 * Incremental lifted owner. Skeleton tokens are joined within 8192 UTF-16 code
 * units; binary chunks are at most 16384 bytes. Stream completion is not validation:
 * its receiver validates materialization limits atomically on a worker before ACK.
 */
export function* liftDocumentSteps(document, options = {}) {
  const steps = liftSteps(document, options);
  let text = '', step;
  try {
    for (;;) {
      step = steps.next();
      if (step.done) { if (text) yield {kind: 'skeleton', text}; return step.value; }
      const item = step.value;
      if (typeof item === 'string') {
        if (text.length + item.length > 8192) { yield {kind: 'skeleton', text}; text = ''; }
        text += item;
      } else if (item === null) {
        // A work checkpoint need not turn every small property into a packet.
        yield null;
      } else {
        if (text) { yield {kind: 'skeleton', text}; text = ''; }
        yield item;
      }
    }
  } finally {
    steps.return();
  }
}

/** Synchronous container callers share all marker/packing/escaping decisions. */
export function liftDocument(document, options = {}) {
  return drainLift(liftDocumentSteps(document, {preBlobs: options.preBlobs, maxBytes: options.maxBytes}));
}

function documentTooLarge() {
  return Object.assign(messageError(new Error(''), "errors.nativeContainer.theStykanimateDocumentExceedsTheUncompressedSizeLimit", {}), {code: 'document_too_large'});
}

/** Assemble v2 from bounded parts; Blob consumers avoid a whole-file copy. */
export async function encodeContainerV2Parts(lifted, kind, options = {}) {
  const {cache, stats} = options;
  if (stats) Object.assign(stats, {hits: 0, misses: 0, rawBytesReused: 0, compressedBytesReused: 0});
  const skeletonBytes = encoder.encode(JSON.stringify(lifted.skeleton));
  const sectionRawBytes = lifted.sections.reduce((sum, section) => sum + section.bytes.byteLength, 0);
  if (skeletonBytes.byteLength > MAX_SKELETON_BYTES ||
      lifted.sections.some(section => section.bytes.byteLength > MAX_BYTES) ||
      skeletonBytes.byteLength + sectionRawBytes * 3 > MAX_BYTES * 4)
    throw Object.assign(messageError(new Error(''), "errors.nativeContainer.theStykanimateDocumentExceedsTheUncompressedSizeLimit", {}), {code:'document_too_large'});
  if (!Object.hasOwn(MAGIC, kind) || lifted.sections.length > 64 || lifted.sections.some(section =>
      ![1, 2].includes(section.kind) || !(section.bytes instanceof Uint8Array)))
    throw messageError(new Error(''), "errors.nativeContainer.unsupportedStykanimateFileVersionUpdateTheAppTo", {});
  const skeletonDigest = await sha256(skeletonBytes);
  const jsonPacked = await packCached(skeletonBytes, skeletonDigest, options);
  const packed = [];
  for (const section of lifted.sections) {
    const digest = await sha256(section.bytes);
    const result = await packCached(section.bytes, digest, options);
    packed.push({kind: section.kind, codec: result.codec, raw: section.bytes, stored: result.bytes,
      storedLength: result.length, digest});
  }
  let total = HEADER + 4 + jsonPacked.length + packed.length * SECTION;
  for (const section of packed) total += section.storedLength;
  if (total > MAX_BYTES) throw Object.assign(messageError(new Error(''), "errors.nativeContainer.theNativeFileExceedsTheCompressedSizeLimit", {}), {code:'document_too_large'});
  const out = new Uint8Array(HEADER + 4), view = new DataView(out.buffer);
  out.set(encoder.encode(MAGIC[kind]), 0);
  view.setUint8(4, 2); view.setUint8(5, jsonPacked.codec); view.setUint16(6, 0, true);
  view.setBigUint64(8, BigInt(skeletonBytes.byteLength), true);
  view.setBigUint64(16, BigInt(jsonPacked.length), true);
  out.set(skeletonDigest, 24);
  view.setUint32(HEADER, packed.length, true);
  const records = new Uint8Array(packed.length * SECTION), table = new DataView(records.buffer);
  let pos = 0;
  for (const section of packed) {
    table.setUint8(pos, section.kind); table.setUint8(pos + 1, section.codec);
    table.setBigUint64(pos + 8, BigInt(section.raw.byteLength), true);
    table.setBigUint64(pos + 16, BigInt(section.storedLength), true);
    records.set(section.digest, pos + 24);
    pos += SECTION;
  }
  if (stats) Object.assign(stats, {cacheBytes: cache?.byteLength || 0, cacheEntries: cache?.size || 0,
    container_raw_bytes: skeletonBytes.byteLength + sectionRawBytes});
  return {parts: [out, jsonPacked.bytes, records, ...packed.map(section => section.stored)], byteLength: total};
}

/** Byte-array compatibility adapter for callers that require contiguous data. */
export async function encodeContainerV2(lifted, kind, options) {
  const {parts, byteLength} = await encodeContainerV2Parts(lifted, kind, options);
  const out = new Uint8Array(byteLength);
  let pos = 0;
  for (const part of parts) {
    const bytes = part instanceof Blob ? new Uint8Array(await part.arrayBuffer()) : part;
    out.set(bytes, pos); pos += bytes.byteLength;
  }
  return out;
}

/** Lift + assemble in one step. */
export async function buildContainerV2(document, kind, options) {
  return encodeContainerV2(liftDocument(document, options), kind, options);
}

/** Serialize a JSON payload as container v1 (compatibility/testing). */
export async function buildContainerV1(payload, kind) {
  const bytes = payload instanceof Uint8Array ? payload : encoder.encode(payload);
  const stored = await deflate(bytes);
  const out = new Uint8Array(HEADER + stored.byteLength), view = new DataView(out.buffer);
  out.set(encoder.encode(MAGIC[kind]), 0);
  view.setUint8(4, 1); view.setUint8(5, 1); view.setUint16(6, 0, true);
  view.setBigUint64(8, BigInt(bytes.byteLength), true);
  view.setBigUint64(16, BigInt(stored.byteLength), true);
  out.set(await sha256(bytes), 24);
  out.set(stored, HEADER);
  return out;
}

/** The self-contained figure shape the file service enforces for .stkz/.stykmov. */
export function validateNativeFigure(document, kind = 'stkz') {
  const figures = document?.figure_types, frames = document?.keyframes;
  if (!Array.isArray(figures) || !figures.length || figures.some(f => !f || typeof f !== 'object') ||
      !Array.isArray(frames) || frames.length !== 1 || !frames[0] || typeof frames[0] !== 'object' ||
      !Array.isArray(frames[0].objects) || !frames[0].objects.length ||
      frames[0].objects.some(o => !o || typeof o !== 'object'))
    throw Object.assign(messageError(new Error(''), "errors.nativeContainer.aFileMustContainReusableFiguresAndOne", {kind: kind}), {code: 'invalid_native_figure'});
  const objects = frames[0].objects, ids = new Set(figures.map(f => f.figure_type_id)), meta = document._styk || {};
  if (typeof meta !== 'object' || objects.some(o => !ids.has(o.figure_type_id)) ||
      document.backgrounds?.length || meta.animation || meta.simpleFrameDraft || meta.layerSheet)
    throw Object.assign(messageError(new Error(''), "errors.nativeContainer.theFigureMustBeSelfContained", {kind: kind}), {code: 'invalid_native_figure'});
  for (const pose of objects) {
    const join = pose.join;
    if (join != null && (typeof join !== 'object' || !Number.isInteger(join.target_object_slot) ||
        join.target_object_slot < 0 || join.target_object_slot >= objects.length))
      throw Object.assign(messageError(new Error(''), "errors.nativeContainer.figureSetJoinsMustStayInsideTheSaved", {}), {code: 'invalid_native_figure'});
  }
  if (kind === 'stykmov' && (figures.length !== 1 || objects.length !== 1 || objects[0].join != null ||
      !figures[0]._styk?.movieclip))
    throw Object.assign(messageError(new Error(''), "errors.nativeContainer.aStykmovFileMustContainOneMovieclip", {}), {code: 'invalid_movieclip'});
}

/** Verify the embedded original source the way the service's source_bytes does:
 *  strict base64, size bound, and the recorded fingerprint. */
export async function verifyEmbeddedSource(document) {
  const metadata = document?._stykSource;
  if (!metadata) return;
  const encoded = metadata.source_base64;
  if (typeof encoded !== 'string' || encoded.length > 256 * 1024 * 1024 * 4 / 3 + 8)
    throw messageError(new Error(''), "errors.nativeContainer.invalidEmbeddedOriginalSource", {});
  let bytes;
  try { bytes = base64ToBytes(encoded); } catch { throw messageError(new Error(''), "errors.nativeContainer.corruptedEmbeddedOriginalSource", {}); }
  if (bytesToBase64(bytes) !== encoded) throw messageError(new Error(''), "errors.nativeContainer.corruptedEmbeddedOriginalSource", {});
  const digest = await sha256(bytes);
  if (bytesToHex(digest) !== metadata.sha256)
    throw messageError(new Error(''), "errors.nativeContainer.embeddedOriginalSourceFingerprintDoesNotMatch", {});
}

/** Apply the decode-time document normalization the file service performs:
 *  wrapper unwrap, native-figure shape check, browser-field removal. */
export function normalizeDecodedDocument(document, kind) {
  if (kind === 'styk' && document && document.format === 'stykanimate') {
    if (document.version !== 1 || !document.document || typeof document.document !== 'object' || Array.isArray(document.document))
      throw Object.assign(messageError(new Error(''), "errors.nativeContainer.unsupportedStykanimateProjectVersion", {}), {code: 'invalid_native_version'});
    document = document.document;
  }
  if (!document || typeof document !== 'object' || Array.isArray(document))
    throw Object.assign(messageError(new Error(''), "errors.nativeContainer.projectDataMustBeAJsonObject", {}), {code: 'invalid_document'});
  if (kind === 'stkz' || kind === 'stykmov') validateNativeFigure(document, kind);
  const cleaned = {...document};
  delete cleaned.editable_paths; delete cleaned._codec; delete cleaned._stykWeb;
  return cleaned;
}
