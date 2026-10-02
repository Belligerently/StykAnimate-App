//! Effects: drawing groups the way the editor filters artwork. `plan` places
//! layers for a view; `executor` runs effect stacks on them. See
//! `docs/engine/vfx.md` (Rendering).

pub mod executor;
pub mod plan;
