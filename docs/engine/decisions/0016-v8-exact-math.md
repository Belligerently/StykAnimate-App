# D16: Stored arithmetic uses one Math, the one Node's V8 has, bit for bit

Status: accepted. Recorded in [`docs/RUST-ENGINE-PORT.md`](../../RUST-ENGINE-PORT.md) section 4.

## Context

Editing commands store the results of trigonometry in documents (rotated roots and angles, poses aimed at the pointer). A port that is to produce the same bytes as the editor (D15) must round each result as V8 does. Drawing does not need this: geometry is rounded to single precision, which hides last-bit differences.

## Decision

`styk-math` provides `sin`, `cos` and `atan2` ported from fdlibm 5.3 as V8 has it (`src/base/ieee754.cc`, over the `libm` crate's `atan` and a vendored copy of its `rem_pio2_large`), and V8's `hypot`. Engine code whose results are stored in documents uses it; `f64::sin` and friends are not used there. `styk-eval`'s inverse-kinematics solver and rig constraints take all four from it.

## Evidence

On two million inputs, against Node's results: Rust's `f64::sin` (glibc) differed in the last bit 2.7 % of the time, `f64::cos` 2.4 %, `f64::atan2` 17 %; the `libm` crate 0.8 % for `sin` and 0.7 % for `cos` (fdlibm 5.3's cosine kernel has a correction musl dropped) and for `atan2` when |y/x| exceeds 2^60. `styk-math` matched V8 on all 2.8 million inputs tried: random angles, tiny and large values up to 1e308, every n·π/2 up to 64 and its neighbours, NaN, infinities and signed zeros. `engine/tools/math-fixtures.mjs` records 6,403 sine and cosine inputs and 4,147 `atan2`/`hypot` pairs for its tests.

## What "the editor's bytes" means (measured 2026-10-03)

JavaScript engines do not agree on these functions. Of 20,000 random inputs, against
Node 24 (V8 13.6, fdlibm): Chromium 153 differs in `sin` for 780, `cos` 593, `atan2`
3,418 and `hypot` 0; WebKit 26.6 in 759, 603, 3,408 and 7,017; Firefox 155 in 243, 233,
0 and 7,189. So the JavaScript app already stores poses that differ in their last
digits depending on the browser it runs in, and there is no single set of bytes to
match. The engine takes one definite answer, Node's, because the command-replay and
record harnesses run the editor in Node: there the comparison is byte for byte, which
is what found wrong key orders, a 2π winding and a held pose. In a browser the shadow
comparison (`src/engine/engine-edit-shadow.mjs`) accepts numbers within 1e-9 relative
and requires everything else to be equal.

## Consequences

The engine gives the same numbers natively and in WebAssembly, on every platform and
in every browser, which the JavaScript engine does not. Evaluation tweens angles
through `styk_math::shortest`, and the IK solver and rig constraints use `styk-math`
throughout: posing a limb by its IK target stores the solved angles, and since the switch
the solver returns the editor's numbers bit for bit (`tests/ik.rs` compares them
exactly). Geometry, whose points are rounded to single precision, and 3D quaternion
`acos`/`asin` still use the platform's functions; `acos` is to be added before the engine
stores 3D poses between keys. A function not yet in `styk-math` is ported and recorded
before a command uses it.
