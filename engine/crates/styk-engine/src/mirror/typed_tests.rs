use super::*;

#[test]
fn recorded_sheet_images_keep_the_exact_editor_namespace_and_scene() {
    use std::collections::HashMap;
    use styk_doc::Obj;
    use styk_edit::session::WorkerSession;
    use styk_eval::{
        assets::{Assets, decode_image},
        doc::ImageData,
    };
    fn start(engine: &Engine, before: &Value, options: &Value) -> WorkerSession {
        let history = before.get("history").unwrap();
        let n = |key| history.get(key).unwrap().as_f64().unwrap() as u64;
        let counters = RevisionCounters {
            revision: n("revision"),
            serial: n("serial"),
            saved: n("saved"),
        };
        engine.stage(0, Change::Reset).unwrap();
        engine
            .stage(
                0,
                Change::Properties(
                    before
                        .get("document")
                        .unwrap()
                        .as_str()
                        .unwrap()
                        .as_bytes()
                        .to_vec(),
                ),
            )
            .unwrap();
        engine.commit(counters.revision, 0).unwrap();
        let (_, root, loader, prepared) = engine.mirrored_for_editing().unwrap();
        let mut state = before.get("state").unwrap().as_obj().unwrap().clone();
        for (key, value) in options.as_obj().unwrap().iter() {
            state.set(key, value.clone());
        }
        let mut session = Session::start_worker_prepared(
            root,
            &json::to_string(&Value::object(state)),
            loader,
            prepared,
            counters,
        )
        .unwrap();
        session
            .prepare_worker_history(HistoryLimits::default(), 0.0, || Ok(()))
            .unwrap();
        session.into_worker()
    }
    fn entries(assets: &Assets) -> Vec<[u32; 9]> {
        assets
            .atlas
            .iter()
            .map(|entry| {
                [
                    entry.layer,
                    entry.x,
                    entry.y,
                    entry.w,
                    entry.h,
                    entry.source_w,
                    entry.source_h,
                    entry.valid as u32,
                    0,
                ]
            })
            .collect()
    }
    fn prefix(previous: &[ImageData], current: &[ImageData], context: &str) {
        assert!(
            current.len() >= previous.len(),
            "namespace shrink: {context}"
        );
        for (index, old) in previous.iter().enumerate() {
            assert!(
                Arc::ptr_eq(&old.bytes, &current[index].bytes),
                "actual image Arc prefix changed {context} index{index}"
            );
            assert_eq!(
                old.color_key, current[index].color_key,
                "color_key prefix changed {context} index{index}"
            );
        }
    }
    fn snapshot(
        images: &[ImageData],
        assets: &Assets,
        identity: &mut HashMap<usize, u64>,
    ) -> Value {
        let mut out = Vec::new();
        for (index, image) in images.iter().enumerate() {
            let address = image.bytes.as_ptr() as usize;
            let fresh = identity.len() as u64 + 1;
            let id = *identity.entry(address).or_insert(fresh);
            let mut row = Obj::new();
            row.set("index", Value::Num(index as f64));
            row.set("allocationId", Value::Num(id as f64));
            row.set("byteCount", Value::Num(image.bytes.len() as f64));
            row.set(
                "bytes",
                Value::array(
                    image
                        .bytes
                        .iter()
                        .map(|byte| Value::Num(*byte as f64))
                        .collect(),
                ),
            );
            row.set(
                "colorKey",
                image
                    .color_key
                    .map(|color| {
                        Value::array(
                            color
                                .into_iter()
                                .map(|byte| Value::Num(byte as f64))
                                .collect(),
                        )
                    })
                    .unwrap_or(Value::Null),
            );
            row.set(
                "rect",
                Value::array(
                    entries(assets)[index]
                        .into_iter()
                        .map(|value| Value::Num(value as f64))
                        .collect(),
                ),
            );
            out.push(Value::object(row));
        }
        Value::array(out)
    }
    let oracle = fixture("../../tests/typed-pairing/image-namespace.observation.json");
    let pictures = oracle.get("pictures").unwrap();
    let mut all = Vec::new();
    let mut count = 0;
    let mut full_returns = 0;
    for tape in oracle.get("tapes").unwrap().as_arr().unwrap() {
        let name = tape.get("name").unwrap().as_str().unwrap();
        let options = tape.get("options").unwrap();
        let first = tape.get("before").unwrap();
        let engine = Engine::new(clock, Budgets::for_device(None));
        let mut session = start(&engine, first, options);
        let generation = engine.generation().unwrap();
        let mut images = generation.prepared.doc.images.clone();
        let mut assets = (*generation.assets).clone();
        let mut identity = HashMap::new();
        let mut retained_images = images.clone();
        let mut rows = Vec::new();
        let mut initial = Obj::new();
        initial.set("method", Value::str("initial"));
        initial.set("namespace", snapshot(&images, &assets, &mut identity));
        rows.push(Value::object(initial));
        if name == "sheet-distinct-payload-values" {
            let before = session.report().clone();
            let revision = session.revision();
            let checkpoint = session.history_checkpoint_worker();
            let import_args = Value::array(vec![pictures.get("B").unwrap().clone()]);
            let mut checks = 0;
            let candidate = session
                .prepare_quiet("importFigure", &import_args, &Value::Null, || {
                    checks += 1;
                    Ok(())
                })
                .unwrap();
            assert_eq!(
                candidate.display().unwrap().prepared.doc.images.len(),
                images.len() + 1
            );
            drop(candidate);
            for fail in 1..=checks {
                let mut reached = 0;
                let cancelled =
                    session.prepare_quiet("importFigure", &import_args, &Value::Null, || {
                        reached += 1;
                        if reached == fail {
                            Err(styk_edit::EditError::Invalid(
                                "recorded-worker-cancel".into(),
                            ))
                        } else {
                            Ok(())
                        }
                    });
                assert!(
                    cancelled.is_err(),
                    "actual callback checkpoint {fail}/{checks}"
                );
                assert!(session.report().same_node(&before));
                assert_eq!(session.revision(), revision);
                assert_eq!(session.history_checkpoint_worker(), checkpoint);
                let current = session
                    .prepare_live(
                        "select",
                        &Value::array(vec![Value::Num(2.0)]),
                        &Value::Null,
                        || Ok(()),
                    )
                    .unwrap();
                assert_eq!(
                    current.display().unwrap().prepared.doc.images.len(),
                    images.len()
                );
                prefix(
                    &images,
                    &current.display().unwrap().prepared.doc.images,
                    "worker cancellation rolls back namespace",
                );
                drop(current);
            }
            eprintln!(
                "ACTUAL_NAMESPACE_WORKER_CANCEL every reached borrowed callback={checks}, exact namespace/root/revision/history unchanged"
            );
        }
        for (index, step) in tape
            .get("steps")
            .unwrap()
            .as_arr()
            .unwrap()
            .iter()
            .enumerate()
        {
            let call = step.get("call").unwrap().as_arr().unwrap();
            let method = call[0].as_str().unwrap();
            let after = step.get("after").unwrap();
            let context = format!("{name}:{index}:{method}");
            count += 1;
            if method == "@abortLoad" {
                assert_eq!(
                    json::to_string(session.report()),
                    after.get("document").unwrap().as_str().unwrap(),
                    "cancelled actual JS load preserves root {context}"
                );
                let mut row = Obj::new();
                row.set("method", Value::str(method));
                row.set("namespace", snapshot(&images, &assets, &mut identity));
                row.set(
                    "scope",
                    Value::str(
                        "actual JS load cancellation; Rust actor replacement remains unadmitted",
                    ),
                );
                rows.push(Value::object(row));
                continue;
            }
            if method == "@reset" {
                let old = engine.generation().unwrap();
                session = start(&engine, after, options);
                let new = engine.generation().unwrap();
                assert_ne!(
                    old.lineage, new.lineage,
                    "fresh namespace needs new acknowledged lineage"
                );
                for old_image in &images {
                    for new_image in &new.prepared.doc.images {
                        assert!(!Arc::ptr_eq(&old_image.bytes, &new_image.bytes));
                    }
                }
                images = new.prepared.doc.images.clone();
                assets = (*new.assets).clone();
                retained_images.extend(images.iter().cloned());
                let mut row = Obj::new();
                row.set("method", Value::str(method));
                row.set("namespace", snapshot(&images, &assets, &mut identity));
                row.set("newLineage", Value::Bool(true));
                rows.push(Value::object(row));
                continue;
            }
            let args = Value::array(
                call[1..]
                    .iter()
                    .map(|arg| {
                        arg.as_str()
                            .and_then(|text| text.strip_prefix('$'))
                            .and_then(|key| pictures.get(key))
                            .cloned()
                            .unwrap_or_else(|| arg.clone())
                    })
                    .collect(),
            );
            let live = matches!(
                method,
                "select" | "beginDrag" | "moveSelection" | "cancelDrag"
            );
            let prepared = if live {
                session.prepare_live(method, &args, &Value::Null, || Ok(()))
            } else {
                session.prepare_quiet(method, &args, &Value::Null, || Ok(()))
            }
            .unwrap_or_else(|error| panic!("{context}: {error}"));
            if prepared.result().get("status").and_then(Value::as_str) == Some("unported") {
                assert!(
                    (matches!(name, "native-images" | "nested-images") && method == "importFigure")
                        || (name == "sheet-images" && method == "importBackground"),
                    "unexpected refusal {context}: {}",
                    json::to_string(prepared.result())
                );
                assert!(
                    prepared.report().same_node(session.report()),
                    "atomic native import refusal"
                );
                let mut row = Obj::new();
                row.set("method", Value::str(method));
                row.set("explicitRefusal", prepared.result().clone());
                row.set("namespace", snapshot(&images, &assets, &mut identity));
                rows.push(Value::object(row));
                break;
            }
            assert_eq!(
                prepared.result().get("status").and_then(Value::as_str),
                Some("ok"),
                "{context}: {}",
                json::to_string(prepared.result())
            );
            assert_eq!(
                json::to_string(prepared.report()),
                after.get("document").unwrap().as_str().unwrap(),
                "actual full report {context}"
            );
            assert_eq!(
                prepared.publication().state(),
                after.get("canonicalState").unwrap(),
                "actual complete state {context}"
            );
            assert_eq!(
                prepared.publication().history(),
                &history_summary(after.get("history").unwrap()),
                "actual history summaries/size/counters {context}"
            );
            let display = prepared.display().unwrap();
            assert_eq!(
                json::to_string(&display.record),
                after.get("frame").unwrap().as_str().unwrap(),
                "actual full display {context}"
            );
            assert_eq!(
                json::to_string(prepared.result().get("result").unwrap()),
                step.get("result").unwrap().as_str().unwrap(),
                "actual full return {context}"
            );
            full_returns += 1;
            assert_eq!(
                prepared.result().get("events"),
                step.get("events"),
                "actual events {context}"
            );
            assert_eq!(
                prepared.revision(),
                after
                    .path(&["history", "revision"])
                    .unwrap()
                    .as_f64()
                    .unwrap() as u64,
                "revision {context}"
            );
            prefix(&images, &display.prepared.doc.images, &context);
            let next_assets = assets.extended(&display.prepared.doc);
            let old_rects = entries(&assets);
            let next_rects = entries(&next_assets);
            assert_eq!(
                &next_rects[..old_rects.len()],
                old_rects.as_slice(),
                "atlas prefix moved {context}"
            );
            let decoded = display
                .prepared
                .doc
                .images
                .iter()
                .enumerate()
                .map(|(i, _)| {
                    let image = decode_image(&display.prepared.doc, &next_assets, i).unwrap();
                    assert_eq!(image.index, i as u32);
                    Value::Num(image.index as f64)
                })
                .collect();
            let mut row = Obj::new();
            row.set("method", Value::str(method));
            row.set(
                "namespace",
                snapshot(&display.prepared.doc.images, &next_assets, &mut identity),
            );
            row.set("decodeIndices", Value::array(decoded));
            let transaction = guard(&engine);
            let admission = engine
                .stage_live_session_publication_worker(
                    7,
                    transaction,
                    &prepared,
                    prepared.result(),
                    Arc::new(std::sync::atomic::AtomicBool::new(false)),
                    || Ok(()),
                )
                .unwrap();
            let (publication, display, _) = session
                .guard(prepared, || Ok(()))
                .unwrap_or_else(|(error, _)| panic!("{context}: {error}"));
            let display = display.unwrap();
            let candidate = if options.get("layerTimeline").is_some_and(Value::truthy) {
                engine.prepare_edit_typed_publication_worker(7, transaction, publication, admission)
            } else {
                engine.prepare_edit_publication_worker(7, transaction, publication, admission)
            }
            .unwrap_or_else(|(error, _, _)| panic!("{context}: {error}"));
            // The opt-in sheet candidate uses the actual existing Editor identities.
            let candidate_images = &candidate.generation().prepared.doc.images;
            if options.get("layerTimeline").is_some_and(Value::truthy) {
                assert_eq!(candidate_images.len(), display.prepared.doc.images.len());
                prefix(
                    &display.prepared.doc.images,
                    candidate_images,
                    "exact typed image handoff",
                );
                assert_eq!(
                    entries(&candidate.generation().assets),
                    entries(&next_assets)
                );
            }
            let equal_index_bytes = candidate_images.len() == display.prepared.doc.images.len()
                && candidate_images
                    .iter()
                    .zip(&display.prepared.doc.images)
                    .all(|(a, b)| a.bytes == b.bytes && a.color_key == b.color_key);
            row.set(
                "existingCandidateEqualImageBytesByIndex",
                Value::Bool(equal_index_bytes),
            );
            row.set(
                "existingCandidateExactImageArcsByIndex",
                Value::Bool(
                    equal_index_bytes
                        && candidate_images
                            .iter()
                            .zip(&display.prepared.doc.images)
                            .all(|(a, b)| Arc::ptr_eq(&a.bytes, &b.bytes)),
                ),
            );
            let list = styk_eval::drawlist::encode(
                &display.prepared.doc,
                &next_assets,
                display.scene.scene(),
            );
            let paired_list = styk_eval::drawlist::encode(
                &candidate.generation().prepared.doc,
                &candidate.generation().assets,
                display.scene.scene(),
            );
            assert_eq!(paired_list.prims, list.prims);
            assert_eq!(paired_list.paints, list.paints);
            assert_eq!(paired_list.points, list.points);
            row.set(
                "actualSceneImagePrimitives",
                Value::Num(
                    list.prims
                        .iter()
                        .filter(|prim| prim.kind == styk_draw::K_IMAGE)
                        .count() as f64,
                ),
            );
            images = display.prepared.doc.images.clone();
            assets = next_assets;
            retained_images.extend(images.iter().cloned());
            let (_, retired, editor_retired) = candidate
                .commit_worker()
                .unwrap_or_else(|(error, _, _)| panic!("{context}: {error}"));
            drop(editor_retired);
            drop(retired);
            rows.push(Value::object(row));
        }
        let mut result = Obj::new();
        result.set("name", Value::str(name));
        result.set("steps", Value::array(rows));
        all.push(Value::object(result));
        assert!(!retained_images.is_empty());
    }
    assert_eq!(all.len(), 5);
    assert_eq!(full_returns, 38);
    eprintln!(
        "ACTUAL_NAMESPACE calls={count} strictFullReturns={full_returns}; exact image Arc/color_key and atlas/decode prefix checked, new lineage isolated. Actual admitted sheet candidates retain the existing Editor image identities."
    );
    assert_eq!(count, 45);
}

