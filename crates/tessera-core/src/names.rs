//! The name pool: every node's name as WTF-8, compressed, in 1 MiB chunks.
//!
//! The first MiB of names is stored as it is. Once [`TRAIN_AT`] bytes are
//! in, a symbol table is trained on a sample of them (after FSST) and every
//! later name is stored encoded with it, which takes about a third less
//! room for typical file names. Scanners encode names on their own threads
//! ([`NamePool::encoder`]) so that the shared pool only appends them. A
//! node keeps the offset and stored length of its name; [`NamePool::get`]
//! hands back the plain bytes either way.

mod fsst;

use std::fmt;
use std::ops::Deref;
use std::sync::Arc;

use fsst::SymbolTable;

/// All names, concatenated in 1 MiB chunks. A name never straddles two
/// chunks; the few bytes left at the end of a full chunk are wasted.
///
/// While a scan runs, identical names are stored once (`__init__.py`,
/// `LICENSE`, `index.js`... repeat hundreds of thousands of times on a
/// typical system drive). The lookup table for that is only kept during the
/// scan ([`NamePool::end_dedup`] frees it); names added later are appended.
pub struct NamePool {
    chunks: Vec<Vec<u8>>,
    dedup: Option<Dedup>,
    codec: Codec,
    /// Plain length of the stored names, in bytes.
    raw_bytes: usize,
    /// Scratch space for encoding.
    buf: Vec<u8>,
}

/// Plain bytes stored before the symbol table is trained.
pub const TRAIN_AT: usize = 1 << 20;
/// About how many bytes of names the table is trained on.
const SAMPLE_BYTES: usize = 256 << 10;

enum Codec {
    /// Names are stored plain; `learned` lists them (offset, length) until
    /// `train_at` bytes are in.
    Plain {
        train_at: usize,
        learned: Vec<(u32, u16)>,
    },
    /// Names from offset `from` on are encoded with `table`.
    Encoded { from: u32, table: Arc<SymbolTable> },
}

/// Encodes names the way a [`NamePool`] stores them, so that threads can
/// do it ahead of [`NamePool::push_encoded`] (from [`NamePool::encoder`]).
#[derive(Clone)]
pub struct Encoder(Arc<SymbolTable>);

impl fmt::Debug for Encoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Encoder")
    }
}

impl Encoder {
    /// Appends the encoding of `name` to `out`.
    #[inline]
    pub fn encode(&self, name: &[u8], out: &mut Vec<u8>) {
        self.0.encode(name, out);
    }
}

const NAME_CHUNK_SHIFT: u32 = 20; // 1 MiB
const NAME_CHUNK: usize = 1 << NAME_CHUNK_SHIFT;

/// Open-addressing set of stored names. A slot packs the name's offset
/// (bits 0..32), its stored length + 1 (bits 32..48, 0 = empty slot) and 16
/// more bits of its hash (bits 48..64) to skip most comparisons.
struct Dedup {
    slots: Vec<u64>,
    used: usize,
}

impl Dedup {
    fn new() -> Self {
        Dedup {
            slots: vec![0; 1 << 16],
            used: 0,
        }
    }
}

#[inline]
fn name_hash(b: &[u8]) -> u64 {
    const K: u64 = 0x517c_c1b7_2722_0a95;
    let mut h = b.len() as u64;
    let mut words = b.chunks_exact(8);
    for w in &mut words {
        h = (h.rotate_left(5) ^ u64::from_le_bytes(w.try_into().unwrap())).wrapping_mul(K);
    }
    let rest = words.remainder();
    if !rest.is_empty() {
        let mut buf = [0u8; 8];
        buf[..rest.len()].copy_from_slice(rest);
        h = (h.rotate_left(5) ^ u64::from_le_bytes(buf)).wrapping_mul(K);
    }
    // Final avalanche so the low bits used for the slot index are well mixed.
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^ (h >> 33)
}

impl NamePool {
    /// A pool that appends every name (no de-duplication).
    pub fn new() -> Self {
        Self::with(None, TRAIN_AT)
    }

    /// A pool that stores identical names once until [`NamePool::end_dedup`].
    pub fn with_dedup() -> Self {
        Self::with(Some(Dedup::new()), TRAIN_AT)
    }

