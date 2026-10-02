//! Headless engine tools.
//!
//!   styk info <file>                      load timing, contents, unsupported features
//!   styk compare <file> <reference.json>  primitive-by-primitive parity with the JS editor
//!                                         (effects too, for a reference written with --vfx)
//!   styk bench <file> [--frames N]        evaluation + geometry throughput
//!   styk layers <file> <tick>             the effect layers of a frame (docs/engine/vfx.md)
//!   styk edit-replay <reference.json>…    editing commands against the JS editor's recording
//!   styk edit-bench <report.json> <uid> <tick>   what a gesture costs in styk-edit on a document
//!   styk records <folder> [--write]       the evaluated frame as records against the JS editor's
//!                                         (engine/tools/records-reference.mjs wrote the folder)

use std::path::Path;
use std::time::Instant;

use rayon::prelude::*;
use styk_eval::eval::Prepared;
use styk_eval::geometry::build;

mod container;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("container") if args.len() >= 3 && args[1] == "roundtrip" => container::roundtrip(&args[2], &args[3..]),
        Some("container") if args.len() >= 4 && args[1] == "json" => container::json(&args[2], &args[3]),
        Some("describe") if args.len() >= 3 => match load(&args[1]) {
            Ok((prepared, _)) => {
                print!("{}", styk_eval::describe_tick(&prepared, args[2].parse().unwrap_or(0)));
                0
            }
            Err(e) => {
                eprintln!("{e}");
                2
            }
        },
        Some("info") if args.len() >= 2 => info(&args[1]),
        Some("layers") if args.len() >= 3 => layers(&args[1], &args[2], args[3..].iter().any(|a| a == "--all")),
        Some("compare") if args.len() >= 3 => compare(&args[1], &args[2], &args[3..]),
        Some("bench") if args.len() >= 2 => bench(&args[1], &args[2..]),
        Some("render") if args.len() >= 4 => render(&args[1], &args[2], &args[3], &args[4..]),
        Some("raster") if args.len() >= 4 => raster(&args[1], &args[2], &args[3], &args[4..]),
        Some("gpubench") if args.len() >= 3 => gpubench(&args[1], &args[2], args[3..].iter().any(|a| a == "--no-effects")),
        Some("edit-replay") if args.len() >= 2 => edit_replay(&args[1..]),
        Some("records") if args.len() >= 2 => records(&args[1], &args[2..]),
        Some("edit-bench") if args.len() >= 4 => edit_bench(&args[1], &args[2], &args[3]),
        Some("diff") if args.len() >= 4 => diff(&args[1], &args[2], &args[3], &args[4..]),
        _ => {
            eprintln!("usage: styk info <file> | compare <file> <reference.json> [--verbose] | bench <file> [--frames N] | render <file> <tick> <out.png> [--size WxH] [--zoom Z] [--center X,Y] [--no-clip] [--neutral] [--after T] | gpubench <file> <tick> [--no-effects] | layers <file> <tick> [--all] | edit-replay <reference.json>...");
            2
        }
    };
    std::process::exit(code);
}

/// Replays command recordings (engine/tools/edit-replay.mjs) in styk-edit. Exit code 1
/// when any step the engine runs differs from the editor's.
fn edit_replay(files: &[String]) -> i32 {
    let mut failed = false;
    for file in files {
        let outcome = std::fs::read(file).map_err(|e| e.to_string()).and_then(|bytes| styk_doc::json::parse(&bytes).map_err(|e| e.to_string())).and_then(|reference| styk_edit::replay::replay(&reference));
        match outcome {
            Ok(outcome) => {
                let unported = outcome.unported.as_ref().map(|(step, what)| format!(", step {step} not ported ({what})")).unwrap_or_default();
                println!("{}: {}/{} steps match{unported}", outcome.scenario, outcome.matched, outcome.steps);
                for mismatch in &outcome.mismatches {
                    println!("  {mismatch}");
                }
                failed |= !outcome.passed();
            }
            Err(error) => {
                println!("{file}: {error}");
                failed = true;
            }
        }
    }
    i32::from(failed)
}

