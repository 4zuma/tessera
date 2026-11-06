//! Parsing of NTFS file records (portable, no I/O).
//!
//! Written from the public NTFS on-disk format documentation and the
//! Windows SDK definitions of `FILE_RECORD_SEGMENT_HEADER`,
//! `ATTRIBUTE_RECORD_HEADER`, `FILE_NAME`, `STANDARD_INFORMATION` and
//! `ATTRIBUTE_LIST_ENTRY`. Every read is bounds checked: a corrupt or torn
//! record is skipped, never trusted and never a panic.
//!
//! [`parse_record`] reduces one record to a few compact facts (sizes, flags,
//! names) appended to a [`ParsedBatch`]; nothing else of the record is kept.

use crate::tree::NodeFlags;
use crate::wtf8;

// ---- Attribute type codes -------------------------------------------------

pub(crate) const AT_STANDARD_INFORMATION: u32 = 0x10;
pub(crate) const AT_ATTRIBUTE_LIST: u32 = 0x20;
pub(crate) const AT_FILE_NAME: u32 = 0x30;
pub(crate) const AT_DATA: u32 = 0x80;
pub(crate) const AT_BITMAP: u32 = 0xB0;
pub(crate) const AT_REPARSE_POINT: u32 = 0xC0;
const AT_END: u32 = 0xFFFF_FFFF;

// ---- File record header ---------------------------------------------------

/// `FILE_RECORD_SEGMENT_IN_USE`.
const FRS_IN_USE: u16 = 0x0001;
/// `FILE_FILE_NAME_INDEX_PRESENT`: the record is a directory.
const FRS_DIRECTORY: u16 = 0x0002;

/// Low 48 bits of a file reference: the record number.
pub(crate) const REF_RECORD_MASK: u64 = 0xFFFF_FFFF_FFFF;

// ---- Attribute record flags -------------------------------------------------

const ATTR_COMPRESSION_MASK: u16 = 0x00FF;
const ATTR_SPARSE: u16 = 0x8000;

// ---- File attributes (as in $STANDARD_INFORMATION / Win32) -----------------

const FILE_ATTRIBUTE_HIDDEN: u32 = 0x0000_0002;
const FILE_ATTRIBUTE_SYSTEM: u32 = 0x0000_0004;
const FILE_ATTRIBUTE_SPARSE_FILE: u32 = 0x0000_0200;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
const FILE_ATTRIBUTE_COMPRESSED: u32 = 0x0000_0800;
const FILE_ATTRIBUTE_OFFLINE: u32 = 0x0000_1000;
/// Shares its value with the internal `FILE_ATTRIBUTE_EA`, so it is only
/// trusted together with a reparse point (cloud placeholders have one).
const FILE_ATTRIBUTE_RECALL_ON_OPEN: u32 = 0x0004_0000;
const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;

// ---- $FILE_NAME namespaces ----------------------------------------------------

/// The 8.3 alias of a name that also has a separate Win32 name.
const NAMESPACE_DOS: u8 = 2;

// ---- Reparse tags ---------------------------------------------------------------

/// Bit 29 of a reparse tag: the reparse point is a name surrogate (it stands
/// for another named entity: junction, mount point, symbolic link).
const REPARSE_TAG_NAME_SURROGATE: u32 = 0x2000_0000;

fn is_cloud_tag(tag: u32) -> bool {
    // IO_REPARSE_TAG_CLOUD and IO_REPARSE_TAG_CLOUD_1 ..= _F.
    tag & 0xFFFF_0FFF == 0x9000_001A
}

// ---- Well-known records -------------------------------------------------------

/// The root directory.
pub(crate) const RECORD_ROOT: u32 = 5;
/// `$BadClus`: its `$Bad` stream spans the whole volume and is not real data.
pub(crate) const RECORD_BADCLUS: u32 = 8;
/// Records below this are reserved for NTFS metadata files.
pub(crate) const FIRST_NORMAL_RECORD: u32 = 16;

/// Name of the stream holding the data of WOF-compressed ("compact") files.
const WOF_STREAM: &str = "WofCompressedData";

// ---- Per-record flag bits produced by the parser ---------------------------------
//
// Bits 0..=9 have the same meaning as in [`NodeFlags`]; the parser only sets
// the ones listed in `NODE_FLAG_MASK`. Bits 10..=15 are internal.

