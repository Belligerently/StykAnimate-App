# D12: A mirror of the document in the engine until the engine owns it

Status: accepted. Recorded in [`docs/RUST-ENGINE-PORT.md`](../../RUST-ENGINE-PORT.md) section 4.

## Context

Commands still run in JavaScript on the editor's document. The engine must evaluate and draw the same document without whole-document transfers on interactive paths.

## Decision

`src/engine/engine-mirror.mjs` sends what changed, record by record: properties as JSON, figures, backgrounds, keyframes and pose sources as lifted records (skeleton JSON plus binary sections), table order as "which old record sits where". Changes are staged per batch and committed atomically by revision (`styk_engine::mirror`); an invalid batch changes nothing. Definitions sync before a frame is drawn; frames sync in slices in the background. An edit frame crosses as the pose on screen, with `null` for every object whose geometry the editor did not rebuild; onion ghosts are poses kept by an id.

## Evidence

Frame sync after a commit: about 10 ms; the edit view of the demo draws with 3.4–4.4 thousand edge pixels different from the Canvas 2D renderer.

## Consequences

Until S5 the document exists twice: the editor's JavaScript objects and the engine's store. Its memory cost is measured in the plan (section 6). It disappears when the engine owns the document and the editor becomes a facade.
