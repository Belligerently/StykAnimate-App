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

Counters remain175js/123partial/35ported/22verified/1removed, OWNED_CLASSES2→2, deletedJS2→2, engine-1 0/28→0/28. The committed B5 full cost is recorded below; the shorter ordinary diagnostic does not certify the class gate. The next scoped candidate still needs its own owning verification, shipping, fresh paired cost and matching checkpoint/default figures admission. Evidence: companion JSON fourth ordinary row/context; `b5-shipping-1/completion.json`2487cb979b78, driver8041d2505955; main JS result8ac3a6f480c1 and shipping receipt6436c52388a6. Earlier measurements and incomplete quota attempts retain their original scope.

The full warmed exact-math paired workload then completed on committed `b4ccf993` and the same shipping `ac4ddf356bac` package. Strict document/history/selection/full undo, matching eligible calls, complete failed-attempt accounting and positive native/adoption work on each core method pass on all three inputs. Source/runtime/package/input guards remain exact throughout (1,529 sources, seven packages, three documents; Node 24.11.0/Python 3.12.9).

| Document | JavaScript ms | Adopted ms | JS fallback ms | Failed attempt ms | Candidate total ms | Cost gate |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| Startup | 220.625 | 197.380 | 41.670 | 0.920 | 239.970 | Fail |
| Rig | 132.530 | 267.845 | 24.560 | 0.400 | 292.805 | Fail |
| Joined-chain | 277.010 | 310.425 | 22.195 | 0 | 332.620 | Fail |

Owned/compared/failed counts are 163/285/1, 119/171/1 and 125/213/0. Rig has four permitted comparison-only residual calls. Startup's rejected `updateBuilderSegmentStyle` and rig's rejected `pasteSegmentValues` attempts are included before JavaScript fallback. Full rows advance **9→12**; the nine previous full rows and four ordinary diagnostics remain unchanged. Rig and joined-chain overlap checkpoint compilation; these loaded measurements establish no causal speedup or regression, idle engine-1 result, new checkpoint or default figures admission. Figures remains closed.

All four port counters stay unchanged: **175 js / 123 partial / 35 ported / 22 verified / 1 removed**, **two owned classes**, **two deleted JavaScript modules**, **engine-1 0/28**. No B6 verification, API/schema/gate/history/threshold change is claimed. Next only the scoped measured figures candidate and its required verification may advance this gate. Evidence: companion JSON B5 rows/batch; `figures-candidate-batch5-1/summary-accounted.json` `54815043`, approved reader output `7c798da5`, and independent completed-cost review `b1d35c6f`.

The next scoped candidate removes repeated same-call IK evaluation for settings and active free/joined targets. One whole editing-crate round passes201tests;21unchanged tapes match565steps;56focused mainJS tests and check pass. Private count evidence removes one evaluation in22free/16joined active calls while34one-shot/pinned controls keep their counts; joined solver counts remain exact. Three real old/six final guards and six current-pose samples pass; the original synthetic missing-definition geometry panic remains preserved and outside that coverage. Shipping `ffedaa2273a9` /10,231,142B retains the required wasm profile and stable sources. No public crate API/stored-arithmetic change or recording regeneration. These checks alone do not open figures. The completed B6 full cost follows; a measured cost fix, matching final checkpoint and positive actual default core adoption remain required. Native evidence: source manifest `3cf36ef2`, complete79-artifact inventory `e5adaa85`, cumulative patch `9e223352`, runtime completion and201/565 logs.

## B6 completed committed full cost

The unchanged warmed exact-math full paired workload completed on committed `05f137ac562b5bb48431156dffcd3a9fafc2e6e8` with shipping Wasm `ffedaa2273a9c957340999b6dafbf4c86b070ea0df8f0a1d480662307613b40d`, 10,231,142 bytes, required unstripped `wasm` profile. All three inputs pass strict document/history/selection/full undo, eligible call matching, positive actual native/adoption work on each core method and complete failed-attempt accounting. All 1,529 source/seven package/three input maps match before and after; Node 24.11.0/Python 3.12.9 and the full driver workload are unchanged.

| Document | JavaScript ms | Adopted ms | JS fallback ms | Failed attempt ms | Candidate total ms | Cost gate |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| Startup | 215.525 | 200.025 | 36.680 | 5.755 | 242.460 | Fail |
| Rig | 127.235 | 129.670 | 11.965 | 0.160 | 141.795 | Fail |
| Joined-chain | 142.555 | 159.345 | 10.240 | 0 | 169.585 | Fail |

