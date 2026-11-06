//! The system calls behind the walk: opening a directory, listing it in
//! large batches, and the facts about the volume that the walk needs.
//!
//! Paths are NT paths (`\??\C:\dir`) without a terminating NUL, which is
//! what `NtOpenFile` takes directly; `\??\` is the NT spelling of the Win32
//! `\\?\` prefix, so no length limit and no Win32 path rewriting applies.
//! Errors are returned as Win32 error codes so callers can classify them
//! uniformly.

use std::mem::{offset_of, size_of};
use std::path::Path;

use windows::Wdk::Foundation::{NtClose, OBJECT_ATTRIBUTES};
use windows::Wdk::Storage::FileSystem::{
    FILE_DIRECTORY_FILE, FILE_FULL_DIR_INFORMATION, FILE_ID_FULL_DIR_INFORMATION,
    FILE_INFORMATION_CLASS, FILE_OPEN_FOR_BACKUP_INTENT, FILE_OPEN_REPARSE_POINT,
    FILE_SYNCHRONOUS_IO_NONALERT, FileAttributeTagInformation, FileFullDirectoryInformation,
    FileIdFullDirectoryInformation, NtOpenFile, NtQueryDirectoryFile, NtQueryInformationFile,
};
use windows::Wdk::System::SystemServices::FILE_ATTRIBUTE_TAG_INFORMATION;
use windows::Win32::Foundation::{
    GetLastError, HANDLE, NTSTATUS, OBJ_CASE_INSENSITIVE, RtlNtStatusToDosError, UNICODE_STRING,
};
use windows::Win32::Storage::FileSystem::{
    FIND_FIRST_EX_LARGE_FETCH, FindClose, FindExInfoBasic, FindExSearchNameMatch, FindFirstFileExW,
    FindNextFileW, GetDiskFreeSpaceW, GetDriveTypeW, GetVolumeInformationW, GetVolumePathNameW,
    WIN32_FIND_DATAW,
};
use windows::Win32::System::IO::IO_STATUS_BLOCK;
use windows::core::{PCWSTR, PWSTR};

// Win32 error codes the walk tells apart.
pub(super) const ERROR_FILE_NOT_FOUND: u32 = 2;
pub(super) const ERROR_PATH_NOT_FOUND: u32 = 3;
pub(super) const ERROR_NO_MORE_FILES: u32 = 18;
pub(super) const ERROR_BAD_NETPATH: u32 = 53;
pub(super) const ERROR_BAD_NET_NAME: u32 = 67;
pub(super) const ERROR_INVALID_NAME: u32 = 123;
pub(super) const ERROR_FILENAME_EXCED_RANGE: u32 = 206;
pub(super) const ERROR_DIRECTORY: u32 = 267;

// NTSTATUS values from the documentation of NtQueryDirectoryFile.
const STATUS_SUCCESS: i32 = 0;
const STATUS_NO_MORE_FILES: i32 = 0x8000_0006_u32 as i32;
const STATUS_NOT_IMPLEMENTED: i32 = 0xC000_0002_u32 as i32;
const STATUS_INVALID_INFO_CLASS: i32 = 0xC000_0003_u32 as i32;
const STATUS_INVALID_PARAMETER: i32 = 0xC000_000D_u32 as i32;
const STATUS_NO_SUCH_FILE: i32 = 0xC000_000F_u32 as i32;
const STATUS_INVALID_DEVICE_REQUEST: i32 = 0xC000_0010_u32 as i32;
const STATUS_NOT_SUPPORTED: i32 = 0xC000_00BB_u32 as i32;
const STATUS_INVALID_LEVEL: i32 = 0xC000_0148_u32 as i32;

// Access rights and share modes (winnt.h).
const FILE_LIST_DIRECTORY: u32 = 0x0001;
const FILE_READ_ATTRIBUTES: u32 = 0x0080;
const SYNCHRONIZE: u32 = 0x0010_0000;
const FILE_SHARE_ALL: u32 = 0x1 | 0x2 | 0x4; // read | write | delete

