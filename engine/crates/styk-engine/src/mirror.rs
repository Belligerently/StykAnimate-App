//! The document as the interface last described it.
//!
//! While editing commands still run in JavaScript, the engine's document is a
//! mirror: the interface stages what changed, record by record, and commits. A
//! commit builds the next tree by copy-on-write, converts only the records that
//! changed into typed views, and publishes a new generation. Nothing is visible
//! to evaluation until the commit succeeds as a whole.

use std::sync::Arc;

use styk_doc::{Obj, Value};
use styk_eval::assets::Assets;
use styk_eval::eval::Prepared;
use styk_eval::load::{Converted, Loader};
use styk_formats::container::parse_lifted;

use crate::{Engine, Generation};

/// The tables of a document that are synchronised record by record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Table {
    /// `figure_types`
    Figures,
    /// `keyframes`
    Keyframes,
    /// `backgrounds`
    Backgrounds,
}

impl Table {
    pub fn key(self) -> &'static str {
        match self {
            Table::Figures => "figure_types",
            Table::Keyframes => "keyframes",
            Table::Backgrounds => "backgrounds",
        }
    }

    pub fn from_code(code: u32) -> Option<Table> {
        match code {
            0 => Some(Table::Figures),
            1 => Some(Table::Keyframes),
            2 => Some(Table::Backgrounds),
            _ => None,
        }
    }
}

/// A record as it crosses the boundary: JSON with markers into two byte sections
/// (the layout `styk_formats::container::lift` writes), so number arrays and
/// images travel as bytes.
pub struct Record {
    pub skeleton: Vec<u8>,
    pub arrays: Vec<u8>,
    pub payloads: Vec<u8>,
}

/// One staged change.
pub enum Change {
    /// Forget the mirrored document: what follows describes another one.
    Reset,
    /// The top-level properties (everything but the tables and `_styk.frameSources`).
    Properties(Vec<u8>),
    /// The new order of a table: each entry is the index a record had before, or
    /// -1 for a record that a following [`Change::Set`] supplies.
    Layout(Table, Vec<i32>),
    Set(Table, u32, Record),
    /// `_styk.frameSources[id]`: a retained pose source, or its removal.
    Source(String, Option<Record>),
}

