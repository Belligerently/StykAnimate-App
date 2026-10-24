//! Thumbnails and library previews: the frames they draw and how they are framed (the
//! GPU draws them in the app; `styk-wasm`).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Instant;

use styk_engine::thumbnails::library::{fitted, LibraryPreview, SAMPLES};
use styk_engine::{Budgets, Engine};

fn clock() -> f64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}

fn repository(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").join(path)
}

fn library(path: &str) -> LibraryPreview {
    let file = repository(path);
    let extension = file.extension().unwrap().to_str().unwrap().to_string();
    LibraryPreview::open(&std::fs::read(&file).unwrap(), &extension).unwrap()
}

#[test]
fn a_thumbnail_frame_is_prepared_on_a_worker_and_never_evaluated_on_the_asking_thread() {
    // A project of a 3D mesh (faces in depth runs) in the file the app saves.
    let file = repository("engine/tests/containers/project-arrays.styk");
    let engine = Engine::new(clock, Budgets::for_device(None));
    engine.open(&std::fs::read(&file).unwrap(), "styk", "test").unwrap();
    let frames = engine.export_frames(0).unwrap();
    assert!(frames.ready(3).is_none(), "nothing is evaluated by asking");
    frames.prefetch([3]);
    assert!(frames.ready(3).is_none(), "queued for a worker");
    while engine.step() {}
    let ready = frames.ready(3).expect("prepared by the worker (run here)");
    assert!(frames.ready(3).is_some(), "left in place");
    assert_eq!(ready.list.bytes(), frames.take(3).unwrap().list.bytes());
}

#[test]
fn a_library_poster_frames_its_artwork_with_a_margin() {
    let preview = library("engine/tests/containers/figure.stkz");
    let (sample, bounds) = preview.poster(clock);
    assert!(sample.drawn && !sample.list.prims.is_empty());
    assert_eq!(sample.bounds, Some(bounds));
    // The view: the longer side spans 90 % of the square, centred.
    let view = fitted(bounds, 320);
    let longer = bounds[2].max(bounds[3]);
    assert!((view.zoom * longer - 288.0).abs() < 1e-9);
    let centre = styk_eval::view::apply(view.matrix, [bounds[0] + bounds[2] / 2.0, bounds[1] + bounds[3] / 2.0]);
    assert!((centre[0] - 160.0).abs() < 1e-9 && (centre[1] - 160.0).abs() < 1e-9);
    // No background: the list fills nothing but the artwork.
    assert!(sample.list.prims.iter().take(sample.list.background_prims as usize).all(|p| p.color >> 24 == 0), "the background is transparent");
    // Records are stored by depth, so a mesh's depth runs may come before the background: look at every fill.
    assert!(sample.list.prims.iter().filter(|p| p.kind & 255 == styk_draw::K_FILL_VIEW).all(|p| p.color >> 24 == 0), "no record fills the view with a colour");
    let f = fitted([10.0, 20.0, 100.0, 50.0], 320);
    assert_eq!(f.matrix[0], 2.88);
    assert_eq!(styk_eval::view::apply(f.matrix, [60.0, 45.0]), [160.0, 160.0]);
}

#[test]
fn a_library_animation_keeps_a_stable_fit_and_wraps() {
    // Generated effects growing through a clip, and a figure swinging out and back (the widest bounds are mid-clip).
    for path in ["engine/tests/containers/movieclip-effects.stykmov", "engine/tests/containers/movieclip-motion.stykmov"] {
        let mut preview = library(path);
        assert!(preview.duration > 1.0, "{path}: a movieclip lasts its own duration ({})", preview.duration);
        let samples = preview.begin(clock);
        assert!(!samples.is_empty() && samples.len() <= SAMPLES as usize);
        let union = samples.iter().fold(None::<[f64; 4]>, |acc, (_, b)| Some(match acc {
            None => *b,
            Some(a) => {
                let (x0, y0) = (a[0].min(b[0]), a[1].min(b[1]));
                [x0, y0, (a[0] + a[2]).max(b[0] + b[2]) - x0, (a[1] + a[3]).max(b[1] + b[3]) - y0]
            }
        }));
        let (first, bounds, tick) = preview.frame(0.0, clock);
        assert_eq!(tick, 0.0);
        assert!(first.drawn);
        let u = union.unwrap();
        assert!(bounds[0] <= u[0] && bounds[1] <= u[1] && bounds[2] >= u[2] && bounds[3] >= u[3], "{path}: the fit holds every sample");
        let period = preview.duration / preview.fps;
        let (_, later, wrapped) = preview.frame(period + 0.5 / preview.fps, clock);
        assert!(wrapped < preview.duration, "{path}: tick {wrapped} wraps around {}", preview.duration);
        assert!(later[2] >= bounds[2] && later[3] >= bounds[3], "{path}: the fit only grows");
    }
}

#[test]
fn a_library_file_decodes_its_own_pictures() {
    // A document with a drawing cel stands in for a library file with pictures.
    let preview = library("engine/crates/styk-eval/tests/fixtures/drawlist/drawing-cel.json");
    assert!(preview.assets().pages > 0);
    assert!(!preview.images().is_empty());
    let other = library("engine/crates/styk-eval/tests/fixtures/drawlist/drawing-cel.json");
    assert_ne!(preview.lineage, other.lineage, "each file's pictures are its own");
}
