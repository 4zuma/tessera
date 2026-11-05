//! Builders for synthetic NTFS records and volume images (tests only).
//!
//! They write the documented on-disk layout so that the parser can be
//! tested without a real volume; the image tests in `image_tests.rs` check
//! the same code against volumes made by `mkntfs`.

use super::parser::*;

fn put16(b: &mut [u8], off: usize, v: u16) {
    b[off..off + 2].copy_from_slice(&v.to_le_bytes());
}
fn put32(b: &mut [u8], off: usize, v: u32) {
    b[off..off + 4].copy_from_slice(&v.to_le_bytes());
}
fn put64(b: &mut [u8], off: usize, v: u64) {
    b[off..off + 8].copy_from_slice(&v.to_le_bytes());
}
fn align8(x: usize) -> usize {
    (x + 7) & !7
}
fn utf16le(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
}

/// A FILE record under construction.
pub(crate) struct RecordBuilder {
    size: usize,
    rec_no: u32,
    seq: u16,
    flags: u16,
    base: u64,
    attrs: Vec<Vec<u8>>,
}

impl RecordBuilder {
    pub fn new(size: usize, rec_no: u32, seq: u16) -> Self {
        RecordBuilder {
            size,
            rec_no,
            seq,
            flags: 1,
            base: 0,
            attrs: Vec::new(),
        }
    }

    pub fn dir(mut self) -> Self {
        self.flags |= 2;
        self
    }

    pub fn unused(mut self) -> Self {
        self.flags &= !1;
        self
    }

    pub fn base(mut self, rec: u64, seq: u16) -> Self {
        self.base = rec | (u64::from(seq) << 48);
        self
    }

    pub fn attr(mut self, a: Vec<u8>) -> Self {
        self.attrs.push(a);
        self
    }

    /// The record as stored on disk (update sequence protection applied).
    pub fn build(self) -> Vec<u8> {
        let mut r = vec![0u8; self.size];
        r[0..4].copy_from_slice(b"FILE");
        let usa_count = self.size / 512 + 1;
        put16(&mut r, 4, 0x30);
        put16(&mut r, 6, usa_count as u16);
        put16(&mut r, 0x10, self.seq);
        put16(&mut r, 0x12, 1);
        let first = align8(0x30 + 2 * usa_count);
        put16(&mut r, 0x14, first as u16);
        put16(&mut r, 0x16, self.flags);
        put64(&mut r, 0x20, self.base);
        put16(&mut r, 0x28, self.attrs.len() as u16);
        put32(&mut r, 0x2C, self.rec_no);
        put16(&mut r, 0x30, 0x0001);
        let mut off = first;
        for (i, a) in self.attrs.iter().enumerate() {
            assert!(off + a.len() + 8 <= self.size, "record overflow");
            r[off..off + a.len()].copy_from_slice(a);
            put16(&mut r, off + 0x0E, i as u16);
            off += a.len();
        }
        put32(&mut r, off, 0xFFFF_FFFF);
        put32(&mut r, 0x18, (off + 8) as u32);
        put32(&mut r, 0x1C, self.size as u32);
        protect(&mut r);
        r
    }
}

/// Applies update sequence protection (the inverse of `apply_fixups`),
/// bumping the update sequence number.
pub(crate) fn protect(r: &mut [u8]) {
    let usa_off = u16_at(r, 4).unwrap() as usize;
    let count = u16_at(r, 6).unwrap() as usize;
    let usn = u16_at(r, usa_off).unwrap().wrapping_add(1).max(1);
    put16(r, usa_off, usn);
    let stride = r.len() / (count - 1);
    for i in 1..count {
        let end = i * stride;
        let (a, b) = (r[end - 2], r[end - 1]);
        r[usa_off + 2 * i] = a;
        r[usa_off + 2 * i + 1] = b;
        put16(r, end - 2, usn);
    }
}

fn resident(type_code: u32, name: Option<&str>, value: &[u8]) -> Vec<u8> {
    let name = name.map(utf16le).unwrap_or_default();
    let value_off = align8(0x18 + name.len());
    let len = align8(value_off + value.len());
    let mut a = vec![0u8; len];
    put32(&mut a, 0, type_code);
    put32(&mut a, 4, len as u32);
    a[9] = (name.len() / 2) as u8;
    put16(&mut a, 0x0A, 0x18);
    a[0x18..0x18 + name.len()].copy_from_slice(&name);
    put32(&mut a, 0x10, value.len() as u32);
    put16(&mut a, 0x14, value_off as u16);
    a[value_off..value_off + value.len()].copy_from_slice(value);
    a
}

