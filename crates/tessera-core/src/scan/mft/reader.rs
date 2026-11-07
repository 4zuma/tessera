//! Locating the MFT on a volume and streaming it through the parser
//! (portable: works on any [`ReadAt`] source, e.g. a raw image file).
//!
//! The MFT is never loaded whole. A reader thread reads it in large
//! cluster-aligned chunks into a few recycled buffers; the calling thread
//! parses each chunk in parallel (rayon) while the next one is being read,
//! then merges the compact results in record order.

use std::alloc::Layout;
use std::io::{self, Read, Seek, SeekFrom};
use std::ops::{Deref, DerefMut};
use std::ptr::NonNull;
use std::sync::mpsc;

use rayon::prelude::*;

use super::build::Accum;
use super::parser::*;
use crate::scan::ScanProgress;
use crate::tree::Encoder;
use crate::{Error, Result};

/// Positioned reads of a raw volume (or volume image).
pub(crate) trait ReadAt: Send {
    /// Fills `buf` from byte `offset`. Offsets and lengths are multiples
    /// of the sector size and `buf` is sector aligned, as unbuffered volume
    /// handles require.
    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()>;
}

impl<T: Read + Seek + Send> ReadAt for T {
    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        self.seek(SeekFrom::Start(offset))?;
        self.read_exact(buf)
    }
}

/// How the MFT is streamed. The defaults suit an NVMe system drive.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Tuning {
    /// Bytes per read (and per parallel parse round).
    pub chunk_bytes: usize,
    /// Free stretches of the MFT (per its `$BITMAP`) at least this long
    /// are not read; shorter ones are read through to keep reads large.
    pub skip_gap_bytes: usize,
    /// Use the MFT's `$BITMAP` to skip unused records.
    pub use_bitmap: bool,
}

impl Default for Tuning {
    fn default() -> Self {
        Tuning {
            chunk_bytes: 8 << 20,
            skip_gap_bytes: 1 << 20,
            use_bitmap: true,
        }
    }
}

/// Records per parallel parse task (256 KiB with 1 KiB records).
const BLOCK_RECORDS: usize = 256;
/// Filled chunks queued between the reader thread and the parser.
const QUEUE_DEPTH: usize = 2;
/// Minimum buffer alignment (unbuffered I/O needs sector alignment).
const IO_ALIGN: usize = 4096;

/// Volume layout parameters needed to find and parse the MFT.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Geometry {
    pub sector_size: u32,
    pub cluster_size: u32,
    pub record_size: u32,
    pub mft_lcn: u64,
    /// Bytes of the MFT in use, when the OS reports it.
    pub mft_valid_len: Option<u64>,
}

impl Geometry {
    /// Reads the NTFS boot sector (BIOS parameter block).
    pub fn from_boot_sector(b: &[u8]) -> Result<Geometry> {
        if b.get(3..11) != Some(b"NTFS    ") {
            return Err(Error::Unsupported("not an NTFS volume"));
        }
        let bad = || Error::Corrupt("NTFS boot sector".into());
        let sector = u32::from(u16_at(b, 0x0B).ok_or_else(bad)?);
        let spc = u32::from(u8_at(b, 0x0D).ok_or_else(bad)?);
        // Values above 0x80 encode 2^(256 - value) sectors (large clusters).
        let spc = if spc <= 0x80 {
            spc
        } else {
            1u32.checked_shl(256 - spc).ok_or_else(bad)?
        };
        let cluster = sector.checked_mul(spc).ok_or_else(bad)?;
        let per_record = u8_at(b, 0x40).ok_or_else(bad)? as i8;
        let record = if per_record > 0 {
            (per_record as u32).checked_mul(cluster).ok_or_else(bad)?
        } else {
            1u32.checked_shl(per_record.unsigned_abs() as u32)
                .ok_or_else(bad)?
        };
        Geometry {
            sector_size: sector,
            cluster_size: cluster,
            record_size: record,
            mft_lcn: u64_at(b, 0x30).ok_or_else(bad)?,
            mft_valid_len: None,
        }
        .validate()
    }

