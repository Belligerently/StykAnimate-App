//! Canonical worker Editor roots in the same guarded mirror batch owner.
//! No raw root setter, serialized document, alternate diff, store or Loader.
use std::collections::LinkedList;
use std::sync::{Arc, Weak};
use std::sync::atomic::AtomicBool;

use styk_doc::Value;
use styk_edit::publication::{GuardedPublication, PreparedPublication, RetiredPublication};

use super::{
    MirrorError, RecordGuard,
    transfer::{Budget, Charge},
};
use crate::{Engine, PublicationRetirement};

// Only the owning prepared Editor can construct this slot. It cannot appear in
// public Change or browser bindings and cannot mix with transported records.
pub(super) struct Root {
    pub(super) before: Value,
    pub(super) after: Value,
    pub(super) charge: Arc<Charge>,
    pub(super) stored: bool,
    pub(super) wake: Option<Weak<crate::session::Core>>,
}

// Private worker continuation of the SAME receiver reservation. This accounts
// only receiver/current-ready raw values, not Editor history or typed storage.
pub(crate) struct ReadyCharge { charge: Arc<Charge> }
impl ReadyCharge {
    pub(crate) fn unique(&mut self) -> bool { Arc::get_mut(&mut self.charge).is_some() }
    pub(crate) fn rescope(owner: &mut Option<Self>, current: Option<&Value>) -> Result<bool, MirrorError> {
        let Some(continuation) = owner.as_mut() else { return Ok(true) };
        let Some(charge) = Arc::get_mut(&mut continuation.charge) else { return Ok(false) };
        if let Some(current) = current {
            let bytes = styk_doc::retained_bytes_checked(&[current], |_| Ok::<_, MirrorError>(()))?
                .checked_add(256).ok_or_else(|| MirrorError::Record("retained raw size overflow".into()))?;
            charge.grow_to(bytes).map_err(|error| MirrorError::Record(error.to_string()))?;
            charge.shrink_to(bytes);
        } else {
            drop(owner.take()); // no current raw head remains, including first failure
        }
        Ok(true)
    }
}
pub(crate) struct SessionRoot<'a> {
    pub(crate) current: Option<&'a Value>,
    pub(crate) charge: &'a mut Option<ReadyCharge>,
    pub(crate) wake: Weak<crate::session::Core>,
}

/// Opaque admission of one exact prepared Editor root. Keep this strong lease
/// on its durable worker while that prepared payload or its refusal is retained.
/// It is required by typed commit; dropping a staged batch cannot release the
/// shared charge before the candidate/retirement owner is actually disposed.
#[must_use = "retain the prepared root admission on its worker until commit or disposal"]
pub struct EditPublicationAdmission {
    batch: u32,
    guard: RecordGuard,
    before: Value,
    after: Value,
    charge: Arc<Charge>,
    editor_before: Option<Value>,
    stored: bool,
}

/// Exact guarded Editor and Mirror preparation, scoped to one worker. The
/// Mirror guard/CommitMarker are retained; no ACK-spanning or UI owner is exposed.
#[must_use = "consume or dispose this candidate on its owning worker"]
pub struct WorkerEditPublication<'engine, 'editor> {
    candidate: super::WorkerPublication<'engine>,
    publication: GuardedPublication<'editor>,
    retirement: PublicationRetirement,
}
impl<'engine, 'editor> WorkerEditPublication<'engine, 'editor> {
    pub fn generation(&self) -> &crate::Generation { self.candidate.generation() }
    pub fn stores_generation(&self) -> bool { self.candidate.stores_generation() }

    /// Consume the exact candidate and both head exchanges once. Caller keeps
    /// returned retirements on the worker; refusal returns the uncalled editor.
    pub fn commit_worker(self) -> Result<(super::Committed, PublicationRetirement, RetiredPublication),
        (MirrorError, GuardedPublication<'editor>, PublicationRetirement)> {
        self.commit_with_worker(|retired| retired)
            .map_err(|(error, publication, retirement, _)| (error, publication, retirement))
    }