// File attributes (winnt.h).
pub(super) const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
pub(super) const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;
pub(super) const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
pub(super) const FILE_ATTRIBUTE_SPARSE_FILE: u32 = 0x200;
pub(super) const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
pub(super) const FILE_ATTRIBUTE_COMPRESSED: u32 = 0x800;
pub(super) const FILE_ATTRIBUTE_OFFLINE: u32 = 0x1000;
pub(super) const FILE_ATTRIBUTE_RECALL_ON_OPEN: u32 = 0x4_0000;
pub(super) const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;

/// Reparse tags whose target is another named entity (junctions, symbolic
/// links, mount points) have this bit set (`IsReparseTagNameSurrogate`).
pub(super) const REPARSE_TAG_NAME_SURROGATE: u32 = 0x2000_0000;
/// Windows Overlay Filter: a file compressed by "compact OS".
pub(super) const IO_REPARSE_TAG_WOF: u32 = 0x8000_0017;

const FILE_SUPPORTS_HARD_LINKS: u32 = 0x0040_0000;
const DRIVE_REMOTE: u32 = 4;

#[inline]
pub(super) fn is_name_surrogate(tag: u32) -> bool {
    tag & REPARSE_TAG_NAME_SURROGATE != 0
}

#[inline]
fn status_to_win32(status: NTSTATUS) -> u32 {
    // SAFETY: a pure lookup in ntdll.
    unsafe { RtlNtStatusToDosError(status) }
}

/// Which directory query the walk uses, from the richest to the most
/// widely supported. The walk steps down when a file system rejects one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub(crate) enum Level {
    /// `NtQueryDirectoryFile(FileIdFullDirectoryInformation)`: sizes,
    /// attributes, reparse tag and file id. Unlike the `...Both...` classes
    /// it carries no 8.3 short name, which NTFS can only supply by reading
    /// each file's MFT record; without it a listing is served from the
    /// directory index alone.
    #[default]
    IdFull = 0,
    /// `NtQueryDirectoryFile(FileFullDirectoryInformation)`: as above
    /// without file ids (no hard-link detection).
    Full = 1,
    /// `FindFirstFileExW(FindExInfoBasic, FIND_FIRST_EX_LARGE_FETCH)`:
    /// logical sizes only; size on disk is estimated from the cluster size.
    Find = 2,
}

impl Level {
    pub(super) fn from_u8(v: u8) -> Level {
        match v {
            0 => Level::IdFull,
            1 => Level::Full,
            _ => Level::Find,
        }
    }

    pub(super) fn next(self) -> Level {
        match self {
            Level::IdFull => Level::Full,
            Level::Full | Level::Find => Level::Find,
        }
    }
}

/// One directory entry, borrowed from the query buffer.
pub(super) struct Entry<'a> {
    pub name: &'a [u16],
    pub attributes: u32,
    /// `EndOfFile`: the logical length.
    pub logical: u64,
    /// `AllocationSize`: bytes on disk, if the query reports it.
    pub allocated: Option<u64>,
    /// File id (NTFS: file reference number), 0 if unknown.
    pub file_id: u64,
    /// Reparse tag if `FILE_ATTRIBUTE_REPARSE_POINT` is set and the file
    /// system reports it (in `EaSize`), else 0.
    pub reparse_tag: u32,
}

/// How to open a directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Open {
    /// Follow a reparse point (the scan root: the user named it).
    Follow,
    /// Open the directory itself even if it is a reparse point, so a link
    /// that appeared since its parent was listed is never followed.
    NoFollow,
    /// Like `NoFollow`, plus the right to read its reparse tag.
    NoFollowReadTag,
}

/// An open directory handle, closed on drop.
pub(super) struct DirHandle(HANDLE);

impl Drop for DirHandle {
    fn drop(&mut self) {
        // SAFETY: we own the handle and close it once.
        unsafe {
            let _ = NtClose(self.0);
        }
    }
}

