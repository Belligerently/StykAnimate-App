//! Documents entering the engine, and the published document following the session's.
//!
//! A document arrives as a file's bytes (Pivot `.piv`, a native `.styk`, …) or as JSON text (a new document,
//! a recovery copy, an import the interface decoded). [`Engine::open_document`] decodes it, normalizes it as
//! the editor's load does (`styk_edit::load`), and converts it into the typed views evaluation reads, all on
//! the calling thread: the interface hands it to a worker (`Engine::spawn`). The result holds the document's
//! tree, the indexes the interface adopts and the typed views, ready to be published ([`Engine::activate`])
//! and edited (`styk_edit::session::Session::start_prepared`).
//!
//! Once a session edits the document, [`Engine::publish_tree`] brings the published document to the
//! session's tree after each call: only the records the call replaced are converted again, as the tree is
//! shared copy-on-write.

use std::sync::Arc;

use styk_doc::Value;
use styk_eval::assets::Assets;
use styk_eval::eval::Prepared;
use styk_eval::load::Loader;

use crate::store::Committed;
use crate::{Engine, Generation};

/// What the interface knows about the editor that opens the document.
#[derive(Clone, Copy, Debug, Default)]
pub struct OpenOptions {
    /// The editor gives every document a layer sheet (the app's editor does).
    pub layer_timeline: bool,
}

/// Where a document comes from.
pub enum OpenSource<'a> {
    /// A file: its bytes and its name (the extension says what kind of file it is).
    File { bytes: &'a [u8], name: &'a str },
    /// A document as JSON text, and the name it is shown under.
    Json { bytes: &'a [u8], name: &'a str },
}

/// How far an open got.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenStage {
    /// The file could not be read.
    Decode,
    /// The document was read but cannot be edited or drawn.
    Prepare,
}

/// Why a document did not open, as the interface words it: a message key with its parameters where the
/// editor has one, else the plain message.
#[derive(Clone, Debug)]
pub struct OpenError {
    pub message: String,
    pub key: Option<String>,
    pub params: Vec<(String, String)>,
    /// The machine code the interface branches on (`invalid_project`, `file_too_large`, …).
    pub code: Option<String>,
    /// The status the Pivot codec answers a bad file with.
    pub status: Option<u16>,
    pub stage: OpenStage,
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.key.as_deref().unwrap_or(&self.message))
    }
}

impl std::error::Error for OpenError {}

/// The key of the message shown for a document the load cannot make sense of.
const UNREADABLE: &str = "errors.importWorker.unreadableProject";

impl OpenError {
    fn decode(message: impl Into<String>) -> OpenError {
        OpenError { message: message.into(), key: None, params: Vec::new(), code: None, status: None, stage: OpenStage::Decode }
    }

    fn unreadable() -> OpenError {
        OpenError { key: Some(UNREADABLE.into()), ..OpenError::decode("The project is not readable.") }
    }

    fn from_codec(error: styk_piv::CodecError) -> OpenError {
        OpenError { code: Some(error.code), status: Some(error.status), ..OpenError::decode(error.message) }
    }

    fn from_file(error: styk_formats::files::FileError) -> OpenError {
        let params = error.kind.map(|kind| vec![("kind".to_string(), kind.to_string())]).unwrap_or_default();
        OpenError { key: Some(error.message_key.into()), params, code: error.code.map(str::to_string), ..OpenError::decode(error.detail) }
    }

    fn from_edit(error: styk_edit::EditError) -> OpenError {
        match error {
            styk_edit::EditError::Invalid(text) if text == styk_edit::load::TYPE_ERROR => OpenError { stage: OpenStage::Prepare, ..OpenError::unreadable() },
            styk_edit::EditError::Invalid(text) if text.starts_with("errors.") => OpenError { key: Some(text.clone()), stage: OpenStage::Prepare, ..OpenError::decode(text) },
            other => OpenError { stage: OpenStage::Prepare, ..OpenError::decode(other.to_string()) },
        }
    }

