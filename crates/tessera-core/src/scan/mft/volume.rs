//! Opening an NTFS volume for an MFT scan (Windows only).
//!
//! Order of operations in [`scan`]:
//! 1. record the USN journal position (`FSCTL_QUERY_USN_JOURNAL`), so the
//!    watcher can replay every change made from here on;
//! 2. flush the volume (best effort, bounded wait), so changes made before
//!    that point are on disk when the MFT is read raw;
//! 3. `FSCTL_GET_NTFS_VOLUME_DATA` for the geometry, then stream the MFT
//!    through an unbuffered read handle (large aligned reads, no pollution
//!    of the system file cache).

use std::ffi::c_void;
use std::fs::File;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_NO_BUFFERING, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ,
    FILE_SHARE_WRITE, GetVolumeNameForVolumeMountPointW, GetVolumePathNameW, OPEN_EXISTING,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Ioctl::{
    FSCTL_GET_NTFS_VOLUME_DATA, FSCTL_QUERY_USN_JOURNAL, NTFS_VOLUME_DATA_BUFFER,
    USN_JOURNAL_DATA_V0,
};
use windows::core::PCWSTR;

use super::build::Built;
use super::reader::{Geometry, Tuning};
use crate::scan::{
    ScanMode, ScanOptions, ScanProgress, ScanSummary, UsnCheckpoint, is_elevated, volume_info,
};
use crate::{Error, NodeId, Result, SharedTree};

/// How long the pre-scan flush may delay the scan.
const FLUSH_WAIT: Duration = Duration::from_secs(2);

const BACKSLASH: u16 = b'\\' as u16;

fn upper(c: u16) -> u16 {
    if (b'a' as u16..=b'z' as u16).contains(&c) {
        c - 32
    } else {
        c
    }
}

/// If `root` is the root of a volume (`C:\`, or a folder a volume is
/// mounted on), returns the volume's device path `\\?\Volume{GUID}`
/// (NUL terminated, no trailing backslash) for `CreateFileW`.
fn volume_device(root: &Path) -> Option<Vec<u16>> {
    let mut path: Vec<u16> = root
        .as_os_str()
        .encode_wide()
        .map(|c| if c == b'/' as u16 { BACKSLASH } else { c })
        .collect();
    // `\\?\C:\` is the same root as `C:\`.
    let verbatim: Vec<u16> = r"\\?\".encode_utf16().collect();
    if path.starts_with(&verbatim) && path.get(5) == Some(&(b':' as u16)) {
        path.drain(..4);
    }
    if path.is_empty() {
        return None;
    }
    if path.last() != Some(&BACKSLASH) {
        path.push(BACKSLASH);
    }
    path.push(0);

    let mut mount = [0u16; 1024];
    // SAFETY: `path` is NUL terminated; the output slice is writable.
    unsafe { GetVolumePathNameW(PCWSTR(path.as_ptr()), &mut mount) }.ok()?;
    let len = mount.iter().position(|&c| c == 0)?;
    let given = &path[..path.len() - 1];
    if len != given.len()
        || !mount[..len]
            .iter()
            .zip(given)
            .all(|(&a, &b)| upper(a) == upper(b))
    {
        return None; // a folder inside a volume, not its root
    }

    let mut guid = [0u16; 128];
    // SAFETY: `mount` is NUL terminated and ends with a backslash, as the
    // API requires; the output slice is writable.
    unsafe { GetVolumeNameForVolumeMountPointW(PCWSTR(mount.as_ptr()), &mut guid) }.ok()?;
    let len = guid.iter().position(|&c| c == 0)?;
    let mut device = guid[..len].to_vec();
    if device.last() == Some(&BACKSLASH) {
        device.pop(); // with the backslash it would open the root directory
    }
    device.push(0);
    Some(device)
}

/// Opens the volume device. `write` is only used for flushing.
fn open_volume(device: &[u16], write: bool) -> Result<File> {
    let (access, flags) = if write {
        (
            GENERIC_READ.0 | GENERIC_WRITE.0,
            FILE_FLAGS_AND_ATTRIBUTES(0),
        )
    } else {
        (GENERIC_READ.0, FILE_FLAG_NO_BUFFERING)
    };
    // SAFETY: `device` is NUL terminated.
    let handle = unsafe {
        CreateFileW(
            PCWSTR(device.as_ptr()),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            flags,
            None,
        )
    }?;
    // SAFETY: we own the freshly opened handle; File closes it.
    Ok(unsafe { File::from_raw_handle(handle.0) })
}

