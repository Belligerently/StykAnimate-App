//! StykAnimate file formats.
//!
//! - [`container`]: the `.styk` (project), `.stkz` (figure) and `.stykmov`
//!   (movieclip) containers, versions 1 and 2, and plain JSON projects. Contract:
//!   `docs/native-file-format.md`. Reading yields a [`styk_doc::Value`] tree;
//!   writing an unchanged tree yields the bytes it was read from.
//! - [`native`]: the document-level rules every reader applies after decoding
//!   (wrapper unwrap, figure-file shape, embedded source fingerprint).
//!
//! Pivot `.piv`/`.stk` files live in the `styk-piv` crate.

pub mod container;
pub mod native;

pub use container::{ContainerError, Kind};
