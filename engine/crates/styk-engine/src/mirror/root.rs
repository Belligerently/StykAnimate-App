//! Canonical worker Editor roots in the same guarded mirror batch owner.
//! No raw root setter, serialized document, alternate diff, store or Loader.
use std::collections::LinkedList;
use std::sync::Arc;
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
        self.stage_publication_response_worker(batch, guard, prepared, response, None, check)
    }

    /// The actor aliases its prebuilt small cancellation atom into the SAME
    /// Batch owner. UI cancellation reaches final paired publication without
    /// acquiring State/staged or detaching any payload on that interface.
    pub(crate) fn stage_session_publication_response_worker(
        &self, batch: u32, guard: RecordGuard, prepared: &PreparedPublication,
        response: &Value, cancelled: Arc<AtomicBool>, check: impl FnMut() -> Result<(), MirrorError>,
    ) -> Result<EditPublicationAdmission, MirrorError> {
        self.stage_publication_response_worker(batch, guard, prepared, response, Some(cancelled), check)
    }
    fn stage_publication_response_worker(
        &self, batch: u32, guard: RecordGuard, prepared: &PreparedPublication,
        response: &Value, cancelled: Option<Arc<AtomicBool>>, mut check: impl FnMut() -> Result<(), MirrorError>,
    ) -> Result<EditPublicationAdmission, MirrorError> {
        check()?;
        {
            let mirror = self
                .inner
                .mirror
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !mirror.tree.same_node(prepared.before_report())
                || prepared.before_revision() != guard.revision
                || !self.record_guard_current(guard)
            {
                return Err(MirrorError::Transaction(
                    "the prepared Editor does not own this mirror generation",
                ));
            }
        }
        let budget = {
            let staged = self.inner.staged.lock();
            staged.get(&batch).map(|owner| owner.budget.clone())
        }
        .unwrap_or_else(|| Arc::new(Budget::default()));
        let mut charge = Charge::new(self.inner.record_budget.clone(), budget.clone())
            .map_err(|error| MirrorError::Record(error.to_string()))?;
        charge
            .grow(256)
            .map_err(|error| MirrorError::Record(error.to_string()))?;
        let mut accounted = 0;
        styk_doc::retained_bytes_checked(
            &[
                prepared.before_report(), prepared.report(), prepared.result(),
                prepared.state(), prepared.history(), prepared.events(),
                prepared.before_state(), prepared.before_history(),
                response,
            ],
            |bytes| {
                check()?;
                charge
                    .grow(bytes.saturating_sub(accounted))
                    .map_err(|error| MirrorError::Record(error.to_string()))?;
                accounted = bytes;
                if !self.record_guard_current(guard) {
                    return Err(MirrorError::Transaction(
                        "the prepared Editor belongs to a stale generation",
                    ));
                }
                Ok(())
            },
        )?;
        check()?;
        let charge = Arc::new(charge);
        let admission = EditPublicationAdmission {
            batch,
            guard,
            before: prepared.before_report().clone(),
            after: prepared.report().clone(),
            charge: charge.clone(),
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
        let started = self.now();
        let mut retained = PublicationRetirement::empty();
        let admitted = admission.batch == batch
            && admission.guard == guard
            && admission.before.same_node(publication.before_report())
            && admission.after.same_node(publication.report());
        retained.root_admission = Some(admission);
        if !admitted {
            return Err((
                MirrorError::Transaction(
                    "the prepared root admission does not match this publication",
                ),
                publication,
                retained,
            ));
        }
        let mirror = self
            .inner
            .mirror
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let before = publication.before_report().clone();
        let after = publication.report().clone();
        let revision = publication.revision();
        let mut publication = Some(publication);
        let mut exchange = Some(|| {
            publication
                .take()
                .expect("one guarded head exchange")
                .commit()
        });
        match self.commit_locked_with(
            mirror,
            started,
            revision,
            batch,
            Some(guard),
            Some((
                &before,
                &after,
                &retained
                    .root_admission
                    .as_ref()
                    .expect("admitted above")
                    .charge,
            )),
            &mut exchange,
        ) {
            Ok((done, mut retirement, editor)) => {
                retirement.root_admission = retained.root_admission.take();
                Ok((done, retirement, editor))
            }
            Err(error) => {
                drop(exchange);
                Err((
                    error,
                    publication.take().expect("refused before head exchange"),
                    retained,
                ))
            }
        }
    }
}

#[cfg(test)]
#[path = "root_tests.rs"]
mod tests;
