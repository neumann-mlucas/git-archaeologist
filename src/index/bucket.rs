use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum BucketSize {
    Commit,
    Day,
    Week,
    Month,
    Tag,
}

impl BucketSize {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "commit" => Some(Self::Commit),
            "day" => Some(Self::Day),
            "week" => Some(Self::Week),
            "month" => Some(Self::Month),
            "tag" => Some(Self::Tag),
            _ => None,
        }
    }
}

/// Auto-pick a bucket size based on total commit count.
pub fn auto(total_commits: usize) -> BucketSize {
    match total_commits {
        0..=499 => BucketSize::Commit,
        500..=4_999 => BucketSize::Day,
        5_000..=49_999 => BucketSize::Week,
        _ => BucketSize::Month,
    }
}

/// Compute the bucket key for a commit timestamp.
///
/// Returns an integer suitable for grouping/ordering:
/// - Commit: unix seconds (each commit its own bucket)
/// - Day:    YYYYMMDD
/// - Week:   YYYYWW (ISO week)
/// - Month:  YYYYMM
/// - Tag:    panics — callers must use `tag_bucket_key` with the tag list.
pub fn bucket_key(ts: OffsetDateTime, size: BucketSize) -> i64 {
    match size {
        BucketSize::Commit => ts.unix_timestamp(),
        BucketSize::Day => {
            let d = ts.date();
            (d.year() as i64) * 10_000
                + (u8::from(d.month()) as i64) * 100
                + (d.day() as i64)
        }
        BucketSize::Week => {
            let (iso_year, iso_week, _) = ts.date().to_iso_week_date();
            (iso_year as i64) * 100 + (iso_week as i64)
        }
        BucketSize::Month => {
            (ts.year() as i64) * 100 + (u8::from(ts.month()) as i64)
        }
        BucketSize::Tag => {
            unreachable!("BucketSize::Tag requires tag_bucket_key(ts, tag_dates)")
        }
    }
}

/// Bucket key for a `Tag` bucket. `tag_dates_sorted` is ascending tagger
/// unix-seconds.
///
/// Returns the 0-based index of the latest tag whose tagged_at ≤ ts. Any
/// commit older than the earliest tag falls into bucket `-1` (a synthetic
/// "pre-history" bucket that queries can filter out or render as "untagged").
pub fn tag_bucket_key(ts: OffsetDateTime, tag_dates_sorted: &[i64]) -> i64 {
    let ts_sec = ts.unix_timestamp();
    let idx = tag_dates_sorted.partition_point(|&t| t <= ts_sec);
    (idx as i64) - 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn commit_bucket_is_unix_seconds() {
        let ts = datetime!(2025-03-15 12:00:00 UTC);
        assert_eq!(
            bucket_key(ts, BucketSize::Commit),
            ts.unix_timestamp()
        );
    }

    #[test]
    fn day_bucket_is_yyyymmdd() {
        let ts = datetime!(2025-03-15 23:59:59 UTC);
        assert_eq!(bucket_key(ts, BucketSize::Day), 20_250_315);
    }

    #[test]
    fn month_bucket_is_yyyymm() {
        let ts = datetime!(2025-03-15 00:00:00 UTC);
        assert_eq!(bucket_key(ts, BucketSize::Month), 202_503);
    }

    #[test]
    fn week_bucket_uses_iso_year_for_dec_jan_wrap() {
        // 2025-12-29 is Monday, ISO week 2026-W01. Calendar year 2025 would
        // sort wrong across the Dec/Jan boundary.
        let ts = datetime!(2025-12-29 12:00:00 UTC);
        assert_eq!(bucket_key(ts, BucketSize::Week), 2026 * 100 + 1);

        // 2024-12-31 is Tuesday of ISO 2025-W01.
        let ts = datetime!(2024-12-31 12:00:00 UTC);
        assert_eq!(bucket_key(ts, BucketSize::Week), 2025 * 100 + 1);
    }

    #[test]
    fn week_bucket_mid_year() {
        // 2025-03-15 is Saturday of ISO 2025-W11.
        let ts = datetime!(2025-03-15 12:00:00 UTC);
        assert_eq!(bucket_key(ts, BucketSize::Week), 2025 * 100 + 11);
    }

    #[test]
    fn auto_thresholds() {
        assert_eq!(auto(0), BucketSize::Commit);
        assert_eq!(auto(499), BucketSize::Commit);
        assert_eq!(auto(500), BucketSize::Day);
        assert_eq!(auto(4_999), BucketSize::Day);
        assert_eq!(auto(5_000), BucketSize::Week);
        assert_eq!(auto(49_999), BucketSize::Week);
        assert_eq!(auto(50_000), BucketSize::Month);
    }

    #[test]
    fn parse_size() {
        assert_eq!(BucketSize::parse("Day"), Some(BucketSize::Day));
        assert_eq!(BucketSize::parse("week"), Some(BucketSize::Week));
        assert_eq!(BucketSize::parse("COMMIT"), Some(BucketSize::Commit));
        assert_eq!(BucketSize::parse("month"), Some(BucketSize::Month));
        assert_eq!(BucketSize::parse("tag"), Some(BucketSize::Tag));
        assert_eq!(BucketSize::parse("year"), None);
    }

    #[test]
    fn tag_bucket_before_first_tag_is_neg_one() {
        // Tags at t=100, 200, 300; commit at t=50 → -1.
        let ts = OffsetDateTime::from_unix_timestamp(50).unwrap();
        assert_eq!(tag_bucket_key(ts, &[100, 200, 300]), -1);
    }

    #[test]
    fn tag_bucket_at_and_after_tags() {
        // Commit at t=100 (== first tag) → bucket 0.
        // Commit at t=150 (between tag 0 and 1) → bucket 0.
        // Commit at t=200 (== tag 1) → bucket 1.
        // Commit at t=350 (after last tag) → bucket 2.
        let tags = &[100i64, 200, 300];
        let mk = |s: i64| OffsetDateTime::from_unix_timestamp(s).unwrap();
        assert_eq!(tag_bucket_key(mk(100), tags), 0);
        assert_eq!(tag_bucket_key(mk(150), tags), 0);
        assert_eq!(tag_bucket_key(mk(200), tags), 1);
        assert_eq!(tag_bucket_key(mk(350), tags), 2);
    }

    #[test]
    fn tag_bucket_empty_tag_list_all_neg_one() {
        let ts = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        assert_eq!(tag_bucket_key(ts, &[]), -1);
    }
}
