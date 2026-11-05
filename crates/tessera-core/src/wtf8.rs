//! WTF-8: UTF-8 extended to carry unpaired UTF-16 surrogates.
//!
//! Windows file names are arbitrary sequences of `u16` and are not always
//! valid UTF-16. WTF-8 stores them losslessly in a byte buffer that is plain
//! UTF-8 whenever the name is well formed (which is nearly always), so the
//! name pool costs about one byte per ASCII character instead of two.

/// Appends `units` (possibly ill-formed UTF-16) to `out` as WTF-8.
pub fn push_utf16(out: &mut Vec<u8>, units: &[u16]) {
    out.reserve(units.len());
    let mut i = 0;
    while i < units.len() {
        let u = units[i];
        // ASCII fast path: the overwhelming majority of file name characters.
        if u < 0x80 {
            out.push(u as u8);
            i += 1;
            continue;
        }
        let cp: u32 = if (0xD800..0xDC00).contains(&u)
            && i + 1 < units.len()
            && (0xDC00..0xE000).contains(&units[i + 1])
        {
            let hi = (u as u32 - 0xD800) << 10;
            let lo = units[i + 1] as u32 - 0xDC00;
            i += 2;
            0x10000 + (hi | lo)
        } else {
            i += 1;
            u as u32
        };
        push_code_point(out, cp);
    }
}

/// Encodes one code point (surrogates allowed) as WTF-8.
#[inline]
fn push_code_point(out: &mut Vec<u8>, cp: u32) {
    if cp < 0x80 {
        out.push(cp as u8);
    } else if cp < 0x800 {
        out.extend_from_slice(&[0xC0 | (cp >> 6) as u8, 0x80 | (cp & 0x3F) as u8]);
    } else if cp < 0x10000 {
        out.extend_from_slice(&[
            0xE0 | (cp >> 12) as u8,
            0x80 | ((cp >> 6) & 0x3F) as u8,
            0x80 | (cp & 0x3F) as u8,
        ]);
    } else {
        out.extend_from_slice(&[
            0xF0 | (cp >> 18) as u8,
            0x80 | ((cp >> 12) & 0x3F) as u8,
            0x80 | ((cp >> 6) & 0x3F) as u8,
            0x80 | (cp & 0x3F) as u8,
        ]);
    }
}

/// Converts UTF-16 to a new WTF-8 buffer.
pub fn from_utf16(units: &[u16]) -> Vec<u8> {
    let mut v = Vec::with_capacity(units.len());
    push_utf16(&mut v, units);
    v
}

/// Iterates the code points of a WTF-8 buffer (surrogates included).
///
/// The input must have been produced by this module; malformed bytes are
/// mapped to U+FFFD rather than panicking.
pub fn code_points(bytes: &[u8]) -> CodePoints<'_> {
    CodePoints { bytes, pos: 0 }
}

pub struct CodePoints<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Iterator for CodePoints<'_> {
    type Item = u32;

    #[inline]
    fn next(&mut self) -> Option<u32> {
        let b = self.bytes;
        let i = self.pos;
        let b0 = *b.get(i)? as u32;
        let (cp, len) = if b0 < 0x80 {
            (b0, 1)
        } else if b0 & 0xE0 == 0xC0 && i + 1 < b.len() {
            (((b0 & 0x1F) << 6) | (b[i + 1] as u32 & 0x3F), 2)
        } else if b0 & 0xF0 == 0xE0 && i + 2 < b.len() {
            (
                ((b0 & 0x0F) << 12) | ((b[i + 1] as u32 & 0x3F) << 6) | (b[i + 2] as u32 & 0x3F),
                3,
            )
        } else if b0 & 0xF8 == 0xF0 && i + 3 < b.len() {
            (
                ((b0 & 0x07) << 18)
                    | ((b[i + 1] as u32 & 0x3F) << 12)
                    | ((b[i + 2] as u32 & 0x3F) << 6)
                    | (b[i + 3] as u32 & 0x3F),
                4,
            )
        } else {
            (0xFFFD, 1)
        };
        self.pos += len;
        Some(cp)
    }
}