    /// A de-duplicating pool that trains its symbol table once `train_at`
    /// bytes are stored (tests use small values).
    pub fn with_dedup_training_at(train_at: usize) -> Self {
        Self::with(Some(Dedup::new()), train_at)
    }

    fn with(dedup: Option<Dedup>, train_at: usize) -> Self {
        NamePool {
            chunks: Vec::new(),
            dedup,
            codec: Codec::Plain {
                train_at,
                learned: Vec::new(),
            },
            raw_bytes: 0,
            buf: Vec::new(),
        }
    }

    /// Frees the de-duplication table (call when a scan ends). Names that
    /// are already shared stay shared.
    pub fn end_dedup(&mut self) {
        self.dedup = None;
    }

    /// An empty pool (without de-duplication) that encodes every name with
    /// a table trained on `sample`.
    pub(crate) fn trained_on(sample: &[&[u8]]) -> NamePool {
        NamePool {
            chunks: Vec::new(),
            dedup: None,
            codec: Codec::Encoded {
                from: 0,
                table: Arc::new(SymbolTable::train(sample)),
            },
            raw_bytes: 0,
            buf: Vec::new(),
        }
    }

    /// Appends a name that this pool's [`NamePool::encoder`] encoded;
    /// `raw_len` is its plain length. Returns what [`NamePool::push`] does.
    pub(crate) fn append_encoded(&mut self, raw_len: usize, encoded: &[u8]) -> (u32, u16) {
        debug_assert!(matches!(self.codec, Codec::Encoded { from: 0, .. }));
        self.raw_bytes += raw_len;
        let len = u16::try_from(encoded.len()).expect("name too long");
        (self.append(encoded), len)
    }

    /// How many bytes of names a symbol table is trained on, about.
    pub(crate) const SAMPLE_BYTES: usize = SAMPLE_BYTES;

    /// Stores `name` (or finds an identical stored copy) and returns its
    /// offset and stored length, which is what [`NamePool::get`] takes.
    ///
    /// # Panics
    /// If `name` is longer than `u16::MAX` bytes, or longer than that once
    /// encoded (only possible for names over 32 KiB, which no file system
    /// allows), or the pool exceeds 4 GiB.
    pub fn push(&mut self, name: &[u8]) -> (u32, u16) {
        assert!(name.len() <= u16::MAX as usize, "name too long");
        self.train_if_due();
        let Codec::Encoded { table, .. } = &self.codec else {
            return self.push_stored(name, name);
        };
        // the bytes as they will be stored
        let mut buf = std::mem::take(&mut self.buf);
        buf.clear();
        table.encode(name, &mut buf);
        let found = self.push_stored(name, &buf);
        self.buf = buf;
        found
    }

    /// The encoder, once the pool encodes names (after the first
    /// [`TRAIN_AT`] bytes).
    pub fn encoder(&self) -> Option<Encoder> {
        match &self.codec {
            Codec::Encoded { table, .. } => Some(Encoder(Arc::clone(table))),
            Codec::Plain { .. } => None,
        }
    }

    /// [`NamePool::push`] for a name that `by` already encoded. Falls back
    /// to [`NamePool::push`] if `by` is not this pool's encoder.
    pub fn push_encoded(&mut self, name: &[u8], encoded: &[u8], by: &Encoder) -> (u32, u16) {
        match &self.codec {
            Codec::Encoded { table, .. } if Arc::ptr_eq(table, &by.0) => {
                assert!(name.len() <= u16::MAX as usize, "name too long");
                self.push_stored(name, encoded)
            }
            _ => self.push(name),
        }
    }

    /// Stores a name whose stored form is `stored`, or finds it.
    fn push_stored(&mut self, name: &[u8], stored: &[u8]) -> (u32, u16) {
        match self.dedup.take() {
            None => self.store(name, stored),
            Some(mut d) => {
                let found = self.find_or_store(&mut d, name, stored);
                self.dedup = Some(d);
                found
            }
        }
    }

