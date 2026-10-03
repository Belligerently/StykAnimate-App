# Engine: evaluation

How the engine turns a document and a tick into world-space geometry and a
draw list (`engine/crates/styk-eval`). The JavaScript editor's evaluation
(`src/pivot/geometry.mjs`, `src/layer-sheet.mjs`, `src/animation-pose.mjs`, …)
is the oracle while both exist.

## Contract

- `Prepared::new(document)` builds the timing tables once per document
  revision (native timeline rows, layer-sheet pose tracks, movieclip
  replacement tracks, presence clocks). `Prepared::evaluate(tick)` is a pure
  function of the prepared document and the tick: any thread evaluates any tick.
- A `Frame` is the evaluated scene: objects (`Arc<Object>`, shared with the
  document when untweened), draw order, camera, backgrounds, presence-clock
  movieclips, where each pose came from (`origins`, for editing) and the frame's
  effects (`vfx`, see [vfx.md](vfx.md)).
- `geometry::build(document, frame)` resolves joins, rigs ([rigs-ik.md](rigs-ik.md)),
  IK, movieclip instances, Stick Nodes shapes and 3D ([spatial.md](spatial.md):
  perspective, one-sided surfaces, figure and attached movieclip meshes) into a
  `Scene`: per object its joints and drawable items in world units, and the
  scene's evaluated meshes and surface depths.
- `drawlist::encode_composed(document, assets, scene, overrides, ghosts, order)`
  turns a scene into a draw list ([rendering.md](rendering.md)): translucent
  figures and outlines become isolation groups; onion ghosts are tinted,
  faded groups placed in `composeSceneGroups` order (`onion.rs`); session
  state (hidden figures, drag offsets, the canvas colour) comes in `Overrides`.

## Order of evaluation

1. Native keyframes tween with Pivot's rules (`evaluatedNativeFrame`): easing,
   Auto curves, colour and background transitions, IK controllers.
2. A layer sheet samples each layer's own pose track (`sampleLayerSheet`),
   applies its keys' movieclip replacements (`replaceMovieclipFrame`) and
   composes layers in order.
3. Movieclip instances choose their local time (`sampleMovieclipObjects`),
   layers order the draw order (`applyLayerOrder`).
4. The scene camera ([below](#the-scene-camera)): where it travels moves parallax
   layers, then camera-motion impulses shake the shot.
5. Effects resolve per object and for the scene.

## Animation origins

Set Origin re-roots a figure's construction tree at another joint while its animation
keeps the original pivot and joint basis (`styk_eval::origin`, `animation-origin.mjs`,
`animation-origin-support.mjs`). The figure's `_styk.animationOrigin` is compiled at load
against its segments: the original tree re-derived from them (`animationOriginLayout`), or,
once construction edits removed joints of it, the retained support graph whose removed
bones are ghosts. A pose stores its pivot (`_stykAnimationOrigin`: `x`, `y`, a local
`offsetX`/`offsetY` after an edit moved the pivot's vertex, and the ghosts' poses).

- A native tween interpolates the pivot (and its offset when either key has one) and the
  angles of the original tree relative to their parents there, as before Set Origin; with a
  support graph the bones are interpolated as their own tree, ghosts included.
- Every native interval (exact keys included) then places each unjoined figure's root so
  its pivot lands where the pose says (`correctAnimationOrigins`), with V8's trigonometry
  (`styk-math`). Poses a layer sheet shows directly, and edit frames, come as the editor
  left them.
- The figure's rotation (`animationRotation`: the rotation reference's turn) is what rigs,
  inverse kinematics, meshes and attached movieclips read (`origin::rotation`, also behind
  `ik::figure_angle`).
- The evaluated frame as records still refuses documents with origins (`Records::new`),
  so their fixtures (`engine/tools/origin-fixtures.mjs`, `tests/origin.rs`: the re-derived
  tree with a mirrored instance and a joined figure, a real local offset, a support graph
  with ghosts) live apart from `eval/`, which `records-fixtures.mjs` records.

## Colour transitions

A pose's `color_transition` (`{from, to, t}`; no command writes one, older documents
carry it) replaces the object's override wherever the editor colours its figure
(`objectColor`): every segment, polygon and mesh palette colour, and the paint stops and
second colour of a gradient, is mixed with both overrides and taken `t` of the way between
them, rounded (`geometry::object_rgb`). Items keep an index into the scene's transitions
(`Scene::transitions`, so items stay plain data) for the draw list's gradients; movieclip
contents use their own object's. A tween keeps the first key's
transition, as the editor's does. Whether a two-colour gradient is drawn at all still
follows the override alone (an opaque override draws the segment solid), as in the editor.

## Stick Nodes sprites

An imported Stick Nodes sprite keeps its stretch and pivot on the pose (`_sticknodesSprite`:
`ratio`, `pivotX`, `pivotY`; the Stick Nodes importer writes it). The editor applies them to
the picture's asset when it builds the segment (`sticknodesSpriteAsset`): shown at the
asset's width and its height times the ratio, anchored at the pivot. The engine applies them
where it resolves an object's figure, after a style overlay (`Loader::styled_object`,
`sticknodes::sprite_figure`, shared by identical sprites), so the draw list, effect bounds
and editing all read the stretched asset. `styk compare` checks where pictures are drawn
when the reference has them (`reference.mjs --images`).

## The scene camera

What the camera does when the scene is seen through it: prepared playback frames
always, edit frames only in the camera preview (`cameraEnabled`). The editor applies
both at render time (`cameraAtTime`, `parallaxResolver`); the engine works them out
with the frame and draws them.

