//! Direct NTFS master file table reader (admin only). STUB: to be implemented.
//!
//! The parser is platform independent (it reads from any `Read + Seek`
//! source, so it can be tested on a disk image); only opening the volume
//! is Windows specific.

#[cfg(windows)]
use std::path::Path;

#[cfg(windows)]
use super::{ScanOptions, ScanProgress, ScanSummary};
#[cfg(windows)]
use crate::{Result, SharedTree};

/// True if an MFT scan of `root` can work: elevated, NTFS, volume root.
#[cfg(windows)]
pub fn can_scan(_root: &Path) -> bool {
    false
}

/// Reads the MFT of the volume whose root is `root` and replaces `tree`.
#[cfg(windows)]
pub fn scan(
    _root: &Path,
    _opts: &ScanOptions,
    _tree: &SharedTree,
    _progress: &ScanProgress,
) -> Result<ScanSummary> {
    Err(crate::Error::Unsupported("MFT scanner not implemented yet"))
}
