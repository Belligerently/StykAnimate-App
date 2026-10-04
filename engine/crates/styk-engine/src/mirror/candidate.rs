//! Worker-scoped preparation in the sole Mirror/Loader/publication owner.
//! The existing mirror guard and in-flight batch marker stay here until consume
//! or worker disposal. This value must not span host ACKs or scheduler turns.
use super::*;
use std::sync::MutexGuard;

fn image_prefix(before: &[styk_eval::doc::ImageData], after: &[styk_eval::doc::ImageData]) -> bool {
    after.len() >= before.len() && before.iter().zip(after).all(|(left, right)|
        Arc::ptr_eq(&left.bytes, &right.bytes) && left.color_key == right.color_key)
}
fn atlas_prefix(before: &Assets, after: &Assets) -> bool {
    after.atlas.len() >= before.atlas.len() && before.atlas.iter().zip(&after.atlas).all(|(a, b)|
        (a.layer,a.x,a.y,a.w,a.h,a.source_w,a.source_h,a.valid)
        == (b.layer,b.x,b.y,b.w,b.h,b.source_w,b.source_h,b.valid))
}
fn same_optional<T>(left: &Option<Arc<T>>, right: &Option<Arc<T>>) -> bool {
    match (left, right) { (None,None) => true, (Some(left),Some(right)) => Arc::ptr_eq(left,right), _ => false }
}
fn layers_equal(left: &Option<Arc<styk_eval::doc::Layers>>, right: &Option<Arc<styk_eval::doc::Layers>>) -> bool {
    match (left,right) {
        (None,None) => true,
        (Some(a),Some(b)) => a.order==b.order && a.assignments==b.assignments && a.default_layer==b.default_layer
            && a.hidden==b.hidden && a.camera_visible==b.camera_visible && a.scene_visible==b.scene_visible && a.parallax==b.parallax,
        _ => false,
    }
}
fn sheet_equal(left: &Option<Arc<styk_eval::doc::LayerSheet>>, right: &Option<Arc<styk_eval::doc::LayerSheet>>) -> bool {
    use styk_eval::doc::SheetValue;
    match (left,right) {
        (None,None) => true,
        (Some(a),Some(b)) => a.duration==b.duration && a.tracks.len()==b.tracks.len()
            && a.tracks.iter().all(|(id,a)| b.tracks.get(id).is_some_and(|b| a.end==b.end && a.keys.len()==b.keys.len()
                && a.keys.iter().zip(&b.keys).all(|(a,b)| {
                    let easing=match (&a.easing,&b.easing) { (None,None)=>true,(Some(a),Some(b))=>a.mode==b.mode&&a.bezier==b.bezier,_=>false };
                    let value=match (&a.value,&b.value) { (SheetValue::Frame(a),SheetValue::Frame(b))=>a==b,
                        (SheetValue::SourceTick(a),SheetValue::SourceTick(b))=>a.to_bits()==b.to_bits(),_=>false };
                    a.tick==b.tick && a.hidden==b.hidden && a.patch_only==b.patch_only && easing && value
                        && a.movieclip_patches.len()==b.movieclip_patches.len()
                        && a.movieclip_patches.iter().zip(&b.movieclip_patches).all(|(a,b)| a.uid==b.uid && a.frame_id==b.frame_id
                            && a.replacement_uid==b.replacement_uid && a.order_index==b.order_index)
                }))),
        _ => false,
    }
}
fn typed_document_heads(after: &styk_eval::doc::Document, canonical: &styk_eval::doc::Document) -> bool {
    // Force each future typed Document field to receive an explicit compatibility
    // decision here; a new field must not silently bypass this capability check.
    let styk_eval::doc::Document { scene_budget: _, name: _, version: _, canvas: _, fps: _,
        auto_easing: _, figures: _, figure_index: _, keyframes: _, backgrounds: _, images: _,
        layers: _, sheet: _, frame_sources: _, camera_motion: _, vfx_clips: _, vfx_elsewhere: _,
        animation: _, unsupported: _, notes: _, origins: _, movieclip_source: _ } = after;
    // The guarded raw/cache node is the authority. The same owning Loader read
    // must reuse its immutable definition/pose/channel heads, not fabricate a
    // compatible-looking second source. Root backgrounds/camera/source scopes
    // need their own recorded typed-pairing proof before this entry admits them.
    Arc::ptr_eq(&after.scene_budget,&canonical.scene_budget)
        && after.name == canonical.name && after.version == canonical.version
        && after.canvas == canonical.canvas && after.fps.to_bits() == canonical.fps.to_bits()
        && after.auto_easing == canonical.auto_easing && after.origins == canonical.origins
        && after.figures.len() == canonical.figures.len()
        && after.figures.iter().zip(&canonical.figures).all(|(a,b)| Arc::ptr_eq(a,b))
        && after.figure_index == canonical.figure_index
        && after.keyframes.len() == canonical.keyframes.len()
        && after.keyframes.iter().zip(&canonical.keyframes).all(|(a,b)| Arc::ptr_eq(a,b))
        && after.frame_sources.len() == canonical.frame_sources.len()
        && after.frame_sources.iter().all(|(id,frame)| canonical.frame_sources.get(id).is_some_and(|other|Arc::ptr_eq(frame,other)))
        && after.images.len() == canonical.images.len() && image_prefix(&after.images,&canonical.images)
        && same_optional(&after.animation,&canonical.animation) && same_optional(&after.vfx_clips,&canonical.vfx_clips)
        && layers_equal(&after.layers,&canonical.layers) && sheet_equal(&after.sheet,&canonical.sheet)
        && after.vfx_elsewhere == canonical.vfx_elsewhere
        && after.unsupported == canonical.unsupported && after.notes == canonical.notes
        && after.backgrounds.is_empty() && canonical.backgrounds.is_empty()
        && after.camera_motion.is_empty() && canonical.camera_motion.is_empty()
        && after.movieclip_source.is_none() && canonical.movieclip_source.is_none()
}