/// Why a commit was refused. The mirror is unchanged.
#[derive(Debug, thiserror::Error)]
pub enum MirrorError {
    #[error("a staged record is not valid: {0}")]
    Record(String),
    #[error("a staged change names record {index} of `{table}`, which has {len}")]
    Index { table: &'static str, index: usize, len: usize },
    #[error("record {index} of `{table}` was announced by the layout but never supplied")]
    Missing { table: &'static str, index: usize },
    #[error("the document cannot be evaluated: {0}")]
    Document(String),
}

/// What a commit did.
#[derive(Clone, Copy, Debug)]
pub struct Committed {
    pub generation: u64,
    pub revision: u64,
    /// Records converted into typed views; everything else was reused.
    pub converted: Converted,
    pub ms: f64,
}

pub(crate) struct Mirror {
    tree: Value,
    /// The interface revision of the last commit.
    revision: u64,
    loader: Loader,
    lineage: u64,
    assets: Option<Arc<Assets>>,
    /// The typed views of `tree`, as published with its generation.
    prepared: Option<Arc<Prepared>>,
}

impl Default for Mirror {
    fn default() -> Self {
        Mirror { tree: Value::object(Obj::new()), revision: 0, loader: Loader::new(), lineage: 0, assets: None, prepared: None }
    }
}

fn record(record: &Record) -> Result<Value, MirrorError> {
    parse_lifted(&record.skeleton, &record.arrays, &record.payloads).map_err(|e| MirrorError::Record(e.to_string()))
}

fn table_items(tree: &Value, table: Table) -> Vec<Value> {
    tree.get(table.key()).and_then(Value::as_arr).map(<[Value]>::to_vec).unwrap_or_default()
}

impl Mirror {
    /// Applies `changes` to a copy of the tree. The copy shares every record the changes do not name.
    fn apply(&self, changes: Vec<Change>) -> Result<(Value, bool), MirrorError> {
        let mut tree = self.tree.clone();
        let mut reset = false;
        // Placeholders left by a layout until their record arrives.
        let mut pending: Vec<(Table, usize)> = Vec::new();
        for change in changes {
            match change {
                Change::Reset => {
                    tree = Value::object(Obj::new());
                    reset = true;
                    pending.clear();
                }
                Change::Properties(text) => {
                    let properties = styk_doc::json::parse(&text).map_err(|e| MirrorError::Record(e.to_string()))?;
                    let Some(incoming) = properties.as_obj() else { return Err(MirrorError::Record("the properties are not an object".into())) };
                    let sources = tree.path(&["_styk", "frameSources"]).cloned();
                    let mut next = Obj::new();
                    for (key, value) in incoming.iter() {
                        next.set(key, value.clone());
                    }
                    for table in [Table::Figures, Table::Keyframes, Table::Backgrounds] {
                        if let Some(items) = tree.get(table.key()) {
                            next.set(table.key(), items.clone());
                        }
                    }
                    if let Some(sources) = sources {
                        let mut styk = next.get("_styk").cloned().filter(Value::is_object).unwrap_or_else(|| Value::object(Obj::new()));
                        styk.make_obj().expect("an object").set("frameSources", sources);
                        next.set("_styk", styk);
                    }
                    tree = Value::object(next);
                }
                Change::Layout(table, layout) => {
                    let old = table_items(&tree, table);
                    let mut items = Vec::with_capacity(layout.len());
                    pending.retain(|(t, _)| *t != table);
                    for (at, &from) in layout.iter().enumerate() {
                        if from < 0 {
                            pending.push((table, at));
                            items.push(Value::Null);
                        } else {
                            let from = from as usize;
                            items.push(old.get(from).cloned().ok_or(MirrorError::Index { table: table.key(), index: from, len: old.len() })?);
                        }
                    }
                    tree.make_obj().expect("the document is an object").set(table.key(), Value::Arr(Arc::new(items)));
                }
                Change::Set(table, index, lifted) => {
                    let value = record(&lifted)?;
                    let index = index as usize;
                    let root = tree.make_obj().expect("the document is an object");
                    let len = root.get(table.key()).and_then(Value::array_len).unwrap_or(0);
                    let items = root.get_mut(table.key()).and_then(Value::make_arr).filter(|items| index < items.len()).ok_or(MirrorError::Index { table: table.key(), index, len })?;
                    items[index] = value;
                    pending.retain(|entry| *entry != (table, index));
                }
                Change::Source(id, lifted) => {
                    let value = lifted.as_ref().map(record).transpose()?;
                    let root = tree.make_obj().expect("the document is an object");
                    if !root.get("_styk").is_some_and(Value::is_object) {
                        root.set("_styk", Value::object(Obj::new()));
                    }
                    let styk = root.get_mut("_styk").and_then(Value::make_obj).expect("set above");
                    if !styk.get("frameSources").is_some_and(Value::is_object) {
                        styk.set("frameSources", Value::object(Obj::new()));
                    }
                    let sources = styk.get_mut("frameSources").and_then(Value::make_obj).expect("set above");
                    match value {
                        Some(value) => sources.set(&id, value),
                        None => {
                            sources.remove(&id);
                        }
                    }
                }
            }
        }
        if let Some(&(table, index)) = pending.first() {
            return Err(MirrorError::Missing { table: table.key(), index });
        }
        Ok((tree, reset))
    }
}

impl Engine {
    /// Stages one change in `batch`. Cheap: nothing is parsed until [`Engine::commit`].
    /// Batches are independent: a long description of the frames can be staged over
    /// many turns of the interface while small commits of other batches go through.
    pub fn stage(&self, batch: u32, change: Change) {
        self.inner.staged.lock().entry(batch).or_default().push(change);
    }

    /// The mirrored document and the interface revision of its last commit, for an
    /// edit session to start from. `None` while a commit is running (this never
    /// waits) or before any document. The tree is shared, not copied.
    pub fn mirrored(&self) -> Option<(u64, Value)> {
        let mirror = self.inner.mirror.try_lock().ok()?;
        mirror.tree.get("keyframes")?;
        Some((mirror.revision, mirror.tree.clone()))
    }

    /// [`Engine::mirrored`] with what an edit session needs to start without converting
    /// the document again: a copy of the mirror's loader (typed records shared) and the
    /// typed views published for that revision. `None` while committing, as there.
    pub fn mirrored_for_editing(&self) -> Option<(u64, Value, Loader, Arc<Prepared>)> {
        let mirror = self.inner.mirror.try_lock().ok()?;
        mirror.tree.get("keyframes")?;
        Some((mirror.revision, mirror.tree.clone(), mirror.loader.clone(), mirror.prepared.clone()?))
    }

    /// Applies everything staged in `batch` and publishes the result as the document
    /// of `revision` (the interface's own revision counter). All or nothing: on an
    /// error the previous document stays. Runs on the calling thread; hosts call it
    /// through [`Engine::spawn`] for anything large.
    pub fn commit(&self, revision: u64, batch: u32) -> Result<Committed, MirrorError> {
        let started = self.now();
        let changes = self.inner.staged.lock().remove(&batch).unwrap_or_default();
        // Only one commit runs at a time; commits are background tasks, so waiting here is allowed.
        let mut mirror = self.inner.mirror.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let (tree, reset) = mirror.apply(changes)?;
        if reset {
            mirror.loader = Loader::new();
            mirror.assets = None;
            mirror.lineage = self.inner.next_generation.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        let name = tree.path(&["_styk", "name"]).and_then(Value::as_str).unwrap_or("").to_string();
        let document = mirror.loader.document(&tree, &name).map_err(|e| MirrorError::Document(e.to_string()))?;
        let converted = mirror.loader.converted;
        let assets = Arc::new(match &mirror.assets {
            Some(previous) => previous.extended(&document),
            None => Assets::build(&document),
        });
        let prepared = Arc::new(Prepared::new(Arc::new(document)));
        mirror.tree = tree;
        mirror.revision = revision;
        mirror.prepared = Some(prepared.clone());
        mirror.assets = Some(assets.clone());
        mirror.loader.release_unused();
        let ms = self.now() - started;
        let id = self.publish(Generation { id: 0, lineage: mirror.lineage, revision, prepared, assets, open_ms: ms, anchors: None });
        Ok(Committed { generation: id, revision, converted, ms })
    }
}
