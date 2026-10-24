//! Preallocated log-linear duration histogram. `record` never allocates, so it is callback-safe.

use crate::model::DurationSummary;

const SUB_BITS: u32 = 3;
const SUB_BUCKETS: usize = 1 << SUB_BITS;
/// 8 exact buckets (0..8 ns) + 8 sub-buckets per power-of-two octave up to 2^63 ns.
const BUCKETS: usize = (64 - SUB_BITS as usize + 1) * SUB_BUCKETS;

const THRESHOLD_100US_NS: u64 = 100_000;
const THRESHOLD_1MS_NS: u64 = 1_000_000;

/// Fixed-size histogram with at most 12.5% bucket error; `max_ns` is exact.
#[derive(Debug, Clone)]
pub struct DurationHistogram {
    counts: [u64; BUCKETS],
    count: u64,
    sum_ns: u128,
    max_ns: u64,
    over_100us: u64,
    over_1ms: u64,
}

impl Default for DurationHistogram {
    fn default() -> Self {
        Self::new()
    }
}

fn bucket_index(ns: u64) -> usize {
    if ns < SUB_BUCKETS as u64 {
        return ns as usize;
    }
    let msb = 63 - ns.leading_zeros();
    let sub = ((ns >> (msb - SUB_BITS)) as usize) & (SUB_BUCKETS - 1);
    (msb - SUB_BITS + 1) as usize * SUB_BUCKETS + sub
}

/// Largest value that maps into bucket `index`.
fn bucket_upper_bound(index: usize) -> u64 {
    if index < SUB_BUCKETS {
        return index as u64;
    }
    let octave = (index / SUB_BUCKETS) as u32; // >= 1
    let sub = (index % SUB_BUCKETS) as u64;
    let shift = octave - 1;
    let lower = (SUB_BUCKETS as u64 + sub) << shift;
    lower + ((1u64 << shift) - 1)
}

impl DurationHistogram {
    pub fn new() -> Self {
        Self {
            counts: [0; BUCKETS],
            count: 0,
            sum_ns: 0,
            max_ns: 0,
            over_100us: 0,
            over_1ms: 0,
        }
    }

    pub fn record(&mut self, ns: u64) {
        self.counts[bucket_index(ns)] += 1;
        self.count += 1;
        self.sum_ns += u128::from(ns);
        self.max_ns = self.max_ns.max(ns);
        if ns > THRESHOLD_100US_NS {
            self.over_100us += 1;
        }
        if ns > THRESHOLD_1MS_NS {
            self.over_1ms += 1;
        }
    }

    pub fn count(&self) -> u64 {
        self.count
    }

    /// Upper bound of the bucket containing the requested quantile, clamped to the observed max.
    pub fn percentile_ns(&self, per_mille: u32) -> u64 {
        if self.count == 0 {
            return 0;
        }
        let rank = (self.count * u64::from(per_mille.min(1000)))
            .div_ceil(1000)
            .max(1);
        let mut seen = 0u64;
        for (index, bucket) in self.counts.iter().enumerate() {
            seen += bucket;
            if seen >= rank {
                return bucket_upper_bound(index).min(self.max_ns);
            }
        }
        self.max_ns
    }

    pub fn summary(&self) -> DurationSummary {
        DurationSummary {
            count: self.count,
            mean_ns: if self.count == 0 {
                0
            } else {
                (self.sum_ns / u128::from(self.count)) as u64
            },
            p50_ns: self.percentile_ns(500),
            p99_ns: self.percentile_ns(990),
            max_ns: self.max_ns,
            over_100us: self.over_100us,
            over_1ms: self.over_1ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_bounds_contain_their_values() {
        for ns in [
            0u64,
            1,
            7,
            8,
            9,
            15,
            16,
            100,
            999,
            100_000,
            1_000_000,
            u64::MAX / 2,
            u64::MAX,
        ] {
            let index = bucket_index(ns);
            assert!(index < BUCKETS, "index in range for {ns}");
            let upper = bucket_upper_bound(index);
            assert!(upper >= ns, "upper bound {upper} covers {ns}");
            // Relative error stays within one sub-bucket (12.5%).
            assert!(upper - ns <= ns / 8 + 1, "{ns} -> {upper} too coarse");
        }
    }

    #[test]
    fn p99_reflects_the_tail_and_max_is_exact() {
        let mut hist = DurationHistogram::new();
        for _ in 0..9_900 {
            hist.record(10_000);
        }
        for _ in 0..99 {
            hist.record(2_000_000);
        }
        hist.record(5_000_123);
        let s = hist.summary();
        assert_eq!(s.count, 10_000);
        assert_eq!(s.max_ns, 5_000_123);
        assert!(s.p50_ns >= 10_000 && s.p50_ns < 12_000);
        // Rank 9900 of 10000 is still the 10 us bulk; the tail only shows at higher quantiles.
        assert!(
            s.p99_ns >= 10_000 && s.p99_ns < 12_000,
            "p99 = {}",
            s.p99_ns
        );
        assert!(hist.percentile_ns(999) >= 2_000_000);
        assert_eq!(s.over_1ms, 100);
        assert_eq!(s.over_100us, 100);
    }

    #[test]
    fn p99_moves_into_the_tail_when_the_tail_exceeds_one_percent() {
        let mut hist = DurationHistogram::new();
        for _ in 0..98 {
            hist.record(1_000);
        }
        for _ in 0..2 {
            hist.record(3_000_000);
        }
        let s = hist.summary();
        assert!(s.p99_ns >= 3_000_000 - 3_000_000 / 8, "p99 = {}", s.p99_ns);
        assert_eq!(s.max_ns, 3_000_000);
    }

    #[test]
    fn empty_histogram_is_all_zero() {
        assert_eq!(
            DurationHistogram::new().summary(),
            DurationSummary::default()
        );
    }
}