pub(crate) const F_DIR: u16 = NodeFlags::DIR.0;
pub(crate) const F_CLOUD: u16 = NodeFlags::CLOUD.0;
pub(crate) const F_HIDDEN: u16 = NodeFlags::HIDDEN.0;
pub(crate) const F_SYSTEM: u16 = NodeFlags::SYSTEM.0;
pub(crate) const F_COMPRESSED: u16 = NodeFlags::COMPRESSED.0;
/// `$STANDARD_INFORMATION` says the file has a reparse point.
pub(crate) const F_SI_REPARSE: u16 = 1 << 10;
/// A reparse tag of the record is a name surrogate.
pub(crate) const F_TAG_SURROGATE: u16 = 1 << 11;
/// A reparse tag of the record is a cloud files tag.
pub(crate) const F_TAG_CLOUD: u16 = 1 << 12;
/// The record has at least one (non-DOS) name.
pub(crate) const F_HAS_NAME: u16 = 1 << 13;
/// The record has extra stream sizes in the side table.
pub(crate) const F_HAS_EXTRA: u16 = 1 << 14;
/// Set only by base records that are in use; marks `seq` as valid.
pub(crate) const F_IN_USE: u16 = 1 << 15;

/// Parser bits that are copied to the node as they are.
pub(crate) const NODE_FLAG_MASK: u16 = F_DIR | F_CLOUD | F_HIDDEN | F_SYSTEM | F_COMPRESSED;

const _: () = assert!(NodeFlags::METAFILE.0 < F_SI_REPARSE);

// ---- Output of the parser -------------------------------------------------------

/// Facts about one in-use record. Extension records report under their base
/// record's number (without `F_IN_USE`), so merging is a plain OR / add.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RecFacts {
    /// The base record these facts belong to.
    pub rec: u32,
    /// Sequence number (valid when `flags` has `F_IN_USE`).
    pub seq: u16,
    pub flags: u16,
    /// On-disk bytes of the unnamed `$DATA` stream.
    pub size: u64,
    /// Logical length of the unnamed `$DATA` stream.
    pub logical: u64,
}

/// One non-DOS `$FILE_NAME` of a record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NameFacts {
    /// The base record that has this name.
    pub rec: u32,
    /// Parent directory record (`u32::MAX` if out of range).
    pub parent: u32,
    pub parent_seq: u16,
    /// Length of the WTF-8 name in `ParsedBatch::name_bytes`.
    pub len: u16,
    /// Start of the WTF-8 name in `ParsedBatch::name_bytes`.
    pub start: u32,
}

/// Sizes of named streams that are counted in some cases (rare, so they
/// live in a side table instead of per record).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ExtraFacts {
    pub rec: u32,
    /// On-disk bytes of `WofCompressedData` (data of WOF-compressed files).
    pub wof_size: u64,
    /// Other non-resident named streams; counted for metadata files only
    /// (e.g. `$UsnJrnl:$J`, `$Secure:$SDS`).
    pub named_size: u64,
    pub named_logical: u64,
}

/// What a run of records parsed to.
#[derive(Default, Debug)]
pub(crate) struct ParsedBatch {
    pub recs: Vec<RecFacts>,
    pub names: Vec<NameFacts>,
    pub name_bytes: Vec<u8>,
    pub extras: Vec<ExtraFacts>,
    /// In-use records that could not be parsed (torn or corrupt).
    pub corrupt: u32,
}

impl ParsedBatch {
    pub fn with_capacity(records: usize) -> Self {
        ParsedBatch {
            recs: Vec::with_capacity(records),
            names: Vec::with_capacity(records + records / 8),
            name_bytes: Vec::with_capacity(records * 24),
            extras: Vec::new(),
            corrupt: 0,
        }
    }

    pub fn name(&self, n: &NameFacts) -> &[u8] {
        &self.name_bytes[n.start as usize..n.start as usize + n.len as usize]
    }
}

// ---- Little-endian readers ---------------------------------------------------------

#[inline]
pub(crate) fn u8_at(b: &[u8], off: usize) -> Option<u8> {
    b.get(off).copied()
}

#[inline]
pub(crate) fn u16_at(b: &[u8], off: usize) -> Option<u16> {
    let s = b.get(off..off.checked_add(2)?)?;
    Some(u16::from_le_bytes([s[0], s[1]]))
}

#[inline]
pub(crate) fn u32_at(b: &[u8], off: usize) -> Option<u32> {
    let s = b.get(off..off.checked_add(4)?)?;
    Some(u32::from_le_bytes(s.try_into().ok()?))
}

#[inline]
pub(crate) fn u64_at(b: &[u8], off: usize) -> Option<u64> {
    let s = b.get(off..off.checked_add(8)?)?;
    Some(u64::from_le_bytes(s.try_into().ok()?))
}

// ---- Multi-sector fixups ---------------------------------------------------------------