    /// The error as JSON: `{message, messageKey?, messageParams?, code?, status?, stage}`.
    pub fn to_json(&self) -> String {
        let text = |value: &str| styk_doc::json::to_string(&Value::str(value));
        let mut out = format!("{{\"message\":{},\"stage\":\"{}\"", text(&self.message), if self.stage == OpenStage::Decode { "decode" } else { "prepare" });
        if let Some(key) = &self.key {
            out.push_str(&format!(",\"messageKey\":{}", text(key)));
            if !self.params.is_empty() {
                let params: Vec<String> = self.params.iter().map(|(name, value)| format!("{}:{}", text(name), text(value))).collect();
                out.push_str(&format!(",\"messageParams\":{{{}}}", params.join(",")));
            }
        }
        if let Some(code) = &self.code {
            out.push_str(&format!(",\"code\":{}", text(code)));
        }
        if let Some(status) = self.status {
            out.push_str(&format!(",\"status\":{status}"));
        }
        out.push('}');
        out
    }
}

/// The typed views of a document and the image atlas that belong to it: what publishing it needs.
pub struct Typed {
    /// The tree the views were made from.
    pub(crate) tree: Value,
    pub(crate) loader: Loader,
    pub(crate) prepared: Arc<Prepared>,
    pub(crate) assets: Arc<Assets>,
    pub(crate) lineage: u64,
}

/// A document ready to be published and edited.
pub struct OpenedDocument {
    /// The normalized document, exactly as the interface's read view will hold it.
    pub tree: Value,
    /// The indexes the editor adopts with it (its `report` is empty: the tree is above).
    pub indexes: styk_edit::load::Prepared,
    /// The name the document is shown under.
    pub name: String,
    /// What kind of file it came from (`piv`, `styk`, …; `json` for JSON text).
    pub kind: &'static str,
    /// The document's `version`.
    pub version: f64,
    /// The typed views and image atlas of the tree.
    pub typed: Typed,
}

/// The extension of a file name, lower case.
fn extension(name: &str) -> &str {
    name.rsplit_once('.').map_or("", |(_, extension)| extension)
}

impl Engine {
    /// Opens a document: decodes it, normalizes it as the editor's load does and builds its typed views. Runs
    /// on the calling thread, for as long as the document takes; `cancel` is polled where the Pivot reader
    /// checks (between frames).
    pub fn open_document(&self, source: OpenSource<'_>, options: OpenOptions, cancel: &dyn Fn() -> bool) -> Result<OpenedDocument, OpenError> {
        let (document, kind, version, name) = match source {
            OpenSource::File { bytes, name } => {
                let extension = extension(name).to_ascii_lowercase();
                match extension.as_str() {
                    "piv" | "stk" => {
                        let decoded = styk_piv::decode_document(bytes, name, cancel).map_err(OpenError::from_codec)?;
                        (decoded.document, decoded.kind, decoded.version, decoded.filename)
                    }
                    other => {
                        let Some(kind) = styk_formats::Kind::from_extension(other) else {
                            return Err(OpenError::decode("Choose a .styk project, .stykmov movieclip, .stkz figure, .piv, or .stk file."));
                        };
                        let document = styk_formats::files::open_document(bytes, kind).map_err(OpenError::from_file)?;
                        let version = document.get("version").and_then(Value::as_f64).unwrap_or(5.0);
                        (document, kind.name(), version, name.to_string())
                    }
                }
            }
            OpenSource::Json { bytes, name } => {
                let document = styk_doc::json::parse(bytes).map_err(|_| OpenError::unreadable())?;
                let version = document.get("version").and_then(Value::as_f64).unwrap_or(5.0);
                (document, "json", version, name.to_string())
            }
        };
        if cancel() {
            return Err(OpenError { code: Some("cancelled".into()), status: Some(499), ..OpenError::decode("The import was cancelled.") });
        }
        let mut indexes = styk_edit::load::prepare_for_editor(document, options.layer_timeline).map_err(OpenError::from_edit)?;
        let tree = std::mem::replace(&mut indexes.report, Value::Null);
        let shown = tree.path(&["_styk", "name"]).and_then(Value::as_str).unwrap_or("").to_string();
        let mut loader = Loader::new();
        let converted = loader.document(&tree, &shown).map_err(|error| OpenError { stage: OpenStage::Prepare, ..OpenError::decode(error.to_string()) })?;
        let assets = Arc::new(Assets::build(&converted));
        let prepared = Arc::new(Prepared::new(Arc::new(converted)));
        let lineage = self.inner.next_generation.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(OpenedDocument { tree: tree.clone(), indexes, name, kind, version, typed: Typed { tree, loader, prepared, assets, lineage } })
    }

