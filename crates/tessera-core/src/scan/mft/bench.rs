//! A synthetic MFT generated on the fly (default 3 million records), to
//! measure the whole read + parse + build pipeline and its peak memory
//! without a real volume:
//!
//! ```text
//! cargo test --release -p tessera-core synthetic_mft -- --ignored --nocapture
//! ```
//!
//! `TESSERA_SYNTH_RECORDS` changes the record count. The "volume" is only
//! generated as it is read, so the MFT never exists in memory at once.

use std::io;
use std::time::Instant;

use super::parser::*;
use super::reader::{Geometry, ReadAt, Tuning};
use super::testutil::*;
use crate::scan::ScanProgress;
use crate::tree::NodeId;

const RS: usize = 1024;
const CS: u64 = 4096;
const BITMAP_LCN: u64 = 1 << 10;
const MFT_LCN: u64 = 1 << 20;

/// A record with fields to patch per record number.
struct Template {
    raw: Vec<u8>,
    parent_off: usize,
    digits_off: usize,
    alloc_off: Option<usize>,
}

fn find(hay: &[u8], needle: &[u8]) -> usize {
    hay.windows(needle.len())
        .position(|w| w == needle)
        .expect("marker")
}

fn utf16le(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
}

fn template(dir: bool) -> Template {
    // Names of realistic length (~20 characters) with 8 digits to patch.
    let name = if dir {
        "Some Directory 00000000"
    } else {
        "file-name-00000000.dat"
    };
    let mut b =
        RecordBuilder::new(RS, 0, 1)
            .attr(si(0x20))
            .attr(file_name(0xA1B2_C3D4, 1, name, 1));
    b = if dir {
        b.dir()
    } else {
        b.attr(data_nonresident(None, 0, 0x7777_0000, 0x7777_0000, None, 0))
    };
    let raw = b.build();
    let parent_off = find(&raw, &0xA1B2_C3D4u32.to_le_bytes());
    let digits_off = find(&raw, &utf16le("00000000"));
    let alloc_off = (!dir).then(|| find(&raw, &0x7777_0000u64.to_le_bytes()));
    // Patched fields must stay clear of the update sequence positions.
    assert!(alloc_off.unwrap_or(0) + 16 < 510 && digits_off + 16 < 510);
    Template {
        raw,
        parent_off,
        digits_off,
        alloc_off,
    }
}

/// Dirs are records 16, 26, 36, ...; each holds the 9 files after it and
/// up to 8 sub-directories (an 8-ary directory tree, ~6 levels deep).
struct Synth {
    records: u64,
    rec0: Vec<u8>,
    root: Vec<u8>,
    dir: Template,
    file: Template,
}

impl Synth {
    fn new(records: u64) -> Synth {
        let mft_clusters = (records * RS as u64).div_ceil(CS);
        let bitmap_len = records.div_ceil(8);
        let bitmap_clusters = bitmap_len.div_ceil(CS);
        let rec0 = RecordBuilder::new(RS, 0, 1)
            .attr(si(0x6))
            .attr(file_name(5, 1, "$MFT", 3))
            .attr(non_resident(
                AT_DATA,
                None,
                0,
                mft_clusters - 1,
                (mft_clusters * CS, records * RS as u64, records * RS as u64),
                None,
                0,
                &encode_runlist(&[(Some(MFT_LCN), mft_clusters)]),
            ))
            .attr(non_resident(
                AT_BITMAP,
                None,
                0,
                bitmap_clusters - 1,
                (bitmap_clusters * CS, bitmap_len, bitmap_len),
                None,
                0,
                &encode_runlist(&[(Some(BITMAP_LCN), bitmap_clusters)]),
            ))
            .build();
        let root = RecordBuilder::new(RS, 5, 1)
            .dir()
            .attr(si(0x6))
            .attr(file_name(5, 1, ".", 3))
            .build();
        Synth {
            records,
            rec0,
            root,
            dir: template(true),
            file: template(false),
        }
    }