- **Travel** (`cameraTravelAt`, `Prepared::travel`) is where the camera is: the
  authored camera, tweened with its baked camera-motion offset (`_parallaxMotionOffset`,
  tweened when either key has one, like the other camera channels), moved by the follow
  clips active at the frame's time.
- **Follow clips** (`styk_eval::follow`, `camera-follow.mjs`): per tick the followed
  figures are evaluated in a projection of the document holding only them, the figures
  they hang from and the camera (`followSamplingReport`), and measured from their
  geometry (`ObjectGeometry::extent`, `figureBounds`: visible segments with half their
  stroke, circles by radius, curves by their points, every polygon including culled
  surfaces, a movieclip's handle). A forward pass shapes the samples per clip frame
  (dead zone with hysteresis, look-ahead, holding the last seen target or aiming at the
  first one ahead, a critically damped spring per axis, framing zoom) into offsets and a
  log zoom stored in single precision, as the editor's `Float32Array`; overlapping clips
  blend by weight. The pass is sequential: a clip's path is shaped once per prepared
  document, lazily up to the frame asked for and 256 frames beyond, by whichever thread
  needs it first, outside any lock (the longest path published wins: a pointer swap under
  a spinning lock, D7). The editor keeps its paths across edits and resamples from the
  key before a change; the engine shapes each revision's paths afresh.
- **Parallax** (`styk_eval::parallax`, `layer-parallax.mjs`): each layer whose parallax
  is on moves by `(1 - response) × (travel - reference - baked offset)` per axis; a
  joined figure moves with the layer of its join chain's root. `Frame::parallax` holds
  the offset of every object by slot (`None` when nothing moves, and always for a
  movieclip's own frame: its contents move with their instance). It is a translation
  of the whole object, artwork, effects and movieclip contents alike, which the draw
  list takes as one more per-object translation after a drag's
  (`Parallax::translations`); geometry, records and editing never see it. Onion skins
  take none: the editor shows them only with the camera off.
- **Impulses** (`camera::at_time`, `sampleCameraMotion`) shake the shot from the
  travelled camera.
- **Edit frames** carry their time on the camera-motion clips (`_cameraMotionTick`); in
  the camera preview the stage draws them through `cameraAtTime` worked out against the
  committed generation (a follow path reflects an edit once the mirror commits it).

## Not evaluated yet

Named in `Document::unsupported` and gated in the app: property channels on
native timelines and frame drafts; a captured movieclip's retained source
(`movieclipSource`) is not read (the engine tweens the clip's keyframes where the
editor samples the source). The work queue in the plan orders them.

## Testing

- `styk compare <file> <reference.json>` against `engine/tools/reference.mjs`
  (every primitive and joint of the sampled ticks; `--vfx` adds effects, `--camera`
  the shot camera and each object's parallax offset).
- Fixture suites built by the editor's own commands:
  `engine/tools/ik-fixtures.mjs`, `eval-fixtures.mjs`, `vfx-fixtures.mjs`,
  `spatial-fixtures.mjs`, `origin-fixtures.mjs` (byte-stable), checked by `cargo test -p styk-eval`.
- `npm run engine:parity -- --corpus` runs the benchmark files at every tick.

## The evaluated frame as records

A gesture commits by storing the frame the editor shows: `snapshotPoseFrame(
editor.evaluatedFrame)` becomes a pose source. `styk_eval::records` produces that record,
byte for byte, so the commit can run in the engine ([editing.md](editing.md)). It mirrors
the record-level shape of the editor's evaluation (which record a pose comes from, which
keys a tween writes and in which order: `sampleLayerSheet`, `evaluatedNativeFrame`,
`buildVirtualPoseTrack`, `sampleMovieclipObjects`, `applyLayerOrder`,
`snapshotPoseFrame`) and takes every interpolated number from the typed evaluator, so the
arithmetic has one implementation. Frames it does not produce yet answer `Unported` with
the reason.

- Covered: imported layers (the native timeline, exact and tweened), layers with authored
  pose keys (virtual pose tracks), the whole-pose shortcut and the per-layer composition,
  tweened IK controllers (`interpolateIK`), figure and scene effect stacks
  (`interpolateVfx`, parameters resolved in the registry's order, paints normalized),
  layers that mix imported and authored keys (the window of keys around the playhead),
  movieclip frame replacements (`replaceMovieclipFrame`, tweened along their own track),
  movieclips leaving the frame outside their span, layer order, 3D poses between keys
  (`interpolateSpatialPose`'s record).
- Not yet (each answers `Unported`): figures joined to a hidden movieclip, animation
  origins, property channels, hidden pose keys, a movieclip's own document. A tweened
  camera keeps its baked camera-motion offset (`_parallaxMotionOffset`) as the editor
  writes it.
- Pivot's own easing unwraps each angle along the whole run of tweened keys before the
  interval. The drawing path unwraps from the previous key only (constant time, the same
  picture); records follow the editor's chain (`evaluate_native_exact`), or a stored
  angle would sit whole turns away (found on `hitoshi vs suzu tomiee ssj.piv`, tick 182;
  the `eval-legacy-winding` fixture fails without it).
- Angles are tweened through `styk_math::shortest`: with the platform's `sin` and `atan2`
  an IK target rotation came out as `-0.06` where the editor stores
  `-0.05999999999999999` (D16).
- Harness: `engine/tools/records-reference.mjs <file> <folder>` records the editor's loaded
  report and, per tick, the snapshot (SHA-256, and the JSON for small documents);
  `styk records <folder> [--write]` compares. `engine/tools/records-fixtures.mjs` records
  every editor-built fixture into `tests/fixtures/records/`; `tests/records.rs` requires
  every recorded frame to be produced and identical.