    pub(crate) fn commit_with_worker<F, R>(self, ready_exchange: F)
        -> Result<(super::Committed, PublicationRetirement, R),
            (MirrorError, GuardedPublication<'editor>, PublicationRetirement, F)>
    where F: FnOnce(RetiredPublication) -> R {
        let Self { candidate, publication, mut retirement } = self;
        let mut publication = Some(publication);
        let mut ready_exchange = Some(ready_exchange);
        let mut exchange = Some(|| {
            let retired = publication.take().expect("one guarded head exchange").commit();
            ready_exchange.take().expect("one ready head exchange")(retired)
        });
        match candidate.commit(&mut exchange) {
            Ok((done, mut retired, editor)) => {
                retired.root_admission = retirement.root_admission.take();
                Ok((done, retired, editor))
            }
            Err((error, candidate)) => {
                drop(exchange);
                drop(candidate); // worker-only original Loader/marker disposition
                Err((error, publication.take().expect("refused before head exchange"),
                    retirement, ready_exchange.take().expect("refused before ready head exchange")))
            }
        }
    }
}

impl Engine {
    /// Worker-only admission of the exact immutable prepared Editor report. The
    /// before root must be this mirror's canonical COW head. Shared graph storage
    /// is charged once across before/after, cooperatively, to the SAME receiver
    /// budget. Cancellation/stale refusal releases only on the calling worker.
    /// There is no corresponding browser/UI root setter.
    pub fn stage_edit_publication_worker(
        &self,
        batch: u32,
        guard: RecordGuard,
        prepared: &PreparedPublication,
        check: impl FnMut() -> Result<(), MirrorError>,
    ) -> Result<EditPublicationAdmission, MirrorError> {
        self.stage_edit_publication_response_worker(batch, guard, prepared, prepared.result(), check)
    }

    /// Actor-only response metadata shares this same graph/retained admission.
    /// It never supplies a root: both canonical roots still come exclusively
    /// from the exact owning PreparedPublication.
    pub(crate) fn stage_edit_publication_response_worker(
        &self, batch: u32, guard: RecordGuard, prepared: &PreparedPublication,
        response: &Value, check: impl FnMut() -> Result<(), MirrorError>,
    ) -> Result<EditPublicationAdmission, MirrorError> {
        self.stage_publication_response_worker(batch, guard, prepared, response, None, false, true, None, None, check)
    }

