//! `ReadDirectoryChangesExW` / `ReadDirectoryChangesW` on the scan root.
//!
//! One recursive watch on the root directory handle. The extended variant
//! (Windows 10 1709+, NTFS) reports attributes and sizes with each change,
//! so most changes are applied without touching the disk again; elsewhere
//! the plain variant is used and changed entries are probed.
//!
//! The record parser is portable and unit tested with synthetic buffers.

#![cfg_attr(not(windows), allow(dead_code))]

use super::apply::{Stat, attr};
use super::coalesce::{Event, Info};
use crate::wtf8;

pub(crate) const FILE_ACTION_ADDED: u32 = 1;
pub(crate) const FILE_ACTION_REMOVED: u32 = 2;
pub(crate) const FILE_ACTION_MODIFIED: u32 = 3;
pub(crate) const FILE_ACTION_RENAMED_OLD_NAME: u32 = 4;
pub(crate) const FILE_ACTION_RENAMED_NEW_NAME: u32 = 5;

/// `FILE_NOTIFY_INFORMATION`: offset of `FileNameLength` and `FileName`.
const PLAIN_NAME_LEN: usize = 8;
const PLAIN_NAME: usize = 12;
/// `FILE_NOTIFY_EXTENDED_INFORMATION` field offsets.
const EXT_ALLOCATED: usize = 40;
const EXT_SIZE: usize = 48;
const EXT_ATTRIBUTES: usize = 56;
const EXT_REPARSE_TAG: usize = 60;
const EXT_NAME_LEN: usize = 80;
const EXT_NAME: usize = 84;

fn u32_at(b: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(off..off + 4)?.try_into().ok()?))
}

fn u64_at(b: &[u8], off: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(off..off + 8)?.try_into().ok()?))
}

/// One parsed notification record.
struct Record {
    action: u32,
    name: Vec<u8>,
    stat: Option<Stat>,
}

fn record(rec: &[u8], extended: bool) -> Option<Record> {
    let action = u32_at(rec, 4)?;
    let (len_off, name_off) = if extended {
        (EXT_NAME_LEN, EXT_NAME)
    } else {
        (PLAIN_NAME_LEN, PLAIN_NAME)
    };
    let name_len = u32_at(rec, len_off)? as usize;
    let raw = rec.get(name_off..name_off.checked_add(name_len)?)?;
    let units: Vec<u16> = raw
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    let stat = if extended {
        let attributes = u32_at(rec, EXT_ATTRIBUTES)?;
        Some(Stat {
            attributes,
            reparse_tag: if attributes & attr::REPARSE_POINT != 0 {
                u32_at(rec, EXT_REPARSE_TAG)?
            } else {
                0
            },
            size: u64_at(rec, EXT_ALLOCATED)?,
            logical: u64_at(rec, EXT_SIZE)?,
        })
    } else {
        None
    };
    Some(Record {
        action,
        name: wtf8::from_utf16(&units),
        stat,
    })
}

/// Parses one buffer returned by `ReadDirectoryChanges(Ex)W`.
///
/// Renames come as an old-name record followed by a new-name record; they
/// become one [`Event::Renamed`]. A buffer may end between the two: the old
/// name is then reported gone at once (so a move out of the watched tree,
/// which has no new-name record, is not held back) and remembered in
/// `carry`, and a new-name record opening the next buffer still pairs with
/// it, which the coalescer turns back into a rename.
pub(crate) fn parse(buf: &[u8], extended: bool, carry: &mut Option<Vec<u8>>, out: &mut Vec<Event>) {
    let carried = carry.take();
    let mut old: Option<Vec<u8>> = None;
    let mut first = true;
    let mut off = 0usize;
    while off < buf.len() {
        let rec = &buf[off..];
        let Some(r) = record(rec, extended) else {
            break;
        };
        let info_present = match r.stat {
            Some(s) => Info::Present(s),
            None => Info::Unknown,
        };
        match r.action {
            FILE_ACTION_RENAMED_OLD_NAME => {
                if let Some(o) = old.take() {
                    out.push(Event::Changed {
                        path: o,
                        info: Info::Gone,
                    });
                }
                old = Some(r.name);
            }
            FILE_ACTION_RENAMED_NEW_NAME => {
                let from = match old.take() {
                    Some(o) => Some(o),
                    None if first => carried.clone(),
                    None => None,
                };
                out.push(match from {
                    Some(from) => Event::Renamed {
                        from,
                        to: r.name,
                        info: info_present,
                    },
                    None => Event::Changed {
                        path: r.name,
                        info: info_present,
                    },
                });
            }
            action => {
                if let Some(o) = old.take() {
                    out.push(Event::Changed {
                        path: o,
                        info: Info::Gone,
                    });
                }
                let info = match action {
                    FILE_ACTION_REMOVED => Info::Gone,
                    FILE_ACTION_ADDED | FILE_ACTION_MODIFIED => info_present,
                    // An action this code does not know: whatever happened,
                    // the entry's current state is what counts.
                    _ => info_present,
                };
                out.push(Event::Changed { path: r.name, info });
            }
        }
        first = false;
        let next = u32_at(rec, 0).unwrap_or(0) as usize;
        if next == 0 {
            break;
        }
        off += next;
    }
    if let Some(o) = old {
        out.push(Event::Changed {
            path: o.clone(),
            info: Info::Gone,
        });
        *carry = Some(o);
    }
}