/// Times a gesture in styk-edit on a loaded report (records-reference.mjs writes one):
/// selecting a figure at a tick, a drag of 30 moves and of 30 joint poses, each committed.
fn edit_bench(report: &str, uid: &str, tick: &str) -> i32 {
    let ms = |since: Instant| since.elapsed().as_secs_f64() * 1000.0;
    let started = Instant::now();
    let tree = match std::fs::read(report).map_err(|e| e.to_string()).and_then(|bytes| styk_doc::json::parse(&bytes).map_err(|e| e.to_string())) {
        Ok(tree) => tree,
        Err(error) => {
            println!("{report}: {error}");
            return 2;
        }
    };
    println!("parse {:.0} ms", ms(started));
    let num = |text: &str| styk_doc::Value::Num(text.parse().unwrap_or(0.0));
    let mut editor = styk_edit::Editor::new(tree, styk_edit::Options { property_channels: true, layer_timeline: true, auto_key: false });
    let mut call = |name: &str, args: &[styk_doc::Value]| {
        let started = Instant::now();
        let result = editor.call(name, args);
        editor.take_events();
        (ms(started), result.map(|value| styk_doc::json::to_string(&value).len()).map_err(|e| e.to_string()))
    };
    let report = |label: &str, (time, result): (f64, Result<usize, String>)| println!("{label}: {time:.2} ms{}", result.err().map(|e| format!(" ({e})")).unwrap_or_default());
    report("seek (first evaluation)", call("seek", &[num(tick)]));
    report("select", call("select", &[num(uid), num("0")]));
    for (gesture, step) in [("moveSelection", "move"), ("poseJoint", "pose")] {
        report(&format!("beginDrag for {step}"), call("beginDrag", &[styk_doc::Value::str("Pose figure")]));
        let mut times = Vec::new();
        for i in 0..30 {
            let (time, result) = call(gesture, &[num(&format!("{}", 300 + i)), num(&format!("{}", 250 + i))]);
            if let Err(error) = result {
                println!("{gesture}: {error}");
                break;
            }
            times.push(time);
        }
        times.sort_by(|a, b| a.total_cmp(b));
        if !times.is_empty() {
            println!("{gesture} step: median {:.2} ms, slowest {:.2} ms over {}", times[times.len() / 2], times[times.len() - 1], times.len());
        }
        report(&format!("commitDrag after {step}"), call("commitDrag", &[]));
    }
    report("undo", call("undo", &[]));
    0
}

