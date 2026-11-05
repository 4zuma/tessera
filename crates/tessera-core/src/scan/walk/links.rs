//! Hard-link deduplication for the walk.
//!
//! A directory listing does not say how many links a file has, so every
//! file's id has to be remembered for the length of the scan. Two
//! representations keep that cheap:
//!
//! * On local NTFS the file id is a file reference: the low 48 bits are the
//!   MFT record number, which is dense from 0 to the size of the MFT. A
//!   lazily allocated bitmap indexed by record number needs one bit per MFT
//!   record (about 0.5 MiB for 4 million records), and testing a bit is a
//!   single lock-free `fetch_or`.
//! * Elsewhere (ReFS, network shares, anything that claims hard-link support
//!   but has no dense ids), a sharded hash set of the full 64-bit id.
//!
//! Everything here lives only for one scan and is freed when the scan ends.

use std::collections::HashSet;
use std::hash::{BuildHasherDefault, Hasher};
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicPtr, AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

/// Records per bitmap chunk (2^20 bits = 128 KiB).
const CHUNK_SHIFT: u32 = 20;
const CHUNK_WORDS: usize = 1 << (CHUNK_SHIFT - 6);
/// Chunk slots: covers record numbers below 2^32.
const DIRECTORY_LEN: usize = 1 << (32 - CHUNK_SHIFT);
/// At most this many chunks (8 MiB, 67 million records) are allocated. A
/// volume that hands out sparse ids while claiming to be NTFS cannot blow
/// the bitmap up: further chunks are diverted to the hash set.
const MAX_CHUNKS: usize = 64;
const SHARDS: usize = 64;

/// How file ids are remembered during one scan.
pub(super) enum LinkSet {
    /// No hard links on this file system (or its ids are unknown).
    Off,
    /// Bitmap over NTFS MFT record numbers, plus a hash set for the rare
    /// id that falls outside it.
    Records(RecordBitmap, ShardedSet),
    /// Full 64-bit ids in a hash set.
    Ids(ShardedSet),
}

impl LinkSet {
    pub(super) fn records() -> Self {
        LinkSet::Records(RecordBitmap::new(), ShardedSet::new())
    }

    pub(super) fn ids() -> Self {
        LinkSet::Ids(ShardedSet::new())
    }

    /// Records a sighting of the file with id `id` and returns true if it
    /// is the first one (its bytes should be counted).
    ///
    /// `empty` files are not remembered by the hash set (their duplicates
    /// add no bytes, and most files on some volumes are empty); the bitmap
    /// tracks them anyway because a bit costs nothing.
    #[inline]
    pub(super) fn first_sighting(&self, id: u64, empty: bool) -> bool {
        // 0 and -1 are what file systems return when they have no id.
        if id == 0 || id == u64::MAX {
            return true;
        }
        match self {
            LinkSet::Off => true,
            LinkSet::Records(bits, overflow) => {
                // Low 48 bits: MFT record number; high 16: sequence number.
                let record = id & 0x0000_FFFF_FFFF_FFFF;
                match bits.test_and_set(record) {
                    Some(was_set) => !was_set,
                    None => empty || overflow.insert(id),
                }
            }
            LinkSet::Ids(set) => empty || set.insert(id),
        }
    }

    /// Heap memory currently held.
    pub(super) fn heap_bytes(&self) -> usize {
        match self {
            LinkSet::Off => 0,
            LinkSet::Records(bits, overflow) => bits.heap_bytes() + overflow.heap_bytes(),
            LinkSet::Ids(set) => set.heap_bytes(),
        }
    }
}

/// A sparse bitmap over record numbers, grown chunk by chunk without
/// blocking readers.
pub(super) struct RecordBitmap {
    /// One slot per chunk: null (not allocated yet), [`diverted`] (never
    /// allocated; use the hash set) or a pointer to `CHUNK_WORDS` words.
    /// Once a slot is non-null it never changes until the bitmap is dropped.
    slots: Box<[AtomicPtr<AtomicU64>]>,
    /// Serializes allocation; holds the number of allocated chunks.
    allocated: Mutex<usize>,
}

fn diverted() -> *mut AtomicU64 {
    // Never dereferenced; distinct from null and from any real allocation.
    NonNull::<AtomicU64>::dangling().as_ptr()
}

impl RecordBitmap {
    fn new() -> Self {
        RecordBitmap {
            slots: (0..DIRECTORY_LEN)
                .map(|_| AtomicPtr::new(ptr::null_mut()))
                .collect(),
            allocated: Mutex::new(0),
        }
    }

    /// Sets the bit for `record` and returns its previous value, or `None`
    /// if `record` is not covered by the bitmap.
    #[inline]
    fn test_and_set(&self, record: u64) -> Option<bool> {
        let chunk = usize::try_from(record >> CHUNK_SHIFT).ok()?;
        let slot = self.slots.get(chunk)?;
        let mut p = slot.load(Ordering::Acquire);
        if p.is_null() {
            p = self.allocate(slot);
        }
        if p == diverted() {
            return None;
        }
        let bit = (record as usize) & ((1 << CHUNK_SHIFT) - 1);
        // SAFETY: p is a live chunk of CHUNK_WORDS words (published under
        // the mutex with Release, read with Acquire) and bit >> 6 is below
        // CHUNK_WORDS. Chunks are only freed in Drop, which has `&mut self`.
        let word = unsafe { &*p.add(bit >> 6) };
        let mask = 1u64 << (bit & 63);
        Some(word.fetch_or(mask, Ordering::Relaxed) & mask != 0)
    }