fn history_summary(history: &Value) -> Value {
    fn entry(command: &Value) -> Value {
        let mut row = styk_doc::Obj::new();
        row.set("label", command.get("label").unwrap().clone());
        row.set("bytes", command.get("bytes").unwrap().clone());
        if command.get("kind").and_then(Value::as_str) == Some("batch") {
            row.set(
                "children",
                Value::array(
                    command
                        .get("commands")
                        .unwrap()
                        .as_arr()
                        .unwrap()
                        .iter()
                        .map(entry)
                        .collect(),
                ),
            );
        }
        Value::object(row)
    }
    let mut row = styk_doc::Obj::new();
    row.set("size", history.get("size").unwrap().clone());
    for key in ["undo", "redo"] {
        let parsed = parsed(history.get(key).unwrap().as_str().unwrap());
        row.set(
            key,
            Value::array(parsed.as_arr().unwrap().iter().map(entry).collect()),
        );
    }
    row.set("revision", history.get("revision").unwrap().clone());
    row.set("revisionSerial", history.get("serial").unwrap().clone());
    row.set("savedRevision", history.get("saved").unwrap().clone());
    Value::object(row)
}

fn worker_from(
    engine: &Engine,
    before: &Value,
    options: &Value,
) -> styk_edit::session::WorkerSession {
    let history = before.get("history").unwrap();
    let n = |key| history.get(key).unwrap().as_f64().unwrap() as u64;
    let counters = RevisionCounters {
        revision: n("revision"),
        serial: n("serial"),
        saved: n("saved"),
    };
    engine.stage(0, Change::Reset).unwrap();
    engine
        .stage(
            0,
            Change::Properties(
                before
                    .get("document")
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .as_bytes()
                    .to_vec(),
            ),
        )
        .unwrap();
    engine.commit(counters.revision, 0).unwrap();
    let (_, root, loader, prepared) = engine.mirrored_for_editing().unwrap();
    let mut state = before.get("state").unwrap().as_obj().unwrap().clone();
    for (key, value) in options.as_obj().unwrap().iter() {
        state.set(key, value.clone());
    }
    let mut session = Session::start_worker_prepared(
        root,
        &json::to_string(&Value::object(state)),
        loader,
        prepared,
        counters,
    )
    .unwrap();
    session
        .prepare_worker_history(HistoryLimits::default(), 0.0, || Ok(()))
        .unwrap();
    session.into_worker()
}