    pub fn validate(self) -> Result<Geometry> {
        let pow2_in = |v: u32, lo: u32, hi: u32| v.is_power_of_two() && (lo..=hi).contains(&v);
        if pow2_in(self.sector_size, 256, 64 << 10)
            && pow2_in(self.cluster_size, self.sector_size, 64 << 20)
            && pow2_in(self.record_size, 512, 64 << 10)
            && self.mft_lcn > 0
        {
            Ok(self)
        } else {
            Err(Error::Corrupt(format!("unusable NTFS geometry {self:?}")))
        }
    }

    fn io_align(&self) -> usize {
        IO_ALIGN.max(self.sector_size as usize)
    }
}

/// A heap buffer with a chosen alignment, for unbuffered reads.
pub(crate) struct AlignedBuf {
    ptr: NonNull<u8>,
    len: usize,
    layout: Layout,
}

// SAFETY: the buffer exclusively owns its allocation, like a Vec<u8>.
unsafe impl Send for AlignedBuf {}
unsafe impl Sync for AlignedBuf {}

impl AlignedBuf {
    pub fn new(len: usize, align: usize) -> AlignedBuf {
        let layout = Layout::from_size_align(len.max(1), align).expect("buffer layout");
        // SAFETY: the layout has a non-zero size.
        let ptr = unsafe { std::alloc::alloc_zeroed(layout) };
        let ptr = NonNull::new(ptr).unwrap_or_else(|| std::alloc::handle_alloc_error(layout));
        AlignedBuf { ptr, len, layout }
    }
}

impl Deref for AlignedBuf {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        // SAFETY: ptr is valid for len initialized (zeroed) bytes.
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }
}

impl DerefMut for AlignedBuf {
    fn deref_mut(&mut self) -> &mut [u8] {
        // SAFETY: as above, and we have exclusive access.
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }
}

impl Drop for AlignedBuf {
    fn drop(&mut self) {
        // SAFETY: allocated in `new` with this layout.
        unsafe { std::alloc::dealloc(self.ptr.as_ptr(), self.layout) }
    }
}

fn round_up(x: u64, to: u64) -> u64 {
    x.div_ceil(to) * to
}

/// Reads bytes `[start, start + len)` of a non-resident stream described by
/// `runs` (sparse runs read as zeros).
fn read_stream<R: ReadAt>(
    src: &mut R,
    runs: &[Run],
    g: &Geometry,
    start: u64,
    len: usize,
) -> Result<Vec<u8>> {
    let cs = u64::from(g.cluster_size);
    let first = start / cs * cs;
    let end = round_up(start + len as u64, cs);
    let mut buf = AlignedBuf::new((end - first) as usize, g.io_align());
    let mut off = first;
    while off < end {
        let vcn = off / cs;
        let run = runs
            .iter()
            .find(|r| r.vcn <= vcn && vcn < r.vcn + r.len)
            .ok_or_else(|| Error::Corrupt(format!("stream offset {off} is not mapped")))?;
        let run_end = (run.vcn + run.len) * cs;
        let n = (run_end.min(end) - off) as usize;
        let dst = &mut buf[(off - first) as usize..(off - first) as usize + n];
        match run.lcn {
            Some(lcn) => src.read_exact_at(lcn * cs + (off - run.vcn * cs), dst)?,
            None => dst.fill(0),
        }
        off += n as u64;
    }
    let s = (start - first) as usize;
    Ok(buf[s..s + len].to_vec())
}

/// (allocated, data size, valid size) of a non-resident attribute.
type Sizes = (u64, u64, u64);

/// One segment of a non-resident attribute of `$MFT`.
struct Segment {
    lowest_vcn: u64,
    runs: Vec<Run>,
    /// Only in the first segment.
    sizes: Option<Sizes>,
}

#[derive(Default)]
struct MftAttrs {
    data: Vec<Segment>,
    bitmap: Vec<Segment>,
    bitmap_resident: Option<Vec<u8>>,
    list_resident: Option<Vec<u8>>,
    list_runs: Option<(Vec<Run>, u64)>,
}

