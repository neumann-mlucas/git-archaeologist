use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum BucketSize {
    Commit,
    Day,
    Week,
    Month,
}

impl BucketSize {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "commit" => Some(Self::Commit),
            "day" => Some(Self::Day),
            "week" => Some(Self::Week),
            "month" => Some(Self::Month),
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
            // Use ISO year (not calendar year) so keys sort correctly across
            // Dec/Jan boundaries where the ISO week rolls into the next year.
            let (iso_year, iso_week, _) = ts.date().to_iso_week_date();
            (iso_year as i64) * 100 + (iso_week as i64)
        }
        BucketSize::Month => {
            (ts.year() as i64) * 100 + (u8::from(ts.month()) as i64)
        }
    }
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
        assert_eq!(BucketSize::parse("year"), None);
    }
}
