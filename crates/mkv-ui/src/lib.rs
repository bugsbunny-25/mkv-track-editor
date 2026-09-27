//! Shared UI layer, compiled for both the native desktop binary and the wasm
//! web binary. Contains the `.slint` definitions and the view-model code
//! that wires them to a [`Backend`] impl. Depends only on `mkv-types`,
//! `mkv-profiles` and `slint` — never `mkv-core` — so it stays wasm-buildable.

pub mod backend;
pub mod document;
pub mod settings;
mod view_model;

slint::include_modules!();

pub use backend::{Backend, BackendCapabilities, ProgressCallback, SourceRef, UiCallback, UiError};
pub use view_model::launch;