fn collect_mft_attrs(rec: &[u8], h: &RecordHeader, out: &mut MftAttrs) -> Result<()> {
    for a in attributes(rec, h) {
        if a.is_named() {
            continue;
        }
        let segment = |a: &Attr| -> Result<Option<Segment>> {
            let Some(nr) = a.non_resident() else {
                return Ok(None);
            };
            let runs = decode_runlist(nr.mapping_pairs, nr.lowest_vcn)
                .ok_or_else(|| Error::Corrupt("$MFT runlist".into()))?;
            Ok(Some(Segment {
                lowest_vcn: nr.lowest_vcn,
                runs,
                sizes: (nr.lowest_vcn == 0).then_some((nr.allocated, nr.data_size, nr.valid_size)),
            }))
        };
        match a.type_code {
            AT_DATA => out.data.extend(segment(&a)?),
            AT_BITMAP => match a.resident_value() {
                Some(v) => out.bitmap_resident = Some(v.to_vec()),
                None => out.bitmap.extend(segment(&a)?),
            },
            AT_ATTRIBUTE_LIST => match a.resident_value() {
                Some(v) => out.list_resident = Some(v.to_vec()),
                None => {
                    if let Some(s) = segment(&a)? {
                        out.list_runs = Some((s.runs, s.sizes.map_or(0, |z| z.1)));
                    }
                }
            },
            _ => {}
        }
    }
    Ok(())
}

/// Applies fixups to a copy of a `$MFT` record and checks it.
fn checked_record(raw: &[u8], what: &str) -> Result<(Vec<u8>, RecordHeader)> {
    let mut rec = raw.to_vec();
    if rec.get(0..4) != Some(b"FILE") || !apply_fixups(&mut rec) {
        return Err(Error::Corrupt(format!("{what} is not a valid file record")));
    }
    let h = record_header(&rec)
        .filter(|h| h.in_use())
        .ok_or_else(|| Error::Corrupt(format!("{what} has a bad header")))?;
    Ok((rec, h))
}

/// Sorts segments by VCN and joins their runs; fails on gaps or overlaps.
fn join_segments(mut segs: Vec<Segment>, what: &str) -> Result<(Vec<Run>, Option<Sizes>)> {
    segs.sort_by_key(|s| s.lowest_vcn);
    let sizes = segs.first().and_then(|s| s.sizes);
    let mut runs: Vec<Run> = Vec::new();
    for s in segs {
        let next = runs.last().map_or(0, |r| r.vcn + r.len);
        if s.lowest_vcn != next {
            return Err(Error::Corrupt(format!(
                "{what} segments are not contiguous"
            )));
        }
        runs.extend(s.runs);
    }
    Ok((runs, sizes))
}

/// Where the MFT lives and which of its records are in use.
pub(crate) struct MftLayout {
    pub record_size: usize,
    pub cluster_size: u64,
    pub align: usize,
    /// The extents of `$MFT:$DATA`, in VCN order.
    pub runs: Vec<Run>,
    /// Records to scan.
    pub records: u64,
    /// `$MFT:$BITMAP`: bit n set = record n in use.
    pub bitmap: Option<Vec<u8>>,
}

/// One read of a chunk: `len` bytes into the chunk buffer at `buf_off`,
/// from `disk_off` (`None`: zero fill, for sparse or unmapped ranges).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Piece {
    pub buf_off: usize,
    pub disk_off: Option<u64>,
    pub len: usize,
}

impl MftLayout {
    #[inline]
    pub fn in_use(&self, rec: u64) -> bool {
        match &self.bitmap {
            None => true,
            // Records past the bitmap's end: read them to be safe.
            Some(b) => b
                .get((rec / 8) as usize)
                .is_none_or(|&x| (x >> (rec % 8)) & 1 != 0),
        }
    }

