# Engine: Pivot files and the codec service

How the Rust engine (`engine/crates/styk-piv`) reads and writes Pivot `.piv`/`.stk` files and answers the codec API. It replaces the Python codec (`codecs/`, `backend/server.py`, `backend/browser_service.py`) and its C++ frame scanner; while both exist the Python codec is the oracle, and the crate must answer what it answers.

## Contract

- **Reading.** `decode_document(bytes, filename, cancel)` returns the editable document as a `styk_doc::Value` tree: the report the Python decoder returned (same keys, same order, same values) after the import checks. Pose arrays are `Value::Nums` built straight from the file's floats (`F32` when every value is a float, `F64` otherwise), so doubles that are not float-exact and negative zero are the file's values. Embedded images are `Value::Blob` (the bytes, shared, plus the `data:…;base64,` prefix).
- **Writing.** `encode_piv(document, source, cancel)` writes a Pivot 5 project: against its `source` when there is one (media and trailer bytes kept, the source itself returned when the rebuilt payload is identical, a legacy source migrated), from a blank scaffold otherwise. `encode_stk(document, figure_type_id, cancel)` writes one figure. Output bytes are the Python writers' bytes, including the zlib stream (`src/zlib` is a bit-exact port of zlib 1.3.1 level 6).
- **Service.** `Codec::new(Limits::web() | Limits::desktop())` and `Codec::request(method, target, headers, body, cancel) -> Response` answer the routes of `browser_service.BrowserCodec.request`: `GET /api/health`, `/api/samples`, `/api/source?id=`; `POST /api/cancel`, `/api/import?filename=` (JSON, or the `STYKPIV1` frame transport when `Accept` names `application/vnd.stykanimate.piv-frames-v1`), `/api/export`, `/api/open` (404), `/api/source/retain`, `/api/source/release`. Same statuses, headers, size limits, cache and lease rules, and error bodies (`{"ok":false,"error","code"}`, as `server.error_response` built them). `request` never panics; every failure is an error response.
- **"Unchanged" exports** return the original file. The decision compares document fingerprints (`src/digest.rs`, version 3 of `server.document_digest`), which native saves store, so their bytes are a file-format contract.
- **Native documents** (`.styk`, `.stkz`, `.stykmov`, `.json`) go through the `NativeDocuments` trait; `Containers` implements it over `styk-formats` (feature `containers`, on by default). Without it those requests answer `415`.
- **No threads, files or clock** in the library. Timing fields are zero unless the host calls `Codec::set_clock`. Cancellation is cooperative: `cancel` is polled where Python called `work_control.checkpoint` (per frame, per record, per table), and a cancelled request answers `499 cancelled` without changing the cache or any lease. The crate builds for `wasm32-unknown-unknown` with and without `containers`.

## Modules

