//! Worker-only accounting of the encoder's retained CPU allocations.
use super::DrawList;
use crate::geometry::{Resolved, arc_allocation};
use crate::vfx::Effect;
use std::collections::HashSet;
use std::mem::size_of;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum RetainedBytesError {
    #[error("draw-list retained allocation size overflow")]
    Overflow,
    #[error("draw-list layer {layer} contains an unknown effect payload")]
    UnknownEffects { layer: usize },
}

struct Total(usize);
impl Total {
    fn add(&mut self, bytes: usize) -> Result<(), RetainedBytesError> {
        self.0 = self
            .0
            .checked_add(bytes)
            .ok_or(RetainedBytesError::Overflow)?;
        Ok(())
    }
    fn buffer<T>(&mut self, values: &Vec<T>) -> Result<(), RetainedBytesError> {
        self.add(
            values
                .capacity()
                .checked_mul(size_of::<T>())
                .ok_or(RetainedBytesError::Overflow)?,
        )
    }
}

/// Requested retained CPU allocation bytes for an immutable encoder result.
/// Run once on its worker, never on readiness/presentation paths. Vec capacities
/// (including passes), Arc headers/alignment, typed stack containers, resolved
/// effects/paints/ids and glyph triangle slices are counted. Shared allocations
/// inside this list count once; separate retained payloads may charge them again.
/// Includes the eventual DrawList Arc allocation. No Arc is retained; the
/// existing parameter accessor may briefly clone a paint Arc while inspecting it.
/// Unknown public Any payloads and overflow refuse an accounting result.
///
/// Scratch HashSets are transient worker work. Authored documents, Frame/Scene,
/// Generation/Assets, renderer/GPU resources and allocator overhead are outside
/// this charge. This neither reserves memory nor changes logical `bytes()` or
/// cache admission. Native callers changing public buffers must recompute it.
pub fn retained_bytes_checked(list: &DrawList) -> Result<usize, RetainedBytesError> {
    let mut bytes = Total(arc_allocation::<DrawList>(1).ok_or(RetainedBytesError::Overflow)?);
    bytes.buffer(&list.prims)?;
    bytes.buffer(&list.paints)?;
    bytes.buffer(&list.points)?;
    bytes.buffer(&list.groups)?;
    bytes.buffer(&list.passes)?;
    bytes.buffer(&list.layers)?;
    bytes.buffer(&list.glyphs)?;
    let mut stacks = HashSet::new();
    let mut resolved = Resolved::default();
    for (layer, value) in list.layers.iter().enumerate() {
        let Some(effects) = &value.effects else {
            continue;
        };
        let Some(stack) = effects.downcast_ref::<Vec<Arc<Effect>>>() else {
            return Err(RetainedBytesError::UnknownEffects { layer });
        };
        if stacks.insert(Arc::as_ptr(effects) as *const ()) {
            bytes
                .add(arc_allocation::<Vec<Arc<Effect>>>(1).ok_or(RetainedBytesError::Overflow)?)?;
            bytes.buffer(stack)?;
            resolved.stack(stack);
        }
    }
    bytes.add(
        resolved
            .checked_bytes()
            .ok_or(RetainedBytesError::Overflow)?,
    )?;
    let mut glyphs = HashSet::new();
    for glyph in &list.glyphs {
        if glyphs.insert(Arc::as_ptr(&glyph.triangles) as *const ()) {
            bytes.add(
                arc_allocation::<[f32; 2]>(glyph.triangles.len())
                    .ok_or(RetainedBytesError::Overflow)?,
            )?;
        }
    }
    Ok(bytes.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checked_outer_charge_cannot_wrap() {
        let mut bytes = Total(usize::MAX);
        assert_eq!(bytes.add(1), Err(RetainedBytesError::Overflow));
        assert_eq!(bytes.0, usize::MAX);
    }
}