    /// Records in use according to the bitmap (all if there is none).
    pub fn in_use_count(&self) -> u64 {
        match &self.bitmap {
            None => self.records,
            Some(b) => {
                let full = (self.records / 8) as usize;
                let mut n: u64 = b.iter().take(full).map(|x| u64::from(x.count_ones())).sum();
                for r in (full as u64 * 8)..self.records {
                    n += u64::from(self.in_use(r));
                }
                n
            }
        }
    }

    /// The reads that fetch records `[r0, r1)` into a chunk buffer whose
    /// byte 0 is record `r0`. Free stretches of at least `gap` bytes are
    /// left out; reads are whole clusters.
    pub fn chunk_pieces(&self, r0: u64, r1: u64, gap: usize) -> Vec<Piece> {
        let rs = self.record_size as u64;
        let cs = self.cluster_size;
        let gap_records = (gap as u64).max(cs).div_ceil(rs).max(1);
        let base = r0 * rs;

        // In-use record ranges, merging gaps shorter than `gap_records`.
        let mut ranges: Vec<(u64, u64)> = Vec::new();
        match &self.bitmap {
            None => ranges.push((r0, r1)),
            Some(bits) => {
                let mut r = r0;
                while r < r1 {
                    if !self.in_use(r) {
                        // Skip whole free bytes of the bitmap quickly.
                        let free_byte = bits.get((r / 8) as usize) == Some(&0);
                        r += if r % 8 == 0 && r + 8 <= r1 && free_byte {
                            8
                        } else {
                            1
                        };
                        continue;
                    }
                    match ranges.last_mut() {
                        Some(last) if r - last.1 < gap_records => last.1 = r + 1,
                        _ => ranges.push((r, r + 1)),
                    }
                    r += 1;
                }
            }
        }

        let mut pieces = Vec::new();
        for (a, b) in ranges {
            let mut off = (a * rs / cs * cs).max(base);
            let end = round_up(b * rs, cs);
            while off < end {
                let vcn = off / cs;
                let idx = self.runs.partition_point(|r| r.vcn + r.len <= vcn);
                let Some(run) = self.runs.get(idx).filter(|r| r.vcn <= vcn) else {
                    pieces.push(Piece {
                        buf_off: (off - base) as usize,
                        disk_off: None,
                        len: (end - off) as usize,
                    });
                    break;
                };
                let n = ((run.vcn + run.len) * cs).min(end) - off;
                pieces.push(Piece {
                    buf_off: (off - base) as usize,
                    disk_off: run.lcn.map(|l| l * cs + (off - run.vcn * cs)),
                    len: n as usize,
                });
                off += n;
            }
        }
        pieces
    }
}