struct Stored {
    tree: Value,
    publication: crate::publication::Prepared,
    converted: Converted,
    previous_lineage: u64,
    previous_assets: Option<Arc<Assets>>,
    loader: Option<Loader>,
}
enum Content {
    Stored(Stored),
    Working(Arc<Generation>),
}

/// Opaque preparation borrowed/consumed entirely in one worker scope. Its exact
/// Generation includes the compatible typed document/Assets, before encoding.
/// It deliberately owns the existing MutexGuard and is not a Send/ACK payload.
#[must_use = "consume or dispose this candidate on its owning worker"]
pub struct WorkerPublication<'a> {
    engine: &'a Engine,
    mirror: MutexGuard<'a, Mirror>,
    staged: transfer::Batch,
    marker: Option<CommitMarker<'a>>,
    content: Option<Content>,
    started: f64,
    revision: u64,
    committed: bool,
}
impl WorkerPublication<'_> {
    /// Borrow on the worker; no independently rebuilt Generation/Assets.
    pub fn generation(&self) -> &Generation {
        match self.content.as_ref().expect("unconsumed publication") {
            Content::Stored(stored) => stored.publication.generation(),
            Content::Working(generation) => generation,
        }
    }
    pub fn stores_generation(&self) -> bool { matches!(self.content, Some(Content::Stored(_))) }

    pub(super) fn commit<F, R>(mut self, exchange: &mut Option<F>)
        -> Result<(Committed, crate::PublicationRetirement, R), (MirrorError, Self)>
    where F: FnOnce() -> R {
        let result = match self.content.take().expect("consume once") {
            Content::Working(generation) => {
                let guard = self.staged.guard.expect("working publication is guarded");
                match self.engine.exchange_session_working_worker(guard, &self.staged.cancelled, exchange) {
                    Some(exchanged) => {
                        let mut retirement = crate::PublicationRetirement::empty();
                        retirement.record_charges.append(&mut self.staged.charges);
                        Ok((Committed { generation: generation.id, revision: guard.revision,
                            converted: Default::default(), ms: self.engine.now() - self.started }, retirement, exchanged))
                    }
                    None => {
                        self.content = Some(Content::Working(generation));
                        Err(MirrorError::Transaction("the working Session was superseded before publication"))
                    }
                }
            }
            Content::Stored(mut stored) => {
                let guard = self.staged.guard.map(|guard| (guard, self.staged.cancelled.clone()));
                let generation = stored.publication.generation().clone();
                match self.engine.publish_prepared_compatible_worker(stored.publication, guard.as_ref(), exchange) {
                    Ok((id, mut retirement, exchanged)) => {
                        // The publication's exact resources are now the sole
                        // State generation. Reuse them for the mirror head.
                        let had_canonical_head = self.mirror.prepared.is_some();
                        let previous_root = std::mem::replace(&mut self.mirror.tree, stored.tree);
                        if had_canonical_head { retirement.canonical_root = Some(previous_root); }
                        retirement.record_charges.append(&mut self.staged.charges);
                        self.mirror.revision = self.revision;
                        self.mirror.prepared = Some(generation.prepared.clone());
                        self.mirror.assets = Some(generation.assets.clone());
                        if let Some(loader) = stored.loader.take() {
                            retirement.typed_loader = Some(std::mem::replace(&mut self.mirror.loader, loader));
                        }
                        self.mirror.loader.release_unused();
                        Ok((Committed { generation: id, revision: self.revision, converted: stored.converted,
                            ms: generation.open_ms }, retirement, exchanged))
                    }
                    Err(publication) => {
                        stored.publication = publication;
                        self.content = Some(Content::Stored(stored));
                        Err(MirrorError::Transaction("the lifted transfer was superseded before publication"))
                    }
                }
            }
        };
        match result {
            Ok(result) => { self.committed = true; Ok(result) }
            Err(error) => Err((error, self)),
        }
    }

    /// Allocation/drop-free exchange under the existing final State guard. The
    /// caller retains all returned retirements on its worker through host ACK.
    /// Refusal returns this candidate and the uncalled callback for worker drop.
    pub fn commit_with_worker<F, R>(self, exchange: F)
        -> Result<(Committed, crate::PublicationRetirement, R), (MirrorError, Self, F)>
    where F: FnOnce() -> R {
        let mut exchange = Some(exchange);
        self.commit(&mut exchange).map_err(|(error, candidate)|
            (error, candidate, exchange.take().expect("refused before exchange")))
    }
}
impl Drop for WorkerPublication<'_> {
    fn drop(&mut self) {
        if !self.committed {
            if let Some(Content::Stored(stored)) = self.content.as_ref() {
                // Same failure disposition as the original guarded publisher:
                // no canonical head changes; sole Loader scratch is rebuilt.
                if stored.loader.is_none() {
                    self.mirror.loader = Loader::new();
                    self.mirror.lineage = stored.previous_lineage;
                    self.mirror.assets = stored.previous_assets.clone();
                }
            }
        }
        // The existing CommitMarker and remaining payloads retire on this
        // worker, after State has released, exactly as ordinary commit did.
        drop(self.marker.take());
    }
}