/// Undoes the update sequence protection of a multi-sector record (`FILE`
/// or `INDX`) in place. The last two bytes of every stride of the record
/// hold the update sequence number on disk; the real bytes are stored in
/// the update sequence array. Returns false if the record is torn or the
/// array is malformed.
pub(crate) fn apply_fixups(rec: &mut [u8]) -> bool {
    let (Some(usa_off), Some(usa_count)) = (u16_at(rec, 4), u16_at(rec, 6)) else {
        return false;
    };
    let (usa_off, usa_count) = (usa_off as usize, usa_count as usize);
    if usa_count < 2 || usa_off + 2 * usa_count > rec.len() {
        return false;
    }
    let strides = usa_count - 1;
    // The stride is 512 bytes in practice; derive it to stay general.
    let stride = rec.len() / strides;
    if stride * strides != rec.len() || stride < 256 || stride % 2 != 0 || usa_off < 8 {
        return false;
    }
    let usn = [rec[usa_off], rec[usa_off + 1]];
    for i in 1..=strides {
        let end = i * stride;
        if rec[end - 2..end] != usn {
            return false;
        }
        rec[end - 2] = rec[usa_off + 2 * i];
        rec[end - 1] = rec[usa_off + 2 * i + 1];
    }
    true
}

// ---- Record header and attributes --------------------------------------------------------

/// The fields of a file record header that the scanner needs.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RecordHeader {
    pub seq: u16,
    pub flags: u16,
    /// Base record reference (0 for a base record).
    pub base: u64,
    first_attr: usize,
    used: usize,
}

impl RecordHeader {
    #[inline]
    pub fn in_use(&self) -> bool {
        self.flags & FRS_IN_USE != 0
    }

    #[inline]
    pub fn is_dir(&self) -> bool {
        self.flags & FRS_DIRECTORY != 0
    }
}

/// Reads the header of a record whose fixups have been applied.
pub(crate) fn record_header(rec: &[u8]) -> Option<RecordHeader> {
    if rec.get(0..4)? != b"FILE" {
        return None;
    }
    let h = RecordHeader {
        seq: u16_at(rec, 0x10)?,
        first_attr: u16_at(rec, 0x14)? as usize,
        flags: u16_at(rec, 0x16)?,
        used: (u32_at(rec, 0x18)? as usize).min(rec.len()),
        base: u64_at(rec, 0x20)?,
    };
    if h.first_attr < 0x18 || h.first_attr >= h.used {
        return None;
    }
    Some(h)
}

/// One attribute record inside a file record.
#[derive(Clone, Copy)]
pub(crate) struct Attr<'a> {
    pub type_code: u32,
    pub flags: u16,
    raw: &'a [u8],
}

/// The header fields of a non-resident attribute.
#[derive(Clone, Copy, Debug)]
pub(crate) struct NonResident<'a> {
    pub lowest_vcn: u64,
    /// Only meaningful in the segment whose `lowest_vcn` is 0.
    pub allocated: u64,
    pub data_size: u64,
    pub valid_size: u64,
    /// Present for compressed and sparse attributes: clusters really allocated.
    pub total_allocated: Option<u64>,
    pub mapping_pairs: &'a [u8],
}

impl<'a> Attr<'a> {
    #[inline]
    pub fn is_resident(&self) -> bool {
        self.raw[8] == 0
    }

    /// The attribute name as raw UTF-16LE bytes (empty if unnamed).
    pub fn name_utf16le(&self) -> &'a [u8] {
        let len = self.raw[9] as usize * 2;
        if len == 0 {
            return &[];
        }
        let off = u16_at(self.raw, 0x0A).unwrap_or(0) as usize;
        self.raw.get(off..off + len).unwrap_or(&[])
    }

    #[inline]
    pub fn is_named(&self) -> bool {
        self.raw[9] != 0
    }

    /// The value of a resident attribute.
    pub fn resident_value(&self) -> Option<&'a [u8]> {
        if !self.is_resident() {
            return None;
        }
        let len = u32_at(self.raw, 0x10)? as usize;
        let off = u16_at(self.raw, 0x14)? as usize;
        self.raw.get(off..off.checked_add(len)?)
    }

    /// The length of a resident attribute's value.
    pub fn resident_len(&self) -> Option<u64> {
        if !self.is_resident() {
            return None;
        }
        u32_at(self.raw, 0x10).map(u64::from)
    }

    pub fn non_resident(&self) -> Option<NonResident<'a>> {
        if self.is_resident() {
            return None;
        }
        let mp_off = u16_at(self.raw, 0x20)? as usize;
        let compressed_form = self.flags & (ATTR_COMPRESSION_MASK | ATTR_SPARSE) != 0;
        let total_allocated = if compressed_form && mp_off >= 0x48 {
            u64_at(self.raw, 0x40)
        } else {
            None
        };
        Some(NonResident {
            lowest_vcn: u64_at(self.raw, 0x10)?,
            allocated: u64_at(self.raw, 0x28)?,
            data_size: u64_at(self.raw, 0x30)?,
            valid_size: u64_at(self.raw, 0x38)?,
            total_allocated,
            mapping_pairs: self.raw.get(mp_off..)?,
        })
    }

    /// True if the attribute is compressed or sparse.
    #[inline]
    pub fn is_compressed_or_sparse(&self) -> bool {
        self.flags & (ATTR_COMPRESSION_MASK | ATTR_SPARSE) != 0
    }
}

