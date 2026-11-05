//! Parallel directory walk (no admin rights needed). STUB: to be implemented.

use std::path::Path;

use super::{ScanOptions, ScanProgress, ScanSummary};
use crate::{Result, SharedTree};

/// Walks `root` in parallel and inserts every entry into `tree`.
pub fn scan(
    _root: &Path,
    _opts: &ScanOptions,
    _tree: &SharedTree,
    _progress: &ScanProgress,
) -> Result<ScanSummary> {
    Err(crate::Error::Unsupported(
        "walk scanner not implemented yet",
    ))
}