/// Opens the directory at the NT path `path` for listing.
pub(super) fn open_dir(path: &[u16], open: Open) -> Result<DirHandle, u32> {
    let bytes = path.len() * 2;
    let Ok(len) = u16::try_from(bytes) else {
        return Err(ERROR_FILENAME_EXCED_RANGE);
    };
    let name = UNICODE_STRING {
        Length: len,
        MaximumLength: len,
        Buffer: PWSTR(path.as_ptr() as *mut u16),
    };
    let attrs = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: HANDLE::default(),
        ObjectName: &name,
        Attributes: OBJ_CASE_INSENSITIVE,
        SecurityDescriptor: std::ptr::null(),
        SecurityQualityOfService: std::ptr::null(),
    };
    let mut access = FILE_LIST_DIRECTORY | SYNCHRONIZE;
    // Backup intent lets a caller that enabled SeBackupPrivilege read
    // everything; without the privilege it changes nothing.
    let mut options =
        FILE_DIRECTORY_FILE | FILE_SYNCHRONOUS_IO_NONALERT | FILE_OPEN_FOR_BACKUP_INTENT;
    match open {
        Open::Follow => {}
        Open::NoFollow => options |= FILE_OPEN_REPARSE_POINT,
        Open::NoFollowReadTag => {
            options |= FILE_OPEN_REPARSE_POINT;
            access |= FILE_READ_ATTRIBUTES;
        }
    }
    let mut handle = HANDLE::default();
    let mut iosb = IO_STATUS_BLOCK::default();
    // SAFETY: all pointers are valid for the duration of the call; `name`
    // borrows `path`, which outlives it.
    let status = unsafe {
        NtOpenFile(
            &mut handle,
            access,
            &attrs,
            &mut iosb,
            FILE_SHARE_ALL,
            options.0,
        )
    };
    if status.0 < 0 {
        return Err(status_to_win32(status));
    }
    Ok(DirHandle(handle))
}

/// The reparse tag of an open directory, or 0 if it is not a reparse point.
/// The handle must have been opened with [`Open::NoFollowReadTag`].
pub(super) fn reparse_tag(dir: &DirHandle) -> Result<u32, u32> {
    let mut info = FILE_ATTRIBUTE_TAG_INFORMATION::default();
    let mut iosb = IO_STATUS_BLOCK::default();
    // SAFETY: `info` is a properly sized, writable output buffer.
    let status = unsafe {
        NtQueryInformationFile(
            dir.0,
            &mut iosb,
            (&mut info as *mut FILE_ATTRIBUTE_TAG_INFORMATION).cast(),
            size_of::<FILE_ATTRIBUTE_TAG_INFORMATION>() as u32,
            FileAttributeTagInformation,
        )
    };
    if status.0 < 0 {
        return Err(status_to_win32(status));
    }
    Ok(if info.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        info.ReparseTag
    } else {
        0
    })
}

/// Why a listing stopped early.
pub(super) enum ListError {
    /// The file system does not support this query level; nothing was
    /// delivered, so the caller can retry with [`Level::next`].
    Unsupported,
    /// Any other failure (Win32 error code). Entries delivered before it
    /// are valid.
    Failed(u32),
}

/// Field offsets of one `NtQueryDirectoryFile` record layout.
struct Layout {
    class: FILE_INFORMATION_CLASS,
    end_of_file: usize,
    allocation_size: usize,
    attributes: usize,
    name_length: usize,
    ea_size: usize,
    file_id: Option<usize>,
    name: usize,
}

const ID_FULL: Layout = Layout {
    class: FileIdFullDirectoryInformation,
    end_of_file: offset_of!(FILE_ID_FULL_DIR_INFORMATION, EndOfFile),
    allocation_size: offset_of!(FILE_ID_FULL_DIR_INFORMATION, AllocationSize),
    attributes: offset_of!(FILE_ID_FULL_DIR_INFORMATION, FileAttributes),
    name_length: offset_of!(FILE_ID_FULL_DIR_INFORMATION, FileNameLength),
    ea_size: offset_of!(FILE_ID_FULL_DIR_INFORMATION, EaSize),
    file_id: Some(offset_of!(FILE_ID_FULL_DIR_INFORMATION, FileId)),
    name: offset_of!(FILE_ID_FULL_DIR_INFORMATION, FileName),
};

const FULL: Layout = Layout {
    class: FileFullDirectoryInformation,
    end_of_file: offset_of!(FILE_FULL_DIR_INFORMATION, EndOfFile),
    allocation_size: offset_of!(FILE_FULL_DIR_INFORMATION, AllocationSize),
    attributes: offset_of!(FILE_FULL_DIR_INFORMATION, FileAttributes),
    name_length: offset_of!(FILE_FULL_DIR_INFORMATION, FileNameLength),
    ea_size: offset_of!(FILE_FULL_DIR_INFORMATION, EaSize),
    file_id: None,
    name: offset_of!(FILE_FULL_DIR_INFORMATION, FileName),
};