/// Iterator over the attributes of a fixed-up record.
pub(crate) struct Attrs<'a> {
    rec: &'a [u8],
    off: usize,
    end: usize,
}

pub(crate) fn attributes<'a>(rec: &'a [u8], h: &RecordHeader) -> Attrs<'a> {
    Attrs {
        rec,
        off: h.first_attr,
        end: h.used,
    }
}

impl<'a> Iterator for Attrs<'a> {
    type Item = Attr<'a>;

    #[inline]
    fn next(&mut self) -> Option<Attr<'a>> {
        let off = self.off;
        let type_code = u32_at(self.rec, off)?;
        if type_code == AT_END {
            return None;
        }
        let len = u32_at(self.rec, off + 4)? as usize;
        // 0x18 is the smallest (resident) header; anything shorter or
        // running past the used part of the record ends the walk.
        if len < 0x18 || off + len > self.end {
            self.off = self.end;
            return None;
        }
        let raw = &self.rec[off..off + len];
        let resident = raw[8] == 0;
        if !resident && len < 0x40 {
            self.off = self.end;
            return None;
        }
        self.off = off + len;
        Some(Attr {
            type_code,
            flags: u16_at(raw, 0x0C)?,
            raw,
        })
    }
}

// ---- Runlists ------------------------------------------------------------------------

/// One run of a non-resident attribute: `len` clusters starting at virtual
/// cluster `vcn`, stored at logical cluster `lcn` (`None` for a sparse run).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Run {
    pub vcn: u64,
    pub lcn: Option<u64>,
    pub len: u64,
}

/// Decodes the mapping pairs (runlist) of one attribute segment whose first
/// virtual cluster is `first_vcn`. Returns `None` if the runlist is malformed.
pub(crate) fn decode_runlist(bytes: &[u8], first_vcn: u64) -> Option<Vec<Run>> {
    let mut runs = Vec::new();
    let mut i = 0usize;
    let mut vcn = first_vcn;
    let mut lcn: i64 = 0;
    while let Some(&header) = bytes.get(i) {
        if header == 0 {
            break;
        }
        let len_size = (header & 0x0F) as usize;
        let off_size = (header >> 4) as usize;
        i += 1;
        if len_size == 0 || len_size > 8 || off_size > 8 || i + len_size + off_size > bytes.len() {
            return None;
        }
        let len = read_unsigned(&bytes[i..i + len_size]);
        i += len_size;
        if len == 0 {
            return None;
        }
        let run_lcn = if off_size == 0 {
            None
        } else {
            lcn = lcn.checked_add(read_signed(&bytes[i..i + off_size]))?;
            if lcn < 0 {
                return None;
            }
            Some(lcn as u64)
        };
        i += off_size;
        runs.push(Run {
            vcn,
            lcn: run_lcn,
            len,
        });
        vcn = vcn.checked_add(len)?;
    }
    Some(runs)
}

fn read_unsigned(b: &[u8]) -> u64 {
    b.iter()
        .rev()
        .fold(0u64, |acc, &x| (acc << 8) | u64::from(x))
}

fn read_signed(b: &[u8]) -> i64 {
    let v = read_unsigned(b);
    let bits = b.len() * 8;
    if bits == 64 {
        v as i64
    } else {
        // Sign-extend from the top bit of the last byte.
        let shift = 64 - bits;
        ((v << shift) as i64) >> shift
    }
}

// ---- Attribute lists ---------------------------------------------------------------

/// One entry of an `$ATTRIBUTE_LIST`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ListEntry {
    pub type_code: u32,
    pub named: bool,
    pub lowest_vcn: u64,
    /// The record holding this attribute segment.
    pub record: u64,
}

