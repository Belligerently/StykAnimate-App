# Engine: visual effects

How the Rust engine (`engine/crates/styk-eval/src/vfx/`) decides, for one tick, which effects apply to what and with which parameter values. The user-facing behaviour is in [`VFX.md`](../VFX.md), [`VFX-DESIGN.md`](../VFX-DESIGN.md) and [`VFX-GENERATORS.md`](../VFX-GENERATORS.md); the JavaScript editor is the oracle while both exist ([`RUST-ENGINE-PORT.md`](../RUST-ENGINE-PORT.md)). Drawing the effects is the renderer's ([Rendering](#rendering)).

## Contract

- `Prepared::evaluate(tick)` (and `evaluate_clip`) fills `Frame::vfx: Option<Arc<vfx::FrameVfx>>`: everything a renderer draws for effects at that tick, or `None` when there is nothing. When `Prepared::has_vfx` is false (no effect clip and no per-pose stack in the document or its movieclips) evaluation does no effect work and allocates nothing.
- `FrameVfx` holds, by object slot:
  - `scene`: the camera image's stack, the scene's per-pose stack then its clips;
  - `objects[slot]` (`FrameVfx::object`): `figure`, the stack of the object's own artwork; `attached`, the connected group it owns (its stack and member slots); `segments`, node stacks by handle id;
  - `roots`: the slots owning a drawn connected group, in compositing order (smaller groups first, so nested groups filter before the groups containing them);
  - `generators`: generated effects inside their clip's range, with their parameters, opacity and start, never simulated;
  - `warnings`: connected groups nested too deep.
- Every stack is what `activeVfx` returns, in render order: enabled filters that change their input, each an `Arc<vfx::Effect>` whose `Params` is one struct per effect type with typed fields (`f64`, `Color` for an RGB integer or a paint, an enum per option list, `bool` switches, `u32` seeds and counts). Values are those the editor hands an executor: keys sampled, poses tweened, `effectStrength` multiplied by the clip's strength and fades.
- `vfx::padding`, `vfx::sampling_padding` and `vfx::working_slots` take such a stack (`vfxPadding`, `vfxSamplingPadding`, `vfxWorkingSlots`).
- Saying what must be drawn: `Document::unsupported` no longer names effects, since they are evaluated. A frame whose `vfx` is `Some` has effects the renderer must draw, and `vfx::document_has_vfx` (`documentHasVfx`) tells whether a document carries any at all. The interface's gate (`src/engine/engine-eligibility.mjs`) still sends every document with effects to the JavaScript renderer; a renderer adopting them must check these, not `unsupported`.

## Evaluation order

1. **Per-pose stacks** (`Object::vfx`, `Object::vfx_scope`, and `Keyframe::vfx` for the scene) are read with the pose. Between two keys they tween with it (`vfx::interpolate`, called from `eval::evaluate_native` for the scene and from `tween_object` for objects) at the pose tween's own progress. A layer sheet takes the scene's stack from the scene track. Values of property channels (`vfx1:*`, `vfxStack`, `vfxScope`) would apply to the tweened stacks here; the engine does not evaluate channels yet and names them in `unsupported`.
2. **Clips** at the tick (`renderVfxOwner`): enabled, in range, with strength; a node clip scoped to another figure type is skipped. Their filters append to the target's stacks after the per-pose ones, clips in saved order. A figure clip that includes attached parts also feeds the connected stack and turns the scope on.
3. **Movieclip containers** (`buildMovieclipGeometrySteps`): an instance's `figure` stack is the clip's own scene stack at the instance's local time (`evaluate_clip`, only when the clip document has effects), then the instance's. The clip's own objects have their stacks in the clip frame's own `FrameVfx`.
4. **Connected groups** (`connectedVfxGeometry`): an object whose scope is on and whose connected stack is active owns a group: itself and every object joined below it, breadth first. Scopes nesting deeper than 16 leave the group out with a warning. The root's `figure` drops the effects the group applies (by identity).
5. **Generated effects** (the clip loop of `collectLayers`): sampled at the tick, at most just before the clip's end, with opacity `strength × fade × effectStrength`, from `start + delay`.

