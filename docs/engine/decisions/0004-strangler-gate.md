# D4: The engine replaces the JavaScript engine document by document, feature by feature

Status: accepted. Recorded in [`docs/RUST-ENGINE-PORT.md`](../../RUST-ENGINE-PORT.md) section 4.

## Context

The app must work at every commit while subsystems move to Rust one at a time.

## Decision

`src/engine/engine-stage.mjs` wraps the JavaScript renderer behind the same interface. `src/engine/engine-eligibility.mjs` names the first feature of a document the engine lacks; such a document keeps the JavaScript renderer whole. The gate also refuses single frames the engine cannot take yet (a drawing with unpublished strokes, a drawing preview). It shrinks as features land and disappears with the JavaScript renderer. Editing will be gated per command group (S5).

## Evidence

The start-up document, the repository's test projects and asset packs, the IK fixtures and the large Pivot files are drawn by the engine and compared with the JavaScript renderers (`scripts/engine-app-parity.mjs`, plan section 7).

## Consequences

Two renderers coexist until S3 ends; a document switching between them shows the same picture (parity), so switching is invisible.