    /// The actor aliases its prebuilt small cancellation atom into the SAME
    /// Batch owner. UI cancellation reaches final paired publication without
    /// acquiring State/staged or detaching any payload on that interface.
    pub(crate) fn stage_session_publication_response_worker(
        &self, batch: u32, guard: RecordGuard, prepared: &styk_edit::session::WorkerPrepared,
        response: &Value, cancelled: Arc<AtomicBool>, session: SessionRoot<'_>, check: impl FnMut() -> Result<(), MirrorError>,
    ) -> Result<EditPublicationAdmission, MirrorError> {
        self.stage_publication_response_worker(batch, guard, prepared.publication(), response, Some(cancelled),
            false, true, prepared.display().map(|display| &display.record), Some(session), check)
    }
    /// The exclusive actor's exact working candidate can differ from the
    /// canonical stored root during native previews without a revision change.
    /// Both roots are borrowed from their existing owners and share one charge.
    pub(crate) fn stage_live_session_publication_worker(
        &self, batch: u32, guard: RecordGuard,
        prepared: &styk_edit::session::WorkerPrepared, response: &Value,
        cancelled: Arc<AtomicBool>, session: SessionRoot<'_>, check: impl FnMut() -> Result<(), MirrorError>,
    ) -> Result<EditPublicationAdmission, MirrorError> {
        let stored = prepared.revision() != prepared.before_revision();
        self.stage_publication_response_worker(batch, guard, prepared.publication(), response,
            Some(cancelled), true, stored, prepared.display().map(|display| &display.record), Some(session), check)
    }
    fn stage_publication_response_worker(
        &self, batch: u32, guard: RecordGuard, prepared: &PreparedPublication,
        response: &Value, cancelled: Option<Arc<AtomicBool>>, live: bool, stored: bool, display_record: Option<&Value>, mut session: Option<SessionRoot<'_>>, mut check: impl FnMut() -> Result<(), MirrorError>,
    ) -> Result<EditPublicationAdmission, MirrorError> {
        check()?;
        let canonical_before = {
            let mirror = self
                .inner
                .mirror
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if (!live && !mirror.tree.same_node(prepared.before_report()))
                || prepared.before_revision() != guard.revision
                || !self.record_guard_current(guard)
            {
                return Err(MirrorError::Transaction(
                    "the prepared Editor does not own this mirror generation",
                ));
            }
            mirror.tree.clone()
        };
        let existing = session.as_ref().and_then(|session| session.charge.as_ref());
        let budget = existing.map(|continuation| continuation.charge.budget()).unwrap_or_else(|| {
            let staged_budget = {
                let staged = self.inner.staged.lock();
                staged.get(&batch).map(|owner| owner.budget.clone())
            };
            staged_budget.unwrap_or_else(|| Arc::new(Budget::default()))
        });
        let mut fresh = None;
        let no_display = Value::Null;
        let current = session.as_ref().and_then(|session| session.current).unwrap_or(&no_display);
        let wake = session.as_ref().map(|session| session.wake.clone());
        let slot = match session.as_mut() {
            Some(session) => &mut *session.charge,
            None => &mut fresh,
        };
        if slot.is_none() {
            *slot = Some(ReadyCharge { charge: Arc::new(Charge::new(self.inner.record_budget.clone(), budget.clone())
                .map_err(|error| MirrorError::Record(error.to_string()))?) });
        }
        let charge_owner = &mut slot.as_mut().expect("existing or admitted reservation").charge;
        let charge = Arc::get_mut(charge_owner).ok_or(MirrorError::Transaction(
            "the Session raw retirement has not been physically disposed"))?;
        charge.grow_to(256).map_err(|error| MirrorError::Record(error.to_string()))?;
        styk_doc::retained_bytes_checked(
            &[
                &canonical_before, prepared.before_report(), prepared.report(), prepared.result(),
                prepared.state(), prepared.history(), prepared.events(),
                prepared.before_state(), prepared.before_history(),
                response, display_record.unwrap_or(&no_display), current,
            ],
            |bytes| {
                check()?;
                charge
                    .grow_to(bytes.checked_add(256).ok_or_else(|| MirrorError::Record("retained raw size overflow".into()))?)
                    .map_err(|error| MirrorError::Record(error.to_string()))?;
                if !self.record_guard_current(guard) {
                    return Err(MirrorError::Transaction(
                        "the prepared Editor belongs to a stale generation",
                    ));
                }
                Ok(())
            },
        )?;
        check()?;
        let charge = charge_owner.clone();
        let admission = EditPublicationAdmission {
            batch,
            guard,
            before: canonical_before,
            after: prepared.report().clone(),
            charge: charge.clone(),
            editor_before: live.then(|| prepared.before_report().clone()),
            stored,
        };
        self.stage_guarded_cancelled_owner(
            batch,
            guard,
            budget,
            LinkedList::new(),
            LinkedList::new(),
            Some(Root {
                before: admission.before.clone(),
                after: admission.after.clone(),
                charge,
                stored,
                wake,
            }),
            cancelled,
        )
        .map_err(|error| MirrorError::Record(error.to_string()))?;
        Ok(admission)
    }

