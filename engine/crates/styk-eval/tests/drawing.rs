//! Drawing cels in assets and draw lists.
//!
//! `tests/fixtures/drawlist/drawing-cel.json` holds the largest drawing cel of
//! `Jay on a new app (repaired).styk` (2,248 × 382 cel pixels at 2 pixels per
//! unit), one object of it in the middle of the canvas.
//! `scripts/engine-app-parity.mjs --zoom` compares the pixels with the editor's
//! GPU drawing display at several zoom levels.

use std::path::Path;

use styk_eval::assets::{decode_cel, Assets};
use styk_eval::drawlist::{encode_composed, Overrides, F_DRAWING};
use styk_eval::geometry::build;

#[test]
fn every_level_of_a_cel_is_placed_decoded_and_drawn() {
    let doc = styk_eval::open(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/drawlist/drawing-cel.json")).expect("fixture");
    let assets = Assets::build(&doc);
    assert_eq!(assets.cels.len(), 1);
    let (cel, starts) = assets.cels.values().next().expect("one cel");
    assert_eq!(cel.levels.len(), styk_eval::drawing::MAX_LEVEL as usize + 1);
    let slots: usize = cel.levels.iter().map(Vec::len).sum();
    assert_eq!(assets.drawing.len(), slots);
    assert!(starts.windows(2).all(|pair| pair[0] <= pair[1]));
    // Each level holds fewer tiles than the one below, down to a tile and its halo.
    assert!(cel.levels.windows(2).all(|pair| pair[1].len() <= pair[0].len()));

    let decoded = decode_cel(&assets, cel);
    assert_eq!(decoded.len(), slots, "a slot per drawn tile and level");
    assert!(decoded.iter().all(|image| image.drawing && image.width == 66 && image.height == 66));
    assert!(decoded.iter().any(|image| image.rgba.chunks_exact(4).any(|texel| texel[3] != 0)), "the cel has visible texels");

    let prepared = styk_eval::prepare(doc);
    let scene = build(&prepared.doc, &prepared.evaluate(0.0));
    let list = encode_composed(&prepared.doc, &assets, &scene, &Overrides { hidden: &[], translations: &[], background: None, bypass_effects: false }, &[], None);
    let drawn = list.prims.iter().filter(|p| p.kind & F_DRAWING != 0).count();
    assert_eq!(drawn, slots, "every level's tiles are in the list; the shader keeps one level");
    for (level, tiles) in cel.levels.iter().enumerate() {
        let at_level = list.prims.iter().filter(|p| p.kind & F_DRAWING != 0 && p.b[3] == level as f32).count();
        assert_eq!(at_level, tiles.len(), "level {level}");
    }
}