/// Bounds-checked little-endian reads from the query buffer, viewed as
/// UTF-16 units (every field and name starts at an even offset).
struct Records<'a>(&'a [u16]);

impl Records<'_> {
    #[inline]
    fn u32_at(&self, byte_off: usize) -> Option<u32> {
        let i = byte_off / 2;
        let w = self.0.get(i..i + 2)?;
        Some(w[0] as u32 | (w[1] as u32) << 16)
    }

    #[inline]
    fn u64_at(&self, byte_off: usize) -> Option<u64> {
        Some(self.u32_at(byte_off)? as u64 | (self.u32_at(byte_off + 4)? as u64) << 32)
    }
}

/// Lists the open directory `dir` with `NtQueryDirectoryFile`, reusing
/// `buf` for every call, and hands each entry (including `.` and `..`) to
/// `sink`. Stops early, returning `Ok`, once `cancelled()` is true.
pub(super) fn list(
    dir: &DirHandle,
    level: Level,
    buf: &mut [u64],
    cancelled: &dyn Fn() -> bool,
    sink: &mut dyn FnMut(&Entry<'_>),
) -> Result<(), ListError> {
    let layout = match level {
        Level::IdFull => &ID_FULL,
        Level::Full => &FULL,
        Level::Find => unreachable!("FindFirstFileExW does not use a handle"),
    };
    let buf_bytes = buf.len() * 8;
    let mut first = true;
    loop {
        let mut iosb = IO_STATUS_BLOCK::default();
        // SAFETY: `buf` is writable for `buf_bytes` bytes and 8-byte
        // aligned; the handle is synchronous, so the call completes before
        // it returns and nothing writes to `buf` or `iosb` afterwards.
        let status = unsafe {
            NtQueryDirectoryFile(
                dir.0,
                None,
                None,
                None,
                &mut iosb,
                buf.as_mut_ptr().cast(),
                buf_bytes as u32,
                layout.class,
                false,
                None,
                first,
            )
        };
        match status.0 {
            STATUS_SUCCESS => {}
            STATUS_NO_MORE_FILES => return Ok(()),
            STATUS_NO_SUCH_FILE if first => return Ok(()),
            STATUS_INVALID_INFO_CLASS
            | STATUS_INVALID_LEVEL
            | STATUS_NOT_IMPLEMENTED
            | STATUS_NOT_SUPPORTED
            | STATUS_INVALID_PARAMETER
            | STATUS_INVALID_DEVICE_REQUEST
                if first =>
            {
                return Err(ListError::Unsupported);
            }
            _ => return Err(ListError::Failed(status_to_win32(status))),
        }
        first = false;
        let filled = iosb.Information.min(buf_bytes);
        if filled == 0 {
            return Ok(());
        }
        // SAFETY: the first `filled` bytes were written by the call above;
        // u16 has a weaker alignment than the u64 buffer.
        let units = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u16>(), filled / 2) };
        parse(&Records(units), layout, sink);
        if cancelled() {
            return Ok(());
        }
    }
}

/// Walks the `NextEntryOffset` chain of one filled buffer. Malformed data
/// ends the walk of this buffer instead of reading out of bounds.
fn parse(rec: &Records<'_>, layout: &Layout, sink: &mut dyn FnMut(&Entry<'_>)) {
    let mut off = 0usize;
    loop {
        let (Some(next), Some(attributes), Some(name_len)) = (
            rec.u32_at(off),
            rec.u32_at(off + layout.attributes),
            rec.u32_at(off + layout.name_length),
        ) else {
            return;
        };
        let start = (off + layout.name) / 2;
        let Some(name) = rec.0.get(start..start + name_len as usize / 2) else {
            return;
        };
        let reparse_tag = if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            rec.u32_at(off + layout.ea_size).unwrap_or(0)
        } else {
            0
        };
        sink(&Entry {
            name,
            attributes,
            logical: rec.u64_at(off + layout.end_of_file).unwrap_or(0),
            allocated: rec.u64_at(off + layout.allocation_size),
            file_id: layout
                .file_id
                .and_then(|o| rec.u64_at(off + o))
                .unwrap_or(0),
            reparse_tag,
        });
        if next == 0 || next % 2 != 0 {
            return;
        }
        off += next as usize;
    }
}