/// Parses the value of an `$ATTRIBUTE_LIST` attribute.
pub(crate) fn parse_attribute_list(mut v: &[u8]) -> Vec<ListEntry> {
    let mut out = Vec::new();
    while v.len() >= 0x1A {
        let (Some(type_code), Some(len)) = (u32_at(v, 0), u16_at(v, 4)) else {
            break;
        };
        let len = len as usize;
        if type_code == AT_END || len < 0x1A || len > v.len() {
            break;
        }
        out.push(ListEntry {
            type_code,
            named: v[6] != 0,
            lowest_vcn: u64_at(v, 8).unwrap_or(0),
            record: u64_at(v, 0x10).unwrap_or(0) & REF_RECORD_MASK,
        });
        v = &v[len..];
    }
    out
}

// ---- The record parser -----------------------------------------------------------------

/// Maps `$STANDARD_INFORMATION` file attributes to parser flag bits.
fn standard_info_flags(a: u32) -> u16 {
    let mut f = 0;
    if a & FILE_ATTRIBUTE_HIDDEN != 0 {
        f |= F_HIDDEN;
    }
    if a & FILE_ATTRIBUTE_SYSTEM != 0 {
        f |= F_SYSTEM;
    }
    if a & (FILE_ATTRIBUTE_COMPRESSED | FILE_ATTRIBUTE_SPARSE_FILE) != 0 {
        f |= F_COMPRESSED;
    }
    if a & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        f |= F_SI_REPARSE;
    }
    if a & (FILE_ATTRIBUTE_OFFLINE | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS) != 0
        || (a & FILE_ATTRIBUTE_RECALL_ON_OPEN != 0 && a & FILE_ATTRIBUTE_REPARSE_POINT != 0)
    {
        f |= F_CLOUD;
    }
    f
}

fn reparse_tag_flags(tag: u32) -> u16 {
    let mut f = 0;
    if tag & REPARSE_TAG_NAME_SURROGATE != 0 {
        f |= F_TAG_SURROGATE;
    }
    if is_cloud_tag(tag) {
        f |= F_TAG_CLOUD;
    }
    f
}

/// True if `name` (UTF-16LE bytes) equals the ASCII string `s`.
fn utf16le_eq(name: &[u8], s: &str) -> bool {
    name.len() == s.len() * 2
        && name
            .chunks_exact(2)
            .zip(s.bytes())
            .all(|(c, b)| c[0] == b && c[1] == 0)
}

/// Appends the UTF-16LE `name` to `out` as WTF-8.
fn push_name(out: &mut Vec<u8>, name: &[u8]) {
    // Fast path: pure ASCII (the vast majority of names).
    if name.chunks_exact(2).all(|c| c[1] == 0 && c[0] < 0x80) {
        out.extend(name.iter().step_by(2));
        return;
    }
    let mut units = [0u16; 255];
    let n = (name.len() / 2).min(units.len());
    for (u, c) in units.iter_mut().zip(name.chunks_exact(2)) {
        *u = u16::from_le_bytes([c[0], c[1]]);
    }
    wtf8::push_utf16(out, &units[..n]);
}

