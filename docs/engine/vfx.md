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
- Saying what must be drawn: `Document::unsupported` does not name effects, since they are evaluated. A frame whose `vfx` is `Some` has effects the renderer must draw, and `vfx::document_has_vfx` (`documentHasVfx`) tells whether a document carries any at all. The engine draws every filter ([Rendering](#rendering)); the interface's gate (`src/engine/engine-eligibility.mjs`) keeps documents with generated effects on the JavaScript renderer, by effect type.

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
| `frame.rs` | `composed`: the `FrameVfx` of an edit frame from the fields the editor composed | `vfxRenderFrame` (its output), `connectedVfxGeometry` |
| `layers.rs` | `objectBounds` for effect layers: `item_extent`, `rounded` | `objectBounds`, `drawableVfxPadding` |
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
- **Edit frames** take their effects from the pose the editor sends, never from the clips again: the editor's geometry already composed clip effects into each pose (`_stykVfx` per key, `_vfxConnectedEffects`, a scope a clip turned on) and the scene's into `_vfxTimelineScene`. `Loader::edit_frame` reads those fields (`Object::vfx_connected`; `_vfxTimelineScene` becomes the frame's scene stack) and `vfx::frame::composed` builds the `FrameVfx` from them: the same stacks, connected groups and movieclip contents as `evaluate`, with nothing sampled twice. Movieclip contents are still evaluated by the engine at the instance's local time. Playback frames are evaluated by the engine itself (`evaluate`).
- **Not evaluated**: property channels of effects (named in `unsupported` as property channels), the clips of a captured movieclip's retained source (`movieclipSource`, noted in `unsupported`; the engine does not read retained sources at all), and `_stykVfx` on a figure definition's segments, which nothing in the editor reads either.

### JavaScript behaviours reproduced that look wrong

- Node stacks and node clips saved without a figure type (`*/handle`) are never drawn: renderers look node stacks up by the object's own figure type. The inspector always scopes, so only older or hand-made data has them.
- With Auto easing, a native pose tween interpolates effect parameters linearly while the pose follows the Auto curve (`interpolateVfx` receives the raw progress).
- `includeAttached` on a node clip is accepted and ignored.
- The inspector's "active" (`vfxClipActiveAt`) ignores fades: a clip at a fade endpoint is listed though nothing is drawn.
- A scope on an object without any effect still counts toward the 16-level limit of nested groups.
- Interpolating two gradients of different kinds or stop counts keeps the first, with no transition; a solid and a gradient interpolate on the gradient's layout.

## Rendering

The engine draws every filter the way the editor's WebGPU renderer does (`rasterObject`, `filterArtworkVfx`, `withVfx` and `CameraVfx` in the JavaScript), on WebGPU, native and WebGL2 alike. Generated effects are not drawn by the engine yet: a document whose effects timeline holds one keeps the JavaScript renderer (`engine-eligibility.mjs`, by effect type). The editor's effects preview switch reaches the engine for edit frames (`Stage::set_effects`) and prepared playback (`EngineHandle::set_effects`), which then draws without effects.

### From effects to layers (`drawlist.rs`, `vfx/layers.rs`)

A filtered group is an isolation group with a raster **layer** (`Group::layer`, `styk_draw::EffectLayer`). Encoding reads the frame's `FrameVfx` (and, for movieclip contents, each owner's clip frame: `Scene::clip_vfx`) and nests groups as the editor's painter does:

- **Levels of a run** (`path`): the connected groups of its frame holding it (outermost first, `FrameVfx::roots` reversed), then each movieclip instance of its chain (`Level::Owner`, its container stack: the clip's scene stack then the instance's) with the connected groups of that clip's frame. Consecutive runs sharing a level share its group, so a connected group splits where a foreign figure interrupts it and its runs share one domain (`owner.domainBounds`), as `connectedVfxGeometry` does. Hidden members do not split a run; a group whose owner is hidden is not formed.
- **An object** (`Builder::object`): its figure stack over its outline and items, then each node with a stack (`segmentVfx`: segments, never polygons) on a layer of its own in painter order. Opacity applies after the stack.
- **Entries**: the outermost group of top-level artwork that holds effects anywhere (an object, a movieclip instance or a connected run) is an *entry* (`LayerKind::Entry`), even when it is opaque and has no stack of its own: the editor rasterizes such artwork whole and resamples it. Groups inside an entry are *nested* (`LayerKind::Nested`).
- **The scene stack** becomes the *camera image* (`LayerKind::Camera`): the background and the frame's runs drawn again inside it, composited last over the unfiltered frame, then the onion skins over it (`ViewportVfx` draws them as overlays).

Every layer carries the rectangles the editor filters on: `objectBounds` with the padding of every stack from that level down to each item (`drawableVfxPadding`), and without it (the effects' **domain**, which global coordinates, gradient positions and centres are relative to), in whole world units. `vfx/layers.rs` ports `objectBounds` (three units beyond half the stroke width and the figure outline, circles by radius, sprites and cels by their corners, text by its glyphs and box). `styk layers <file> <tick>` prints them; `engine/tools/vfx-layer-bounds.mjs` prints the editor's for a project, to compare.

Edit frames take their stacks from the pose the editor sends (see the Contract): nothing is applied twice.

### Spaces, density and units (`styk-render/src/effects/plan.rs`)

Each frame, every group is placed in a *space* (world units to layer pixels) for the view:

- the frame and plain isolation groups: their parent's space (the screen at the top);
- an entry: a world-aligned grid anchored at its bounds, at `rasterResolution` of the view's pixels per world unit (the scale rounded up to a quarter octave, at least 0.5), lowered by the editor's raster budget (128 MiB), the texture limit and `vfxRasterScale` (3,968 pixels over the bounds plus twice the largest padding); above 8× or past the texture limit it is the editor's `rasterBounds` crop of the complete grid;
- a nested layer: its entry's grid, on its bounds grown by its effects' padding plus two pixels (`withVfx`);
- the camera image: the scene camera at the camera preview's density (the workspace zoom, times the camera's scale in the neutral workspace, at most 2,048 pixels on the longest side and within the effect surface), with the effects' padding plus two pixels around it.

So effects are view independent exactly as the editor's are: sizes are authored units times the owner's absolute scale times the layer's density (`Filter::scale`), coordinates are global pixels from the domain's corner (`Filter::origin`, `output_size`), and the density follows the view in quarter octaves. Between buckets an entry is resampled to the screen (bilinear, clamped to its edge texels as an exact texture is); the camera image is stretched onto the camera rectangle as the editor's quad is.

Every layer is **cropped** to what can reach the screen: the drawn part of its parent, widened by its effects' sampling reach (`vfx::sampling_padding`) and two pixels. A crop keeps the pixel grid and the global coordinates, so visible pixels are those of the complete layer; a layer entirely off screen is not drawn.

### Passes (`frame.rs`, `effects/executor.rs`, `effects/passes.wgsl`)

Layers are packed into the two isolation atlases as before (one pass per nesting depth, rounds when they overflow). Pages are at least the frame's size and grow, in 256-pixel steps, to the largest layer (an entry at the bucketed density, the camera image). Each group has a six-texel record in the group table (`effects::record`): world to atlas, page, atlas rectangle, its composite into its parent (atlas pixels to the parent's target) and the part shown.

After a depth's layer passes, each filtered layer of that depth is copied into working slot 0, filtered, and copied back in place, so the next depth composites the result. The executor records each stack exactly as `VfxWebGPU.run` does: the same operations (`passes.wgsl` is the editor's WGSL with the operation as a uniform), the same slot choice, saves, three box passes per axis with downsampling on the global grid past 20 pixels, rounded or separable dilation, partial strength through the strength source, and the same uniform values in single precision. Slots are pooled at one size, so a pass uses its slot's top-left corner; sampling scales into it and clamps where the editor's exact-size textures would.