/// Lists the directory at the NT path `path` with `FindFirstFileExW`, for
/// file systems that support none of the `NtQueryDirectoryFile` levels.
pub(super) fn list_find(
    path: &[u16],
    cancelled: &dyn Fn() -> bool,
    sink: &mut dyn FnMut(&Entry<'_>),
) -> Result<(), u32> {
    let mut pattern = win32_path(path);
    if pattern.last() != Some(&(b'\\' as u16)) {
        pattern.push(b'\\' as u16);
    }
    pattern.extend_from_slice(&[b'*' as u16, 0]);
    let mut data = WIN32_FIND_DATAW::default();
    // SAFETY: `pattern` is NUL-terminated and `data` is a writable
    // WIN32_FIND_DATAW, as FindExInfoBasic requires.
    let find = unsafe {
        FindFirstFileExW(
            PCWSTR(pattern.as_ptr()),
            FindExInfoBasic,
            (&mut data as *mut WIN32_FIND_DATAW).cast(),
            FindExSearchNameMatch,
            None,
            FIND_FIRST_EX_LARGE_FETCH,
        )
    };
    let find = match find {
        Ok(h) => h,
        Err(_) => {
            // SAFETY: reads the calling thread's last-error value.
            let code = unsafe { GetLastError() }.0;
            // No entry matched at all, not even "." (e.g. an empty root).
            return if code == ERROR_FILE_NOT_FOUND {
                Ok(())
            } else {
                Err(code)
            };
        }
    };
    let result = loop {
        let name_len = data.cFileName.iter().position(|&c| c == 0).unwrap_or(260);
        let attributes = data.dwFileAttributes;
        sink(&Entry {
            name: &data.cFileName[..name_len],
            attributes,
            logical: (data.nFileSizeHigh as u64) << 32 | data.nFileSizeLow as u64,
            allocated: None,
            file_id: 0,
            reparse_tag: if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                data.dwReserved0
            } else {
                0
            },
        });
        if cancelled() {
            break Ok(());
        }
        // SAFETY: `find` is a live search handle and `data` is writable.
        if unsafe { FindNextFileW(find, &mut data) }.is_err() {
            // SAFETY: reads the calling thread's last-error value.
            let code = unsafe { GetLastError() }.0;
            break if code == ERROR_NO_MORE_FILES {
                Ok(())
            } else {
                Err(code)
            };
        }
    };
    // SAFETY: closes the search handle opened above, once.
    unsafe {
        let _ = FindClose(find);
    }
    result
}

/// The Win32 spelling (`\\?\...`) of an NT path from [`root_path`], not
/// NUL-terminated (with room to append a pattern and a NUL).
fn win32_path(nt: &[u16]) -> Vec<u16> {
    let mut w = Vec::with_capacity(nt.len() + 3);
    w.extend_from_slice(nt);
    if w.len() >= 4 {
        w[1] = b'\\' as u16; // \??\ -> \\?\
    }
    w
}

/// The Win32 verbatim spelling (`\\?\...`) of an NT path, as a path.
pub(super) fn win32_root(nt: &[u16]) -> std::path::PathBuf {
    use std::os::windows::ffi::OsStringExt;
    std::ffi::OsString::from_wide(&win32_path(nt)).into()
}

