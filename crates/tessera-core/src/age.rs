//! How long ago a file was last written, in one byte per node (the "age
//! byte"; the front end's `app/src/lib/age.ts` has the same scale).
//!
//! 0 means not known. 1 is under a day (or a time in the future), then a log
//! scale up to 255 for 30 years or more, about 3.7 % per step. A folder's or
//! a group's age is the size-weighted mean of the bytes of the files in it
//! (see [`Mean`]). The tree keeps the bytes relative to when it was created
//! ([`crate::Tree::age_of`]); a long session drifts by its own length, which
//! the log scale hides.
//!
//! The legend counts bytes in [`BINS`] fixed bins, one per span between two
//! of the edges a user can pick ([`EDGE_CHOICES`]), so every choice of age
//! bands is a sum of bins.

/// The byte of a node whose age is not known (folders, and files whose
/// scanner reports no time).
pub const UNKNOWN: u8 = 0;

const MAX_DAYS: f64 = 30.0 * 365.25;

/// ln(days) per step.
fn step() -> f64 {
    MAX_DAYS.ln() / 254.0
}

/// The age byte of something written `days` days ago.
pub fn byte_of_days(days: f64) -> u8 {
    if days.is_nan() {
        return UNKNOWN;
    }
    if days < 1.0 {
        return 1;
    }
    (1.0 + (days.ln() / step()).floor()).min(255.0) as u8
}

/// The age a byte stands for, in days (the start of its step); `None` when
/// not known.
pub fn days_of(b: u8) -> Option<f64> {
    match b {
        UNKNOWN => None,
        1 => Some(0.0),
        _ => Some(((b - 1) as f64 * step()).exp()),
    }
}

/// 100 ns ticks per day, in FILETIME units.
const TICKS_PER_DAY: f64 = 864e9;

/// The age byte of a FILETIME (100 ns since 1601) as seen at `now` (same
/// units). 0, the "no time" value scanners report, gives [`UNKNOWN`].
pub fn byte_of_filetime(t: u64, now: u64) -> u8 {
    if t == 0 {
        return UNKNOWN;
    }
    byte_of_days(now.saturating_sub(t) as f64 / TICKS_PER_DAY)
}

/// The current time as a FILETIME.
pub fn now_filetime() -> u64 {
    /// FILETIME of 1970-01-01.
    const UNIX_EPOCH: u64 = 116_444_736_000_000_000;
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    UNIX_EPOCH + (since.as_nanos() / 100) as u64
}

/// The band edges a user can pick, in days: 1, 3 days, 1, 2 weeks, 1, 3,
/// 6 months, 1, 2, 3, 5, 10 years.
pub const EDGE_CHOICES: [u32; 12] = [1, 3, 7, 14, 30, 91, 182, 365, 730, 1095, 1826, 3652];

/// Bins of the legend by age: not known, then one per span between edges.
pub const BINS: usize = EDGE_CHOICES.len() + 2;

/// The legend bin of an age byte: 0 when not known, else 1 + the number of
/// edge choices at or below its age.
pub fn bin(b: u8) -> usize {
    BIN_OF[b as usize] as usize
}

static BIN_OF: std::sync::LazyLock<[u8; 256]> = std::sync::LazyLock::new(|| {
    let mut t = [0u8; 256];
    for (b, slot) in t.iter_mut().enumerate().skip(1) {
        let d = days_of(b as u8).unwrap_or(0.0);
        *slot = 1 + EDGE_CHOICES.iter().filter(|&&e| d >= e as f64).count() as u8;
    }
    t
});

/// A size-weighted mean of age bytes, for folders and groups. Bytes whose
/// age is not known count for nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mean {
    sum: u128,
    weight: u64,
}

impl Mean {
    #[inline]
    pub fn add(&mut self, age: u8, size: u64) {
        if age != UNKNOWN && size > 0 {
            self.sum += age as u128 * size as u128;
            self.weight = self.weight.saturating_add(size);
        }
    }

    pub fn merge(&mut self, o: &Mean) {
        self.sum += o.sum;
        self.weight = self.weight.saturating_add(o.weight);
    }

    /// The mean, rounded; [`UNKNOWN`] when nothing had an age.
    pub fn byte(&self) -> u8 {
        if self.weight == 0 {
            return UNKNOWN;
        }
        let w = self.weight as u128;
        ((self.sum + w / 2) / w).clamp(1, 255) as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scale_matches_the_front_end() {
        assert_eq!(byte_of_days(f64::NAN), UNKNOWN);
        assert_eq!(byte_of_days(0.0), 1);
        assert_eq!(byte_of_days(0.99), 1);
        assert_eq!(byte_of_days(1.0), 1);
        assert_eq!(byte_of_days(7.0), 54);
        assert_eq!(byte_of_days(365.0), 162);
        assert_eq!(byte_of_days(MAX_DAYS * 1.001), 255);
        assert_eq!(byte_of_days(1e9), 255);
        assert_eq!(days_of(UNKNOWN), None);
        assert_eq!(days_of(1), Some(0.0));
        // a byte covers the days from its start to the next byte's start
        for b in 2..=255u8 {
            let d = days_of(b).unwrap();
            assert_eq!(byte_of_days(d * 1.000_001), b);
        }
    }

    #[test]
    fn filetimes_count_back_from_now() {
        let now = 134_000_000_000_000_000u64;
        assert_eq!(byte_of_filetime(0, now), UNKNOWN);
        assert_eq!(
            byte_of_filetime(now + 1_000_000, now),
            1,
            "the future is new"
        );
        let year_ago = now - (400.0 * TICKS_PER_DAY) as u64;
        assert_eq!(byte_of_filetime(year_ago, now), byte_of_days(400.0));
        assert!(now_filetime() > now);
    }

    #[test]
    fn bins_split_at_the_edge_choices() {
        assert_eq!(bin(UNKNOWN), 0);
        assert_eq!(bin(1), 1);
        assert_eq!(bin(byte_of_days(2.0)), 2);
        assert_eq!(bin(byte_of_days(400.0)), 9);
        assert_eq!(bin(255), BINS - 1);
        for b in 1..=255u8 {
            let d = days_of(b).unwrap();
            let i = bin(b);
            assert!(i == 1 || d >= EDGE_CHOICES[i - 2] as f64);
            assert!(i == BINS - 1 || d < EDGE_CHOICES[i - 1] as f64);
        }
    }

    #[test]
    fn means_weigh_by_size() {
        let mut m = Mean::default();
        assert_eq!(m.byte(), UNKNOWN);
        m.add(100, 3);
        m.add(200, 1);
        m.add(UNKNOWN, 1000);
        assert_eq!(m.byte(), 125);
        let mut n = Mean::default();
        n.add(50, 4);
        n.merge(&m);
        assert_eq!(n.byte(), 88);
    }
}