#[test]
fn actual_imported_editor_glyphs_encode_with_the_paired_candidate_assets() {
    let oracle = fixture("../../tests/typed-pairing/glyph-import.observation.json");
    let engine = Engine::new(clock, Budgets::for_device(None));
    let mut session = worker_from(
        &engine,
        oracle.get("before").unwrap(),
        oracle.get("options").unwrap(),
    );
    let old = engine.generation().unwrap();
    let call = oracle.get("call").unwrap().as_arr().unwrap();
    let prepared = session
        .prepare_quiet(
            call[0].as_str().unwrap(),
            &Value::array(call[1..].to_vec()),
            &Value::Null,
            || Ok(()),
        )
        .unwrap();
    assert_eq!(
        prepared.result().get("status").and_then(Value::as_str),
        Some("ok")
    );
    let after = oracle.get("after").unwrap();
    assert_eq!(
        json::to_string(prepared.report()),
        after.get("document").unwrap().as_str().unwrap()
    );
    assert_eq!(
        json::to_string(&prepared.display().unwrap().record),
        after.get("frame").unwrap().as_str().unwrap()
    );
    assert_eq!(
        json::to_string(prepared.result().get("result").unwrap()),
        after.get("resultJSON").unwrap().as_str().unwrap()
    );
    assert_eq!(prepared.result().get("events"), after.get("events"));
    assert_eq!(
        prepared.publication().state(),
        after.get("canonicalState").unwrap()
    );
    let transaction = guard(&engine);
    let admission = engine
        .stage_live_session_publication_worker(
            7,
            transaction,
            &prepared,
            prepared.result(),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            || Ok(()),
        )
        .unwrap();
    let (publication, display, _) = session
        .guard(prepared, || Ok(()))
        .unwrap_or_else(|(error, _)| panic!("{error}"));
    let display = display.unwrap();
    let candidate = engine
        .prepare_edit_typed_publication_worker(7, transaction, publication, admission)
        .unwrap_or_else(|(error, _, _)| panic!("{error}"));
    assert!(candidate.stores_generation());
    assert!(Arc::ptr_eq(&old, &engine.generation().unwrap()));
    let mut glyphs = 0;
    for figure in &display.scene.scene().figures {
        for text in &figure.texts {
            for glyph in &text.glyphs {
                glyphs += 1;
                assert!(
                    candidate.generation().assets.glyph(glyph).is_some(),
                    "the actual Editor glyph Arc must encode"
                );
            }
        }
    }
    assert!(glyphs > 0);
    for (actual, canonical) in display
        .prepared
        .doc
        .figures
        .iter()
        .zip(&candidate.generation().prepared.doc.figures)
    {
        assert!(Arc::ptr_eq(actual, canonical));
    }
    let expected = styk_eval::assets::Assets::build(&display.prepared.doc);
    let list = styk_eval::drawlist::encode(&display.prepared.doc, &expected, display.scene.scene());
    let paired = styk_eval::drawlist::encode(
        &candidate.generation().prepared.doc,
        &candidate.generation().assets,
        display.scene.scene(),
    );
    assert_eq!(paired.prims, list.prims);
    assert_eq!(paired.paints, list.paints);
    assert_eq!(paired.points, list.points);
    assert_eq!(paired.glyphs.len(), list.glyphs.len());
    for (a, b) in paired.glyphs.iter().zip(&list.glyphs) {
        assert_eq!(a.triangles.as_ref(), b.triangles.as_ref());
    }
    let wrong =
        styk_eval::drawlist::encode(&display.prepared.doc, &old.assets, display.scene.scene());
    assert!(wrong.prims.len() < list.prims.len());
    let id = candidate.generation().id;
    let typed = candidate.generation().prepared.clone();
    let assets = candidate.generation().assets.clone();
    let (result, counts) =
        crate::allocation::watch(&engine.inner.state as *const _ as usize, || {
            candidate.commit_worker()
        });
    let (done, retired, editor) = result.unwrap_or_else(|(error, _, _)| panic!("{error}"));
    assert_eq!((counts.allocations, counts.deallocations), (0, 0));
    assert_eq!(done.generation, id);
    assert!(retired.typed_loader.is_some());
    let published = engine.generation().unwrap();
    assert!(Arc::ptr_eq(&typed, &published.prepared));
    assert!(Arc::ptr_eq(&assets, &published.assets));
    {
        let mirror = engine.inner.mirror.lock().unwrap();
        assert!(Arc::ptr_eq(mirror.prepared.as_ref().unwrap(), &typed));
        assert_eq!(mirror.loader.images.len(), typed.doc.images.len());
    }
    drop((editor, retired));
    assert!(
        engine
            .prepare_record_publication_worker(done.revision, 7, transaction)
            .is_err()
    );
}