    #[cold]
    fn allocate(&self, slot: &AtomicPtr<AtomicU64>) -> *mut AtomicU64 {
        let mut count = self
            .allocated
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let p = slot.load(Ordering::Acquire);
        if !p.is_null() {
            return p;
        }
        let new = if *count < MAX_CHUNKS {
            *count += 1;
            let words: Box<[AtomicU64]> = (0..CHUNK_WORDS).map(|_| AtomicU64::new(0)).collect();
            Box::into_raw(words) as *mut AtomicU64
        } else {
            diverted()
        };
        slot.store(new, Ordering::Release);
        new
    }

    fn heap_bytes(&self) -> usize {
        let count = *self
            .allocated
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        self.slots.len() * size_of::<AtomicPtr<AtomicU64>>() + count * CHUNK_WORDS * 8
    }
}

impl Drop for RecordBitmap {
    fn drop(&mut self) {
        for slot in self.slots.iter_mut() {
            let p = *slot.get_mut();
            if !p.is_null() && p != diverted() {
                // SAFETY: allocated in `allocate` from a boxed slice of
                // exactly CHUNK_WORDS words, and freed only here.
                drop(unsafe { Box::from_raw(ptr::slice_from_raw_parts_mut(p, CHUNK_WORDS)) });
            }
        }
    }
}

type Shard = Mutex<HashSet<u64, BuildHasherDefault<IdHasher>>>;

/// A hash set of 64-bit ids split into independently locked shards.
pub(super) struct ShardedSet {
    shards: Box<[Shard]>,
}

impl ShardedSet {
    fn new() -> Self {
        ShardedSet {
            shards: (0..SHARDS).map(|_| Mutex::default()).collect(),
        }
    }

    /// Inserts `id`; true if it was not present.
    #[inline]
    fn insert(&self, id: u64) -> bool {
        // The shard comes from the middle bits of the mixed id: the hash
        // table itself uses the low bits (bucket) and the top 7 (tag).
        let shard = (mix(id) >> 32) as usize % SHARDS;
        self.shards[shard]
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id)
    }

    fn heap_bytes(&self) -> usize {
        self.shards
            .iter()
            .map(|s| {
                let cap = s.lock().unwrap_or_else(PoisonError::into_inner).capacity();
                // Buckets are a power of two at most 7/8 full, one control
                // byte per bucket; approximate from the usable capacity.
                if cap == 0 {
                    0
                } else {
                    (cap * 8 / 7).next_power_of_two() * 9
                }
            })
            .sum::<usize>()
            + self.shards.len() * size_of::<Mutex<HashSet<u64>>>()
    }
}

/// SplitMix64 finalizer: file ids are often sequential, so they need mixing
/// before their bits pick a bucket.
#[inline]
fn mix(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

#[derive(Default)]
struct IdHasher(u64);

impl Hasher for IdHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }

    #[inline]
    fn write_u64(&mut self, x: u64) {
        self.0 = mix(x);
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = mix(self.0 ^ b as u64);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitmap_counts_each_record_once() {
        let s = LinkSet::records();
        // Same record, different sequence numbers in the high 16 bits are
        // still the same MFT record.
        assert!(s.first_sighting(5 | (1 << 48), false));
        assert!(!s.first_sighting(5 | (1 << 48), false));
        assert!(!s.first_sighting(5 | (2 << 48), true));
        assert!(s.first_sighting(6, false));
        // Ids the file system could not supply are never deduplicated.
        assert!(s.first_sighting(0, false));
        assert!(s.first_sighting(0, false));
        assert!(s.first_sighting(u64::MAX, false));
        assert!(s.first_sighting(u64::MAX, false));
        // One chunk plus the slot directory.
        assert_eq!(s.heap_bytes() - empty_overhead(), 128 * 1024);
    }

    fn empty_overhead() -> usize {
        LinkSet::records().heap_bytes()
    }

    #[test]
    fn bitmap_diverts_sparse_ids_to_the_hash_set() {
        let s = LinkSet::records();
        // Records beyond 2^32 are not covered by the bitmap at all.
        let far = 1u64 << 40;
        assert!(s.first_sighting(far, false));
        assert!(!s.first_sighting(far, false));
        // Touch more chunks than allowed; the later ones divert, and the
        // answer for each id stays consistent.
        for c in 0..(MAX_CHUNKS as u64 + 10) {
            let id = c << CHUNK_SHIFT | 7;
            assert!(s.first_sighting(id, false), "chunk {c}");
            assert!(!s.first_sighting(id, false), "chunk {c}");
        }
        let LinkSet::Records(bits, _) = &s else {
            unreachable!()
        };
        assert_eq!(*bits.allocated.lock().unwrap(), MAX_CHUNKS);
    }

    #[test]
    fn id_set_skips_empty_files() {
        let s = LinkSet::ids();
        assert!(s.first_sighting(42, true));
        assert!(s.first_sighting(42, true));
        assert!(s.first_sighting(42, false));
        assert!(!s.first_sighting(42, false));
        assert!(LinkSet::Off.first_sighting(42, false));
    }

    #[test]
    fn concurrent_sightings_count_once() {
        let s = LinkSet::records();
        let ids = LinkSet::ids();
        let firsts = std::sync::atomic::AtomicU64::new(0);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    // (Id 0 means "unknown" and is never deduplicated.)
                    for r in 1..=50_000u64 {
                        if s.first_sighting(r * 37, false) {
                            firsts.fetch_add(1, Ordering::Relaxed);
                        }
                        if ids.first_sighting(r * 37 + 1, false) {
                            firsts.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                });
            }
        });
        assert_eq!(firsts.load(Ordering::Relaxed), 2 * 50_000);
        let _ = ids.heap_bytes();
    }
}