/// Compares the evaluated frame as records, tick by tick, with what the editor stores
/// (engine/tools/records-reference.mjs). `--write` leaves the engine's record of the
/// first differing tick in `<folder>/engine-<tick>.json`. Exit code 1 on a difference;
/// ticks the engine does not produce as records yet are counted, not failed.
fn records(folder: &str, flags: &[String]) -> i32 {
    use sha2::Digest;
    let read = |name: &str| std::fs::read(Path::new(folder).join(name)).map_err(|e| format!("{folder}/{name}: {e}"));
    let run = || -> Result<(usize, usize, Vec<String>, Vec<String>), String> {
        let tree = styk_doc::json::parse(&read("report.json")?).map_err(|e| e.to_string())?;
        let reference = styk_doc::json::parse(&read("records.json")?).map_err(|e| e.to_string())?;
        let document = styk_eval::from_value(&tree, "").map_err(|e| e.to_string())?;
        let prepared = styk_eval::prepare(document);
        let records = styk_eval::records::Records::new(&tree, &prepared).map_err(|e| e.to_string())?;
        let ticks: Vec<styk_doc::Value> = reference.get("ticks").and_then(|t| t.elements()).map(|e| e.iter().collect()).unwrap_or_default();
        let (mut matched, mut unported, mut wrong) = (0, Vec::new(), Vec::new());
        for entry in &ticks {
            let tick = entry.get("tick").and_then(|t| t.as_f64()).unwrap_or(0.0);
            let json = match records.snapshot(tick) {
                Ok(value) => styk_doc::json::to_string(&value),
                Err(styk_eval::records::Error::Unported(what)) => {
                    unported.push(format!("tick {tick}: {what}"));
                    continue;
                }
                Err(error) => return Err(format!("tick {tick}: {error}")),
            };
            let digest: String = sha2::Sha256::digest(json.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
            if entry.get("sha256").and_then(|s| s.as_str()) == Some(digest.as_str()) {
                matched += 1;
                continue;
            }
            let expected = entry.get("json").and_then(|j| j.as_str());
            let detail = match expected {
                Some(expected) => {
                    let at = json.bytes().zip(expected.bytes()).position(|(a, b)| a != b).unwrap_or(json.len().min(expected.len()));
                    let from = json.floor_char_boundary(at.saturating_sub(120));
                    let cut = |text: &str| text[text.floor_char_boundary(from.min(text.len()))..text.floor_char_boundary((at + 100).min(text.len()))].to_string();
                    format!("at byte {at}:\n    engine …{}…\n    editor …{}…", cut(&json), cut(expected))
                }
                None => format!("{} bytes, the editor's {}", json.len(), entry.get("bytes").and_then(|b| b.as_f64()).unwrap_or(0.0)),
            };
            if wrong.is_empty() && flags.iter().any(|f| f == "--write") {
                let _ = std::fs::write(Path::new(folder).join(format!("engine-{tick}.json")), &json);
            }
            wrong.push(format!("tick {tick} differs {detail}"));
        }
        Ok((ticks.len(), matched, unported, wrong))
    };
    match run() {
        Ok((total, matched, unported, wrong)) => {
            println!("{folder}: {matched}/{total} ticks identical, {} not ported, {} different", unported.len(), wrong.len());
            for line in unported.iter().take(3).chain(wrong.iter().take(3)) {
                println!("  {line}");
            }
            i32::from(!wrong.is_empty())
        }
        Err(error) => {
            println!("{folder}: {error}");
            1
        }
    }
}

fn load(path: &str) -> Result<(Prepared, f64), String> {
    let started = Instant::now();
    let doc = styk_eval::open(Path::new(path)).map_err(|e| format!("{path}: {e}"))?;
    let t0 = Instant::now();
    let prepared = styk_eval::prepare(doc);
    if std::env::var_os("STYK_TIMING").is_some() {
        eprintln!("  prepare (timelines, presence index) {:.1} ms", t0.elapsed().as_secs_f64() * 1000.0);
    }
    Ok((prepared, started.elapsed().as_secs_f64() * 1000.0))
}

/// The raster layers of one frame's draw list: kind, rectangles, scale and effects.
fn layers(path: &str, tick: &str, all: bool) -> i32 {
    let (p, _) = match load(path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let assets = styk_eval::assets::Assets::build(&p.doc);
    let scene = build(&p.doc, &p.evaluate(tick.parse().unwrap_or(0.0)));
    let list = styk_eval::drawlist::encode(&p.doc, &assets, &scene);
    for (g, group) in list.groups.iter().enumerate() {
        let prims = list.prims.iter().filter(|p| p.group as usize == g).count();
        let Some(layer) = list.layers.get(group.layer as usize) else {
            if all {
                println!("group {g} (parent {}, depth {}, opacity {}): {prims} prims, bounds {:?}", group.parent, group.depth, group.opacity, group.bounds);
            }
            continue;
        };
        let effects: Vec<&str> = styk_eval::drawlist::layer_effects(layer).iter().map(|e| e.kind().name()).collect();
        println!("group {g} (parent {}, depth {}, opacity {}): {prims} prims, {:?} [{}]", group.parent, group.depth, group.opacity, layer.kind, effects.join(", "));
    }
    0
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

/// Parity with the JavaScript editor. The comparison itself is `styk_eval::parity`,
/// shared with the engine's fixture tests.
fn compare(path: &str, reference: &str, rest: &[String]) -> i32 {
    let verbose = rest.iter().any(|a| a == "--verbose");
    let (p, _) = match load(path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let bytes = match std::fs::read(reference) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{reference}: {e}");
            return 1;
        }
    };
    let r = styk_doc::json::parse(&bytes).expect("reference JSON");
    let report = styk_eval::parity::compare(&p, &r);
    if let Some((reference, engine)) = report.total_ticks {
        println!("total ticks differ: reference {reference}, engine {engine}");
    }
    for frame in report.bad_frames.iter().take(if verbose { usize::MAX } else { 3 }) {
        println!("tick {}:", frame.tick);
        for p in &frame.problems {
            println!("  {p}");
        }
    }
    println!(
        "{}: {} frames, {} reference primitives, {} mismatched primitives, {} joints, {} mismatched joints, {} frames with differences",
        Path::new(path).file_name().unwrap().to_string_lossy(),
        report.frames,
        report.primitives,
        report.mismatched_primitives,
        report.joints,
        report.mismatched_joints,
        report.bad_frames.len()
    );
    let effects = &report.vfx;
    if effects.frames > 0 {
        println!(
            "  effects: {} frames, {} stacks, {} reference effects, {} mismatched effects, {} other differences",
            effects.frames, effects.stacks, effects.effects, effects.mismatched_effects, effects.other
        );
    }
    if report.matches() { 0 } else { 3 }
}

fn option<'a>(rest: &'a [String], name: &str) -> Option<&'a str> {
    rest.iter().position(|a| a == name).and_then(|i| rest.get(i + 1)).map(String::as_str)
}

