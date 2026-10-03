# D15: Editing moves to the engine command by command, record for record

Status: accepted, phase 1 in progress. Recorded in [`docs/RUST-ENGINE-PORT.md`](../../RUST-ENGINE-PORT.md) section 4; design in [`../editing.md`](../editing.md).

## Context

The JavaScript `Editor` owns the document and runs about 800 methods across some 80 modules. The engine holds a mirror of the document (D12). Moving every command at once would leave the app broken for weeks and give no way to tell a port from a rewrite.

## Decision

A command moves to `styk-edit` only when it reproduces the editor record for record: from the same document, selection, playhead and arguments it changes the same records into the same bytes, returns the same value, leaves the same selection and history state and emits the same change events. The command-replay harness checks this step by step: `engine/tools/edit-scenarios.mjs` builds scenarios with the editor's own fixture kit, `engine/tools/edit-replay.mjs` records them in the JavaScript editor, and `styk edit-replay` (and `cargo test -p styk-edit`) replays them in Rust. The `Editor` facade keeps its API; a ported method will run in the engine and adopt the changed records and event detail; everything else runs in JavaScript and reaches the engine through the mirror. Calls `styk-edit` cannot run yet answer `Unported`.

## Evidence

Selection, seeking, gestures and moving, rotating, scaling, flipping, centring and resetting figures, with undo and redo, match the editor at every step of their recordings (plan, milestone log). A tampered recording is caught.

## Revision (2026-10-03): shadow before ownership

The app builds its editor with property channels and the layer timeline, so every
command runs through drafts whose JavaScript state (the preview frame, geometry
caches, layer bookkeeping, history objects) is entangled with rendering. Adopting the
engine's records command by command would mean keeping two copies of that runtime state
equal by hand. Instead:

1. Commands are ported and proven in the Node harness, byte for byte.
2. In the app they run in **shadow**: the JavaScript editor stays in charge, the engine
   runs the same call on the mirrored document, and the two are compared
   (`src/engine/engine-edit-shadow.mjs`, `scripts/engine-edit-shadow.mjs`). No behaviour
   changes, real documents and real gestures are covered, and the boundary's cost is measured.
3. Ownership flips in one step per document once its commands are ported: the engine's
   session becomes the document, and the JavaScript editor a view of it.

Step 3 is implemented as the editor facade adopting the engine's results
(`?engineEdit=on`, [`../editing.md`](../editing.md), "Engine-owned editing in the app"):
for ported calls the engine computes and the editor keeps only its bookkeeping (publishing,
events, history), checked by running the same interactions with and without the engine
owning. It becomes the default for a document class once its cost is not higher than the
editor's. The first class flipped on 2026-10-03: large figure documents (no 3D figures or
movieclips, 1,000 segments or more across the figure definitions), where engine-owned
editing cost 18 to 39 % less than the editor alone in the shadow driver's interactions;
small figure documents stay with the editor (the JSON boundary and adoption cost more than
the editor's own call), as do 3D documents and movieclips for now (editing.md).

## Consequences

Each port is a mechanical comparison rather than a judgement. The editor's arithmetic is
reproduced as Node computes it (D16). Runtime state the document does not carry (layer
focus locks, the timeline's layer selection, counters, auto key) is handed to the engine
with each session and call; `editing.md` lists it. Until the flip nothing the engine
computes for an edit is shown to the user, so a wrong port cannot damage a document.