#[test]
fn typed_image_prefix_checks_actual_arcs_and_nonempty_color_keys() {
    use styk_eval::doc::ImageData;
    let first = ImageData {
        bytes: Arc::from([1u8, 2, 3]),
        color_key: Some([10, 20, 30]),
    };
    let before = vec![first.clone()];
    let second = ImageData {
        bytes: Arc::from([4u8, 5]),
        color_key: None,
    };
    assert!(image_prefix(&before, &[first.clone(), second.clone()]));
    let mut changed = first.clone();
    changed.color_key = Some([11, 20, 30]);
    assert!(!image_prefix(&before, &[changed]));
    let mut changed = first.clone();
    changed.color_key = None;
    assert!(!image_prefix(&before, &[changed]));
    let changed = ImageData {
        bytes: Arc::from([1u8, 2, 3]),
        color_key: first.color_key,
    };
    assert!(!image_prefix(&before, &[changed]));
    assert!(!image_prefix(&before, &[second, first]));
    assert!(!image_prefix(&before, &[]));
}

#[test]
fn typed_after_cache_cancellation_stale_lineage_and_drop_preserve_the_acknowledged_loader() {
    let oracle = fixture("../../tests/typed-pairing/glyph-import.observation.json");
    for action in ["cancel", "stop", "stale", "lineage", "drop"] {
        let engine = Engine::new(clock, Budgets::for_device(None));
        let mut session = worker_from(
            &engine,
            oracle.get("before").unwrap(),
            oracle.get("options").unwrap(),
        );
        let old = engine.generation().unwrap();
        let root = session.report().clone();
        let epoch = session.operation_epoch();
        let call = oracle.get("call").unwrap().as_arr().unwrap();
        let prepared = session
            .prepare_quiet(
                call[0].as_str().unwrap(),
                &Value::array(call[1..].to_vec()),
                &Value::Null,
                || Ok(()),
            )
            .unwrap();
        let transaction = guard(&engine);
        let admission = engine
            .stage_live_session_publication_worker(
                7,
                transaction,
                &prepared,
                prepared.result(),
                Arc::new(std::sync::atomic::AtomicBool::new(false)),
                || Ok(()),
            )
            .unwrap();
        let (publication, display, _) = session
            .guard(prepared, || Ok(()))
            .unwrap_or_else(|(error, _)| panic!("{error}"));
        let display = display.unwrap();
        let candidate = engine
            .prepare_edit_typed_publication_worker(7, transaction, publication, admission)
            .unwrap_or_else(|(error, _, _)| panic!("{error}"));
        let encoded = styk_eval::drawlist::encode(
            &candidate.generation().prepared.doc,
            &candidate.generation().assets,
            display.scene.scene(),
        );
        assert!(!encoded.prims.is_empty());
        match action {
            "cancel" => engine.discard_batch(7),
            "stop" => engine.stop(),
            "stale" | "lineage" => {
                engine.publish(Generation {
                    id: 0,
                    lineage: old.lineage + u64::from(action == "lineage"),
                    revision: old.revision + 1,
                    prepared: old.prepared.clone(),
                    assets: old.assets.clone(),
                    open_ms: 0.0,
                    anchors: None,
                });
            }
            _ => {}
        }
        if action == "drop" {
            drop(candidate);
        } else {
            let (result, counts) =
                crate::allocation::watch(&engine.inner.state as *const _ as usize, || {
                    candidate.commit_worker()
                });
            assert_eq!((counts.allocations, counts.deallocations), (0, 0));
            assert!(result.is_err());
            drop(result);
        }
        assert!(session.report().same_node(&root));
        assert_eq!(session.operation_epoch(), epoch);
        let mirror = engine.inner.mirror.lock().unwrap();
        assert!(Arc::ptr_eq(
            mirror.prepared.as_ref().unwrap(),
            &old.prepared
        ));
        let mut loader = mirror.loader.clone();
        let converted = loader
            .document(&mirror.tree, &old.prepared.doc.name)
            .unwrap();
        assert_eq!(loader.converted.figures, 0);
        for (a, b) in converted.figures.iter().zip(&old.prepared.doc.figures) {
            assert!(
                Arc::ptr_eq(a, b),
                "{action} changed acknowledged loader identity"
            );
        }
    }
}