/// Parses MFT record number `rec_no` (raw on-disk bytes, exactly one record
/// long) and appends what the tree builder needs to `out`. Applies the
/// fixups in place. Unused and never-written records are skipped silently;
/// in-use records that fail validation are counted in `out.corrupt`.
pub(crate) fn parse_record(rec_no: u32, rec: &mut [u8], out: &mut ParsedBatch) {
    match rec.get(0..4) {
        Some(b"FILE") => {}
        Some(b"BAAD") => {
            out.corrupt += 1;
            return;
        }
        _ => return,
    }
    // The flags sit in the first stride, which fixups do not touch at that
    // offset, so unused records are skipped before any further work.
    match u16_at(rec, 0x16) {
        Some(flags) if flags & FRS_IN_USE != 0 => {}
        _ => return,
    }
    if !apply_fixups(rec) {
        out.corrupt += 1;
        return;
    }
    let Some(h) = record_header(rec) else {
        out.corrupt += 1;
        return;
    };
    // A base record has a zero base reference. Extension records of $MFT
    // itself refer to record 0 (with a non-zero sequence number).
    let is_base = h.base == 0;
    let target = if is_base {
        rec_no
    } else {
        match u32::try_from(h.base & REF_RECORD_MASK) {
            Ok(b) => b,
            Err(_) => {
                out.corrupt += 1;
                return;
            }
        }
    };

    let mut flags: u16 = 0;
    if is_base {
        flags |= F_IN_USE;
        if h.is_dir() {
            flags |= F_DIR;
        }
    }
    let (mut size, mut logical) = (0u64, 0u64);
    let mut extra = ExtraFacts {
        rec: target,
        ..ExtraFacts::default()
    };

    for attr in attributes(rec, &h) {
        match attr.type_code {
            AT_STANDARD_INFORMATION => {
                if let Some(v) = attr.resident_value() {
                    if let Some(a) = u32_at(v, 0x20) {
                        flags |= standard_info_flags(a);
                    }
                }
            }
            AT_FILE_NAME => {
                let Some(v) = attr.resident_value() else {
                    continue;
                };
                let (Some(parent), Some(fn_attrs), Some(tag), Some(nlen), Some(ns)) = (
                    u64_at(v, 0),
                    u32_at(v, 0x38),
                    u32_at(v, 0x3C),
                    u8_at(v, 0x40),
                    u8_at(v, 0x41),
                ) else {
                    continue;
                };
                if fn_attrs & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                    flags |= reparse_tag_flags(tag);
                }
                if ns == NAMESPACE_DOS || nlen == 0 {
                    continue;
                }
                let Some(name) = v.get(0x42..0x42 + 2 * nlen as usize) else {
                    continue;
                };
                let parent_rec = parent & REF_RECORD_MASK;
                let start = out.name_bytes.len();
                push_name(&mut out.name_bytes, name);
                out.names.push(NameFacts {
                    rec: target,
                    parent: u32::try_from(parent_rec).unwrap_or(u32::MAX),
                    parent_seq: (parent >> 48) as u16,
                    len: (out.name_bytes.len() - start) as u16,
                    start: start as u32,
                });
                flags |= F_HAS_NAME;
            }
            AT_DATA => {
                if attr.is_resident() {
                    // Resident data lives inside the MFT record: no clusters.
                    if !attr.is_named() {
                        logical += attr.resident_len().unwrap_or(0);
                    }
                    continue;
                }
                let Some(nr) = attr.non_resident() else {
                    continue;
                };
                // Only the first segment of a stream carries its sizes.
                if nr.lowest_vcn != 0 {
                    continue;
                }
                let packed = attr.is_compressed_or_sparse();
                let on_disk = if packed {
                    nr.total_allocated.unwrap_or(nr.allocated)
                } else {
                    nr.allocated
                };
                if !attr.is_named() {
                    if packed {
                        flags |= F_COMPRESSED;
                    }
                    size += on_disk;
                    logical += nr.data_size;
                } else if target != RECORD_BADCLUS {
                    if utf16le_eq(attr.name_utf16le(), WOF_STREAM) {
                        extra.wof_size += on_disk;
                        flags |= F_COMPRESSED;
                    } else {
                        extra.named_size += on_disk;
                        extra.named_logical += if packed { on_disk } else { nr.data_size };
                    }
                }
            }
            AT_REPARSE_POINT => {
                if let Some(tag) = attr.resident_value().and_then(|v| u32_at(v, 0)) {
                    flags |= reparse_tag_flags(tag);
                }
            }
            _ => {}
        }
    }

    if extra.wof_size != 0 || extra.named_size != 0 || extra.named_logical != 0 {
        flags |= F_HAS_EXTRA;
        out.extras.push(extra);
    }
    out.recs.push(RecFacts {
        rec: target,
        seq: h.seq,
        flags,
        size,
        logical,
    });
}

#[cfg(test)]
mod tests {
    use super::super::testutil::*;
    use super::*;

    #[test]
    fn fixups_round_trip_and_detect_tearing() {
        let mut r = RecordBuilder::new(1024, 40, 3)
            .attr(si(0))
            .attr(file_name(5, 5, "a.txt", 1))
            .build();
        // The builder stores the record as it is on disk (protected).
        let usa = u16_at(&r, 0x30).unwrap();
        assert_eq!(u16_at(&r, 510).unwrap(), usa);
        assert_eq!(u16_at(&r, 1022).unwrap(), usa);
        let mut torn = r.clone();
        assert!(apply_fixups(&mut r));
        assert!(record_header(&r).is_some());
        torn[1022] ^= 0xFF;
        assert!(!apply_fixups(&mut torn));
    }

