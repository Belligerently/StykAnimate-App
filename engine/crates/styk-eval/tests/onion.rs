//! Onion skins in draw lists: what a ghost's tint changes and what it keeps.
//!
//! `tests/fixtures/drawlist/painted-limbs.json` is a two-limb figure whose
//! segments 1 and 2 have custom paints (linear, radial) and segment 4 a
//! secondary colour. The editor's onion skins (`onion-skin.mjs`) replace each
//! drawable's colour with the tint and drop its secondary colour, but a custom
//! paint keeps its own colours; `scripts/engine-app-parity.mjs` compares the
//! pixels with the editor's renderer.

use std::path::Path;

use styk_eval::assets::Assets;
use styk_eval::drawlist::{encode_composed, Ghost, Layer, Overrides, NO_PAINT};
use styk_eval::geometry::build;

const TINT: [u8; 3] = [231, 107, 112];

#[test]
fn a_tinted_ghost_keeps_custom_paints_and_drops_secondary_colours() {
    let doc = styk_eval::open(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/drawlist/painted-limbs.json")).expect("fixture");
    let assets = Assets::build(&doc);
    let prepared = styk_eval::prepare(doc);
    let scene = build(&prepared.doc, &prepared.evaluate(0.0));
    let ghost_scene = build(&prepared.doc, &prepared.evaluate(8.0));
    let overrides = Overrides { hidden: &[], translations: &[], background: None, bypass_effects: false };
    let plain = encode_composed(&prepared.doc, &assets, &scene, &overrides, &[], None);
    let ghost = Ghost { scene: &ghost_scene, opacity: 0.14, tint: Some(TINT), hidden: &[] };
    // One figure: the ghost under the current pose.
    let with_ghost = encode_composed(&prepared.doc, &assets, &scene, &overrides, &[ghost], Some(&[Layer::Ghost(0, 0), Layer::Current(0)]));
    let painted = |list: &styk_eval::drawlist::DrawList| list.prims.iter().filter(|p| p.paint != NO_PAINT).count();
    // The current frame: two custom paints and one two-colour gradient.
    assert_eq!(painted(&plain), 3, "custom paints and the secondary colour");
    // The ghost adds its two custom paints, not the secondary-colour gradient.
    assert_eq!(painted(&with_ghost), 5, "a ghost keeps custom paints only");
    let tint = u32::from_le_bytes([TINT[0], TINT[1], TINT[2], 255]);
    let tinted = with_ghost.prims.iter().filter(|p| p.paint == NO_PAINT && p.color == tint).count();
    assert!(tinted >= 4, "the ghost's other segments are drawn in its tint ({tinted})");
}