    /// The de-duplicating part of [`NamePool::push`]. Names are keyed by
    /// the hash of their encoded form, so finding one needs no decoding.
    fn find_or_store(&mut self, d: &mut Dedup, name: &[u8], stored: &[u8]) -> (u32, u16) {
        if (d.used + 1) * 4 > d.slots.len() * 3 {
            let n = d.slots.len() * 2;
            self.rehash(d, n);
        }
        let h = name_hash(stored);
        let mask = d.slots.len() - 1;
        let tag = h >> 48;
        let mut i = h as usize & mask;
        loop {
            let slot = d.slots[i];
            if slot == 0 {
                let (off, len) = self.store(name, stored);
                // a stored length of 65,535 does not fit the slot: keep it
                // out of the table
                if len < u16::MAX {
                    d.slots[i] = tag << 48 | (u64::from(len) + 1) << 32 | u64::from(off);
                    d.used += 1;
                }
                return (off, len);
            }
            if slot >> 48 == tag {
                let (off, len) = unpack(slot);
                let want = if self.is_plain(off) { name } else { stored };
                if self.stored(off, len) == want {
                    return (off, len);
                }
            }
            i = (i + 1) & mask;
        }
    }

    /// Whether the name at `off` is stored as it is.
    #[inline]
    fn is_plain(&self, off: u32) -> bool {
        match &self.codec {
            Codec::Plain { .. } => true,
            Codec::Encoded { from, .. } => off < *from,
        }
    }

    /// Trains the symbol table once enough plain names are in.
    fn train_if_due(&mut self) {
        let Codec::Plain { train_at, learned } = &mut self.codec else {
            return;
        };
        if self.raw_bytes < *train_at {
            return;
        }
        let learned = std::mem::take(learned);
        let every = self.raw_bytes.div_ceil(SAMPLE_BYTES).max(1);
        let sample: Vec<&[u8]> = learned
            .iter()
            .step_by(every)
            .map(|&(off, len)| self.stored(off, len))
            .collect();
        let table = Arc::new(SymbolTable::train(&sample));
        let from = match self.chunks.last() {
            Some(c) => ((self.chunks.len() - 1) << NAME_CHUNK_SHIFT) + c.len(),
            None => 0,
        };
        self.codec = Codec::Encoded {
            from: u32::try_from(from).expect("name pool exceeds 4 GiB"),
            table,
        };
        // the plain names are keyed by their encoded form from now on
        if let Some(mut d) = self.dedup.take() {
            let n = d.slots.len();
            self.rehash(&mut d, n);
            self.dedup = Some(d);
        }
    }

    /// Appends a name: `stored` is `name` as it is stored.
    fn store(&mut self, name: &[u8], stored: &[u8]) -> (u32, u16) {
        self.raw_bytes += name.len();
        let len = u16::try_from(stored.len()).expect("name too long");
        let off = self.append(stored);
        if let Codec::Plain { learned, .. } = &mut self.codec {
            learned.push((off, len));
        }
        (off, len)
    }

    /// Re-inserts every stored name into a table of `n` slots.
    fn rehash(&self, d: &mut Dedup, n: usize) {
        let mut slots = vec![0u64; n];
        let mask = n - 1;
        let mut buf = Vec::new();
        for &slot in d.slots.iter().filter(|&&s| s != 0) {
            let (off, len) = unpack(slot);
            let bytes = self.stored(off, len);
            let key = match &self.codec {
                Codec::Encoded { table, from } if off < *from => {
                    buf.clear();
                    table.encode(bytes, &mut buf);
                    &buf[..]
                }
                _ => bytes,
            };
            let h = name_hash(key);
            let mut i = h as usize & mask;
            while slots[i] != 0 {
                i = (i + 1) & mask;
            }
            slots[i] = (h >> 48) << 48 | (slot & 0xFFFF_FFFF_FFFF);
        }
        d.slots = slots;
    }

    fn append(&mut self, bytes: &[u8]) -> u32 {
        // A full chunk takes nothing more, not even an empty name: its
        // offset would point at the next chunk.
        let need_new = match self.chunks.last() {
            Some(c) => c.len() + bytes.len().max(1) > NAME_CHUNK,
            None => true,
        };
        if need_new {
            self.chunks.push(Vec::with_capacity(NAME_CHUNK));
        }
        let ci = self.chunks.len() - 1;
        let chunk = &mut self.chunks[ci];
        let off = (ci << NAME_CHUNK_SHIFT) + chunk.len();
        chunk.extend_from_slice(bytes);
        u32::try_from(off).expect("name pool exceeds 4 GiB")
    }