#[test]
fn typed_capability_refuses_cold_mismatched_and_foreign_after_caches_without_changing_heads() {
    use styk_edit::{Editor, Options};
    use styk_eval::eval::Prepared;
    let oracle = fixture("../../tests/typed-pairing/image-namespace.observation.json");
    let tape = oracle
        .get("tapes")
        .unwrap()
        .as_arr()
        .unwrap()
        .iter()
        .find(|tape| tape.get("name").and_then(Value::as_str) == Some("sheet-images"))
        .unwrap();
    for action in [
        "cold",
        "mismatched-root",
        "canvas",
        "name",
        "layers",
        "budget",
        "image-key",
        "image-arc",
        "image-reorder",
    ] {
        let engine = Engine::new(clock, Budgets::for_device(None));
        let original = worker_from(
            &engine,
            tape.get("before").unwrap(),
            tape.get("options").unwrap(),
        );
        let old = engine.generation().unwrap();
        let (_, root, loader, prepared) = engine.mirrored_for_editing().unwrap();
        let mut editor = Editor::new(
            root.clone(),
            Options {
                layer_timeline: true,
                property_channels: true,
                auto_key: false,
            },
        );
        editor.set_revision_counters(old.revision, old.revision, old.revision);
        editor
            .prepare_worker_history(HistoryLimits::default(), 0.0, || Ok(()))
            .unwrap();
        if action != "cold" {
            editor.adopt_prepared(loader.clone(), prepared.clone());
        }
        let before_state = editor.state_value();
        let before_history = editor.history_size();
        let candidate = editor
            .prepare_worker_publication(
                |editor, _| {
                    if action == "mismatched-root" {
                        editor.call("setFPS", &[Value::Num(30.0)])?;
                    } else if action != "cold" {
                        let mut document = (*prepared.doc).clone();
                        let mut adopted = loader.clone();
                        match action {
                            "canvas" => document.canvas[0] += 1,
                            "name" => document.name.push_str(" foreign"),
                            "layers" => {
                                let layer = document.layers.as_mut().unwrap();
                                Arc::make_mut(layer).hidden.push("foreign".into());
                            }
                            "budget" => {
                                adopted = styk_eval::load::Loader::new();
                                document.scene_budget = adopted.scene_budget().clone();
                            }
                            "image-key" => adopted.images[0].color_key = Some([1, 2, 3]),
                            "image-arc" => {
                                adopted.images[0].bytes =
                                    Arc::from(adopted.images[0].bytes.as_ref())
                            }
                            "image-reorder" => {
                                let extra = adopted.images[0].clone();
                                adopted.images.insert(0, extra);
                            }
                            _ => {}
                        }
                        editor.adopt_prepared(adopted, Arc::new(Prepared::new(Arc::new(document))));
                    }
                    Ok(Value::Null)
                },
                || Ok(()),
            )
            .unwrap();
        let transaction = guard(&engine);
        let admission = engine
            .stage_edit_publication_worker(7, transaction, &candidate, || Ok(()))
            .unwrap();
        let publication = candidate
            .guard_worker(&mut editor)
            .unwrap_or_else(|(error, _)| panic!("{error}"));
        let refused =
            engine.prepare_edit_typed_publication_worker(7, transaction, publication, admission);
        assert!(refused.is_err(), "foreign cache admitted: {action}");
        drop(refused);
        engine.discard_batch(7);
        assert!(editor.report().same_node(&root));
        assert_eq!(editor.state_value(), before_state);
        assert_eq!(editor.history_size(), before_history);
        assert!(Arc::ptr_eq(&old, &engine.generation().unwrap()));
        let mirror = engine.inner.mirror.lock().unwrap();
        assert!(mirror.tree.same_node(&root));
        assert!(Arc::ptr_eq(
            mirror.prepared.as_ref().unwrap(),
            &old.prepared
        ));
        let mut surviving = mirror.loader.clone();
        let document = surviving.document(&root, &old.prepared.doc.name).unwrap();
        assert_eq!(surviving.converted.figures, 0);
        for (a, b) in document.figures.iter().zip(&old.prepared.doc.figures) {
            assert!(Arc::ptr_eq(a, b), "{action} altered the owning Loader");
        }
        drop(original);
    }
}