fn handle(f: &File) -> HANDLE {
    HANDLE(f.as_raw_handle())
}

/// Issues an FSCTL with no input and a fixed-size output structure.
fn fsctl<T: Default>(f: &File, code: u32) -> windows::core::Result<T> {
    let mut out = T::default();
    let mut returned = 0u32;
    // SAFETY: the output buffer is a valid, writable T.
    unsafe {
        DeviceIoControl(
            handle(f),
            code,
            None,
            0,
            Some(&mut out as *mut T as *mut c_void),
            std::mem::size_of::<T>() as u32,
            Some(&mut returned),
            None,
        )
    }?;
    Ok(out)
}

/// The current end of the USN journal, or `None` if there is no journal.
fn usn_checkpoint(vol: &File) -> Option<UsnCheckpoint> {
    let j: USN_JOURNAL_DATA_V0 = fsctl(vol, FSCTL_QUERY_USN_JOURNAL).ok()?;
    Some(UsnCheckpoint {
        journal_id: j.UsnJournalID,
        next_usn: j.NextUsn,
    })
}

fn ntfs_geometry(vol: &File) -> Result<Geometry> {
    let d: NTFS_VOLUME_DATA_BUFFER = fsctl(vol, FSCTL_GET_NTFS_VOLUME_DATA)
        .map_err(|_| Error::Unsupported("not an NTFS volume"))?;
    Geometry {
        sector_size: d.BytesPerSector,
        cluster_size: d.BytesPerCluster,
        record_size: d.BytesPerFileRecordSegment,
        mft_lcn: d.MftStartLcn as u64,
        mft_valid_len: Some(d.MftValidDataLength as u64),
    }
    .validate()
}

/// Asks NTFS to write cached metadata (and data) of the volume to disk, so
/// that the raw MFT read sees recent changes. Best effort: waits at most
/// `FLUSH_WAIT`; a slow flush simply continues in the background.
fn flush_volume(device: &[u16]) {
    let Ok(vol) = open_volume(device, true) else {
        return;
    };
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = vol.sync_all(); // FlushFileBuffers on the volume handle
        let _ = tx.send(());
    });
    let _ = rx.recv_timeout(FLUSH_WAIT);
}

pub(super) fn can_scan(root: &Path) -> bool {
    is_elevated()
        && volume_device(root).is_some()
        && volume_info(root).is_ok_and(|v| v.file_system.eq_ignore_ascii_case("NTFS"))
}

pub(super) fn scan(
    root: &Path,
    _opts: &ScanOptions,
    tree: &SharedTree,
    progress: &ScanProgress,
) -> Result<ScanSummary> {
    let start = Instant::now();
    if progress.is_cancelled() {
        return Err(Error::Cancelled);
    }
    if !is_elevated() {
        return Err(Error::Unsupported(
            "reading the MFT needs administrator rights",
        ));
    }
    let device = volume_device(root).ok_or(Error::Unsupported(
        "an MFT scan needs a volume root such as C:\\",
    ))?;
    let mut vol = open_volume(&device, false)?;
    let usn = usn_checkpoint(&vol);
    flush_volume(&device);
    let geometry = ntfs_geometry(&vol)?;

    let root_name = {
        let t = tree.read().unwrap_or_else(|e| e.into_inner());
        t.name(NodeId::ROOT).to_vec()
    };
    let built = super::scan_source(&mut vol, geometry, &root_name, progress, &Tuning::default())?;
    drop(vol);
    super::publish_progress(progress, &built);
    let Built {
        tree: new_tree,
        frn,
        files,
        dirs,
        skipped,
    } = built;
    *tree.write().unwrap_or_else(|e| e.into_inner()) = new_tree;

    Ok(ScanSummary {
        mode: ScanMode::Mft,
        elapsed: start.elapsed(),
        files,
        dirs,
        inaccessible: skipped,
        volume: volume_info(root).ok(),
        frn_index: Some(frn),
        usn_checkpoint: usn,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_inside_a_volume_are_not_roots() {
        // Works without admin rights; under wine C: maps to a directory.
        let windir = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        assert!(volume_device(Path::new(&windir)).is_none());
        assert!(!can_scan(Path::new(&windir)));
    }
}
