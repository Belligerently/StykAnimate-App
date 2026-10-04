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
| Every ported call on documents with property channels as the app holds them (a layer sheet's `legacyAnimation`): the frames a call drafts, captures and materializes are composed as the editor composes them (`styk_eval::records`) | yes | channels-figure-sheet, channels-pose-sheet, channels-vfx-sheet, channels-camera-scene-sheet, channels-movieclip-sheet, channels-style-sheet, channels-curves-sheet, channels-outline-sheet, channels-follow-sheet |
| Every ported call on documents with 3D bones, meshes, surfaces, perspective and part proportions | yes | spatial-mesh-perspective-sheet, spatial-mesh-sheet-sheet, spatial-surfaces-skin-sheet, spatial-surfaces-perspective-sheet, spatial-mesh-mechanism-sheet, spatial-clip-mesh-sheet |
| 3D bones (`styk-edit/src/spatial.rs`): turned (`setSpatialPose`, `rotateJoint` without `isolate`, from the angles the bone is drawn with), dragged flat on screen or swung in depth with the depth key, the alternate side and a mid-drag switch (`poseSpatialJoint`, `_flatSpatialJoint`, `_arcSpatialJoint`), stretched, twisted (`bendJoint` → `twistSpatialJoint`); mechanisms' hinges and slides set (`setSpatialMotion`, a hinge through `setSpatialPose`) and dragged along their track (`_moveMechanicalJoint`), a driven hinge included. Results go through `quaternionPose` with `styk-math`'s `asin` | yes | spatial-turn-sheet, spatial-mechanism-sheet, spatial-drag-sheet, spatial-mechanism-drag-sheet |
| Pose choices (`styk-edit/src/pose_choice.rs`, `setPoseChoice`): a group of 3D bones shown one at a time or all hidden (`_stykPoseChoice`), a group of presets over existing angle and length channels (`figure._styk.poseChoices`, `applyChannelPoseChoice`); what is selected read back from the channels as `poseChoiceGroups` does | yes | pose-choices-sheet, pose-presets-sheet, pose-choice-mechanism-sheet |
| 3D IK controllers (`spatialTarget`): every call on their frames, with posing a bone of a planted limb releasing it (`_releaseSpatialChain`), a posed hand or foot planting its new orientation (`_plantSpatialFollower`) and turned followers' 3D orientations (`spatialFollowers`, the editor's quirk of keeping only the last of several turned included) | yes | ik-spatial-sheet, ik-spatial-side-sheet, ik-spatial-mirrored-sheet |
| Steering planted 3D limbs: dragging a knee or elbow swings the limb's bend plane (`getSpatialIKPole`, `setSpatialIKPole`); twisting a bone of the limb swings its swivel and twisting the ankle or wrist marker turns the foot or hand (`twistSpatialJoint`); a whole figure turned with its 3D targets, bend planes and planted orientations (`setSpatialPose` with `wholeFigure`); `resetSpatialPose` for a bone, a mechanism or the whole rig, releasing its targets | yes | ik-spatial-steer-sheet, ik-spatial-side-steer-sheet, ik-spatial-mirrored-steer-sheet, spatial-reset-sheet |
| Posing 3D IK targets (`styk-edit/src/spatial_ik.rs`): a planted target moves with its knee kept on its side on screen (`_flatSpatialIKPole`), the depth key pushes it in depth and a plain drag continues from there (`poseSpatialIKDepth`, `_spatialIKPointer`), `setSpatialIKTarget` sets depth, pole and swivel; a free 3D limb is captured once per gesture with its 3D target and followers' orientations (`captureSpatialIK`, `_drag.ikCapture`) and solved without following the target's orientation; IK posing on rigged figures, the drawn angles read from the rig (`RigJoint`) | yes, not across joined figures | ik-spatial-target-sheet, ik-spatial-side-target-sheet, ik-spatial-free-sheet |
| Rig controls: a joint in a rig space takes the pointer through `authoredRigTarget` (from `styk_eval::ik::rig`; the drawn shape and rig space from geometry's `RigJoint`), an angle lock only stretches along its chord and does not turn, a connector slides (`slideConnector`, `setConnectorPosition`) and bends through the pointer (`_bendConnector`); skins, angle paths and weighted joints are not posed by hand | yes, without 3D bones and IK posing on rigged figures | rig-sheet, ik-rig-follow-sheet, ik-rig-stretch-sheet |
| IK settings (`styk-edit/src/ik_settings.rs`): `setNodeIK` switches a controller on (pinned where the limb is drawn, refused on a joint a rig derives and on chains that would share bones) or off (the limb kept as drawn, a 3D limb's record kept), pins and releases it (released lengths kept on the chain), sets stretch, chain length, bend side ("Both" recapturing the bend) and a 3D limb's swivel; `rotateIKTarget` turns a target (a free limb captured first, its branches turning with it); `setSpatialFeetPinned` plants or releases every foot of a 3D figure in one gesture | yes, not across joined figures | ik-settings-sheet, ik-settings-pinned-sheet, ik-settings-spatial-sheet, ik-settings-rig-sheet, ik-settings-followers-sheet |
| `poseJoint` on the end of a limb with an IK controller (`poseIKTarget`): a pinned target moves; an unpinned limb is captured as drawn, solved to the pointer and baked, its followers and bend kept in the controller; across joined figures on the assembly's skeleton (`poseGraph`, `captureFor`) | yes, without perspective or 3D controllers | startup-sheet, ik-unpinned-sheet, pinned-ik-sheet, ik-joined-unpinned-sheet, startup-join-sheet |
| Gestures around pinned limbs: the solution baked before a pose edit (`_objectEdit(…, {posed})`, `_bakeIKFollowers`), follower offsets turned with their joint (`_rotateIKFollowers`), the bend remembered after every change (`_rememberIKBends`), a stretched end followed by its pin (`_followStretchedIK`); a gesture on joined figures takes in their whole assembly (`_ikPoseSlots`, `_expandIKDrag`) | yes | pinned-ik-sheet, ik-followers-sheet, ik-chains-sheet, ik-joined-sheet, ik-joined-late-sheet |
| The project palette (`styk-edit/src/palette.rs`): `addPaletteColor`, `addPaletteColors`, `removePaletteColor`, `movePaletteColor` (colours as `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa` or bytes), one `palette` history entry each; compared in both editors | yes | palette-sheet |
| Audio tracks and clips (`styk-edit/src/audio.rs`): `setAudioTrack` (name, volume, mute, solo), `deleteAudioTrack`, `setAudioClip` (start, trim, duration, volume, fades kept within the clip, another track; the clip edited goes last on its track, as in the editor), `deleteAudioClip`, recordings nothing uses dropped; validated as `validateAudioState` does, each recording checked once (the editor's `WeakSet`, here by identity without keeping it alive); one `audio` history entry each. Not run in the app yet: the engine's document leaves the audio out ([document.md](document.md)), so these stay with the editor until the engine saves. Adding tracks, importing, splitting, duplicating and pasting clips draw random identities and ripples with frame edits are batches: they stay in the editor | yes | audio-sheet, audio-none-sheet |
| Camera motion clips (`styk-edit/src/camera_motion.rs`): `setCameraMotion` (start, duration, switch, parameters, followed figures; muted ranges move with the clip) and `deleteCameraMotion`, validated as `validateCameraMotion` does, one `camera-motion` history entry each, the sheet lengthened to the last clip; compared in both editors, not adopted. Adding, duplicating, pasting and randomizing clips draw random identities or seeds and stay in the editor | yes | camera-motion-sheet |
| `updateSegment(patch)`: the selected joint's angle, length and bend (one posed edit; a connector's length within 0 to 1) and its definition's colour, transparency, thickness, shape and construction flag with the raw flags following (one `figure` history entry); compared in both editors, not adopted (its definition event is not a sheet publication) | yes | update-segment-sheet, update-segment-rig-sheet |
| `updateObject(patch)`, `updateSelection(patch)`: the inspector's figure fields (scale, position, opacity, name, colour, mirroring, metadata) with the editor's refusals; with several figures selected, shared fields edit them all | yes | update-object-sheet, update-object-native |
| `addFigure(typeId)` on a layer sheet: a new figure of a library definition in the middle of the canvas at the playhead, selected, in the active layer, through the range commit (`applyFigureRange`, `insert`, `_commitSheetRange`: only the inserted figure's layer is keyed, unlike a whole-frame transaction, which also stores joined figures' roots) | yes, not for movieclips or on a native timeline | add-figure-sheet, add-figure-layers-sheet |
| The layer list (`styk-edit/src/layers.rs`): `renameLayer`, `setLayerColor`, `setLayerVisible`, `setLayerLocked` (the scene and camera tracks too), `reorderLayers`, `moveLayerTo`, `createLayer`, and folders (`createLayerFolder`, `renameLayerFolder`, `moveLayersToFolder`, `arrangeLayers`, `deleteLayerFolder`): one `layers` history entry each, as the paths of the layer state it changed (`_layerEdit`, including its quirk of recording a removal of what was absent), the sheet's tracks following the layers | yes; run in both editors and compared, not adopted (cheap, and the session then lives through them) | layer-list-sheet, layers-folders-sheet, layers-native |
| `sheetInsertKey`, `sheetDuplicateFrameAfter`, `sheetSetEasing`, `sheetClearKeys`, `sheetClearFrames`, `sheetInsertFrames`, `sheetRemoveFrames`, `sheetSetDuration`, `sheetAppendFrame`, `sheetMoveFrames` (keys moved or copied along their tracks, replacing the keys they land on): the layer timeline's keys and frames, clips on the document's clock retimed (and, when their keys move, their span and restarts with them: `shiftClipTime`) | yes, without drawing layers and audio ripple; copying a drawing layer's keys is not ported | keys-sheet, frames-sheet, frames-movieclip-sheet, frames-timed-clip-sheet, move-frames-sheet, move-frames-imported-sheet, move-frames-clip-sheet, move-frames-presence-clip-sheet |
| The timeline clipboard (`styk-edit/src/sheet_clipboard.rs`): `sheetCopyFrames` keeps each layer's poses over a range (at its start, its keys and the layer's end inside it, with their easing; a figure joined to one on another layer is kept where geometry puts it, unjoined, as `ownedFrame` does) and `sheetPasteFrames` writes them over or inserted before what follows, on the same layers or on others (the figures, joins and IK followers renamed together, the new identities assigned to the destination; a movieclip on the presence clock, or a continuation of one, that takes a new identity carries on from where it was when copied: `phases`, from the evaluator's presence clock); the copy's return value is the editor's clipboard, a view. A session pastes only what it copied itself: the facade asks the engine only when the editor's clipboard is the object the session's copy made (or both have none) | yes, without drawing layers, movieclip parts and clipboards from another document | sheet-clipboard-sheet, sheet-clipboard-empty-sheet, sheet-clipboard-ik-sheet, clipboard-presence-clip-sheet |
| Polygons (`styk-edit/src/polygons.rs`): `addPolygon` (through at least three distinct handles, a closing repeat dropped, behind the skeleton unless an order is given, the next identity kept in the definition), `updatePolygon` (handles, order, a surface removed), `removePolygon` (polygons without an identity take theirs from their place), refusing a cycle another polygon draws in either direction; one `figure` history entry each (`editDefinition`, native order arrays dropped); compared in both editors, not adopted. A colour edit of a figure on show is recoloured (below); a new surface is fitted to the figure's geometry: `Unported`; an opacity edit with property channels is keyed (property edits, below) | yes | polygons-sheet, polygons-native, polygons-native-file-sheet |
| Effect clips (`styk-edit/src/vfx_clips.rs`): `setVfxClip` (timing, name, switch, strength, fades, effects, keys), `trimVfxClipStart` (keys rebased when time before the source is revealed), `deleteVfxClip`, `moveVfxClipOrder` (among the clips of the same target), `reorderVfxGroupEffect`, `setVfxClipParameter(s)` (set, or keyed when asked, with auto key or where the parameter has keys), `toggleVfxClipKey`, `moveVfxClipKey(s)`, `deleteVfxClipKeys`, `setVfxClipEasing`, and the gizmo edit (`beginVfxClipEdit`, previews, one entry on commit; cancelled by cancel, undo and redo); one `vfx-clips` history entry each, the sheet lengthened to the last timed clip; validated as `validateVfxClips` does against the engine's effect registry (the clips and effects a command changes; the others were checked when they arrived, as the editor's set of checked clips assumes); compared in both editors, not adopted. Adding, pasting and duplicating clips and effects draw random identities and seeds; paint colours, connected scopes and sampling between keys: `Unported` | yes | vfx-clips-sheet |
| Batches (`movieclipBatch`, as the timeline runs several commands as one entry): `beginBatch`, `endBatch`, `abortBatch` (and `@batch` in recordings) record the entries made in between as one `movieclip-batch` entry with the revision before the batch, send the batch's own change event, collect pose sources after it and undo the entries of a failed batch; undo and redo replay the entries in order and restore the batch's selection. In the app the facade opens and closes the session's batch around the editor's, compares the closing event and state, starts no session inside a batch and ends the session if the batch fails or brings its own starting selection | yes | batch-sheet |
| The timeline's layer selection: `selectSheetLayer` (the layer active and, with its contents, its figures on show selected, the first not joined to another the main one), seeks following it, `keepSelectionOnSeek`, any other selection dropping it, history restoring it; `setActiveLayer` | yes | sheet-selection-sheet |
| The figure clipboard (`styk-edit/src/figure_clipboard.rs`): `copySelection` (snapshots of the selected figures, joined ones where geometry draws them, joins kept only inside the copy, their definitions, layers and drawing order), `cutSelection` (pasted back in place), `pasteSelection` at the playhead (`insertionPlan`: new identities, a copy of each definition in the default and unique modes or the definition itself as an instance, offsets growing with each paste, front depth, joins and IK followers renamed, IK targets moved, node effects moved to the new definition; `insert`; `_commitSheetRange` keying the pasted figures' layers, with the new definitions added by the sheet's publication and removed by its undo), `duplicateSelection` (instances with new identities on their own layers, the clipboard kept). Movieclip instances: their definitions shared unless pasted as unique, a copy of one on show carrying on from where it was copied (`movieclipSamples`), an instance pasted where it is not on show keeping its identity (`linked`, `retained`), others playing on the presence clock from their start; movieclips alone are inserted as frame replacements of their own (`replaceAt`), and a pasted movieclip keys the playhead rather than a held cell. A session pastes only what it copied: the facade asks the engine only when the editor's clipboard is the object the session's copy made | yes, without drawings, figures with effect clips, branch selections and clipboards from another document or format version | figure-clipboard-sheet, figure-clipboard-ik-sheet, figure-clipboard-vfx-sheet, figure-clipboard-clip-sheet |
| Project settings (`styk-edit/src/project.rs`): `setFPS` (with the frame delay format 4 stores), `setCanvasSize`, `updateProject`, one `fields` history entry each (`_fields`, nothing recorded when unchanged); compared in both editors, not adopted | yes | project-settings-sheet, project-settings-format4-sheet |
| `importFigure(document, {position})`: a file's first frame (or a single definition) pasted as new figures with new definitions, centred on its unjoined roots at the position (the canvas's centre by default), joins named by slot in the file kept, the first figure selected; owned with the large figure class, adoption carrying the new definitions | yes, without movieclips, drawings and animation origins | import-figure-sheet |
| `setLayerParallax(id, patch)`: a figure layer's response to camera travel (linked axes following the one given, the reference the camera on show unless set), validated as `validateParallax`, one `layers` history entry | yes | layer-parallax-sheet, layer-parallax-camera-sheet |
| `renameMovieclip(name)` (`styk-edit/src/movieclips.rs`): the selected movieclip's definition renamed (trimmed, 1 to 120 characters, unchanged names not recorded), one `movieclip-definition` history entry (`publishDefinition`) | yes | movieclip-rename-sheet |
| `setMovieclipPlayback(patch)` and `setMovieclipDrawing(uid, group, view)` on the frame on show: an instance's mode, speed, offset, placement and restarts (or a drawing set's view, held), checked as `validatePlayback` checks them (presence-clock clips refuse placement), one object edit; across every frame (`{allFrames}`, `restartMovieclip`, `removeMovieclipRestart`), a clip on the presence clock takes the patch on every key of its layer, each written from the frame without its replacements, and in every replacement holding it (`sheetSetMovieclipPlayback`) | yes; across every frame only for clips on the presence clock | movieclip-playback-sheet, movieclip-playback-timed-sheet, clip-playback-all-sheet |
| `sheetTransferFrames(ids, destinations, start, end, tick, {copy})`: frames dragged to other rows, copied, cleared when moved and pasted in one batch, the clipboard left as it was; moved clips on the presence clock leave continuations on their source rows (`Retain moved movieclip phase`); compared in the app (its answer holds two publications, which the facade does not adopt) | yes | sheet-transfer-sheet, clipboard-presence-clip-sheet |
| Handle Edit Mode (`styk-edit/src/handle_edit.rs`): `beginHandleEditDrag({label, bend})`, `editHandlePosition(x, y, {bend})` (one edit outside a gesture), `commitHandleEditDrag`, `cancelHandleEditDrag`; the segments meeting the handle keep their arcs (`handleEditSegmentPose`, a bend edit keeping the tangent at the moved end), figures joined at the handle move with it, a gesture back at its start restores what it moved; angles of pinned limbs as solved, stretched lengths kept; rigged figures refused, movieclips `Unported` | yes | handle-edit-native, handle-edit-sheet, handle-edit-startup-sheet, handle-edit-ik-sheet, handle-edit-chain-sheet, handle-edit-sticknodes-sheet, handle-edit-full-turn-sheet |
| Whole figures by a handle (`styk-edit/src/figure_transform.rs`): `beginFigureHandleDrag(x, y, {rotate, scale, joint, label})`, `transformFigureHandle(x, y, {rotate, scale, ctrlKey, shiftKey})` (one edit outside a gesture), `commitFigureHandleDrag`, `cancelFigureHandleDrag`; the selected figures and those joined below them turn and scale about the dragged figure's origin, turns past a half turn accumulate, Ctrl keeps the scale and Shift the angle, back at the start the figures are restored | yes | figure-transform-native, figure-transform-sheet, figure-transform-ik-sheet |
| Handle commands (`styk-edit/src/handle_commands.rs`): `nudgeHandle(dx, dy, options)` (screen pixels through the view's zoom, rotation and mirroring; a root moves its figure, a joint poses, bends, stretches, moves in Handle Edit Mode or turns the whole figure), `alignHandle` (on an axis through its start or the figure's origin), `locateEditedHandle` (to where the handle is on the previous or next key), `segmentValues`, `beginSegmentValueEdit`, `setSegmentValues` (degrees, relative to the incoming tangent through joined origins, in Handle Edit Mode or by turning), `copySegmentValues` (text, compared as a view), `pasteSegmentValues`; a session pastes only the values it copied. Nudges go through the frame's camera; follow clips and camera motion active at the playhead: `Unported` | yes, without camera motion at the playhead | handle-commands-native, handle-commands-sheet, handle-commands-ik-sheet, handle-commands-camera-native, handle-commands-camera-sheet |
| Camera commands on a layer sheet (`styk-edit/src/camera_commands.rs`): `copyCamera`, `pasteCamera`, `setCameraValues`, `resetCamera`, `transformCamera`, `alignCameraToView`, `locateCamera` (each changed value a camera key through `addAnimationKeys`), camera drags (`beginCameraDrag`, `updateCameraDrag` previewing its keys on the frame on show, `commitCameraDrag`, `cancelCameraDrag`, and `commitDrag`/`cancelDrag` during one; other calls while one is open: `Unported`), `locateHandle` (the handle's figure moved by where the handle is on the neighbouring key); a session pastes only a camera it copied. `applyCameraToFigures(indices, {ticks})`: at each chosen frame whose camera is not the identity, every figure keyed where the camera shows it (root through the inverse camera matrix, scale, rotation) and the camera keyed back to the identity, through `_sheetPropertyEntries`. On a native timeline, and `centerCamera`: `Unported` | yes, on layer sheets | camera-commands-sheet, camera-bake-sheet |
| `joinTo`, `unjoinFigure`, `joinFigure`, `deleteSelection`, `deleteFigure`, `duplicateFigure`: the editor's `transaction`, a draft of the whole frame in which figures come and go; deleting with a movieclip instance selected leaves the cell as replacements by nothing (`sheetDeleteFigures`, `replaceAt`) | yes, on a layer sheet; not for several figures at once | figures-sheet, figures-layers-sheet, figure-clipboard-clip-sheet |
| Property edits on a layer sheet (`styk-edit/src/property_edit.rs`): `addAnimationKeys`, `addAnimationKey`, the sheet branches of `beginAnimationEdit`, `previewAnimationEdit`, `commitAnimationEdit` and `cancelAnimationEdit`, and the commands that reach `_sheetPropertyEntries` there: `setSceneColor`, `setSceneBackground`, `setBackgroundColor`, `updateFigureStyle` with property channels, `updatePolygon`'s opacity, `setVfxParameter`. Per tick, the frame there with the one-key channels the entries build composed (`styk_eval::records::Records::compose_channels`) is written as a key of each layer they touch, through the planner and `publish` (a held cell at its key unless auto key is on); a value left out is the one on show (`animationValue`, `Records::property_value`). A scrub's preview (`_sheetPropertyPreview`) is the frame on show until the edit ends; the session holds it the same way and its answer to `previewAnimationEdit` carries it, which the app compares and does not adopt. While an edit is open only its own calls run in the engine; undo and redo cancel it first. Owned with the class: what the commands publish is adopted, `beginAnimationEdit`, `previewAnimationEdit` and `cancelAnimationEdit` run in both and are compared (a key whose movieclip frames are replaced keeps its replacements, its pose written from the frame without them: `at(…, {skipMovieclipPatches})`, `Records::evaluated_options`) | yes, without fill styles and effect colours given as paints and native timelines | property-scrub-sheet, property-autokey-sheet, property-keys-sheet, property-locked-sheet, scene-color-sheet, figure-style-sheet, polygon-opacity-sheet, vfx-parameter-sheet, property-clip-patches-sheet |
| Figures changed across frames (`styk-edit/src/range.rs`, `range_plan.rs`: `applyFigureRange`, `planFigureFrameOperation`): `updateFiguresInFrames` (colour override, transparency, opacity; an override drops a colour transition), `orderFiguresInFrames`, `joinFiguresInFrames`, `detachFiguresInFrames`, `deleteFiguresInFrames`, `duplicateInFrames` (joins kept among the copies), `mirrorFiguresInFrames`, `scaleFiguresInFrames` (an absolute scale keeps the figures' proportions; a zero scale collapses), `rotateFiguresInFrames`, `centerFiguresInFrames`, `resetPoseInFrames`, `addFigureInFrames`, and `moveSelectionInFrames` (`sheetMoveSelection`). Every frame of the range (the timeline's chosen ticks as the app passes them, keyframe starts, or the playhead) is planned on its evaluated frame; figures joined below a changed one are re-joined at their parent's handle (`jointEndpoints`: single precision, `styk-math`); one `_commitSheetRange` keys every layer whose figures changed at each tick (a held cell at its key unless auto key is on), or on a native timeline one `figure-range` entry patches the keyframes | yes, without movieclips among the changed figures on a native timeline (on a layer sheet a deletion with a movieclip among them leaves replacements by nothing, and a duplicated movieclip plays on the presence clock from its start), duplicating figures with effect clips (random identities), aligning (the app measures bounds with a function of its own), pasting across frames, and adding, duplicating or deleting across frames of a native timeline | range-appearance-sheet, range-appearance-colors-sheet, range-joins-sheet, range-transforms-sheet, range-autokey-sheet, range-locked-sheet, range-native, range-ik-pinned-sheet, range-ik-joined-sheet, range-ik-followers-sheet, range-movieclip-sheet, range-clip-sheet |
| Figure order: `bringToFront`, `sendToBack`, `moveSelectionLayer`, `orderSelection` on whole figures and `orderFiguresInFrames`, as depths (`_stykZIndex`) within each layer: reordered ties separated in the direction of the move, a run pushed past ±1,000,000 shifted back or numbered again | yes, with property channels or depths on show (always in the app); a partial selection of one figure orders its segments (a builder command): `Unported` | range-order-sheet |
| Recolouring (`styk-edit/src/recolor.rs`): `editFigureColors`, `setFigureColor`, `replaceFigureColor` (with a whole figure selected, the colour looked up in the palette of the figure and those joined below it, `attachedFigurePalette`, and replaced in each figure that shows it), `resetFigureColors`, and `updatePolygon`'s colour on a figure on show (`mode: 'style'`): the instance's style overlay (`_stykStyle`) written as `recolorObject` writes it, from the colours on show (`resolveInstanceFigure`'s overlays, `displayedSegmentPaint`: overrides mixed in, a full override solid), gradient ends and paint stops, polygons and mesh colours; a reset samples the definition's paints; format 4's whole-figure colour switches the figure to its segments' colours; validated as `validateRecolor` does (palette targets, paints, refusing hidden or locked layers); through the range operation, so at the playhead or across a range | yes, without movieclips and the builder's segment styles (`updateBuilderSegmentStyle`) | recolor-sheet, recolor-format4-sheet, recolor-mesh-sheet, recolor-native |
| Flipping a branch: `mirrorSelection` and `mirrorFigure` with one figure's joint selected turn its segment over (angle and bend), figures joined below following; `mirrorFigure` on whole figures | yes; a selected branch and several selected segments stay with the editor (the facade, below) | range-branch-sheet, range-native, transform-native, range-transforms-sheet |
| `undo`, `redo` of what those record | yes | all of the above |
| everything else | no: `Unported` | |

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
native timeline (a layer sheet's `legacyAnimation`, which every document opened in the app
has instead, is edited), animation origins or a movieclip source; for posing a joined 3D IK target and
configuring or turning IK targets across joined figures;
for poses on a drawing layer; for a replaced figure joined to one
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
- **Layer focus locks** are runtime state of the JavaScript editor: a focus lock refuses
  edits on the other layers. The facade starts no engine session while one is on, and
  changing the focus ends the current session (measured with the driver's focus step:
  without this, owned editing inserted a key the editor refuses on a locked layer).
- **Frame ranges**: on a layer sheet the app passes a timeline range as an array carrying the
  chosen ticks (`rangeFrames()`, `sheetTicks`), which JSON drops; the facade sends such arrays
  as `{"@array": [...], "sheetTicks": [...]}`, and recorded steps may spell them the same way
  (`engine/tools/edit-replay.mjs`). Found while porting the range commands: owned
  `setSceneColor` with a timeline range selected had keyed the playhead only.
- **Several selected segments** (`_segmentSelection`, `toggleSegment`): a branch flip turns them
  all over. As with a selected branch, the facade starts no session and ends the current one;
  both are checked again before every call, since undo can restore them.
- **A selected branch** (`_branchSelection`, `selectBranch`): commands act on the branch's
  segments instead of whole figures (`deleteSelection` deletes the branch). The facade starts
  no session while a branch is selected, and selecting one ends the current session
  (measured with the driver's branch step: without this, owned editing deleted the whole
  figure).
- **Records the editor changes in place** (a topology edit rewrites the figure and every
  pose of it, drawing edits name their figures): the mirror sees changes by identity, so the
  facade notes what the change events name (a definition edit: every figure; one that
  changes structure outside a sheet publication: every figure, keyframe and pose source)
  and resends it before the next session starts (measured: after deleting a branch in the
  editor, engine sessions posed the old pose arrays and adopting them failed the editor's
  validation).
- **The timeline's layer selection** (`_sheetSelection`, `styk-edit/src/select.rs`): the
  facade hands it over with every call, and the engine follows it as the editor does
  (`selectSheetLayer`, seeks selecting the layer's figures on show, `keepSelectionOnSeek`,
  any other selection dropping it, history restoring it with the selection).
- **`runtime_segments`** of legacy Pivot poses are read by the range operations (lengths and
  bends materialized before a transform, and by geometry): the facade keeps those calls in the
  editor for documents whose keyframes carry them.
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

With `--exact-math` the driver replaces the page's `Math.sin`, `cos`, `atan2`, `hypot`,
`asin` and `acos` by the engine's (exported from the WebAssembly binding for this purpose,
`styk-wasm/src/math.rs`) before the document loads: the editor then computes what Node
computes, and the comparison is exact. This is how a difference is told from the browser's
own rounding: on the Kung-Fu Master and Karate Fighter rigs, dragging a free 3D foot
differed by up to 1.54 rad in 34 angles with Chromium's `Math` (a knee's side decided on a
nearly degenerate limb) and was identical with the engine's (95 of 95 calls on each rig,
measured 2026-10-03; also exact: the start-up document 94 of 94, a 3D catalogue scene 74 of
74, joined 55 of 55, the rigged document 78 of 78, sheet-movieclip-loose 78 of 78). With
the driver's 3D steps added (a planted knee dragged, a bone twisted with the right button,
the rig reset), measured the same day: the planted-leg fixture 140 of 140, the Kung-Fu
Master and Karate Fighter rigs 115 of 115 each, a 3D dojo prop 94 of 94.
With the driver's property steps (scrubs with their previews, keyed values, the scene's
colour and background, the figure's outline, an effect's parameter keyed and scrubbed),
measured 2026-10-04 with the engine's `Math`: the start-up document 211 of 211 calls the same,
the property calls among them (`beginAnimationEdit` 4, `previewAnimationEdit` 6,
`commitAnimationEdit` 3, `cancelAnimationEdit` 3, `addAnimationKeys` 2, `setSceneColor`,
`setBackgroundColor`, `setSceneBackground`, `updateFigureStyle` and `setVfxParameter` 1 each);
owned editing the same as the editor alone (106,877 bytes, 55 history entries, and after
undoing everything). The previews are compared: an engine composing a preview with the edit's
first value instead of its last was caught (2 previews different). On documents saved with
property channels (the channel fixtures as the app holds them), the same day: `channels-figure`
184 of 184 calls the same, owned editing the same as the editor alone (70,724 bytes, 46
history entries, and after undoing everything); `channels-pose` 152 of 152; `channels-vfx`
184 of 184.