#[test]
fn typed_cancel_after_actual_scene_encode_uses_the_same_final_state_linearization() {
    use std::sync::Barrier;
    let oracle = fixture("../../tests/typed-pairing/glyph-import.observation.json");
    for before_exchange in [true, false] {
        let engine = Engine::new(clock, Budgets::for_device(None));
        let session = worker_from(
            &engine,
            oracle.get("before").unwrap(),
            oracle.get("options").unwrap(),
        );
        let old = engine.generation().unwrap();
        let original = engine.mirrored().unwrap().1;
        let barriers = Arc::new([Barrier::new(2), Barrier::new(2)]);
        let old_revision = old.revision;
        let pointer = &engine.inner.state as *const _ as usize;
        let worker = engine.clone();
        let wait = barriers.clone();
        let args = oracle.get("call").unwrap().as_arr().unwrap()[1..].to_vec();
        let thread = std::thread::spawn(move || {
            let mut session = session;
            let prepared = session
                .prepare_quiet("importFigure", &Value::array(args), &Value::Null, || Ok(()))
                .unwrap();
            let transaction = guard(&worker);
            let admission = worker
                .stage_live_session_publication_worker(
                    7,
                    transaction,
                    &prepared,
                    prepared.result(),
                    Arc::new(std::sync::atomic::AtomicBool::new(false)),
                    || Ok(()),
                )
                .unwrap();
            let (publication, display, _) = session
                .guard(prepared, || Ok(()))
                .unwrap_or_else(|(error, _)| panic!("{error}"));
            let display = display.unwrap();
            let candidate = worker
                .prepare_edit_typed_publication_worker(7, transaction, publication, admission)
                .unwrap_or_else(|(error, _, _)| panic!("{error}"));
            let encoded = styk_eval::drawlist::encode(
                &candidate.generation().prepared.doc,
                &candidate.generation().assets,
                display.scene.scene(),
            );
            assert!(!encoded.prims.is_empty());
            let id = candidate.generation().id;
            let commit = || crate::allocation::watch(pointer, || candidate.commit_worker());
            let (result, counts) = if before_exchange {
                crate::allocation::with_pause(pointer, 1, wait, commit)
            } else {
                let result = commit();
                wait[0].wait();
                wait[1].wait();
                result
            };
            assert_eq!((counts.allocations, counts.deallocations), (0, 0));
            assert_eq!(result.is_ok(), !before_exchange);
            drop(result);
            assert_eq!(session.revision() == old_revision, before_exchange);
            id
        });
        barriers[0].wait();
        engine.discard_batch(7);
        barriers[1].wait();
        let id = thread.join().unwrap();
        assert_eq!(
            engine.generation().unwrap().id,
            if before_exchange { old.id } else { id }
        );
        assert_eq!(
            engine.mirrored().unwrap().1.same_node(&original),
            before_exchange
        );
        while engine.step() {}
    }
}

