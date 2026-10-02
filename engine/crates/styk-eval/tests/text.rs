//! Text in draw lists.
//!
//! `tests/fixtures/drawlist/outlined-text.json` holds one glyph of a Pivot text
//! figure (from `finisher tdd2.piv`) with three figure outlines: 1.5 wide at
//! scale 1, 4 wide at scale 2.5, and 8 wide at half opacity at scale 4, all red.
//! `scripts/engine-app-parity.mjs` compares the pixels with the editor's renderers.

use std::path::Path;
use std::sync::Arc;

use styk_eval::assets::Assets;
use styk_eval::drawlist::{encode_composed, rgba8, Overrides, K_TRI};
use styk_eval::geometry::build;

fn open() -> styk_eval::doc::Document {
    styk_eval::open(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/drawlist/outlined-text.json")).expect("fixture")
}

fn outline_triangles(doc: styk_eval::doc::Document) -> usize {
    let assets = Assets::build(&doc);
    let prepared = styk_eval::prepare(doc);
    let scene = build(&prepared.doc, &prepared.evaluate(0.0));
    let list = encode_composed(&prepared.doc, &assets, &scene, &Overrides { hidden: &[], translations: &[], background: None, bypass_effects: false }, &[], None);
    let red = rgba8([220, 30, 30], 1.0);
    list.prims.iter().filter(|p| p.kind & 255 == K_TRI && p.color == red).count()
}

#[test]
fn a_figure_outline_strokes_its_text() {
    let doc = open();
    assert!(doc.unsupported.is_empty(), "{:?}", doc.unsupported);
    assert!(outline_triangles(doc) > 0, "outlined text draws its outline");
}

#[test]
fn text_without_a_figure_outline_has_none() {
    let mut doc = open();
    for figure in &mut doc.figures {
        Arc::make_mut(figure).outline = None;
    }
    assert_eq!(outline_triangles(doc), 0);
}