`node scripts/engine-edit-shadow.mjs [--file <document.json>]` drives the real app in
headless Chromium: pointer drags on joint handles and on the origin, keyboard undo and
redo, transforms, timeline keys and frames, duplicate/join/unjoin/delete, on a figure with
3D bones a planted knee, a right-button twist and resetting the rig, and on a rig with pose
choices another choice. Measured
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

The engine runs ported calls in the editor's place while a session lives (the same
`src/engine/engine-edit-shadow.mjs`): by default for documents of an editing class in
`OWNED_CLASSES` (D15: a class flips once its calls cost no more in the engine than in the
editor), for every document with `?engineEdit=on` (or
`localStorage['stykanimate.engine.ownEdit'] = 'on'`), and never with `off`; `?engineShadow=on`
keeps the editor in charge and compares. `editingClass(report)` names a document's class:
`unported` for documents the engine does not edit at all (property channels on a native
timeline, animation origins, a captured movieclip's own document; the engine draws them),
`channels` for documents whose layer sheet keeps saved property channels (`legacyAnimation`,
edited since 2026-10-04; not owned by default: on `channels-figure` owned editing cost
289.3 ms against the editor's 116.9 ms in one run, and large documents are not measured),
`movieclips` when a
figure is a movieclip, `3d` when a figure has 3D bones or a mesh,
`large figures` from 1,000 segments across its figure definitions, else `figures`. The
calls that change the document
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
compared as in shadow mode (history stays the editor's). Commands the timeline groups into one history entry
(`movieclipBatch`) run their ported calls through the session, then the batch's own event ends
it, and undoing the batch ends the next one (history the session did not record); measured
with the driver's batch step, owned editing still matches the editor alone. A call the engine refuses, or whose
answer the editor cannot adopt, runs in the editor alone and ends the session; an error runs
the editor's call so that the editor raises it with its message. If the engine itself fails a call (it
throws), the editor runs it and engine sessions stop for the rest of the page. In shadow mode the answer's
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

### Which classes the engine edits by default

`large figures` since 2026-10-03. `node scripts/engine-edit-shadow.mjs --own --default`
runs the shadow driver's interactions with the editor alone, then with the app as users get
it; on large figure documents the engine owned every call it could and cost less in total,
in every run (headless Chromium, the machine under other load, two rounds):

| Document | Calls owned | Editor alone | Engine owning, adoption included | Same document, history and selection |
| --- | ---: | ---: | ---: | --- |
| 20 figures of 200 segments | 54 | 163.2 ms, 213.6 ms | 100.1 ms, 175.8 ms | yes |
| AyameDemo.piv (537 figure definitions, 6,854 segments) | 54 | 946.7 ms, 1,171.2 ms | 757.7 ms, 801.8 ms | yes |

`figures` stays with the editor: on the document the app starts with, the same run cost
87.8 ms owned against 76.8 ms alone, a pose about 0.31 ms in the engine plus 0.49 ms to adopt
its answer against the editor's 0.58 ms for the whole call (medians of 48 in the app; the
answer crosses the JSON boundary and the editor's own events and caches still run). Warmed
up (`--warm`: the interactions once untimed, then timed, since the browser optimizes hot
WebAssembly after its first calls: session calls measured in one page went from 0.24 to
0.09–0.13 ms a pose), small figure documents still cost more owned: the start document 66.1
against 50.5 ms, rig 123.3 against 50.9 ms, joined-chain 75.1 against 46.1 ms (startup-pinned
95.3 against 260.2 ms). The editor computes a pose of a ten-segment figure in about 0.07 ms;
the engine's call (0.25–0.35 ms in the app) and the adoption of its answer cannot get below
that while both editors keep their own bookkeeping, so small documents wait for the engine
to own the document outright (S5, phases 2 and 3). Running the animation while calling the
session did not slow the calls (the engine's worker threads do not contend with it).

`3d` flipped on 2026-10-04: with every 3D command and gesture the shadow driver drives ported,
owned editing was the same as the editor alone and cost less on each 3D document measured
(`node scripts/engine-edit-shadow.mjs --own --warm --exact-math`, one run each, the machine
under other load):

| Document | Calls owned | Editor alone | Engine owning, adoption included |
| --- | ---: | ---: | ---: |
| a planted 3D leg (ik/spatial) | 97 | 217.6 ms | 101.5 ms |
| Kung-Fu Master (81 segments, 3D, IK; imported) | 100 | 510.1 ms | 228.9 ms |
| Hand-Right (3D hand; imported) | 110 | 513.4 ms | 480.3 ms |

With the browser's own `Math` instead of the engine's, an owned 3D pose can differ from the
editor's by more than its last digits (one angle of the planted leg by 4.6e-6 rad after a
drag): the 3D solvers iterate on values the browser rounds differently (D16); the engine's
answer is the same in every browser. `movieclips` stays with the editor: every call the driver
makes on the movieclip documents now runs in the engine (229 of 229 the same), but these small
documents cost more owned, as small figure documents do (sheet-movieclip 433.9 ms against
299.4, sheet-movieclip-loose 528.3 against 337.4, owned the same as the editor alone).

What the same calls cost on AyameDemo.piv (one run each; the editor's time from a shadow
run, the engine's from an owned run, adoption included). The first measurement found
`poseJoint` at 200.6 ms; the causes were fixed: a rotation's targets were found with list
membership tests (quadratic in the figure's 515 segments) and on every call, each angle was
changed by unpacking and repacking the whole angle array, every evaluation scanned the
document for 3D, and every call drew the whole frame. Sessions now also start from the
mirror's prepared document instead of converting it again.

| Call | Calls | Editor | Engine | Adoption |
| --- | ---: | ---: | ---: | ---: |
| `poseJoint` (pointer drags) | 36 | 25.3 ms | 56.1 ms | 15.9 ms |
| `moveSelection` | 3 | 10.2 ms | 17.0 ms | 8.0 ms |
| `sheetInsertFrames` | 1 | 393.3 ms | 359.6 ms | 70.8 ms |
| `duplicateFigure`, `joinTo`, `unjoinFigure`, `deleteSelection` | 4 | 365.9 ms | about 100 ms | about 40 ms |

All owned calls together: 899 ms against the editor's 898 ms. Those whole-run totals mix in
the session's first call (about 6 ms once, while the browser compiles the engine's code to
its fast tier) and the load of the machine, so the costs were then measured call by call.

`node scripts/engine-edit-cost.mjs <file>...` (headless Chromium, the same document,
figure and joint): four drags of 15 pointer moves, the first not counted, through an edit
session as engine-owned editing calls it (the answer parsed) and through the editor (the
time its change listeners take subtracted: both pay it). Adopting a pose answer adds about
0.05 ms. Medians, 2026-10-03, after two fixes found this way: publishing a gesture matched
each key of a layer against every earlier key (quadratic in a layer's keys), and the
document's typed views re-read every style overlay after each commit (AyameDemo's commit
was 7.7 ms in WebAssembly, and natively 12.7 to 13.0 ms against 1.5 to 1.7 ms after, in
interleaved runs of `styk edit-bench`):

| Document | Class | Pose: editor | Pose: engine | Commit: editor | Commit: engine |
| --- | --- | ---: | ---: | ---: | ---: |
| the document the app starts with (10 segments, unpinned IK) | figures | 0.325 ms | 0.16 ms | 0.81 ms | 0.22 ms |
| rig (connectors, angle locks) | figures | 0.19 ms | 0.065 ms | 0.58 ms | 0.115 ms |
| joined-chain (three joined figures) | figures | 0.22 ms | 0.075 ms | 0.68 ms | 0.185 ms |
| 20 figures of 200 segments | figures | 0.36 ms | 0.23 ms | 4.02 ms | 1.915 ms |
| AyameDemo.piv (105 figures, largest 699 segments) | figures | 1.59 ms | 0.835 ms | 3.1 ms | 2.69 ms |
| a planted 3D leg (ik/spatial) | 3d | 0.855 ms | 0.895 ms | 0.585 ms | 0.195 ms |
| Kung-Fu Master (81 segments, 3D, IK) | 3d | 1.66 ms | 0.74 ms | 0.5 ms | 0.25 ms |
| Hand-and-Face Talk and React (6 figures) | 3d | 1.985 ms | 1.31 ms | 0.055 ms | 0.115 ms |

Figures of 10 to 800 segments (synthetic trees, one figure) measured the same way: poses
0.07 to 0.235 ms in the engine against 0.185 to 0.93 ms, commits 0.11 to 0.44 ms against
0.52 to 1.36 ms.

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
median 0.39 ms (8.42 ms before: also the drag's cached rotation targets, as the editor's
`_drag.rotationTargets`, and the geometry of the posed figure and those it hangs from only,
`geometry::build_only`), a move step 0.02 ms, commits 5.0–9.0 ms, undo 2.1 ms.

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