/// Reads `$MFT` (record 0) and works out the MFT's extents, size and bitmap.
pub(crate) fn load_layout<R: ReadAt>(src: &mut R, g: &Geometry) -> Result<MftLayout> {
    let rs = g.record_size as usize;
    let cs = u64::from(g.cluster_size);
    let first_len = round_up(rs as u64, cs) as usize;
    let mut buf = AlignedBuf::new(first_len, g.io_align());
    src.read_exact_at(g.mft_lcn * cs, &mut buf)?;
    let (rec0, h0) = checked_record(&buf[..rs], "MFT record 0")?;
    drop(buf);

    let mut attrs = MftAttrs::default();
    collect_mft_attrs(&rec0, &h0, &mut attrs)?;

    // A very fragmented MFT keeps later runlist segments in extension
    // records, listed in $MFT's attribute list.
    let list = match (attrs.list_resident.take(), attrs.list_runs.take()) {
        (Some(v), _) => Some(v),
        (None, Some((runs, len))) => Some(read_stream(src, &runs, g, 0, len as usize)?),
        _ => None,
    };
    if let Some(list) = list {
        let mut wanted: Vec<u64> = parse_attribute_list(&list)
            .into_iter()
            .filter(|e| !e.named && e.record != 0)
            .filter(|e| e.type_code == AT_DATA || e.type_code == AT_BITMAP)
            .map(|e| e.record)
            .collect();
        wanted.sort_unstable();
        wanted.dedup();
        // Each extension record must be reachable through the runs known
        // so far; keep going while that makes progress.
        while !wanted.is_empty() {
            let known: Vec<Run> = {
                let mut segs: Vec<&Segment> = attrs.data.iter().collect();
                segs.sort_by_key(|s| s.lowest_vcn);
                segs.iter().flat_map(|s| s.runs.iter().copied()).collect()
            };
            let before = wanted.len();
            let mut still = Vec::new();
            for &r in &wanted {
                match read_stream(src, &known, g, r * rs as u64, rs) {
                    Ok(raw) => {
                        let (rec, h) = checked_record(&raw, "MFT extension record")?;
                        collect_mft_attrs(&rec, &h, &mut attrs)?;
                    }
                    Err(Error::Corrupt(_)) => still.push(r),
                    Err(e) => return Err(e),
                }
            }
            if still.len() == before {
                return Err(Error::Corrupt("unreachable $MFT extension records".into()));
            }
            wanted = still;
        }
    }

    let (runs, sizes) = join_segments(attrs.data, "$MFT:$DATA")?;
    let (_, data_size, valid_size) =
        sizes.ok_or_else(|| Error::Corrupt("$MFT has no first $DATA segment".into()))?;
    let mapped: u64 = runs.iter().map(|r| r.len).sum::<u64>() * cs;
    let bytes = data_size
        .min(valid_size)
        .min(g.mft_valid_len.unwrap_or(u64::MAX))
        .min(mapped);
    let records = bytes / rs as u64;
    if records > u64::from(u32::MAX) {
        return Err(Error::Corrupt("MFT has more than 2^32 records".into()));
    }

    let bitmap = match attrs.bitmap_resident {
        Some(v) => Some(v),
        None if !attrs.bitmap.is_empty() => {
            let (bruns, bsizes) = join_segments(attrs.bitmap, "$MFT:$BITMAP")?;
            let len = bsizes.map_or(0, |s| s.1).min(records.div_ceil(8));
            Some(read_stream(src, &bruns, g, 0, len as usize)?)
        }
        None => None,
    };

    Ok(MftLayout {
        record_size: rs,
        cluster_size: cs,
        align: g.io_align(),
        runs,
        records,
        bitmap,
    })
}

struct Chunk {
    first: u64,
    count: usize,
    buf: AlignedBuf,
}

/// Parses one chunk in parallel; the batches come back in record order,
/// with their names encoded by `enc` if there is one.
fn parse_chunk(layout: &MftLayout, chunk: &mut Chunk, enc: Option<&Encoder>) -> Vec<ParsedBatch> {
    let rs = layout.record_size;
    let first = chunk.first;
    let len = chunk.count * rs;
    chunk.buf[..len]
        .par_chunks_mut(BLOCK_RECORDS * rs)
        .enumerate()
        .map(|(bi, block)| {
            let mut out = ParsedBatch::with_capacity(block.len() / rs);
            for (j, rec) in block.chunks_exact_mut(rs).enumerate() {
                let no = first + (bi * BLOCK_RECORDS + j) as u64;
                if layout.in_use(no) {
                    parse_record(no as u32, rec, &mut out);
                }
            }
            out.prepare_names(enc);
            out
        })
        .collect()
}

