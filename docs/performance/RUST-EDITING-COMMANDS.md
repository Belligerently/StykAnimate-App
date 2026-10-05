# Recorded command adoption measurements

These affected-command observations use the original full paired app driver with
`--own --default --warm --exact-math`. Warm-up calls are excluded; Native call time
is part of engine/parse/validation time. Values below are milliseconds rounded to
three decimal places. They do not advance full-class cost rows, establish idle
engine-1 results or prove a causal speed gain. Class policy/history remain in
[RUST-EDITING-CLASS-GATES.md](RUST-EDITING-CLASS-GATES.md).

| Date | Command | Adopted calls | Native call ms | Engine / parse / validation ms | Adoption ms | Owned total ms | Editor-alone ms | Answer JSON length (UTF-16 units) |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 2026-10-05 | `splitSegment` | 1 | 4.670 | 9.585 | 5.750 | 15.335 | 17.085 | 235,775 |

The startup full default run reports 164 owned / 288 compared / 452 calls, zero
failed attempts, rounding, mismatches or errors. The document JSON length of 710,546, 80-entry
history, selection and full undo match the editor-alone page. The benchmark counter
advances from zero to one affected-command row; full-class rows remain 43 and idle
engine-1 remains 0/28. This is one warmed observed call, without a causal speed claim.
Build UI checks pass in 8.754 s: Split/Undo, branch scope, repeated sessions and
drawing/discard, without page errors. Four screenshots were captured; desktop and
compact layouts were inspected. The existing compatibility renderer banner reports
no WebGPU adapter, so this is not engine-1 or GPU certification.

Raw command clocks and accepted app scope:
`tmp/root-verification-temp/builder-split-focused/app-default-full.json` and
`app-default-full-reviewed.json`. The completed input guard covers 6,412 unchanged
files. Focused checks pass 94 Rust tests, 12 recordings / 272 exact calls, 225 JS
and Main check; 1,340 fixture JSON regenerate byte-stably.

## Make Movieclip Unique (2026-10-05)

The scoped paired driver uses `--own --default --warm --exact-math
--movieclip-unique` and the unchanged recorded sheet input. It clicks the actual
UI action at ticks 2 and 2.25 after matched warm-up. Both Unique calls adopt with
positive Native/engine/adoption clocks and no original-editor replay. Twelve paired
document/history/selection/phase boundaries, final state and full undo match exactly.

| Command | Adopted calls | Native call ms | Engine / parse / validation ms | Adoption ms | Owned subtotal ms | Editor-alone ms | Answer JSON total (UTF-16 units) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `makeMovieclipUnique` | 2 | 1.700 | 2.180 | 19.850 | 22.030 | 19.740 | 25,465 |

Native call time is included in the engine column. Average answer length is
12,732.5 UTF-16 units. This subtotal is slower in this observed pair: 1.145 ms
extra per Unique call, and its numerical cost comparison is false. It is a
matched-command diagnostic with no exit cost gate, full-class row advance, idle
result or causal speed claim. The existing full-class cost policy is unchanged.

The actual run completes in 16.693 s with classes mode, 6 owned / 10 compared /
16 calls and zero failed attempts, rounding, mismatches or page errors. Each button
invocation includes exactly one Unique and the original quiet `commitDrag` and
`commitAnimationEdit` preludes; these account for the four other owned calls.
All 6,412 guarded source/package inputs remain unchanged. The measurement counter
advances from one to two affected-command rows; full-class rows stay 43, classes
stay 5 and engine-1 stays 0/28.

Focused checks pass 233 Rust tests (all three new controls), five recorded replays /
79 exact calls, 266 JS tests and Main check. Shipping uses the required Wasm
profile: 10,257,065 bytes in 98.559 s, with codecs unchanged. Raw scoped command
clocks and completion are in
`tmp/root-verification-temp/movieclip-unique-focused/app-default-unique.json` and
`app-completion.json`; the initial rejected UI-accounting proof remains separate.

The optional broader movieclip UI test is not a pass claim: both old and new app
bytes time out waiting for the sixth repeated-paste counter in simple mode, before
its first Unique action. That existing clipboard regression remains separate from
the accepted scoped Unique proof.

## Layer folders (2026-10-05)