/// Appends a WTF-8 buffer to `out` as UTF-16 (for Win32 calls).
pub fn push_to_utf16(out: &mut Vec<u16>, bytes: &[u8]) {
    out.reserve(bytes.len());
    for cp in code_points(bytes) {
        if cp >= 0x10000 {
            let c = cp - 0x10000;
            out.push(0xD800 | (c >> 10) as u16);
            out.push(0xDC00 | (c & 0x3FF) as u16);
        } else {
            out.push(cp as u16);
        }
    }
}

/// Converts WTF-8 to a new UTF-16 buffer (no terminating NUL).
pub fn to_utf16(bytes: &[u8]) -> Vec<u16> {
    let mut v = Vec::with_capacity(bytes.len());
    push_to_utf16(&mut v, bytes);
    v
}

/// Converts WTF-8 to a `String`, replacing lone surrogates with U+FFFD.
pub fn to_string_lossy(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_owned(),
        Err(_) => code_points(bytes)
            .map(|cp| char::from_u32(cp).unwrap_or('\u{FFFD}'))
            .collect(),
    }
}

/// Simple case folding close to what NTFS does for name lookups
/// (NTFS compares names through an upper-case table).
#[inline]
fn fold(cp: u32) -> u32 {
    if cp < 0x80 {
        return (cp as u8).to_ascii_uppercase() as u32;
    }
    match char::from_u32(cp) {
        Some(c) => {
            let mut up = c.to_uppercase();
            match (up.next(), up.next()) {
                (Some(u), None) => u as u32,
                // Multi-char expansions (e.g. 'ß' -> "SS") are not folded by NTFS.
                _ => cp,
            }
        }
        None => cp, // lone surrogate
    }
}

/// Case-insensitive comparison of two WTF-8 names, Windows style.
pub fn eq_ignore_case(a: &[u8], b: &[u8]) -> bool {
    if a == b {
        return true;
    }
    if a.is_ascii() && b.is_ascii() {
        return a.eq_ignore_ascii_case(b);
    }
    let mut x = code_points(a);
    let mut y = code_points(b);
    loop {
        match (x.next(), y.next()) {
            (None, None) => return true,
            (Some(p), Some(q)) if fold(p) == fold(q) => {}
            _ => return false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_well_formed_utf16() {
        for s in [
            "",
            "hello.txt",
            "Program Files (x86)",
            "照片 2024",
            "🦀 crab.rs",
            "naïve",
        ] {
            let units: Vec<u16> = s.encode_utf16().collect();
            let w = from_utf16(&units);
            assert_eq!(w, s.as_bytes(), "well-formed UTF-16 must give plain UTF-8");
            assert_eq!(to_utf16(&w), units);
            assert_eq!(to_string_lossy(&w), s);
        }
    }

    #[test]
    fn round_trips_lone_surrogates() {
        let units = [0x0041, 0xD800, 0x0042, 0xDC00, 0xD83E, 0xDD80, 0xDBFF];
        let w = from_utf16(&units);
        assert_eq!(to_utf16(&w), units);
        assert!(std::str::from_utf8(&w).is_err());
        assert_eq!(to_string_lossy(&w), "A\u{FFFD}B\u{FFFD}🦀\u{FFFD}");
    }

    #[test]
    fn case_insensitive_compare() {
        assert!(eq_ignore_case(b"Windows", b"WINDOWS"));
        assert!(eq_ignore_case("Übung".as_bytes(), "üBUNG".as_bytes()));
        assert!(eq_ignore_case("ΣΊΣΥΦΟΣ".as_bytes(), "σίσυφοσ".as_bytes()));
        assert!(!eq_ignore_case(b"abc", b"abd"));
        assert!(!eq_ignore_case(b"abc", b"abcd"));
    }
}