    #[test]
    fn runlist_decoding() {
        // 0x21: 1-byte length, 2-byte offset. Then a negative delta and a
        // sparse run.
        let bytes = [
            0x21, 0x18, 0x34, 0x56, // 24 clusters at 0x5634
            0x11, 0x08, 0xF0, // 8 clusters at 0x5634 - 16
            0x01, 0x04, // 4 sparse clusters
            0x31, 0x02, 0x00, 0x00, 0x01, // 2 clusters at +0x10000
            0x00,
        ];
        let runs = decode_runlist(&bytes, 0).unwrap();
        assert_eq!(
            runs,
            vec![
                Run {
                    vcn: 0,
                    lcn: Some(0x5634),
                    len: 24
                },
                Run {
                    vcn: 24,
                    lcn: Some(0x5624),
                    len: 8
                },
                Run {
                    vcn: 32,
                    lcn: None,
                    len: 4
                },
                Run {
                    vcn: 36,
                    lcn: Some(0x15624),
                    len: 2
                },
            ]
        );
        assert!(decode_runlist(&[0x21, 0x18], 0).is_none(), "truncated");
        assert!(
            decode_runlist(&[0x11, 0x01, 0x80], 0).is_none(),
            "negative LCN"
        );
        assert_eq!(decode_runlist(&[0x00], 7).unwrap(), vec![]);
    }

    fn parse(rec_no: u32, mut r: Vec<u8>) -> ParsedBatch {
        let mut out = ParsedBatch::default();
        parse_record(rec_no, &mut r, &mut out);
        out
    }

    #[test]
    fn parses_names_sizes_and_flags() {
        let r = RecordBuilder::new(1024, 40, 7)
            .attr(si(FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM))
            .attr(file_name(30, 2, "LONGFI~1.TXT", NAMESPACE_DOS))
            .attr(file_name(30, 2, "Long file name.txt", 1))
            .attr(file_name(31, 4, "照片🦀.jpg", 0))
            .attr(data_nonresident(None, 0, 12288, 10_000, None, 0))
            .attr(data_resident(Some("Zone.Identifier"), 26))
            .build();
        let out = parse(40, r);
        assert_eq!(out.corrupt, 0);
        assert_eq!(
            out.recs,
            vec![RecFacts {
                rec: 40,
                seq: 7,
                flags: F_IN_USE | F_HIDDEN | F_SYSTEM | F_HAS_NAME,
                size: 12288,
                logical: 10_000,
            }]
        );
        let names: Vec<(&[u8], u32, u16)> = out
            .names
            .iter()
            .map(|n| (out.name(n), n.parent, n.parent_seq))
            .collect();
        assert_eq!(
            names,
            vec![
                (&b"Long file name.txt"[..], 30, 2),
                ("照片🦀.jpg".as_bytes(), 31, 4),
            ]
        );
        assert!(out.extras.is_empty());
    }

    #[test]
    fn resident_data_has_no_clusters_and_dirs_are_flagged() {
        let out = parse(
            41,
            RecordBuilder::new(1024, 41, 1)
                .attr(si(0))
                .attr(file_name(5, 5, "tiny", 3))
                .attr(data_resident(None, 100))
                .build(),
        );
        assert_eq!((out.recs[0].size, out.recs[0].logical), (0, 100));

        let out = parse(
            42,
            RecordBuilder::new(1024, 42, 1)
                .dir()
                .attr(si(0))
                .attr(file_name(5, 5, "dir", 1))
                .build(),
        );
        assert_eq!(out.recs[0].flags & F_DIR, F_DIR);
    }

    #[test]
    fn compressed_and_sparse_use_total_allocated() {
        let out = parse(
            43,
            RecordBuilder::new(1024, 43, 1)
                .attr(si(FILE_ATTRIBUTE_SPARSE_FILE))
                .attr(file_name(5, 5, "sparse", 1))
                .attr(data_nonresident(
                    None,
                    0,
                    1 << 30,
                    1 << 30,
                    Some(8192),
                    ATTR_SPARSE,
                ))
                .build(),
        );
        assert_eq!(out.recs[0].size, 8192);
        assert_eq!(out.recs[0].logical, 1 << 30);
        assert_ne!(out.recs[0].flags & F_COMPRESSED, 0);
    }

    #[test]
    fn extension_records_report_under_their_base() {
        // A later segment of $DATA (lowest VCN != 0) carries no sizes.
        let out = parse(
            90,
            RecordBuilder::new(1024, 90, 2)
                .base(44, 3)
                .attr(file_name(5, 5, "second link", 1))
                .attr(data_nonresident(None, 100, 0, 0, None, 0))
                .build(),
        );
        assert_eq!(out.recs.len(), 1);
        let f = out.recs[0];
        assert_eq!((f.rec, f.size, f.logical), (44, 0, 0));
        assert_eq!(f.flags & F_IN_USE, 0, "extension records never mark in-use");
        assert_eq!(out.names[0].rec, 44);
    }

