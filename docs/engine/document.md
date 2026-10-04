# Engine: the document store

How the engine holds a StykAnimate document (`engine/crates/styk-doc`) and how
the typed views evaluation reads are derived from it (`styk_eval::load`). The
file formats are in [formats.md](formats.md); why the store is shaped this way
is [D2](decisions/0002-lossless-document-store.md).

## Contract

- A document is a tree of `styk_doc::Value`: `Null`, `Bool`, `Num(f64)`,
  `Str(Arc<str>)`, `Arr(Arc<Vec<Value>>)`, `Nums(Arc<Nums>)` (a packed numeric
  array, `F32` or `F64`), `Obj(Arc<Obj>)` and `Blob(Arc<Blob>)` (binary payload
  with its `data:` prefix). Every node is shared and copy-on-write: an edit
  clones the path to the record it changes, never the document.
- Objects keep JavaScript's key order: integer-like keys first in ascending
  order, then insertion order (`Obj`, `index_key`). Objects of the same shape
  share one key list (`Shape`).
- `styk_doc::json` reads and writes JSON exactly as JavaScript does: numbers in
  `JSON.stringify`'s spelling (`number::format_number`, ryu-js), strings with
  JavaScript's escapes, nesting up to 512 levels. Writing a value read from
  JavaScript gives JavaScript's bytes back.
- Typed views (`styk_eval::doc::Document`: figures, keyframes, pose sources,
  layers, layer sheet, effects, …) are derived by a `styk_eval::load::Loader`
  that lives as long as a document's lineage. It keeps the view of every record
  whose tree node is unchanged (by node address), interns images by payload and
  resolves style overlays once per base figure; an edit that replaces one
  keyframe converts one keyframe.

## While the editor owns the document (S1–S4)

Commands run in JavaScript. The engine keeps a mirror
([D12](decisions/0012-document-mirror.md)): `src/engine/engine-mirror.mjs`
lifts the changed records (`liftDocument`: skeleton JSON plus number arrays and
payloads as binary sections), stages them per batch
(`EngineHandle.stage_reset/stage_properties/stage_layout/stage_record/stage_source`)
and commits by revision (`commit`, `commit_now`). `styk_engine::mirror` builds
the next tree, converts only changed records and publishes a new generation, or
changes nothing if any staged change is invalid. The cost: the document exists
twice (plan section 6 measures it) until the engine owns it (S5). The mirror leaves
out the audio (`_styk.audio`, whose recordings can reach 96 MiB): nothing the engine
draws reads it, and holding the recordings twice is not worth the audio commands
`styk-edit` has ported; it joins the engine's document when the engine saves (S5).

Only one commit runs at a time. `commit_now` runs on the interface thread, which
may not wait ([D7](decisions/0007-interface-thread-never-blocks.md)): while a
background commit of the frames holds the mirror it answers `{busy: true}` and
leaves the batch staged (`Engine::try_commit`), and `syncDefinitions` throws
`engine_busy` without forgetting what it sent; the next `commit_now` commits it.
The stage then draws that frame with the JavaScript renderer, as for any refusal;
a frame sync (`syncFrames`) and an export wait for the background commit
instead. (Before, the interface thread waited on the lock, which a browser
refuses: the batch was lost and the mirror silently out of step.)

## Canonical worker publication prerequisites (S5)

The checked worker boundary stages the same before/after COW roots that the editor's
prepared publication owns. `Engine::stage_edit_publication_worker` returns an opaque
`EditPublicationAdmission`; `commit_edit_publication_worker` validates that admission,
the original revision and roots, then exchanges the mirror generation and editor heads
in one guarded publication. The common record-transfer protocol retains displaced roots
and all related charges until terminal ACK. Cancellation and stale publication dispose
their retained owners on workers.

`styk_doc::retained_bytes_checked` counts each shared allocation once, including reserved
capacity and packed numeric arrays, with bounded cancellation checks. Root and ordinary
record admissions share the existing byte and slot budgets. Only a genuinely empty
default mirror has no canonical root to retire; an empty-keyframe document still has an
owned root. Cleanup targets the exact displaced generation, so an older publication
cannot cancel record packets or planning work installed for a newer generation.

These portable APIs support the pending canonical Session actor. The current JavaScript
editor and mirror remain in charge until the app handoff and class cost gates pass.

## Invariants

- No silent loss: a field the engine does not understand stays in the tree and
  is written back; a feature it does not evaluate is named in
  `Document::unsupported` and gated in the app (`engine-eligibility.mjs`).
- Never mutate a published node: build a new path (`Arc::make_mut` on a clone).
- Number identity: a value read and written unchanged is the same bytes.

## Known differences

- A lone UTF-16 surrogate in a string becomes U+FFFD (JavaScript keeps it).
- Integers between 2^53 and 2^64 are `f64` like in JavaScript; Python's integer
  spelling of them is not kept ([formats-piv.md](formats-piv.md)).

## Testing

`cargo test -p styk-doc -p styk-formats`; `npm run engine:parity` decodes every
native file of the repository with both readers and compares the documents
(`engine/tools/container-parity.mjs`).
