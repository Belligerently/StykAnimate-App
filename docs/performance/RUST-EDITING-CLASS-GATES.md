# Rust editing class gates

The figures class remains closed: exact app parity passes, but startup and joined-chain editing still cost more than JavaScript. These loaded measurements unblock work on that gate; they are separate from baseline-0 and the idle engine-1 matrix.

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