/// Streams every record of the MFT through the parser into `acc`.
pub(crate) fn stream_records<R: ReadAt>(
    src: &mut R,
    layout: &MftLayout,
    tuning: &Tuning,
    acc: &mut Accum,
    progress: &ScanProgress,
) -> Result<()> {
    let rs = layout.record_size;
    let total = layout.records;
    if total == 0 {
        return Ok(());
    }
    // Chunks hold whole records and whole clusters (both powers of two).
    let unit = (layout.cluster_size as usize).max(rs);
    let chunk_bytes = tuning.chunk_bytes.max(unit).div_ceil(unit) * unit;
    let per_chunk = (chunk_bytes / rs) as u64;
    let n_chunks = total.div_ceil(per_chunk);
    let buf_len = (round_up(total * rs as u64, unit as u64) as usize).min(chunk_bytes);
    let gap = tuning.skip_gap_bytes;

    std::thread::scope(|s| {
        let (full_tx, full_rx) = mpsc::sync_channel::<io::Result<Chunk>>(QUEUE_DEPTH);
        let (free_tx, free_rx) = mpsc::channel::<AlignedBuf>();
        let buffers = (QUEUE_DEPTH + 2).min(n_chunks as usize);
        for _ in 0..buffers {
            let _ = free_tx.send(AlignedBuf::new(buf_len, layout.align));
        }

        let reader = s.spawn(move || {
            for k in 0..n_chunks {
                if progress.is_cancelled() {
                    return;
                }
                let r0 = k * per_chunk;
                let r1 = (r0 + per_chunk).min(total);
                let pieces = layout.chunk_pieces(r0, r1, gap);
                if pieces.is_empty() {
                    continue; // nothing in use here
                }
                let Ok(mut buf) = free_rx.recv() else {
                    return;
                };
                let mut res = Ok(());
                for p in &pieces {
                    let dst = &mut buf[p.buf_off..p.buf_off + p.len];
                    match p.disk_off {
                        Some(off) => {
                            if let Err(e) = src.read_exact_at(off, dst) {
                                res = Err(e);
                                break;
                            }
                        }
                        None => dst.fill(0),
                    }
                }
                let failed = res.is_err();
                let msg = res.map(|()| Chunk {
                    first: r0,
                    count: (r1 - r0) as usize,
                    buf,
                });
                if full_tx.send(msg).is_err() || failed {
                    return;
                }
            }
        });

        // Each chunk is parsed (on the pool) while the one before it is
        // merged (single-threaded), so the two overlap.
        let mut result = Ok(());
        let mut parsed: Vec<ParsedBatch> = Vec::new();
        loop {
            let next = match full_rx.recv() {
                Ok(Ok(c)) => Some(c),
                Ok(Err(e)) => {
                    result = Err(Error::Io(e));
                    break;
                }
                Err(_) => None,
            };
            if progress.is_cancelled() {
                break;
            }
            let enc = acc.name_encoder();
            let ready = std::mem::take(&mut parsed);
            let free_tx = &free_tx;
            let (next, ()) = rayon::join(
                || {
                    next.map(|mut chunk| {
                        let batches = parse_chunk(layout, &mut chunk, enc.as_ref());
                        let _ = free_tx.send(chunk.buf);
                        batches
                    })
                },
                || {
                    for b in &ready {
                        acc.merge(b, progress);
                    }
                },
            );
            match next {
                Some(batches) => parsed = batches,
                None => break,
            }
        }
        // Unblock the reader whatever it is waiting on, then wait for it.
        drop(full_rx);
        drop(free_tx);
        if reader.join().is_err() && result.is_ok() {
            result = Err(Error::Corrupt("MFT reader thread panicked".into()));
        }
        if result.is_ok() && progress.is_cancelled() {
            result = Err(Error::Cancelled);
        }
        result
    })
}

#[cfg(test)]
mod tests {
    use super::super::testutil::*;
    use super::*;

    #[test]
    fn boot_sector_geometry() {
        let mut v = SynthVolume::new(4096, 1024, vec![(4, 8)], 32);
        v.add(5, RecordBuilder::new(1024, 5, 5).dir().build());
        let img = v.image();
        let g = Geometry::from_boot_sector(&img[..512]).unwrap();
        assert_eq!(
            g,
            Geometry {
                sector_size: 512,
                cluster_size: 4096,
                record_size: 1024,
                mft_lcn: 4,
                mft_valid_len: None
            }
        );
        let mut b = img[..512].to_vec();
        b[0x40] = 1; // one cluster per record
        assert_eq!(Geometry::from_boot_sector(&b).unwrap().record_size, 4096);
        b[3] = b'X';
        assert!(matches!(
            Geometry::from_boot_sector(&b),
            Err(Error::Unsupported(_))
        ));
    }

    fn layout_with_bitmap(bits: Vec<u8>, records: u64, runs: Vec<Run>) -> MftLayout {
        MftLayout {
            record_size: 1024,
            cluster_size: 4096,
            align: 4096,
            runs,
            records,
            bitmap: Some(bits),
        }
    }

