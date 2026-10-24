//! Documents entering the engine: opened from JSON text and from a file, normalized as the editor's load
//! does, published, edited by a session whose changes the published document follows, and refused with the
//! message the interface shows.
use styk_doc::{Value, json};
use styk_engine::document::{OpenOptions, OpenSource};
use styk_engine::{Budgets, Engine};

fn clock() -> f64 {
    0.0
}

fn engine() -> Engine {
    Engine::new(clock, Budgets::for_device(None))
}

fn repo(path: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..").join(path)
}

const APP: OpenOptions = OpenOptions { layer_timeline: true };
const STATE: &str = r#"{"propertyChannels":true,"layerTimeline":true,"tick":0,"frameIndex":0,"layerRevision":1,"nextFrameIdentity":2,"nextUid":2}"#;

#[test]
fn a_json_document_opens_normalized_and_is_published() {
    let engine = engine();
    let text = std::fs::read(repo("engine/tests/edit/startup.document.json")).unwrap();
    let opened = engine.open_document(OpenSource::Json { bytes: &text, name: "Untitled" }, APP, &|| false).unwrap();
    assert_eq!(opened.kind, "json");
    assert_eq!(opened.indexes.next_uid, 2.0);
    assert!(opened.tree.path(&["_styk", "layerSheet"]).is_some());
    assert!(engine.generation().is_none(), "opening publishes nothing");
    let id = engine.activate(&opened.typed, &opened.tree, 7).unwrap().generation;
    let generation = engine.generation().expect("the document is published");
    assert_eq!((generation.id, generation.revision), (id, 7));
    assert_eq!(generation.prepared.doc.keyframes.len(), 1);
}

#[test]
fn a_file_opens_like_the_document_it_holds() {
    let engine = engine();
    let folder = repo("engine/tests/containers");
    let mut opened_any = false;
    for entry in std::fs::read_dir(folder).unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".styk") {
            continue;
        }
        let bytes = std::fs::read(entry.path()).unwrap();
        // A file may be refused (a figure file is no project); one that opens must publish.
        if let Ok(opened) = engine.open_document(OpenSource::File { bytes: &bytes, name: &name }, APP, &|| false) {
            opened_any = true;
            assert_eq!(opened.kind, "styk", "{name}");
            engine.activate(&opened.typed, &opened.tree, 1).unwrap();
            assert!(engine.generation().unwrap().prepared.doc.keyframes.len() >= 1, "{name}");
        }
    }
    assert!(opened_any, "the container fixtures hold a project");
}

#[test]
fn the_published_document_follows_the_session() {
    let engine = engine();
    let text = std::fs::read(repo("engine/tests/edit/startup.document.json")).unwrap();
    let opened = engine.open_document(OpenSource::Json { bytes: &text, name: "Untitled" }, APP, &|| false).unwrap();
    engine.activate(&opened.typed, &opened.tree, 1).unwrap();
    let before = engine.generation().unwrap();
    let mut session = opened.start_session(STATE).unwrap();
    let answer = json::parse(session.call("setFPS", "[30]", "null").as_bytes()).unwrap();
    assert_eq!(answer.get("status").and_then(Value::as_str), Some("ok"), "{answer}");
    let committed = engine.publish_tree(opened.lineage(), session.report().clone(), session.revision()).unwrap().expect("nothing reads the document");
    let after = engine.generation().unwrap();
    assert_eq!(after.id, committed.generation);
    assert_ne!(after.id, before.id);
    assert_eq!(after.lineage, before.lineage, "an edit stays in the document's lineage");
    assert_eq!(after.prepared.doc.fps, 30.0);
    assert_eq!(before.prepared.doc.fps, 24.0);
    assert_eq!(committed.converted.keyframes, 0, "no keyframe was converted again");
    assert_eq!(engine.stored().unwrap().1.get("fps").and_then(Value::as_f64), Some(30.0));
}

#[test]
fn a_document_that_cannot_open_says_why() {
    let engine = engine();
    let refused = |source: OpenSource| engine.open_document(source, APP, &|| false).err().expect("refused");
    let broken = refused(OpenSource::Json { bytes: b"{not json", name: "x" });
    assert_eq!(broken.key.as_deref(), Some("errors.importWorker.unreadableProject"));
    let mut document = json::parse(&std::fs::read(repo("engine/tests/edit/startup.document.json")).unwrap()).unwrap();
    document.make_obj().unwrap().set("figure_types", Value::str("not a table"));
    let bytes = json::to_string(&document).into_bytes();
    let damaged = refused(OpenSource::Json { bytes: &bytes, name: "x" });
    assert!(damaged.key.is_some(), "a damaged project is refused with a message key: {damaged:?}");
    assert!(damaged.to_json().contains("\"messageKey\""));
    let unsupported = refused(OpenSource::File { bytes: b"abc", name: "notes.txt" });
    assert!(unsupported.message.contains("Choose a"), "{}", unsupported.message);
    let piv = refused(OpenSource::File { bytes: b"definitely not a pivot file", name: "x.piv" });
    assert_eq!(piv.code.as_deref(), Some("invalid_project"));
    assert_eq!(piv.status, Some(422));
    let cancelled = engine.open_document(OpenSource::Json { bytes: &json::to_string(&document).into_bytes(), name: "x" }, APP, &|| true).err().expect("cancelled");
    assert_eq!(cancelled.code.as_deref(), Some("cancelled"));
}
