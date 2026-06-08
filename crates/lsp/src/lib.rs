//! `meow-lsp` — first-party language-server surface.
//!
//! PROTOTYPE only: shared resolution plus the `.meow/deps/` shadow symlink map.
//! This crate adds no resolver logic of its own. Editors resolve through the same
//! `meow_loader::Resolver` the runtime uses, then project cached modules onto the
//! global unpacked store for traversal.

mod resolver;
mod shadow;

pub use crate::resolver::{EditorResolution, EditorResolver};
pub use crate::shadow::{ShadowDeps, ShadowError};