/// Turns a user-supplied directory path into the NT path the walk opens:
/// absolute, `\??\`-prefixed (so paths beyond `MAX_PATH` work), backslash
/// separated, and without a trailing separator except where one is part of
/// the name (`\??\C:\`: without it, `\??\C:` would open the volume itself).
pub(super) fn root_path(path: &Path) -> std::io::Result<Vec<u16>> {
    use std::os::windows::ffi::OsStrExt;
    let abs = std::path::absolute(path)?;
    let w: Vec<u16> = abs.as_os_str().encode_wide().collect();
    let bs = b'\\' as u16;
    let starts = |p: &str| {
        let p: Vec<u16> = p.encode_utf16().collect();
        w.len() >= p.len() && w[..p.len()] == p[..]
    };
    let mut out: Vec<u16> = Vec::with_capacity(w.len() + 8);
    out.extend(r"\??\".encode_utf16());
    if starts(r"\\?\") || starts(r"\??\") || starts(r"\\.\") {
        out.extend_from_slice(&w[4..]);
    } else if starts(r"\\") {
        out.extend(r"UNC\".encode_utf16());
        out.extend_from_slice(&w[2..]);
    } else {
        out.extend_from_slice(&w);
    }
    for c in &mut out[4..] {
        if *c == b'/' as u16 {
            *c = bs;
        }
    }
    while out.len() > 4 && out.last() == Some(&bs) {
        out.pop();
    }
    // A single component after the prefix (`C:`, `Volume{...}`) names a
    // volume: its root directory needs the trailing backslash.
    if !out[4..].contains(&bs) {
        out.push(bs);
    }
    Ok(out)
}

/// Appends `\name` to the NT path of a directory.
pub(super) fn child_path(parent: &[u16], name: &[u16]) -> Box<[u16]> {
    let sep = parent.last() != Some(&(b'\\' as u16));
    let mut p = Vec::with_capacity(parent.len() + sep as usize + name.len());
    p.extend_from_slice(parent);
    if sep {
        p.push(b'\\' as u16);
    }
    p.extend_from_slice(name);
    p.into_boxed_slice()
}

/// What the walk needs to know about the volume being scanned.
#[derive(Debug, Clone)]
pub(super) struct VolumeCaps {
    pub file_system: String,
    pub supports_hard_links: bool,
    pub remote: bool,
    /// Bytes per cluster (allocation unit), used to estimate size on disk
    /// when only logical sizes are known.
    pub cluster: u64,
}

/// Queries the volume holding the directory at the NT path `path`.
pub(super) fn volume_caps(path: &[u16]) -> Option<VolumeCaps> {
    // The plain spelling is the classic one; the verbatim one is needed
    // beyond MAX_PATH.
    query_volume(plain_path(path)).or_else(|| query_volume(win32_path(path)))
}

/// The plain Win32 spelling of an NT path (`C:\dir`, `\\server\share\dir`).
fn plain_path(nt: &[u16]) -> Vec<u16> {
    let unc: Vec<u16> = r"\??\UNC\".encode_utf16().collect();
    if nt.starts_with(&unc) {
        let mut p: Vec<u16> = r"\\".encode_utf16().collect();
        p.extend_from_slice(&nt[unc.len()..]);
        p
    } else {
        nt.get(4..).unwrap_or_default().to_vec()
    }
}

fn query_volume(mut w: Vec<u16>) -> Option<VolumeCaps> {
    w.push(0);
    let mut vol = vec![0u16; w.len() + 64];
    let mut fs = [0u16; 64];
    let mut flags = 0u32;
    let (mut per_cluster, mut per_sector) = (0u32, 0u32);
    // SAFETY: NUL-terminated inputs and correctly sized output buffers.
    unsafe {
        GetVolumePathNameW(PCWSTR(w.as_ptr()), &mut vol).ok()?;
        let root = PCWSTR(vol.as_ptr());
        GetVolumeInformationW(root, None, None, None, Some(&mut flags), Some(&mut fs)).ok()?;
        let remote = GetDriveTypeW(root) == DRIVE_REMOTE;
        let cluster = match GetDiskFreeSpaceW(
            root,
            Some(&mut per_cluster),
            Some(&mut per_sector),
            None,
            None,
        ) {
            Ok(()) if per_cluster > 0 && per_sector > 0 => per_cluster as u64 * per_sector as u64,
            _ => 4096,
        };
        let fs_len = fs.iter().position(|&c| c == 0).unwrap_or(fs.len());
        Some(VolumeCaps {
            file_system: String::from_utf16_lossy(&fs[..fs_len]),
            supports_hard_links: flags & FILE_SUPPORTS_HARD_LINKS != 0,
            remote,
            cluster,
        })
    }
}

/// An `io::Error` for a Win32 error code.
pub(super) fn io_error(code: u32) -> std::io::Error {
    std::io::Error::from_raw_os_error(code as i32)
}
