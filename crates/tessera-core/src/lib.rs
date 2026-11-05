//! Tessera core: everything below the UI.
//!
//! * [`tree`]: the compact file tree (40-byte nodes, WTF-8 name pool).
//! * [`scan`]: fills a tree, by parallel directory walk or by reading the
//!   NTFS master file table directly when elevated.
//! * [`watch`]: live, batched incremental updates (ReadDirectoryChangesW / USN).
//! * [`delete`]: deletion to the recycle bin (or permanently) via `IFileOperation`.
//! * [`layout`]: squarified treemap layout and the binary rectangle format
//!   sent to the front end.
//!
//! Hard rule from the architecture doc: the full tree stays in Rust. The UI
//! only ever receives the rectangles currently on screen.

pub mod category;
pub mod delete;
mod error;
pub mod layout;
pub mod scan;
pub mod tree;
pub mod watch;
pub mod wtf8;

pub use error::{Error, Result};
pub use tree::{EntryBatch, Node, NodeFlags, NodeId, SharedTree, Tree};
