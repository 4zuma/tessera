//! The NTFS USN change journal (admin only).
//!
//! After an MFT scan the watcher reads the journal from the scan's
//! checkpoint, so no change made during or after the scan is missed.
//! `FSCTL_READ_USN_JOURNAL` with `BytesToWaitFor = 1` blocks until a record
//! is written; records are mapped to nodes through the scan's
//! [`FrnIndex`](crate::scan::FrnIndex).
//!
//! The record parser is portable and unit tested with synthetic buffers.

#![cfg_attr(not(windows), allow(dead_code))]

use super::coalesce::{Event, UsnRecord};
use crate::wtf8;

/// `USN_REASON_*` flags (values from the Windows SDK).
pub(crate) mod reason {
    pub const DATA_OVERWRITE: u32 = 0x0000_0001;
    pub const DATA_EXTEND: u32 = 0x0000_0002;
    pub const DATA_TRUNCATION: u32 = 0x0000_0004;
    pub const FILE_CREATE: u32 = 0x0000_0100;
    pub const FILE_DELETE: u32 = 0x0000_0200;
    pub const RENAME_OLD_NAME: u32 = 0x0000_1000;
    pub const RENAME_NEW_NAME: u32 = 0x0000_2000;
    pub const BASIC_INFO_CHANGE: u32 = 0x0000_8000;
    pub const HARD_LINK_CHANGE: u32 = 0x0001_0000;
    pub const COMPRESSION_CHANGE: u32 = 0x0002_0000;
    pub const ENCRYPTION_CHANGE: u32 = 0x0004_0000;
    pub const REPARSE_POINT_CHANGE: u32 = 0x0010_0000;
    pub const CLOSE: u32 = 0x8000_0000;

    /// Reasons that can change what the tree shows. A close record repeats
    /// the reasons accumulated while the file was open, so with `CLOSE` in
    /// the read mask the final size of a written file is always seen.
    pub const RELEVANT: u32 = DATA_OVERWRITE
        | DATA_EXTEND
        | DATA_TRUNCATION
        | FILE_CREATE
        | FILE_DELETE
        | RENAME_OLD_NAME
        | RENAME_NEW_NAME
        | BASIC_INFO_CHANGE
        | HARD_LINK_CHANGE
        | COMPRESSION_CHANGE
        | ENCRYPTION_CHANGE
        | REPARSE_POINT_CHANGE;
}

/// `USN_RECORD_V2` field offsets.
const HEADER: usize = 60;
const FRN: usize = 8;
const PARENT_FRN: usize = 16;
const REASON: usize = 40;
const ATTRIBUTES: usize = 52;
const NAME_LEN: usize = 56;
const NAME_OFF: usize = 58;