    /// The bytes as stored (encoded or not).
    #[inline]
    fn stored(&self, off: u32, len: u16) -> &[u8] {
        let off = off as usize;
        let c = &self.chunks[off >> NAME_CHUNK_SHIFT];
        let start = off & (NAME_CHUNK - 1);
        &c[start..start + len as usize]
    }

    /// The name at `off` with stored length `len` (as [`NamePool::push`]
    /// returned them).
    #[inline]
    pub fn get(&self, off: u32, len: u16) -> Name<'_> {
        let bytes = self.stored(off, len);
        match &self.codec {
            Codec::Encoded { from, table } if off >= *from => Name::decode(table, bytes),
            _ => Name(Repr::Borrowed(bytes)),
        }
    }

    /// Heap bytes held, including the de-duplication table while it exists.
    pub fn heap_bytes(&self) -> usize {
        let codec = match &self.codec {
            Codec::Plain { learned, .. } => learned.capacity() * 8,
            Codec::Encoded { table, .. } => table.heap_bytes() + std::mem::size_of::<SymbolTable>(),
        };
        self.chunks.iter().map(|c| c.capacity()).sum::<usize>()
            + self.chunks.capacity() * std::mem::size_of::<Vec<u8>>()
            + self.dedup.as_ref().map_or(0, |d| d.slots.capacity() * 8)
            + self.buf.capacity()
            + codec
    }

    /// Bytes of name data actually stored (excluding unused chunk tails).
    pub fn stored_bytes(&self) -> usize {
        self.chunks.iter().map(|c| c.len()).sum()
    }

    /// Plain length of all stored names (what [`NamePool::stored_bytes`]
    /// would be without compression).
    pub fn raw_bytes(&self) -> usize {
        self.raw_bytes
    }
}

#[inline]
fn unpack(slot: u64) -> (u32, u16) {
    (slot as u32, ((slot >> 32) & 0xFFFF) as u16 - 1)
}

impl Default for NamePool {
    fn default() -> Self {
        Self::new()
    }
}

/// Room for a decoded name kept on the stack; longer ones go to the heap.
const INLINE: usize = 256;
/// [`SymbolTable::decode`] writes up to 7 bytes past the end.
const SLACK: usize = 7;

/// A name read from a [`NamePool`], as WTF-8: borrowed when stored plain,
/// else decoded into a small buffer. Use it as a `&[u8]`.
pub struct Name<'a>(Repr<'a>);

// The inline buffer is the point: reading a name never allocates.
#[allow(clippy::large_enum_variant)]
enum Repr<'a> {
    Borrowed(&'a [u8]),
    Inline { len: u16, buf: [u8; INLINE + SLACK] },
    Heap(Vec<u8>),
}

impl Name<'_> {
    fn decode(table: &SymbolTable, codes: &[u8]) -> Name<'static> {
        // a code stands for at most 8 bytes
        let fits = codes.len() * 8 <= INLINE || table.decoded_len(codes) <= INLINE;
        if fits {
            let mut buf = [0u8; INLINE + SLACK];
            let len = table.decode(codes, &mut buf) as u16;
            Name(Repr::Inline { len, buf })
        } else {
            let n = table.decoded_len(codes);
            let mut v = vec![0u8; n + SLACK];
            table.decode(codes, &mut v);
            v.truncate(n);
            Name(Repr::Heap(v))
        }
    }
}

impl Deref for Name<'_> {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &[u8] {
        match &self.0 {
            Repr::Borrowed(b) => b,
            Repr::Inline { len, buf } => &buf[..*len as usize],
            Repr::Heap(v) => v,
        }
    }
}

impl AsRef<[u8]> for Name<'_> {
    fn as_ref(&self) -> &[u8] {
        self
    }
}

impl fmt::Debug for Name<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&crate::wtf8::to_string_lossy(self), f)
    }
}

impl PartialEq for Name<'_> {
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}

impl Eq for Name<'_> {}

impl PartialEq<[u8]> for Name<'_> {
    fn eq(&self, other: &[u8]) -> bool {
        **self == *other
    }
}

impl PartialEq<&[u8]> for Name<'_> {
    fn eq(&self, other: &&[u8]) -> bool {
        **self == **other
    }
}

impl<const N: usize> PartialEq<[u8; N]> for Name<'_> {
    fn eq(&self, other: &[u8; N]) -> bool {
        **self == other[..]
    }
}