#[cfg(windows)]
pub(crate) use imp::DirReader;

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::path::Path;

    use windows::Win32::Foundation::{
        ERROR_INVALID_FUNCTION, ERROR_INVALID_PARAMETER, ERROR_NOT_SUPPORTED,
        ERROR_NOTIFY_ENUM_DIR, ERROR_OPERATION_ABORTED, GetLastError, HANDLE,
    };
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED, FILE_LIST_DIRECTORY,
        FILE_NOTIFY_CHANGE, FILE_NOTIFY_CHANGE_ATTRIBUTES, FILE_NOTIFY_CHANGE_DIR_NAME,
        FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_SIZE, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, OPEN_EXISTING, ReadDirectoryChangesW,
        ReadDirectoryNotifyExtendedInformation,
    };
    use windows::Win32::System::IO::OVERLAPPED;
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    use windows::core::{PCWSTR, s, w};

    use super::super::coalesce::Event;
    use super::super::sys::{Next, OwnedHandle, Reader, wide_nul};
    use crate::Result;

    /// What to be told about: names (files and directories), sizes, and
    /// attributes (hidden, compressed, cloud placeholder state).
    const FILTER: FILE_NOTIFY_CHANGE = FILE_NOTIFY_CHANGE(
        FILE_NOTIFY_CHANGE_FILE_NAME.0
            | FILE_NOTIFY_CHANGE_DIR_NAME.0
            | FILE_NOTIFY_CHANGE_SIZE.0
            | FILE_NOTIFY_CHANGE_ATTRIBUTES.0,
    );

    /// 64 KiB: the largest buffer that also works on network shares. The
    /// system keeps a buffer of the same size between reads.
    const BUFFER_BYTES: usize = 64 * 1024;

    type ReadDirectoryChangesExW = unsafe extern "system" fn(
        HANDLE,
        *mut c_void,
        u32,
        i32,
        u32,
        *mut u32,
        *mut OVERLAPPED,
        *const c_void,
        i32,
    ) -> i32;

    /// `ReadDirectoryChangesExW` if this system has it (looked up at run
    /// time so the program still loads where it does not).
    fn extended_api() -> Option<ReadDirectoryChangesExW> {
        unsafe {
            let k32 = GetModuleHandleW(w!("kernel32.dll")).ok()?;
            let f = GetProcAddress(k32, s!("ReadDirectoryChangesExW"))?;
            Some(std::mem::transmute::<
                unsafe extern "system" fn() -> isize,
                ReadDirectoryChangesExW,
            >(f))
        }
    }

    pub(crate) struct DirReader {
        dir: OwnedHandle,
        ex: Option<ReadDirectoryChangesExW>,
        buf: Vec<u64>,
        carry: Option<Vec<u8>>,
    }

    impl DirReader {
        pub fn open(root: &Path) -> Result<DirReader> {
            let path = wide_nul(root.as_os_str());
            let dir = unsafe {
                CreateFileW(
                    PCWSTR(path.as_ptr()),
                    FILE_LIST_DIRECTORY.0,
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                    None,
                    OPEN_EXISTING,
                    FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
                    None,
                )?
            };
            Ok(DirReader {
                dir: OwnedHandle(dir),
                ex: extended_api(),
                buf: vec![0u64; BUFFER_BYTES / 8],
                carry: None,
            })
        }
    }

    impl Reader for DirReader {
        fn handle(&self) -> HANDLE {
            self.dir.0
        }

        unsafe fn issue(&mut self, ov: *mut OVERLAPPED) -> std::result::Result<(), u32> {
            let buf = self.buf.as_mut_ptr().cast::<c_void>();
            let len = (self.buf.len() * 8) as u32;
            let ok = unsafe {
                match self.ex {
                    Some(f) => {
                        f(
                            self.dir.0,
                            buf,
                            len,
                            1,
                            FILTER.0,
                            std::ptr::null_mut(),
                            ov,
                            std::ptr::null(),
                            ReadDirectoryNotifyExtendedInformation.0,
                        ) != 0
                    }
                    None => ReadDirectoryChangesW(
                        self.dir.0,
                        buf,
                        len,
                        true,
                        FILTER,
                        None,
                        Some(ov),
                        None,
                    )
                    .is_ok(),
                }
            };
            if ok {
                Ok(())
            } else {
                Err(unsafe { GetLastError() }.0)
            }
        }

        fn finish(&mut self, res: std::result::Result<u32, u32>, out: &mut Vec<Event>) -> Next {
            match res {
                // Zero bytes: the system could not keep all changes.
                Ok(0) => {
                    self.carry = None;
                    out.push(Event::Overflow(Vec::new()));
                    Next::Continue
                }
                Ok(n) => {
                    let bytes: &[u8] = unsafe {
                        std::slice::from_raw_parts(self.buf.as_ptr().cast::<u8>(), n as usize)
                    };
                    super::parse(bytes, self.ex.is_some(), &mut self.carry, out);
                    Next::Continue
                }
                Err(e) if e == ERROR_NOTIFY_ENUM_DIR.0 => {
                    self.carry = None;
                    out.push(Event::Overflow(Vec::new()));
                    Next::Continue
                }
                // The extended variant only works on NTFS: fall back.
                Err(e)
                    if self.ex.is_some()
                        && (e == ERROR_INVALID_FUNCTION.0
                            || e == ERROR_INVALID_PARAMETER.0
                            || e == ERROR_NOT_SUPPORTED.0) =>
                {
                    self.ex = None;
                    Next::Continue
                }
                Err(e) if e == ERROR_OPERATION_ABORTED.0 => Next::Stop,
                Err(e) => {
                    out.push(Event::Failed(format!(
                        "watching the directory failed: {}",
                        std::io::Error::from_raw_os_error(e as i32)
                    )));
                    Next::Stop
                }
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Builds a `FILE_NOTIFY_INFORMATION` / extended buffer.
    pub(crate) fn buffer(records: &[(u32, &str, Option<Stat>)], extended: bool) -> Vec<u8> {
        let mut out = Vec::new();
        let mut starts = Vec::new();
        for (action, name, stat) in records {
            let start = out.len();
            starts.push(start);
            let units: Vec<u16> = name.encode_utf16().collect();
            let name_off = if extended { EXT_NAME } else { PLAIN_NAME };
            let mut rec = vec![0u8; name_off];
            rec[4..8].copy_from_slice(&action.to_le_bytes());
            let len_off = if extended {
                EXT_NAME_LEN
            } else {
                PLAIN_NAME_LEN
            };
            rec[len_off..len_off + 4].copy_from_slice(&((units.len() * 2) as u32).to_le_bytes());
            if extended {
                let s = stat.unwrap_or_default();
                rec[EXT_ALLOCATED..EXT_ALLOCATED + 8].copy_from_slice(&s.size.to_le_bytes());
                rec[EXT_SIZE..EXT_SIZE + 8].copy_from_slice(&s.logical.to_le_bytes());
                rec[EXT_ATTRIBUTES..EXT_ATTRIBUTES + 4]
                    .copy_from_slice(&s.attributes.to_le_bytes());
                rec[EXT_REPARSE_TAG..EXT_REPARSE_TAG + 4]
                    .copy_from_slice(&s.reparse_tag.to_le_bytes());
            }
            for u in units {
                rec.extend_from_slice(&u.to_le_bytes());
            }
            while rec.len() % 8 != 0 {
                rec.push(0);
            }
            out.extend_from_slice(&rec);
        }
        for (i, &s) in starts.iter().enumerate() {
            let next = match starts.get(i + 1) {
                Some(&n) => (n - s) as u32,
                None => 0,
            };
            out[s..s + 4].copy_from_slice(&next.to_le_bytes());
        }
        out
    }

    fn file(size: u64) -> Stat {
        Stat {
            attributes: 0x20,
            reparse_tag: 0,
            size: 4096,
            logical: size,
        }
    }

    fn changed(p: &str, info: Info) -> Event {
        Event::Changed {
            path: p.as_bytes().to_vec(),
            info,
        }
    }

    #[test]
    fn parses_plain_records() {
        let buf = buffer(
            &[
                (FILE_ACTION_ADDED, "a.txt", None),
                (FILE_ACTION_MODIFIED, "dir\\b.txt", None),
                (FILE_ACTION_REMOVED, "c.txt", None),
                (FILE_ACTION_RENAMED_OLD_NAME, "dir\\old", None),
                (FILE_ACTION_RENAMED_NEW_NAME, "new", None),
                (FILE_ACTION_ADDED, "照片.jpg", None),
            ],
            false,
        );
        let mut out = Vec::new();
        let mut carry = None;
        parse(&buf, false, &mut carry, &mut out);
        assert_eq!(
            out,
            vec![
                changed("a.txt", Info::Unknown),
                changed("dir\\b.txt", Info::Unknown),
                changed("c.txt", Info::Gone),
                Event::Renamed {
                    from: b"dir\\old".to_vec(),
                    to: b"new".to_vec(),
                    info: Info::Unknown
                },
                changed("照片.jpg", Info::Unknown),
            ]
        );
        assert!(carry.is_none());
    }

    #[test]
    fn parses_extended_records() {
        let link = Stat {
            attributes: attr::DIRECTORY | attr::REPARSE_POINT,
            reparse_tag: 0xA000_0003,
            size: 0,
            logical: 0,
        };
        let buf = buffer(
            &[
                (FILE_ACTION_MODIFIED, "big.iso", Some(file(5_000_000_000))),
                (FILE_ACTION_ADDED, "junction", Some(link)),
                (FILE_ACTION_REMOVED, "gone.txt", Some(file(1))),
            ],
            true,
        );
        let mut out = Vec::new();
        parse(&buf, true, &mut None, &mut out);
        assert_eq!(
            out,
            vec![
                changed("big.iso", Info::Present(file(5_000_000_000))),
                changed("junction", Info::Present(link)),
                changed("gone.txt", Info::Gone),
            ]
        );
    }

    #[test]
    fn rename_split_across_buffers() {
        let mut carry = None;
        let mut out = Vec::new();
        let a = buffer(
            &[
                (FILE_ACTION_ADDED, "x", None),
                (FILE_ACTION_RENAMED_OLD_NAME, "from", None),
            ],
            false,
        );
        parse(&a, false, &mut carry, &mut out);
        assert_eq!(
            out,
            vec![changed("x", Info::Unknown), changed("from", Info::Gone)]
        );
        out.clear();
        let b = buffer(&[(FILE_ACTION_RENAMED_NEW_NAME, "to", None)], false);
        parse(&b, false, &mut carry, &mut out);
        assert_eq!(
            out,
            vec![Event::Renamed {
                from: b"from".to_vec(),
                to: b"to".to_vec(),
                info: Info::Unknown
            }]
        );
        // An old name with no partner: just gone.
        out.clear();
        let c = buffer(&[(FILE_ACTION_RENAMED_OLD_NAME, "lonely", None)], false);
        parse(&c, false, &mut carry, &mut out);
        let d = buffer(&[(FILE_ACTION_ADDED, "other", None)], false);
        parse(&d, false, &mut carry, &mut out);
        assert_eq!(
            out,
            vec![
                changed("lonely", Info::Gone),
                changed("other", Info::Unknown)
            ]
        );
    }

    #[test]
    fn truncated_buffers_do_not_panic() {
        let buf = buffer(&[(FILE_ACTION_ADDED, "abcdef", None)], false);
        for cut in 0..buf.len() {
            let mut out = Vec::new();
            parse(&buf[..cut], false, &mut None, &mut out);
            assert!(out.len() <= 1);
        }
        let mut bad = buf.clone();
        bad[PLAIN_NAME_LEN..PLAIN_NAME_LEN + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        let mut out = Vec::new();
        parse(&bad, false, &mut None, &mut out);
        assert!(out.is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn offsets_match_the_sdk() {
        use std::mem::offset_of;
        use windows::Win32::Storage::FileSystem::{
            FILE_NOTIFY_EXTENDED_INFORMATION as X, FILE_NOTIFY_INFORMATION as P,
        };
        assert_eq!(offset_of!(P, FileNameLength), PLAIN_NAME_LEN);
        assert_eq!(offset_of!(P, FileName), PLAIN_NAME);
        assert_eq!(offset_of!(X, AllocatedLength), EXT_ALLOCATED);
        assert_eq!(offset_of!(X, FileSize), EXT_SIZE);
        assert_eq!(offset_of!(X, FileAttributes), EXT_ATTRIBUTES);
        assert_eq!(offset_of!(X, Anonymous), EXT_REPARSE_TAG);
        assert_eq!(offset_of!(X, FileNameLength), EXT_NAME_LEN);
        assert_eq!(offset_of!(X, FileName), EXT_NAME);
    }
}
