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

## Ordinary-pose diagnostics before the next full figures gate

Four clean paired first-five-interaction runs use the same captured six-figure document and exact math. Each adopts 31 calls, compares 15, and matches document, history, selection and full undo with zero errors, mismatches or rounded comparisons. All three core methods adopt; per-method counts and failed-attempt accounting match. These are loaded diagnostics on one document, not the full class cost gate or engine-1.

| Shipping Wasm | JavaScript ms | Candidate total ms |
| --- | ---: | ---: |
| B2 `36ed40b529d2` | 17.005 | 22.800 |
| Scoped geometry/view `24e199e815ba` | 17.205 | 22.760 |
| Shared history `d0aebc42737d` | 17.170 | 22.380 |
| Typed views/captures/membership `ac4ddf356bac` | 17.075 | 22.925 |

No causal speedup is established. The final package is 10,222,396 bytes with the required Wasm profile. Its 24 ordinary poses spend 5.720 ms in the native call, 0.835 ms parsing and 8.835 ms adopting the answer. The final run retains one session and ends with ten history entries before the full undo comparison. All captured production sources, package files, inputs and HEAD remain stable per run. The existing snapshot excludes the untracked test-only history alias file; its exact source is separately verified by the native freeze. The original six full cost rows remain unchanged and the latest three still fail.

The bounded native changes preserve unchanged parsed views and raw absent selection metadata, evaluate an unpinned limb with its join ancestors, and share private history heads until a write. A recorded clip-parent gesture matched the old engine before the scoped change; selected and parent geometry, full/compact answers and every recorded step remain exact. No public crate API or response schema changes.

A separate 100-microsecond CPU profile retains exact parity but all its times are instrumented diagnostics. A private named binding has identical executable sections to shipping Wasm and maps its samples without guessed function offsets. Rendering and mirror samples are excluded from edit-session attribution. The small ordinary-pose sample count does not establish a dominant cost owner. Fresh full paired cost, a matching committed-HEAD full checkpoint and actual figures default adoption remain required.

Evidence: `ordinaryCoreDiagnostic` in the companion JSON; `/tmp/styk-figures-ordinary-core-b4-comparison-20261006`; `tmp/root-verification-temp/figures-cost-batch3-*` and `figures-cost-batch4-*`; the final native/view/IK/history freezes and independent reviews.

## Figures: completed committed B4 full cost

The full unchanged warmed exact-math paired workload completed on committed `3e0fe620` with shipping Wasm `d0aebc42737d` (10,222,396 bytes, required unstripped `wasm` profile). Startup, rig and joined-chain pass strict document/history/selection/full undo, eligible per-method count matching and complete failed-attempt accounting, with zero errors, mismatches or rounded comparisons. All three core methods show actual positive native and adoption work. Source/package/input maps remain exact before and after each run: 1,529 sources, seven package files and three documents; Node 24.11.0 and Python 3.12.9.

| Document | JavaScript ms | Adopted ms | JS fallback ms | Failed attempt ms | Candidate total ms | Cost gate |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| Startup | 224.715 | 245.000 | 37.185 | 1.360 | 283.545 | Fail |
| Rig | 137.570 | 150.505 | 15.455 | 0.220 | 166.180 | Fail |
| Joined-chain | 147.620 | 159.255 | 10.395 | 0 | 169.650 | Fail |

Owned/compared/failed calls are 163/285/1, 119/170/1 and 125/213/0. The failed eligible attempts are `updateBuilderSegmentStyle` for startup and `pasteSegmentValues` for rig; both durations are included. Rig retains four comparison-only refusal calls, outside eligible owned-command timing. Counts for every eligible timed method match the paired editor. The original six full cost rows and three ordinary diagnostics stay unchanged; full cost rows advance **6→9**, with all three new rows failing. Figures remains closed. Different loaded rounds establish no causal speedup; visible JS history does not measure Native history depth. Load averages range from 1.94 to 15.41.