## Modules

| Module (`src/vfx/`) | Holds | JavaScript |
| --- | --- | --- |
| `mod.rs` | `Effect`, `Stack`, `Stacks`/`StackKey` (`_stykVfx`), `active`, `interpolate`, `document_has_vfx` | `activeVfx`, `interpolateVfx`, `documentHasVfx` (src/vfx.mjs) |
| `registry.rs` | `Kind`, `Params` and one struct per type, `Spec`, option enums, `neutral`, `available` | `VFX_REGISTRY`, `neutralVfx`, `effectAvailable` |
| `paint.rs` | `normalize`, `interpolate`, `js_round` | `vfxPaint`, `interpolateVfxColor`, `normalizePaint`, `samplePaint` |
| `parse.rs` | saved effects, stacks and clips → typed (lenient) | `validateVfxStack`, `validateVfxOwner`, `validateVfxClips`, `vfxDescriptor` |
| `clips.rs` | `Clip`, `Clips`, `Target`, `Channel`, `Key`, `sample_keys`, `Clip::{contains, time, fade, sample, active_at}` | `clipContains`, `clipTime`, `vfxFadeAt`, `sampleVfxKeys`, `sampleVfxClip`, `vfxClipActiveAt` |
| `frame.rs` | `evaluate` → `FrameVfx`, `generators` | `frameWithVfxTime`, `vfxRenderFrame`, `renderVfxOwner`, `connectedVfxGeometry` (planning), `collectLayers` (clips) |
| `padding.rs` | `padding`, `sampling_padding`, `working_slots` | `vfxPadding`, `vfxSamplingPadding`, `vfxWorkingSlots` |
| `parity.rs` | comparison with `frames[].vfx` of a reference dump | — |

The loader (`load.rs`) reads `_stykVfx`, `_stykVfxScope` and `_styk.vfxClips` (clips cached by record, like figures and keyframes). Generator particles (`vfx-generators.mjs`), rendering and the inspector are not here.

## Invariants

- **Same arithmetic as the JavaScript**: operand order, clamps where the editor clamps (tweened numbers to their range, colour channels to 0–255), no clamp where it does not (keyed values), `Math.round` (`paint::js_round`, halves up), stable sorts of gradient stops, `Map` semantics (the last effect with an identity wins).
- **Effects resolve once**, when read: defaults filled, paints normalized, version 1 outlines square. Keys and tweens produce valid values, so nothing is validated again during evaluation; unkeyed, untweened effects are shared `Arc`s from frame to frame.
- **Order is render order**: per-pose effects first, clips in saved order; a container's clip scene effects before its own.
- **Presence and activity differ**: `document_has_vfx` counts an empty `_stykVfx`; stacks only hold active effects.
- **Pure**: a function of the prepared document and the tick; no clocks, threads or files (it builds for `wasm32-unknown-unknown`).

## Differences from the JavaScript, on purpose

- **Lenient reading**: the editor refuses a project with an invalid effect record; the engine drops the record (an effect, a stack entry, a key channel, a clip) and says so in `Document::notes`. Key easing is not validated, nor the limits on keys and simultaneous effects.
- **Generators** are listed within their clip's range only: particles outlive a clip by a tail only the generator compiler knows (`generatorClipTail`); `vfx::generators(clips, tick, tail)` takes it. Whether the target is present (an anchor) is the generator's to check. Their colours are normalized paints; the editor's generator code reads the saved form.
- **Connected groups** list every member; the renderer leaves out hidden ones and skips a group whose owner is hidden (the editor plans with visibility). The depth warning is given for every too-deep owner, hidden or not.
- **Edit frames** (`Loader::edit_frame`) keep their per-pose stacks but get no `vfx`. The editor sends the poses of its geometry, which already hold clip effects (`_stykVfx` composed, `_vfxConnectedEffects`): strip those before evaluating effects for an edit frame.
- **Not evaluated**: property channels of effects (named in `unsupported` as property channels), the clips of a captured movieclip's retained source (`movieclipSource`, noted in `unsupported`; the engine does not read retained sources at all), and `_stykVfx` on a figure definition's segments, which nothing in the editor reads either.

