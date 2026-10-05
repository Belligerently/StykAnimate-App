# Rust editing class gates

The figures class remains closed: exact app parity passes, but all three latest editing measurements cost more than JavaScript. These loaded measurements unblock work on that gate; they are separate from baseline-0 and the idle engine-1 matrix.

## Figures: recorded clipboard and compact adoption milestone

One fresh paired Chromium run per document uses `scripts/engine-edit-shadow.mjs --own --warm --exact-math --verbose`, adding `--file engine/tests/edit/rig.document.json` or `--file engine/tests/edit/joined-chain.document.json` as applicable. Each page first completes the existing warm workload, then resets timing and performs the timed workload on that resulting document. The startup case is the app's startup document. Selection, history labels, final document and undoing everything match exactly; both pages have zero errors, rounded comparisons or mismatches, and per-method workload counts match. This is eligible owned-command CPU cost, not all application wall time or interaction latency.

The candidate total includes native call/parse/adoption, original JavaScript fallback, and rejected eligible native attempts before fallback. Comparison-only calls remain diagnostics, as in the existing gate protocol. No parity or cost threshold changed.

| Document | JavaScript ms | Adopted ms | JS fallback ms | Failed attempt ms | Candidate total ms | Cost gate |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| Startup | 240.125 | 244.425 | 45.910 | 5.785 | 296.120 | Fail |
| Rig | 162.520 | 147.400 | 14.325 | 0.220 | 161.945 | Pass, narrowly |
| Joined-chain | 149.640 | 156.580 | 11.285 | 0 | 167.865 | Fail |

Owned/compared/failed calls are respectively 163/289/2, 119/171/1 and 125/213/0. All three runs pass strict app parity and complete cost accounting. Rig has four additional comparison-only refusal calls; these are excluded from eligible-owned timing and remain recorded diagnostics.

Source base is `69e0646b` plus the accompanying clipboard/compact adoption changes. Shipping Wasm is `6d128ac283b121e8a17db32b5a9bee2c81208c0130ab7d56c2177eb35e8113b6`, 10,202,308 bytes, profile `wasm`, with the required unstripped profile. Node is 24.11.0 and Python 3.12.9. Source, package, input and runtime hashes remained stable throughout. Load averages ranged from 6.6 to 17.1; these are loaded diagnostic measurements and do not establish idle performance or a speedup against earlier reports with incomplete attempt accounting.

Exact numbers, commands, source identities, load and raw-receipt hashes are in [the evidence JSON](RUST-EDITING-CLASS-GATES.json). The next bounded fixes address response object assembly, the redundant adopted-sheet endpoint scan and ordinary pose-commit preview reconstruction; they require fresh measurements before any gate claim.

Existing default 3D adoption also passes a fresh `ik-spatial-free.document.json --own --default --figures-gate --exact-math --verbose` smoke: mode `classes`, 42 owned and 15 compared calls, nonzero native/adoption work on every core method, exact document/history/selection/full undo, zero errors. This shorter cold workload certifies admission and behavior only.

## Figures: response, publication and committed-preview batch

The next batch assembles response keys once, skips the adopted sheet's no-op endpoint scan and retains the exact ordinary committed preview when its explicit compact delta is empty. Existing publication, history, joined-root reconstruction and cancellation owners remain unchanged. Focused checks pass: 182 `styk-edit` tests, ten affected recordings with 293 exact calls, 191 JavaScript tests and `npm run check` (2,000 files). No new recording or public crate API is introduced.

Three fresh paired runs use the same full warmed exact-math workload and accounting rule above. All pass strict document/history/selection/full-undo parity, matching per-method counts and complete failed-attempt accounting, with zero errors, mismatches or rounded comparisons.

| Document | JavaScript ms | Adopted ms | JS fallback ms | Failed attempt ms | Candidate total ms | Cost gate |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| Startup | 218.420 | 217.455 | 37.535 | 4.950 | 259.940 | Fail |
| Rig | 131.325 | 129.045 | 13.065 | 0.215 | 142.325 | Fail |
| Joined-chain | 150.130 | 156.755 | 8.755 | 0 | 165.510 | Fail |

Owned/compared/failed calls remain 163/289/2, 119/171/1 and 125/213/0. Rig's four additional refusals remain comparison-only diagnostics. Source base is `9d1ffda0` plus these five owning paths; shipping Wasm is `36ed40b529d24d86d427016c3dfa703cf6c4c53bc96e07772de61ce9b252c706`, 10,204,765 bytes, using the required unstripped `wasm` profile. All 1,528 captured production sources, seven package files and three input documents remain unchanged during the run. Node/Python and browser settings match the preceding protocol; load averages range from 2.8 to 13.7. Different loaded rounds establish no causal speedup.

The evidence JSON preserves the original three rows and appends these three with their own source/package/input and raw-receipt hashes. A fresh existing 3D default smoke again adopts 42 and compares 15, with every core method adopted and exact behavior; it does not certify the closed figures class or full editing cost. The matching committed-HEAD class checkpoint and figures default admission remain required.

A separate private full-startup profile preserves exact parity and stable B2 inputs, but every instrumented time is diagnostic only. Its 48 owned poses emit exactly 48 edit events; synchronous event work is shared with the ordinary editor. The warmed active sheet source contains six unjoined figures, three with four enabled free IK targets. The next bounded candidate uses the existing scoped evaluator when posing one free limb, retaining join ancestors and removing unrelated geometry work. Unchanged-view and snapshot-reconciliation candidates remain subject to their focused checks. Native history-clone cost is unproved: the JavaScript history labels are not the native session's history size, and the final Wasm's sampled functions are unnamed.
