use std::fmt;

/// Errors returned by the core.
#[derive(Debug)]
pub enum Error {
    /// An OS call failed.
    Io(std::io::Error),
    /// The operation is not possible here (e.g. MFT scan without admin
    /// rights, or on a non-NTFS volume). The caller may fall back.
    Unsupported(&'static str),
    /// The volume's on-disk data could not be parsed.
    Corrupt(String),
    /// The scan was cancelled through [`crate::scan::ScanProgress::cancel`].
    Cancelled,
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Unsupported(what) => write!(f, "not supported: {what}"),
            Error::Corrupt(what) => write!(f, "unexpected on-disk data: {what}"),
            Error::Cancelled => f.write_str("cancelled"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

#[cfg(windows)]
impl From<windows::core::Error> for Error {
    fn from(e: windows::core::Error) -> Self {
        // HRESULT_FROM_WIN32 wraps Win32 codes as 0x8007xxxx; unwrap them so
        // io::Error prints the familiar message.
        let hr = e.code().0 as u32;
        let code = if hr & 0xFFFF_0000 == 0x8007_0000 {
            hr & 0xFFFF
        } else {
            hr
        };
        Error::Io(std::io::Error::from_raw_os_error(code as i32))
    }
}