impl Engine {
    /// Prepare a completed guarded record batch in the existing sole Mirror
    /// owner. Worker-scoped only: borrow generation/assets, then consume/drop.
    pub fn prepare_record_publication_worker(&self, revision: u64, batch: u32, guard: RecordGuard)
        -> Result<WorkerPublication<'_>, MirrorError> {
        let started = self.now();
        let mirror = self.inner.mirror.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        self.prepare_locked(mirror, started, revision, batch, Some(guard), None, None)
    }

    pub(super) fn prepare_locked<'a>(&'a self, mut mirror: MutexGuard<'a, Mirror>, started: f64,
        revision: u64, batch: u32, guard: Option<RecordGuard>,
        expected_root: Option<(&Value, &Value, &Arc<transfer::Charge>, bool)>,
        typed: Option<&styk_edit::publication::GuardedTypedCache<'_>>)
        -> Result<WorkerPublication<'a>, MirrorError> {
        let mut empty = Some(transfer::Batch::default());
        let mut staged = {
            let mut staged = self.inner.staged.lock();
            if guard.is_none() && staged.get(&batch).is_some_and(|owner| owner.guard.is_some()) {
                return Err(MirrorError::Transaction("a lifted transaction requires its guarded commit"));
            }
            if guard.is_some_and(|guard| staged.get(&batch).and_then(|batch| batch.guard) != Some(guard)) {
                return Err(MirrorError::Transaction("the lifted transaction was discarded or replaced"));
            }
            match (staged.get(&batch).and_then(|batch| batch.root.as_ref()), expected_root) {
                (None, None) => {},
                (Some(root), Some((before, after, charge, stored))) if root.before.same_node(before) && root.after.same_node(after)
                    && Arc::ptr_eq(&root.charge, charge) && root.stored == stored && mirror.tree.same_node(before) => {},
                _ => return Err(MirrorError::Transaction("a prepared root requires its exact guarded Editor publication")),
            }
            if staged.get(&batch).and_then(|batch| batch.transfer.as_ref()).is_some_and(|transfer| !transfer.acknowledged()) {
                return Err(MirrorError::Transaction("the lifted transfer has not completed and been acknowledged"));
            }
            if guard.is_some() {
                let owner = staged.get_mut(&batch).expect("guard checked above");
                if owner.committing { return Err(MirrorError::Transaction("the lifted transaction is already preparing")); }
                owner.committing = true;
                transfer::Batch { changes: std::mem::take(&mut owner.changes), guard: owner.guard,
                    transfer: owner.transfer.clone(), cancelled: owner.cancelled.clone(), committing: true,
                    budget: owner.budget.clone(), charges: std::mem::take(&mut owner.charges), root: owner.root.take() }
            } else { staged.remove(&batch).unwrap_or_else(|| empty.take().expect("prebuilt empty batch")) }
        };
        let marker = staged.guard.map(|_| CommitMarker { engine: self, batch, cancelled: staged.cancelled.clone() });
        if staged.cancelled.load(std::sync::atomic::Ordering::SeqCst) || staged.guard.is_some_and(|guard| !self.record_guard_current(guard)) {
            return Err(MirrorError::Transaction("the lifted transfer belongs to another document generation"));
        }
        let content = if expected_root.is_some_and(|(_, _, _, stored)| !stored) {
            let generation = self.generation().ok_or(MirrorError::Transaction("the working Session has no stored generation"))?;
            let guard = staged.guard.expect("working publication is guarded");
            if generation.id != guard.generation || generation.lineage != guard.lineage || generation.revision != guard.revision {
                return Err(MirrorError::Transaction("the lifted transfer belongs to another document generation"));
            }
            if typed.is_some_and(|context| !expected_root.is_some_and(|(_, after, _, _)| context.report().same_node(after))
                || !typed_document_heads(&context.prepared().doc, &generation.prepared.doc)
                || !image_prefix(&generation.prepared.doc.images, &mirror.loader.images)
                || generation.prepared.doc.images.len() != mirror.loader.images.len())
            {
                return Err(MirrorError::Transaction("the working typed cache differs from its acknowledged assets"));
            }
            Content::Working(generation)
        } else {
            let (tree, reset) = match staged.root.take() {
                Some(root) => (root.after, false), None => mirror.apply(std::mem::take(&mut staged.changes))?,
            };
            let previous_lineage = mirror.lineage;
            let previous_assets = mirror.assets.clone();
            if reset {
                mirror.loader = Loader::new();mirror.assets = None;
                mirror.lineage = self.inner.next_generation.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            let name = tree.path(&["_styk", "name"]).and_then(Value::as_str).unwrap_or("").to_string();
            // The opt-in actual Editor path clones its existing after Loader on
            // this worker. Ordinary record candidates keep their original owner.
            // The Mirror's prior Loader stays untouched until final consume.
            let mut next_loader = typed.map(|context| context.loader().clone());
            let loader = next_loader.as_mut().unwrap_or(&mut mirror.loader);
            let document = loader.document(&tree, &name).map_err(|e| MirrorError::Document(e.to_string()))?;
            let converted = loader.converted;
            if let Some(context) = typed {
                let previous = mirror.prepared.as_ref().ok_or(MirrorError::Transaction("typed publication requires an acknowledged document"))?;
                let previous_assets = mirror.assets.as_ref().ok_or(MirrorError::Transaction("typed publication requires acknowledged assets"))?;
                if reset || !context.report().same_node(&tree)
                    || !typed_document_heads(&context.prepared().doc, &document)
                    || !Arc::ptr_eq(&previous.doc.scene_budget, &document.scene_budget)
                    || !image_prefix(&previous.doc.images, &document.images)
                    || !image_prefix(&previous.doc.images, &mirror.loader.images)
                    || previous.doc.images.len() != mirror.loader.images.len()
                    || previous_assets.atlas.len() != previous.doc.images.len()
                {
                    return Err(MirrorError::Transaction("the guarded typed cache has a foreign document or image namespace"));
                }
            }
            let assets = Arc::new(match &mirror.assets {
                Some(previous) => previous.extended(&document), None => Assets::build(&document),
            });
            if typed.is_some() && mirror.assets.as_ref().is_some_and(|previous| !atlas_prefix(previous, &assets)) {
                return Err(MirrorError::Transaction("typed publication changed an acknowledged image rectangle"));
            }
            let prepared = Arc::new(Prepared::new(Arc::new(document)));
            let ms = self.now() - started;
            let guard = staged.guard.map(|guard| (guard, staged.cancelled.clone()));
            let generation = Generation { id: 0, lineage: mirror.lineage, revision, prepared, assets, open_ms: ms, anchors: None };
            let publication = match self.prepare_publication_worker(generation, guard.as_ref()) {
                Some(publication) => publication,
                None => {
                    if next_loader.is_none() {
                        mirror.loader = Loader::new();mirror.lineage = previous_lineage;mirror.assets = previous_assets;
                    }
                    return Err(MirrorError::Transaction("the lifted transfer was superseded before publication"));
                }
            };
            Content::Stored(Stored { tree, publication, converted, previous_lineage, previous_assets, loader: next_loader })
        };
        Ok(WorkerPublication { engine: self, mirror, staged, marker, content: Some(content),
            started, revision, committed: false })
    }
}

#[cfg(test)]
#[path = "candidate_tests.rs"]
mod tests;
