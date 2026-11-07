//! Symbol-table compression for short strings, after FSST (Boncz, Neumann,
//! Leis: "FSST: Fast Random Access String Compression", VLDB 2020).
//!
//! A table holds up to 255 symbols of 1 to 8 bytes each. A string is encoded
//! greedily as one code byte per symbol; a byte no symbol covers becomes the
//! escape code 255 followed by the byte itself. Any single string can be
//! decoded on its own, which suits a name pool that is read one name at a
//! time.
//!
//! As in FSST, each symbol of 3 or more bytes is found through a hash of its
//! first 3 bytes, with room for one symbol per slot; training skips a symbol
//! whose slot is taken. Encoding is deterministic: equal strings always get
//! equal codes.

/// The code that escapes one literal byte.
pub(crate) const ESCAPE: u8 = 255;
const MAX_SYMBOLS: usize = 255;
const HASH_BITS: u32 = 10;
/// Training rounds; the last one only re-counts and makes no new symbols.
const ROUNDS: usize = 5;
/// Candidates seen fewer times than this in the sample are not kept.
const MIN_COUNT: u32 = 5;

/// Symbols and the lookup tables used to encode with them.
pub(crate) struct SymbolTable {
    /// Symbol bytes by code, little-endian, zero above the length.
    sym: [u64; 256],
    /// Symbol length by code (0 for unused codes).
    len: [u8; 256],
    /// For the next two input bytes `b0 | b1 << 8`: the 2-byte symbol if there
    /// is one, else the 1-byte symbol for `b0`, else [`ESCAPE`]. Packed as
    /// `code | consumed << 8`.
    short: Box<[u16]>,
    /// The same for a last single byte.
    single: [u16; 256],
    /// The symbol of 3+ bytes in each hash slot.
    long: Box<[Slot]>,
    count: usize,
}

/// A symbol of 3 or more bytes, as stored in its hash slot.
#[derive(Clone, Copy)]
#[repr(C)]
struct Slot {
    sym: u64,
    /// `64 - 8 * len`: shifting `u64::MAX` right by it masks the symbol.
    shift: u32,
    code: u8,
    /// 1 to 8; [`Slot::EMPTY`] has 9, longer than any input left.
    len: u8,
}

impl Slot {
    const EMPTY: Slot = Slot {
        sym: 0,
        shift: 0,
        code: ESCAPE,
        len: 9,
    };
}

#[inline]
fn slot(prefix3: u64) -> usize {
    ((prefix3 as u32).wrapping_mul(0x9E37_79B1) >> (32 - HASH_BITS)) as usize
}

#[inline]
fn mask(len: usize) -> u64 {
    u64::MAX >> (64 - 8 * len.clamp(1, 8))
}

/// Up to 8 bytes of `s` from `pos`, little-endian, zero-padded.
#[inline]
fn load(s: &[u8], pos: usize) -> u64 {
    match s.get(pos..pos + 8) {
        Some(w) => u64::from_le_bytes(w.try_into().unwrap()),
        None => {
            let mut buf = [0u8; 8];
            let rest = &s[pos..];
            buf[..rest.len()].copy_from_slice(rest);
            u64::from_le_bytes(buf)
        }
    }
}

impl SymbolTable {
    fn empty() -> SymbolTable {
        const ESC: u16 = ESCAPE as u16 | 1 << 8;
        SymbolTable {
            sym: [0; 256],
            len: [0; 256],
            short: vec![ESC; 1 << 16].into_boxed_slice(),
            single: [ESC; 256],
            long: vec![Slot::EMPTY; 1 << HASH_BITS].into_boxed_slice(),
            count: 0,
        }
    }

    /// Adds a symbol unless the table is full or its hash slot is taken.
    fn add(&mut self, sym: u64, len: usize) -> bool {
        if self.count == MAX_SYMBOLS {
            return false;
        }
        let code = self.count as u8;
        match len {
            1 => {
                let b = sym as u8;
                if self.single[b as usize] as u8 != ESCAPE {
                    return false;
                }
                self.single[b as usize] = u16::from(code) | 1 << 8;
            }
            2 => {
                let i = (sym & 0xFFFF) as usize;
                if self.short[i] >> 8 == 2 {
                    return false;
                }
                self.short[i] = u16::from(code) | 2 << 8;
            }
            _ => {
                let s = slot(sym & 0xFF_FFFF);
                if self.long[s].len != Slot::EMPTY.len {
                    return false;
                }
                self.long[s] = Slot {
                    sym: sym & mask(len),
                    shift: (64 - 8 * len) as u32,
                    code,
                    len: len as u8,
                };
            }
        }
        self.sym[code as usize] = sym & mask(len);
        self.len[code as usize] = len as u8;
        self.count += 1;
        true
    }

    /// Fills the 2-byte entries that have no 2-byte symbol with the 1-byte
    /// symbol (or escape) for their first byte. Call once all symbols are in.
    fn finish(&mut self) {
        for (i, e) in self.short.iter_mut().enumerate() {
            if *e >> 8 != 2 {
                *e = self.single[i & 0xFF];
            }
        }
    }

