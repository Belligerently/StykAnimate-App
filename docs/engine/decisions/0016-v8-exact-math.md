# D16: Stored arithmetic uses V8's Math functions bit for bit

Status: accepted. Recorded in [`docs/RUST-ENGINE-PORT.md`](../../RUST-ENGINE-PORT.md) section 4.

## Context

Editing commands store the results of trigonometry in documents (rotated roots and angles, poses aimed at the pointer). A port that is to produce the same bytes as the editor (D15) must round each result as V8 does. Drawing does not need this: geometry is rounded to single precision, which hides last-bit differences.

## Decision

`styk-math` provides `sin`, `cos` and `atan2` ported from fdlibm 5.3 as V8 has it (`src/base/ieee754.cc`, over the `libm` crate's `atan` and a vendored copy of its `rem_pio2_large`), and V8's `hypot`. Engine code whose results are stored in documents uses it; `f64::sin` and friends are not used there. `styk-eval`'s inverse-kinematics solver takes `hypot` from it too.

## Evidence

On two million inputs, against Node's results: Rust's `f64::sin` (glibc) differed in the last bit 2.7 % of the time, `f64::cos` 2.4 %, `f64::atan2` 17 %; the `libm` crate 0.8 % for `sin` and 0.7 % for `cos` (fdlibm 5.3's cosine kernel has a correction musl dropped) and for `atan2` when |y/x| exceeds 2^60. `styk-math` matched V8 on all 2.8 million inputs tried: random angles, tiny and large values up to 1e308, every n·π/2 up to 64 and its neighbours, NaN, infinities and signed zeros. `engine/tools/math-fixtures.mjs` records 6,403 sine and cosine inputs and 4,147 `atan2`/`hypot` pairs for its tests.

## Consequences

The evaluation code still uses the platform's functions where results are only drawn. Before the engine stores evaluation results in documents (baking inverse-kinematics poses, for one), those paths move to `styk-math`. A function not yet in `styk-math` is ported and recorded the same way before a command uses it.