The scoped paired driver uses `--own --default --warm --exact-math
--layer-folders` with the unchanged recorded sheet input. All five mutators adopt
once with positive Native/engine/adoption clocks and no editor fallback. The
Advanced timeline Ungroup folder action exercises `deleteLayerFolder`. All 21
paired document/frame/history/selection/counter boundaries and both final outcomes
match literally through undo→redo→undo.

| Command | Adopted calls | Native call ms | Engine / parse / validation ms | Adoption ms | Owned subtotal ms | Editor-alone ms | Answer JSON length (UTF-16 units) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `createLayerFolder` | 1 | 0.795 | 1.000 | 1.960 | 2.960 | 3.565 | 799 |
| `renameLayerFolder` | 1 | 0.455 | 0.580 | 1.160 | 1.740 | 1.115 | 668 |
| `moveLayersToFolder` | 1 | 0.525 | 0.635 | 1.250 | 1.885 | 2.530 | 653 |
| `arrangeLayers` | 1 | 0.490 | 0.615 | 9.705 | 10.320 | 1.390 | 614 |
| `deleteLayerFolder` | 1 | 0.405 | 0.495 | 0.960 | 1.455 | 1.020 | 736 |

Native call time is included in the engine column. The five observed subtotals
sum to 18.360 ms versus 9.620 ms editor-alone. The regular checkpoint was running
concurrently; these matched-command diagnostics impose no cost gate, advance no
full-class rows, and establish no causal speed or idle engine-1 result.

The actual run completes in 36.160 s with classes mode, 5 owned / 15 compared /
20 calls and zero failed attempts, rounding, mismatches or page errors. All quiet
UI prelude counts are zero, and all 6,412 guarded source/package inputs remain
unchanged. Five individual measurements advance affected-command rows from two
to seven; full-class rows remain 43, classes remain 5 and engine-1 remains 0/28.

Focused checks pass 236 Rust tests (zero failed/ignored, 53 groups), three fresh
recordings / 58 exact calls, 204 JS tests (zero failed/skipped), and final Main
check in 90.776 s. Shipping uses the required Wasm profile: 10,291,828 bytes in
118.211 s, with codecs unchanged. Raw accepted command clocks and paired snapshots
are in `tmp/root-verification-temp/layer-folders-focused/app-default-folders.json`;
`actual-app-summary.json` and `app-completion.json` record the accepted scope and
completion. The existing joined one-shot event-time gap and repeated-paste UI
regression remain separate from folder adoption.

## Figure clipboard counters (2026-10-05)

The scoped paired driver uses `--own --default --warm --exact-math
--figure-clipboard-pastes` and the actual recorded sheet scenario. All 34 paired
result/error/event/document/frame/history/selection/counter boundaries and full
undo match. The default run has 13 owned / 17 compared / 33 attempted calls:
30 successful calls plus three expected validation errors. Twelve clipboard calls
adopt: nine pastes, two nonempty duplicates and one empty duplicate.
Those eleven calls have positive Native/engine/adoption clocks; the empty
no-op permits zero adoption time. There are no unexpected errors, mismatches or
rounding, and all 6,414 guarded source/package inputs remain unchanged.

| Command | Attempts | Native-adopted successes | Native call ms | Engine / parse / validation ms | Adoption ms | Eligible JS fallback ms | Failed Native ms | Candidate subtotal ms | Editor-alone ms | Answer JSON total (UTF-16 units) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `pasteSelection` | 12 | 9 | 6.390 | 6.885 | 17.160 | 0.385 | 0.430 | 24.860 | 20.630 | 21,068 |

Native call time is included in engine time. The three expected errors each
include their failed Native attempt and separate original JS fallback exactly
once. These clocks are a matched-command diagnostic, with no command-cost gate,
full-class row advance or causal speed claim. The measurement counter advances
from seven to eight; full-class rows stay 43, classes stay 5 and engine-1 stays 0/28.

The accepted scoped run completes in 22.837 s. Checks pass 238 Rust tests, three
recorded replays / 106 exact calls, 263 JS tests and final Main check in 47.783 s.
Required-profile shipping is 10,293,395 bytes in 55.648 s, with codecs unchanged.
Raw clocks, paired observations and completion are in
`tmp/root-verification-temp/figure-clipboard-focused/app-final.json` and
`app-final-completion.json`. Actual simple UI keyboard pastes reach counter six
without a shadow mismatch. Both broader movieclip UI attempts crash with unchanged
inputs; no overall UI pass is claimed. The joined one-shot event-time geometry gap
remains separate.
