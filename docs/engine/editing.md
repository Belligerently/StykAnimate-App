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
- Order: what the app runs. The app constructs the editor with `propertyChannels` and
  `layerTimeline` (`src/app.mjs`), and `load` gives every document a layer sheet
  (`initializeLayerSheet`). Every gesture in the app therefore takes the authored path
  (`src/editor-authored-pose.mjs`): a draft of the evaluated frame at the playhead, the same
  per-figure mutations as on a native frame, and a commit that captures the edited frame
  into pose sources and writes keys on the layer tracks (`_commitSheetPose`, the sheet
  planner and `publish` in `src/editor-layer-sheet.mjs`). The native-timeline commands
  ported first are those mutations, checked on native frames; the app needs, in order:
  1. **The evaluated frame as records**: `editor.evaluatedFrame` reproduced as JSON byte for
     byte (layer sheets, tweens, movieclips, layer order), because a commit stores it.
     Done: `styk_eval::records` ([evaluation.md](evaluation.md)).
  2. **The authored lifecycle and the sheet planner**: drafts, ownership, key writing,
     pose-source capture and the history entry `publish` records. Done for gestures on
     existing figures (see the status below).
  3. **The facade**: the app's gestures run in the engine.
  Then frames and keys, layers and the layer sheet, figures and the builder, movieclips,
  effects and clips, camera, drawings' metadata, clipboard, sessions.

## Status of phase 1

`engine/crates/styk-edit` holds an `Editor` over the report as the mirror receives it, with
the playhead, the selection, the gesture in progress and a history of its own (for the
harness; the app's history stays in JavaScript in this phase). `styk edit-replay
<reference.json>…` and `cargo test -p styk-edit` replay every recording in
`engine/tests/edit/`.

| Call | Ported | Recorded in |
| --- | --- | --- |
| `select(uid, joint, {toggle, add})`, `selectMany(uids)` | yes | selection-native, every sheet scenario |
| `seek(tick)` (its return value, the evaluated frame, is read by the facade from its own copy) | yes | selection-native, move-joined, gestures-sheet |
| `beginDrag(label, {scope})`, `beginTranslationDrag`, `commitDrag`, `cancelDrag` | yes | move-native, drag-cancel, gestures-sheet |
| `moveSelection(dx, dy, {fromStart, detachExternal})`, joined roots refreshed from the engine's geometry | yes | move-native, move-joined, drag-cancel, move-sheet |
| `rotateSelection`, `scaleSelection`, `scaleSelected`, `mirrorSelection` (whole figures), `rotateFigure`, `centerFigure`, `resetPose`: pivots from the engine's geometry | yes | rotate-native, transform-native, transform-joined, gestures-sheet |
| `poseJoint`, `rotateJoint`, `bendJoint` (with `stretch`, `isolate`, snapping back to the definition), rotations carried into joined figures (`rotationEdits`) | yes, without rigs, inverse kinematics, 3D bones or Stick Nodes curves | pose-native, pose-joints, pose-joined, pose-snap, gestures-sheet |
| `undo`, `redo` of what those record | yes | all of the above |
| flipping a branch, everything else | no: `Unported` | transform-native |

Every ported call runs in both of the editor's modes:

- **On a layer sheet** (the app: every document): a gesture drafts the frame on show
  (`styk_eval::records`), changes the draft, and commits through the sheet planner: the
  layers whose figures changed get a key at the playhead (at the key of a held cell,
  unless auto key is on) holding a new pose source captured from the draft; imported
  keys whose span changed are frozen as poses first; one `layer-sheet` history entry
  records the sheet, the layers and the sources, and undo, redo and the collection of
  sources from a dropped redo branch follow the editor. A movieclip instance, which a
  key's frame replacement places, is edited in that replacement (re-captured in place);
  editing other figures of the layer bakes the replacement into the new pose. After a
  commit the draft stays on show until a seek, an edit or history invalidates it, as in
  the editor. Recorded in move-sheet, gestures-sheet, autokey-sheet, layers-sheet (two
  layers, a join across them, a held key, a multi-layer selection), locked-sheet
  (refusals and their cancel events), movieclip-sheet and movieclip-loose-sheet.
- **On a native timeline** (tools and tests): the keyframe at the playhead is edited in place.

A call answers `Unported`, and changes nothing, on documents with property channels on a
native timeline, animation origins, a movieclip source or inverse-kinematics
controllers; for joint gestures on rigged or Stick Nodes figures; for poses on a drawing
layer; for a replaced figure joined to one outside its movieclip frame; and wherever the
engine does not evaluate the frame as records yet ([evaluation.md](evaluation.md)).

What the facade must still account for before it calls the engine (none of it is a
document difference the harness can see):

- **Counters and settings the document does not carry**: the layer revision named by
  events, the number of the next pose source, auto key, and the layer hints of the last
  published poses. The engine keeps them across its own calls; the facade hands them over
  when it starts from the editor's state (`set_counters`, `Options::auto_key`).
- **Layer focus locks and the timeline's layer selection** (`_sheetSelection`) are
  runtime state of the JavaScript editor; the facade runs selection and seeking in
  JavaScript while either is on.
- **`runtime_segments`** are not in the engine's copy (the mirror strips them). The
  editor's history snapshots keep them, so on a native timeline a gesture that moves a
  figure with `runtime_segments` and puts it back is a change in JavaScript and none in
  the engine. On a layer sheet a commit compares poses without them, as the engine does.
- **The history byte budget** (`historyBytes`) is not ported; the engine's history trims
  by count (80 entries) only. Phase 1 keeps the app's history in JavaScript.
- **Validation** (`validateObjectChanges`, `validateLayerState`,
  `validateLayerSheetChanges`) is not ported; the ported commands only write finite poses
  and keys the planner builds.

## What a gesture costs in the engine

`styk edit-bench <report.json> <uid> <tick>` times a gesture in `styk-edit` on a loaded
report. Native build, one run each while the machine was busy with other work
(2026-10-03), on Pivot imports of about 100 MB of JSON:

| | hitoshi vs suzu tomiee ssj (256 keys, 75 figures on show) | AyameDemo (617 keys, 2 figures on show) |
| --- | --- | --- |
| First call on a document (typed views of all of it) | 102 ms | 68 ms |
| Begin a gesture (evaluate the frame as records) | 2.5–6.2 ms | 2.6–6.0 ms |
| A move step | 0.01 ms | under 0.01 ms |
| A joint-pose step (geometry of the draft) | 0.56 ms | 0.03 ms |
| Commit (planner, capture, history) | 1.8–2.7 ms | 7.3–7.5 ms |
| Undo | 6.1 ms | 5.2 ms |

Not measured: WebAssembly, and the boundary (arguments in, changed records out).

## Arithmetic

A ported command must round every stored result as the editor does. The platform's
maths library does not: against V8 on two million inputs, Rust's `f64::sin` differed in
the last bit 2.7 % of the time and `f64::atan2` 17 %; even the `libm` crate (musl)
differed for 0.8 % of sines and 0.7 % of cosines, in fdlibm's cosine kernel, and for `atan2` beyond
|y/x| = 2^60. `styk-math` ports V8's fdlibm 5.3 `sin`, `cos` and `atan2` and its
`hypot`; on 2.8 million inputs (random angles, tiny and huge values up to 1e308, every
n·π/2 up to 64 and its neighbours, special values) it returned V8's bits every time.
`engine/tools/math-fixtures.mjs` records the regression set its tests use.

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
