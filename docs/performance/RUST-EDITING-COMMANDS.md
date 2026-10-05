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