Owned/compared/failed counts are 163/289/2, 119/171/1 and 125/213/0. Only rig has four permitted comparison-only residual calls. Startup’s failed `commitDrag` and `updateBuilderSegmentStyle` attempts and rig’s failed `pasteSegmentValues` attempt are included before JS fallback. Core pose/move/commit counts are 48/6/8, 36/6/4 and 36/6/4; eligible family/call counts are 76/171, 65/128 and 66/129, each matching its paired baseline. Startup history has 80 entries; rig/joined have 79, all with exact full undo and zero errors/mismatches/rounding/broken state.

Heavy jobs finished before this run and no cargo/rustc is present in the boundary process receipts. Nonzero load remains recorded; this is loaded app evidence, not idle engine-1 or causal speed attribution. The earlier empty `batch6-1` hash-guard refusal ran no app and measured no cost; it and all failed/incomplete evidence remain unchanged. The approved reader returns COST_FAIL on all three cases using the unchanged exact <= cost rule; its ULP rule applies only to failed-duration accumulation.

Full cost rows advance **12→15**. All twelve historical rows, three older batch metadata objects, four ordinary diagnostics and other historical JSON fields remain byte-exact. Counters remain **175 js / 123 partial / 35 ported / 22 verified / 1 removed**, **two owned classes**, **two deleted JavaScript modules**, **engine-1 0/28**. Figures remains closed. This B6 cost evidence makes no B7 verification/shipping/runtime/speed, ownership flip, new checkpoint, API/schema/history/threshold or engine-1 claim.

Evidence: companion JSON B6 rows/batch; `figures-candidate-batch6-head-checked-1/summary-accounted.json` `42a0d1ba`, approved input receipt `9100b089`, reader source `e18078fb`, reader output `20413996`, and independent full review `c7723030`. Next only the measured figures cost blocker and the required matching checkpoint/default admission may advance this gate.


## B7 joint-pose candidate, focused verification

The packed-angle update retains sequential duplicate arithmetic and final packing. The shape-angle query retains all controllers and conflicts in the selected joined component, its local fallback and the unchanged global owner for duplicate identities. Only unrelated second-query solves and post-solve reporting are omitted. Benefit depends on the actual provided frame; no current-frame pin presence or app speed is assumed.

One unfiltered owning-crate round passes 207 tests, including all six new numeric and solver controls. The fresh CLI matches 32 existing tapes / 1,519 steps. All 3,670 engine fixture JSON remain unchanged. Main passes 56 focused JavaScript tests and check; shipping `e95371c6c75e` is 10,234,289 bytes, source-stable and retains the required profile. No public crate API/schema or stored-arithmetic change. Native completion `f3307237`, independent runtime review `98e8b0d7` and composition review `dbaa1d11` bind this candidate; B6's fifteen full rows and four ordinary diagnostics remain unchanged. The completed B7 full app cost below fails all three cases; matching final checkpoint and default core adoption remain pending.

## B7 completed committed full cost

The unchanged warmed exact-math full paired workload completed on committed `b748119b5047c21e96cc8677f5caf2b731e69b03` with shipping Wasm `e95371c6c75e708aaf91618332282f18924cf4a38beb5cb293bbb8d06728d111`,10,234,289 bytes, required unstripped `wasm` profile. Strict document/history/selection/full undo, matched eligible calls, complete failed-attempt accounting and positive actual native/adoption work on each core method pass on all three inputs. All1,529source/sevenpackage/threeinput maps match before and after; Node24.11.0/Python3.12.9 and the full driver workload are unchanged.

| Document | JavaScript ms | Adopted ms | JS fallback ms | Failed attempt ms | Candidate total ms | Cost gate |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| Startup | 224.850 | 190.170 | 39.845 | 4.375 | 234.390 | Fail |
| Rig | 133.930 | 122.865 | 12.005 | 0.190 | 135.060 | Fail |
| Joined-chain | 147.280 | 143.125 | 9.985 | 0 | 153.110 | Fail |