    #[test]
    fn chunk_pieces_follow_extents_and_skip_free_space() {
        // 64 records = 16 clusters: 8 at LCN 100, 8 at LCN 500.
        let runs = vec![
            Run {
                vcn: 0,
                lcn: Some(100),
                len: 8,
            },
            Run {
                vcn: 8,
                lcn: Some(500),
                len: 8,
            },
        ];
        // In use: 0..4 and 60..64; everything between is free.
        let mut bits = vec![0u8; 8];
        bits[0] = 0x0F;
        bits[7] = 0xF0;
        let l = layout_with_bitmap(bits, 64, runs.clone());
        assert_eq!(l.in_use_count(), 8);
        // A small gap threshold skips the free middle.
        assert_eq!(
            l.chunk_pieces(0, 64, 8192),
            vec![
                Piece {
                    buf_off: 0,
                    disk_off: Some(100 * 4096),
                    len: 4096
                },
                Piece {
                    buf_off: 60 * 1024,
                    disk_off: Some(507 * 4096),
                    len: 4096
                },
            ]
        );
        // A large one reads straight through, split at the extent boundary.
        assert_eq!(
            l.chunk_pieces(0, 64, 1 << 20),
            vec![
                Piece {
                    buf_off: 0,
                    disk_off: Some(100 * 4096),
                    len: 8 * 4096
                },
                Piece {
                    buf_off: 8 * 4096,
                    disk_off: Some(500 * 4096),
                    len: 8 * 4096
                },
            ]
        );
        // A chunk that starts mid-MFT is relative to its first record.
        assert_eq!(
            l.chunk_pieces(32, 64, 8192),
            vec![Piece {
                buf_off: 28 * 1024,
                disk_off: Some(507 * 4096),
                len: 4096
            }]
        );
        // Nothing in use: nothing to read.
        assert!(l.chunk_pieces(8, 56, 8192).is_empty());
        // Without a bitmap everything is read; unmapped space is zero filled.
        let mut l = layout_with_bitmap(vec![], 72, runs);
        l.bitmap = None;
        assert_eq!(
            l.chunk_pieces(60, 72, 8192),
            vec![
                Piece {
                    buf_off: 0,
                    disk_off: Some(507 * 4096),
                    len: 4096
                },
                Piece {
                    buf_off: 4096,
                    disk_off: None,
                    len: 8192
                },
            ]
        );
    }

    fn user_records(v: &mut SynthVolume, n: u32) {
        let rs = v.record_size;
        v.add(
            5,
            RecordBuilder::new(rs, 5, 5)
                .dir()
                .attr(si(6))
                .attr(file_name(5, 5, ".", 3))
                .build(),
        );
        v.add(
            16,
            RecordBuilder::new(rs, 16, 1)
                .dir()
                .attr(si(0))
                .attr(file_name(5, 5, "d", 1))
                .build(),
        );
        for i in 0..n {
            let r = 17 + i;
            v.add(
                r,
                RecordBuilder::new(rs, r, 1)
                    .attr(si(0))
                    .attr(file_name(16, 1, &format!("f{i}"), 1))
                    .attr(data_nonresident(None, 0, 4096, u64::from(i), None, 0))
                    .build(),
            );
        }
    }

    fn scan_image(img: Vec<u8>, tuning: Tuning) -> super::super::build::Built {
        let mut src = std::io::Cursor::new(img);
        let mut boot = [0u8; 512];
        src.read_exact_at(0, &mut boot).unwrap();
        let g = Geometry::from_boot_sector(&boot).unwrap();
        let progress = ScanProgress::new();
        super::super::scan_source(&mut src, g, b"S:\\", &progress, &tuning).unwrap()
    }

