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