Owned/compared/failed counts are162/286/2,119/171/1 and125/213/0. Core pose/move/commit counts are48/6/8,36/6/4 and36/6/4. Eligible method/call counts76/171,65/128 and66/129 match the paired editor. Startup history has80entries, rig/joined79, all with exact full undo and zero errors/mismatches/rounding/broken state. Startup’s failed `commitDrag`3.2100000083446503ms and `updateBuilderSegmentStyle`1.1649999916553497ms attempts and rig’s failed `pasteSegmentValues`.1899999976158142ms attempt are included once before fallback. Actual argument subtypes are unavailable; cumulative warm reason maps do not establish them. Rig retains four allowed comparison-only residual calls on both pages, outside eligible owned timing and without a paired method-count gap.

Full cost rows advance **15→18**, all three new rows FAIL under literal candidate owned+fallback+failed <= editor-alone cost. All fifteen historical rows, four older batch metadata objects, four ordinary diagnostics and other historical JSON fields remain byte-exact. Nonzero load is recorded (1.525–10.603 across the run boundaries); these are loaded app observations, not an idle engine-1 result or causal speed comparison. No gate, threshold, history, public crate API, response schema or stored-arithmetic change follows.

Counters remain **175 js /123 partial /35 ported /22 verified /1 removed**, **two owned classes**, **two deleted JavaScript modules**, **engine-1 0/28**. Figures remains closed. B8 owning verification and shipping build are complete as recorded in the plan; full B8 cost, matching checkpoint and default admission remain pending. This B7 measurement append makes no B8 speed claim. A final candidate must pass the unchanged full cost rule, its matching committed-source checkpoint and actual default core adoption before the class opens.

Evidence: companion JSON B7 rows/batch; `figures-candidate-batch7-1/summary-accounted.json`9cb5a93d, approved input receipt3e0222e6, approved reader output4b33c96e and independent complete review78164101. Exact artifact/package identities, double-precision values, commands and loads remain in those receipts. The old milestone history and four ordinary diagnostics keep their original scope.


## B8 completed committed full cost

The unchanged warmed exact-math full paired workload completed on committed `c0048287efb998c1fc3cdcb36ff111c87baa5b79` with shipping Wasm `8d47d635183e29b4a401f136d5544d88a8074864462e09e6c9e369f03c2454d9`, 10,238,324 bytes and the required unstripped `wasm` profile. All three inputs pass strict document/history/selection/full undo, paired eligible-call counts, complete failed-attempt accounting and positive actual native/adoption work on each core method. All 1,529 source, seven package and three input maps match before and after; Node 24.11.0, Python 3.12.9 and the full driver workload are unchanged.

| Document | JavaScript ms | Adopted ms | JS fallback ms | Failed attempt ms | Candidate total ms | Cost gate |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| Startup | 191.290 | 186.300 | 34.585 | 0.000 | 220.885 | Fail |
| Rig | 121.675 | 128.405 | 13.275 | 0.185 | 141.865 | Fail |
| Joined-chain | 144.420 | 155.725 | 12.260 | 0.000 | 167.985 | Fail |

Owned/compared/failed counts are **163/289/0**, **119/171/1** and **125/213/0**. Core pose/move/commit counts are **48/6/8**, **36/6/4** and **36/6/4**; eligible method/call counts **76/171**, **65/128** and **66/129** match the paired editor. Startup history has 80 entries and rig/joined 79, with exact full undo and zero differences, mismatches, rounding or page problems. Rig's failed `pasteSegmentValues` attempt, **0.1850000023841858 ms**, is included once before fallback; no argument subtype is inferred. Rig retains the same four allowed comparison-only residual calls on both pages, outside eligible timing with no method-count gap.

Full cost rows advance **18→21**; all three new rows FAIL the literal candidate owned+fallback+failed <= editor-alone rule. All eighteen historical rows, five older batch metadata objects, four ordinary diagnostics and other historical JSON fields remain byte-exact. These loaded observations establish no causal speedup or idle engine-1 result. Method clocks remain diagnostic: each `engineMs` includes its native `callMs`; it is not added a second time to the official total.

Counters remain **175 js / 123 partial / 35 ported / 22 verified / 1 removed**, **two owned classes**, **two deleted JavaScript modules**, **engine-1 0/28**. Figures remains closed. Only a measured cost fix, its fresh unchanged full cost proof, matching committed-source checkpoint and positive actual default core adoption can advance the gate. This measurement append introduces no implementation, gate, public API, response schema, stored-arithmetic or threshold change. This completed append accompanies the verified B9 source fix recorded in the port plan.