/// Renders one tick headlessly through the GPU renderer and writes a PNG.
/// `styk raster`: the same frame as `styk render`, rasterized on the CPU (styk-raster).
fn raster(path: &str, tick: &str, out: &str, rest: &[String]) -> i32 {
    use styk_eval::assets::Assets;
    use styk_eval::drawlist::encode;
    use styk_eval::view::camera_matrix;
    let (p, ms) = match load(path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let tick: f64 = tick.parse().unwrap_or(0.0);
    let (w, h) = option(rest, "--size")
        .and_then(|s| s.split_once('x'))
        .and_then(|(a, b)| Some((a.parse::<u32>().ok()?, b.parse::<u32>().ok()?)))
        .unwrap_or((p.doc.canvas[0] + 80, p.doc.canvas[1] + 80));
    let started = Instant::now();
    let assets = Assets::build(&p.doc);
    let images = styk_raster::decode_all(&p.doc, &assets);
    let assets_ms = started.elapsed().as_secs_f64() * 1000.0;
    let started = Instant::now();
    let frame = p.evaluate(tick);
    let scene = build(&p.doc, &frame);
    let list = encode(&p.doc, &assets, &scene);
    let encode_ms = started.elapsed().as_secs_f64() * 1000.0;
    let (cw, ch) = (p.doc.canvas[0] as f64, p.doc.canvas[1] as f64);
    let zoom = option(rest, "--zoom").and_then(|z| z.parse::<f64>().ok()).unwrap_or_else(|| ((w as f64 - 40.0) / cw).min((h as f64 - 40.0) / ch));
    let center = option(rest, "--center")
        .and_then(|s| s.split_once(','))
        .and_then(|(a, b)| Some([a.parse::<f64>().ok()?, b.parse::<f64>().ok()?]))
        .unwrap_or([cw / 2.0, ch / 2.0]);
    let pan = [w as f64 / 2.0 - center[0] * zoom, h as f64 / 2.0 - center[1] * zoom];
    let view = styk_raster::View {
        matrix: camera_matrix(&p.doc, scene.camera.as_ref(), zoom, pan),
        width: w,
        height: h,
        artboard: [pan[0], pan[1], pan[0] + cw * zoom, pan[1] + ch * zoom],
        clip_to_artboard: !rest.iter().any(|a| a == "--no-clip"),
        clear: [0.16, 0.16, 0.17, 1.0],
        anti_alias: !rest.iter().any(|a| a == "--aliased"),
    };
    let started = Instant::now();
    let Some(pixmap) = styk_raster::render(&list, &images, &view) else {
        eprintln!("cannot rasterize a {w}x{h} frame");
        return 1;
    };
    let raster_ms = started.elapsed().as_secs_f64() * 1000.0;
    image::save_buffer(out, &styk_raster::straight_rgba(&pixmap), w, h, image::ColorType::Rgba8).expect("write png");
    println!(
        "{}: tick {tick}, {} prims, {} groups, opened {ms:.0} ms, assets {assets_ms:.0} ms ({} images), frame {encode_ms:.2} ms, raster {raster_ms:.1} ms → {out}",
        p.doc.name,
        list.prims.len(),
        list.groups.len() - 1,
        images.len()
    );
    0
}

fn render(path: &str, tick: &str, out: &str, rest: &[String]) -> i32 {
    use styk_eval::assets::Assets;
    use styk_eval::drawlist::encode;
    use styk_eval::view::camera_matrix;
    use styk_render::{wgpu, Renderer, ViewParams};
    let (p, ms) = match load(path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let tick: f64 = tick.parse().unwrap_or(0.0);
    let (w, h) = option(rest, "--size")
        .and_then(|s| s.split_once('x'))
        .and_then(|(a, b)| Some((a.parse::<u32>().ok()?, b.parse::<u32>().ok()?)))
        .unwrap_or((p.doc.canvas[0] + 80, p.doc.canvas[1] + 80));
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
        ..Default::default()
    })) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("No GPU adapter: {e}");
            return 1;
        }
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("styk cli"),
        required_limits: adapter.limits(),
        ..Default::default()
    }))
    .expect("device");
    let started = Instant::now();
    let assets = Assets::build(&p.doc);
    let images = styk_render::decode_images(&p.doc, &assets);
    let assets_ms = started.elapsed().as_secs_f64() * 1000.0;
    let mut renderer = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm);
    renderer.prepare_sprites(&assets);
    renderer.upload_images(&assets, &images);
    let started = Instant::now();
    let frame = p.evaluate(tick);
    let scene = build(&p.doc, &frame);
    let list = std::sync::Arc::new(encode(&p.doc, &assets, &scene));
    let encode_ms = started.elapsed().as_secs_f64() * 1000.0;
    let (cw, ch) = (p.doc.canvas[0] as f64, p.doc.canvas[1] as f64);
    let zoom = option(rest, "--zoom").and_then(|z| z.parse::<f64>().ok()).unwrap_or_else(|| ((w as f64 - 40.0) / cw).min((h as f64 - 40.0) / ch));
    let center = option(rest, "--center")
        .and_then(|s| s.split_once(','))
        .and_then(|(a, b)| Some([a.parse::<f64>().ok()?, b.parse::<f64>().ok()?]))
        .unwrap_or([cw / 2.0, ch / 2.0]);
    let pan = [w as f64 / 2.0 - center[0] * zoom, h as f64 / 2.0 - center[1] * zoom];
    // --neutral: the editor's workspace (no scene camera in the view; the camera image sits on the camera rectangle).
    let neutral = rest.iter().any(|a| a == "--neutral");
    let matrix = camera_matrix(&p.doc, if neutral { None } else { scene.camera.as_ref() }, zoom, pan);
    let view = ViewParams {
        matrix,
        artboard: [pan[0], pan[1], pan[0] + cw * zoom, pan[1] + ch * zoom],
        clip_to_artboard: !rest.iter().any(|a| a == "--no-clip"),
        clear: [0.16, 0.16, 0.17, 1.0],
        zoom,
        pan,
        camera_view: !neutral,
    };
    // --after T: the same renderer drew tick T first, as in the app, where resources outlive frames.
    if let Some(first) = option(rest, "--after").and_then(|t| t.parse::<f64>().ok()) {
        let earlier = std::sync::Arc::new(encode(&p.doc, &assets, &build(&p.doc, &p.evaluate(first))));
        renderer.set_list(&earlier);
        let mut encoder = device.create_command_encoder(&Default::default());
        renderer.render(&mut encoder, &view, [w, h]);
        queue.submit([encoder.finish()]);
    }
    renderer.set_list(&list);
    let mut encoder = device.create_command_encoder(&Default::default());
    renderer.render(&mut encoder, &view, [w, h]);
    queue.submit([encoder.finish()]);
    let (fw, fh, pixels) = styk_render::read_frame(&device, &queue, renderer.frame_texture().unwrap());
    image::save_buffer(out, &pixels, fw, fh, image::ColorType::Rgba8).expect("write png");
    println!(
        "{}: tick {tick}, {} prims, {} groups, {} passes, opened {ms:.0} ms, assets {assets_ms:.0} ms, frame {encode_ms:.2} ms → {out} ({})",
        p.doc.name,
        renderer.stats.prims,
        renderer.stats.groups,
        renderer.stats.passes,
        adapter.get_info().name
    );
    0
}

