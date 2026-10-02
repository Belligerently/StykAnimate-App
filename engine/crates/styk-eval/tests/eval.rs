//! Evaluation against the JavaScript editor.
//!
//! Every fixture in `tests/fixtures/eval/` is a small project and the editor's
//! own evaluation of it (`node engine/tools/eval-fixtures.mjs` regenerates
//! them). The engine must draw the same joints and primitives, within the
//! tolerance `styk compare` uses.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use styk_eval::doc::{Document, SheetTrack};
use styk_eval::parity::{self, Report};

fn folder() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/eval")
}

/// Fixture names, from the reference dumps present in the folder.
fn fixtures() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(folder())
        .expect("fixture folder")
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.strip_suffix(".reference.json").map(str::to_string))
        .collect();
    names.sort();
    names
}

fn open(name: &str) -> Document {
    styk_eval::open(&folder().join(format!("{name}.json"))).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn compare(name: &str, document: Document) -> Report {
    let path = folder().join(format!("{name}.reference.json"));
    let reference = styk_doc::json::parse(&std::fs::read(&path).expect("reference")).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    parity::compare(&styk_eval::prepare(document), &reference)
}

fn describe(report: &Report) -> String {
    report.bad_frames.iter().map(|frame| format!("tick {}:\n  {}", frame.tick, frame.problems.join("\n  "))).collect::<Vec<_>>().join("\n")
}

#[test]
fn every_fixture_evaluates_like_the_editor() {
    let names = fixtures();
    assert!(names.len() >= 3, "fixtures are missing: run node engine/tools/eval-fixtures.mjs");
    for name in &names {
        let document = open(name);
        assert!(document.unsupported.is_empty(), "{name}: not evaluated: {:?}", document.unsupported);
        let report = compare(name, document);
        assert!(report.frames > 0 && report.joints > 0, "{name}: an empty reference");
        assert!(report.total_ticks.is_none(), "{name}: timeline length {:?}", report.total_ticks);
        assert!(
            report.matches(),
            "{name}: {} of {} primitives and {} of {} joints differ\n{}",
            report.mismatched_primitives,
            report.primitives,
            report.mismatched_joints,
            report.joints,
            describe(&report)
        );
    }
}

/// The movieclip fixtures would prove nothing if they passed without the
/// replacements their layer keys carry.
#[test]
fn movieclip_fixtures_depend_on_their_replacements() {
    for name in fixtures().into_iter().filter(|name| name.starts_with("movieclip-")) {
        let mut document = open(&name);
        let sheet = Arc::make_mut(document.sheet.as_mut().unwrap_or_else(|| panic!("{name}: no layer sheet")));
        let mut replacements = 0;
        for track in sheet.tracks.values_mut() {
            let mut bare: SheetTrack = (**track).clone();
            for key in &mut bare.keys {
                replacements += key.movieclip_patches.len();
                key.movieclip_patches.clear();
            }
            *track = Arc::new(bare);
        }
        assert!(replacements > 0, "{name}: no replacement in the fixture");
        assert!(!compare(&name, document).matches(), "{name}: matches without its replacements");
    }
}
