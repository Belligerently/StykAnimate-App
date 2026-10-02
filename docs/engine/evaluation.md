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
  IK, movieclip instances and Stick Nodes shapes into a `Scene`: per object
  its joints and drawable items in world units.
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
   layers order the draw order (`applyLayerOrder`), camera presets apply.
4. Effects resolve per object and for the scene.

## Not evaluated yet

Named in `Document::unsupported` and gated in the app: 3D poses and meshes,
surfaces, property channels on native timelines, animation origins, follow
cameras, layer parallax, frame drafts, retained movieclip sources. The work
queue in the plan orders them.

## Testing

- `styk compare <file> <reference.json>` against `engine/tools/reference.mjs`
  (every primitive and joint of the sampled ticks; `--vfx` adds effects).
- Fixture suites built by the editor's own commands:
  `engine/tools/ik-fixtures.mjs`, `eval-fixtures.mjs`, `vfx-fixtures.mjs`
  (byte-stable), checked by `cargo test -p styk-eval`.
- `npm run engine:parity -- --corpus` runs the benchmark files at every tick.