/// Compares two renders inside an optional rectangle (`--rect x0,y0,x1,y1`).
fn diff(a: &str, b: &str, out: &str, rest: &[String]) -> i32 {
    let (Ok(ia), Ok(ib)) = (image::open(a), image::open(b)) else {
        eprintln!("cannot read inputs");
        return 1;
    };
    let (ia, ib) = (ia.to_rgba8(), ib.to_rgba8());
    let (w, h) = (ia.width().min(ib.width()), ia.height().min(ib.height()));
    let rect: [u32; 4] = option(rest, "--rect")
        .and_then(|s| {
            let v: Vec<u32> = s.split(',').filter_map(|x| x.parse().ok()).collect();
            (v.len() == 4).then(|| [v[0], v[1], v[2].min(w), v[3].min(h)])
        })
        .unwrap_or([0, 0, w, h]);
    let mut img = image::RgbaImage::new(w, h);
    let (mut total, mut count, mut over8, mut over32) = (0f64, 0u64, 0u64, 0u64);
    for y in rect[1]..rect[3] {
        for x in rect[0]..rect[2] {
            let (pa, pb) = (ia.get_pixel(x, y).0, ib.get_pixel(x, y).0);
            let d = (0..3).map(|c| (pa[c] as i32 - pb[c] as i32).abs()).max().unwrap();
            total += d as f64;
            count += 1;
            if d > 8 {
                over8 += 1;
            }
            if d > 32 {
                over32 += 1;
            }
            let g = (pa[0] as u32 + pa[1] as u32 + pa[2] as u32) / 12;
            img.put_pixel(x, y, if d > 32 { image::Rgba([255, 0, 64, 255]) } else if d > 8 { image::Rgba([255, 170, 0, 255]) } else { image::Rgba([g as u8, g as u8, g as u8, 255]) });
        }
    }
    img.save(out).expect("write diff");
    let n = count.max(1) as f64;
    println!(
        "mean |Δ| {:.3}/255, {:.3}% pixels differ by >8, {:.3}% by >32 ({} px compared) → {out}",
        total / n,
        over8 as f64 * 100.0 / n,
        over32 as f64 * 100.0 / n,
        count
    );
    0
}