    #[test]
    fn streams_a_fragmented_mft_in_small_chunks() {
        // 512-byte clusters: every 1 KiB record spans two clusters, and the
        // odd-length extents make records straddle extent boundaries.
        let mut v = SynthVolume::new(
            512,
            1024,
            vec![(40, 33), (200, 17), (90, 61), (400, 89)],
            100,
        );
        user_records(&mut v, 80);
        let img = v.image();
        for (chunk, use_bitmap) in [(2048, true), (4096, false), (8 << 20, true)] {
            let tuning = Tuning {
                chunk_bytes: chunk,
                skip_gap_bytes: 1024,
                use_bitmap,
            };
            let b = scan_image(img.clone(), tuning);
            b.tree.check_invariants().unwrap();
            assert_eq!(b.files, 80 + 1, "80 files plus $MFT, chunk {chunk}");
            let d = b.tree.lookup_relative(b"d").unwrap();
            assert_eq!(b.tree.child_count(d), 80);
            assert_eq!(b.tree.node(d).logical(), (0..80).sum::<u64>());
            let f79 = b.tree.lookup_relative(b"d\\f79").unwrap();
            assert_eq!(b.frn.get(96), Some(f79));
        }
    }

    #[test]
    fn mft_runlist_continued_in_an_extension_record() {
        let rs = 1024;
        // Extents: 16 records at LCN 8 (4 clusters), 48 more at LCN 100.
        let mut v = SynthVolume::new(4096, rs, vec![(8, 4), (100, 12)], 64);
        user_records(&mut v, 30);
        // $MFT keeps the first runlist segment and lists the second one in
        // extension record 15.
        let list = attribute_list(&[
            (AT_STANDARD_INFORMATION, None, 0, 0),
            (AT_DATA, None, 0, 0),
            (AT_DATA, None, 4, 15),
            (AT_BITMAP, None, 0, 0),
        ]);
        let mut bits = vec![0u8; 8];
        for r in [0u32, 5, 15].into_iter().chain(16..47) {
            bits[r as usize / 8] |= 1 << (r % 8);
        }
        let rec0 = RecordBuilder::new(rs, 0, 1)
            .attr(si(6))
            .attr(attribute_list_attr(&list))
            .attr(file_name(5, 5, "$MFT", 3))
            .attr(non_resident(
                AT_DATA,
                None,
                0,
                3,
                (16 * 4096, 64 * 1024, 64 * 1024),
                None,
                0,
                &encode_runlist(&[(Some(8), 4)]),
            ))
            .attr(bitmap_resident(&bits))
            .build();
        let rec15 = RecordBuilder::new(rs, 15, 1)
            .base(0, 1)
            .attr(non_resident(
                AT_DATA,
                None,
                4,
                15,
                (0, 0, 0),
                None,
                0,
                &encode_runlist(&[(Some(100), 12)]),
            ))
            .build();
        v.add(0, rec0);
        v.add(15, rec15);
        let img = v.image();

        let mut src = std::io::Cursor::new(img.clone());
        let g = Geometry::from_boot_sector(&img[..512]).unwrap();
        let layout = load_layout(&mut src, &g).unwrap();
        assert_eq!(layout.records, 64);
        assert_eq!(
            layout.runs,
            vec![
                Run {
                    vcn: 0,
                    lcn: Some(8),
                    len: 4
                },
                Run {
                    vcn: 4,
                    lcn: Some(100),
                    len: 12
                }
            ]
        );
        let b = scan_image(img, Tuning::default());
        b.tree.check_invariants().unwrap();
        assert_eq!(b.files, 30 + 1, "30 files plus $MFT");
        assert!(b.tree.lookup_relative(b"d\\f29").is_some());
    }

    #[test]
    fn cancelled_scan_stops() {
        let mut v = SynthVolume::new(4096, 1024, vec![(4, 32)], 128);
        user_records(&mut v, 100);
        let img = v.image();
        let g = Geometry::from_boot_sector(&img[..512]).unwrap();
        let progress = ScanProgress::new();
        progress.cancel();
        let mut src = std::io::Cursor::new(img);
        let r = super::super::scan_source(&mut src, g, b"S:\\", &progress, &Tuning::default());
        assert!(matches!(r, Err(Error::Cancelled)));
    }
}