fn u16_at(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

fn u32_at(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(b[off..off + 4].try_into().unwrap())
}

fn u64_at(b: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(b[off..off + 8].try_into().unwrap())
}

/// Parses the records of a `FSCTL_READ_USN_JOURNAL` output buffer (after
/// its leading next-USN) and appends those with a relevant reason. Only
/// version 2 records (64-bit reference numbers, NTFS) are read.
pub(crate) fn parse_records(buf: &[u8], out: &mut Vec<Event>) {
    let mut off = 0usize;
    while off + HEADER <= buf.len() {
        let rec = &buf[off..];
        let len = u32_at(rec, 0) as usize;
        if len < HEADER || len > rec.len() {
            break;
        }
        let rec = &rec[..len];
        let major = u16_at(rec, 4);
        let reason = u32_at(rec, REASON);
        if major == 2 && reason & reason::RELEVANT != 0 {
            let name_len = u16_at(rec, NAME_LEN) as usize;
            let name_off = u16_at(rec, NAME_OFF) as usize;
            if let Some(raw) = rec.get(name_off..name_off + name_len) {
                let units: Vec<u16> = raw
                    .chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect();
                out.push(Event::Usn(UsnRecord {
                    frn: u64_at(rec, FRN),
                    parent_frn: u64_at(rec, PARENT_FRN),
                    reason,
                    attributes: u32_at(rec, ATTRIBUTES),
                    name: wtf8::from_utf16(&units),
                }));
            }
        }
        // Records start on 8-byte boundaries.
        off += (len + 7) & !7;
    }
}

#[cfg(windows)]
pub(crate) use imp::UsnReader;
#[cfg(windows)]
pub use imp::query_usn_checkpoint;

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::path::Path;

    use windows::Win32::Foundation::{
        ERROR_INVALID_PARAMETER, ERROR_IO_PENDING, ERROR_JOURNAL_DELETE_IN_PROGRESS,
        ERROR_JOURNAL_ENTRY_DELETED, ERROR_JOURNAL_NOT_ACTIVE, ERROR_OPERATION_ABORTED,
        GENERIC_READ, HANDLE,
    };
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_OVERLAPPED, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ,
        FILE_SHARE_WRITE, GetVolumeNameForVolumeMountPointW, GetVolumePathNameW, OPEN_EXISTING,
    };
    use windows::Win32::System::IO::{DeviceIoControl, OVERLAPPED};
    use windows::Win32::System::Ioctl::{
        FSCTL_QUERY_USN_JOURNAL, FSCTL_READ_USN_JOURNAL, READ_USN_JOURNAL_DATA_V0,
        USN_JOURNAL_DATA_V0,
    };
    use windows::core::PCWSTR;

    use super::super::coalesce::Event;
    use super::super::sys::{Next, OwnedHandle, Reader, ioctl_wait, wide_nul, win32_code};
    use super::{parse_records, reason};
    use crate::Result;
    use crate::scan::UsnCheckpoint;

    const BUFFER_BYTES: usize = 64 * 1024;

    /// Opens the volume that holds `path` (e.g. `\\?\Volume{…}` or `\\.\C:`).
    fn open_volume(path: &Path, overlapped: bool) -> Result<OwnedHandle> {
        let wide = wide_nul(path.as_os_str());
        let mut mount = [0u16; 1024];
        unsafe { GetVolumePathNameW(PCWSTR(wide.as_ptr()), &mut mount)? };
        let mut candidates: Vec<Vec<u16>> = Vec::new();
        let mut guid = [0u16; 64];
        if unsafe { GetVolumeNameForVolumeMountPointW(PCWSTR(mount.as_ptr()), &mut guid) }.is_ok() {
            let mut v: Vec<u16> = guid.iter().copied().take_while(|&c| c != 0).collect();
            if v.last() == Some(&(b'\\' as u16)) {
                v.pop();
            }
            v.push(0);
            candidates.push(v);
        }
        // `C:\` -> `\\.\C:`
        let m: Vec<u16> = mount.iter().copied().take_while(|&c| c != 0).collect();
        if m.len() >= 2 && m[1] == b':' as u16 {
            let mut v: Vec<u16> = r"\\.\".encode_utf16().collect();
            v.extend_from_slice(&m[..2]);
            v.push(0);
            candidates.push(v);
        }
        let flags = if overlapped {
            FILE_FLAG_OVERLAPPED
        } else {
            FILE_FLAGS_AND_ATTRIBUTES(0)
        };
        let mut last = None;
        for name in candidates {
            match unsafe {
                CreateFileW(
                    PCWSTR(name.as_ptr()),
                    GENERIC_READ.0,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    None,
                    OPEN_EXISTING,
                    flags,
                    None,
                )
            } {
                Ok(h) => return Ok(OwnedHandle(h)),
                Err(e) => last = Some(e),
            }
        }
        Err(match last {
            Some(e) => e.into(),
            None => crate::Error::Unsupported("the path is not on a local volume"),
        })
    }

    fn query(volume: HANDLE, overlapped: bool) -> std::result::Result<USN_JOURNAL_DATA_V0, u32> {
        let mut data = USN_JOURNAL_DATA_V0::default();
        let out = (&mut data as *mut USN_JOURNAL_DATA_V0).cast::<c_void>();
        let size = std::mem::size_of::<USN_JOURNAL_DATA_V0>() as u32;
        if overlapped {
            ioctl_wait(volume, FSCTL_QUERY_USN_JOURNAL, None, out, size)?;
        } else {
            let mut n = 0u32;
            unsafe {
                DeviceIoControl(
                    volume,
                    FSCTL_QUERY_USN_JOURNAL,
                    None,
                    0,
                    Some(out),
                    size,
                    Some(&mut n),
                    None,
                )
            }
            .map_err(|e| win32_code(&e))?;
        }
        Ok(data)
    }

    /// Where the USN journal of the volume holding `path` stands now. Pass
    /// it to the watcher together with an index of the tree's reference
    /// numbers to receive every change made from this moment on. Needs
    /// admin rights; fails if the volume has no active journal.
    pub fn query_usn_checkpoint(path: &Path) -> Result<UsnCheckpoint> {
        let volume = open_volume(path, false)?;
        let data =
            query(volume.0, false).map_err(|e| std::io::Error::from_raw_os_error(e as i32))?;
        Ok(UsnCheckpoint {
            journal_id: data.UsnJournalID,
            next_usn: data.NextUsn,
        })
    }

    pub(crate) struct UsnReader {
        volume: OwnedHandle,
        journal_id: u64,
        next_usn: i64,
        /// Must stay put while a read is pending.
        input: Box<READ_USN_JOURNAL_DATA_V0>,
        buf: Vec<u64>,
    }

    impl UsnReader {
        /// Opens the journal of the volume holding `root`, positioned at
        /// `from`. If the journal was recreated since, the returned events
        /// report the gap.
        pub fn open(root: &Path, from: UsnCheckpoint) -> Result<(UsnReader, Vec<Event>)> {
            let volume = open_volume(root, true)?;
            let data =
                query(volume.0, true).map_err(|e| std::io::Error::from_raw_os_error(e as i32))?;
            let mut events = Vec::new();
            let mut next_usn = from.next_usn;
            if data.UsnJournalID != from.journal_id {
                events.push(Event::Overflow(Vec::new()));
                next_usn = data.NextUsn;
            }
            Ok((
                UsnReader {
                    volume,
                    journal_id: data.UsnJournalID,
                    next_usn,
                    input: Box::default(),
                    buf: vec![0u64; BUFFER_BYTES / 8],
                },
                events,
            ))
        }

        /// Re-reads the journal's identity after an error. Returns true if
        /// it changed (the reader then continues at its current end).
        fn requery(&mut self) -> std::result::Result<bool, u32> {
            let data = query(self.volume.0, true)?;
            let changed = data.UsnJournalID != self.journal_id || self.next_usn < data.FirstUsn;
            if changed {
                self.journal_id = data.UsnJournalID;
                self.next_usn = data.NextUsn;
            }
            Ok(changed)
        }
    }

    impl Reader for UsnReader {
        fn handle(&self) -> HANDLE {
            self.volume.0
        }

        unsafe fn issue(&mut self, ov: *mut OVERLAPPED) -> std::result::Result<(), u32> {
            *self.input = READ_USN_JOURNAL_DATA_V0 {
                StartUsn: self.next_usn,
                ReasonMask: reason::RELEVANT | reason::CLOSE,
                ReturnOnlyOnClose: 0,
                Timeout: 0,
                // Block until at least one record is written.
                BytesToWaitFor: 1,
                UsnJournalID: self.journal_id,
            };
            let r = unsafe {
                DeviceIoControl(
                    self.volume.0,
                    FSCTL_READ_USN_JOURNAL,
                    Some((&*self.input as *const READ_USN_JOURNAL_DATA_V0).cast::<c_void>()),
                    std::mem::size_of::<READ_USN_JOURNAL_DATA_V0>() as u32,
                    Some(self.buf.as_mut_ptr().cast::<c_void>()),
                    (self.buf.len() * 8) as u32,
                    None,
                    Some(ov),
                )
            };
            match r {
                Ok(()) => Ok(()),
                Err(e) if win32_code(&e) == ERROR_IO_PENDING.0 => Ok(()),
                Err(e) => Err(win32_code(&e)),
            }
        }

        fn finish(&mut self, res: std::result::Result<u32, u32>, out: &mut Vec<Event>) -> Next {
            let failed = |out: &mut Vec<Event>, what: String| {
                out.push(Event::Overflow(Vec::new()));
                out.push(Event::Failed(what));
                Next::Stop
            };
            match res {
                Ok(n) => {
                    let n = n as usize;
                    let bytes: &[u8] =
                        unsafe { std::slice::from_raw_parts(self.buf.as_ptr().cast::<u8>(), n) };
                    if n >= 8 {
                        self.next_usn = i64::from_le_bytes(bytes[..8].try_into().unwrap());
                        parse_records(&bytes[8..], out);
                    }
                    Next::Continue
                }
                Err(e) if e == ERROR_OPERATION_ABORTED.0 => Next::Stop,
                // Records we had not read yet were purged: a gap.
                Err(e) if e == ERROR_JOURNAL_ENTRY_DELETED.0 => match self.requery() {
                    Ok(_) => {
                        out.push(Event::Overflow(Vec::new()));
                        Next::Continue
                    }
                    Err(q) => failed(out, os_error("querying the USN journal", q)),
                },
                Err(e)
                    if e == ERROR_JOURNAL_DELETE_IN_PROGRESS.0
                        || e == ERROR_JOURNAL_NOT_ACTIVE.0 =>
                {
                    failed(out, "the USN journal was deleted or disabled".into())
                }
                // A journal id that is no longer current.
                Err(e) if e == ERROR_INVALID_PARAMETER.0 => match self.requery() {
                    Ok(true) => {
                        out.push(Event::Overflow(Vec::new()));
                        Next::Continue
                    }
                    _ => failed(out, os_error("reading the USN journal", e)),
                },
                Err(e) => failed(out, os_error("reading the USN journal", e)),
            }
        }
    }

    fn os_error(what: &str, code: u32) -> String {
        format!(
            "{what} failed: {}",
            std::io::Error::from_raw_os_error(code as i32)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::reason::*;
    use super::*;

    /// Builds one `USN_RECORD_V2`.
    fn record(frn: u64, parent: u64, reason: u32, attributes: u32, name: &str) -> Vec<u8> {
        let units: Vec<u16> = name.encode_utf16().collect();
        let len = (HEADER + units.len() * 2 + 7) & !7;
        let mut r = vec![0u8; len];
        r[0..4].copy_from_slice(&(len as u32).to_le_bytes());
        r[4..6].copy_from_slice(&2u16.to_le_bytes());
        r[FRN..FRN + 8].copy_from_slice(&frn.to_le_bytes());
        r[PARENT_FRN..PARENT_FRN + 8].copy_from_slice(&parent.to_le_bytes());
        r[REASON..REASON + 4].copy_from_slice(&reason.to_le_bytes());
        r[ATTRIBUTES..ATTRIBUTES + 4].copy_from_slice(&attributes.to_le_bytes());
        r[NAME_LEN..NAME_LEN + 2].copy_from_slice(&((units.len() * 2) as u16).to_le_bytes());
        r[NAME_OFF..NAME_OFF + 2].copy_from_slice(&(HEADER as u16).to_le_bytes());
        for (i, u) in units.iter().enumerate() {
            r[HEADER + 2 * i..HEADER + 2 * i + 2].copy_from_slice(&u.to_le_bytes());
        }
        r
    }

    #[test]
    fn parses_v2_records_and_skips_noise() {
        let mut buf = Vec::new();
        buf.extend(record(10, 5, FILE_CREATE, 0x20, "a.txt"));
        buf.extend(record(11, 5, 0x800 | CLOSE, 0x20, "acl-only")); // security change
        let mut v3 = record(12, 5, FILE_CREATE, 0x20, "v3");
        v3[4] = 3;
        buf.extend(v3);
        buf.extend(record(13, 10, DATA_EXTEND | CLOSE, 0x20, "長い名前.bin"));
        let mut out = Vec::new();
        parse_records(&buf, &mut out);
        assert_eq!(
            out,
            vec![
                Event::Usn(UsnRecord {
                    frn: 10,
                    parent_frn: 5,
                    reason: FILE_CREATE,
                    attributes: 0x20,
                    name: b"a.txt".to_vec()
                }),
                Event::Usn(UsnRecord {
                    frn: 13,
                    parent_frn: 10,
                    reason: DATA_EXTEND | CLOSE,
                    attributes: 0x20,
                    name: "長い名前.bin".as_bytes().to_vec()
                }),
            ]
        );
        // Truncated input never panics.
        for cut in 0..buf.len() {
            parse_records(&buf[..cut], &mut Vec::new());
        }
    }

    #[cfg(windows)]
    #[test]
    fn offsets_match_the_sdk() {
        use std::mem::offset_of;
        use windows::Win32::System::Ioctl::USN_RECORD_V2 as R;
        assert_eq!(offset_of!(R, FileReferenceNumber), FRN);
        assert_eq!(offset_of!(R, ParentFileReferenceNumber), PARENT_FRN);
        assert_eq!(offset_of!(R, Reason), REASON);
        assert_eq!(offset_of!(R, FileAttributes), ATTRIBUTES);
        assert_eq!(offset_of!(R, FileNameLength), NAME_LEN);
        assert_eq!(offset_of!(R, FileNameOffset), NAME_OFF);
        assert_eq!(offset_of!(R, FileName), HEADER);
    }
}