    fn write_record(&self, r: u64, out: &mut [u8]) {
        match r {
            0 => return out.copy_from_slice(&self.rec0),
            5 => return out.copy_from_slice(&self.root),
            _ if r < 16 || r >= self.records => return out.fill(0),
            _ => {}
        }
        let group = (r - 16) / 10;
        let is_dir = (r - 16) % 10 == 0;
        let (t, parent) = if is_dir {
            let p = if group == 0 {
                5
            } else {
                16 + 10 * ((group - 1) / 8)
            };
            (&self.dir, p)
        } else {
            (&self.file, 16 + 10 * group)
        };
        out.copy_from_slice(&t.raw);
        out[t.parent_off..t.parent_off + 8].copy_from_slice(&(parent | 1 << 48).to_le_bytes());
        let mut n = r;
        for k in (0..8).rev() {
            out[t.digits_off + 2 * k] = b'0' + (n % 10) as u8;
            n /= 10;
        }
        if let Some(a) = t.alloc_off {
            let len = (r % 50_000) * 7;
            out[a..a + 8].copy_from_slice(&len.div_ceil(CS).wrapping_mul(CS).to_le_bytes());
            out[a + 8..a + 16].copy_from_slice(&len.to_le_bytes());
            out[a + 16..a + 24].copy_from_slice(&len.to_le_bytes());
        }
    }
}

impl ReadAt for Synth {
    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        if offset >= MFT_LCN * CS {
            let first = (offset - MFT_LCN * CS) / RS as u64;
            for (i, rec) in buf.chunks_exact_mut(RS).enumerate() {
                self.write_record(first + i as u64, rec);
            }
        } else if offset >= BITMAP_LCN * CS {
            buf.fill(0xFF);
        } else {
            return Err(io::Error::other("unexpected read"));
        }
        Ok(())
    }
}

/// Peak resident memory of the process so far (Linux).
/// Peak resident set size; only known on Linux (`n/a` elsewhere).
fn rss(bytes: Option<u64>) -> String {
    bytes.map_or_else(
        || "n/a".to_owned(),
        |b| format!("{:.1} MiB", b as f64 / 1048576.0),
    )
}

fn peak_rss() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = s.lines().find(|l| l.starts_with("VmHWM:"))?;
    line.split_whitespace()
        .nth(1)?
        .parse::<u64>()
        .ok()
        .map(|kb| kb * 1024)
}

#[test]
#[ignore = "benchmark: cargo test --release -p tessera-core synthetic_mft -- --ignored --nocapture"]
fn synthetic_mft() {
    let n: u64 = std::env::var("TESSERA_SYNTH_RECORDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3_000_000);
    let mut src = Synth::new(n);
    let g = Geometry {
        sector_size: 512,
        cluster_size: CS as u32,
        record_size: RS as u32,
        mft_lcn: MFT_LCN,
        mft_valid_len: None,
    };
    let progress = ScanProgress::new();
    let before = peak_rss();
    // The steps of `scan_source`, timed one by one.
    let t0 = Instant::now();
    let layout = super::reader::load_layout(&mut src, &g).unwrap();
    let mut acc = super::build::Accum::new(
        b"C:\\",
        layout.records as usize,
        layout.in_use_count() as usize,
    );
    super::reader::stream_records(&mut src, &layout, &Tuning::default(), &mut acc, &progress)
        .unwrap();
    let t_stream = t0.elapsed();
    let b = acc.finish();
    let elapsed = t0.elapsed();
    eprintln!(
        "read + parse + merge {t_stream:?}, sort + build + sum {:?}",
        elapsed - t_stream
    );
    let peak = peak_rss();
    let t = &b.tree;
    let mem = t.memory();
    let mib = |x: u64| x as f64 / 1048576.0;
    eprintln!(
        "synthetic MFT: {n} records -> {} nodes in {elapsed:?} ({:.0} records/s, {} threads)\n\
         tree {:.1} MiB (nodes {:.1}, names {:.1}, uncompressed {:.1}) = {:.1} B/node; FRN index {:.1} MiB\n\
         peak RSS {} (before scan {})",
        t.len(),
        n as f64 / elapsed.as_secs_f64(),
        rayon::current_num_threads(),
        mib(mem.total() as u64),
        mib(mem.nodes as u64),
        mib(mem.names as u64),
        mib(mem.names_raw as u64),
        mem.total() as f64 / t.len() as f64,
        mib(b.frn.heap_bytes() as u64),
        rss(peak),
        rss(before),
    );
    assert_eq!(
        t.len() as u64,
        n - 16 + 2,
        "every record plus root and $MFT"
    );
    assert_eq!(b.skipped, 0);
    let expected: u64 = (16..n)
        .filter(|r| (r - 16) % 10 != 0)
        .map(|r| (r % 50_000) * 7)
        .sum::<u64>();
    assert_eq!(t.node(NodeId::ROOT).logical(), expected + (n * RS as u64));
    let t1 = Instant::now();
    t.check_invariants().unwrap();
    eprintln!("check_invariants: {:?}", t1.elapsed());
}