The matching committed `3e0fe620` checkpoint passes **805 native tests**, with two existing ignored diagnostics, **64 stages** and **358 containers**. Three preceding Chromium attempts ended before a complete cost comparison when the per-user temporary quota was exhausted. They are incomplete measurements, not cost or gate outcomes, and do not establish a Native memory leak. The corrective relocation preserves all 6,194 proof paths (6,191 regular-file hashes and three symlink targets), with the old path retained as a symlink and approximately 2.21 GiB temporary headroom restored; no source/package/HEAD or cache-policy change occurred.

Next bounded Native cache/same-call IK and JS unchanged-layer publication candidates remain **PENDING** root integration, focused checks and fresh full cost. No completed patch milestone, public crate API/schema change, ownership flip or engine-1 result is claimed by this evidence addition. Counters remain **175 js / 123 partial / 35 ported / 22 verified / 1 removed**, **two owned classes**, **two deleted JS modules**, **engine-1 0/28**. Actual default figures adoption remains pending.

Evidence: companion JSON B4 rows and batch; `/tmp/styk-figures-gate-execution-20261004/figures-candidate-batch4-quota-retry-1`; independent accounting review `fe9c9a3193cb`; `tmp/root-verification-temp/checkpoint-3e0fe620-completion.json`; quota relocation receipt and independent review. These are loaded class-gate measurements, separate from baseline-0 and idle engine-1.

## B5 figure cost-owner verification and fourth ordinary diagnostic

The existing typed-frame Cache/Loader, same-call IK evaluation/capture and layer publication/index owners now avoid their recorded redundant work. Same-call capture still recomputes each updated controller spec; 2D/3D baselines, joined ancestors, malformed/refused paths, membership/source GC and history remain exact. No public crate API/schema/arithmetic or gate/threshold change.

Native final twelve-path source and independent review verify199 owning tests,30 CLI tapes/743 calls and two byte-stable edit regeneration rounds (all1,334 prior/1,338 final files). Two actual-editor double-capture tapes matched58/58 OLD engine calls before source changes; final full/compact116-step, numeric-bit and drag controls pass. A preserved57/58 test-only projection failure is corrected with existing depth4 replay projection; the cached69 lane locale-context check failure remains failed. Composed-main nine suites pass147/147, and check passes2,000 JS,93 stylesheets/682 modules and8,066 message references after25 standard generated location-only updates; catalog/preload/registry bytes remain unchanged.

Shipping `ac4ddf356bac86f35dab233badd85558897bc8e93549870032f63272341daf84` is10,227,868B, required unstripped wasm profile, source-stable58.89s build. Codec831eb67ec33f/1,532,303B and drawing69df6ea195e7/449,096B are unchanged. The actual94.55s ordinary paired app uses the same46de1ab70d07... warm input and unchanged first-five driver:31owned/15compared/46calls, one session, ten history entries, core24pose/2move/3commit all positive and matching editor counts. Strict whole document/history/selection/full undo, zero errors/mismatches/rounding, complete attempt accounting and source/package/HEAD/input stability pass. Approved runtime HEAD is3e with the captured uncommitted B5 source; its source/build receipts, not an older committed binary, identify this result.

Candidate22.92500001192093ms versus JS17.075000017881393ms remains above JS. Owned time includes Native/parse/adoption; fallback and failed-owned times are both zero. Pose24 calls spend5.930000066757202ms Native,0.8500000238418579ms parse/acceptance residual and9.254999935626984ms adoption. Move2 and commit3 also retain positive Native/adoption values. Load averages before/after are0.927/1.880/3.717 and7.776/4.177/4.371. These are loaded observations, not a causal speedup, class-cost certificate or engine-1. Ordinary diagnostic rows advance3→4; all nine full class-cost rows remain exact, latest B4 three FAIL.

Counters remain175js/123partial/35ported/22verified/1removed, OWNED_CLASSES2→2, deletedJS2→2, engine-1 0/28→0/28. Next root commits the verified source/measurement milestone, then runs its matching checkpoint and unchanged full startup/rig/joined cost; actual default figures core adoption must pass before any gate commit. Evidence: companion JSON fourth ordinary row/context; `b5-shipping-1/completion.json`2487cb979b78, driver8041d2505955; main JS result8ac3a6f480c1 and shipping receipt6436c52388a6. Earlier measurements and incomplete quota attempts retain their original scope.