    /// The symbol to use at `pos`: `(code, bytes consumed)`, where the code is
    /// [`ESCAPE`] for a literal byte.
    #[inline]
    fn find(&self, s: &[u8], pos: usize) -> (u8, usize) {
        self.find_word(load(s, pos), s.len() - pos)
    }

    /// [`SymbolTable::find`] for the next (up to) 8 input bytes `w`, of
    /// which `left` are real.
    #[inline]
    fn find_word(&self, w: u64, left: usize) -> (u8, usize) {
        let e = if left >= 2 {
            self.short[(w & 0xFFFF) as usize]
        } else {
            self.single[(w & 0xFF) as usize]
        };
        let long = self.long[slot(w & 0xFF_FFFF)];
        // chosen without a branch: whether a long symbol matches is
        // unpredictable
        let hit = (long.len as usize) <= left && w & (u64::MAX >> long.shift) == long.sym;
        let code = if hit { long.code } else { e as u8 };
        let len = if hit {
            long.len as usize
        } else {
            (e >> 8) as usize
        };
        (code, len)
    }

    /// Encodes `s` into `out`, which needs `2 * s.len()` bytes of room, and
    /// returns the encoded length.
    #[inline]
    pub(crate) fn encode_into(&self, s: &[u8], out: &mut [u8]) -> usize {
        let mut pos = 0;
        let mut k = 0;
        let mut step = |w: u64, left: usize, pos: &mut usize| {
            let (c, l) = self.find_word(w, left);
            // the literal is written in any case and kept only after an escape
            out[k] = c;
            out[k + 1] = w as u8;
            k += 1 + usize::from(c == ESCAPE);
            *pos += l;
        };
        // whole 8-byte words while they last, then the zero-padded tail
        while pos + 8 <= s.len() {
            let w = u64::from_le_bytes(s[pos..pos + 8].try_into().unwrap());
            step(w, 8, &mut pos);
        }
        while pos < s.len() {
            step(load(s, pos), s.len() - pos, &mut pos);
        }
        k
    }

    /// Appends the encoding of `s` to `out`.
    pub(crate) fn encode(&self, s: &[u8], out: &mut Vec<u8>) {
        let start = out.len();
        out.resize(start + 2 * s.len(), 0);
        let k = self.encode_into(s, &mut out[start..]);
        out.truncate(start + k);
    }

    /// Length of `codes` once decoded.
    pub(crate) fn decoded_len(&self, codes: &[u8]) -> usize {
        let mut n = 0;
        let mut i = 0;
        while i < codes.len() {
            let c = codes[i];
            if c == ESCAPE {
                n += 1;
                i += 2;
            } else {
                n += self.len[c as usize] as usize;
                i += 1;
            }
        }
        n
    }

    /// Decodes `codes` into the start of `out` and returns the length.
    /// `out` needs [`SymbolTable::decoded_len`] plus 7 bytes of room.
    #[inline]
    pub(crate) fn decode(&self, codes: &[u8], out: &mut [u8]) -> usize {
        let mut pos = 0;
        let mut i = 0;
        while i < codes.len() {
            let c = codes[i];
            if c == ESCAPE {
                out[pos] = codes[i + 1];
                pos += 1;
                i += 2;
            } else {
                out[pos..pos + 8].copy_from_slice(&self.sym[c as usize].to_le_bytes());
                pos += self.len[c as usize] as usize;
                i += 1;
            }
        }
        pos
    }