/// GPU time per frame (submit + wait) across a fit → 11× zoom, at 1440×900.
fn gpubench(path: &str, tick: &str, no_effects: bool) -> i32 {
    use styk_eval::assets::Assets;
    use styk_eval::drawlist::{encode_composed, Overrides};
    use styk_eval::view::camera_matrix;
    use styk_render::{wgpu, Renderer, ViewParams};
    let (p, _) = match load(path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .expect("adapter");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor { required_limits: adapter.limits(), ..Default::default() })).expect("device");
    let assets = Assets::build(&p.doc);
    let images = styk_render::decode_images(&p.doc, &assets);
    let mut renderer = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm);
    renderer.prepare_sprites(&assets);
    renderer.upload_images(&assets, &images);
    let scene = build(&p.doc, &p.evaluate(tick.parse().unwrap_or(0.0)));
    let overrides = Overrides { bypass_effects: no_effects, ..Default::default() };
    let list = std::sync::Arc::new(encode_composed(&p.doc, &assets, &scene, &overrides, &[], None));
    renderer.set_list(&list);
    let (w, h) = (1440u32, 900u32);
    let (cw, ch) = (p.doc.canvas[0] as f64, p.doc.canvas[1] as f64);
    let fit = ((w as f64 - 64.0) / cw).min((h as f64 - 64.0) / ch);
    let mut rows = Vec::new();
    for k in 0..=200 {
        let zoom = fit * (2.4 * k as f64 / 200.0).exp();
        let pan = [w as f64 / 2.0 - cw / 2.0 * zoom, h as f64 / 2.0 - ch / 2.0 * zoom];
        let view = ViewParams {
            matrix: camera_matrix(&p.doc, scene.camera.as_ref(), zoom, pan),
            artboard: [pan[0], pan[1], pan[0] + cw * zoom, pan[1] + ch * zoom],
            clip_to_artboard: true,
            clear: [0.07, 0.07, 0.09, 1.0],
            zoom,
            pan,
            camera_view: true,
        };
        let start = Instant::now();
        let mut encoder = device.create_command_encoder(&Default::default());
        renderer.render(&mut encoder, &view, [w, h]);
        queue.submit([encoder.finish()]);
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        if k > 0 {
            let s = &renderer.stats;
            rows.push((zoom / fit, start.elapsed().as_secs_f64() * 1000.0, s.passes, s.groups, s.effect_passes, s.effect_bytes + s.layer_bytes));
        }
    }
    let mut times: Vec<f64> = rows.iter().map(|r| r.1).collect();
    times.sort_by(|a, b| a.total_cmp(b));
    println!(
        "{}: {} prims, GPU frame (submit+wait) p50 {:.2} ms, p95 {:.2} ms, max {:.2} ms over fit→11x ({})",
        p.doc.name,
        list.prims.len(),
        times[times.len() / 2],
        times[times.len() * 95 / 100],
        times[times.len() - 1],
        adapter.get_info().name
    );
    for r in rows.iter().step_by(40) {
        println!("  {:5.2}x  {:6.2} ms  {} passes  {} groups  {} effect passes  {:.1} MiB layers and effect slots", r.0, r.1, r.2, r.3, r.4, r.5 as f64 / 1048576.0);
    }
    0
}