### JavaScript behaviours reproduced that look wrong

- Node stacks and node clips saved without a figure type (`*/handle`) are never drawn: renderers look node stacks up by the object's own figure type. The inspector always scopes, so only older or hand-made data has them.
- With Auto easing, a native pose tween interpolates effect parameters linearly while the pose follows the Auto curve (`interpolateVfx` receives the raw progress).
- `includeAttached` on a node clip is accepted and ignored.
- The inspector's "active" (`vfxClipActiveAt`) ignores fades: a clip at a fade endpoint is listed though nothing is drawn.
- A scope on an object without any effect still counts toward the 16-level limit of nested groups.
- Interpolating two gradients of different kinds or stop counts keeps the first, with no transition; a solid and a gradient interpolate on the gradient's layout.

## Rendering

*To be written by the renderer owner*: how `FrameVfx` reaches the GPU and CPU executors (camera stack, per-object and node stacks, connected runs, container stacks, generator batches), budgets and caches.

## Changing it

- **A parameter**: add a field to its type in `registry.rs`, in the registry's order, with its saved name, default and range; a switch is `bool`, an option list an enum (`choice!`), a discrete number `u32`. If it changes what is neutral, the padding or the working slots, update `Params::neutral`, `padding.rs`. Regenerate the fixtures; give the parameter a non-default value in one.
- **An effect type**: an `effects!` entry (with `generator`/`scene_only`), its `neutral` rule, its padding and slot rules, `Params::delay` for a generator, `Kind::available` if the scene catalogue changes. Add it to a fixture.
- **Behaviour changes start in the JavaScript** while it is the oracle: change it there, regenerate the fixtures, then make the engine match.

## Testing

```sh
node engine/tools/vfx-fixtures.mjs                # regenerate engine/crates/styk-eval/tests/fixtures/vfx/ (byte-stable)
cd engine && cargo test -p styk-eval --test vfx   # well under a second
cargo clippy -p styk-eval --all-targets
cargo build -p styk-eval --target wasm32-unknown-unknown
```

`tests/vfx.rs` loads every fixture and:

- compares geometry, then every stack effect by effect and parameter by parameter (continuous values within 1e-9 relative, colours, options, seeds, paddings and working slots exactly), connected groups and their order, generated effects and active clips;
- checks that each fixture stops matching without its clips or without its per-pose stacks, and that tweened fixtures stop matching when keys cannot tween;
- checks that documents without effects resolve none;
- replays recorded inputs of `interpolateVfxColor`, `sampleVfxKeys`, `vfxPadding`, `vfxSamplingPadding`, `vfxWorkingSlots` and `neutralVfx` (`vectors.json`).

Against a real file:

```sh
cd engine && cargo build -p styk-cli
node tools/reference.mjs <file.styk> /tmp/reference.json --every 1 --vfx
target/debug/styk compare <file.styk> /tmp/reference.json     # adds an effects line
```

Measured when this landed: `TDD2 URIEL NEMESIS 3.styk` (40 clips), all 232 ticks: 602 stacks and 337 effects compared, no difference (nor in its geometry). The Impact FX pack (`AssetReleases/Impact-FX`: the fight scene, 240 ticks, and the 28 movieclip files) keeps its effects, all particles, inside 8 distinct movieclip documents; opened alone, each compares 18 ticks and 16 generated effects with no difference. Their geometry differs where they use 3D poses and surfaces, which the engine does not evaluate yet.
