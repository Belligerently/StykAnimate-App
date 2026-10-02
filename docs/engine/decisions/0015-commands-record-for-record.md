# D15: Editing moves to the engine command by command, record for record

Status: accepted, phase 1 in progress. Recorded in [`docs/RUST-ENGINE-PORT.md`](../../RUST-ENGINE-PORT.md) section 4; design in [`../editing.md`](../editing.md).

## Context

The JavaScript `Editor` owns the document and runs about 800 methods across some 80 modules. The engine holds a mirror of the document (D12). Moving every command at once would leave the app broken for weeks and give no way to tell a port from a rewrite.

## Decision

A command moves to `styk-edit` only when it reproduces the editor record for record: from the same document, selection, playhead and arguments it changes the same records into the same bytes, returns the same value, leaves the same selection and history state and emits the same change events. The command-replay harness checks this step by step: `engine/tools/edit-scenarios.mjs` builds scenarios with the editor's own fixture kit, `engine/tools/edit-replay.mjs` records them in the JavaScript editor, and `styk edit-replay` (and `cargo test -p styk-edit`) replays them in Rust. The `Editor` facade keeps its API; a ported method will run in the engine and adopt the changed records and event detail; everything else runs in JavaScript and reaches the engine through the mirror. Calls `styk-edit` cannot run yet answer `Unported`.

## Evidence

Selection, seeking, gestures and moving, rotating, scaling, flipping, centring and resetting figures, with undo and redo, match the editor at every step of their recordings (plan, milestone log). A tampered recording is caught.

## Consequences

Each port is a mechanical comparison rather than a judgement. The editor's arithmetic has to be reproduced bit for bit (D16). Runtime state the document does not carry (layer focus locks) and caches the mirror strips (`runtime_segments`) stay the facade's to account for; `editing.md` lists them.