    /// Number of symbols.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.count
    }

    /// Heap bytes held.
    pub(crate) fn heap_bytes(&self) -> usize {
        self.short.len() * 2 + self.long.len()
    }

    /// Builds a table for strings like those in `sample`.
    pub(crate) fn train(sample: &[&[u8]]) -> SymbolTable {
        // Codes while counting: 0..255 are table symbols, 256 + b the literal b.
        const CODES: usize = 512;
        let mut table = SymbolTable::empty();
        table.finish();
        let mut count1 = vec![0u32; CODES];
        let mut count2 = vec![0u32; CODES * CODES];
        for round in 0..ROUNDS {
            count1.fill(0);
            count2.fill(0);
            let pairs = round + 1 < ROUNDS;
            for s in sample.iter().filter(|s| !s.is_empty()) {
                let step = |pos: usize| {
                    let (c, l) = table.find(s, pos);
                    let code = if c == ESCAPE {
                        256 + s[pos] as usize
                    } else {
                        c as usize
                    };
                    (code, l)
                };
                let (mut prev, l) = step(0);
                count1[prev] += 1;
                if l > 1 {
                    // let a single byte that a longer symbol covers win back
                    // its own code if the longer symbol falls out
                    count1[256 + s[0] as usize] += 1;
                }
                let mut pos = l;
                while pos < s.len() {
                    let (code, l) = step(pos);
                    count1[code] += 1;
                    if l > 1 {
                        count1[256 + s[pos] as usize] += 1;
                    }
                    if pairs {
                        count2[prev * CODES + code] += 1;
                    }
                    prev = code;
                    pos += l;
                }
            }
            let symbol = |code: usize| -> (u64, usize) {
                if code >= 256 {
                    ((code - 256) as u64, 1)
                } else {
                    (table.sym[code], table.len[code] as usize)
                }
            };
            let mut gains: std::collections::HashMap<(u64, u8), u64> =
                std::collections::HashMap::new();
            for a in 0..CODES {
                let ca = count1[a];
                if ca < MIN_COUNT {
                    continue;
                }
                let (sa, la) = symbol(a);
                // promoting single bytes keeps escapes rare (as FSST does)
                let weight = if la == 1 { 8 } else { 1 };
                *gains.entry((sa, la as u8)).or_default() += u64::from(ca) * la as u64 * weight;
                if !pairs || la == 8 {
                    continue;
                }
                let row = &count2[a * CODES..(a + 1) * CODES];
                for (b, &cb) in row.iter().enumerate() {
                    if cb < MIN_COUNT {
                        continue;
                    }
                    let (sb, lb) = symbol(b);
                    let l = (la + lb).min(8);
                    let s = (sa | sb << (8 * la)) & mask(l);
                    *gains.entry((s, l as u8)).or_default() += u64::from(cb) * l as u64;
                }
            }
            let mut cands: Vec<((u64, u8), u64)> = gains.into_iter().collect();
            // by gain, then longer, then by bytes: the same table every time
            cands.sort_unstable_by(|a, b| {
                b.1.cmp(&a.1)
                    .then(b.0.1.cmp(&a.0.1))
                    .then(a.0.0.cmp(&b.0.0))
            });
            let mut next = SymbolTable::empty();
            for ((s, l), _) in cands {
                if next.count == MAX_SYMBOLS {
                    break;
                }
                next.add(s, l as usize);
            }
            next.finish();
            table = next;
        }
        table
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(t: &SymbolTable, s: &[u8]) -> Vec<u8> {
        let mut enc = Vec::new();
        t.encode(s, &mut enc);
        let n = t.decoded_len(&enc);
        assert_eq!(n, s.len());
        let mut out = vec![0u8; n + 7];
        assert_eq!(t.decode(&enc, &mut out), n);
        out.truncate(n);
        enc
    }

    fn sample() -> Vec<Vec<u8>> {
        let mut v = Vec::new();
        for i in 0..2000 {
            v.push(
                format!("amd64_microsoft-windows-part{i}_31bf3856ad364e35_10.0.22621.{i}_none")
                    .into_bytes(),
            );
            v.push(format!("libfoo-{i}.so.{}", i % 7).into_bytes());
            v.push(format!("IMG_{:04}.JPG", i).into_bytes());
            v.push(
                format!(
                    "{:016x}.cache",
                    (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
                )
                .into_bytes(),
            );
        }
        v
    }

    #[test]
    fn empty_table_escapes_everything() {
        let mut t = SymbolTable::empty();
        t.finish();
        let enc = roundtrip(&t, b"abc");
        assert_eq!(enc, [ESCAPE, b'a', ESCAPE, b'b', ESCAPE, b'c']);
        assert!(roundtrip(&t, b"").is_empty());
    }

    #[test]
    fn trained_table_compresses_and_roundtrips() {
        let s = sample();
        let refs: Vec<&[u8]> = s.iter().map(|v| &v[..]).collect();
        let t = SymbolTable::train(&refs);
        assert!(t.len() > 100 && t.len() <= MAX_SYMBOLS);
        let (mut raw, mut enc) = (0, 0);
        for v in &s {
            raw += v.len();
            enc += roundtrip(&t, v).len();
        }
        assert!(enc * 2 < raw, "{enc} of {raw} bytes");
        // strings unlike the sample, every byte value and every length
        let all: Vec<u8> = (0..=255u8).collect();
        for n in 0..all.len() {
            roundtrip(&t, &all[..n]);
            roundtrip(&t, &all[all.len() - n..]);
        }
        roundtrip(&t, "日本語のファイル名.txt".as_bytes());
        roundtrip(&t, &[0xED, 0xA0, 0x80, b'x']); // a lone surrogate in WTF-8
    }

    #[test]
    fn training_is_deterministic() {
        let s = sample();
        let refs: Vec<&[u8]> = s.iter().map(|v| &v[..]).collect();
        let (a, b) = (SymbolTable::train(&refs), SymbolTable::train(&refs));
        assert_eq!(a.sym, b.sym);
        assert_eq!(a.len, b.len);
    }

    #[test]
    fn equal_strings_encode_equally() {
        let s = sample();
        let refs: Vec<&[u8]> = s.iter().map(|v| &v[..]).collect();
        let t = SymbolTable::train(&refs);
        let (mut a, mut b) = (Vec::new(), Vec::new());
        t.encode(b"amd64_microsoft-windows-x.dll", &mut a);
        t.encode(b"amd64_microsoft-windows-x.dll", &mut b);
        assert_eq!(a, b);
    }
}