#[test]
fn typed_scope_refuses_malformed_markers_and_empty_sheet_via_the_same_schema_owner() {
    use styk_edit::{Editor, Options};
    let oracle = fixture("../../tests/typed-pairing/image-namespace.observation.json");
    let tape = oracle
        .get("tapes")
        .unwrap()
        .as_arr()
        .unwrap()
        .iter()
        .find(|tape| tape.get("name").and_then(Value::as_str) == Some("sheet-images"))
        .unwrap();
    let initial = parsed(
        tape.get("before")
            .unwrap()
            .get("document")
            .unwrap()
            .as_str()
            .unwrap(),
    );
    let revision = tape
        .path(&["before", "history", "revision"])
        .unwrap()
        .as_f64()
        .unwrap() as u64;
    let mut observations = Vec::new();
    for (name, marker) in [
        ("null", Value::Null),
        ("false", Value::Bool(false)),
        ("number", Value::Num(17.0)),
        ("string", Value::str("not a sheet")),
        ("array", Value::array(Vec::new())),
        ("empty-object", Value::object(styk_doc::Obj::new())),
    ] {
        let mut report = initial.clone();
        report
            .make_obj()
            .unwrap()
            .get_mut("_styk")
            .unwrap()
            .make_obj()
            .unwrap()
            .set("layerSheet", marker.clone());
        let engine = Engine::new(clock, Budgets::for_device(None));
        engine.stage(0, Change::Reset).unwrap();
        engine
            .stage(0, Change::Properties(json::to_string(&report).into_bytes()))
            .unwrap();
        engine.commit(revision, 0).unwrap();
        let old = engine.generation().unwrap();
        let (_, root, loader, ready) = engine.mirrored_for_editing().unwrap();
        let typed_sheet = ready.doc.sheet.is_some();
        let mut editor = Editor::new(
            root.clone(),
            Options {
                layer_timeline: true,
                property_channels: true,
                auto_key: false,
            },
        );
        editor.adopt_prepared(loader, ready);
        editor.set_revision_counters(revision, revision, revision);
        editor
            .prepare_worker_history(HistoryLimits::default(), 0.0, || Ok(()))
            .unwrap();
        let before_state = editor.state_value();
        let checkpoint = editor.history_size();
        let prepared = editor
            .prepare_worker_publication(|_, _| Ok(Value::Null), || Ok(()))
            .unwrap();
        assert!(prepared.report().same_node(&root));
        let before_history = prepared.before_history().clone();
        let transaction = guard(&engine);
        let admission = engine
            .stage_edit_publication_worker(7, transaction, &prepared, || Ok(()))
            .unwrap();
        let publication = prepared
            .guard_worker(&mut editor)
            .unwrap_or_else(|(error, _)| panic!("{error}"));
        let candidate =
            engine.prepare_edit_typed_publication_worker(7, transaction, publication, admission);
        let admitted = candidate.is_ok();
        drop(candidate);
        let mut row = styk_doc::Obj::new();
        row.set("marker", Value::str(name));
        row.set("raw", marker);
        row.set("typedSheetSome", Value::Bool(typed_sheet));
        row.set("admitted", Value::Bool(admitted));
        observations.push(Value::object(row));
        engine.discard_batch(7);
        assert!(editor.report().same_node(&root));
        assert_eq!(editor.state_value(), before_state);
        assert_eq!(editor.history_size(), checkpoint);
        assert!(Arc::ptr_eq(&old, &engine.generation().unwrap()));
        let mirror = engine.inner.mirror.lock().unwrap();
        assert!(mirror.tree.same_node(&root));
        assert!(Arc::ptr_eq(
            mirror.prepared.as_ref().unwrap(),
            &old.prepared
        ));
        let mut after_loader = mirror.loader.clone();
        let after = after_loader
            .document(&root, &old.prepared.doc.name)
            .unwrap();
        assert_eq!(after_loader.converted.figures, 0);
        for (a, b) in after.figures.iter().zip(&old.prepared.doc.figures) {
            assert!(Arc::ptr_eq(a, b));
        }
        drop(mirror);
        let unchanged = editor
            .prepare_worker_publication(|_, _| Ok(Value::Null), || Ok(()))
            .unwrap();
        assert_eq!(unchanged.before_history(), &before_history);
        drop(unchanged);
    }
    let result = Value::array(observations);
    if let Ok(path) = std::env::var("STYK_TYPED_SCOPE_OBSERVATION") {
        std::fs::write(path, json::to_string(&result) + "\n").unwrap();
    }
    assert!(
        result
            .as_arr()
            .unwrap()
            .iter()
            .all(|row| row.get("admitted") == Some(&Value::Bool(false))),
        "malformed marker incorrectly admitted: {}",
        json::to_string(&result)
    );
}

