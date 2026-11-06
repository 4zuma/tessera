//! Windows plumbing for the watcher: owned handles, the blocking read loop
//! shared by both collectors, and the attribute probe (which also reads
//! hard links: a file's link count by path, its names by file id).

use std::ffi::{OsStr, c_void};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

use windows::Wdk::Foundation::OBJECT_ATTRIBUTES;
use windows::Wdk::Storage::FileSystem::{
    FILE_INFORMATION_CLASS, FILE_NETWORK_OPEN_INFORMATION, FILE_STAT_INFORMATION,
    FileStatInformation, NtQueryFullAttributesFile,
};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_HANDLE_EOF, ERROR_IO_PENDING, ERROR_MORE_DATA, HANDLE, NTSTATUS,
    OBJ_CASE_INSENSITIVE, STATUS_DELETE_PENDING, STATUS_INVALID_INFO_CLASS,
    STATUS_INVALID_PARAMETER, STATUS_NOT_IMPLEMENTED, STATUS_OBJECT_NAME_NOT_FOUND,
    STATUS_OBJECT_PATH_NOT_FOUND, UNICODE_STRING, WAIT_OBJECT_0,
};
use windows::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_DESCRIPTOR, FILE_ID_DESCRIPTOR_0, FILE_NAME_NORMALIZED,
    FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_STANDARD_INFO,
    FIND_FIRST_EX_FLAGS, FileIdType, FileStandardInfo, FindClose, FindExInfoBasic,
    FindExSearchNameMatch, FindFirstFileExW, FindFirstFileNameW, FindNextFileNameW,
    GETFINALPATHNAMEBYHANDLE_FLAGS, GetFileInformationByHandle, GetFileInformationByHandleEx,
    GetFinalPathNameByHandleW, GetLongPathNameW, GetVolumeInformationByHandleW, OPEN_EXISTING,
    OpenFileById, VOLUME_NAME_GUID, VOLUME_NAME_NONE, WIN32_FIND_DATAW,
};
use windows::Win32::System::IO::{
    CancelIoEx, DeviceIoControl, GetOverlappedResult, IO_STATUS_BLOCK, OVERLAPPED,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::System::Threading::{
    CreateEventW, INFINITE, ResetEvent, SetEvent, WaitForMultipleObjects,
};
use windows::core::{PCWSTR, PWSTR, s, w};

use super::apply::{Links, Probe, Stat, attr};
use super::coalesce::{Event, Info, components, fold_key, skip_components};
use crate::{Result, wtf8};

/// A kernel handle closed on drop.
pub(crate) struct OwnedHandle(pub HANDLE);

// SAFETY: kernel handles may be used and closed from any thread.
unsafe impl Send for OwnedHandle {}
unsafe impl Sync for OwnedHandle {}

impl OwnedHandle {
    /// A manual-reset event, initially not signaled.
    pub fn event() -> Result<OwnedHandle> {
        Ok(OwnedHandle(unsafe {
            CreateEventW(None, true, false, PCWSTR::null())?
        }))
    }

    pub fn set(&self) {
        let _ = unsafe { SetEvent(self.0) };
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
}

/// The Win32 error code inside a `windows` error.
pub(crate) fn win32_code(e: &windows::core::Error) -> u32 {
    let hr = e.code().0 as u32;
    if hr & 0xFFFF_0000 == 0x8007_0000 {
        hr & 0xFFFF
    } else {
        hr
    }
}

/// `s` as UTF-16 with a terminating NUL.
pub(crate) fn wide_nul(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(Some(0)).collect()
}

/// A device I/O control on an overlapped handle, waited for.
pub(crate) fn ioctl_wait(
    handle: HANDLE,
    code: u32,
    input: Option<(*const c_void, u32)>,
    out: *mut c_void,
    out_len: u32,
) -> std::result::Result<u32, u32> {
    let event = OwnedHandle::event().map_err(|_| 8u32 /* ERROR_NOT_ENOUGH_MEMORY */)?;
    let mut ov = OVERLAPPED {
        hEvent: event.0,
        ..Default::default()
    };
    let (inp, in_len) = match input {
        Some((p, n)) => (Some(p), n),
        None => (None, 0),
    };
    let r = unsafe {
        DeviceIoControl(
            handle,
            code,
            inp,
            in_len,
            Some(out),
            out_len,
            None,
            Some(&mut ov),
        )
    };
    if let Err(e) = r {
        if win32_code(&e) != ERROR_IO_PENDING.0 {
            return Err(win32_code(&e));
        }
    }
    let mut n = 0u32;
    unsafe { GetOverlappedResult(handle, &ov, &mut n, true) }.map_err(|e| win32_code(&e))?;
    Ok(n)
}

/// What a collector does after a read finished.
pub(crate) enum Next {
    Continue,
    Stop,
}

/// A source of change records read with overlapped I/O.
pub(crate) trait Reader: Send {
    fn handle(&self) -> HANDLE;

    /// Starts one read that completes through `ov` (whose event is set
    /// when done). Returns a Win32 error code if it could not start.
    ///
    /// # Safety
    /// `ov` must stay valid until the read has completed.
    unsafe fn issue(&mut self, ov: *mut OVERLAPPED) -> std::result::Result<(), u32>;

    /// Turns a finished read (bytes, or a Win32 error) into events.
    fn finish(&mut self, res: std::result::Result<u32, u32>, out: &mut Vec<Event>) -> Next;
}

/// The collector loop: start a read, block until it completes or `stop`
/// is signaled, pass the records on, repeat. Idle, the thread sleeps in
/// `WaitForMultipleObjects` with no timeout. `started` is told whether the
/// first read could be started.
pub(crate) fn run_reader(
    r: &mut dyn Reader,
    stop: &OwnedHandle,
    tx: &Sender<Vec<Event>>,
    mut started: Option<Box<dyn FnOnce(Result<()>) + Send>>,
) {
    let io = match OwnedHandle::event() {
        Ok(e) => e,
        Err(e) => {
            if let Some(s) = started.take() {
                s(Err(e));
            }
            return;
        }
    };
    let mut ov: Box<OVERLAPPED> = Box::default();
    let mut failures_in_a_row = 0;
    loop {
        *ov = OVERLAPPED {
            hEvent: io.0,
            ..Default::default()
        };
        let _ = unsafe { ResetEvent(io.0) };
        let res = match unsafe { r.issue(&mut *ov) } {
            Ok(()) => {
                if let Some(s) = started.take() {
                    s(Ok(()));
                }
                failures_in_a_row = 0;
                // `stop` first: it wins when both are signaled.
                let w = unsafe { WaitForMultipleObjects(&[stop.0, io.0], false, INFINITE) };
                if w.0 != WAIT_OBJECT_0.0 + 1 {
                    // Stop (or a failed wait): cancel the read and wait
                    // until the system no longer uses the buffer.
                    unsafe {
                        let _ = CancelIoEx(r.handle(), Some(&*ov));
                        let mut n = 0u32;
                        let _ = GetOverlappedResult(r.handle(), &*ov, &mut n, true);
                    }
                    return;
                }
                let mut n = 0u32;
                unsafe { GetOverlappedResult(r.handle(), &*ov, &mut n, false) }
                    .map(|()| n)
                    .map_err(|e| win32_code(&e))
            }
            Err(code) => {
                failures_in_a_row += 1;
                Err(code)
            }
        };
        let mut out = Vec::new();
        let next = if failures_in_a_row > 3 {
            Next::Stop
        } else {
            r.finish(res, &mut out)
        };
        if let Next::Stop = next {
            if let Some(s) = started.take() {
                // Never started: the caller gets the error instead of events.
                let code = res.err().unwrap_or(0);
                s(Err(std::io::Error::from_raw_os_error(code as i32).into()));
                return;
            }
            if failures_in_a_row > 3 {
                out.push(Event::Failed(format!(
                    "watching stopped: {}",
                    std::io::Error::from_raw_os_error(res.err().unwrap_or(0) as i32)
                )));
            }
        }
        if !out.is_empty() && tx.send(out).is_err() {
            return; // the applier is gone
        }
        if let Next::Stop = next {
            return;
        }
    }
}

/// `root` made absolute in the `\\?\` form, without a trailing separator
/// (except for a drive root such as `\\?\C:\`).
pub(crate) fn verbatim_root(root: &Path) -> std::io::Result<Vec<u16>> {
    let abs = std::path::absolute(root)?;
    let w: Vec<u16> = abs.as_os_str().encode_wide().collect();
    let u = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
    let mut out = if w.starts_with(&u(r"\\?\")) {
        w
    } else if w.starts_with(&u(r"\\.\")) {
        [u(r"\\?\"), w[4..].to_vec()].concat()
    } else if w.starts_with(&u(r"\\")) {
        [u(r"\\?\UNC\"), w[2..].to_vec()].concat()
    } else {
        [u(r"\\?\"), w].concat()
    };
    while out.len() > 4 && out.last() == Some(&(b'\\' as u16)) {
        out.pop();
    }
    if out.last() == Some(&(b':' as u16)) {
        out.push(b'\\' as u16);
    }
    Ok(out)
}

/// `base` joined with a relative WTF-8 path.
pub(crate) fn join(base: &[u16], rel: &[u8]) -> Vec<u16> {
    let mut v = base.to_vec();
    if !rel.is_empty() {
        if v.last() != Some(&(b'\\' as u16)) {
            v.push(b'\\' as u16);
        }
        v.extend(
            wtf8::to_utf16(rel)
                .into_iter()
                .map(|c| if c == b'/' as u16 { b'\\' as u16 } else { c }),
        );
    }
    v
}

pub(crate) fn join_path(base: &[u16], rel: &[u8]) -> PathBuf {
    std::ffi::OsString::from_wide(&join(base, rel)).into()
}

/// `NtQueryInformationByName` (Windows 10 1709+).
type QueryByName = unsafe extern "system" fn(
    *const OBJECT_ATTRIBUTES,
    *mut IO_STATUS_BLOCK,
    *mut c_void,
    u32,
    FILE_INFORMATION_CLASS,
) -> NTSTATUS;

/// `NtQueryInformationByName` if this system has it (looked up at run time
/// so the program still loads where it does not).
fn query_by_name_api() -> Option<QueryByName> {
    unsafe {
        let ntdll = GetModuleHandleW(w!("ntdll.dll")).ok()?;
        let f = GetProcAddress(ntdll, s!("NtQueryInformationByName"))?;
        Some(std::mem::transmute::<
            unsafe extern "system" fn() -> isize,
            QueryByName,
        >(f))
    }
}

/// Reads attributes of entries below the watched root without opening
/// their contents.
pub(crate) struct WinProbe {
    /// `\\?\C:\root`
    win32: Vec<u16>,
    /// `\??\C:\root`, for the native API.
    nt: Vec<u16>,
    /// For `locate` and `links`: a handle on the root and its
    /// volume-relative path.
    locator: Option<(OwnedHandle, Vec<u8>)>,
    /// Hard links are followed: local NTFS, and the root has a locator.
    links: bool,
    /// For `link_count`, until it turns out not to work here.
    by_name: Option<QueryByName>,
}

impl WinProbe {
    /// `locate` and `links` need the root opened by handle; with
    /// `need_locator` (USN) failing to open it is an error, otherwise hard
    /// links just go untracked.
    pub fn new(root: &Path, need_locator: bool) -> Result<WinProbe> {
        let win32 = verbatim_root(root)?;
        let mut nt: Vec<u16> = r"\??\".encode_utf16().collect();
        nt.extend_from_slice(&win32[4..]);
        let path = [win32.clone(), vec![0]].concat();
        let opened = unsafe {
            CreateFileW(
                PCWSTR(path.as_ptr()),
                FILE_READ_ATTRIBUTES.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                None,
            )
        };
        let locator = match opened {
            Ok(h) => {
                let h = OwnedHandle(h);
                final_path(h.0, VOLUME_NAME_NONE).map(|p| (h, wtf8::from_utf16(&p)))
            }
            Err(e) if need_locator => return Err(e.into()),
            Err(_) => None,
        };
        let links = locator.as_ref().is_some_and(|(h, _)| is_ntfs(h.0));
        Ok(WinProbe {
            win32,
            nt,
            locator,
            links,
            by_name: if links { query_by_name_api() } else { None },
        })
    }

    pub fn win32_root(&self) -> &[u16] {
        &self.win32
    }

    /// The directory entry of `rel` (attributes, logical size, reparse tag).
    fn find_data(&self, rel: &[u8]) -> std::result::Result<WIN32_FIND_DATAW, u32> {
        let path = [join(&self.win32, rel), vec![0]].concat();
        let mut data = WIN32_FIND_DATAW::default();
        let h = unsafe {
            FindFirstFileExW(
                PCWSTR(path.as_ptr()),
                FindExInfoBasic,
                (&mut data as *mut WIN32_FIND_DATAW).cast::<c_void>(),
                FindExSearchNameMatch,
                None,
                FIND_FIRST_EX_FLAGS(0),
            )
        }
        .map_err(|e| win32_code(&e))?;
        let _ = unsafe { FindClose(h) };
        Ok(data)
    }
}

/// The path of an open file: volume-relative (`\dir\file`) with
/// `VOLUME_NAME_NONE`, `\\?\Volume{…}\dir\file` with `VOLUME_NAME_GUID`.
fn final_path(h: HANDLE, volume: GETFINALPATHNAMEBYHANDLE_FLAGS) -> Option<Vec<u16>> {
    let mut buf = vec![0u16; 512];
    loop {
        let n = unsafe {
            GetFinalPathNameByHandleW(
                h,
                &mut buf,
                GETFINALPATHNAMEBYHANDLE_FLAGS(FILE_NAME_NORMALIZED.0 | volume.0),
            )
        } as usize;
        if n == 0 {
            return None;
        }
        if n < buf.len() {
            buf.truncate(n);
            return Some(buf);
        }
        buf.resize(n + 1, 0);
    }
}

/// True if the volume of `h` is NTFS (hard links, file reference numbers).
fn is_ntfs(h: HANDLE) -> bool {
    let mut name = [0u16; 32];
    let ok = unsafe { GetVolumeInformationByHandleW(h, None, None, None, None, Some(&mut name)) }
        .is_ok();
    let len = name.iter().position(|&c| c == 0).unwrap_or(name.len());
    ok && name[..len] == *"NTFS".encode_utf16().collect::<Vec<u16>>()
}

/// Opens the file with id `id` on the volume of `hint`, for attributes.
fn open_by_id(hint: HANDLE, id: u64) -> Option<OwnedHandle> {
    let desc = FILE_ID_DESCRIPTOR {
        dwSize: std::mem::size_of::<FILE_ID_DESCRIPTOR>() as u32,
        Type: FileIdType,
        Anonymous: FILE_ID_DESCRIPTOR_0 { FileId: id as i64 },
    };
    let h = unsafe {
        OpenFileById(
            hint,
            &desc,
            FILE_READ_ATTRIBUTES.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
        )
    };
    h.ok().map(OwnedHandle)
}

/// A handle from `FindFirstFileNameW`, closed on drop.
struct FindHandle(HANDLE);

impl Drop for FindHandle {
    fn drop(&mut self) {
        let _ = unsafe { FindClose(self.0) };
    }
}

/// All names (hard links) of the file at `path`, relative to its volume
/// (`\dir\file`).
fn link_names(path: &[u16]) -> Option<Vec<Vec<u16>>> {
    let path = [path, &[0]].concat();
    let mut buf = vec![0u16; 512];
    let mut names = Vec::new();
    let mut find: Option<FindHandle> = None;
    loop {
        let mut len = buf.len() as u32;
        let r = match &find {
            None => unsafe {
                FindFirstFileNameW(PCWSTR(path.as_ptr()), 0, &mut len, PWSTR(buf.as_mut_ptr()))
                    .map(|h| find = Some(FindHandle(h)))
            },
            Some(h) => unsafe { FindNextFileNameW(h.0, &mut len, PWSTR(buf.as_mut_ptr())) },
        };
        match r {
            Ok(()) => {
                let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
                names.push(buf[..n].to_vec());
            }
            Err(e) if win32_code(&e) == ERROR_MORE_DATA.0 && len as usize > buf.len() => {
                buf.resize(len as usize, 0);
            }
            Err(e) if find.is_some() && win32_code(&e) == ERROR_HANDLE_EOF.0 => {
                return Some(names);
            }
            Err(_) => return None,
        }
    }
}

/// `path` (volume-relative, WTF-8) relative to `root` (likewise), if it is
/// the root or below it.
fn below_root(path: &[u8], root: &[u8]) -> Option<Vec<u8>> {
    let (key, root_key) = (fold_key(path), fold_key(root));
    if key == root_key {
        return Some(Vec::new());
    }
    let below = key.len() > root_key.len()
        && key.starts_with(&root_key)
        && (root_key.is_empty() || key[root_key.len()] == b'\\');
    if !below {
        return None;
    }
    Some(skip_components(path, components(root).count()).to_vec())
}

fn is_not_found(status: NTSTATUS) -> bool {
    const STATUS_NO_SUCH_FILE: i32 = 0xC000_000F_u32 as i32;
    const STATUS_OBJECT_NAME_INVALID: i32 = 0xC000_0033_u32 as i32;
    const STATUS_NOT_A_DIRECTORY: i32 = 0xC000_0103_u32 as i32;
    status == STATUS_OBJECT_NAME_NOT_FOUND
        || status == STATUS_OBJECT_PATH_NOT_FOUND
        || status == STATUS_DELETE_PENDING
        || matches!(
            status.0,
            STATUS_NO_SUCH_FILE | STATUS_OBJECT_NAME_INVALID | STATUS_NOT_A_DIRECTORY
        )
}

impl Probe for WinProbe {
    fn stat(&mut self, rel: &[u8]) -> Info {
        if rel.is_empty() {
            return Info::Unknown;
        }
        // NtQueryFullAttributesFile (documented as ZwQueryFullAttributesFile)
        // is what GetFileAttributesExW uses, but it also returns the
        // allocation size, so one call gives everything.
        let path = join(&self.nt, rel);
        let Ok(bytes) = u16::try_from(path.len() * 2) else {
            return Info::Unknown;
        };
        let name = UNICODE_STRING {
            Length: bytes,
            MaximumLength: bytes,
            Buffer: PWSTR(path.as_ptr() as *mut u16),
        };
        let oa = OBJECT_ATTRIBUTES {
            Length: std::mem::size_of::<OBJECT_ATTRIBUTES>() as u32,
            ObjectName: &name,
            Attributes: OBJ_CASE_INSENSITIVE,
            ..Default::default()
        };
        let mut info = FILE_NETWORK_OPEN_INFORMATION::default();
        let status = unsafe { NtQueryFullAttributesFile(&oa, &mut info) };
        if status.0 >= 0 {
            let reparse_tag = if info.FileAttributes & attr::REPARSE_POINT != 0 {
                self.find_data(rel).map_or(0, |d| d.dwReserved0)
            } else {
                0
            };
            return Info::Present(Stat {
                attributes: info.FileAttributes,
                reparse_tag,
                size: info.AllocationSize as u64,
                logical: info.EndOfFile as u64,
                mtime: crate::tree::mtime_from_filetime(info.LastWriteTime as u64),
            });
        }
        if is_not_found(status) {
            return Info::Gone;
        }
        // E.g. a sharing violation on a paging file: fall back to the
        // directory entry (its allocation size is not known: use the length).
        match self.find_data(rel) {
            Ok(d) => {
                let logical = ((d.nFileSizeHigh as u64) << 32) | d.nFileSizeLow as u64;
                Info::Present(Stat {
                    attributes: d.dwFileAttributes,
                    reparse_tag: if d.dwFileAttributes & attr::REPARSE_POINT != 0 {
                        d.dwReserved0
                    } else {
                        0
                    },
                    size: logical,
                    logical,
                    mtime: crate::tree::mtime_from_filetime(
                        ((d.ftLastWriteTime.dwHighDateTime as u64) << 32)
                            | d.ftLastWriteTime.dwLowDateTime as u64,
                    ),
                })
            }
            Err(2 | 3) => Info::Gone, // ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND
            Err(_) => Info::Unknown,
        }
    }

    fn long_name(&mut self, rel: &[u8]) -> Option<Vec<u8>> {
        let path = [join(&self.win32, rel), vec![0]].concat();
        let mut buf = vec![0u16; 1024];
        loop {
            let n = unsafe { GetLongPathNameW(PCWSTR(path.as_ptr()), Some(&mut buf)) } as usize;
            if n == 0 {
                return None;
            }
            if n < buf.len() {
                buf.truncate(n);
                break;
            }
            buf.resize(n + 1, 0);
        }
        let start = buf
            .iter()
            .rposition(|&c| c == b'\\' as u16)
            .map_or(0, |i| i + 1);
        Some(wtf8::from_utf16(&buf[start..]))
    }

    fn locate(&mut self, frn: u64) -> Option<Vec<u8>> {
        let (hint, root) = self.locator.as_ref()?;
        let h = open_by_id(hint.0, frn)?;
        let path = wtf8::from_utf16(&final_path(h.0, VOLUME_NAME_NONE)?);
        below_root(&path, root)
    }

    fn tracks_links(&self) -> bool {
        self.links
    }

    fn link_count(&mut self, rel: &[u8]) -> Option<(u64, u32)> {
        if !self.links || rel.is_empty() {
            return None;
        }
        if let Some(query) = self.by_name {
            // One call, as for `stat`: the file's id and link count by name.
            let path = join(&self.nt, rel);
            let bytes = u16::try_from(path.len() * 2).ok()?;
            let name = UNICODE_STRING {
                Length: bytes,
                MaximumLength: bytes,
                Buffer: PWSTR(path.as_ptr() as *mut u16),
            };
            let oa = OBJECT_ATTRIBUTES {
                Length: std::mem::size_of::<OBJECT_ATTRIBUTES>() as u32,
                ObjectName: &name,
                Attributes: OBJ_CASE_INSENSITIVE,
                ..Default::default()
            };
            let mut io = IO_STATUS_BLOCK::default();
            let mut info = FILE_STAT_INFORMATION::default();
            let status = unsafe {
                query(
                    &oa,
                    &mut io,
                    (&mut info as *mut FILE_STAT_INFORMATION).cast(),
                    std::mem::size_of::<FILE_STAT_INFORMATION>() as u32,
                    FileStatInformation,
                )
            };
            if status.0 >= 0 {
                return Some((info.FileId as u64, info.NumberOfLinks));
            }
            if status != STATUS_INVALID_INFO_CLASS
                && status != STATUS_NOT_IMPLEMENTED
                && status != STATUS_INVALID_PARAMETER
            {
                return None; // gone, access denied...
            }
            // Not supported here: open the file instead from now on.
            self.by_name = None;
        }
        let path = [join(&self.win32, rel), vec![0]].concat();
        let h = OwnedHandle(
            unsafe {
                CreateFileW(
                    PCWSTR(path.as_ptr()),
                    FILE_READ_ATTRIBUTES.0,
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                    None,
                    OPEN_EXISTING,
                    FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                    None,
                )
            }
            .ok()?,
        );
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        unsafe { GetFileInformationByHandle(h.0, &mut info) }.ok()?;
        let id = ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64;
        Some((id, info.nNumberOfLinks))
    }

    fn links(&mut self, id: u64) -> Option<Links> {
        if !self.links {
            return None;
        }
        let (hint, root) = self.locator.as_ref()?;
        let h = open_by_id(hint.0, id)?;
        let mut info = FILE_STANDARD_INFO::default();
        unsafe {
            GetFileInformationByHandleEx(
                h.0,
                FileStandardInfo,
                (&mut info as *mut FILE_STANDARD_INFO).cast(),
                std::mem::size_of::<FILE_STANDARD_INFO>() as u32,
            )
        }
        .ok()?;
        if info.Directory {
            return None;
        }
        // The names are listed through a path; any of the file's will do.
        let path = final_path(h.0, VOLUME_NAME_GUID)?;
        let names = link_names(&path)?
            .iter()
            .filter_map(|n| below_root(&wtf8::from_utf16(n), root))
            .filter(|rel| !rel.is_empty())
            .collect();
        Some(Links {
            size: info.AllocationSize as u64,
            logical: info.EndOfFile as u64,
            names,
        })
    }
}

/// True when running under Wine (for diagnostics and tests).
#[allow(dead_code)]
pub(crate) fn is_wine() -> bool {
    unsafe {
        match GetModuleHandleW(w!("ntdll.dll")) {
            Ok(m) => GetProcAddress(m, s!("wine_get_version")).is_some(),
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[u16]) -> String {
        String::from_utf16_lossy(v)
    }

    #[test]
    fn verbatim_roots() {
        assert_eq!(s(&verbatim_root(Path::new(r"C:\")).unwrap()), r"\\?\C:\");
        assert_eq!(
            s(&verbatim_root(Path::new(r"C:\a\b\")).unwrap()),
            r"\\?\C:\a\b"
        );
        assert_eq!(
            s(&verbatim_root(Path::new(r"\\?\D:\x")).unwrap()),
            r"\\?\D:\x"
        );
        assert_eq!(
            s(&verbatim_root(Path::new(r"\\server\share\dir")).unwrap()),
            r"\\?\UNC\server\share\dir"
        );
        let base = verbatim_root(Path::new(r"C:\")).unwrap();
        assert_eq!(s(&join(&base, b"a/b")), r"\\?\C:\a\b");
        assert_eq!(s(&join(&base, b"")), r"\\?\C:\");
    }

    #[test]
    fn paths_below_the_root() {
        assert_eq!(
            below_root(b"\\a\\B\\c.txt", b"\\A\\b"),
            Some(b"c.txt".to_vec())
        );
        assert_eq!(below_root(b"\\a\\b", b"\\a\\b"), Some(Vec::new()));
        assert_eq!(below_root(b"\\a\\bc\\x", b"\\a\\b"), None);
        assert_eq!(below_root(b"\\x\\y", b"\\"), Some(b"x\\y".to_vec()));
        assert_eq!(below_root(b"\\x", b"\\a"), None);
    }

    #[test]
    fn hard_links_by_id() {
        let dir = std::env::temp_dir().join(format!("tessera-links-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.bin"), vec![1u8; 5000]).unwrap();
        std::fs::hard_link(dir.join("a.bin"), dir.join("sub").join("b.bin")).unwrap();
        let mut p = WinProbe::new(&dir, false).unwrap();
        if !p.tracks_links() || is_wine() {
            eprintln!("skipped: hard links are not tracked here");
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        let (id, n) = p.link_count(b"sub\\b.bin").unwrap();
        assert_eq!(n, 2);
        assert_eq!(p.link_count(b"a.bin").unwrap(), (id, 2));
        // The fallback through a handle agrees.
        p.by_name = None;
        assert_eq!(p.link_count(b"a.bin").unwrap(), (id, 2));
        let mut l = p.links(id).unwrap();
        l.names.sort();
        assert_eq!(l.names, vec![b"a.bin".to_vec(), b"sub\\b.bin".to_vec()]);
        assert_eq!(l.logical, 5000);
        assert!(l.size >= 5000);
        assert!(p.link_count(b"nope").is_none());
        std::fs::remove_file(dir.join("a.bin")).unwrap();
        assert_eq!(p.links(id).unwrap().names, vec![b"sub\\b.bin".to_vec()]);
        drop(p);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
