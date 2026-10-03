# Engine: editing (stage S5, phase 1)

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
- In the app a ported command first runs in **shadow** (below): the JavaScript editor
  stays in charge and the engine's answer is compared, on real documents and gestures.
  Ownership of a document moves to the engine in one step once its commands are ported
  ([D15](decisions/0015-commands-record-for-record.md), revised): adopting records command
  by command would need two copies of the editor's runtime state kept equal by hand.
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
| `poseJoint`, `rotateJoint`, `bendJoint` (with `stretch`, `isolate`, snapping back to the definition), rotations carried into joined figures (`rotationEdits`), Stick Nodes curves (a straight segment with a sagitta, which geometry reports with a bend of 1e-6) | yes, without 3D bones | pose-native, pose-joints, pose-joined, pose-snap, gestures-sheet, sticknodes-sheet |
| Rig controls: a joint in a rig space takes the pointer through `authoredRigTarget` (from `styk_eval::ik::rig`; the drawn shape and rig space from geometry's `RigJoint`), an angle lock only stretches along its chord and does not turn, a connector slides (`slideConnector`, `setConnectorPosition`) and bends through the pointer (`_bendConnector`); skins, angle paths and weighted joints are not posed by hand | yes, without 3D bones and IK posing on rigged figures | rig-sheet, ik-rig-follow-sheet, ik-rig-stretch-sheet |
| `poseJoint` on the end of a limb with an IK controller (`poseIKTarget`): a pinned target moves; an unpinned limb is captured as drawn, solved to the pointer and baked, its followers and bend kept in the controller; across joined figures on the assembly's skeleton (`poseGraph`, `captureFor`) | yes, without perspective or 3D controllers | startup-sheet, ik-unpinned-sheet, pinned-ik-sheet, ik-joined-unpinned-sheet, startup-join-sheet |
| Gestures around pinned limbs: the solution baked before a pose edit (`_objectEdit(…, {posed})`, `_bakeIKFollowers`), follower offsets turned with their joint (`_rotateIKFollowers`), the bend remembered after every change (`_rememberIKBends`), a stretched end followed by its pin (`_followStretchedIK`); a gesture on joined figures takes in their whole assembly (`_ikPoseSlots`, `_expandIKDrag`) | yes | pinned-ik-sheet, ik-followers-sheet, ik-chains-sheet, ik-joined-sheet, ik-joined-late-sheet |
| `sheetInsertKey`, `sheetDuplicateFrameAfter`, `sheetSetEasing`, `sheetClearKeys`, `sheetClearFrames`, `sheetInsertFrames`, `sheetRemoveFrames`, `sheetSetDuration`, `sheetAppendFrame`: the layer timeline's keys and frames, clips on the document's clock retimed | yes, without drawing layers and audio ripple | keys-sheet, frames-sheet, frames-movieclip-sheet, frames-timed-clip-sheet |
| `joinTo`, `unjoinFigure`, `joinFigure`, `deleteSelection`, `deleteFigure`, `duplicateFigure`: the editor's `transaction`, a draft of the whole frame in which figures come and go | yes, on a layer sheet; not for movieclip instances or several figures at once | figures-sheet, figures-layers-sheet |
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
native timeline, animation origins or a movieclip source; for documents with 3D poses,
meshes or surfaces, 3D bones (posing, turning or twisting one) and posing an IK target on
a rigged figure; for poses on a drawing layer; for a replaced figure joined to one
outside its movieclip frame; and wherever the engine does not evaluate the frame as
records yet ([evaluation.md](evaluation.md)).

Inverse-kinematics controllers (`_stykIK`) are gated per call on the frame on show
(`styk-edit/src/ik.rs`). Controller targets move with their figure (`translateIKTargets`:
`moveSelection` with `movePins`, a duplicate's targets offset with it), and selection,
seeking and history run anywhere. Dragging the end of a limb moves a pinned target, or
solves an unpinned limb with `styk_eval::ik::solve_object` and bakes it (`poseIKTarget`).
Around pinned limbs, gestures read the solution geometry draws (`ik::solve_local`): a
pose edit bakes it first, turning a follower changes its offset, and every change
remembers the bend the limb is now solved with. Joined figures with IK are compiled into
one temporary skeleton (`styk_eval::ik::joined::Assembly`, which geometry uses too): a
gesture takes in the whole assembly, and followers in other figures are kept by their
identity (`joinedFollowers`). All of it matches the editor bit for bit. A call answers
`Unported` on a frame with a 3D controller and when posing a target of a figure in
perspective. Recorded in startup-sheet and startup-join-sheet (the document the app
starts with, alone and with a copy joined to a hand), ik-unpinned-sheet (a foot whose
branches follow the target, a three-bone chain that stretches, drags, a cancel, undo),
pinned-ik-sheet, ik-followers-sheet and ik-chains-sheet (pinned two-bone limbs, a foot
with followers, four-bone chains on Both and on one side), ik-joined-sheet and
ik-joined-unpinned-sheet (a shin's controller driving another figure's thigh, pinned and
free) and ik-joined-late-sheet (a foot joined after the controller was set).

What the facade must still account for before it calls the engine (none of it is a
document difference the harness can see):

- **Counters and settings the document does not carry**: the layer revision named by
  events, the number of the next pose source, the next figure identity, auto key, and
  the layer hints of the last published poses. The engine keeps them across its own
  calls; the facade hands them over when it starts from the editor's state
  (`set_counters`, `Options::auto_key`).
- **How many poses hold each figure** (`_layerUidCounts`, `_layerSeenUids`) decides
  whether a command adds one more layer revision (a figure appears or disappears
  altogether). The engine counts them from the document when first needed; the editor's
  set of figures *ever seen* can be larger after figures were deleted and collected.
- **Layer focus locks and the timeline's layer selection** (`_sheetSelection`) are
  runtime state of the JavaScript editor; the facade runs selection and seeking in
  JavaScript while either is on.
- **`runtime_segments`** are not in the engine's copy (the mirror strips them). The
  editor's history snapshots keep them, so on a native timeline a gesture that moves a
  figure with `runtime_segments` and puts it back is a change in JavaScript and none in
  the engine. On a layer sheet a commit compares poses without them, as the engine does.
- **The history byte budget** (`historyBytes`) is not ported; the engine's history trims
  by count (80 entries) only. Phase 1 keeps the app's history in JavaScript.
- **Validation**: a published sheet is checked as `validateLayerSheetChanges` checks it
  (keys in order inside their layer's length, valid easing, poses that exist, the key
  counter), with the editor's errors; movieclip replacement records, the million-key
  limits, `validateObjectChanges` and `validateLayerState` are not ported.
- **Audio ripple** (`editor.audioRipple`): inserting or removing frames on every track
  also shifts audio in the editor; the facade runs those in JavaScript while it is on.

## Shadow editing in the app

With `?engineShadow=on` (or `localStorage['stykanimate.engine.shadowEdit'] = 'on'`) every
ported call the app makes also runs in the engine: `EngineHandle.edit_session` starts
`styk_edit::session::Session` on the mirrored document with the editor's view and
counters, `EditSession.call` answers each call as JSON (result, state, events, the
gesture's draft, and the sheet, layers and pose sources a command published), and
`src/engine/engine-edit-shadow.mjs` compares that with what the editor did. The editor
stays in charge; nothing from the engine is applied. A session lives while every change
to the document goes through calls it shadows; anything else ends it, and the next one
starts when the mirror holds the editor's revision again. Results accumulate in
`globalThis.__stykEditShadow`.

`node scripts/engine-edit-shadow.mjs [--file <document.json>]` drives the real app in
headless Chromium: pointer drags on joint handles and on the origin, keyboard undo and
redo, transforms, timeline keys and frames, duplicate/join/unjoin/delete. Measured
2026-10-03 (WebAssembly, SwiftShader), one run each:

| Document | Calls run in the engine | The same as the editor | Different | Editor time | Engine time |
| --- | ---: | ---: | ---: | ---: | ---: |
| joined-chain (imported, joined figures) | 77 | 77 | 0 | 57.7 ms | 28.2 ms |
| sheet-layers (two layers, held key) | 77 | 77 | 0 | 59.9 ms | 30.2 ms |
| sheet-movieclip-loose (a movieclip) | 77 | 77 | 0 | 62.6 ms | 30.6 ms |
| sheet-mixed-keys (imported and authored keys) | 77 | 77 | 0 | 57.9 ms | 27.8 ms |
| the document the app starts with (unpinned IK on every limb) | 77 | 77 | 0 | 174.0 ms | 81.7 ms |
| startup-pinned (every limb pinned) | 88 | 88 | 0 | 167.1 ms | 100.0 ms |
| followers (a pinned foot with followers and a polygon) | 88 | 88 | 0 | 181.5 ms | 93.3 ms |
| joined (IK across four joined figures) | 54 | 54 | 0 | 71.4 ms | 43.9 ms |

"The same" means equal keys, order, strings and integers, and numbers within 1e-9
relative: 28 to 53 of the 77 calls differ from Chromium's editor in the last digits of a
pose, because Chromium's `Math` is not Node's ([D16](decisions/0016-v8-exact-math.md)).
The engine time includes the JSON boundary. The IK documents were measured 2026-10-03
after IK editing was ported, joined figures included: every call the script makes runs in
the engine (on joined, two undos reach history from before the session, which ends it and
starts the next). Their times include a duplicate, join and unjoin that each re-evaluate
the frame with IK.

## Engine-owned editing in the app

With `?engineEdit=on` (or `localStorage['stykanimate.engine.ownEdit'] = 'on'`) the engine
runs ported calls in the editor's place while a session lives (the same
`src/engine/engine-edit-shadow.mjs`, mode `own`). The calls that change the document
(the gestures' moves, poses and transforms, `commitDrag`, the layer timeline's keys and
frames, joining, duplicating and deleting figures) run in the engine only; the editor
adopts what they produced and does not compute it:

- a gesture's draft: the engine's figures replace the editor's in its draft, and the
  editor's snapshots take in the figures the engine's gesture took in (`drag` in the
  answer: their identities and whether the gesture moves whole connected figures);
- what a call published: `adoptLayerSheet` (src/editor-layer-sheet.mjs) runs the editor's
  own `publish` on a plan made of the engine's sheet, layers and new pose sources, so
  validation, layer bookkeeping, events and the history entry are the editor's. The answer
  carries only the tracks the call replaced (`trackOrder` names them all); the others stay
  the editor's objects, shared with history as before;
- a committed gesture's draft stays on show as in `_commitSheetPose`
  (`_authoredCommitPreview`): the answer carries its figures (`retained`), as the commit
  leaves them (joined figures' roots refreshed from geometry), in place of the editor's
  draft or, for a gesture the call began and ended itself, of the frame it showed;
- the selection, the next figure identity, the live-edit events and a seek the call made.

Selecting, seeking, starting and cancelling a gesture, and undo and redo run in both and are
compared as in shadow mode (history stays the editor's). A call the engine refuses, or whose
answer the editor cannot adopt, runs in the editor alone and ends the session; an error runs
the editor's call so that the editor raises it with its message. In shadow mode the answer's
unchanged tracks are checked to be the editor's unchanged objects.

`node scripts/engine-edit-shadow.mjs --own [--file <document.json> | --import <file.piv>]`
runs the same interactions twice, with the editor alone and with the engine owning, and
compares the documents, the history labels and the selection after them and after undoing
everything. Numbers may differ in their last digits ([D16](decisions/0016-v8-exact-math.md)):
the engine computes with Node's `Math`, the browser's editor with Chromium's. Measured
2026-10-03, headless Chromium:

| Document | Calls adopted from the engine | Same as the editor alone | Numbers differing in last digits |
| --- | ---: | --- | ---: |
| the document the app starts with | 54 | yes (32,872 bytes, 13 history entries) | 31 |
| startup-pinned | 65 | yes | 43 |
| followers | 65 | yes | 76 |
| joined (four joined figures with IK) | 39 | yes | 38 |
| joined-chain | 54 | yes | 50 |
| sheet-layers | 54 | yes | 80 |
| sheet-movieclip-loose | 54 | yes | 28 |
| AyameDemo.piv, imported (48.7 MB document) | 55 | yes (10 history entries) | 996 |

Two adoption defects were found this way and fixed: on AyameDemo the committed draft the
editor keeps on show was dropped, so the next gesture began from a fresh evaluation
(another figure slot and keyframe index); on joined-chain the kept draft lacked the joined
roots the commit refreshes, so the next gesture began with a joined figure 13 px away.

What the same calls cost on AyameDemo.piv (one run each; the editor's time from a shadow
run, the engine's from an owned run, adoption included). The first measurement found
`poseJoint` at 200.6 ms; three causes were fixed: a rotation's targets were found with
list membership tests (quadratic in the figure's 515 segments), each angle was changed by
unpacking and repacking the whole angle array, and every evaluation scanned the document
for 3D. Sessions now also start from the mirror's prepared document instead of converting
it again.

| Call | Calls | Editor | Engine | Adoption |
| --- | ---: | ---: | ---: | ---: |
| `poseJoint` (pointer drags) | 36 | 25.3 ms | 87.8 ms | 17.1 ms |
| `moveSelection` | 3 | 10.2 ms | 18.0 ms | 9.7 ms |
| `sheetInsertFrames` | 1 | 393.3 ms | 362.0 ms | 74.5 ms |
| `duplicateFigure`, `joinTo`, `unjoinFigure`, `deleteSelection` | 4 | 365.9 ms | 100.2 ms | 41.4 ms |
| keys: `sheetInsertKey`, `sheetDuplicateFrameAfter`, `sheetSetEasing`, `sheetRemoveFrames` | 4 | 61.1 ms | 121.2 ms | 28.9 ms |

All owned calls together: 953 ms against the editor's 898 ms. The frame-drafting commands
are three to six times faster in the engine; joint poses are still about three and a half
times slower (2.4 ms a call in WebAssembly plus 0.5 ms adoption, against 0.7 ms), because
each gesture call evaluates the whole frame's geometry where the editor rebuilds what
changed, and the answer carries the whole figure (515 segments here). Engine-owned editing
therefore stays opt-in; making it the default for a document class waits for gestures
that evaluate only the figures they need.

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

The same tool on AyameDemo as the app holds it after import (its layer sheet, 105 figures
on show, uid 9043 with 515 segments at tick 734), after the fixes above: a joint-pose step
median 0.84 ms (8.42 ms before), a move step 0.02 ms, commits 5.0–9.0 ms, undo 2.1 ms.

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