#[test]
fn typed_scope_requires_the_same_owning_cache_to_have_a_prepared_sheet() {
    use styk_edit::{Editor, Options};
    let oracle = fixture("../../tests/typed-pairing/image-namespace.observation.json");
    let tape = oracle
        .get("tapes")
        .unwrap()
        .as_arr()
        .unwrap()
        .iter()
        .find(|tape| tape.get("name").and_then(Value::as_str) == Some("sheet-images"))
        .unwrap();
    let engine = Engine::new(clock, Budgets::for_device(None));
    let _original = worker_from(
        &engine,
        tape.get("before").unwrap(),
        tape.get("options").unwrap(),
    );
    let old = engine.generation().unwrap();
    let (_, root, loader, ready) = engine.mirrored_for_editing().unwrap();
    assert!(root.path(&["_styk", "layerSheet"]).unwrap().is_object());
    assert!(ready.doc.sheet.is_some());
    let mut document = (*ready.doc).clone();
    document.sheet = None;
    let mut editor = Editor::new(
        root.clone(),
        Options {
            layer_timeline: true,
            property_channels: true,
            auto_key: false,
        },
    );
    editor.adopt_prepared(
        loader,
        Arc::new(styk_eval::eval::Prepared::new(Arc::new(document))),
    );
    editor
        .prepare_worker_history(HistoryLimits::default(), 0.0, || Ok(()))
        .unwrap();
    let state = editor.state_value();
    let prepared = editor
        .prepare_worker_publication(|_, _| Ok(Value::Null), || Ok(()))
        .unwrap();
    let publication = prepared
        .guard_worker(&mut editor)
        .unwrap_or_else(|(error, _)| panic!("{error}"));
    assert!(
        publication.typed_cache_worker().is_err(),
        "raw sheet object cannot grant a capability with typed sheetNone"
    );
    drop(publication);
    assert!(editor.report().same_node(&root));
    assert_eq!(editor.state_value(), state);
    assert!(Arc::ptr_eq(&old, &engine.generation().unwrap()));
}