    #[test]
    fn reparse_points_and_cloud_files() {
        let junction = parse(
            50,
            RecordBuilder::new(1024, 50, 1)
                .dir()
                .attr(si(FILE_ATTRIBUTE_REPARSE_POINT))
                .attr(file_name(5, 5, "junction", 1))
                .attr(reparse(0xA000_0003))
                .build(),
        );
        let f = junction.recs[0].flags;
        assert_ne!(f & F_TAG_SURROGATE, 0);
        assert_ne!(f & F_SI_REPARSE, 0);
        assert_eq!(f & F_TAG_CLOUD, 0);

        let placeholder = parse(
            51,
            RecordBuilder::new(1024, 51, 1)
                .attr(si(
                    FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS
                ))
                .attr(file_name(5, 5, "online-only.docx", 1))
                .attr(reparse(0x9000_101A))
                .build(),
        );
        let f = placeholder.recs[0].flags;
        assert_ne!(f & F_TAG_CLOUD, 0);
        assert_ne!(f & F_CLOUD, 0);
        assert_eq!(f & F_TAG_SURROGATE, 0);

        // 0x40000 alone is FILE_ATTRIBUTE_EA as far as we can tell.
        let ea = parse(
            52,
            RecordBuilder::new(1024, 52, 1)
                .attr(si(FILE_ATTRIBUTE_RECALL_ON_OPEN))
                .attr(file_name(5, 5, "wsl-file", 0))
                .build(),
        );
        assert_eq!(ea.recs[0].flags & F_CLOUD, 0);
    }

    #[test]
    fn named_streams_are_side_facts() {
        let out = parse(
            60,
            RecordBuilder::new(1024, 60, 1)
                .attr(si(0))
                .attr(file_name(5, 5, "compact.dll", 1))
                .attr(data_nonresident(None, 0, 0, 50_000, Some(0), ATTR_SPARSE))
                .attr(data_nonresident(
                    Some("WofCompressedData"),
                    0,
                    20480,
                    18_000,
                    None,
                    0,
                ))
                .attr(data_nonresident(Some("other"), 0, 4096, 4000, None, 0))
                .build(),
        );
        assert_eq!((out.recs[0].size, out.recs[0].logical), (0, 50_000));
        assert_ne!(out.recs[0].flags & F_HAS_EXTRA, 0);
        assert_eq!(
            out.extras,
            vec![ExtraFacts {
                rec: 60,
                wof_size: 20480,
                named_size: 4096,
                named_logical: 4000,
            }]
        );

        // $BadClus:$Bad spans the volume and must never count.
        let bad = parse(
            RECORD_BADCLUS,
            RecordBuilder::new(1024, RECORD_BADCLUS, 8)
                .attr(si(0))
                .attr(file_name(5, 5, "$BadClus", 3))
                .attr(data_resident(None, 0))
                .attr(data_nonresident(Some("$Bad"), 0, 1 << 40, 1 << 40, None, 0))
                .build(),
        );
        assert!(bad.extras.is_empty());
    }

    #[test]
    fn skips_unused_and_counts_corrupt() {
        let unused = RecordBuilder::new(1024, 70, 1).unused().build();
        let out = parse(70, unused);
        assert!(out.recs.is_empty() && out.corrupt == 0);

        let zero = vec![0u8; 1024];
        assert!(parse(71, zero).recs.is_empty());

        let mut torn = RecordBuilder::new(1024, 72, 1).attr(si(0)).build();
        torn[510] ^= 1;
        let out = parse(72, torn);
        assert!(out.recs.is_empty());
        assert_eq!(out.corrupt, 1);

        // An attribute running past the record must not panic.
        let mut bad = RecordBuilder::new(1024, 73, 1)
            .attr(si(0))
            .attr(file_name(5, 5, "x", 1))
            .build();
        assert!(apply_fixups(&mut bad));
        let first = u16_at(&bad, 0x14).unwrap() as usize;
        bad[first + 4..first + 8].copy_from_slice(&5000u32.to_le_bytes());
        protect(&mut bad);
        let out = parse(73, bad);
        assert_eq!(out.recs.len(), 1);
        assert!(out.names.is_empty());
    }

    #[test]
    fn attribute_list_entries() {
        let v = attribute_list(&[
            (AT_STANDARD_INFORMATION, None, 0, 0),
            (AT_DATA, None, 0, 0),
            (AT_DATA, None, 900, 33),
            (AT_BITMAP, None, 0, 0),
        ]);
        let e = parse_attribute_list(&v);
        assert_eq!(e.len(), 4);
        assert_eq!(
            e[2],
            ListEntry {
                type_code: AT_DATA,
                named: false,
                lowest_vcn: 900,
                record: 33
            }
        );
    }
}