Evidence: companion JSON B8 rows/batch; `figures-candidate-batch8-1/summary-accounted.json` `497e06af`, approved input receipt `112cd258`, approved reader output `e2c0c183`, and private raw-accounting receipt `a5858260`. Exact double-precision values, artifact/package identities, commands and loads remain in those receipts. Historical milestone text and all four ordinary diagnostics retain their original scope.


## B9 completed committed full cost

The unchanged warmed exact-math full paired workload completed on committed `75bb37f89ee491825f1a7c5284639c534120c385` with shipping Wasm `b9cf70669501635cf51b9bc313f94fffef0c35aac05cb395d038e0bc5308f31f`, 10,240,701 bytes and the required unstripped `wasm` profile. All three inputs pass strict document/history/selection/full undo, paired eligible-call counts, complete failed-attempt accounting and positive actual native/adoption work on each core method. All 1,529 source, seven package and three input maps match before and after; Node 24.11.0, Python 3.12.9 and the full driver workload are unchanged.

| Document | JavaScript ms | Adopted ms | JS fallback ms | Failed attempt ms | Candidate total ms | Cost gate |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| Startup | 229.515 | 199.680 | 37.775 | 0.000 | 237.455 | Fail |
| Rig | 141.250 | 121.825 | 14.710 | 0.200 | 136.735 | Pass |
| Joined-chain | 138.235 | 145.340 | 11.280 | 0.000 | 156.620 | Fail |

Owned/compared/failed counts are **163/285/0**, **119/171/1** and **125/213/0**. Core pose/move/commit counts are **48/6/8**, **36/6/4** and **36/6/4**; eligible method/call counts **76/171**, **65/128** and **66/129** match the paired editor. Startup history has 80 entries and rig/joined 79, with exact full undo and zero differences, mismatches, rounding or page problems. Rig's failed `pasteSegmentValues` attempt, **0.19999998807907104 ms**, is included once before fallback; no timed argument subtype is inferred. Rig retains the same four allowed comparison-only residual calls on both pages. Startup's actual compared count is 285 rather than B8's 289; all 171 eligible calls still match, and the raw counts are preserved without inferring a workload reduction.

Full cost rows advance **21→24**. The literal unchanged candidate owned+fallback+failed <= editor-alone rule yields **startup FAIL, rig PASS, joined-chain FAIL**; the complete class gate does not pass. All twenty-one historical rows, six older batch metadata objects, four ordinary diagnostics and other historical JSON fields remain byte-exact. These loaded observations establish no causal speed gain or idle engine-1 result. Method clocks remain diagnostic: each `engineMs` includes its native `callMs`; it is not added again to the official total.

Counters remain **175 js / 123 partial / 35 ported / 22 verified / 1 removed**, **two owned classes**, **two deleted JavaScript modules**, **engine-1 0/28**. Figures remains closed. A measured cost fix, fresh unchanged all-three passing cost, matching committed-source checkpoint and positive actual default core adoption are still required. This completed evidence append introduces no implementation, gate, public API, response schema, stored-arithmetic or threshold change and is reserved for the next meaningful verified figures fix.

Evidence: companion JSON B9 rows/batch; `figures-candidate-batch9-1/summary-accounted.json` `1c68c696`, approved input receipt `2a9ac16d`, independent completed all-three acceptance `ec7a9a86`, and private accounting receipt `80fe0665`. Independent startup `9315c8be` and rig `cfebfa0e` receipts remain unchanged. No additional completed-reader execution was needed. Exact double-precision values, artifact/package identities, commands and loads remain in those receipts; historical milestone text and all four ordinary diagnostics retain their original scope.


## B10 completed committed full cost

The unchanged warmed exact-math full paired workload completed on committed `69bbe7ab03f7d83077302f7b1fce0cd7e724c54d` with shipping Wasm `61537fd7901f5e630cd79870673260b7f27e5fbbd523624f24cdf3712b4960a9`, 10,241,121 bytes and the required unstripped `wasm` profile. All three inputs pass strict document/history/selection/full undo, paired eligible-call counts, complete failed-attempt accounting and positive actual native/adoption work on each core method. All 1,529 source, seven package and three input maps match before and after; Node 24.11.0, Python 3.12.9 and the full driver workload are unchanged. These identities bind the completed B10 runs, not later authorized Native source edits.