impl<const N: usize> PartialEq<&[u8; N]> for Name<'_> {
    fn eq(&self, other: &&[u8; N]) -> bool {
        **self == other[..]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedups_while_scanning() {
        let mut p = NamePool::with_dedup();
        let a = p.push(b"__init__.py");
        let b = p.push(b"LICENSE");
        assert_eq!(p.push(b"__init__.py"), a);
        assert_ne!(p.push(b"license"), b, "names are case sensitive here");
        assert_eq!(p.push(b""), p.push(b""));
        // Enough distinct names to force several table growths.
        let refs: Vec<(u32, u16)> = (0..200_000)
            .map(|i| p.push(format!("f{i}.txt").as_bytes()))
            .collect();
        for (i, &r) in refs.iter().enumerate().step_by(997) {
            let n = format!("f{i}.txt");
            assert_eq!(p.push(n.as_bytes()), r);
            assert_eq!(p.get(r.0, r.1), n.as_bytes());
        }
        let stored = p.stored_bytes();
        p.end_dedup();
        let again = p.push(b"LICENSE");
        assert_ne!(again, b, "after the scan, names are appended");
        assert_eq!(p.stored_bytes(), stored + again.1 as usize);
        assert_eq!(p.get(again.0, again.1), b"LICENSE");
    }

    #[test]
    fn does_not_straddle_chunks() {
        let mut p = NamePool::with(None, usize::MAX);
        let long = vec![b'x'; 60_000];
        let mut refs = Vec::new();
        for _ in 0..40 {
            refs.push(p.push(&long));
        }
        for &(off, len) in &refs {
            assert_eq!(p.get(off, len).len(), 60_000);
        }
        assert!(p.heap_bytes() >= 40 * 60_000);
    }

    #[test]
    fn empty_name_after_a_full_chunk() {
        let mut p = NamePool::with(None, usize::MAX);
        let fill = vec![b'x'; 1 << 15];
        for _ in 0..(NAME_CHUNK >> 15) {
            p.push(&fill);
        }
        let (off, len) = p.push(b"");
        assert_eq!(p.get(off, len).len(), 0);
        let (off, len) = p.push(b"next");
        assert_eq!(p.get(off, len), b"next");
    }

    fn file_name(i: u32) -> String {
        match i % 5 {
            0 => format!("amd64_microsoft-windows-part{i}_31bf3856ad364e35_10.0.22621.{i}_none"),
            1 => format!("IMG_{i:05}.JPG"),
            2 => format!("node_modules-{}-index.js", i % 300),
            3 => format!("{:016x}", u64::from(i).wrapping_mul(0x9E37_79B9_7F4A_7C15)),
            _ => format!("Документ {i}.docx"),
        }
    }

    #[test]
    fn compresses_after_training_and_reads_back() {
        let mut p = NamePool::with_dedup_training_at(64 << 10);
        let mut refs = Vec::new();
        for i in 0..60_000 {
            refs.push(p.push(file_name(i).as_bytes()));
        }
        assert!(matches!(p.codec, Codec::Encoded { .. }));
        for (i, &(off, len)) in refs.iter().enumerate() {
            let want = file_name(i as u32);
            assert_eq!(p.get(off, len), want.as_bytes(), "name {i}");
            // the same name again finds the stored copy, plain or encoded
            assert_eq!(p.push(want.as_bytes()), (off, len));
        }
        assert!(
            p.stored_bytes() * 10 < p.raw_bytes() * 7,
            "{} of {}",
            p.stored_bytes(),
            p.raw_bytes()
        );
        // names longer than the inline buffer, and every byte value
        let long = "長い名前".repeat(40);
        let r = p.push(long.as_bytes());
        assert!(long.len() > INLINE);
        assert_eq!(p.get(r.0, r.1), long.as_bytes());
        let bytes: Vec<u8> = (0..=255u8).collect();
        let r = p.push(&bytes);
        assert_eq!(p.get(r.0, r.1), &bytes[..]);
        let r = p.push(b"");
        assert_eq!(p.get(r.0, r.1), b"");
        // after the scan, new names are still encoded
        p.end_dedup();
        let r = p.push(b"amd64_microsoft-windows-new_31bf3856ad364e35_none");
        assert!(r.1 < 30);
        assert_eq!(
            p.get(r.0, r.1),
            b"amd64_microsoft-windows-new_31bf3856ad364e35_none"
        );
    }
}
