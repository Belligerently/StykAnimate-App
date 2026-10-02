# Engine: editing (stage S5, phase 1 started)

How editing moves from the JavaScript `Editor` (`src/editor.mjs` and about 80
`src/editor-*.mjs` modules, roughly 800 methods) into the engine without
changing the interface or breaking the app at any commit. The plan is
[`../RUST-ENGINE-PORT.md`](../RUST-ENGINE-PORT.md); the decision is D15.

## Where it starts

The editor owns the document as JavaScript objects and runs every command; the
engine mirrors it ([document.md](document.md), [D12](decisions/0012-document-mirror.md)).
Commands mutate records copy-on-write, record history entries (records before
and after), and emit change events the interface listens to.

## Phase 1: commands one at a time, record for record

- A command is ported when `styk-edit` reproduces it **record for record**:
  given the same document, selection, tick and arguments, it changes the same
  records into the same bytes, leaves the same selection and returns the same
  event detail (`kind`, `frameIndices`, `figureIndices`, `sourceIds`,
  `structural`, `definition`, labels).
- The `Editor` facade keeps its API. A ported method asks the engine to run the
  command on the engine's document; the engine answers with the changed records
  (lifted, as the mirror sends them the other way), which the facade puts into
  its JavaScript document in place of the old records, and with the event
  detail, which it emits as before. History stays in JavaScript in this phase:
  the facade records the before/after records exactly as the JavaScript command
  did. An unported command runs in JavaScript as today and reaches the engine
  through the mirror. Both copies stay identical after every command.
- Every port is checked by the **command-replay harness**: a scenario (a
  document, editor options, a list of `[method, ...arguments]` steps including
  undo and redo) runs in the JavaScript editor, which records the document
  bytes, selection and events after every step; `styk edit-replay` runs the same
  steps in `styk-edit` and compares. Scenarios come from the existing editor
  tests, from fixtures built with `engine/tools/fixture-kit.mjs`, and from
  recorded sessions.
- Order: the gestures first (pose a joint, move, rotate, scale, IK targets: the
  edit-to-frame path), then frames and keys, layers and the layer sheet,
  figures and the builder, movieclips, effects and clips, camera, drawings'
  metadata, clipboard, sessions.

## Status of phase 1

`engine/crates/styk-edit` holds an `Editor` over the report as the mirror receives it, with
the playhead, the selection, the gesture in progress and a history of its own (for the
harness; the app's history stays in JavaScript in this phase). `styk edit-replay
<reference.json>…` and `cargo test -p styk-edit` replay every recording in
`engine/tests/edit/`.

| Call | Ported | Recorded in |
| --- | --- | --- |
| `select(uid, joint, {toggle, add})`, `selectMany(uids)` | yes | selection-native |
| `seek(tick)` (its return value, the evaluated frame, is read by the facade from its own copy) | yes | selection-native, move-joined |
| `beginDrag(label, {scope})`, `beginTranslationDrag`, `commitDrag`, `cancelDrag` | yes | move-native, drag-cancel |
| `moveSelection(dx, dy, {fromStart, detachExternal})`, joined roots refreshed from the engine's geometry | yes | move-native, move-joined, drag-cancel |
| `undo`, `redo` of what those record | yes | all of the above |
| `rotateSelection`, `poseJoint`, everything else | no: `Unported` | rotate-native, pose-native |

Every call answers `Unported` on documents with a layer sheet, property channels, animation
origins, a movieclip source or inverse-kinematics controllers; those run in JavaScript.

What the facade must still account for before it calls the engine (none of it is a
document difference the harness can see):

- **Layer focus locks** are runtime state of the JavaScript editor, not of the document;
  the facade runs selection in JavaScript while a focus lock is on.
- **`runtime_segments`** are not in the engine's copy (the mirror strips them). The
  editor's history snapshots keep them, so a gesture that moves a figure with
  `runtime_segments` and puts it back is a change in JavaScript and none in the engine.
  Phase 1 keeps the app's history in JavaScript, so the facade's decision stands.
- **The history byte budget** (`historyBytes`) is not ported; the engine's history trims
  by count (80 entries) only.
- **`validateObjectChanges`** on commit is not ported; moves only change finite roots.

## Phase 2: queries from the engine

Handle positions, picking, snapping, bounds, motion paths and inspector values
come from the engine (it already builds the edit frame's geometry,
`styk_engine::edit::EditScene`), so the interface no longer needs JavaScript
geometry. Then the JavaScript evaluation, geometry, IK, rig, movieclip and
effect-model modules have no callers and are deleted.

## Phase 3: the engine owns the document

History moves to the engine (a generation per entry, sharing unchanged records),
the JavaScript copy of the document becomes a read view the interface queries
for what it displays (timeline rows, layer lists, inspector), and the mirror
disappears. Opening and saving go through `styk-formats` and `styk-piv`
directly.

## Invariants

- No commit leaves a command half-ported: a facade method calls the engine only
  when its replay scenarios pass.
- The JavaScript document and the engine's are identical after every command
  in phases 1 and 2 (checked in development builds by comparing the records a
  command touched).
- Interactive commands never send the whole document; a gesture's updates are
  the records it changes.
