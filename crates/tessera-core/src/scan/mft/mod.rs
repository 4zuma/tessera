//! Direct NTFS master file table reader (admin only on Windows).
//!
//! * `parser`: decodes file records into compact facts (portable).
//! * `reader`: finds the MFT through `$MFT`'s own runlist and streams it
//!   in large chunks, parsing each chunk in parallel (portable; reads any
//!   [`Read`] + [`Seek`] source, e.g. a raw NTFS image).
//! * `build`: merges the facts and builds the [`Tree`] breadth first
//!   (portable).
//! * `volume`: opening `\\.\C:`, the volume and USN journal queries
//!   (Windows only).
//!
//! What ends up in the tree:
//! * Every in-use record reachable from the root directory (record 5) by
//!   its non-DOS names. Record 5 becomes the existing root node.
//! * File sizes from the unnamed `$DATA` stream: resident data counts as
//!   logical bytes but 0 bytes on disk (it lives in the MFT record);
//!   non-resident data uses the allocated size, or the total allocated
//!   size when compressed or sparse. Named streams are ignored, except the
//!   `WofCompressedData` stream of WOF-compressed files (that is where
//!   their data is) and the named streams of NTFS metadata files such as
//!   `$UsnJrnl:$J` and `$Secure:$SDS`.
//! * NTFS metadata files (`$MFT`, `$LogFile`, everything under `$Extend`)
//!   are flagged `METAFILE`: they really occupy space.
//! * A file with several names (hard links) is counted once: the name
//!   reached first breadth first gets the sizes; the other names are
//!   `HARDLINK` nodes with zero sizes.
//! * Records that cannot be reached (parent missing, stale parent
//!   reference, directory cycle) and corrupt records are skipped and
//!   reported as `inaccessible`.

use std::io::{Read, Seek};
#[cfg(windows)]
use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::Instant;

use super::{ScanMode, ScanProgress, ScanSummary};
use crate::{Error, NodeId, Result, Tree};

mod build;
mod parser;
mod reader;
#[cfg(windows)]
mod volume;

#[cfg(test)]
mod bench;
#[cfg(all(test, target_os = "linux"))]
mod image_tests;
#[cfg(test)]
mod testutil;

use build::{Accum, Built};
use reader::{Geometry, ReadAt, Tuning};

#[cfg(windows)]
use super::ScanOptions;
#[cfg(windows)]
use crate::SharedTree;

/// True if an MFT scan of `root` can work: elevated, NTFS, volume root.
#[cfg(windows)]
pub fn can_scan(root: &Path) -> bool {
    volume::can_scan(root)
}

/// Reads the MFT of the volume whose root is `root` and replaces `tree`
/// (whose root node name is kept) with the result. Fills the summary's
/// `frn_index` and `usn_checkpoint` for the USN watcher.
#[cfg(windows)]
pub fn scan(
    root: &Path,
    opts: &ScanOptions,
    tree: &SharedTree,
    progress: &ScanProgress,
) -> Result<ScanSummary> {
    volume::scan(root, opts, tree, progress)
}

/// Builds a tree from a raw NTFS volume image (or any reader positioned
/// over a raw NTFS volume), with root node name `root_name`. Portable: used
/// for tests and offline analysis of images.
pub fn scan_reader<R: Read + Seek + Send>(
    mut reader: R,
    root_name: &[u8],
    progress: &ScanProgress,
) -> Result<(Tree, ScanSummary)> {
    let start = Instant::now();
    let mut boot = reader::AlignedBuf::new(512, 512);
    reader.read_exact_at(0, &mut boot)?;
    let geometry = Geometry::from_boot_sector(&boot)?;
    let built = scan_source(
        &mut reader,
        geometry,
        root_name,
        progress,
        &Tuning::default(),
    )?;
    publish_progress(progress, &built);
    let Built {
        tree,
        frn,
        files,
        dirs,
        skipped,
    } = built;
    Ok((
        tree,
        ScanSummary {
            mode: ScanMode::Mft,
            elapsed: start.elapsed(),
            files,
            dirs,
            inaccessible: skipped,
            volume: None,
            frn_index: Some(frn),
            usn_checkpoint: None,
        },
    ))
}

/// Locates the MFT, streams it through the parser and builds the tree.
fn scan_source<R: ReadAt>(
    src: &mut R,
    geometry: Geometry,
    root_name: &[u8],
    progress: &ScanProgress,
    tuning: &Tuning,
) -> Result<Built> {
    if progress.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let mut layout = reader::load_layout(src, &geometry)?;
    if !tuning.use_bitmap {
        layout.bitmap = None;
    }
    let in_use = layout.in_use_count() as usize;
    let mut acc = Accum::new(
        root_name,
        layout.records as usize,
        in_use,
        u64::from(geometry.cluster_size),
    );
    reader::stream_records(src, &layout, tuning, &mut acc, progress)?;
    drop(layout);
    if progress.is_cancelled() {
        return Err(Error::Cancelled);
    }
    Ok(acc.finish())
}

/// Replaces the running estimates with the final counts.
fn publish_progress(progress: &ScanProgress, b: &Built) {
    progress.files.store(b.files, Ordering::Relaxed);
    progress.dirs.store(b.dirs, Ordering::Relaxed);
    progress
        .bytes
        .store(b.tree.node(NodeId::ROOT).size(), Ordering::Relaxed);
    progress.inaccessible.store(b.skipped, Ordering::Relaxed);
}