| Document | JavaScript ms | Adopted ms | JS fallback ms | Failed attempt ms | Candidate total ms | Cost gate |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| Startup | 214.480 | 200.715 | 39.540 | 0.000 | 240.255 | Fail |
| Rig | 130.040 | 124.180 | 13.875 | 0.240 | 138.295 | Fail |
| Joined-chain | 142.495 | 152.350 | 10.485 | 0.000 | 162.835 | Fail |

Owned/compared/failed counts are **163/289/0**, **119/171/1** and **125/213/0**. Core pose/move/commit counts are **48/6/8**, **36/6/4** and **36/6/4**; eligible method/call counts **76/171**, **65/128** and **66/129** match the paired editor. Startup history has 80 entries and rig/joined 79, with exact full undo and zero differences, mismatches, rounding or page problems. Rig's failed `pasteSegmentValues` Native attempt, **0.24000000953674316 ms**, is included once before its separate original JavaScript fallback interval. No timed argument subtype is inferred. Rig retains the same four allowed comparison-only residual calls on both pages.

Full cost rows advance **24→27**; all three new rows FAIL the literal unchanged candidate owned+fallback+failed <= editor-alone rule. All twenty-four historical rows, seven older batch metadata objects, four ordinary diagnostics and other historical JSON fields remain byte-exact. These loaded observations establish no causal speed gain or idle engine-1 result. Method clocks remain diagnostic: each `engineMs` includes its native `callMs`; it is not added again to the official total.

Counters remain **175 js / 123 partial / 35 ported / 22 verified / 1 removed**, **two owned classes**, **two deleted JavaScript modules**, **engine-1 0/28**. Figures remains closed. A measured cost fix, fresh unchanged all-three passing cost, matching committed-source checkpoint and positive actual default core adoption are still required. This completed evidence append introduces no implementation, gate, public API, response schema, stored-arithmetic or threshold change and is reserved for the next meaningful verified figures fix.

Evidence: companion JSON B10 rows/batch; `figures-candidate-batch10-1/summary-accounted.json` `0e24675c`, approved input receipt `88c76647`, independent completed all-three acceptance `093c0a27`, and the private accounting receipt in the companion metadata. Independent startup `a9545728` and rig `10f75e4b` receipts remain unchanged. No additional completed-reader execution was needed. Exact double-precision values, artifact/package identities, commands and loads remain in those receipts; historical milestone text and all four ordinary diagnostics retain their original scope.


## B11 instrumented full joined-pose diagnostic

The full warmed joined-chain workload completed on committed `ebf5c694` with shipping Wasm `15cbbdc513ea` (10,241,300 bytes), while the full checkpoint ran concurrently. Document, history (79 entries), selection and full undo match; all 36 poses appear in three 12-call ranges on both pages. The instrumented whole-workload clocks are **305.950 ms candidate** (284.765 owned + 21.185 JS fallback + zero failed time) and **280.750 ms JavaScript**. CDP profiling and diagnostic wrappers make these attribution observations, not class-gate acceptance or a speed comparison. Independent acceptance and both profile reviews are complete, with sample limits preserved.

| 36 pose calls | JavaScript ms | Native outer ms | Included Native call ms | Adoption ms |
| --- | ---: | ---: | ---: | ---: |
| Joined-chain | 25.860 | 17.315 | 15.330 | 22.085 |

The Native call is already included in the outer clock. Helper clocks overlap; neither they nor CPU samples are added to official totals. One completed full-workload instrumented joined diagnostic advances **0→1**; the **27 official full cost rows, eight batch objects, four ordinary diagnostics, ownership classes and engine-1 counter remain unchanged**. Figures stays closed. Exact clocks, loads, source bindings and evidence are in the separate `joinedPoseDiagnostic` JSON object.

Evidence: independent acceptance `83a2ca49`, Native profile review `57cfa98f` and JS/adoption review `34bb6172`; bounded raw artifact hashes are in the companion JSON. Two signed negative oracle sample intervals (-1/-2 µs) and boundary-overlapping intervals are excluded from attribution without changing the raw profiles. The launcher's original pending-acceptance flag is preserved; the separate independent receipt accepts this diagnostic only.