    /// Makes the document that `typed` belongs to the engine's document, at `revision`, with `tree` as it
    /// is now: the stage draws it, playback and exports read it. What changed in `tree` since `typed` was made
    /// is converted again; nothing else is.
    pub fn activate(&self, typed: &Typed, tree: &Value, revision: u64) -> Result<Committed, OpenError> {
        let mut store = self.inner.store.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        store.loader = typed.loader.clone();
        store.lineage = typed.lineage;
        store.assets = Some(typed.assets.clone());
        store.prepared = Some(typed.prepared.clone());
        store.tree = typed.tree.clone();
        // The tree it was opened as needs no conversion: its views are the ones made at the open.
        let reuse = tree.same_node(&typed.tree).then(|| typed.prepared.clone());
        self.publish_locked(store, tree.clone(), revision, reuse)
    }

    /// Brings the engine's document to `tree` at `revision`, converting only the records that changed since the
    /// last publication. `None` when `lineage` is not the document the engine holds (another document is in
    /// front, or none): nothing is published.
    pub fn publish_tree(&self, lineage: u64, tree: Value, revision: u64) -> Result<Option<Committed>, OpenError> {
        let store = self.inner.store.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if store.lineage != lineage || store.prepared.is_none() {
            return Ok(None);
        }
        // A revision that moved with the tree untouched (a document marked as changed) keeps its typed views.
        let views = if tree.same_node(&store.tree) { store.prepared.clone() } else { None };
        self.publish_locked(store, tree, revision, views).map(Some)
    }

    fn publish_locked(&self, mut store: std::sync::MutexGuard<'_, crate::store::Store>, tree: Value, revision: u64, views: Option<Arc<Prepared>>) -> Result<Committed, OpenError> {
        let started = self.now();
        let (prepared, assets, converted) = match views {
            Some(prepared) => (prepared, store.assets.clone().expect("an activated document has its atlas"), Default::default()),
            None => {
                let name = tree.path(&["_styk", "name"]).and_then(Value::as_str).unwrap_or("").to_string();
                let document = store.loader.document(&tree, &name).map_err(|error| OpenError { stage: OpenStage::Prepare, ..OpenError::decode(error.to_string()) })?;
                let converted = store.loader.converted;
                let assets = Arc::new(match &store.assets {
                    Some(previous) => previous.extended(&document),
                    None => Assets::build(&document),
                });
                (Arc::new(Prepared::new(Arc::new(document))), assets, converted)
            }
        };
        let generation = Generation { id: 0, lineage: store.lineage, revision, prepared: prepared.clone(), assets: assets.clone(), open_ms: self.now() - started, anchors: None };
        store.tree = tree;
        store.revision = revision;
        store.prepared = Some(prepared);
        store.assets = Some(assets);
        store.loader.release_unused();
        drop(store);
        let id = self.publish(generation);
        Ok(Committed { generation: id, revision, converted, ms: self.now() - started })
    }
}

impl OpenedDocument {
    /// Starts an editing session on the document with the editor's `state` (JSON: options, view and counters;
    /// see `styk_edit::session::Session`). The session shares the tree and the typed views, so nothing is
    /// converted again.
    pub fn start_session(&self, state: &str) -> Result<styk_edit::session::Session, String> {
        styk_edit::session::Session::start_prepared(self.tree.clone(), state, self.typed.loader.clone(), self.typed.prepared.clone())
    }

    /// The lineage of the document's generations.
    pub fn lineage(&self) -> u64 {
        self.typed.lineage
    }
}

impl OpenedDocument {
    /// Gives the document the Pivot file it was read from, as a saved native document carries its source
    /// (`_stykSource`: the codec's description of the file and the file itself), so that saving it needs no
    /// copy of the file anywhere else. `description` is an object; the file is added to it as `source_base64`.
    pub fn attach_source(&mut self, description: &Value, file: Vec<u8>) -> Result<(), OpenError> {
        let Some(description) = description.as_obj() else {
            return Err(OpenError::decode("The description of the original source is not an object."));
        };
        let mut source = description.clone();
        source.set("source_base64", Value::Blob(Arc::new(styk_doc::Blob { prefix: Arc::from(""), bytes: Arc::from(file) })));
        let mut root = self.tree.as_obj().cloned().ok_or_else(|| OpenError::decode("The document is not an object."))?;
        root.set("_stykSource", Value::object(source));
        self.tree = Value::object(root);
        Ok(())
    }
}