/// A non-resident attribute segment.
#[allow(clippy::too_many_arguments)]
pub(crate) fn non_resident(
    type_code: u32,
    name: Option<&str>,
    lowest_vcn: u64,
    highest_vcn: u64,
    sizes: (u64, u64, u64),
    total_allocated: Option<u64>,
    flags: u16,
    runlist: &[u8],
) -> Vec<u8> {
    let name = name.map(utf16le).unwrap_or_default();
    let header = if total_allocated.is_some() {
        0x48
    } else {
        0x40
    };
    let mp_off = align8(header + name.len());
    let len = align8(mp_off + runlist.len() + 1);
    let mut a = vec![0u8; len];
    put32(&mut a, 0, type_code);
    put32(&mut a, 4, len as u32);
    a[8] = 1;
    a[9] = (name.len() / 2) as u8;
    put16(&mut a, 0x0A, header as u16);
    put16(&mut a, 0x0C, flags);
    a[header..header + name.len()].copy_from_slice(&name);
    put64(&mut a, 0x10, lowest_vcn);
    put64(&mut a, 0x18, highest_vcn);
    put16(&mut a, 0x20, mp_off as u16);
    if flags & 0x00FF != 0 {
        a[0x22] = 4;
    }
    let (alloc, size, valid) = sizes;
    put64(&mut a, 0x28, alloc);
    put64(&mut a, 0x30, size);
    put64(&mut a, 0x38, valid);
    if let Some(t) = total_allocated {
        put64(&mut a, 0x40, t);
    }
    a[mp_off..mp_off + runlist.len()].copy_from_slice(runlist);
    a
}

/// `$STANDARD_INFORMATION` with the given file attributes.
pub(crate) fn si(file_attributes: u32) -> Vec<u8> {
    let mut v = vec![0u8; 0x48];
    put32(&mut v, 0x20, file_attributes);
    resident(AT_STANDARD_INFORMATION, None, &v)
}

/// `$FILE_NAME` in namespace `ns` (0 POSIX, 1 Win32, 2 DOS, 3 Win32+DOS).
pub(crate) fn file_name(parent: u64, parent_seq: u16, name: &str, ns: u8) -> Vec<u8> {
    file_name_ex(parent, parent_seq, name, ns, 0, 0)
}

pub(crate) fn file_name_ex(
    parent: u64,
    parent_seq: u16,
    name: &str,
    ns: u8,
    attrs: u32,
    reparse_tag: u32,
) -> Vec<u8> {
    let n = utf16le(name);
    let mut v = vec![0u8; 0x42 + n.len()];
    put64(&mut v, 0, parent | (u64::from(parent_seq) << 48));
    put32(&mut v, 0x38, attrs);
    put32(&mut v, 0x3C, reparse_tag);
    v[0x40] = (n.len() / 2) as u8;
    v[0x41] = ns;
    v[0x42..].copy_from_slice(&n);
    resident(AT_FILE_NAME, None, &v)
}

pub(crate) fn data_resident(name: Option<&str>, len: usize) -> Vec<u8> {
    resident(AT_DATA, name, &vec![0x5A; len])
}

/// A non-resident `$DATA` segment with a dummy runlist.
pub(crate) fn data_nonresident(
    name: Option<&str>,
    lowest_vcn: u64,
    alloc: u64,
    size: u64,
    total_allocated: Option<u64>,
    flags: u16,
) -> Vec<u8> {
    let clusters = alloc.div_ceil(4096).max(1);
    let runs = encode_runlist(&[(Some(100 + lowest_vcn), clusters)]);
    non_resident(
        AT_DATA,
        name,
        lowest_vcn,
        lowest_vcn + clusters - 1,
        (alloc, size, size),
        total_allocated,
        flags,
        &runs,
    )
}

pub(crate) fn reparse(tag: u32) -> Vec<u8> {
    let mut v = vec![0u8; 16];
    put32(&mut v, 0, tag);
    put16(&mut v, 4, 8);
    resident(AT_REPARSE_POINT, None, &v)
}

/// The value of an `$ATTRIBUTE_LIST`: (type, name, lowest VCN, record).
pub(crate) fn attribute_list(entries: &[(u32, Option<&str>, u64, u64)]) -> Vec<u8> {
    let mut out = Vec::new();
    for &(t, name, vcn, rec) in entries {
        let name = name.map(utf16le).unwrap_or_default();
        let len = align8(0x1A + name.len());
        let mut e = vec![0u8; len];
        put32(&mut e, 0, t);
        put16(&mut e, 4, len as u16);
        e[6] = (name.len() / 2) as u8;
        e[7] = 0x1A;
        put64(&mut e, 8, vcn);
        put64(&mut e, 0x10, rec);
        e[0x1A..0x1A + name.len()].copy_from_slice(&name);
        out.extend_from_slice(&e);
    }
    out
}

pub(crate) fn attribute_list_attr(value: &[u8]) -> Vec<u8> {
    resident(AT_ATTRIBUTE_LIST, None, value)
}

pub(crate) fn bitmap_resident(bits: &[u8]) -> Vec<u8> {
    resident(AT_BITMAP, None, bits)
}

