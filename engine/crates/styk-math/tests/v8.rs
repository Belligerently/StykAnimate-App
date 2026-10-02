//! styk-math against what V8 returned (engine/tools/math-fixtures.mjs), bit for bit.

fn read(name: &str) -> Vec<f64> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join(name);
    std::fs::read(path).unwrap().chunks_exact(8).map(|c| f64::from_le_bytes(c.try_into().unwrap())).collect()
}

fn same(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
}

#[test]
fn sine_and_cosine() {
    let cases = read("sincos.bin");
    let wrong: Vec<String> = cases
        .chunks_exact(3)
        .filter(|c| !same(styk_math::sin(c[0]), c[1]) || !same(styk_math::cos(c[0]), c[2]))
        .map(|c| format!("x {:e}: sin {:e} (V8 {:e}), cos {:e} (V8 {:e})", c[0], styk_math::sin(c[0]), c[1], styk_math::cos(c[0]), c[2]))
        .collect();
    assert!(cases.len() > 18_000 && wrong.is_empty(), "{} of {} differ: {:#?}", wrong.len(), cases.len() / 3, &wrong[..wrong.len().min(10)]);
}

#[test]
fn arc_tangent_and_hypotenuse() {
    let cases = read("atan2-hypot.bin");
    for c in cases.chunks_exact(4) {
        assert!(same(styk_math::atan2(c[0], c[1]), c[2]), "atan2({:e}, {:e})", c[0], c[1]);
        assert!(same(styk_math::hypot(c[0], c[1]), c[3]), "hypot({:e}, {:e})", c[0], c[1]);
    }
}

/// The platform's functions are not V8's: the reason this crate exists. If this
/// starts failing, std became V8-exact on this platform and the crate could go.
#[test]
fn std_differs_from_v8() {
    let cases = read("sincos.bin");
    assert!(cases.chunks_exact(3).any(|c| !same(c[0].cos(), c[2]) || !same(libm::cos(c[0]), c[2])));
}