| Module (`src/`) | Holds | Python |
| --- | --- | --- |
| `service.rs` | `Codec`, `Limits`, `Response`, routing, URL and query parsing | `browser_service.py`, `server.Handler` |
| `store.rs` | source cache (LRU, byte and entry limits), leases, import, export | `server.ProjectStore` |
| `piv/mod.rs`, `piv/v5.rs` | container unpacking, the Pivot 5 grammar, figures, backgrounds | `piv_codec.py`, `piv_document.py` |
| `piv/frames.rs` | the frame table: build, validate or scan; duplicate-ID recovery; pose budget | `fast_decode.py`, `native_scan.py`, `native/piv-frame-scan.cpp` |
| `piv/side_tables.rs` | image and text records, glyph outlines, anchors | `piv_side_tables.py` |
| `piv/legacy*.rs` | Pivot 2-4, keyed sprites as PNG | `piv_legacy.py`, `piv_legacy_v2.py`, `piv_legacy_v4.py`, `pivot_decoder.py` (layout detection) |
| `piv/image_stream.rs` | where an embedded JPEG really ends | `image_stream.py` |
| `write/` | frames-only writer, full rebuild, media tables, legacy migration | `piv_full_writer.py`, `piv_asset_writer.py`, `piv_migrate.py` |
| `bridge.rs` | document validation, STK import/export, scaffold, media aliases | `codec_bridge.py` |
| `wire.rs` | the `STYKPIV1` frame transport | `import_wire.py` |
| `native.rs` | `NativeDocuments`, `Containers`, the service's native-document rules | `server.py` (native parts), `native_format.py` (via `styk-formats`) |
| `digest.rs`, `pyjson.rs`, `pyfmt.rs` | fingerprints; Python's JSON text, number text and parse errors | `server.document_digest`, `json`, `repr(float)` |
| `py.rs`, `error.rs`, `ctx.rs` | Python semantics the codec relied on (truthiness, `==`, indexing errors), exception classes, cancellation | — |
| `zlib/` | deflate (bit-exact) and inflate (zlib's error messages and order) | `zlib` |

## Invariants

- **Errors are Python's.** Each failure carries the Python exception class it was (`error::Exc`), which decides the response as `server.error_response` did: service errors keep their status and code; `PivError`, `ValueError`, `TypeError`, `KeyError`, `IndexError`, `struct.error` are `422 invalid_project` with Python's message; `AttributeError`, `OverflowError`, `zlib.error`, `RuntimeError` are `500 codec_error`. Messages are reproduced, including those Python produced by accident (a `KeyError` text is the key's `repr`).
- **A client's document is JavaScript's.** The service reads request JSON with `styk_doc::json::parse` (one number type). Fingerprints of a client's document spell numbers as Python saw the JavaScript text: integers between 2^53 and 2^64 as their padded digits, so such a value never fingerprints like the decoded file (`pyjson::Origin`). Python could tell `1e18` (float) from `1000000000000000000` (int); the engine cannot, and assumes a JavaScript client.
- **Bounded work.** Uploads are limited before decoding; the pose budget is charged before the data it admits is read; zlib output is capped at the size limits; recursion in document walks is iterative or depth-limited.
- **Frame transport bytes are the file's**, except renamed duplicate object IDs, which are patched at their recorded offsets.

## Adding a format field

1. Read it where the grammar has it (`piv/v5.rs` for project and figure tables, `piv/frames.rs` for per-object or per-frame data, `piv/side_tables.rs` for media), and set it on the report in the position the Python decoder did, with `ctx::key`/`ObjB` so records share their layout.
2. Write it back in `write/` (frames-only and full writers) and, if the editor may change it, compare it in `write::frames::same_signature` or the figure planner.
3. If Python validated it, add the check to `bridge::validate_document` with Python's message.
4. Regenerate fixtures (`engine/tools/piv/fixtures.py`) with a file that has the field, and run the parity replay.

## Testing

- `cargo test -p styk-piv`: unit tests next to the code, and `tests/service.rs`, `tests/formats.rs` (the behaviours the Python tests assert, on generated fixtures). `tests/fixtures/expected.json` and `upgraded-legacy-v*.json` are what the Python codec answered for the fixtures; `.venv/bin/python engine/tools/piv/fixtures.py` regenerates fixtures and answers, `--check` verifies them.
- Parity with the Python codec: `engine/tools/piv/parity.sh` records the oracle (`oracle.py`) and replays it (`styk-piv replay`), comparing import JSON (as parsed documents: numbers equal as doubles, negative zero included), `STYKPIV1` bytes, export bytes, native containers (as documents) and error responses. Suites that need the private corpus (`~/Downloads/pivs`, `WafflesPack`, `Valthim - Yuki.piv`) are skipped without it. `STYK_PIV_PARITY=<dir> cargo test -p styk-piv --test parity` replays an existing recording.
- Timing: `styk-piv import <file> --time` (one import, as the service does it), `styk-piv profile <file>` (inflate, decode, JSON text, hashing).
