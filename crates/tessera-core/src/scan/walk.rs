//! Parallel directory walk (no admin rights needed). STUB: to be implemented.

use std::path::Path;

use super::{ScanOptions, ScanProgress, ScanSummary};
use crate::{NodeId, Result, SharedTree};

/// Walks `root` in parallel and inserts every entry into `tree`, whose
/// root node must have no children yet.
pub fn scan(
    root: &Path,
    opts: &ScanOptions,
    tree: &SharedTree,
    progress: &ScanProgress,
) -> Result<ScanSummary> {
    scan_into(tree, NodeId::ROOT, root, opts, progress)
}

/// Walks the directory at `path` and inserts its contents below the
/// existing directory node `dir`, which must have no live children.
/// Used for the initial scan (with the root) and by the watcher to fill in
/// a newly created or moved-in directory.
pub fn scan_into(
    _tree: &SharedTree,
    _dir: NodeId,
    _path: &Path,
    _opts: &ScanOptions,
    _progress: &ScanProgress,
) -> Result<ScanSummary> {
    Err(crate::Error::Unsupported(
        "walk scanner not implemented yet",
    ))
}
