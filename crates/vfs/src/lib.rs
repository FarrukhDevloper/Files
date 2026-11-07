//! Core contract shared by every crate in the workspace.
//!
//! Everything the UI can browse is a [`Backend`] addressed by a [`SourceId`].
//! The local disk and every SSH host implement the same trait, so copy/move/
//! stream code never cares where a file physically lives.

mod backend;
mod error;
pub mod local;
pub mod path;
mod types;

pub use backend::{Backend, BackendResolver, BoxRead, BoxWrite, Capabilities, DiskSpace, WriteMode};
pub use error::{Result, VfsError};
pub use types::{Entry, EntryKind, Location, Place, PlaceKind, SourceId};
