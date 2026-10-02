//! Headless engine tools.
//!
//!   styk info <file>                      load timing, contents, unsupported features
//!   styk compare <file> <reference.json>  primitive-by-primitive parity with the JS editor
//!   styk bench <file> [--frames N]        evaluation + geometry throughput

use std::path::Path;
use std::time::Instant;

use rayon::prelude::*;
use serde_json::Value;
use styk_core::eval::Prepared;
use styk_core::geometry::{build, ItemKind, Scene};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("info") if args.len() >= 2 => info(&args[1]),
        Some("compare") if args.len() >= 3 => compare(&args[1], &args[2], &args[3..]),
        Some("bench") if args.len() >= 2 => bench(&args[1], &args[2..]),
        _ => {
            eprintln!("usage: styk info <file> | compare <file> <reference.json> [--verbose] | bench <file> [--frames N]");
            2
        }
    };
    std::process::exit(code);
}

fn load(path: &str) -> Result<(Prepared, f64), String> {
    let started = Instant::now();
    let doc = styk_core::open(Path::new(path)).map_err(|e| format!("{path}: {e}"))?;
    let prepared = styk_core::prepare(doc);
    Ok((prepared, started.elapsed().as_secs_f64() * 1000.0))
}

fn info(path: &str) -> i32 {
    let (p, ms) = match load(path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let d = &p.doc;
    println!("{}: version {}, {}x{}, {} fps, {} ticks", d.name, d.version, d.canvas[0], d.canvas[1], d.fps, p.total_ticks);
    println!(
        "  {} figures ({} segments), {} keyframes, {} objects, {} images, {} pose sources{}",
        d.figures.len(),
        d.segment_count(),
        d.keyframes.len(),
        d.keyframes.iter().map(|k| k.objects.len()).sum::<usize>(),
        d.images.len(),
        d.frame_sources.len(),
        if d.sheet.is_some() { ", layer sheet" } else { "" }
    );
    println!("  opened in {ms:.1} ms");
    if !d.unsupported.is_empty() {
        println!("  not evaluated yet: {}", d.unsupported.join(", "));
    }
    0
}

fn bench(path: &str, rest: &[String]) -> i32 {
    let (p, ms) = match load(path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let frames: u32 = rest.iter().position(|a| a == "--frames").and_then(|i| rest.get(i + 1)).and_then(|v| v.parse().ok()).unwrap_or(p.total_ticks);
    let frames = frames.min(p.total_ticks).max(1);
    println!("{}: opened in {ms:.1} ms, {} ticks", p.doc.name, p.total_ticks);
    // Warm up.
    let _ = build(&p.doc, &p.evaluate(0.0));
    let started = Instant::now();
    let mut items = 0usize;
    let mut worst = 0f64;
    for t in 0..frames {
        let s = Instant::now();
        let scene = build(&p.doc, &p.evaluate(t as f64));
        worst = worst.max(s.elapsed().as_secs_f64() * 1000.0);
        items += scene.items.len();
    }
    let serial = started.elapsed().as_secs_f64() * 1000.0;
    let started = Instant::now();
    let parallel_items: usize = (0..frames).into_par_iter().map(|t| build(&p.doc, &p.evaluate(t as f64)).items.len()).sum();
    let parallel = started.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(items, parallel_items);
    println!(
        "  one frame at a time: {:.3} ms/frame (worst {:.2} ms), {:.0} primitives/frame",
        serial / frames as f64,
        worst,
        items as f64 / frames as f64
    );
    println!(
        "  all cores ({} threads): {:.3} ms/frame → {:.0} frames/s of evaluation",
        rayon::current_num_threads(),
        parallel / frames as f64,
        frames as f64 / (parallel / 1000.0)
    );
    0
}

fn kind_name(k: ItemKind) -> &'static str {
    k.name()
}

fn pt(v: &Value) -> Option<[f64; 2]> {
    let a = v.as_array()?;
    Some([a.first()?.as_f64()?, a.get(1)?.as_f64()?])
}

fn close(a: [f64; 2], b: [f64; 2], tol: f64) -> bool {
    (a[0] - b[0]).abs() <= tol && (a[1] - b[1]).abs() <= tol
}

fn compare(path: &str, reference: &str, rest: &[String]) -> i32 {
    let verbose = rest.iter().any(|a| a == "--verbose");
    let (p, _) = match load(path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let text = match std::fs::read_to_string(reference) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{reference}: {e}");
            return 1;
        }
    };
    let r: Value = serde_json::from_str(&text).expect("reference JSON");
    let ref_total = r["totalTicks"].as_u64().unwrap_or(0) as u32;
    if ref_total != p.total_ticks {
        println!("total ticks differ: reference {ref_total}, engine {}", p.total_ticks);
    }
    let tol = 2e-3;
    let mut frames = 0;
    let mut bad_frames = 0;
    let mut total = 0usize;
    let mut mismatched = 0usize;
    let mut first_reports = 0;
    for f in r["frames"].as_array().unwrap() {
        let tick = f["tick"].as_f64().unwrap();
        let scene: Scene = build(&p.doc, &p.evaluate(tick));
        let expected = f["drawables"].as_array().unwrap();
        frames += 1;
        total += expected.len();
        let mut problems: Vec<String> = Vec::new();
        if expected.len() != scene.items.len() {
            problems.push(format!("drawable count: reference {}, engine {}", expected.len(), scene.items.len()));
        }
        for (i, (e, it)) in expected.iter().zip(scene.items.iter()).enumerate() {
            let uid = scene.objects[it.slot as usize].uid;
            let mut why = Vec::new();
            if e["uid"].as_u64() != Some(uid as u64) {
                why.push(format!("uid {} vs {}", e["uid"], uid));
            }
            if e["type"].as_str() != Some(kind_name(it.kind)) {
                why.push(format!("type {} vs {}", e["type"], kind_name(it.kind)));
            }
            if let Some(z) = e["z"].as_f64()
                && (z - it.z).abs() > 1e-6
            {
                why.push(format!("z {z} vs {}", it.z));
            }
            let rgb: Vec<u8> = e["rgb"].as_array().map(|a| a.iter().filter_map(|v| v.as_u64().map(|x| x as u8)).collect()).unwrap_or_default();
            if rgb.len() == 3 && rgb != it.rgb {
                why.push(format!("rgb {rgb:?} vs {:?}", it.rgb));
            }
            for (key, value) in [("alpha", it.alpha), ("opacity", it.opacity)] {
                if let Some(v) = e[key].as_f64()
                    && (v - value).abs() > 1e-4
                {
                    why.push(format!("{key} {v} vs {value}"));
                }
            }
            if it.kind == ItemKind::Poly {
                let pts: Vec<[f64; 2]> = e["points"].as_array().map(|a| a.iter().filter_map(pt).collect()).unwrap_or_default();
                let ours = scene.points_of(it);
                if pts.len() != ours.len() {
                    why.push(format!("poly points {} vs {}", pts.len(), ours.len()));
                } else if let Some(k) = (0..pts.len()).find(|&k| !close(pts[k], ours[k], tol)) {
                    why.push(format!("poly point {k} {:?} vs {:?}", pts[k], ours[k]));
                }
            } else {
                if let (Some(s), Some(en)) = (pt(&e["start"]), pt(&e["end"])) {
                    if !close(s, it.start, tol) {
                        why.push(format!("start {s:?} vs {:?}", it.start));
                    }
                    if !close(en, it.end, tol) {
                        why.push(format!("end {en:?} vs {:?}", it.end));
                    }
                }
                if let Some(th) = e["thickness"].as_f64()
                    && (th - it.thickness).abs() > tol
                {
                    why.push(format!("thickness {th} vs {}", it.thickness));
                }
            }
            if !why.is_empty() {
                mismatched += 1;
                if problems.len() < 6 {
                    problems.push(format!("#{i} {} uid {} seg/poly {}: {}", kind_name(it.kind), uid, it.index, why.join("; ")));
                }
            }
        }
        if !problems.is_empty() {
            bad_frames += 1;
            if verbose || first_reports < 3 {
                first_reports += 1;
                println!("tick {tick}:");
                for p in &problems {
                    println!("  {p}");
                }
            }
        }
    }
    println!(
        "{}: {frames} frames, {total} reference primitives, {mismatched} mismatched primitives, {bad_frames} frames with differences",
        Path::new(path).file_name().unwrap().to_string_lossy()
    );
    if bad_frames == 0 { 0 } else { 3 }
}
