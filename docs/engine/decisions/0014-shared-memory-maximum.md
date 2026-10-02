# D14: The shared memory's maximum adapts to what the device can reserve

Status: accepted. Recorded in [`docs/RUST-ENGINE-PORT.md`](../../RUST-ENGINE-PORT.md) section 4.

## Context

A shared WebAssembly memory cannot move when it grows, so browsers reserve its
maximum up front. The engine is linked with a 2 GiB maximum. Mobile WebKit
(iOS, iPadOS) has a smaller address-space budget per process; a page that
cannot reserve 2 GiB must not lose the engine.

## Decision

The page creates the memory itself (`createEngineMemory` in
`src/engine/engine.mjs`) and hands it to the module: 2 GiB, else 1 GiB, else
512 MiB, the first the browser accepts. It starts at the size the module
declares for the memory it imports, read from the module's import section
(`declaredMemoryPages` in `src/engine/engine-memory.mjs`): that size grows with
the module's static data, and a smaller memory does not link, which leaves the
page on the JavaScript renderer with only a console warning. `localStorage['stykanimate.engine.memory']`
(MiB) forces a maximum for testing. A memory of 1 GiB or less budgets the
frame cache like a 2 GiB device (48 MiB). If no maximum can be reserved the
engine does not start and the JavaScript renderer draws, as without the engine
build.

## Evidence

Measured on 2026-10-02 with Playwright's Chromium and WebKit (Linux builds),
cross-origin isolated and not: a shared memory with a 2 GiB, 1 GiB or 512 MiB
maximum is created and grows to 252 MiB in both; the engine loads threaded in
both; with the first two maxima refused (simulated), the 512 MiB memory is
used; the override is honoured. Not measured: real iOS and iPadOS devices,
whose limits differ from desktop WebKit.

## Consequences

The module's declared maximum stays 2 GiB (`scripts/engine-build.mjs`); the
page may choose less. An allocation beyond the chosen maximum traps the
module; recovering from that (leaving the engine for the session without
blocking the interface) is not implemented yet and is listed in the plan's
open problems, as is verification on real Apple devices.