### Memory

- **No effects**: no slot, no pass, no executor; draw lists have no layers. Group records are 96 bytes per isolation group instead of 32.
- **Slots**: up to eight `Rgba8Unorm` textures of one size (only those a stack uses are created), grown in 256-pixel steps to the largest filtered layer, kept across frames, shrunk after 240 frames that need under a quarter of their area, released after 240 frames without effects. The uniform buffer (768 bytes per pass) and bind groups (by slot triple) are reused.
- **Atlases**: pages grow for layers larger than the frame and return to the frame's size when it changes, or after 240 frames whose layers all fit it.
- `Renderer::stats` reports `effect_layers`, `effect_passes`, `effect_bytes` (slots) and `layer_bytes` (atlases); `styk gpubench <file> <tick> [--no-effects]` times a frame with and without effects.

### Differences from the JavaScript, on purpose or known

- The editor rasterizes every translucent figure and every entry through its raster cache at bucketed densities; the engine does so only for entries (where effect kernels depend on the density) and draws other artwork at screen density, as before. Edges differ by a pixel.
- The editor lowers all raster densities when a scene's rasters exceed its preview budget (`previewRasterScale`); the engine does not, so effects of such scenes are sharper there.
- A foreign object without any primitive does not split a connected run in the engine (the editor's layout lists it); such runs filter together.
- A drawing cel inside an effect layer is sampled as the cel display samples it (one display level, bilinear); the editor paints it into the effect raster through Canvas 2D with high-quality smoothing. Thin strokes differ, and a threshold (bloom) amplifies it.
- Effects of the editor's Canvas 2D fallback run on its WebGL 2 or CPU executor with the same kernels; on devices without WebGPU the engine draws like WebGPU devices.

### Adding an effect type to the renderer

After its evaluation (see Changing it): add its passes to `Recorder::effect` in `effects/executor.rs`, as `VfxWebGPU.run` records them; a new operation goes into `passes.wgsl` as in the editor's shader (one more `if (operation == N)`); its padding and sampling padding come from `vfx::padding`. Add it to a document of `engine/tools/vfx-parity-docs.mjs` with non-default parameters and compare in the app.

### Testing the rendering

```sh
cd engine && cargo test -p styk-render -p styk-eval --test vfx_layers   # planning, records, pass recording, layer structure
target/debug/styk layers <file> <tick>          # a frame's layers; compare with:
node tools/vfx-layer-bounds.mjs <project.json> <tick>
target/debug/styk render <file> <tick> out.png [--zoom Z] [--neutral] [--after T]   # --after: the renderer drew tick T first
node engine/tools/vfx-parity-docs.mjs           # tmp/vfx-parity/*.json and their ticks
node scripts/engine-app-parity.mjs --target electron --file tmp/vfx-parity/<doc>.json --ticks <ticks> --zoom 1,2
node scripts/engine-app-parity.mjs --target web --file tmp/vfx-parity/<doc>.json --ticks <ticks> --zoom 1,2
# --no-effects and --no-onion tell whether a difference belongs to effects or to onion skins
```

### Measured when this landed

Machine: Linux, NVIDIA RTX 4070 SUPER, shared with other work (load average 4 to 12 during the runs), 2026-10-02 and 03. Pictures are compared by `scripts/engine-app-parity.mjs` with its limits unchanged (a step fails on any pixel more than 32 levels off together with its eight neighbours, or a mean above 0.25 levels not counting differences of 2 or less; [D13](decisions/0013-app-parity-rule.md)), in a 1,440×900 window, onion skins on (the app's default), at the fitted view and four zoom-in steps (2.07×).

**Editor-built documents** (`engine/tools/vfx-parity-docs.mjs`). Each cell: steps passing, largest mean, largest single difference, solid pixels.

| Document | Electron: engine (WebGPU) against the WebGPU renderer, fit | zoomed | Web: engine (WebGL2) against Canvas 2D, fit | zoomed |
| --- | --- | --- | --- | --- |
| `figures-light`: tint (gradient), glow, outline (outer, centred only), shadow, bloom; one figure translucent | 1/1, 0.036, 39, 0 | 1/1, 0.040, 33, 0 | 1/1, 0.046, 58, 0 | 1/1, 0.048, 46, 0 |
| `figures-blur`: blur in its four modes (radius 7 and 80), grade | 1/1, 0.006, 22, 0 | 1/1, 0.001, 19, 0 | 1/1, 0.020, 58, 0 | 1/1, 0.004, 19, 0 |
| `figures-texture`: pixelate, noise then tint, dissolve, RGB split, wave at partial strength, turbulence then light sweep | 1/1, 0.028, 113, 0 | 1/1, 0.032, 26, 0 | 0/1, 0.064, 45, 2 | 1/1, 0.061, 59, 0 |
| `figures-inner`: inner glow, shadow and outline, two-effect stacks at partial strength, bloom then grade | 1/1, 0.019, 28, 0 | 1/1, 0.019, 26, 0 | 1/1, 0.045, 44, 0 | 1/1, 0.043, 57, 0 |
| `targets`: nodes, figures, a connected group inside another, clips with fades and strength (four ticks) | 4/4, 0.023, 51, 0 | 4/4, 0.020, 41, 0 | 4/4, 0.026, 42, 0 | 4/4, 0.027, 47, 0 |
| `movieclip`: a clip with its own scene stack and effects inside, instanced with a stack (two ticks) | 2/2, 0.053, 63, 0 | 2/2, 0.075, 65, 0 | 2/2, 0.070, 71, 0 | 2/2, 0.060, 53, 0 |
| `scene`: grade, vignette, shockwave, blur on the scene, a glow inside it | 1/1, 0.002, 9, 0 | 1/1, 0.000, 1, 0 | 1/1, 0.001, 8, 0 | 1/1, 0.000, 2, 0 |
| `scene-camera`: CRT, then RGB split from a clip, with a moved, scaled and rotated camera (three ticks) | 3/3, 0.010, 20, 0 | 3/3, 0.020, 20, 0 | 0/3, 1.846, 49, 14,548 | 0/3, 1.185, 53, 9,275 |

Electron: 30 of 30 steps. Web: 23 of 30, and the seven are the reference's, not the engine's:

- `scene-camera` on the web: the Canvas 2D fallback clears the camera rectangle before drawing the camera image, the WebGPU renderer draws the image over the unfiltered frame. Where CRT curvature leaves the image transparent the fallback shows the artboard and WebGPU (and the engine) the unfiltered frame. See the behaviours below.
- `figures-texture` at the fitted view on the web: two pixelate cells along a figure's edge, which pixelate enlarges from one antialiased pixel each (Canvas 2D and the engine cover edge pixels differently).

Playback through the engine's prepared frames was started and stopped on `targets` (web); the script compares the stage after stopping, not the frames shown while playing, which no step captures. The Electron numbers were measured before the onion-skin fix below and before the last merge of `engine-rust`; the web numbers are the same to the last digit before and after both, and after the merge the native renders of the eight documents (fit, 2.5× and through the camera) are byte for byte the same, so Electron was not measured again.

**`TDD2 URIEL NEMESIS 3.styk`** (40 effect clips, drawing layers, a nested explosion generator, 3D surfaces), Electron, ticks 41 to 112 and 128 to 164, after twenty warm-up steps on ticks 0 to 19. The gates for 3D surfaces and generated effects were disabled in a local change that is not committed, so the engine drew every tick without its 3D surfaces and without the generator. 103 of the 109 ticks pass. Mean per tick: 42 to 45 0.01 to 0.09; 47 to 60 0.09 to 0.21; 65 to 92 0.03 to 0.05; 93 to 112 and 128 to 164 0.00 to 0.02; largest single difference among passing ticks 110, no solid pixel. The six:

| Tick | Result | What differs |
| --- | --- | --- |
| 41 | mean 3.58, 9,363 solid | The reference had not shown the drawing cel that first appears at this tick when the step was captured; the engine had. Captured after tick 42 instead, in an earlier run, the tick passed (mean 0.08). |
| 46 | mean 0.96, 19 solid | A drawing layer's onion skin (the layer's last drawing, shown past its end) is missing in the engine. The same with effects off (mean 0.45): drawing-layer onion skins, not effects. |
| 61 to 64 | mean 0.21 to 0.35, 4 to 26 solid | A drawing cel under blur, bloom and glow. Same shapes; the streaks are thinner in the engine. Inside an effect raster the editor paints cels through Canvas 2D with high-quality smoothing, the engine with the cel display's bilinear filter, and the bloom threshold amplifies the difference. With effects off the ticks pass (largest difference 23). |

Found on the way and fixed: onion skins drew solid paints in the paint's colour instead of the skin's tint (ticks 47 and 56 to 60 failed until then).

**Cost.** `styk gpubench`: one frame at 200 zoom levels from the fitted view to 11×, 1,440×900, native Vulkan, submit and wait. Two runs; the first under a load average near 5, the second near 11.

| Frame | p50 with effects | p50 without (`--no-effects`) | Effect passes | Atlases and slots with effects | without |
| --- | --- | --- | --- | --- | --- |
| `figures-light` | 1.15 and 1.41 ms | 0.12 and 0.13 ms | 30 to 58 | 12 to 34 MiB | 0 |
| `figures-blur` | 0.62 and 0.56 ms | 0.14 and 0.16 ms | 8 to 18 | 17 to 79 MiB | 0 |
| `targets`, tick 12 | 0.78 and 1.04 ms | 0.22 and 0.25 ms | 26 to 50 | 22 to 56 MiB | 20 MiB |
| `movieclip` | 1.25 and 1.67 ms | 0.19 and 0.23 ms | 25 to 34 | 29 to 112 MiB | 10 MiB |
| `scene` | 1.08 and 1.61 ms | 0.14 and 0.18 ms | 25 to 29 | 70 to 140 MiB | 0 |
| `scene-camera`, tick 4 | 0.59 and 1.06 ms | 0.11 and 0.26 ms | 3 | 60 to 132 MiB | 0 |
| URIEL, tick 57 | 1.93 and 6.96 ms | 0.25 and 0.33 ms | 31 to 39 | 56 to 160 MiB | 20 MiB |
| URIEL, tick 60 | 1.08 and 1.51 ms | 0.29 and 0.40 ms | 10 to 12 | 70 to 108 MiB | 20 MiB |
| URIEL, tick 140 | 0.36 and 0.37 ms | 0.22 and 0.25 ms | 1 | 32 to 36 MiB | 10 MiB |

Documents without effects, against the renderer before this work (`engine-rust` at 66686509, same command): `hitoshi vs suzu tomiee ssj.piv` tick 129 p50 0.36 to 0.44 ms against 0.41 to 0.54 ms; `svr_uriel.piv` tick 100 0.61 to 0.91 ms against 0.66 to 1.02 ms, with the same atlas memory. The no-effect path is not slower within this machine's noise. Not measured: frame times in the browser, other GPUs, integrated graphics.

The memory is bounded but the bound is large: a layer is at most 3,968 pixels on a side (60 MiB), an atlas has two to eight pages of the largest layer, and a stack uses up to eight slots of it. The largest measured total is 160 MiB.

### JavaScript behaviours reproduced that look wrong (rendering)

- With a scene stack, onion skins are drawn over the frame instead of behind the current pose (`ViewportVfx` paints them after the camera image), so the whole current frame takes their tint.
- The WebGPU renderer draws the camera image over the unfiltered frame. Where the scene stack leaves the image transparent (CRT curvature corners, a dissolve) the unfiltered frame shows through; the Canvas 2D fallback clears the camera rectangle first and shows the artboard. The two JavaScript renderers disagree; the engine follows WebGPU.
- The camera image is stretched by up to a pixel onto the camera rectangle where its whole-pixel size rounds the two axes differently.

### Open

- Generated effects (particles, beam, explosion, smoke) are not drawn by the engine: the gate keeps their documents on the JavaScript renderer.
- Drawing cels inside effect layers are resampled bilinearly, not with Canvas 2D's high-quality smoothing (URIEL ticks 61 to 64).
- The CPU rasterizer (`styk-raster`) does not filter and does not know the camera image group: for a frame with a scene stack it would draw the frame twice.
- `previewRasterScale` (the editor lowering all raster densities under memory pressure) is not followed.
- Not caused by effects, seen while measuring: the stage uploads decoded images only when it draws (8 MiB per frame) and nothing asks for another frame while images are pending, so a still frame can lack a drawing cel until the next redraw (headless: 3,606 images pending at the first capture, the cel appears once none is and the frame is drawn again); a drawing layer's onion skin past its last drawing is not drawn (tick 46).

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