    /// Worker-only paired commit of that exact root and its guarded Editor heads.
    /// All conversion uses the mirror's sole Loader. Final nonce/cancel/generation
    /// validation and both head swaps share the receiver's State linearization.
    /// The exact stage admission is required and moves into the opaque Engine
    /// retirement. Keep BOTH retirements worker-owned through terminal success ACK.
    /// Refusal returns the still-uncalled guarded Editor plus its opaque charged
    /// retirement. Both stay worker-owned until terminal error/cancel disposition.
    pub fn commit_edit_publication_worker<'a>(
        &self,
        batch: u32,
        guard: RecordGuard,
        publication: GuardedPublication<'a>,
        admission: EditPublicationAdmission,
    ) -> Result<
        (super::Committed, PublicationRetirement, RetiredPublication),
        (MirrorError, GuardedPublication<'a>, PublicationRetirement),
    > {
        self.commit_edit_publication_with_worker(batch, guard, publication, admission, |retired| retired)
            .map_err(|(error, publication, retirement, _)| (error, publication, retirement))
    }

    /// Same final root/cancel owner, with an additional prebuilt ready-head swap.
    /// The callback only exchanges pointers/scalars; all retirements return to
    /// the durable actor. Refusal returns the uncalled callback on its worker.
    pub(crate) fn commit_edit_publication_with_worker<'a, F, R>(
        &self, batch: u32, guard: RecordGuard, publication: GuardedPublication<'a>,
        admission: EditPublicationAdmission, ready_exchange: F,
    ) -> Result<(super::Committed, PublicationRetirement, R),
        (MirrorError, GuardedPublication<'a>, PublicationRetirement, F)>
    where F: FnOnce(RetiredPublication) -> R {
        match self.prepare_edit_publication_worker(batch, guard, publication, admission) {
            Ok(candidate) => candidate.commit_with_worker(ready_exchange),
            Err((error, publication, retirement)) => Err((error, publication, retirement, ready_exchange)),
        }
    }
    /// Prebuild the existing exact guarded Editor/Mirror publication on its
    /// worker. Borrow compatible Generation/Assets before encoding, then consume
    /// or dispose within this same scope; the existing Mirror guard stays owned.
    pub fn prepare_edit_publication_worker<'engine, 'editor>(
        &'engine self, batch: u32, guard: RecordGuard,
        publication: GuardedPublication<'editor>, admission: EditPublicationAdmission,
    ) -> Result<WorkerEditPublication<'engine, 'editor>,
        (MirrorError, GuardedPublication<'editor>, PublicationRetirement)> {
        self.prepare_edit_publication_worker_impl(batch, guard, publication, admission, false)
    }

    /// Opt-in worker-scoped pairing with the exact guarded Editor after-cache.
    /// This recorded sheet prerequisite preserves actual Glyph/image identities;
    /// it has no Core, browser, ACK-spanning or presentation admission caller.
    pub fn prepare_edit_typed_publication_worker<'engine, 'editor>(
        &'engine self, batch: u32, guard: RecordGuard,
        publication: GuardedPublication<'editor>, admission: EditPublicationAdmission,
    ) -> Result<WorkerEditPublication<'engine, 'editor>,
        (MirrorError, GuardedPublication<'editor>, PublicationRetirement)> {
        self.prepare_edit_publication_worker_impl(batch, guard, publication, admission, true)
    }

    fn prepare_edit_publication_worker_impl<'engine, 'editor>(
        &'engine self, batch: u32, guard: RecordGuard,
        publication: GuardedPublication<'editor>, admission: EditPublicationAdmission, typed: bool,
    ) -> Result<WorkerEditPublication<'engine, 'editor>,
        (MirrorError, GuardedPublication<'editor>, PublicationRetirement)> {
        let started = self.now();
        let mut retirement = PublicationRetirement::empty();
        let admitted = admission.batch == batch && admission.guard == guard
            && admission.editor_before.as_ref().unwrap_or(&admission.before).same_node(publication.before_report())
            && admission.after.same_node(publication.report());
        retirement.root_admission = Some(admission);
        if !admitted {
            return Err((MirrorError::Transaction("the prepared root admission does not match this publication"),
                publication, retirement));
        }
        if typed && !publication.report().path(&["_styk", "layerSheet"]).is_some_and(Value::is_object) {
            return Err((MirrorError::Transaction("typed publication requires its recorded layer-sheet scope"), publication, retirement));
        }
        let context = if typed {
            match publication.typed_cache_worker() {
                Ok(context) => Some(context),
                Err(_) => return Err((MirrorError::Transaction("the guarded report has no compatible prepared cache"), publication, retirement)),
            }
        } else { None };
        let mirror = self.inner.mirror.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let admission = retirement.root_admission.as_ref().expect("admitted above");
        match self.prepare_locked(mirror, started, publication.revision(), batch, Some(guard),
            Some((&admission.before, &admission.after, &admission.charge, admission.stored)), context.as_ref()) {
            Ok(candidate) => Ok(WorkerEditPublication { candidate, publication, retirement }),
            Err(error) => Err((error, publication, retirement)),
        }
    }

}

#[cfg(test)]
#[path = "root_tests.rs"]
mod tests;