/// Encodes runs `(lcn, clusters)` as mapping pairs (`None` = sparse).
pub(crate) fn encode_runlist(runs: &[(Option<u64>, u64)]) -> Vec<u8> {
    fn bytes_unsigned(v: u64) -> usize {
        (1..=8).find(|&n| n == 8 || v >> (8 * n) == 0).unwrap()
    }
    fn bytes_signed(v: i64) -> usize {
        (1..=8)
            .find(|&n| {
                let shift = 64 - 8 * n;
                n == 8 || (v << shift) >> shift == v
            })
            .unwrap()
    }
    let mut out = Vec::new();
    let mut prev: i64 = 0;
    for &(lcn, len) in runs {
        let ls = bytes_unsigned(len);
        let (os, delta) = match lcn {
            None => (0, 0),
            Some(l) => {
                let d = l as i64 - prev;
                prev = l as i64;
                (bytes_signed(d), d)
            }
        };
        out.push(((os as u8) << 4) | ls as u8);
        out.extend_from_slice(&len.to_le_bytes()[..ls]);
        out.extend_from_slice(&delta.to_le_bytes()[..os]);
    }
    out.push(0);
    out
}

/// An in-memory NTFS-like volume: a boot sector, an MFT laid out over the
/// given extents, and records placed at their offsets. Record 0 (`$MFT`)
/// is generated from the extents unless one is supplied.
pub(crate) struct SynthVolume {
    pub cluster: u64,
    pub record_size: usize,
    /// MFT extents as `(lcn, clusters)`.
    pub extents: Vec<(u64, u64)>,
    pub records: Vec<(u32, Vec<u8>)>,
    /// Number of records the MFT claims (data size / record size).
    pub mft_records: u64,
    /// Whether record 0 gets a `$BITMAP` matching the supplied records.
    pub with_bitmap: bool,
}

impl SynthVolume {
    pub fn new(
        cluster: u64,
        record_size: usize,
        extents: Vec<(u64, u64)>,
        mft_records: u64,
    ) -> Self {
        SynthVolume {
            cluster,
            record_size,
            extents,
            records: Vec::new(),
            mft_records,
            with_bitmap: true,
        }
    }

    pub fn add(&mut self, rec_no: u32, raw: Vec<u8>) {
        assert_eq!(raw.len(), self.record_size);
        self.records.push((rec_no, raw));
    }

    /// The `$MFT` record describing this volume's MFT.
    pub fn mft_record(&self) -> Vec<u8> {
        let runs: Vec<(Option<u64>, u64)> =
            self.extents.iter().map(|&(l, n)| (Some(l), n)).collect();
        let clusters: u64 = self.extents.iter().map(|e| e.1).sum();
        let alloc = clusters * self.cluster;
        let size = self.mft_records * self.record_size as u64;
        let mut b = RecordBuilder::new(self.record_size, 0, 1)
            .attr(si(0x6))
            .attr(file_name(5, 5, "$MFT", 3))
            .attr(non_resident(
                AT_DATA,
                None,
                0,
                clusters - 1,
                (alloc, size, size),
                None,
                0,
                &encode_runlist(&runs),
            ));
        if self.with_bitmap {
            let mut bits = vec![0u8; (self.mft_records as usize).div_ceil(8)];
            bits[0] |= 1;
            for (r, _) in &self.records {
                bits[*r as usize / 8] |= 1 << (r % 8);
            }
            b = b.attr(bitmap_resident(&bits));
        }
        b.build()
    }

    /// Byte offset on the volume of MFT byte `off`.
    pub fn locate(&self, off: u64) -> u64 {
        let mut vcn_bytes = 0;
        for &(lcn, n) in &self.extents {
            let len = n * self.cluster;
            if off < vcn_bytes + len {
                return lcn * self.cluster + (off - vcn_bytes);
            }
            vcn_bytes += len;
        }
        panic!("MFT offset {off} beyond the extents");
    }

    pub fn image(&self) -> Vec<u8> {
        let end = self
            .extents
            .iter()
            .map(|&(l, n)| (l + n) * self.cluster)
            .max()
            .unwrap()
            + 16 * self.cluster;
        let mut img = vec![0u8; end as usize];
        img[3..11].copy_from_slice(b"NTFS    ");
        put16(&mut img, 0x0B, 512);
        img[0x0D] = (self.cluster / 512) as u8;
        put64(&mut img, 0x28, end / 512 - 1);
        put64(&mut img, 0x30, self.extents[0].0);
        img[0x40] = if self.record_size as u64 >= self.cluster {
            (self.record_size as u64 / self.cluster) as u8
        } else {
            (-(self.record_size.trailing_zeros() as i8)) as u8
        };
        let mut recs = self.records.clone();
        if !recs.iter().any(|(r, _)| *r == 0) {
            recs.push((0, self.mft_record()));
        }
        for (r, raw) in recs {
            let mut done = 0;
            while done < raw.len() {
                let off = r as u64 * self.record_size as u64 + done as u64;
                let at = self.locate(off) as usize;
                // A record may straddle two extents when clusters are small.
                let room = (self.cluster - (off % self.cluster)) as usize;
                let n = room.min(raw.len() - done);
                img[at..at + n].copy_from_slice(&raw[done..done + n]);
                done += n;
            }
        }
        img
    }
}
